use anyhow::Result;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};

fn restart(root: &Path, args: &[&str], fail: &str) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_mysync"))
        .args(args)
        .env("PATH", root.join("bin"))
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("MYSYNC_TEST_SYSTEMCTL_LOG", root.join("calls"))
        .env("MYSYNC_TEST_SYSTEMCTL_FAIL", fail)
        .output()?)
}

fn systemctl(root: &Path) -> Result<()> {
    fs::create_dir(root.join("bin"))?;
    let path = root.join("bin/systemctl");
    fs::write(
        &path,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$MYSYNC_TEST_SYSTEMCTL_LOG"
if [ "$2" = "$MYSYNC_TEST_SYSTEMCTL_FAIL" ]; then exit 1; fi
"#,
    )?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[test]
fn restart_reloads_user_units_before_restarting_without_a_selected_profile() -> Result<()> {
    let temp = tempfile::tempdir()?;
    systemctl(temp.path())?;
    let output = restart(temp.path(), &["restart"], "")?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(temp.path().join("calls"))?,
        "--user daemon-reload\n--user restart mysync.service\n"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Restarted mysync.service"));
    assert!(!temp.path().join("config").exists());
    Ok(())
}

#[test]
fn restart_reports_failures_and_stops_after_a_failed_reload() -> Result<()> {
    for failed in ["daemon-reload", "restart"] {
        let temp = tempfile::tempdir()?;
        systemctl(temp.path())?;
        let output = restart(temp.path(), &["restart"], failed)?;
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(&format!("systemctl --user {failed} failed")),
            "{error}"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Restarted"));
        let expected = if failed == "daemon-reload" {
            "--user daemon-reload\n"
        } else {
            "--user daemon-reload\n--user restart mysync.service\n"
        };
        assert_eq!(fs::read_to_string(temp.path().join("calls"))?, expected);
    }
    Ok(())
}

#[test]
fn restart_explains_manual_daemons_and_rejects_profile_selectors() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let output = restart(temp.path(), &["restart"], "")?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Ctrl+C"));
    systemctl(temp.path())?;
    for selector in ["--profile", "--config"] {
        let output = restart(temp.path(), &[selector, "example", "restart"], "")?;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("all profiles"));
        assert!(!temp.path().join("calls").exists());
    }
    Ok(())
}
