# Actmon design

Actmon is a small Linux activity recorder for investigating computer usage,
inactivity, slowdowns, and failures alongside Batmon and the system journal. It
records input measurements and collection coverage in a local SQLite database.
Measurements describe observed device events; they do not establish human
presence, intent, or whether an application responded.

This document captures the agreed direction as of 2026-11-15. Implementation
choices and verification requirements below are proposals within that direction,
not claims about an existing implementation.

## Decisions

| Area | Decision |
| --- | --- |
| Language | Rust, edition 2024 |
| Deployment | Independent daemon, managed by systemd |
| Input interface | Linux evdev, using ordinary readers without exclusive grabs |
| Keyboard source | The virtual keyboard produced by keyboard-debouncer |
| Primary devices | Keyboard and mouse; touchpad support is optional and separately identified |
| Measurement resolution | One-second summaries with first and last event timestamps per category |
| Retention | Permanent interval and lifecycle history |
| Database | SQLite through rusqlite, WAL mode, synchronous=FULL |
| Commit cadence | Each completed one-second interval |
| Collection lifetime | While awake, independent of KDE login and lock state |
| Context | Collector and device health, boot identity, lightweight suspend/resume observations |
| Backups | Automatic daily verified local snapshots in a configurable directory |
| Uploads | User-managed rclone; no cloud integration or upload scheduling in Actmon |

## Scope

The initial recorder should answer when monitored input occurred, how many key
presses or clicks were observed, when input stopped, and whether collection was
complete enough to interpret a quiet interval. Its timestamps allow correlation
with Batmon measurements and journal evidence.

Actmon remains independent of Batmon's availability, battery detection, schema,
and sampling loop. The collectors do not need synchronized timers: investigation
queries correlate timestamps and overlapping observation intervals.

The initial scope excludes typed text, persisted key identities, application or
window tracking, cursor coordinates, human-presence classification, and permanent
storage of every input event. It also excludes changes to Batmon. A future
short-retention detailed recorder would require its own measurement and storage
contract.

## Input sources

The normal keyboard path is:

```text
Physical keyboard
    -> keyboard-debouncer, holding an exclusive grab
    -> virtual keyboard named keyboard-debouncer
         -> KDE
         -> Actmon, using an ordinary reader
```

The debouncer's grab prevents Actmon from receiving new events directly from the
physical keyboard. The virtual device can have multiple ordinary readers, so
Actmon can observe its output without interfering with KDE. Counts therefore
describe post-debounce events, not physical switch transitions or discarded
chatter. Opening an input device successfully does not by itself establish that
events are reaching the reader. See the [evdev grab
documentation](https://docs.rs/evdev/latest/evdev/struct.Device.html#method.grab).

Actmon must never acquire an exclusive grab. Select sources using device metadata
and explicit configuration, rather than treating an event node number as stable.
Record each selected source and its connection lifetime. Device matching must
avoid counting both a physical device and a virtual replacement as independent
user actions.

When the debouncer's virtual keyboard disappears, keyboard coverage becomes
unavailable while mouse collection continues. Rediscover and reopen the virtual
device when it returns. Do not silently fall back to the physical keyboard:
that would change measurement semantics and could overlap during transitions.

Touchpad activity is optional. If excluded, touchpad-only use is outside recorded
coverage. If included later, distinguish raw contact or motion from interpreted
clicks and scrolling. A separate libinput instance would observe its own
interpretation, not prove that KDE processed the same action. See the
[libinput architecture](https://wayland.freedesktop.org/libinput/doc/latest/what-is-libinput.html).

## Measurements

Listen continuously and aggregate events into intervals. Sampling a device's
current state once a second would miss transitions and is not the design.

| Category | Persisted measurements |
| --- | --- |
| Keyboard | Key-down count; separately counted autorepeat when observable; first and last key-down timestamps |
| Mouse buttons | Button-down counts by supported button; first and last button-down timestamps |
| Mouse motion | Whether motion occurred; first and last motion timestamps |
| Mouse wheel | Horizontal and vertical amounts, with opposite directions accumulated separately; first and last scroll timestamps |
| Coverage | Source availability, partial intervals, and detected loss for each category |

Keyboard releases do not count as new presses. Autorepeat is not a new physical
keypress, and an unsupported or unavailable repeat measurement must not silently
be presented as a measured zero. Input classification must distinguish keyboard
keys, pointer buttons, and unrelated switch events.

For wheel measurements, define source units and any normalization explicitly.
Do not equate wheel units with application lines or screen pixels. Devices that
provide both legacy and high-resolution wheel events must not be counted twice.
Opposing movement must not cancel into a misleading zero-activity interval. The
[Linux input event documentation](https://docs.kernel.org/input/event-codes.html)
defines presses, releases, repeats, event frames, and wheel representations.

A motion flag means movement was observed somewhere in the interval, not that
the pointer moved for the entire interval. Motion-report counts are not a primary
usage metric because device reporting behavior affects them.

First and last timestamps improve temporal precision without retaining every
event. An interval with three key-downs at 10:00:05.120, an unknown middle time,
and 10:00:05.870 preserves the count and endpoints. It cannot reconstruct all
pauses inside that second. Event timestamp precision is not a guarantee of
physical timing accuracy.

## Time and interval semantics

Use explicit half-open intervals: an event at the end boundary belongs to the
next interval. Record actual boundaries and observed duration; do not label a
delayed or interrupted interval as a complete ordinary second.

Store UTC for correlation, boot ID for machine restarts, and a collector run ID
for executions within a boot. Use a documented monotonic clock domain for event
ordering and elapsed-time calculations. Monotonic values are meaningful only
within their associated boot and clock domain.

The implementation should configure a known evdev timestamp clock and verify
alignment with its interval clock. Preserve wall-clock/monotonic mappings across
clock adjustments; do not calculate inactivity solely by subtracting UTC values.
Use clock observations that distinguish elapsed suspension where needed. The
exact Linux clock selection is an implementation decision to document and test.

Missing time is never backfilled with zero-input rows. Boundaries and coverage
must represent collector delays, source transitions, suspension, and downtime.
Define how delayed events affect finalized intervals: do not silently place old
input into the current second or count reconstructed state as observed input.

## Coverage and lifecycle

Track coverage per selected source and category. A healthy mouse reader cannot
establish keyboard coverage. Zero is meaningful only when the relevant source
was monitored without a known interruption or detected loss.

| Condition | Recording behavior |
| --- | --- |
| Reader operating, no detected loss | Record measurements, including zero |
| Device absent or inaccessible | Record source unavailability |
| Device transition within an interval | Record the transition and partial coverage |
| Kernel event loss or queue overflow | Mark affected measurements incomplete |
| Collector downtime | Leave a gap, associated with distinct runs where observable |
| Suspend/resume observed | Record lifecycle observations and interrupt interval continuity |
| Lifecycle metadata unavailable | Continue input collection with unknown context |

Availability is an observation, not proof that no other process has diverted
events with an exclusive grab. In particular, silence alone cannot establish
end-to-end input health.

Handle SYN_DROPPED explicitly. The evdev synchronizing API can synthesize events
to recover device state; these must not inflate observed input counts. Recovery
restores usable state but cannot recover lost historical events. See the
[evdev stream documentation](https://docs.rs/evdev/latest/evdev/).

Preserve collector starts, orderly stops, device lifetimes, source changes, and
known collection failures in the database. An abrupt exit may have no stop event.
Store configuration and measurement-version information with each run so later
queries can distinguish changed definitions.

Retain lightweight suspend/resume observations when directly available. Treat
notifications as observations, not exact proof of the hardware sleep boundary.
Dedicated lock/unlock tracking is secondary and can follow the initial collector;
comprehensive login/session history is outside the initial scope.

The journal supplies detailed errors, service failures, and system-state
explanations. The database retains the compact context needed to interpret its
measurements even after journal retention expires. An old unexplained gap remains
unknown rather than being reclassified as inactivity.

## Runtime structure

Start with one Cargo package and executable. Separate responsibilities internally:

1. Input capture and in-memory aggregation continuously drain the selected devices.
2. A bounded queue carries completed measurements to a dedicated database writer.
3. Source discovery and lightweight lifecycle observation report transitions.

The writer separation prevents a brief SQLite stall from immediately blocking
event collection. Buffering remains bounded: prolonged stalls can still cause
loss, which must be accounted for. Do not introduce a shared framework with
Batmon or choose an async runtime solely for consistency with other projects.

| Responsibility | Proposed library or interface |
| --- | --- |
| Input | evdev |
| Discovery and hotplug | udev |
| SQLite | rusqlite with bundled SQLite |
| CLI | clap |
| UTC handling | jiff |
| Linux clocks and descriptor operations | A suitable safe Linux interface such as rustix |
| Lifecycle observation | zbus where D-Bus is needed |
| Diagnostics | tracing and tracing-subscriber, written to the journal |

Select current compatible dependency versions during implementation. Candidate
commands are collect, status, devices, and backup; names and flags remain an
implementation detail.

## Storage and durability

Use one local database with logical records for runs, source lifetimes, intervals,
and lifecycle or coverage events. A compact combined interval row with source
provenance is the starting schema direction. Exact columns, indexes, and
migrations should follow the measurement contract rather than freezing the
temporary sizing schema as production SQL.

Keep history permanently. Do not silently downsample or delete measurements when
space runs low. A separately authorized export or retention change can be added
later.

Configure WAL and synchronous=FULL on the writer. Commit each completed second
and define a bounded persistence path for lifecycle events. FULL strengthens
durability of completed commits when the filesystem and storage honor
synchronization. It does not preserve uncommitted input or guarantee at most one
second of loss when the writer is delayed. A page-count checkpoint threshold is
not a time-based durability guarantee. See [SQLite synchronization
semantics](https://www.sqlite.org/pragma.html#pragma_synchronous).

Track the latest successful commit and writer backlog. Retry transient storage
failures with bounded buffering and backoff; expose persistent failures through
status and the journal. Once storage recovers, persist known loss ranges where
available. A crash during a storage outage may leave only an unexplained gap.

Checkpoint management must account for long-running readers and WAL growth.
Do not assume WAL prevents all lock contention or disk-full failures.

## Service and permissions

The proposed deployment is a systemd system service under a dedicated
unprivileged account, with read access to selected input devices and write access
to its data directory. It starts independently of KDE and restarts after failure.
Install-time permission configuration must cover device recreation. The running
collector does not need exclusive grabs or unrestricted root privileges.

Resolve concrete data/configuration paths and user read access during packaging.
Collect while locked, logged out, or on a text console when configured sources
are available. Coverage begins when the service and sources are ready; firmware
screens and the earliest boot stages are not covered. Counts identify source
activity, not which person generated it.

## Local backups and recovery

Enable daily local snapshots by default, with a configurable destination and an
explicit opt-out. A dedicated systemd timer should invoke the backup command,
including a catch-up attempt after missed scheduling. Avoid overlapping backup
jobs. Collection continues if a backup fails.

Use the SQLite Online Backup API through the application's database stack, with
separate connections, incremental work, bounded retries, and a completion
deadline. A live copy of only the database file is not a backup protocol: committed
data may reside in the WAL. Source changes can restart an incremental backup, so
record snapshot coverage and completion instead of promising it represents the
initial start instant. See the [SQLite backup API](https://www.sqlite.org/backup.html).

The publication sequence is:

1. Create private staging outside the published backup directory, on the same
   filesystem as the final destination.
2. Capture the SQLite snapshot and required configuration/version metadata.
3. Validate database integrity, schema identity, and applicable foreign keys;
   record checksums and the snapshot's measurement coverage.
4. Synchronize files, publish under a unique name with atomic rename, and
   synchronize the relevant directories.
5. Report successful local publication; leave published artifacts immutable.

The user adds the published directory to an existing daily rclone job. Actmon
does not configure rclone, Google Drive, or upload timing, and local success is
not evidence of remote completion. The accepted disaster-recovery priority is
daily backups; loss of roughly a day of history after SSD failure is acceptable.
No remote recovery-age guarantee is made by Actmon.

Seven successful daily snapshots are the initial local retention target. Also
create a verified recovery snapshot before schema migration; its expiry must
follow an explicit policy rather than counting accidentally as another daily
snapshot. Preserve previous successful backups after a failed attempt.

Automatic deletion must not race an external reader. Finalize deletion ownership
and a safe reader/cleanup contract before enabling rotation; the existence of a
new snapshot alone does not prove an older upload has finished. If no contract
is established, preserve published snapshots and expose excess retention rather
than claiming that automatic pruning is safe. This boundary does not authorize
changes to the user's uploader.

Status should distinguish last attempt, last verified local snapshot, its age and
coverage, and the last backup error. Restore into a separate location, verify
checksums and database contents, and exercise application reads before replacing
live data. Preserve the original backup. An authorized live replacement must stop
writers and avoid pairing restored data with stale WAL or journal sidecars.

## Capacity estimate

A preliminary experiment inserted 604,800 intervals per profile, representing
seven days, into SQLite 3.51.2. The proposed schema used one combined row per
second, integer microsecond UTC and monotonic timestamps, within-interval offsets
for first/last events, timestamp indexes, and a synthetic lifecycle event each
hour. Idle, mixed, and active profiles occupied approximately 72–85 bytes per
interval including those indexes and sparse metadata.

| Collection schedule | Estimated live growth per year | Live data plus seven full snapshots near year end |
| --- | --- | --- |
| 8 hours daily | 0.7–0.85 GiB | 5.7–6.6 GiB |
| 24 hours daily | 2.1–2.5 GiB | 17–20 GiB |

Collection includes awake idle time. Budget approximately 1 GiB/year at eight
hours daily or 3 GiB/year continuously for live history, plus backups. These sizes
were accepted as practical for this project.

The estimate excludes WAL, publication staging, migration snapshots, additional
source metadata, and future schema changes. It extrapolates a short synthetic
dataset; integer-width growth and different distributions can change the result.
Full snapshots grow with all retained history, so backup duration and validation
cost also increase over time. No compression or deduplication savings are assumed.

The experiment used batches of 10,000 inserts and is only a capacity estimate.
It does not measure the latency, power consumption, or crash recovery of actual
one-second FULL commits. All three generated databases passed quick_check;
that is not a substitute for backup integrity validation or restore testing.

## Verification and implementation checklist

- Test press/release/repeat classification, mouse buttons, wheel normalization,
  direction preservation, and exact interval-boundary assignment.
- Test missing categories, source changes, hotplug, debouncer recreation, and
  recovery after SYN_DROPPED without counting synthetic recovery events.
- Test clock adjustments, delayed events, partial intervals, and observable
  suspend boundaries without fabricating idle history.
- Verify capture continues through brief writer delays; test queue saturation,
  disk-full errors, retries, and honest recording of known loss.
- Test interrupted snapshot creation, backup contention, validation failures,
  publication, retention safety, and restoration independently of live files.
- Validate schema migrations on disposable fixtures and recovered copies.
- Measure commit latency, collection lag, CPU, memory, and battery impact under
  the intended one-second workload before changing durability policy.
- Review long-term database/WAL growth and backup cost with a larger dataset.
- Verify device permissions and lifecycle behavior on this KDE Wayland laptop.
  Live service changes and disruptive power-loss tests require separate scope.

Before implementation, finalize the production schema and clock model, source
matching rules, buffering limits, service paths, and backup deletion contract.
These choices must preserve the measurement, durability, and coverage decisions
above. Actmon development does not include changing or deploying Batmon.
