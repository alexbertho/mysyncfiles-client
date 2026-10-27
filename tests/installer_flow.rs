use anyhow::Result;
use mysyncfiles_client::release;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::Mutex,
};

// A concurrent fork can inherit another test's writable executable descriptor
// until exec closes it, causing ETXTBSY even after fs::write has returned.
// Serialize executable writes and process creation, but not child execution.
static EXECUTABLE_IO: Mutex<()> = Mutex::new(());

struct InstallerFixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    releases: PathBuf,
    home: PathBuf,
    script: PathBuf,
    mocks: PathBuf,
}

fn safe_directory(path: impl AsRef<Path>) -> Result<()> {
    fs::create_dir_all(&path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn executable(path: &Path, bytes: &str) -> Result<()> {
    let _guard = EXECUTABLE_IO.lock().unwrap();
    fs::write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn spawn_command(command: &mut Command) -> Result<Child> {
    let _guard = EXECUTABLE_IO.lock().unwrap();
    Ok(command.spawn()?)
}

fn command_output(command: &mut Command) -> Result<Output> {
    let child = spawn_command(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )?;
    Ok(child.wait_with_output()?)
}

impl InstallerFixture {
    fn new(doctor_ok: bool) -> Result<Self> {
        Self::new_with_setup(doctor_ok, true)
    }

    fn new_with_setup(doctor_ok: bool, setup_ok: bool) -> Result<Self> {
        Self::new_with_capabilities(doctor_ok, setup_ok, true)
    }

    fn new_with_capabilities(doctor_ok: bool, setup_ok: bool, web_status: bool) -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().to_path_buf();
        let home = root.join("home");
        let mocks = root.join("mockbin");
        fs::create_dir(&home)?;
        fs::create_dir(&mocks)?;
        let key = root.join("signing.key");
        let public = release::generate_key(&key)?;
        let binary = root.join("source-client");
        executable(
            &binary,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'mysync 0.3.0';;\n  doctor) if [ \"${{MYSYNC_TEST_REQUIRE_EK_CERT:-}}\" = 1 ] && [ \"$2\" != --ek-cert ]; then echo 'EK cert missing' >&2; exit 1; fi; {};;\n  setup) if [ \"$2\" = --help ]; then {}; fi; test -t 0 || exit 23; printf '%s\\n' \"$*\" > \"$MYSYNC_TEST_SETUP_CALLED\"; exit \"${{MYSYNC_TEST_SETUP_EXIT:-0}}\";;\n  *) exit 1;;\nesac\n",
                if doctor_ok {
                    r#"if [ -e "$MYSYNC_TEST_ROOT/needs-ek" ]; then
    if [ "${2:-}" != --ek-cert ]; then
        echo 'TPM 2.0 is reachable, but no readable RSA-2048 or P-256 EK certificate was found' >&2
        exit 1
    fi
    [ "$(cat "$3")" = valid-test-certificate ] || { echo 'EK certificate public key does not match this TPM' >&2; exit 1; }
fi
echo 'TPM 2.0 is ready.'"#
                } else {
                    "echo 'TPM unavailable' >&2; exit 1"
                },
                if setup_ok {
                    if web_status {
                        "echo '--web-status <true|false>'; exit 0"
                    } else {
                        "exit 0"
                    }
                } else {
                    "case \"$0\" in */source-client) exit 0;; *) exit 2;; esac"
                }
            ),
        )?;
        let releases_root = root.join("releases");
        let target = release::current_target().unwrap();
        {
            // Publication spawns the client for both compatibility probes.
            let _guard = EXECUTABLE_IO.lock().unwrap();
            release::publish(&key, &binary, "0.3.0", target, &releases_root)?;
        }
        let releases = releases_root.join(target);
        executable(
            &mocks.join("curl"),
            r##"#!/bin/sh
destination= url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) destination=$2; shift 2 ;;
        --write-out|--proto|--proto-redir|--max-redirs|--connect-timeout|--max-time|--max-filesize) shift 2 ;;
        --fail|--silent|--show-error|--location) shift ;;
        *) url=$1; shift ;;
    esac
done
case "$url" in
    https://github.com/alexbertho/mysyncfiles-client/releases/latest/download/*) ;;
    *) echo 'Client releases must come from the client repository' >&2; exit 1 ;;
esac
file=${url##*/}
if [ "${MYSYNC_TEST_HTTP_STATUS:-200}" != 200 ] && {
    [ "$file" = latest-linux-x86_64.signed.json ] || [ "$file" = latest-linux-aarch64.signed.json ] || [ "$file" = latest.json ] || [ "$file" = latest.sig ];
}; then
    printf '%s' "$MYSYNC_TEST_HTTP_STATUS"
    exit "${MYSYNC_TEST_CURL_EXIT:-0}"
fi
if [ ! -e "$MYSYNC_TEST_RELEASES/$file" ]; then
    printf 404
    exit 0
fi
cp "$MYSYNC_TEST_RELEASES/$file" "$destination" || exit 22
printf 200
"##,
        )?;
        executable(&mocks.join("tpm2_getcap"), "#!/bin/sh\nexit 0\n")?;
        executable(
            &mocks.join("systemctl"),
            "#!/bin/sh\nif [ \"$2\" = is-active ]; then exit 3; fi\n: > \"$MYSYNC_TEST_SYSTEMCTL_CALLED\"\nexit \"${MYSYNC_TEST_SYSTEMCTL_STATUS:-99}\"\n",
        )?;
        let template = include_str!("../deploy/install.sh");
        let script = root.join("install.sh");
        fs::write(
            &script,
            template
                .replace(
                    "SERVER_URL=${MYSYNC_SERVER_URL:-}",
                    "SERVER_URL=https://sync.example.test\nMYSYNC_PROFILE=default",
                )
                .replace(release::PUBLIC_KEY_HEX.trim(), &public)
                .replace(
                    "@MYSYNC_UNIT@",
                    include_str!("../deploy/mysync.service").trim_end_matches('\n'),
                ),
        )?;
        Ok(Self {
            _temp: temp,
            root,
            releases,
            home,
            script,
            mocks,
        })
    }

    fn run(&self, status: &str) -> Result<Output> {
        self.run_with_cert(status, None)
    }

    fn command(&self, program: &str) -> Result<Command> {
        let limited_path = self.root.join("limited-path");
        let path = if limited_path.is_dir() {
            limited_path.display().to_string()
        } else {
            format!("{}:{}", self.mocks.display(), std::env::var("PATH")?)
        };
        let mut command = Command::new(program);
        command
            .env("HOME", &self.home)
            // Keep profiles and units private even when the runner exports XDG_CONFIG_HOME.
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("SHELL", "/bin/sh")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env_remove("MYSYNC_WEB_STATUS")
            .env_remove("MYSYNC_EK_CERT")
            .env("MYSYNC_TEST_ROOT", &self.root)
            .env(
                "MYSYNC_TEST_INSTALL_BIN",
                if limited_path.is_dir() {
                    &limited_path
                } else {
                    &self.mocks
                },
            )
            .env("MYSYNC_TEST_RELEASES", &self.releases)
            .env("MYSYNC_TEST_HTTP_STATUS", "200")
            .env("MYSYNC_TEST_CURL_EXIT", "0")
            .env("MYSYNC_TEST_SETUP_CALLED", self.root.join("setup-called"))
            .env(
                "MYSYNC_TEST_SYSTEMCTL_CALLED",
                self.root.join("systemctl-called"),
            )
            .env("PATH", path);
        Ok(command)
    }

    fn without_tools(&self, missing: &[&str]) -> Result<()> {
        let destination = self.root.join("limited-path");
        let mut commands = Vec::new();
        for tool in [
            "cat",
            "chmod",
            "cmp",
            "cp",
            "curl",
            "flock",
            "grep",
            "id",
            "install",
            "ln",
            "mkdir",
            "mktemp",
            "mv",
            "openssl",
            "python3",
            "rm",
            "script",
            "sed",
            "sh",
            "stat",
            "sudo",
            "systemctl",
            "tpm2_createek",
            "tpm2_getcap",
            "tpm2_getekcertificate",
            "uname",
        ] {
            if missing.contains(&tool) {
                continue;
            }
            let output =
                command_output(
                    self.command("sh")?
                        .args(["-c", "command -v \"$1\"", "sh", tool]),
                )?;
            if output.status.success() {
                commands.push((tool, String::from_utf8(output.stdout)?.trim().to_owned()));
            }
        }
        fs::create_dir(&destination)?;
        for (tool, source) in commands {
            std::os::unix::fs::symlink(source, destination.join(tool))?;
        }
        Ok(())
    }

    fn cachyos(&self) -> Result<()> {
        let path = self.root.join("os-release");
        fs::write(&path, "ID=cachyos\nID_LIKE=arch\nPRETTY_NAME=CachyOS\n")?;
        fs::write(
            &self.script,
            fs::read_to_string(&self.script)?.replace("/etc/os-release", path.to_str().unwrap()),
        )?;
        Ok(())
    }

    fn run_with_cert(&self, status: &str, certificate: Option<&Path>) -> Result<Output> {
        let mut command = self.command("sh")?;
        command.arg(&self.script);
        if let Some(certificate) = certificate {
            command.env("MYSYNC_EK_CERT", certificate);
            command.env("MYSYNC_TEST_REQUIRE_EK_CERT", "1");
        }
        command_output(command.env("MYSYNC_TEST_HTTP_STATUS", status))
    }

    fn run_interactive(&self, setup_exit: &str, piped: bool) -> Result<Output> {
        self.run_interactive_with_input(setup_exit, piped, b"\n\n")
    }

    fn run_interactive_with_input(
        &self,
        setup_exit: &str,
        piped: bool,
        input: &[u8],
    ) -> Result<Output> {
        self.run_interactive_with_setting(setup_exit, piped, input, None)
    }

    fn run_interactive_with_setting(
        &self,
        setup_exit: &str,
        piped: bool,
        input: &[u8],
        setting: Option<&str>,
    ) -> Result<Output> {
        let command = if piped {
            format!("cat {} | sh", self.script.display())
        } else {
            format!("sh {}", self.script.display())
        };
        let mut process = self.command("script")?;
        if let Some(setting) = setting {
            process.env("MYSYNC_WEB_STATUS", setting);
        }
        let mut child = spawn_command(
            process
                .args(["-q", "-e", "-c"])
                .arg(command)
                .arg("/dev/null")
                .env("MYSYNC_TEST_SETUP_EXIT", setup_exit)
                .env("MYSYNC_SERVER_PUBLIC_KEY", "test-origin-public-key")
                .env("MYSYNC_TEST_SYSTEMCTL_STATUS", "0")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )?;
        child.stdin.take().unwrap().write_all(input)?;
        Ok(child.wait_with_output()?)
    }

    fn assert_not_installed(&self) {
        assert!(!self.home.join(".local/bin/mysync").exists());
        assert!(!self.root.join("systemctl-called").exists());
    }
}

fn mock_inaccessible_tss_device(fixture: &InstallerFixture) -> Result<()> {
    let script = fs::read_to_string(&fixture.script)?;
    let guard = "if [ -e /dev/tpmrm0 ] && { [ ! -r /dev/tpmrm0 ] || [ ! -w /dev/tpmrm0 ]; }; then";
    assert!(script.contains(guard));
    // Force only the device access predicate; keep the installer decision path intact.
    fs::write(&fixture.script, script.replacen(guard, "if true; then", 1))?;
    executable(
        &fixture.mocks.join("stat"),
        "#!/bin/sh\ncase \"$2\" in %G) echo tss;; %A) echo crw-rw----;; *) exit 1;; esac\n",
    )?;
    executable(
        &fixture.mocks.join("id"),
        "#!/bin/sh\ncase \"$1\" in\n  -un) echo test-user;;\n  -nG) if [ \"$#\" -eq 1 ]; then echo 'test-user users'; elif [ \"${MYSYNC_TEST_TSS_ASSIGNED:-}\" = 1 ]; then echo 'test-user users tss'; else echo 'test-user users'; fi;;\nesac\n",
    )?;
    executable(
        &fixture.mocks.join("sudo"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$MYSYNC_TEST_SUDO_CALLED\"\n",
    )?;
    Ok(())
}

fn mock_ek_recovery(fixture: &InstallerFixture, vendor: &str) -> Result<()> {
    fs::write(fixture.root.join("needs-ek"), "")?;
    fs::write(
        fixture.root.join("tpm-properties"),
        format!("TPM2_PT_MANUFACTURER:\n  raw: {vendor}\n"),
    )?;
    executable(
        &fixture.mocks.join("tpm2_getcap"),
        r#"#!/bin/sh
[ "$*" = '-T device:/dev/tpmrm0 properties-fixed' ] || exit 1
cat "$MYSYNC_TEST_ROOT/tpm-properties"
"#,
    )?;
    executable(
        &fixture.mocks.join("tpm2_createek"),
        r#"#!/bin/sh
[ "$#" = 8 ] && [ "$1" = -T ] && [ "$2" = device:/dev/tpmrm0 ] && [ "$3" = -G ] && [ "$5" = -u ] && [ "$7" = -c ] || exit 1
printf '%s\n' "$4" > "$6"
: > "$8"
"#,
    )?;
    executable(
        &fixture.mocks.join("tpm2_getekcertificate"),
        r#"#!/bin/sh
# Exact options exclude TLS bypass, arbitrary hosts and TPM hierarchy changes.
[ "$#" = 6 ] && [ "$1" = -T ] && [ "$2" = device:/dev/tpmrm0 ] && [ "$3" = -u ] && [ "$5" = -o ] || exit 1
printf '%s\n' "$(cat "$4")" >> "$MYSYNC_TEST_ROOT/ek-downloads"
[ ! -e "$MYSYNC_TEST_ROOT/download-fails" ] || exit 1
if [ -e "$MYSYNC_TEST_ROOT/rsa-fails" ] && [ "$(cat "$4")" = rsa ]; then exit 1; fi
if [ -e "$MYSYNC_TEST_ROOT/download-mismatch" ]; then
    printf wrong-certificate > "$6"
elif [ -e "$MYSYNC_TEST_ROOT/download-oversized" ]; then
    python3 -c 'import pathlib, sys; pathlib.Path(sys.argv[1]).write_bytes(b"x" * 20000)' "$6"
else
    printf valid-test-certificate > "$6"
fi
"#,
    )?;
    Ok(())
}

#[test]
fn installer_recovers_and_reuses_an_amd_ek_certificate() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    fixture.cachyos()?;
    mock_ek_recovery(&fixture, "0x414D4400")?;
    let output = fixture.run_interactive_with_input("0", false, b"y\n\n\n")?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let transcript = String::from_utf8_lossy(&output.stdout);
    assert!(transcript.contains("TPM manufacturer: AMD"), "{transcript}");
    assert!(
        transcript.contains("identifier derived from this TPM"),
        "{transcript}"
    );
    let cache = fixture.home.join(".config/mysync/ek.der");
    assert_eq!(fs::read_to_string(&cache)?, "valid-test-certificate");
    assert_eq!(fs::metadata(&cache)?.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(cache.parent().unwrap())?.permissions().mode() & 0o777,
        0o700
    );
    assert!(
        fs::read_to_string(fixture.root.join("setup-called"))?
            .contains(&format!("--ek-cert {}", cache.display()))
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("ek-downloads"))?,
        "rsa\n"
    );
    let output = fixture.run("200")?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Saved TPM certificate verified"));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("--ek-cert {}", cache.display()))
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("ek-downloads"))?,
        "rsa\n"
    );
    Ok(())
}

#[test]
fn installer_recovers_an_intel_ek_and_falls_back_to_ecc() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    mock_ek_recovery(&fixture, "0x494E5443")?;
    fs::write(fixture.root.join("rsa-fails"), "")?;
    let output = fixture.run_interactive_with_input("0", false, b"y\n\n\n")?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("TPM manufacturer: Intel"));
    assert_eq!(
        fs::read_to_string(fixture.root.join("ek-downloads"))?,
        "rsa\necc\n"
    );
    Ok(())
}

#[test]
fn installer_never_downloads_a_certificate_without_consent() -> Result<()> {
    for interactive in [false, true] {
        let fixture = InstallerFixture::new(true)?;
        mock_ek_recovery(&fixture, "0x414D4400")?;
        let output = if interactive {
            fixture.run_interactive_with_input("0", false, b"n\n")?
        } else {
            fixture.run("200")?
        };
        assert!(!output.status.success());
        assert!(!fixture.root.join("ek-downloads").exists());
        assert!(!fixture.home.join(".config/mysync/ek.der").exists());
        fixture.assert_not_installed();
    }
    Ok(())
}

#[test]
fn installer_preserves_a_saved_certificate_after_failed_recovery() -> Result<()> {
    for failure in ["download-fails", "download-mismatch", "download-oversized"] {
        let fixture = InstallerFixture::new(true)?;
        mock_ek_recovery(&fixture, "0x414D4400")?;
        safe_directory(fixture.home.join(".config/mysync"))?;
        let cache = fixture.home.join(".config/mysync/ek.der");
        fs::write(&cache, "old-certificate")?;
        fs::write(fixture.root.join(failure), "")?;
        let output = fixture.run_interactive_with_input("0", false, b"y\n")?;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("Could not retrieve a certificate matching this TPM")
        );
        assert_eq!(fs::read_to_string(cache)?, "old-certificate");
        fixture.assert_not_installed();
    }
    Ok(())
}

#[test]
fn installer_guides_other_manufacturers_and_rejects_symlink_caches() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    mock_ek_recovery(&fixture, "0x49465800")?;
    let output = fixture.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("MYSYNC_EK_CERT=/path/to/ek.der"));
    assert!(!fixture.root.join("ek-downloads").exists());
    fixture.assert_not_installed();

    let fixture = InstallerFixture::new(true)?;
    mock_ek_recovery(&fixture, "0x414D4400")?;
    safe_directory(fixture.home.join(".config/mysync"))?;
    let outside = fixture.root.join("outside-certificate");
    fs::write(&outside, "keep")?;
    std::os::unix::fs::symlink(&outside, fixture.home.join(".config/mysync/ek.der"))?;
    let output = fixture.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe saved TPM certificate"));
    assert_eq!(fs::read_to_string(outside)?, "keep");
    fixture.assert_not_installed();
    Ok(())
}

#[test]
fn installer_offers_missing_tpm_tools_on_cachyos_and_resumes() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    fixture.cachyos()?;
    mock_ek_recovery(&fixture, "0x414D4400")?;
    let templates = fixture.root.join("tool-templates");
    fs::create_dir(&templates)?;
    for tool in ["tpm2_getcap", "tpm2_createek", "tpm2_getekcertificate"] {
        fs::copy(fixture.mocks.join(tool), templates.join(tool))?;
    }
    executable(
        &fixture.mocks.join("sudo"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$MYSYNC_TEST_ROOT/package-installs"
[ "$*" = 'pacman -S --needed tpm2-tools' ] || exit 1
cp "$MYSYNC_TEST_ROOT/tool-templates/"* "$MYSYNC_TEST_INSTALL_BIN/"
"#,
    )?;
    fixture.without_tools(&["tpm2_getcap", "tpm2_createek", "tpm2_getekcertificate"])?;
    let output = fixture.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("sudo pacman -S --needed tpm2-tools"));
    assert!(!fixture.root.join("package-installs").exists());
    let output = fixture.run_interactive_with_input("0", false, b"n\n")?;
    assert!(!output.status.success());
    assert!(!fixture.root.join("package-installs").exists());
    assert!(!fixture.root.join("ek-downloads").exists());
    fixture.assert_not_installed();
    let output = fixture.run_interactive_with_input("0", false, b"y\ny\n\n\n")?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("package-installs"))?,
        "pacman -S --needed tpm2-tools\n"
    );
    assert!(fixture.root.join("setup-called").exists());
    Ok(())
}

#[test]
fn installer_detects_missing_base_tools_before_downloading() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    fixture.cachyos()?;
    fixture.without_tools(&["python3", "curl", "flock"])?;
    let output = fixture.run("200")?;
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("sudo pacman -S --needed curl python util-linux"),
        "{error}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("Downloading signed release metadata")
    );
    fixture.assert_not_installed();
    Ok(())
}

#[test]
fn installer_guides_tss_membership_without_an_interactive_terminal() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    mock_inaccessible_tss_device(&fixture)?;
    let output = fixture.run("200")?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("sudo usermod -aG tss test-user"),
        "{stderr}"
    );
    assert!(!fixture.root.join("sudo-called").exists());
    fixture.assert_not_installed();

    let output = command_output(
        fixture
            .command("sh")?
            .arg(&fixture.script)
            .env("MYSYNC_TEST_TSS_ASSIGNED", "1"),
    )?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("sign out and back in"));
    assert!(!fixture.root.join("sudo-called").exists());
    Ok(())
}

#[test]
fn installer_offers_tss_group_update_in_a_terminal() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    mock_inaccessible_tss_device(&fixture)?;
    let mut child = spawn_command(
        fixture
            .command("script")?
            .args(["-q", "-e", "-c"])
            .arg(format!("sh {}", fixture.script.display()))
            .arg("/dev/null")
            .env("MYSYNC_TEST_SUDO_CALLED", fixture.root.join("sudo-called"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )?;
    child.stdin.take().unwrap().write_all(b"y\n")?;
    let output = child.wait_with_output()?;
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Add test-user to the tss group with sudo?"),
        "{stdout}"
    );
    assert!(stdout.contains("Sign out and back in"), "{stdout}");
    assert_eq!(
        fs::read_to_string(fixture.root.join("sudo-called"))?,
        "usermod -aG tss test-user\n"
    );
    fixture.assert_not_installed();
    Ok(())
}

#[test]
fn signed_installer_accepts_explicit_ek_certificate() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let certificate = fixture.root.join("ek certificate.der");
    fs::write(&certificate, b"test certificate path")?;
    let output = fixture.run_with_cert("200", Some(&certificate))?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.home.join(".local/bin/mysync").exists());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("--ek-cert '{}'", certificate.display()))
    );
    Ok(())
}

#[test]
fn installer_rejects_a_signed_client_without_setup() -> Result<()> {
    let fixture = InstallerFixture::new_with_setup(true, false)?;
    let output = fixture.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("lacks the setup command"));
    fixture.assert_not_installed();
    Ok(())
}

#[test]
fn installer_backs_up_an_older_or_different_same_version_client() -> Result<()> {
    for previous_version in ["0.2.9", "0.3.0"] {
        let fixture = InstallerFixture::new(true)?;
        let bin_dir = fixture.home.join(".local/bin");
        safe_directory(&bin_dir)?;
        let old_binary = format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'mysync {previous_version}'; else exit 2; fi\n"
        );
        executable(&bin_dir.join("mysync"), &old_binary)?;

        let output = fixture.run("200")?;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Back up"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(bin_dir.join("mysync"))?, old_binary);

        let output = fixture.run_interactive_with_input("0", false, b"y\n\n\n")?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(
            fs::read(bin_dir.join("mysync"))?,
            fs::read(fixture.root.join("source-client"))?
        );
        let backups: Vec<_> = fs::read_dir(&bin_dir)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("mysync.backup.")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].join("mysync"))?, old_binary);
        assert!(fixture.root.join("setup-called").exists());
    }
    Ok(())
}

#[test]
fn installer_archives_a_legacy_token_profile_before_pairing() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let config_dir = fixture.home.join(".config/mysync");
    let mirror = fixture.home.join("Sync");
    safe_directory(&config_dir)?;
    safe_directory(&mirror)?;
    fs::write(mirror.join("local.txt"), "local content")?;
    let profile = serde_json::to_vec(&serde_json::json!({
        "server": "https://sync.example.test",
        "root": mirror,
        "token": "obsolete-secret"
    }))?;
    fs::write(config_dir.join("config.json"), &profile)?;
    fs::write(config_dir.join("config.state.json"), b"old state")?;
    fs::write(config_dir.join("config.state.journal"), b"old journal")?;

    let output = fixture.run_interactive_with_input("0", true, b"y\n\n\n")?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!config_dir.join("config.json").exists());
    assert!(!config_dir.join("config.state.json").exists());
    assert!(!config_dir.join("config.state.journal").exists());
    let backups: Vec<_> = fs::read_dir(&config_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("legacy-profile.")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(backups[0].join("config.json"))?, profile);
    assert_eq!(
        fs::read(backups[0].join("config.state.json"))?,
        b"old state"
    );
    assert_eq!(
        fs::read(backups[0].join("config.state.journal"))?,
        b"old journal"
    );
    assert_eq!(
        fs::read_to_string(mirror.join("local.txt"))?,
        "local content"
    );
    assert!(fixture.root.join("setup-called").exists());
    assert!(fixture.root.join("systemctl-called").exists());
    Ok(())
}

#[test]
fn installer_leaves_legacy_profile_untouched_without_consent() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let config_dir = fixture.home.join(".config/mysync");
    safe_directory(&config_dir)?;
    fs::write(config_dir.join("config.json"), b"{\"token\":\"old\"}")?;
    fs::write(config_dir.join("config.state.json"), b"old state")?;

    let output = fixture.run_interactive_with_input("0", true, b"\n")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("legacy token profile"));
    assert_eq!(
        fs::read(config_dir.join("config.json"))?,
        b"{\"token\":\"old\"}"
    );
    assert_eq!(
        fs::read(config_dir.join("config.state.json"))?,
        b"old state"
    );
    assert!(!fixture.root.join("setup-called").exists());
    assert!(!fixture.root.join("systemctl-called").exists());

    let output = fixture.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Rerun this installer in a terminal"));
    assert_eq!(
        fs::read(config_dir.join("config.json"))?,
        b"{\"token\":\"old\"}"
    );
    Ok(())
}

#[test]
fn signed_installer_stages_unit_but_does_not_start_service() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let output = fixture.run("200")?;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.home.join(".local/bin/mysync").exists());
    assert_eq!(
        fs::read(fixture.home.join(".config/systemd/user/mysync.service"))?,
        include_bytes!("../deploy/mysync.service")
    );
    assert!(!fixture.root.join("systemctl-called").exists());
    assert!(!output.stdout.contains(&0x1b));
    assert!(String::from_utf8_lossy(&output.stdout).contains("mysync setup"));
    Ok(())
}

#[test]
fn interactive_installer_starts_service_only_after_setup_succeeds() -> Result<()> {
    let success = InstallerFixture::new(true)?;
    let output = success.run_interactive("0", false)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let setup = fs::read_to_string(success.root.join("setup-called"))?;
    assert!(setup.contains("setup --server https://sync.example.test"));
    assert!(setup.contains("--dir"));
    assert!(setup.contains("--web-status true"));
    assert!(setup.contains("--server-public-key test-origin-public-key"));
    assert!(success.root.join("systemctl-called").exists());

    let failed = InstallerFixture::new(true)?;
    let output = failed.run_interactive("2", false)?;
    assert!(!output.status.success());
    assert!(failed.root.join("setup-called").exists());
    assert!(!failed.root.join("systemctl-called").exists());

    let custom_unit = InstallerFixture::new(true)?;
    let unit_dir = custom_unit.home.join(".config/systemd/user");
    safe_directory(&unit_dir)?;
    fs::write(
        unit_dir.join("mysync.service"),
        "[Service]\nExecStart=/custom/client\n",
    )?;
    let output = custom_unit.run_interactive("0", false)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(custom_unit.root.join("setup-called").exists());
    assert!(!custom_unit.root.join("systemctl-called").exists());
    Ok(())
}

#[test]
fn installer_offers_web_status_and_preserves_saved_opt_out() -> Result<()> {
    for (input, setting) in [
        (b"\nn\n".as_slice(), None),
        (b"\n".as_slice(), Some("false")),
    ] {
        let fixture = InstallerFixture::new(true)?;
        let output = fixture.run_interactive_with_setting("0", true, input, setting)?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            fs::read_to_string(fixture.root.join("setup-called"))?.contains("--web-status false")
        );
        assert!(fixture.root.join("systemctl-called").exists());
    }
    // Both an active profile and a pairing awaiting approval keep their choice.
    for filename in ["config.json", "config.pairing.json"] {
        let fixture = InstallerFixture::new(true)?;
        let config_dir = fixture.home.join(".config/mysync");
        safe_directory(&config_dir)?;
        fs::write(config_dir.join(filename), b"{\"web_status_enabled\":false}")?;
        let output = fixture.run_interactive("0", false)?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("[y/N]"));
        assert!(
            fs::read_to_string(fixture.root.join("setup-called"))?.contains("--web-status false")
        );
    }
    Ok(())
}

#[test]
fn installer_checks_web_status_input_and_legacy_capability() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let output = command_output(
        fixture
            .command("sh")?
            .arg(&fixture.script)
            .env("MYSYNC_WEB_STATUS", "invalid"),
    )?;
    assert!(!output.status.success());
    fixture.assert_not_installed();

    let output = fixture.run_interactive_with_input("0", false, b"\ninvalid\nn\n")?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Please answer yes or no"));
    assert!(fs::read_to_string(fixture.root.join("setup-called"))?.contains("--web-status false"));

    let legacy = InstallerFixture::new_with_capabilities(true, true, false)?;
    let output = command_output(
        legacy
            .command("sh")?
            .arg(&legacy.script)
            .env("MYSYNC_WEB_STATUS", "false"),
    )?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("lacks --web-status"));
    legacy.assert_not_installed();
    let output = legacy.run_interactive_with_input("0", false, b"\n")?;
    assert!(output.status.success());
    assert!(!fs::read_to_string(legacy.root.join("setup-called"))?.contains("--web-status"));
    Ok(())
}

#[test]
fn noninteractive_installer_passes_web_status_to_the_next_setup_command() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let output = command_output(
        fixture
            .command("sh")?
            .arg(&fixture.script)
            .env("MYSYNC_WEB_STATUS", "false"),
    )?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--web-status false"));
    assert!(!fixture.root.join("setup-called").exists());
    assert!(!fixture.root.join("systemctl-called").exists());
    Ok(())
}

#[test]
fn source_installer_applies_web_status_before_starting_service() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    executable(&fixture.mocks.join("id"), "#!/bin/sh\necho test-user\n")?;
    let config = fixture.home.join(".config/mysync/config.json");
    safe_directory(config.parent().unwrap())?;
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "server": "https://sync.example.test", "root": fixture.home
        }))?,
    )?;
    executable(&fixture.mocks.join("loginctl"), "#!/bin/sh\necho yes\n")?;
    // A service start checks that the selected preference was already saved.
    executable(
        &fixture.mocks.join("systemctl"),
        r##"#!/bin/sh
python3 - "$XDG_CONFIG_HOME/mysync/config.json" "$MYSYNC_TEST_EXPECT_WEB_STATUS" <<'PY'
import json, sys
with open(sys.argv[1]) as source:
    assert json.load(source)['web_status_enabled'] == (sys.argv[2] == 'true')
PY
"##,
    )?;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("deploy/install-client.sh");
    for setting in ["false", "true"] {
        let output = command_output(
            fixture
                .command("bash")?
                .arg(&script)
                .arg(env!("CARGO_BIN_EXE_mysync"))
                .env("MYSYNC_WEB_STATUS", setting)
                .env("MYSYNC_TEST_EXPECT_WEB_STATUS", setting),
        )?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[test]
fn piped_installer_still_pairs_in_an_interactive_terminal() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let output = fixture.run_interactive("0", true)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(fixture.root.join("setup-called").exists());
    assert!(fixture.root.join("systemctl-called").exists());
    Ok(())
}

#[test]
fn signed_installer_rejects_bad_signature_binary_and_tpm() -> Result<()> {
    let invalid_signature = InstallerFixture::new(true)?;
    let path = invalid_signature.releases.join(format!(
        "latest-{}.signed.json",
        release::current_target().unwrap()
    ));
    let mut envelope: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
    envelope["signature"] = serde_json::Value::String("00".repeat(64));
    fs::write(path, serde_json::to_vec(&envelope)?)?;
    let output = invalid_signature.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("signature is invalid"));
    invalid_signature.assert_not_installed();

    let invalid_envelope = InstallerFixture::new(true)?;
    fs::write(
        invalid_envelope.releases.join(format!(
            "latest-{}.signed.json",
            release::current_target().unwrap()
        )),
        serde_json::json!({"manifest": "", "signature": "00".repeat(64)}).to_string(),
    )?;
    let output = invalid_envelope.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("signature is invalid"));
    invalid_envelope.assert_not_installed();

    let invalid_binary = InstallerFixture::new(true)?;
    let artifact = format!("mysync-0.3.0-{}", release::current_target().unwrap());
    fs::write(invalid_binary.releases.join(artifact), "tampered")?;
    let output = invalid_binary.run("200")?;
    assert!(!output.status.success());
    invalid_binary.assert_not_installed();

    let no_tpm = InstallerFixture::new(false)?;
    let output = no_tpm.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("TPM preflight failed"));
    no_tpm.assert_not_installed();
    Ok(())
}

#[test]
fn signed_installer_rejects_redirects_and_missing_releases() -> Result<()> {
    for (status, curl_exit) in [
        ("302", "0"),
        ("404", "0"),
        ("404", "22"),
        ("500", "22"),
        ("000", "7"),
    ] {
        let fixture = InstallerFixture::new(true)?;
        let output = command_output(
            fixture
                .command("sh")?
                .arg(&fixture.script)
                .env("MYSYNC_TEST_HTTP_STATUS", status)
                .env("MYSYNC_TEST_CURL_EXIT", curl_exit),
        )?;
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        if status == "404" {
            assert!(
                error.contains("Signed release metadata is unavailable"),
                "{error}"
            );
            assert!(error.contains("latest-"), "{error}");
            assert!(error.contains("docs/releases.md"), "{error}");
        } else if curl_exit != "0" {
            assert!(error.contains("Download failed:"), "{error}");
            assert!(!error.contains("repository maintainer"), "{error}");
        }
        fixture.assert_not_installed();
    }
    Ok(())
}

#[test]
fn signed_installer_does_not_replace_existing_client_or_follow_bin_symlink() -> Result<()> {
    let existing = InstallerFixture::new(true)?;
    safe_directory(existing.home.join(".local/bin"))?;
    let destination = existing.home.join(".local/bin/mysync");
    executable(&destination, "#!/bin/sh\necho 'mysync 9.0.0'\n")?;
    let output = existing.run("200")?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("refusing to downgrade"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination)?,
        "#!/bin/sh\necho 'mysync 9.0.0'\n"
    );

    let symlink = InstallerFixture::new(true)?;
    fs::create_dir(symlink.home.join(".local"))?;
    let outside = symlink.root.join("outside");
    fs::create_dir(&outside)?;
    std::os::unix::fs::symlink(&outside, symlink.home.join(".local/bin"))?;
    let output = symlink.run("200")?;
    assert!(!output.status.success());
    assert!(!outside.join("mysync").exists());
    Ok(())
}

#[test]
fn signed_installer_rejects_unsupported_architecture() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    executable(
        &fixture.mocks.join("uname"),
        "#!/bin/sh\ncase \"$1\" in -s) echo Linux;; -m) echo mips64;; esac\n",
    )?;
    let output = fixture.run("200")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unsupported CPU architecture"));
    fixture.assert_not_installed();
    Ok(())
}

#[test]
fn installer_selects_an_independent_profile_from_the_server_origin() -> Result<()> {
    let fixture = InstallerFixture::new(true)?;
    let script = fs::read_to_string(&fixture.script)?;
    fs::write(
        &fixture.script,
        script.replace("\nMYSYNC_PROFILE=default", ""),
    )?;
    let output = fixture.run_interactive("0", false)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let setup = fs::read_to_string(fixture.root.join("setup-called"))?;
    assert!(setup.contains("profiles/sync-example-test.json"), "{setup}");
    assert!(setup.contains("--server https://sync.example.test"));
    assert!(
        fs::read_to_string(fixture.home.join(".config/systemd/user/mysync.service"))?
            .contains("daemon --all")
    );
    Ok(())
}

#[test]
fn installer_rejects_invalid_origins_even_with_an_explicit_profile() -> Result<()> {
    for url in [
        "https://sync.example.test/path",
        "https://user:password@sync.example.test",
        "https://sync.example.test?query=1",
        "https://",
    ] {
        let fixture = InstallerFixture::new(true)?;
        let script = fs::read_to_string(&fixture.script)?;
        fs::write(
            &fixture.script,
            script.replace(
                "SERVER_URL=https://sync.example.test",
                &format!("SERVER_URL='{url}'"),
            ),
        )?;
        assert!(!fixture.run("200")?.status.success());
        fixture.assert_not_installed();
    }
    Ok(())
}
