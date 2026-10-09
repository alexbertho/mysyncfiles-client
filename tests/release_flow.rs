use anyhow::Result;
use mysyncfiles_client::{
    client::ClientConfig,
    release,
    update::{self, UpdateOutcome},
};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn executable(path: &Path, version: &str) -> Result<()> {
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\ncase \"$1:$2\" in\n  --version:) printf 'mysync {version}\\n';;\n  setup:--help) exit 0;;\n  *) exit 1;;\nesac\n"
        ),
    )?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[test]
fn publishing_rejects_a_client_without_setup() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let signing_key = temp.path().join("signing.key");
    release::generate_key(&signing_key)?;
    let binary = temp.path().join("source-mysync");
    std::fs::write(
        &binary,
        "#!/bin/sh\ncase \"$1\" in --version) echo 'mysync 0.3.3';; *) exit 2;; esac\n",
    )?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let releases = temp.path().join("releases");
    let error = release::publish(
        &signing_key,
        &binary,
        "0.3.3",
        release::current_target().unwrap(),
        &releases,
    )
    .unwrap_err();
    assert!(error.to_string().contains("setup command"), "{error}");
    assert!(!releases.exists());
    Ok(())
}

#[test]
fn publishing_requires_a_new_version_for_a_changed_binary() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let key = temp.path().join("signing.key");
    release::generate_key(&key)?;
    let binary = temp.path().join("mysync");
    executable(&binary, "0.3.6")?;
    let releases = temp.path().join("releases");
    let target = release::current_target().unwrap();
    let manifest = release::publish(&key, &binary, "0.3.6", target, &releases)?;
    let envelope = releases
        .join(target)
        .join(format!("latest-{target}.signed.json"));
    let previous = std::fs::read(&envelope)?;
    let original_binary = std::fs::read(&binary)?;
    let mut changed = original_binary.clone();
    changed.extend_from_slice(b"\n# new client behavior\n");
    std::fs::write(&binary, changed)?;
    let error = release::publish(&key, &binary, "0.3.6", target, &releases).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("newer than the published version")
    );
    assert_eq!(std::fs::read(envelope)?, previous);
    assert_eq!(
        std::fs::read(releases.join(target).join(manifest.artifact))?,
        original_binary
    );
    Ok(())
}

#[tokio::test]
async fn delayed_older_signed_update_cannot_replace_a_newer_installation() -> Result<()> {
    use axum::{Router, routing::get};
    let temp = tempfile::tempdir()?;
    let key = temp.path().join("key");
    let public = release::generate_key(&key)?;
    let target = release::current_target().unwrap();
    let installed = temp.path().join("mysync");
    executable(&installed, "0.1.0")?;
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let mut configs = Vec::new();
    let mut tasks = Vec::new();
    for version in ["0.2.0", "0.3.0"] {
        let binary = temp.path().join(format!("source-{version}"));
        executable(&binary, version)?;
        let root = temp.path().join(version);
        let manifest = release::publish(&key, &binary, version, target, &root)?;
        let bytes = std::fs::read(root.join(target).join("latest.json"))?;
        let envelope = std::fs::read(
            root.join(target)
                .join(format!("latest-{target}.signed.json")),
        )?;
        let artifact = std::fs::read(&binary)?;
        let first = Arc::new(AtomicBool::new(true));
        let router = Router::new()
            .route(
                &format!("/v1/updates/{target}/latest.json"),
                get(move || {
                    let b = bytes.clone();
                    async move { b }
                }),
            )
            .route(
                &format!("/latest-{target}.signed.json"),
                get(move || {
                    let b = envelope.clone();
                    async move { b }
                }),
            )
            .route(
                &format!("/{}", manifest.artifact),
                get({
                    let started = started.clone();
                    let resume = resume.clone();
                    move || {
                        let (started, resume, b, first) = (
                            started.clone(),
                            resume.clone(),
                            artifact.clone(),
                            first.clone(),
                        );
                        async move {
                            if version == "0.2.0" && first.swap(false, Ordering::SeqCst) {
                                started.notify_one();
                                resume.notified().await;
                            }
                            b
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        configs.push(ClientConfig {
            server_public_key: String::new(),
            server: format!("http://{}", listener.local_addr()?),
            identity: None,
            root: temp.path().into(),
            auto_update: true,
            web_status_enabled: false,
            web_files_enabled: false,
            web_uploads_enabled: false,
            web_management_enabled: false,
            web_edit_enabled: false,
            web_run_enabled: false,
            update_public_key: public.clone(),
        });
        tasks.push(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }));
    }
    let newer = async {
        tokio::time::timeout(Duration::from_secs(10), started.notified()).await?;
        let result = update::check_and_install_at(
            &configs[1].server,
            &configs[1].update_public_key,
            &installed,
            "0.1.0",
        )
        .await;
        resume.notify_one();
        result
    };
    let (old, new) = tokio::join!(
        update::check_and_install_at(
            &configs[0].server,
            &configs[0].update_public_key,
            &installed,
            "0.1.0"
        ),
        newer
    );
    assert_eq!(new?, UpdateOutcome::Installed("0.3.0".into()));
    let current = UpdateOutcome::Current {
        installed_version: "0.3.0".into(),
        published_version: "0.2.0".into(),
    };
    assert_eq!(old?, current);
    assert!(std::fs::read_to_string(&installed)?.contains("0.3.0"));
    // Also cover a stale daemon that starts its check after the newer install.
    assert_eq!(
        update::check_and_install_at(
            &configs[0].server,
            &configs[0].update_public_key,
            &installed,
            "0.1.0"
        )
        .await?,
        current
    );
    for task in tasks {
        task.abort();
    }
    Ok(())
}

#[tokio::test]
async fn update_keeps_installed_client_when_signed_candidate_cannot_run() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for incompatible in ["exit 127", "printf 'mysync 0.0.0\\n'"] {
        let temp = tempfile::tempdir()?;
        let key = temp.path().join("key");
        let public_key = release::generate_key(&key)?;
        let binary = temp.path().join("source-mysync");
        let version = env!("CARGO_PKG_VERSION");
        // Publication host succeeds; installation host cannot execute the same
        // signed candidate (or sees a mismatched version).
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\ncase \"$0:$1:$2\" in\n*/source-mysync:--version:) printf 'mysync {version}\\n';;\n*/source-mysync:setup:--help) exit 0;;\n*) {incompatible};;\nesac\n"
            ),
        )?;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
        let releases = temp.path().join("releases");
        release::publish(
            &key,
            &binary,
            version,
            release::current_target().unwrap(),
            &releases,
        )?;
        let state = releases;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task =
            tokio::spawn(
                async move { axum::serve(listener, release_router(state)).await.unwrap() },
            );
        let installed = temp.path().join("mysync");
        std::fs::write(&installed, "old binary")?;
        let config = ClientConfig {
            server_public_key: String::new(),
            server: format!("http://{address}"),
            identity: None,
            root: temp.path().into(),
            auto_update: true,
            web_status_enabled: false,
            web_files_enabled: false,
            web_uploads_enabled: false,
            web_management_enabled: false,
            web_edit_enabled: false,
            web_run_enabled: false,
            update_public_key: public_key,
        };
        assert!(
            update::check_and_install_at(
                &config.server,
                &config.update_public_key,
                &installed,
                "0.1.0"
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read(&installed)?, b"old binary");
        assert!(!std::fs::read_dir(temp.path())?.any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".mysync-update-")
        }));
        task.abort();
    }
    Ok(())
}

#[tokio::test]
async fn signed_release_is_served_and_installed_atomically() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let signing_key = temp.path().join("signing.key");
    let public_key = release::generate_key(&signing_key)?;
    let binary = temp.path().join("source-mysync");
    let version = env!("CARGO_PKG_VERSION");
    executable(&binary, version)?;
    let target = release::current_target().unwrap();
    let releases = temp.path().join("releases");
    let manifest = release::publish(&signing_key, &binary, version, target, &releases)?;
    let manifest_bytes = std::fs::read(releases.join(target).join("latest.json"))?;
    let envelope: release::SignedRelease = serde_json::from_slice(&std::fs::read(
        releases
            .join(target)
            .join(format!("latest-{target}.signed.json")),
    )?)?;
    let signature = envelope.signature;
    let verified = release::verify_manifest(&manifest_bytes, &signature, &public_key)?;
    assert_eq!(verified.sha256, manifest.sha256);
    let mut tampered = manifest_bytes.clone();
    tampered[0] ^= 1;
    assert!(release::verify_manifest(&tampered, &signature, &public_key).is_err());

    let state = releases;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, release_router(state)).await.unwrap();
    });
    let installed = temp.path().join("mysync");
    executable(&installed, "0.3.6")?;
    let config = ClientConfig {
        server_public_key: String::new(),
        server: format!("http://{address}"),
        identity: None,
        root: temp.path().to_path_buf(),
        auto_update: true,
        web_status_enabled: false,
        web_files_enabled: false,
        web_uploads_enabled: false,
        web_management_enabled: false,
        web_edit_enabled: false,
        web_run_enabled: false,
        update_public_key: public_key,
    };
    let response = reqwest::get(format!("{}/latest-{target}.signed.json", config.server)).await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    // An independently opened lock is shared by every updater process.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(temp.path().join(".mysync-update.lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let previous = std::fs::read(&installed)?;
    assert!(
        update::check_and_install_at(
            &config.server,
            &config.update_public_key,
            &installed,
            "0.3.6"
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(&installed)?, previous);
    fs2::FileExt::unlock(&lock)?;
    assert_eq!(
        update::check_and_install_at(
            &config.server,
            &config.update_public_key,
            &installed,
            "0.3.6"
        )
        .await?,
        UpdateOutcome::Installed(version.into())
    );
    assert_eq!(std::fs::read(&installed)?, std::fs::read(&binary)?);
    assert_eq!(
        update::check_and_install_at(
            &config.server,
            &config.update_public_key,
            &installed,
            version
        )
        .await?,
        UpdateOutcome::Current {
            installed_version: version.into(),
            published_version: version.into(),
        }
    );
    task.abort();
    Ok(())
}

#[tokio::test]
async fn update_reports_the_actual_published_version_without_replacing_current_clients()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let key = temp.path().join("signing.key");
    let public_key = release::generate_key(&key)?;
    let binary = temp.path().join("source-mysync");
    executable(&binary, "0.3.6")?;
    let target = release::current_target().unwrap();
    let releases = temp.path().join("releases");
    let manifest = release::publish(&key, &binary, "0.3.6", target, &releases)?;
    // Equal or newer installed clients must not download an artifact.
    std::fs::remove_file(releases.join(target).join(manifest.artifact))?;
    let state = releases;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server_url = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move {
        axum::serve(listener, release_router(state)).await.unwrap();
    });
    let installed = temp.path().join("mysync");
    for version in ["0.3.6", env!("CARGO_PKG_VERSION")] {
        executable(&installed, version)?;
        let previous = std::fs::read(&installed)?;
        assert_eq!(
            update::check_and_install_at(&server_url, &public_key, &installed, version).await?,
            UpdateOutcome::Current {
                installed_version: version.into(),
                published_version: "0.3.6".into(),
            }
        );
        assert_eq!(std::fs::read(&installed)?, previous);
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn update_rejects_unsigned_metadata() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let signing_key = temp.path().join("signing.key");
    let public_key = release::generate_key(&signing_key)?;
    let binary = temp.path().join("source-mysync");
    let version = env!("CARGO_PKG_VERSION");
    executable(&binary, version)?;
    let target = release::current_target().unwrap();
    let releases = temp.path().join("releases");
    release::publish(&signing_key, &binary, version, target, &releases)?;
    let envelope_path = releases
        .join(target)
        .join(format!("latest-{target}.signed.json"));
    let valid_envelope = std::fs::read(&envelope_path)?;
    let mut invalid_envelope: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&envelope_path)?)?;
    invalid_envelope["signature"] = serde_json::Value::String("00".repeat(64));
    std::fs::write(&envelope_path, serde_json::to_vec(&invalid_envelope)?)?;
    let state = releases.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, release_router(state)).await.unwrap();
    });
    let installed = temp.path().join("mysync");
    std::fs::write(&installed, "old binary")?;
    let server_url = format!("http://{address}");
    assert!(
        update::check_and_install_at(&server_url, &public_key, &installed, "0.1.0")
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&installed)?, b"old binary");
    task.abort();

    // Missing signed metadata fails closed, including when an old manifest remains.
    std::fs::remove_file(&envelope_path)?;
    let artifact_path = releases
        .join(target)
        .join(format!("mysync-{version}-{target}"));
    let state = releases;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        axum::serve(listener, release_router(state)).await.unwrap();
    });
    let installed = temp.path().join("mysync");
    std::fs::write(&installed, "old binary")?;
    let config = ClientConfig {
        server_public_key: String::new(),
        server: format!("http://{address}"),
        identity: None,
        root: temp.path().to_path_buf(),
        auto_update: true,
        web_status_enabled: false,
        web_files_enabled: false,
        web_uploads_enabled: false,
        web_management_enabled: false,
        web_edit_enabled: false,
        web_run_enabled: false,
        update_public_key: public_key,
    };
    assert!(
        update::check_and_install_at(
            &config.server,
            &config.update_public_key,
            &installed,
            "0.1.0"
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(&installed)?, b"old binary");
    std::fs::write(&envelope_path, valid_envelope)?;
    let mut corrupted = std::fs::read(&artifact_path)?;
    corrupted[0] ^= 1;
    std::fs::write(&artifact_path, corrupted)?;
    assert!(
        update::check_and_install_at(
            &config.server,
            &config.update_public_key,
            &installed,
            "0.1.0"
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(&installed)?, b"old binary");
    task.abort();
    Ok(())
}

// This serves only generated fixtures in a private directory, emulating release
// assets. Production client builds contain no server or database dependency.
fn release_router(root: std::path::PathBuf) -> axum::Router {
    use axum::{Router, extract::Path, http::StatusCode, routing::get};
    Router::new().route(
        "/{file}",
        get(move |Path(file): Path<String>| {
            let root = root.clone();
            async move {
                if file.contains('/') || file.contains("..") {
                    return Err(StatusCode::NOT_FOUND);
                }
                tokio::fs::read(root.join(release::current_target().unwrap()).join(file))
                    .await
                    .map_err(|_| StatusCode::NOT_FOUND)
            }
        }),
    )
}
