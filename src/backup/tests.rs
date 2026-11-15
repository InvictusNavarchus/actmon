use super::*;
use crate::{
    clock::Stamp,
    db::Store,
    model::{Action, Aggregator, KEYBOARD},
};

fn source(root: &Path) -> (PathBuf, Store) {
    let path = root.join("live.db");
    let mut store = Store::open(&path, "test-boot", r#"{"keyboard_name":"fixture"}"#).unwrap();
    let stamp = |mono_us| Stamp {
        mono_us,
        utc_us: 1_000_000 + mono_us,
        boot_us: mono_us,
    };
    let mut agg = Aggregator::new(stamp(0));
    agg.coverage(stamp(0), KEYBOARD, true, false);
    agg.input(stamp(100), 100, Action::Key);
    store.write(&agg.finish(stamp(3_000_000), false)).unwrap();
    (path, store)
}
fn make(path: &Path, dest: &Path) -> Status {
    create(
        path,
        dest,
        Duration::from_secs(5),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}

#[test]
fn live_wal_snapshot_restores_without_original_and_supports_new_writes() {
    let root = tempfile::tempdir().unwrap();
    let (path, store) = source(root.path());
    assert!(path.with_file_name("live.db-wal").metadata().unwrap().len() > 0);
    let dest = root.path().join("published");
    let report = make(&path, &dest);
    let snapshot = &report.state.last_success.as_ref().unwrap().snapshot;
    assert_eq!(fs::read_dir(snapshot).unwrap().count(), 2);
    let manifest = verify(snapshot).unwrap();
    assert_eq!(manifest.sqlite_version, rusqlite::version());
    assert_eq!(manifest.coverage.intervals, 2);
    assert_eq!(fs::metadata(snapshot).unwrap().mode() & 0o777, 0o700);
    let restored = root.path().join("restored");
    fs::create_dir(&restored).unwrap();
    for name in [DATABASE, MANIFEST] {
        fs::copy(snapshot.join(name), restored.join(name)).unwrap();
    }
    drop(store);
    fs::remove_file(path).unwrap();
    verify(&restored).unwrap();
    let read = crate::db::status(&restored.join(DATABASE)).unwrap();
    assert_eq!(read.run_id, Some(1));
    let mut store = Store::open(&restored.join(DATABASE), "restored-boot", "{}").unwrap();
    store.write(&crate::model::Batch::default()).unwrap();
    assert_eq!(
        store
            .connection()
            .query_row("select sum(key_downs) from intervals", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        crate::db::status(&restored.join(DATABASE)).unwrap().run_id,
        Some(2)
    );
    verify(snapshot).unwrap(); // Restoring never changes the published artifact.
}

#[test]
fn failed_attempt_preserves_success_and_cleans_owned_staging() {
    let root = tempfile::tempdir().unwrap();
    let (path, _store) = source(root.path());
    let dest = root.path().join("published");
    let previous = make(&path, &dest).state.last_success.unwrap().snapshot;
    assert!(
        create(
            &path,
            &dest,
            Duration::ZERO,
            Arc::new(AtomicBool::new(false))
        )
        .is_err()
    );
    let report = status(&dest).unwrap();
    assert!(report.state.last_error.unwrap().contains("deadline"));
    assert!(report.state.attempt_finished_utc_us.is_some());
    assert_eq!(report.state.last_success.unwrap().snapshot, previous);
    assert_eq!(report.snapshot_directories, 1);
    verify(&previous).unwrap();
    assert!(
        fs::read_dir(control_path(&dest).unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("staging-"))
    );
}

#[test]
fn checksum_corruption_missing_file_and_sidecars_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let (path, _store) = source(root.path());
    let snapshot = make(&path, &root.path().join("published"))
        .state
        .last_success
        .unwrap()
        .snapshot;
    let copy = root.path().join("copy");
    fs::create_dir(&copy).unwrap();
    fs::copy(snapshot.join(MANIFEST), copy.join(MANIFEST)).unwrap();
    assert!(verify(&copy).is_err());
    fs::copy(snapshot.join(DATABASE), copy.join(DATABASE)).unwrap();
    fs::write(copy.join("activity.db-wal"), []).unwrap();
    assert!(verify(&copy).unwrap_err().to_string().contains("sidecar"));
    fs::remove_file(copy.join("activity.db-wal")).unwrap();
    OpenOptions::new()
        .append(true)
        .open(copy.join(DATABASE))
        .unwrap()
        .write_all(b"corruption")
        .unwrap();
    assert!(verify(&copy).unwrap_err().to_string().contains("checksum"));
    verify(&snapshot).unwrap();
}

#[test]
fn refuses_overlap_foreign_database_and_destination_reuse() {
    let root = tempfile::tempdir().unwrap();
    let (path, _store) = source(root.path());
    let stop = || Arc::new(AtomicBool::new(false));
    assert!(create(&path, root.path(), Duration::from_secs(5), stop()).is_err());
    let dest = root.path().join("published");
    make(&path, &dest);
    let other = root.path().join("other.db");
    Connection::open(&other)
        .unwrap()
        .execute_batch("create table foreign_data(x);")
        .unwrap();
    assert!(
        create(&other, &dest, Duration::from_secs(5), stop())
            .unwrap_err()
            .to_string()
            .contains("different database")
    );
    let foreign_dest = root.path().join("foreign");
    assert!(create(&other, &foreign_dest, Duration::from_secs(5), stop()).is_err());
    assert_eq!(fs::read_dir(foreign_dest).unwrap().count(), 0);
}

#[test]
fn concurrent_backup_is_refused_without_overwriting_attempt_state() {
    let root = tempfile::tempdir().unwrap();
    let (path, _store) = source(root.path());
    let dest = root.path().join("published");
    make(&path, &dest);
    let control = control_path(&dest).unwrap();
    let state = fs::read(control.join("state.json")).unwrap();
    let lock = File::open(control.join("lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let error = create(
        &path,
        &dest,
        Duration::from_secs(5),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap_err();
    assert!(error.to_string().contains("another backup"));
    assert_eq!(state, fs::read(control.join("state.json")).unwrap());
}

#[test]
fn publication_never_replaces_existing_directory() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let published = root.path().join("published");
    fs::create_dir(&staging).unwrap();
    fs::create_dir(&published).unwrap();
    fs::write(staging.join("payload"), b"new").unwrap();
    assert!(publish_directory(&staging, &published).is_err());
    assert!(staging.join("payload").exists());
    assert_eq!(fs::read_dir(published).unwrap().count(), 0);
}

#[test]
fn excess_snapshots_are_reported_but_never_deleted() {
    let root = tempfile::tempdir().unwrap();
    let (path, _store) = source(root.path());
    let dest = root.path().join("published");
    let first = make(&path, &dest).state.last_success.unwrap().snapshot;
    for _ in 0..7 {
        make(&path, &dest);
    }
    let report = status(&dest).unwrap();
    assert_eq!(report.snapshot_directories, 8);
    assert_eq!(report.excess_snapshots, 1);
    assert!(!report.automatic_deletion);
    verify(&first).unwrap();
}

#[test]
fn deadline_bounds_contended_source_and_cancellation_preserves_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let (path, store) = source(root.path());
    drop(store);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("pragma journal_mode=DELETE; begin exclusive;")
        .unwrap();
    let dest = root.path().join("published");
    let start = Instant::now();
    assert!(
        create(
            &path,
            &dest,
            Duration::from_millis(50),
            Arc::new(AtomicBool::new(false))
        )
        .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    writer.execute_batch("rollback;").unwrap();
    let first = make(&path, &dest).state.last_success.unwrap().snapshot;
    assert!(
        create(
            &path,
            &dest,
            Duration::from_secs(5),
            Arc::new(AtomicBool::new(true))
        )
        .is_err()
    );
    verify(&first).unwrap();
}

#[test]
fn validation_failure_never_publishes_and_keeps_last_good_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (path, store) = source(root.path());
    let dest = root.path().join("published");
    let first = make(&path, &dest).state.last_success.unwrap().snapshot;
    store
        .connection()
        .execute_batch("PRAGMA foreign_keys=OFF; UPDATE intervals SET run_id=999;")
        .unwrap();
    let error = create(
        &path,
        &dest,
        Duration::from_secs(5),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap_err();
    assert!(error.to_string().contains("foreign key"));
    assert_eq!(status(&dest).unwrap().snapshot_directories, 1);
    assert!(
        fs::read_dir(control_path(&dest).unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("staging-"))
    );
    verify(&first).unwrap();
}

#[test]
fn concurrent_wal_transactions_remain_atomic_in_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (path, store) = source(root.path());
    // More than one backup step, with two values committed as one transaction.
    store.connection().execute_batch("INSERT INTO events(run_id,utc_us,mono_us,kind,details) VALUES(1,1,1,'pair-a','0'),(1,1,1,'pair-b','0'); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO events(run_id,utc_us,mono_us,kind,details) SELECT 1,1,1,'padding',hex(zeroblob(2048)) FROM n;").unwrap();
    let writer_path = path.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let writer_barrier = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        let mut c = Connection::open(writer_path).unwrap();
        c.busy_timeout(Duration::from_secs(2)).unwrap();
        writer_barrier.wait();
        for value in 1..=20 {
            let tx = c.transaction().unwrap();
            tx.execute(
                "UPDATE events SET details=?1 WHERE kind in ('pair-a','pair-b')",
                [value.to_string()],
            )
            .unwrap();
            tx.commit().unwrap();
            thread::sleep(Duration::from_millis(2));
        }
    });
    barrier.wait();
    let report = make(&path, &root.path().join("published"));
    writer.join().unwrap();
    let snapshot = report.state.last_success.unwrap().snapshot;
    verify(&snapshot).unwrap();
    let c = Connection::open_with_flags(snapshot.join(DATABASE), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let values: (i64, i64) = c
        .query_row(
            "SELECT count(*),count(distinct details) FROM events WHERE kind in ('pair-a','pair-b')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(values, (2, 1));
}

#[test]
fn dangling_sidecars_are_rejected_without_following_links() {
    let root = tempfile::tempdir().unwrap();
    let (path, _store) = source(root.path());
    let snapshot = make(&path, &root.path().join("published"))
        .state
        .last_success
        .unwrap()
        .snapshot;
    let copy = root.path().join("restore");
    fs::create_dir(&copy).unwrap();
    for name in [DATABASE, MANIFEST] {
        fs::copy(snapshot.join(name), copy.join(name)).unwrap();
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = copy.join(format!("{DATABASE}{suffix}"));
        std::os::unix::fs::symlink(root.path().join("missing"), &sidecar).unwrap();
        assert!(verify(&copy).unwrap_err().to_string().contains("sidecar"));
        fs::remove_file(sidecar).unwrap();
    }
    verify(&copy).unwrap();
}
