use super::*;
use crate::{
    clock::Stamp,
    model::{Action, Aggregator, Event, KEYBOARD, SECOND},
};

fn stamp(mono_us: i64) -> Stamp {
    Stamp {
        mono_us,
        utc_us: 1_790_000_000_000_000 + mono_us,
        boot_us: mono_us,
    }
}

#[test]
fn persists_measurements_and_updates_prior_coverage_on_late_arrival() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("activity.db");
    let mut store = Store::open(&path, "boot-a", "{}").unwrap();
    assert_eq!(
        store
            .connection()
            .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .connection()
            .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    let mut agg = Aggregator::new(stamp(0));
    agg.coverage(stamp(0), KEYBOARD, true, false);
    agg.input(stamp(SECOND + 100), SECOND + 100, Action::Key);
    store.write(&agg.finish(stamp(3 * SECOND), false)).unwrap();
    let row=store.connection().query_row("SELECT key_downs,key_first_us,key_last_us,key_repeats,incomplete_mask FROM intervals WHERE start_mono_us=1000000",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,i64>(4)?))).unwrap();
    assert_eq!(row, (1, 100, 100, None, 0));
    agg.input(stamp(3 * SECOND), SECOND + 500, Action::Key);
    store.write(&agg.finish(stamp(4 * SECOND), false)).unwrap();
    let incomplete: i64 = store
        .connection()
        .query_row(
            "SELECT incomplete_mask FROM intervals WHERE start_mono_us=1000000",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(incomplete, i64::from(KEYBOARD));
    assert!(status(&path).unwrap().previous_commit_latency_us.is_some());
}

#[test]
fn failed_batch_rolls_back_intervals_and_events() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("activity.db");
    let mut store = Store::open(&path, "boot-a", "{}").unwrap();
    let mut agg = Aggregator::new(stamp(0));
    let mut batch = agg.finish(stamp(3 * SECOND), false);
    batch.intervals[1].duration_us = 0;
    batch.events.push(Event::new("test", stamp(0), "{}".into()));
    assert!(store.write(&batch).is_err());
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM intervals", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn refusing_unknown_databases_and_reading_status_does_not_create_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("other.db");
    assert!(status(&path).is_err());
    assert!(!path.exists());
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE unrelated(value TEXT)")
        .unwrap();
    drop(c);
    assert!(Store::open(&path, "boot-a", "{}").is_err());
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
}

#[test]
fn restarts_have_distinct_runs_and_do_not_overwrite_old_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("activity.db");
    for boot in ["boot-a", "boot-b"] {
        let mut store = Store::open(&path, boot, "{}").unwrap();
        let mut agg = Aggregator::new(stamp(0));
        store.write(&agg.finish(stamp(SECOND), true)).unwrap();
    }
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM intervals", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(status(&path).unwrap().orderly_stop_utc_us.is_some());
}

#[test]
fn retry_after_uncertain_commit_does_not_duplicate_a_successful_batch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("activity.db");
    let mut store = Store::open(&path, "boot-a", "{}").unwrap();
    let mut agg = Aggregator::new(stamp(0));
    let batch = agg.finish(stamp(SECOND), true);
    store.write(&batch).unwrap();
    // Simulate a successful disk commit whose acknowledgement was lost.
    store.next_batch = 1;
    store.write(&batch).unwrap();
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM intervals", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(store.next_batch, 2);
}
