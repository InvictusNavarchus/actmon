# Data model and queries

This document describes persisted measurements and how to interpret them. Activity
records describe observed input; they do not establish human presence, intent, or
whether an application responded. Commands below run from the repository root;
querying with `sqlite3` requires the SQLite CLI.

## Read status and measurements

```sh
./target/release/actmon status --database /path/to/actmon/activity.db
sqlite3 -readonly /path/to/actmon/activity.db
```

Status opens an existing database read-only and reports the newest run's latest
persisted interval and commit metadata. It does not create or migrate a database.
An absent orderly-stop timestamp does not prove that the collector is still
running: the process may have crashed. Check commit age and journal evidence.

## Timestamps and provenance

Times use integer microseconds. `start_utc_us` is a UTC mapping of interval start;
`start_mono_us` uses Linux CLOCK_MONOTONIC. Intervals are half-open and normally
1,000,000 microseconds. First/last fields are offsets from interval start; add them
to `start_utc_us` for UTC endpoints. Runs retain boot identity, program version,
and effective source configuration. Source connection events retain node and
sysfs metadata.

## Coverage masks

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

## Query example

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

## Measurement definitions

Keyboard releases do not increment key-down counts. `key_repeats` is NULL when
repeat reporting is not advertised and no repeats were observed; it is not an
estimate of compositor-generated repeat. Mouse button columns follow Linux's
left, right, middle, side, extra, forward, back, and task button codes.

`moved` means nonzero relative X/Y motion occurred, not a full second of movement.
Wheel amounts use units of 1/120 nominal detent; legacy wheel steps are multiplied
by 120. High-resolution axes take precedence over duplicate legacy axes, per
axis. Up/down and right/left remain separate, so opposing scrolls do not cancel.
These values do not measure application pixels or lines.

See [collector architecture](architecture.md#commit-timing-and-freshness) for the
meaning and limits of commit timing fields and freshness indicators.
