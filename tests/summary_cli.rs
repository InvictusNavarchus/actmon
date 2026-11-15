use std::process::Command;

#[test]
fn summary_supports_text_json_and_local_midnight_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("activity.db");
    let store = actmon::db::Store::open(&path, "fixture", "{}").unwrap();
    let before = store.connection().total_changes();
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .env("TZ", "Asia/Jakarta")
        .args(["summary", "--database"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("No recorded intervals"));
    assert!(text.contains("Idle after 300s"));
    assert!(text.contains("Timezone: UTC+07:00"));
    assert_eq!(text.matches("+07:00").count(), 1);
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .env("TZ", "UTC")
        .args(["summary", "--database"])
        .arg(&path)
        .args(["--json", "--idle-after-seconds", "600"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["start"]
            .as_str()
            .unwrap()
            .ends_with("T00:00:00.000000+00:00")
    );
    assert_eq!(report["idle_after_seconds"], 600);
    assert_eq!(report["recorded_intervals"], 0);
    assert_eq!(report["idle_us"], 0);
    assert!(report["unknown_us"].as_i64().unwrap() > 0);
    assert_eq!(store.connection().total_changes(), before);
}

#[test]
fn invalid_thresholds_and_missing_database_fail_without_creating_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("missing.db");
    for threshold in ["0", "86401", "-1", "invalid"] {
        let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
            .args(["summary", "--database"])
            .arg(&path)
            .args(["--idle-after-seconds", threshold])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(2));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .args(["summary", "--database"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!path.exists());
}
