# Development and verification

Run these checks from the repository root. See the [README](../README.md) for build
requirements and basic usage, and the [collector architecture](architecture.md)
for implemented behavior.

## Checks

```sh
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

## Test coverage

Tests use temporary databases and synthetic events or fixture metadata. They
exercise classification, wheel deduplication, boundaries, gaps, late arrivals,
queue saturation, transaction rollback and retry identity, collector locking,
orderly shutdown, abrupt process exits, live-WAL snapshots, restoration without
the original database, backup failures, publication collisions, and retention
preservation. They do not read live input or alter
the keyboard debouncer. Killing a test process verifies application-crash
recovery; it does not simulate power loss or establish SSD flush behavior.

Summary tests cover idle thresholds, input endpoints, repeat-only activity,
per-category coverage, incomplete input counts, gaps and restarts, overlapping
recordings, midnight lookback, clipped boundaries, hourly durations, partial-hour
timezone transitions, and read-only live-WAL queries with retrospective loss.
CLI checks cover text/JSON reporting and
invalid thresholds.

## Offline packaging validation

The systemd templates and device rules can be checked without deployment:

```sh
bash scripts/verify-packaging.sh
```

## Remaining deployment validation

Runtime service permissions, reconnects, real suspend/resume, one-second commit
power cost, and long-duration collection still need deployment validation. A prior
foreground smoke test recorded normal keyboard/mouse activity successfully; it
does not establish behavior under the packaged service sandbox.
