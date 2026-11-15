use super::*;
use crate::model::{KEYBOARD, MOTION, MOUSE};

fn time(mono_us: i64) -> Stamp {
    Stamp {
        mono_us,
        utc_us: 1_790_000_000_000_000 + mono_us,
        boot_us: mono_us,
    }
}

fn source(mask: u8, last_drained_us: i64) -> Source {
    Source {
        role: Role::Mouse,
        name: "test".into(),
        reader: None,
        fault: None,
        loss_start: None,
        last_drained_us,
        capabilities: mask,
    }
}

#[test]
fn pre_flush_gap_check_prevents_complete_looking_idle_history() {
    let mut agg = Aggregator::new(time(0));
    agg.coverage(time(0), KEYBOARD, true, false);
    agg.finish(time(3 * SECOND), false);
    let mut previous = time(3 * SECOND);
    assert!(observe_time(&mut agg, &mut previous, time(20 * SECOND)));
    let batch = agg.finish(time(20 * SECOND), false);
    assert!(batch.intervals.is_empty());
    assert_eq!(
        batch.events[0].loss,
        Some((2 * SECOND, 20 * SECOND, KEYBOARD | MOUSE))
    );
}

#[test]
fn unread_shutdown_tail_invalidates_a_preceding_full_interval() {
    let mut agg = Aggregator::new(time(0));
    agg.coverage(time(0), MOTION, true, false);
    agg.finish(time(2 * SECOND), false);
    let mut source = source(MOTION, 1_900_000);
    source.invalidate_tail(&mut agg, time(3_100_000), "shutdown_unread_tail");
    let batch = agg.finish(time(3_100_000), true);
    let row = batch
        .intervals
        .iter()
        .find(|r| r.start_mono_us == 2 * SECOND)
        .unwrap();
    assert_eq!(row.available, MOTION);
    assert_eq!(row.incomplete, MOTION);
    assert_eq!(batch.events[0].loss, Some((1_900_000, 3_100_000, MOTION)));
}

#[test]
fn disconnect_and_overrun_only_invalidate_supported_categories() {
    let mut agg = Aggregator::new(time(0));
    agg.coverage(time(0), MOTION, true, false);
    let mut source = source(MOTION, 100);
    source.record_loss(&mut agg, time(200), 100);
    source.disconnect(&mut agg, time(300), "removed".into());
    let batch = agg.finish(time(2 * SECOND), false);
    assert!(
        batch
            .events
            .iter()
            .all(|event| event.loss.unwrap().2 == MOTION)
    );
    assert_eq!(batch.intervals[0].incomplete, MOTION);
    assert_eq!(source.mask(), 0);
}
