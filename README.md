# Actmon

Actmon records Linux keyboard and mouse activity in SQLite for investigations
alongside Batmon and the system journal. It preserves one-second measurements
permanently, including collection coverage and first/last event timing. It does
not persist key identities, typed text, or pointer coordinates.

The collector and verified local backup commands are implemented. Daily backup
scheduling, systemd installation, direct suspend notifications, and optional
session metadata remain later work;
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

## Local snapshots and recovery

Create a snapshot while the collector continues running:

```sh
./target/release/actmon backup \
  --database /path/to/actmon/activity.db \
  --destination /path/to/actmon/backups
./target/release/actmon backup-status --destination /path/to/actmon/backups
```

The destination's parent must already exist. Actmon creates the destination and
its sibling `.backups.actmon-backup` control directory with mode 0700; existing
ones must be private real directories, not symlinks. Use a dedicated destination
for one canonical database path. Its parent must be trusted and writable by the
backup user. A destination containing the live database is refused. Staging and
publication must be on the same local filesystem; cross-device publication fails
without falling back to copying into the published directory.

**Upload only the published `backups/` directory**, not its parent. The sibling
control directory contains the lock, attempt state, and temporary staging. No
rclone configuration or cloud interaction is performed by Actmon.

Each successful invocation publishes a unique `snapshot-*/` directory containing:

- `activity.db`: a standalone snapshot including committed WAL data, converted to
  DELETE journal mode so it needs no WAL/SHM sidecars. Collector configuration,
  boot IDs, and collector versions remain in its `runs` records.
- `manifest.json`: format/schema and tool/SQLite versions, source path, validation
  timestamp, database size, SHA-256 checksum, and interval count/time bounds.
  Time bounds do not imply continuous or complete input coverage; consult the
  interval masks and events inside the snapshot.

The command uses incremental SQLite Online Backup steps, validates identity,
`integrity_check`, foreign keys, and application reads, then synchronizes files
and directories and publishes with a no-replacement atomic rename. Published
artifacts are never edited. Source writes may restart copying; the snapshot is
not promised to represent the invocation's start time. Checksums detect corruption,
not malicious replacement of both the database and manifest.

`--timeout-seconds` defaults to 300 (allowed range 1–86400). The deadline and
SIGINT/SIGTERM are checked between copy/hash steps and during SQLite validation.
They are cooperative limits: an individual filesystem operation, flush, or SQLite
step can exceed them. Failed or cancelled work exits nonzero, preserves earlier
snapshots, and cleans only its own staging. A hard kill can leave staging behind;
Actmon does not automatically remove unknown stale directories. A sync failure
after rename leaves the complete visible artifact in place and reports uncertain
publication durability. There is no power-loss hardware guarantee from these tests.

A per-destination lock rejects overlapping jobs. `backup-status` distinguishes
last attempt, its finish/error, and the last verified local publication with age
and coverage. It reads metadata without rechecking snapshots or cloud state.
An unfinished attempt after a crash remains unfinished; a snapshot published
just before a crash may not yet appear in last-success state. Inspect and verify
that artifact directly. Preflight failures (missing source, invalid paths, or lock
contention) are reported on stderr before attempt state is updated. Diagnostics
use stderr; successful commands emit JSON on stdout. A future systemd job will
capture stderr in the journal.

Seven successful daily snapshots remain the retention target. This command
reports snapshot directory count and excess over seven, **but deletes nothing**:
there is no reader/cleanup coordination contract with the independent uploader.
Repeated manual invocations create additional snapshots, even on the same day.
Arrange manual cleanup only when no uploader or restore reader uses those files.
Pre-migration snapshots and their separate expiry policy will be integrated when
versioned schema migrations are introduced.

This milestone does not install or schedule jobs. Until systemd packaging lands,
backups run only on explicit invocation. The planned daily timer will be enabled
by default on installation, with a destination override and explicit opt-out.

To rehearse restoration, copy the **whole chosen snapshot** into a new private
working directory; never open the published database for collection:

```sh
mkdir -m 700 /path/to/restore-test
cp /path/to/actmon/backups/snapshot-EXAMPLE/activity.db /path/to/restore-test/
cp /path/to/actmon/backups/snapshot-EXAMPLE/manifest.json /path/to/restore-test/
./target/release/actmon verify-backup --snapshot /path/to/restore-test
./target/release/actmon status --database /path/to/restore-test/activity.db
```

Verification rejects missing files, symlinked payloads, checksum/size mismatches,
SQLite sidecars, unsupported identities, corruption, foreign-key violations, and
coverage metadata mismatches. Keep the published snapshot unchanged. Once the
restored working database receives writes, its original manifest no longer matches.
Live replacement is a separate operation: stop writers, preserve the old dataset,
and never pair the restored database with old WAL/SHM/journal files. Local
verification does not establish that an off-device upload completed.

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
orderly shutdown, abrupt process exits, live-WAL snapshots, restoration without
the original database, backup failures, publication collisions, and retention
preservation. They do not read live input or alter
the keyboard debouncer. Killing a test process verifies application-crash
recovery; it does not simulate power loss or establish SSD flush behavior.

Live-device access, real suspend/resume, one-second commit power cost, and
long-duration collection still need verification on the host. The current
execution environment exposes sysfs metadata but no `/dev/input` nodes.
