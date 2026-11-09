use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use mysyncfiles_client::client::{self, Api, SyncReport};

#[derive(Parser)]
#[command(
    name = "mysync",
    version,
    about = "Synchronize a local folder with a personal server"
)]
struct Cli {
    /// Configuration file (default: XDG_CONFIG_HOME/mysync/config.json)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Named server connection, independent of all other profiles
    #[arg(long, global = true, conflicts_with = "config")]
    profile: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pair this client with an administrator, then verify the first sync
    Setup {
        /// TPM transport for machines with several TPMs (e.g. device:/dev/tpmrm1)
        #[arg(long)]
        tcti: Option<String>,
        #[arg(long)]
        server: String,
        /// Origin key obtained directly from the administrator (prompts if omitted)
        #[arg(long)]
        server_public_key: Option<String>,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        ek_chain: Option<PathBuf>,
        #[arg(long)]
        ek_cert: Option<PathBuf>,
        /// Enable browser status (new profiles default to true; existing choices are kept)
        #[arg(long, action = clap::ArgAction::Set)]
        web_status: Option<bool>,
        /// Allow or refuse all browser permissions, including local code execution
        #[arg(long, action = clap::ArgAction::Set, conflicts_with = "web_status")]
        web_permissions: Option<bool>,
    },
    /// Check TPM 2.0 access and an EK certificate without enrolling
    Doctor {
        /// TPM transport (defaults to the Linux resource manager)
        #[arg(long)]
        tcti: Option<String>,
        /// Manufacturer EK leaf certificate in DER when absent from TPM NV
        #[arg(long)]
        ek_cert: Option<PathBuf>,
    },
    /// Enroll a TPM identity; administrator approval is required before activation
    Enroll {
        /// TPM transport, saved with the identity for subsequent requests
        #[arg(long)]
        tcti: Option<String>,
        #[arg(long)]
        server: String,
        /// Origin key obtained directly from the administrator
        #[arg(long)]
        server_public_key: Option<String>,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        invitation_stdin: bool,
        /// PEM intermediate EK certificates, obtained from the TPM manufacturer
        #[arg(long)]
        ek_chain: Option<PathBuf>,
        /// Manufacturer EK leaf certificate in DER when absent from TPM NV
        #[arg(long)]
        ek_cert: Option<PathBuf>,
    },
    /// Activate an approved enrollment
    EnrollActivate,
    /// Pin an administrator-provided origin key for an existing profile
    TrustServer {
        #[arg(long)]
        public_key: String,
    },
    /// Show or change local browser status; restart a running daemon after changes
    WebStatus {
        #[command(subcommand)]
        action: Option<WebStatusAction>,
    },
    /// Allow or refuse all browser permissions for this profile; then run mysync restart
    WebPermissions {
        #[command(subcommand)]
        action: Option<WebStatusAction>,
    },
    /// Configure individual browser file permissions; then run mysync restart
    WebFiles {
        #[command(subcommand)]
        action: Option<WebFilesAction>,
    },
    /// Run one synchronization pass
    Sync,
    /// Watch local changes and poll the server continuously
    Daemon {
        /// Synchronize every registered server concurrently through one browser bridge
        #[arg(long)]
        all: bool,
    },
    /// Reload systemd user units and restart mysync.service for all profiles
    Restart,
    /// List local server profiles without connecting
    Profiles,
    /// Show local and server file counts
    Status,
    /// List recoverable server-side deletions
    Trash,
    /// Check for a signed client release and install it if newer
    Update,
    /// Restore an item from the server trash
    Restore { id: i64 },
}

#[derive(Subcommand)]
enum WebStatusAction {
    /// Enable this browser capability in the client profile
    Enable,
    /// Disable this browser capability in the client profile
    Disable,
    /// Show the saved setting without contacting the TPM or server
    Status,
}

#[derive(Subcommand)]
enum WebFilesAction {
    /// Allow read-only browser file sessions
    Enable,
    /// Refuse new browser file sessions, including uploads and directory management
    Disable,
    /// Allow browser uploads as well (requires read access to be enabled)
    EnableUpload,
    /// Refuse new browser upload authorizations
    DisableUpload,
    /// Allow browser directory renames and deletions (requires read access)
    EnableManage,
    /// Refuse new browser directory management authorizations
    DisableManage,
    /// Allow editing existing Python and C files (requires read access)
    EnableEdit,
    /// Refuse new browser editing authorizations
    DisableEdit,
    /// Allow isolated local code execution (requires read access)
    EnableRun,
    /// Refuse browser code execution after restarting the daemon
    DisableRun,
    /// Show saved browser permissions without contacting the TPM or server
    Status,
}

fn print_report(report: SyncReport) {
    println!(
        "uploaded={} downloaded={} deleted_local={} deleted_remote={} conflicts={}",
        report.uploaded,
        report.downloaded,
        report.deleted_local,
        report.deleted_remote,
        report.conflicts
    );
}

fn invitation_from_stdin() -> Result<String> {
    let mut value = String::new();
    std::io::stdin().read_line(&mut value)?;
    Ok(value.trim_end_matches(['\r', '\n']).to_owned())
}

fn server_key(config: &Path, supplied: Option<String>) -> Result<String> {
    if let Some(key) = supplied {
        return Ok(key);
    }
    if config.exists() {
        let config = client::load_config(config)?;
        if !config.server_public_key.is_empty() {
            return Ok(config.server_public_key);
        }
    }
    if !std::io::stdin().is_terminal() {
        bail!("--server-public-key is required; obtain it directly from the administrator");
    }
    println!(
        "On the server, run `make get` and copy the Server ID (public key) through a trusted channel."
    );
    println!(
        "An already open `make pair` also displays this key and can stay open for the next step."
    );
    print!("Server ID (public key) provided directly by the administrator: ");
    std::io::stdout().flush()?;
    let mut key = String::new();
    std::io::stdin().read_line(&mut key)?;
    Ok(key.trim().to_owned())
}

fn restart_hint() {
    println!("To apply to the installed user service, run: mysync restart");
    println!(
        "For a daemon started in a terminal, stop it with Ctrl+C and rerun its mysync daemon command."
    );
}

async fn restart_service() -> Result<()> {
    for action in ["daemon-reload", "restart"] {
        let mut command = tokio::process::Command::new("systemctl");
        command.args(["--user", action]);
        if action == "restart" {
            command.arg("mysync.service");
        }
        let status = command
            .status()
            .await
            .with_context(|| format!("cannot run systemctl --user {action}; for a terminal daemon, stop it with Ctrl+C and rerun its mysync daemon command"))?;
        ensure!(
            status.success(),
            "systemctl --user {action} failed ({status}); inspect `systemctl --user status mysync.service`; for a terminal daemon, stop it with Ctrl+C and rerun its mysync daemon command"
        );
    }
    println!("Restarted mysync.service for all registered profiles.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Command::Restart) {
        ensure!(
            cli.profile.is_none() && cli.config.is_none(),
            "mysync restart restarts the user service for all profiles; omit --profile and --config"
        );
        return restart_service().await;
    }
    if matches!(cli.command, Command::Profiles) {
        for (name, path) in client::profiles::list()? {
            let config = client::load_config(&path)?;
            println!("{name}\t{}\t{}", config.server, config.root.display());
        }
        return Ok(());
    }
    if matches!(cli.command, Command::Daemon { all: true }) {
        if cli.profile.is_some() || cli.config.is_some() {
            bail!("--all cannot be combined with --profile or --config");
        }
        return client::profiles::daemon_all().await;
    }
    if let Command::Doctor { ek_cert, tcti } = &cli.command {
        let certificate = ek_cert
            .as_deref()
            .map(mysyncfiles_client::tpm::read_ek_certificate)
            .transpose()?;
        let kind = mysyncfiles_client::tpm::doctor_with_certificate(
            &tcti
                .clone()
                .unwrap_or_else(mysyncfiles_client::tpm::default_tcti),
            certificate.as_deref(),
        )?;
        println!("TPM 2.0 is reachable and the {kind} EK matches its certificate.");
        println!("Manufacturer trust is checked during enrollment.");
        return Ok(());
    }
    if matches!(cli.command, Command::Update) {
        let key = if cli.profile.is_some() || cli.config.is_some() {
            let path = client::profiles::selected(cli.profile.as_deref(), cli.config)?;
            client::load_config(&path)?.update_public_key
        } else {
            mysyncfiles_client::release::PUBLIC_KEY_HEX
                .trim()
                .to_owned()
        };
        match mysyncfiles_client::update::check_and_install(&key).await? {
            mysyncfiles_client::update::UpdateOutcome::Current {
                installed_version,
                published_version,
            } => println!(
                "installed client: {installed_version}; latest signed release: {published_version}; no update installed"
            ),
            mysyncfiles_client::update::UpdateOutcome::Installed(version) => {
                println!("installed {version}; restart the running daemon to use this version");
                restart_hint();
            }
        }
        return Ok(());
    }
    let config_path = client::profiles::selected(cli.profile.as_deref(), cli.config)?;
    match cli.command {
        Command::Setup {
            tcti,
            server,
            server_public_key,
            dir,
            ek_chain,
            ek_cert,
            web_status,
            web_permissions,
        } => {
            let key = server_key(&config_path, server_public_key)?;
            let outcome = client::setup_with_options(
                &config_path,
                server,
                key,
                dir,
                client::SetupOptions {
                    ek_cert,
                    ek_chain,
                    web_status_enabled: web_status,
                    web_permissions_enabled: web_permissions,
                    tcti,
                },
            )
            .await?;
            let conflicts = outcome.conflicts.max(outcome.report.conflicts);
            print_report(outcome.report);
            if conflicts != 0 {
                bail!(
                    "{conflicts} conflict file(s) need review in .mysync-conflicts/; the service was not started"
                );
            }
            println!("Initial synchronization verified; the service can now be started.");
            restart_hint();
        }
        Command::Doctor { .. } => unreachable!(),
        Command::Enroll {
            tcti,
            server,
            server_public_key,
            dir,
            invitation_stdin,
            ek_chain,
            ek_cert,
        } => {
            let key = server_key(&config_path, server_public_key)?;
            let invitation = if invitation_stdin {
                invitation_from_stdin()?
            } else {
                rpassword::prompt_password("Enrollment invitation: ")?
            };
            let result = client::enroll_with_tcti(
                &config_path,
                server,
                key,
                dir,
                invitation,
                ek_cert,
                ek_chain,
                tcti.unwrap_or_else(mysyncfiles_client::tpm::default_tcti),
            )
            .await?;
            println!(
                "enrollment={}\nfingerprint={}\nstatus={}\nCompare this fingerprint with the administrator, then run mysync enroll-activate.",
                result.id, result.fingerprint, result.status
            );
        }
        Command::EnrollActivate => print_report(client::activate_enrollment(&config_path).await?),
        Command::TrustServer { public_key } => {
            client::trust_server(&config_path, &public_key).await?;
            println!("Server public key pinned in the client profile.");
            restart_hint();
        }
        Command::WebStatus { action } => {
            let enabled = match action {
                Some(WebStatusAction::Enable) => Some(true),
                Some(WebStatusAction::Disable) => Some(false),
                Some(WebStatusAction::Status) | None => None,
            };
            if let Some(enabled) = enabled {
                client::set_web_status(&config_path, enabled)?;
                println!("web_status_enabled={enabled}");
                restart_hint();
            } else {
                println!(
                    "web_status_enabled={}",
                    client::load_config(&config_path)?.web_status_enabled
                );
            }
        }
        Command::WebPermissions { action } => match action {
            Some(WebStatusAction::Enable | WebStatusAction::Disable) => {
                let enabled = matches!(action, Some(WebStatusAction::Enable));
                client::set_web_permissions(&config_path, enabled)?;
                println!("web_permissions_enabled={enabled}");
                println!("Existing browser file sessions expire within 30 minutes.");
                restart_hint();
            }
            Some(WebStatusAction::Status) | None => {
                let config = client::load_config(&config_path)?;
                let state = match config.web_permissions_enabled() {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "custom",
                };
                println!("web_permissions_enabled={state}");
            }
        },
        Command::WebFiles { action } => match action {
            Some(WebFilesAction::Enable | WebFilesAction::Disable) => {
                let enabled = matches!(action, Some(WebFilesAction::Enable));
                client::set_web_files(&config_path, enabled)?;
                println!(
                    "web_files_enabled={enabled}; existing browser sessions expire within 30 minutes"
                );
                restart_hint();
            }
            Some(WebFilesAction::EnableUpload | WebFilesAction::DisableUpload) => {
                let enabled = matches!(action, Some(WebFilesAction::EnableUpload));
                client::set_web_uploads(&config_path, enabled)?;
                println!(
                    "web_uploads_enabled={enabled}; existing browser sessions expire within 30 minutes"
                );
                restart_hint();
            }
            Some(WebFilesAction::EnableManage | WebFilesAction::DisableManage) => {
                let enabled = matches!(action, Some(WebFilesAction::EnableManage));
                client::set_web_management(&config_path, enabled)?;
                println!(
                    "web_management_enabled={enabled}; existing browser sessions expire within 30 minutes"
                );
                restart_hint();
            }
            Some(
                WebFilesAction::EnableEdit
                | WebFilesAction::DisableEdit
                | WebFilesAction::EnableRun
                | WebFilesAction::DisableRun,
            ) => {
                let execution = matches!(
                    action,
                    Some(WebFilesAction::EnableRun | WebFilesAction::DisableRun)
                );
                let enabled = matches!(
                    action,
                    Some(WebFilesAction::EnableEdit | WebFilesAction::EnableRun)
                );
                client::set_web_editor(&config_path, execution, enabled)?;
                println!(
                    "web_{}_enabled={enabled}",
                    if execution { "run" } else { "edit" }
                );
                restart_hint();
            }
            Some(WebFilesAction::Status) | None => {
                let config = client::load_config(&config_path)?;
                println!(
                    "web_files_enabled={}\nweb_uploads_enabled={}\nweb_management_enabled={}\nweb_edit_enabled={}\nweb_run_enabled={}",
                    config.web_files_enabled,
                    config.web_uploads_enabled,
                    config.web_management_enabled,
                    config.web_edit_enabled,
                    config.web_run_enabled
                );
            }
        },
        Command::Sync => print_report(client::sync(&config_path).await?),
        Command::Daemon { .. } => client::daemon(&config_path).await?,
        Command::Restart => unreachable!(),
        Command::Profiles => unreachable!(),
        Command::Status => {
            let status = client::status(&config_path).await?;
            println!("server_name={}", status.server_name);
            println!(
                "local_files={} remote_files={} pending_local={} conflicts={} server_generation={}",
                status.local_files,
                status.remote_files,
                status.pending_local,
                status.conflicts,
                status.generation
            );
        }
        Command::Trash => {
            let api = Api::new(&client::load_config(&config_path)?)?;
            for item in api.trash().await? {
                println!(
                    "id={} path={} bytes={} deleted_at={} expires_at={}",
                    item.id, item.path, item.size, item.deleted_at, item.expires_at
                );
            }
        }
        Command::Update => unreachable!(),
        Command::Restore { id } => {
            let api = Api::new(&client::load_config(&config_path)?)?;
            let entry = api.restore(id).await?;
            println!("restored {} at revision {}", entry.path, entry.revision);
            print_report(client::sync(&config_path).await?);
        }
    }
    Ok(())
}
