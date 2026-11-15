use super::*;

fn stamp(mono_us: i64) -> Stamp {
    Stamp {
        mono_us,
        utc_us: 1_790_000_000_000_000 + mono_us,
        boot_us: mono_us,
    }
}

#[test]
fn half_open_intervals_preserve_endpoints_and_separate_repeats() {
    let mut agg = Aggregator::new(stamp(0));
    agg.coverage(stamp(0), KEYBOARD, true, true);
    agg.input(stamp(100), 100, Action::Key);
    agg.input(stamp(900), 900, Action::Key);
    agg.input(stamp(950), 950, Action::Repeat);
    agg.input(stamp(SECOND), SECOND, Action::Key);
    let batch = agg.finish(stamp(3 * SECOND), false);
    assert_eq!(batch.intervals.len(), 2);
    assert_eq!(batch.intervals[0].keys, 2);
    assert_eq!(batch.intervals[0].repeats, 1);
    assert_eq!(batch.intervals[0].spans[0].first, Some(100));
    assert_eq!(batch.intervals[0].spans[0].last, Some(900));
    assert_eq!(batch.intervals[1].keys, 1);
    assert_eq!(batch.intervals[1].spans[0].first, Some(0));
}

#[test]
fn availability_requires_full_interval_and_is_category_specific() {
    let mut agg = Aggregator::new(stamp(0));
    agg.coverage(stamp(20), KEYBOARD, true, false);
    let batch = agg.finish(stamp(3 * SECOND), false);
    assert_eq!(batch.intervals[0].available, 0);
    assert_eq!(batch.intervals[0].incomplete, KEYBOARD);
    assert_eq!(batch.intervals[1].available, KEYBOARD);
    assert_eq!(batch.intervals[1].incomplete, 0);
    assert_eq!(batch.intervals[1].keys, 0);
    assert!(!batch.intervals[1].repeats_supported);
    agg.coverage(stamp(3 * SECOND + 5), KEYBOARD, false, false);
    let batch = agg.finish(stamp(5 * SECOND), false);
    assert_eq!(batch.intervals.last().unwrap().available, 0);
}

#[test]
fn late_events_invalidate_persisted_history_instead_of_counting_now() {
    let mut agg = Aggregator::new(stamp(0));
    agg.finish(stamp(3 * SECOND), false);
    agg.input(stamp(3 * SECOND), 500, Action::Key);
    let batch = agg.finish(stamp(4 * SECOND), false);
    assert_eq!(batch.events[0].kind, "late_or_invalid_input");
    assert_eq!(batch.events[0].loss, Some((500, 3 * SECOND + 1, KEYBOARD)));
    assert!(batch.intervals.iter().all(|row| row.keys == 0));
}

#[test]
fn opposite_scroll_directions_do_not_cancel() {
    let mut agg = Aggregator::new(stamp(0));
    agg.input(
        stamp(10),
        10,
        Action::Wheel {
            axis: 0,
            amount: 120,
        },
    );
    agg.input(
        stamp(20),
        20,
        Action::Wheel {
            axis: 0,
            amount: -120,
        },
    );
    let row = agg.finish(stamp(2 * SECOND), false).intervals.remove(0);
    assert_eq!(row.wheel, [120, 120, 0, 0]);
    assert_eq!(row.spans[3].last, Some(20));
}

#[test]
fn discontinuity_does_not_fabricate_hours_of_idle_rows() {
    let mut agg = Aggregator::new(stamp(0));
    agg.input(stamp(20), 20, Action::Key);
    agg.discontinuity(stamp(3_600 * SECOND));
    let batch = agg.finish(stamp(3_602 * SECOND), false);
    assert_eq!(batch.intervals.len(), 1);
    assert_eq!(batch.intervals[0].start_mono_us, 3_600 * SECOND);
    assert_eq!(
        batch.events[0].loss,
        Some((0, 3_600 * SECOND, KEYBOARD | MOUSE))
    );
}

#[test]
fn orderly_stop_keeps_a_partial_interval_explicit() {
    let mut agg = Aggregator::new(stamp(0));
    agg.input(stamp(100), 100, Action::Motion);
    let batch = agg.finish(stamp(500), true);
    assert_eq!(batch.intervals[0].duration_us, 500);
    assert_eq!(batch.intervals[0].incomplete, KEYBOARD | MOUSE);
    assert!(batch.intervals[0].moved);
}
