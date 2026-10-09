//! Named connections have independent identities, consent, roots and checkpoints.
use super::{Api, ClientConfig, default_config_path, load_config};
use crate::web_status_protocol::DaemonState;
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn path(name: &str) -> Result<PathBuf> {
    ensure!(
        valid_name(name),
        "profile name must contain 1 to 48 letters, digits, hyphens or underscores"
    );
    if name == "default" {
        return default_config_path();
    }
    Ok(default_config_path()?
        .parent()
        .unwrap()
        .join("profiles")
        .join(format!("{name}.json")))
}

pub fn list_in(base: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut profiles = Vec::new();
    let legacy = base.join("config.json");
    if let Ok(metadata) = legacy.symlink_metadata() {
        ensure!(
            metadata.file_type().is_file(),
            "profile must be a regular file"
        );
        profiles.push(("default".into(), legacy));
    }
    let directory = base.join("profiles");
    if directory.exists() {
        ensure!(
            !directory.symlink_metadata()?.file_type().is_symlink(),
            "profile directory must not be a symbolic link"
        );
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
                if valid_name(name) {
                    ensure!(name != "default", "default must use mysync/config.json");
                    ensure!(
                        entry.file_type()?.is_file(),
                        "profile must be a regular file"
                    );
                    profiles.push((name.to_owned(), path));
                }
            }
        }
    }
    profiles.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(profiles)
}

pub fn list() -> Result<Vec<(String, PathBuf)>> {
    list_in(default_config_path()?.parent().unwrap())
}

pub fn selected(name: Option<&str>, explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Some(name) = name {
        return path(name);
    }
    let profiles = list()?;
    match profiles.as_slice() {
        [] => default_config_path(),
        [(_, path)] => Ok(path.clone()),
        _ => bail!("several servers are configured; select --profile NAME or use daemon --all"),
    }
}

fn registry_base(config: &Path) -> &Path {
    let parent = config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if parent.file_name().is_some_and(|n| n == "profiles") {
        parent.parent().unwrap_or(parent)
    } else {
        parent
    }
}

/// Serialize root reservations while writing pairing/enrollment/profile JSON.
/// Pending profiles also reserve their root, so interrupted enrollment is safe.
pub fn registration_lock(config: &Path, root: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    use std::os::unix::fs::OpenOptionsExt;
    let base = registry_base(config);
    std::fs::create_dir_all(base)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(base.join(".profiles.lock"))?;
    lock.try_lock_exclusive()
        .context("another profile registration is in progress; retry shortly")?;
    validate_root(config, root)?;
    Ok(lock)
}

pub fn validate_root(config: &Path, root: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let parent = config.parent().unwrap_or(Path::new(".")).canonicalize()?;
    let config = parent.join(config.file_name().context("invalid profile path")?);
    let base = registry_base(&config);
    for directory in [base.to_path_buf(), base.join("profiles")] {
        if !directory.exists() {
            continue;
        }
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path == config
                || path == config.with_extension("pairing.json")
                || path == config.with_extension("enrollment.json")
            {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".json") || name.ends_with(".state.json") {
                continue;
            }
            ensure!(
                !path.symlink_metadata()?.file_type().is_symlink(),
                "profile must not be a symbolic link"
            );
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let value = value.get("config").unwrap_or(&value);
            if let Some(other) = value.get("root").and_then(|r| r.as_str()) {
                let other = Path::new(other)
                    .canonicalize()
                    .context("another profile root is unavailable")?;
                ensure!(
                    !root.starts_with(&other) && !other.starts_with(&root),
                    "synchronization folders must be separate and must not contain one another: {}",
                    path.display()
                );
                ensure!(
                    !parent.starts_with(&other),
                    "profile configuration must stay outside all synchronized folders"
                );
            }
        }
    }
    ensure!(
        !parent.starts_with(&root),
        "profile configuration must stay outside the synchronized folder"
    );
    Ok(())
}

pub async fn daemon_all() -> Result<()> {
    let profiles = list()?;
    ensure!(
        !profiles.is_empty(),
        "no servers configured; run mysync --profile NAME setup"
    );
    let mut prepared = Vec::new();
    let mut bridge_profiles = Vec::new();
    for (name, path) in profiles {
        let config: ClientConfig = load_config(&path)?;
        validate_root(&path, &config.root)?;
        let api = Arc::new(Api::new(&config)?);
        let activity = Arc::new(Mutex::new(DaemonState::Starting));
        if config.web_status_enabled {
            bridge_profiles.push((api.clone(), activity.clone()));
        }
        prepared.push((name, path, config, api, activity));
    }
    let _bridge = if bridge_profiles.is_empty() {
        None
    } else {
        match super::local_api::LocalBridge::start_many(bridge_profiles) {
            Ok(bridge) => Some(bridge),
            Err(error) => {
                eprintln!("web status bridge disabled: {error:#}; synchronization continues");
                None
            }
        }
    };
    let mut tasks = tokio::task::JoinSet::new();
    for (name, path, config, api, activity) in prepared {
        tasks.spawn(async move {
            super::daemon_with_runtime(&path, config, api, activity)
                .await
                .with_context(|| format!("server profile {name}"))
        });
    }
    // Successful auto-update or signal restarts/stops the whole process, keeping
    // every profile on the same binary. Dropping JoinSet cancels all workers.
    if let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}
