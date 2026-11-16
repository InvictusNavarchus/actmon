use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use rusqlite::Connection;

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn collect(root: &Path, database: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_actmon"));
    command
        .args(["collect", "--database"])
        .arg(database)
        .args(["--mouse-name", "test mouse", "--sysfs-root"])
        .arg(root.join("sys"))
        .arg("--device-root")
        .arg(root.join("dev"));
    command
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for child");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn absent_sources_are_unavailable_and_collector_locks_database_until_shutdown() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("sys")).unwrap();
    let db = root.path().join("activity.db");
    let mut child = Process(
        collect(root.path(), &db)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until(|| actmon::db::status(&db).is_ok_and(|s| s.latest_interval_utc_us.is_some()));
    let duplicate = collect(root.path(), &db).output().unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("another collector"));
    let pid = rustix::process::Pid::from_raw(child.0.id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    wait_until(|| child.0.try_wait().unwrap().is_some());
    assert!(child.0.wait().unwrap().success());
    let c = Connection::open(&db).unwrap();
    let values = c
        .query_row(
            "SELECT SUM(key_downs),MAX(available_mask) FROM intervals",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap();
    assert_eq!(values, (0, 0)); // Zero counts alone do not mean healthy inactivity.
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM events WHERE kind='source_error'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert!(
        actmon::db::status(&db)
            .unwrap()
            .orderly_stop_utc_us
            .is_some()
    );
    assert_eq!(
        c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn status_defaults_to_home_database_and_allows_explicit_overrides() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home with spaces");
    let db = home.join(".local/share/actmon/activity.db");
    let store = actmon::db::Store::open(&db, "fixture", "{}").unwrap();
    let before: i64 = store
        .connection()
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .env("HOME", &home)
        .current_dir(root.path())
        .arg("status")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report,
        serde_json::to_value(actmon::db::status(&db).unwrap()).unwrap()
    );
    let after: i64 = store
        .connection()
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, before);

    let custom = root.path().join("custom.db");
    drop(actmon::db::Store::open(&custom, "fixture", "{}").unwrap());
    let _store = actmon::db::Store::open(&custom, "second-run", "{}").unwrap();
    let expected = serde_json::to_value(actmon::db::status(&custom).unwrap()).unwrap();
    assert_ne!(report["run_id"], expected["run_id"]);
    for home in [None, Some(home.as_path()), Some(Path::new("relative-home"))] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_actmon"));
        command.env_remove("HOME").current_dir(root.path()).args([
            "status",
            "--database",
            "custom.db",
        ]);
        if let Some(value) = home {
            command.env("HOME", value);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report, expected);
    }
}

#[test]
fn status_default_rejects_missing_database_and_invalid_home_without_creating_files() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .env("HOME", root.path())
        .arg("status")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("open existing database"));
    assert!(!root.path().join(".local").exists());
    for home in [None, Some(""), Some("relative-home")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_actmon"));
        command
            .env_remove("HOME")
            .current_dir(root.path())
            .arg("status");
        if let Some(value) = home {
            command.env("HOME", value);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("specify --database"));
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn metadata_and_status_commands_do_not_create_databases() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("sys")).unwrap();
    let db = root.path().join("missing.db");
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .args(["status", "--database"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!db.exists());
    fs::create_dir_all(root.path().join("sys/event0/device")).unwrap();
    fs::write(root.path().join("sys/event0/device/name"), "test mouse \n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_actmon"))
        .args(["devices", "--sysfs-root"])
        .arg(root.path().join("sys"))
        .arg("--device-root")
        .arg(root.path().join("absent-dev-root"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let devices: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(devices[0]["name"], "test mouse ");
    assert_eq!(
        devices[0]["node"],
        root.path().join("absent-dev-root/event0").to_str().unwrap()
    );
    assert!(!db.exists());
    assert!(!root.path().join("absent-dev-root").exists());
}

#[test]
fn abrupt_process_exit_preserves_committed_rows_and_restart_creates_new_run() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("sys")).unwrap();
    let db = root.path().join("activity.db");
    let mut child = Process(
        collect(root.path(), &db)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until(|| actmon::db::status(&db).is_ok_and(|s| s.latest_interval_utc_us.is_some()));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let status = actmon::db::status(&db).unwrap();
    assert!(status.orderly_stop_utc_us.is_none());
    let previous_run = status.run_id.unwrap();
    let mut restarted = Process(
        collect(root.path(), &db)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until(|| {
        actmon::db::status(&db).is_ok_and(|s| s.run_id.is_some_and(|id| id > previous_run))
    });
    restarted.0.kill().unwrap();
    restarted.0.wait().unwrap();
    let c = Connection::open(&db).unwrap();
    assert!(
        c.query_row(
            "SELECT count(*) FROM intervals WHERE run_id=?1",
            [previous_run],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            > 0
    );
    assert_eq!(
        c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}
