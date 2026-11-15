CREATE TABLE runs (
    id INTEGER PRIMARY KEY,
    boot_id TEXT NOT NULL,
    version TEXT NOT NULL,
    configuration TEXT NOT NULL,
    started_utc_us INTEGER NOT NULL,
    started_mono_us INTEGER NOT NULL,
    latest_commit_utc_us INTEGER,
    stopped_utc_us INTEGER,
    last_batch INTEGER NOT NULL DEFAULT 0,
    previous_commit_latency_us INTEGER,
    capture_to_commit_us INTEGER
) STRICT;

CREATE TABLE intervals (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id),
    start_mono_us INTEGER NOT NULL,
    start_utc_us INTEGER NOT NULL,
    duration_us INTEGER NOT NULL CHECK(duration_us > 0 AND duration_us <= 1000000),
    available_mask INTEGER NOT NULL,
    incomplete_mask INTEGER NOT NULL,
    key_downs INTEGER NOT NULL,
    key_repeats INTEGER,
    button_left INTEGER NOT NULL,
    button_right INTEGER NOT NULL,
    button_middle INTEGER NOT NULL,
    button_side INTEGER NOT NULL,
    button_extra INTEGER NOT NULL,
    button_forward INTEGER NOT NULL,
    button_back INTEGER NOT NULL,
    button_task INTEGER NOT NULL,
    moved INTEGER NOT NULL CHECK(moved IN (0,1)),
    wheel_up INTEGER NOT NULL,
    wheel_down INTEGER NOT NULL,
    wheel_right INTEGER NOT NULL,
    wheel_left INTEGER NOT NULL,
    key_first_us INTEGER, key_last_us INTEGER,
    button_first_us INTEGER, button_last_us INTEGER,
    motion_first_us INTEGER, motion_last_us INTEGER,
    scroll_first_us INTEGER, scroll_last_us INTEGER,
    UNIQUE(run_id,start_mono_us)
) STRICT;
CREATE INDEX intervals_utc ON intervals(start_utc_us);

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs(id),
    utc_us INTEGER NOT NULL,
    mono_us INTEGER NOT NULL,
    kind TEXT NOT NULL,
    details TEXT NOT NULL,
    loss_start_mono_us INTEGER,
    loss_end_mono_us INTEGER,
    loss_mask INTEGER
) STRICT;
CREATE INDEX events_utc ON events(utc_us);
