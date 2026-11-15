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
