use super::*;
use crate::{
    clock::Stamp,
    db::Store,
    model::{Action, Aggregator, Event},
};
use chrono::FixedOffset;

// 2026-11-15 00:00:00 +07:00.
const MIDNIGHT: i64 = 1_794_675_600 * SECOND;

fn quiet(second: i64) -> Row {
    Row {
        run: 1,
        mono: second * SECOND,
        start: MIDNIGHT + second * SECOND,
        duration: SECOND,
        available: ALL_INPUT,
        incomplete: 0,
        keys: 0,
        repeats: Some(0),
        clicks: 0,
        moved: false,
        scrolled: false,
        first: None,
        last: None,
    }
}

fn input(row: &mut Row, offset: i64) {
    row.keys = 1;
    row.first = Some(offset);
    row.last = Some(offset);
}

fn report(rows: &[Row], from: i64, to: i64, threshold: u64) -> Summary {
    build(
        Path::new("fixture.db"),
        MIDNIGHT + from * SECOND,
        MIDNIGHT + to * SECOND,
        threshold,
        rows,
        &FixedOffset::east_opt(7 * 3600).unwrap(),
    )
    .unwrap()
}

#[test]
fn five_minute_grace_is_excluded_and_input_ends_idle() {
    let mut rows: Vec<_> = (0..1201).map(quiet).collect();
    input(&mut rows[0], 0);
    input(&mut rows[1200], 0);
    let r = report(&rows, 0, 1201, 300);
    assert_eq!(r.idle_us, 900 * SECOND);
    assert_eq!(r.longest_idle_us, 900 * SECOND);
    assert_eq!(r.idle_periods.len(), 1);
    assert_eq!(r.counts.key_presses, 2);
    assert_eq!(r.minutes_containing_input, 2);
    assert_eq!(r.unknown_us, 0);
    assert_eq!(r.hours.iter().map(|h| h.idle_us).sum::<i64>(), r.idle_us);
    let text = r.render();
    assert_eq!(text.matches("+07:00").count(), 1);
    assert!(text.contains("Timezone: UTC+07:00"));
    assert!(text.contains("First input: 00:00:00\nLast input: 00:20:00\n"));
    assert!(text.contains("Idle 00:05:00 to 00:20:00"));
}

#[test]
fn first_last_offsets_bound_idle_with_subsecond_precision() {
    let mut rows: Vec<_> = (0..10).map(quiet).collect();
    input(&mut rows[0], 500_000);
    input(&mut rows[9], 250_000);
    let r = report(&rows, 0, 10, 5);
    assert_eq!(r.idle_us, 3_750_000);
    assert!(r.first_input.unwrap().contains("00:00:00.500000+07:00"));
    assert!(r.last_input.unwrap().contains("00:00:09.250000+07:00"));
}

#[test]
fn repeats_clicks_motion_and_scroll_each_interrupt_idle() {
    for category in 0..4 {
        let mut rows: Vec<_> = (0..12).map(quiet).collect();
        let row = &mut rows[6];
        match category {
            0 => row.repeats = Some(2),
            1 => row.clicks = 1,
            2 => row.moved = true,
            _ => row.scrolled = true,
        }
        row.first = Some(0);
        row.last = Some(0);
        let r = report(&rows, 0, 12, 5);
        assert_eq!(r.idle_us, if category == 0 { SECOND } else { 2 * SECOND });
        assert_eq!(r.minutes_containing_input, 1);
    }
}

#[test]
fn unavailable_incomplete_missing_and_restart_reset_the_timer() {
    for failure in 0..5 {
        let mut rows: Vec<_> = (0..12).map(quiet).collect();
        match failure {
            0 => rows[6].available &= !MOTION,
            1 => rows[6].incomplete |= KEYBOARD,
            2 => {
                rows.remove(6);
            }
            3 => {
                for row in &mut rows[6..] {
                    row.run = 2;
                }
            }
            _ => {
                for row in &mut rows[6..] {
                    row.mono += SECOND;
                }
            }
        }
        let r = report(&rows, 0, 12, 5);
        assert_eq!(r.idle_us, if failure < 3 { SECOND } else { 2 * SECOND });
        assert_eq!(r.unknown_us, if failure < 3 { SECOND } else { 0 });
    }
}

#[test]
fn positive_input_in_incomplete_intervals_is_preserved() {
    let mut rows: Vec<_> = (0..12).map(quiet).collect();
    input(&mut rows[6], 0);
    rows[6].incomplete = KEYBOARD;
    rows[6].repeats = None;
    let r = report(&rows, 0, 12, 5);
    assert_eq!(r.counts.key_presses, 1);
    assert_eq!(r.repeat_reporting_unknown_intervals, 1);
    assert_eq!(r.idle_us, SECOND);
    assert_eq!(r.unknown_us, SECOND);
    assert_eq!(r.coverage[0].complete_us, 11 * SECOND);
    assert_eq!(r.coverage[1].complete_us, 12 * SECOND);
}

#[test]
fn no_data_is_unknown_and_quiet_does_not_extend_past_last_commit() {
    let r = report(&[], 0, 1000, 300);
    assert_eq!(r.recorded_intervals, 0);
    assert_eq!(r.unknown_us, 1000 * SECOND);
    assert_eq!(r.idle_us, 0);
    assert!(r.first_input.is_none());
    let rows: Vec<_> = (100..500).map(quiet).collect();
    let r = report(&rows, 0, 1000, 300);
    assert_eq!(r.idle_us, 100 * SECOND);
    assert_eq!(r.unknown_us, 600 * SECOND);
    assert_eq!(r.unknown_periods.len(), 2);
}

#[test]
fn midnight_lookback_establishes_idle_but_counts_are_today_only() {
    let mut rows: Vec<_> = (-400..20).map(quiet).collect();
    input(&mut rows[0], 0);
    let r = report(&rows, 0, 20, 300);
    assert_eq!(r.idle_us, 20 * SECOND);
    assert_eq!(r.counts.key_presses, 0);
    assert!(r.first_input.is_none());
    assert!(r.start.starts_with("2026-11-15T00:00:00"));
}

#[test]
fn boundary_counts_are_omitted_but_coverage_and_idle_are_clipped() {
    let mut rows: Vec<_> = (-10..10).map(quiet).collect();
    for row in &mut rows {
        row.start += 500_000;
    }
    input(rows.last_mut().unwrap(), 0);
    let r = report(&rows, 0, 10, 5);
    assert_eq!(r.complete_coverage_us, 10 * SECOND);
    assert_eq!(r.unknown_us, 0);
    assert_eq!(r.counts.key_presses, 0);
    assert_eq!(r.idle_us, 9_500_000);
}

#[test]
fn overlapping_runs_do_not_double_count_coverage_or_establish_idle() {
    let mut rows: Vec<_> = (0..12).map(quiet).collect();
    let mut duplicate = quiet(6);
    duplicate.run = 2;
    rows.insert(7, duplicate);
    let r = report(&rows, 0, 12, 5);
    assert_eq!(r.unknown_us, SECOND);
    assert_eq!(r.complete_coverage_us, 11 * SECOND);
    assert_eq!(r.idle_us, SECOND);
    // Nested partial overlaps must also remain unknown.
    let mut nested = quiet(6);
    nested.start += 100_000;
    nested.duration = 100_000;
    rows.insert(8, nested);
    let mut later = quiet(6);
    later.start += 300_000;
    later.duration = 100_000;
    rows.insert(9, later);
    let r = report(&rows, 0, 12, 5);
    assert_eq!(r.unknown_us, SECOND);
}

#[test]
fn hourly_durations_split_straddling_intervals_and_share_timezone_header() {
    let mut rows: Vec<_> = (3590..3610).map(quiet).collect();
    for row in &mut rows {
        row.start += 500_000;
    }
    let r = report(&rows, 3590, 3610, 5);
    assert_eq!(r.hours.len(), 2);
    assert_eq!(r.hours[0].complete_coverage_us, 9_500_000);
    assert_eq!(r.hours[1].complete_coverage_us, 10 * SECOND);
    assert_eq!(r.hours[0].idle_us, 4_500_000);
    assert_eq!(r.hours[1].idle_us, 10 * SECOND);
    let text = r.render();
    assert!(text.contains("\n01:00 "));
    assert_eq!(text.matches("+07:00").count(), 1);
}

fn stamp(second: i64) -> Stamp {
    Stamp {
        mono_us: second * SECOND,
        utc_us: MIDNIGHT + second * SECOND,
        boot_us: second * SECOND,
    }
}

#[test]
fn reads_live_wal_and_retrospective_loss_without_modifying_the_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("activity.db");
    let mut store = Store::open(&path, "fixture", "{}").unwrap();
    let mut agg = Aggregator::new(stamp(0));
    agg.coverage(stamp(0), ALL_INPUT, true, false);
    agg.input(stamp(1), SECOND, Action::Key);
    store.write(&agg.finish(stamp(12), true)).unwrap();
    let mut batch = crate::model::Batch::default();
    let mut loss = Event::new("kernel_event_loss", stamp(20), "{}".into());
    loss.loss = Some((6 * SECOND, 7 * SECOND, KEYBOARD));
    batch.events.push(loss);
    store.write(&batch).unwrap();
    let before = std::fs::read(&path).unwrap();
    let r = summarize(
        &path,
        MIDNIGHT,
        MIDNIGHT + 12 * SECOND,
        5,
        &FixedOffset::east_opt(7 * 3600).unwrap(),
    )
    .unwrap();
    assert_eq!(r.counts.key_presses, 1);
    assert_eq!(r.unknown_us, SECOND);
    assert_eq!(r.idle_us, 0);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let c = store.connection();
    assert_eq!(
        c.query_row("SELECT count(*) FROM runs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn refuses_missing_foreign_and_unsupported_databases() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("missing.db");
    assert!(today(&path, 300).is_err());
    assert!(!path.exists());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE unrelated(value TEXT)")
        .unwrap();
    assert!(today(&path, 300).is_err());
    conn.execute_batch("PRAGMA application_id=1094931533; PRAGMA user_version=2")
        .unwrap();
    assert!(today(&path, 300).is_err());
    assert!(today(&path, 0).is_err());
}

#[test]
fn daylight_saving_hours_preserve_distinct_offsets_and_actual_day_length() {
    // Set TZ on a child process rather than changing the multithreaded test
    // runner's environment. This also exercises Chrono's OS timezone lookup.
    if std::env::var_os("ACTMON_TEST_DST").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env("TZ", "America/New_York")
            .env("ACTMON_TEST_DST", "1")
            .args(["--exact", "summary::tests::daylight_saving_hours_preserve_distinct_offsets_and_actual_day_length"])
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    for (month, day, next_day, length) in [(3, 8, 9, 23), (11, 1, 2, 25)] {
        let start = Local.with_ymd_and_hms(2026, month, day, 0, 0, 0).unwrap();
        let end = Local
            .with_ymd_and_hms(2026, month, next_day, 0, 0, 0)
            .unwrap();
        let r = build(
            Path::new("fixture.db"),
            start.timestamp_micros(),
            end.timestamp_micros(),
            300,
            &[],
            &Local,
        )
        .unwrap();
        assert_eq!(r.hours.len(), length);
        assert_eq!(r.unknown_us, length as i64 * 3600 * SECOND);
        if length == 25 {
            let text = r.render();
            assert!(text.contains("Timezone: UTC-04:00 to UTC-05:00"));
            assert_eq!(text.matches("-04:00").count(), 1);
            assert_eq!(text.matches("-05:00").count(), 1);
            assert!(r.hours[1].start.ends_with("-04:00"));
            assert!(r.hours[2].start.ends_with("-05:00"));
            assert!(r.hours[1].start.contains("T01:00:"));
            assert!(r.hours[2].start.contains("T01:00:"));
        }
    }
}
