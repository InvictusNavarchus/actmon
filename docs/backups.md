# Local backups and recovery

Actmon creates verified local snapshots while collection continues. Commands
below run from the repository root. For service installation and daily scheduling,
see [systemd deployment](systemd.md).

## Create a snapshot

Create a snapshot while the collector continues running:

```sh
./target/release/actmon backup \
  --database /path/to/actmon/activity.db \
  --destination /path/to/actmon/backups
./target/release/actmon backup-status --destination /path/to/actmon/backups
```

## Destination and upload directory

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

## Snapshot contents and validation

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

## Deadlines, cancellation, and failures

`--timeout-seconds` defaults to 300 (allowed range 1–86400). The deadline and
SIGINT/SIGTERM are checked between copy/hash steps and during SQLite validation.
They are cooperative limits: an individual filesystem operation, flush, or SQLite
step can exceed them. The last cancellation/deadline check is immediately before
publication begins. Cancellation observed before that boundary exits nonzero,
preserves earlier snapshots, and cleans only its own staging. Once publication
begins, Actmon completes directory synchronization and records the outcome even
if cancellation or the deadline arrives meanwhile; successful publication remains
a success. Actual publication, synchronization, or state-write failures still exit
nonzero. A hard kill can leave staging behind;
Actmon does not automatically remove unknown stale directories. A sync failure
after rename leaves the complete visible artifact in place and reports uncertain
publication durability. There is no power-loss hardware guarantee from these tests.

## Backup status

A per-destination lock rejects overlapping jobs. `backup-status` distinguishes
last attempt, its finish/error, and the last verified local publication with age
and coverage. It reads metadata without rechecking snapshots or cloud state.
An unfinished attempt after a crash remains unfinished; a snapshot published
just before a crash may not yet appear in last-success state. Inspect and verify
that artifact directly. Preflight failures (missing source, invalid paths, or lock
contention) are reported on stderr before attempt state is updated. Diagnostics
use stderr; successful commands emit JSON on stdout. The packaged systemd jobs
send both streams to the journal when deployed.

## Retention and scheduling

Seven successful daily snapshots remain the retention target. This command
reports snapshot directory count and excess over seven, **but deletes nothing**:
there is no reader/cleanup coordination contract with the independent uploader.
Repeated manual invocations create additional snapshots, even on the same day.
Arrange manual cleanup only when no uploader or restore reader uses those files.
Pre-migration snapshots and their separate expiry policy will be integrated when
versioned schema migrations are introduced.

The CLI does not install or schedule jobs. The [systemd installation flow](systemd.md)
enables a daily timer by default, with a destination override and explicit opt-out.
Until that flow is carried out, backups run only on explicit invocation.

## Restore rehearsal

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
