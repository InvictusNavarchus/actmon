# Actmon

Actmon records Linux keyboard and mouse activity in a local SQLite database to
help investigate computer usage, inactivity, slowdowns, and failures alongside
Batmon and the system journal.

It keeps one-second summaries of key presses, mouse clicks, movement, and scrolling,
including first/last event timing and collection coverage. History is retained
permanently. It does not persist typed text, key identities, or pointer coordinates.

Recorded activity does not establish human presence or whether an application
responded. Missing or incomplete collection coverage must not be read as inactivity.

## Getting started

Requires Linux, Rust 1.89 or newer, and a C compiler for bundled SQLite.
Building Actmon does not install or start services.

### Install or update the command

From this checkout, run:

```sh
bash install.sh
actmon status
```

The script builds a native release with locked dependencies, checks the executable,
and atomically installs it at `/usr/local/bin/actmon`. Run it as your normal user;
it requests `sudo` only when the destination requires it. Run it again after source
changes to update. Rust and a C compiler are build requirements; the installed
command does not need Rust or the checkout.

To choose a different existing directory, use
`bash install.sh --bin-dir "$HOME/.local/bin"`. The script reports missing
PATH entries or another `actmon` taking precedence, without editing your shell.
It refuses to replace a symlink or a directory at the executable path.

Installation changes only the executable. Running services keep their existing
executable until restarted; service configuration, permissions, and recorded data
are preserved. The packaged services use `/usr/local/bin/actmon` regardless of
`--bin-dir`. See [systemd deployment](docs/systemd.md) for service setup and upgrades.

The previous `bash scripts/install.sh` entrypoint remains supported. The root
entrypoint is the recommended install/update command; both invoke the same logic.
System service installation is administrative, while collection runs unprivileged.
The collector receives input access through its service-specific supplementary
group, avoiding a global device-access grant to your login account. Lingering is
not required. Unit installation and first activation are separate steps.

### Build and find your devices

```sh
cargo build --locked --release
./target/release/actmon devices
```

The device listing prints JSON metadata without opening input devices or recording
input. Choose the exact mouse name, preserving any trailing spaces.

### Record activity

Replace the database path and mouse name below with your own values:

```sh
./target/release/actmon collect \
  --database /path/to/actmon/activity.db \
  --mouse-name 'USB Optical Mouse '
```

The keyboard defaults to the virtual device named `keyboard-debouncer`; use
`--keyboard-name` to select a different exact name. Ambiguous matches are refused.
The collector needs read access to the selected `/dev/input/event*` nodes and never
grabs devices exclusively. It does not change device permissions or fall back to
the physical keyboard when the selected keyboard is unavailable.

The database path is required; its parent directory is created when needed.
Use a separate database for development runs. Press Ctrl+C to stop cleanly and
drain queued writes. Missing or inaccessible devices are recorded as unavailable
while the other source can keep collecting.

For device permissions and collection as a system service, follow the
[systemd deployment guide](docs/systemd.md). The packaged `actmon.service` and
`actmon-backup.service` run as `invictus`, with data under
`/home/invictus/.local/share/actmon`. The installation flow enables daily backups
through `actmon-backup.timer`, with an explicit opt-out.

### Check status

```sh
./target/release/actmon status
./target/release/actmon status --database /path/to/actmon/activity.db
```

Status defaults to `$HOME/.local/share/actmon/activity.db`, matching Summary and
the standard service deployment. Use `--database` for a custom path. The default
requires an absolute `HOME`; an explicit path works without it. Missing databases
produce an error without creating files or directories.

Status reads an existing database and reports the latest persisted interval and
commit metadata as JSON. It does not prove the collector is still running; check
freshness and journal evidence. See [data model and queries](docs/data-model.md)
for interpreting coverage and inspecting measurements with SQL.

### Summarize today's activity

```sh
./target/release/actmon summary
./target/release/actmon summary --details
./target/release/actmon summary --json
./target/release/actmon summary --database /path/to/another/activity.db
```

Summary defaults to `$HOME/.local/share/actmon/activity.db`, matching the standard
service deployment. After installation, run `actmon summary` from any directory.
Use `--database` when your collector uses a custom path. The default requires an
absolute `HOME`; an explicit database path works without it. Missing databases
produce an error without creating files or directories.

The read-only report covers local midnight to now. It shows recorded input totals,
first/last input, minutes containing input, hourly activity, collection coverage,
and idle totals. Unknown coverage includes its total duration, gap count, and longest
gap. Use `--details` to list individual idle and unknown periods; `--json` always
includes all periods, regardless of `--details`.

Idle begins after five minutes of continuously observed quiet;
the first five minutes are excluded. Use `--idle-after-seconds 600` to change the
threshold (1–86400 seconds).

Idle measures absence of monitored input, including mouse movement and scrolling.
Reading or watching a video can count as idle. Missing or incomplete coverage stays
unknown and interrupts the idle timer. Summary does not change collection, create
a database, or persist idle classifications. See
[summary semantics](docs/data-model.md#daily-activity-and-idle-summary) for timing,
coverage, and boundary details.

## Backups

Create a verified local snapshot while the collector continues running:

```sh
./target/release/actmon backup \
  --database /path/to/actmon/activity.db \
  --destination /path/to/actmon/backups
./target/release/actmon backup-status --destination /path/to/actmon/backups
```

The destination's parent must already exist. Use a dedicated backup directory;
Actmon creates private snapshot and control directories beneath that parent.
**Upload only the published `backups/` directory**, not its parent or live database.
For Homekeep, select only that published directory. Actmon does not configure uploads.

Manual commands do not schedule backups, and old snapshots are not automatically
deleted. See [backups and recovery](docs/backups.md) for destination requirements,
verification, restoration, and retention.

## Documentation

- [Systemd deployment](docs/systemd.md): installation, device permissions, scheduling,
  upgrades, and removal.
- [Data model and queries](docs/data-model.md): timestamps, measurement definitions,
  coverage masks, and SQL examples.
- [Collector architecture](docs/architecture.md): device recovery, aggregation,
  loss handling, and durability.
- [Backups and recovery](docs/backups.md): snapshot creation, validation, status,
  restoration, and retention.
- [Development and verification](docs/development.md): checks, test coverage, and
  remaining deployment validation.
- [Design](docs/design.md): intended direction, rationale, and implementation milestones.
