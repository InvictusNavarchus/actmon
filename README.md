# Actmon

Actmon records Linux keyboard and mouse activity in SQLite for investigations
alongside Batmon and the system journal. It preserves one-second measurements
permanently, including collection coverage and first/last event timing. It does
not persist key identities, typed text, or pointer coordinates.

The initial collector is implemented. Automatic backups, systemd installation,
direct suspend notifications, and optional session metadata remain later work;
the full target architecture is in [DESIGN.md](DESIGN.md).

## Build and inspect devices

Requires Linux, Rust 1.89 or newer, and a C compiler for bundled SQLite.

```sh
cargo build --locked --release
./target/release/actmon devices
```

Device discovery reads `/sys/class/input` metadata and prints JSON with the exact
name, event node, and sysfs identity. It does not open input devices or collect
input. Device names can contain trailing spaces; preserve them in selectors.

## Record activity

Choose the mouse name from the device listing. The default keyboard selector is
the virtual device named `keyboard-debouncer`. For example:

```sh
./target/release/actmon collect \
  --database /path/to/actmon/activity.db \
  --mouse-name 'USB Optical Mouse '
```

The database path is required. The collector creates its parent directory when
needed. SIGINT or SIGTERM requests an orderly stop and drains queued writes.
Do not point development runs at an installed collector's database.

The collector needs read access to the selected `/dev/input/event*` nodes. It
opens them read-only, never grabs them, and never falls back to the physical
keyboard. Configure permissions explicitly when deploying; installation and
permission management are not provided yet. Running as root is not required by
the design.

Selectors use exact names and refuse ambiguous matches. A missing or inaccessible
device is recorded as unavailable while the other source can keep collecting.
Hotplug discovery scans sysfs every two seconds and retries unavailable devices;
this first milestone uses periodic discovery rather than a udev monitor. Discovery
failures are reported separately and retried while existing readers continue;
reader errors still close the affected source. Recreated virtual devices are
reopened. There may be an unobserved period before discovery.

The collector holds an advisory lock next to the database to reject a second
Actmon collector targeting the same resolved path. The lock file is persistent;
the kernel releases its lock when a process exits. SQLite itself does not enforce
this application-level single-collector policy against unrelated writers.

Use `RUST_LOG=actmon=debug` to include transaction latency diagnostics on stderr.
Structured error messages contain source metadata, not input contents.

## Read status and measurements

```sh
./target/release/actmon status --database /path/to/actmon/activity.db
sqlite3 -readonly /path/to/actmon/activity.db
```

Status opens an existing database read-only and reports the newest run's latest
persisted interval and commit metadata. It does not create or migrate a database.
An absent orderly-stop timestamp does not prove that the collector is still
running: the process may have crashed. Check commit age and journal evidence.

Times use integer microseconds. `start_utc_us` is a UTC mapping of interval start;
`start_mono_us` uses Linux CLOCK_MONOTONIC. Intervals are half-open and normally
1,000,000 microseconds. First/last fields are offsets from interval start; add them
to `start_utc_us` for UTC endpoints. Runs retain boot identity, program version,
and effective source configuration. Source connection events retain node and
sysfs metadata.

Coverage masks use these bits:

| Bit | Category |
| --- | --- |
| 1 | Keyboard |
| 2 | Mouse buttons |
| 4 | Relative motion |
| 8 | Wheel scrolling |

`available_mask` identifies categories available for the entire interval.
`incomplete_mask` marks known loss or partial observation. A category's zero
count is evidence of observed inactivity only if its available bit is set and
its incomplete bit is clear. Queries must apply both conditions. Capability
absence, missing devices, and disabled categories are not healthy zero activity.

For example, inspect only complete keyboard intervals from the latest hour:

```sql
SELECT datetime(start_utc_us / 1000000, 'unixepoch') AS utc,
       key_downs, key_repeats, key_first_us, key_last_us
FROM intervals
WHERE start_utc_us >= (unixepoch('now') - 3600) * 1000000
  AND (available_mask & 1) = 1
  AND (incomplete_mask & 1) = 0
ORDER BY id;
```

Keyboard releases do not increment key-down counts. `key_repeats` is NULL when
repeat reporting is not advertised and no repeats were observed; it is not an
estimate of compositor-generated repeat. Mouse button columns follow Linux's
left, right, middle, side, extra, forward, back, and task button codes.

`moved` means nonzero relative X/Y motion occurred, not a full second of movement.
Wheel amounts use units of 1/120 nominal detent; legacy wheel steps are multiplied
by 120. High-resolution axes take precedence over duplicate legacy axes, per
axis. Up/down and right/left remain separate, so opposing scrolls do not cancel.
These values do not measure application pixels or lines.

## Coverage and durability

Input is captured independently of the SQLite writer. A one-second grace period
allows delayed boundary events to arrive before an interval is finalized.
Normally roughly one to two seconds remain in aggregation before being queued;
writer delay can add to that. Already-finalized intervals receiving late input
are conservatively marked incomplete rather than silently moving old input into
the current second.

Loss ranges also apply to intervals created or inserted in later batches. Source
coverage changes at exact interval boundaries do not create artificial partial
intervals. Loss and disconnect events only affect the source's supported
categories. Shutdown invalidates the unread tail since each source's last
successful drain, including any full interval crossed by that tail.

Raw evdev mode exposes SYN_DROPPED directly. The decoder ignores events through
the next SYN_REPORT, does not synthesize replacement keypresses, and marks the
potentially affected range incomplete. It conservatively includes the period
since the previous drained read. Clock changes, detected suspension through
CLOCK_BOOTTIME comparison, and scheduling gaps over two seconds reset interval
continuity and leave explicit gaps. These clock observations do not identify a
specific suspend cause or replace system journal evidence.

WAL with `synchronous=FULL` synchronizes completed transactions. Measurements
still in memory are not durable. The capture-to-writer queue holds 120 batches;
it is not a guaranteed number of seconds. Persistent write failures retry every
250 ms with a 250 ms SQLite busy timeout. When the queue fills, dropped batches
become an explicit loss range on the next accepted batch. Shutdown ends failed
write retries; storage failure at exit can leave only a journal explanation and
an unexplained database gap.

The latest commit timestamp is sampled before its successful transaction, so it
is an approximate freshness indicator. `latest_capture_to_commit_us` retains
the newest interval's measured age at the start of the transaction that inserted
it, excluding the commit itself. Event-only commits preserve this measurement
and its association with `latest_interval_utc_us`; it is not a continuously
updated age or a measurement of the event-only commit.
`previous_commit_latency_us` retains the preceding transaction's measured cost
without adding a second synchronization per batch. If a retry establishes that
an uncertain commit persisted, the next batch records that latency as NULL rather
than reusing an older transaction's timing. The recovery is logged separately.
No heartbeat or timestamp proves uninterrupted collection or power-loss recovery.

## Development and verification

```sh
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Tests use temporary databases and synthetic events or fixture metadata. They
exercise classification, wheel deduplication, boundaries, gaps, late arrivals,
queue saturation, transaction rollback and retry identity, collector locking,
orderly shutdown, and abrupt process exits. They do not read live input or alter
the keyboard debouncer. Killing a test process verifies application-crash
recovery; it does not simulate power loss or establish SSD flush behavior.

Live-device access, real suspend/resume, one-second commit power cost, and
long-duration collection still need verification on the host. The current
execution environment exposes sysfs metadata but no `/dev/input` nodes.
