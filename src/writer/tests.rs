use super::*;

#[test]
fn saturated_queue_is_bounded_and_next_batch_exposes_earliest_lost_evidence() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut writer = Writer {
        sender: Some(sender),
        thread: None,
        stopping: Arc::new(AtomicBool::new(false)),
        lost_since: None,
    };
    writer.submit(Batch::default()).unwrap();
    let mut loss = Event::new("late", clock::now(), "{}".into());
    loss.loss = Some((10, 20, KEYBOARD));
    writer
        .submit(Batch {
            events: vec![loss],
            ..Batch::default()
        })
        .unwrap();
    assert_eq!(writer.lost_since, Some(10));
    receiver.recv().unwrap();
    writer.submit(Batch::default()).unwrap();
    let batch = receiver.recv().unwrap();
    assert_eq!(batch.events[0].kind, "writer_queue_loss");
    assert_eq!(batch.events[0].loss.unwrap().0, 10);
    assert_eq!(batch.events[0].loss.unwrap().2, KEYBOARD | MOUSE);
    assert!(writer.lost_since.is_none());
}

#[test]
fn closed_writer_is_reported_to_capture() {
    let (sender, receiver) = mpsc::sync_channel(1);
    drop(receiver);
    let mut writer = Writer {
        sender: Some(sender),
        thread: None,
        stopping: Arc::new(AtomicBool::new(false)),
        lost_since: None,
    };
    assert!(writer.submit(Batch::default()).is_err());
}
