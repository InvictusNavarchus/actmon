use std::process::Command;

#[test]
fn summary_supports_text_json_and_local_midnight_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home with spaces");
    let path = home.join(".local/share/actmon/activity.db");
    let store = actmon::db::Store::open(&path, "fixture", "{}").unwrap();
    let data_version = || {
        store
            .connection()
            .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
            .unwrap()
    };
    let before = data_version();
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .env("TZ", "Asia/Jakarta")
        .env("HOME", &home)
        .current_dir(temp.path())
        .arg("summary")
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
    assert!(text.contains(&format!("Database: {}", path.display())));
    assert_eq!(data_version(), before);
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .env("TZ", "UTC")
        .env_remove("HOME")
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
    assert_eq!(data_version(), before);
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
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .env("HOME", temp.path())
        .arg("summary")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!temp.path().join(".local").exists());
    for home in [None, Some(""), Some("relative-home")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_actmon"));
        command.env_remove("HOME").arg("summary");
        if let Some(value) = home {
            command.env("HOME", value);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("specify --database"));
    }
}
