use actmon::db::Store;
use std::{fs, process::Command};

#[test]
fn backup_commands_publish_verify_and_report_failed_attempt_without_touching_source() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("live.db");
    let dest = root.path().join("snapshots");
    let _store = Store::open(&db, "fixture", "{}").unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_actmon"))
            .arg("backup")
            .arg("--database")
            .arg(&db)
            .arg("--destination")
            .arg(&dest)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let snapshot = report["state"]["last_success"]["snapshot"]
        .as_str()
        .unwrap();
    let verified = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .args(["verify-backup", "--snapshot", snapshot])
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(manifest["coverage"]["intervals"], 0);
    let state = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .arg("backup-status")
        .arg("--destination")
        .arg(&dest)
        .output()
        .unwrap();
    assert!(state.status.success());
    let state: serde_json::Value = serde_json::from_slice(&state.stdout).unwrap();
    assert_eq!(state["snapshot_directories"], 1);
    assert_eq!(actmon::db::status(&db).unwrap().run_id, Some(1));
    let missing = root.path().join("absent.db");
    let failed = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .arg("backup")
        .arg("--database")
        .arg(&missing)
        .arg("--destination")
        .arg(root.path().join("unused"))
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!missing.exists());
    assert!(!root.path().join("unused").exists());
    assert_eq!(fs::read_dir(dest).unwrap().count(), 1);
}

#[test]
fn fifo_source_is_rejected_before_sqlite_can_block() {
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("source.fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .arg("backup")
        .arg("--database")
        .arg(&fifo)
        .arg("--destination")
        .arg(root.path().join("published"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("backup blocked opening a FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("regular file"));
    assert!(!root.path().join("published").exists());
}
