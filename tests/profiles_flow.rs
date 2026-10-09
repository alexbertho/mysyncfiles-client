use anyhow::Result;
use mysyncfiles_client::client::profiles;
use std::process::Command;

#[test]
fn profiles_are_selected_explicitly_and_never_escape_the_registry() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let base = temp.path().join("mysync");
    std::fs::create_dir_all(base.join("profiles"))?;
    let root1 = temp.path().join("one");
    let root2 = temp.path().join("two");
    std::fs::create_dir(&root1)?;
    std::fs::create_dir(&root2)?;
    for (name, root) in [("home", &root1), ("work", &root2)] {
        std::fs::write(
            base.join("profiles").join(format!("{name}.json")),
            serde_json::to_vec(
                &serde_json::json!({"server":format!("https://{name}.example.com"),"root":root}),
            )?,
        )?;
    }
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mysync"))
            .env("XDG_CONFIG_HOME", temp.path())
            .args(args)
            .output()
    };
    let listed = run(&["profiles"])?;
    assert!(listed.status.success());
    let text = String::from_utf8(listed.stdout)?;
    assert!(text.contains("home\thttps://home.example.com"));
    assert!(text.contains("work\thttps://work.example.com"));
    assert!(!run(&["status"])?.status.success());
    let doctor = run(&["doctor"])?;
    assert!(!String::from_utf8_lossy(&doctor.stderr).contains("several servers"));
    assert!(run(&["--profile", "home", "web-status"])?.status.success());
    for name in ["../home", "a/b", "", "é", "."] {
        assert!(!profiles::valid_name(name));
    }
    assert!(
        !run(&["--profile", "../home", "web-status"])?
            .status
            .success()
    );
    assert!(
        !run(&["--profile", "home", "daemon", "--all"])?
            .status
            .success()
    );
    Ok(())
}

#[test]
fn overlapping_roots_pending_enrollments_and_symlinks_are_rejected() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let base = temp.path().join("config");
    std::fs::create_dir_all(base.join("profiles"))?;
    let root = temp.path().join("mirror");
    std::fs::create_dir_all(root.join("child"))?;
    let path = base.join("profiles/second.json");
    std::fs::write(
        base.join("profiles/first.enrollment.json"),
        serde_json::to_vec(&serde_json::json!({"config":{"root":root}}))?,
    )?;
    assert!(profiles::validate_root(&path, &root).is_err());
    assert!(profiles::validate_root(&path, &root.join("child")).is_err());
    assert!(profiles::validate_root(&path, temp.path()).is_err());
    let separate = temp.path().join("separate");
    std::fs::create_dir(&separate)?;
    profiles::validate_root(&path, &separate)?;
    let lock = profiles::registration_lock(&path, &separate)?;
    assert!(profiles::registration_lock(&path, &separate).is_err());
    drop(lock);
    std::os::unix::fs::symlink(root.join("child"), temp.path().join("alias"))?;
    assert!(profiles::validate_root(&path, &temp.path().join("alias")).is_err());
    std::os::unix::fs::symlink(
        base.join("profiles/first.enrollment.json"),
        base.join("profiles/unsafe.json"),
    )?;
    assert!(profiles::list_in(&base).is_err());
    std::fs::remove_file(base.join("profiles/unsafe.json"))?;
    std::os::unix::fs::symlink(
        base.join("profiles/first.enrollment.json"),
        base.join("config.json"),
    )?;
    assert!(profiles::list_in(&base).is_err());
    Ok(())
}
