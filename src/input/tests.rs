use super::*;

fn ev(ty: u16, code: u16, value: i32) -> InputEvent {
    InputEvent::new(ty, code, value)
}

#[test]
fn keyboard_counts_transitions_without_counting_buttons_or_releases() {
    let mut decoder = Decoder::new(Role::Keyboard, false, false);
    assert_eq!(decoder.decode(&ev(1, 30, 1)), Decoded::Action(Action::Key));
    assert_eq!(decoder.decode(&ev(1, 30, 0)), Decoded::Ignore);
    assert_eq!(
        decoder.decode(&ev(1, 30, 2)),
        Decoded::Action(Action::Repeat)
    );
    assert_eq!(decoder.decode(&ev(1, 272, 1)), Decoded::Ignore);
    assert_eq!(decoder.decode(&ev(1, 544, 1)), Decoded::Ignore);
    assert_eq!(decoder.decode(&ev(1, 352, 1)), Decoded::Action(Action::Key));
}

#[test]
fn high_resolution_wheel_suppresses_duplicate_legacy_axis() {
    let mut decoder = Decoder::new(Role::Mouse, true, false);
    assert_eq!(decoder.decode(&ev(2, 8, 1)), Decoded::Ignore);
    assert_eq!(
        decoder.decode(&ev(2, 11, 60)),
        Decoded::Action(Action::Wheel {
            axis: 0,
            amount: 60
        })
    );
    assert_eq!(
        decoder.decode(&ev(2, 6, -1)),
        Decoded::Action(Action::Wheel {
            axis: 1,
            amount: -120
        })
    );
    assert_eq!(decoder.decode(&ev(2, 0, 0)), Decoded::Ignore);
    assert_eq!(
        decoder.decode(&ev(1, 272, 1)),
        Decoded::Action(Action::Button(0))
    );
}

#[test]
fn overrun_discards_everything_through_next_report() {
    let mut decoder = Decoder::new(Role::Keyboard, false, false);
    assert_eq!(decoder.decode(&ev(0, 3, 0)), Decoded::Dropped);
    assert_eq!(decoder.decode(&ev(1, 30, 1)), Decoded::Ignore);
    assert_eq!(decoder.decode(&ev(0, 0, 0)), Decoded::Recovered);
    assert_eq!(decoder.decode(&ev(1, 30, 1)), Decoded::Action(Action::Key));
}

#[test]
fn ambiguous_names_are_not_silently_selected() {
    let info = DeviceInfo {
        node: "event0".into(),
        name: "keyboard-debouncer".into(),
        sysfs: "device0".into(),
    };
    assert!(select(&[info.clone(), info], "keyboard-debouncer").is_err());
    assert!(select(&[], "keyboard-debouncer").unwrap().is_none());
}

#[test]
fn metadata_discovery_preserves_name_spaces_and_ignores_non_event_nodes() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("event5/device")).unwrap();
    fs::write(root.path().join("event5/device/name"), "USB Mouse \n").unwrap();
    fs::create_dir_all(root.path().join("mouse0/device")).unwrap();
    fs::write(root.path().join("mouse0/device/name"), "ignore\n").unwrap();
    let list = discover(root.path(), Path::new("/not-live")).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "USB Mouse ");
    assert_eq!(list[0].node, PathBuf::from("/not-live/event5"));
}
