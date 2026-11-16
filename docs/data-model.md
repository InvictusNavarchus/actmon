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

## Daily activity and idle summary

`actmon summary` defaults to `$HOME/.local/share/actmon/activity.db`; use
`--database PATH` to read a custom location. The default requires an absolute
`HOME`, and an explicit path bypasses home-directory lookup. Missing databases
are not created. Summary reads an existing v1 Actmon database in a single
read transaction, including committed WAL data. It reports local midnight to the
time the command started, using the process's local timezone (including `TZ` when
set). Text is the default; `--json` provides RFC 3339 timestamps with UTC offsets
and integer microsecond durations. Text shows the date and UTC offset once in the
header, with time-only labels throughout the report. If the offset changes during
the day, the header shows the starting and ending offsets; JSON retains offsets
on each timestamp to distinguish repeated hours. A nonexistent local midnight
produces an explicit error rather than guessing a boundary.

Input totals include key downs, separately observed repeats, all recorded mouse
button downs, and counts of intervals containing motion or scrolling. First/last
input uses stored event offsets when no repeats are present. Any interval containing
repeats uses its endpoints conservatively, even when other input has precise
offsets, because repeat events have no stored first/last offsets.
NULL repeat measurements are counted as intervals with unknown repeat reporting;
the observed repeat total does not imply those intervals had zero repeats.

Minutes containing input are distinct minute buckets with an input-bearing
interval, assigned using the interval's start. Counts and input minutes are also
assigned to the starting hour. This can shift attribution by up to one second
at minute/hour boundaries. Aggregate counts cannot be split exactly at midnight
or the report endpoint, so counts and first/last input from boundary-straddling
intervals are omitted. Coverage durations are clipped to the window, and hourly
coverage and idle durations are split at hour boundaries.

Idle begins after `--idle-after-seconds` of continuous observed quiet (default
300); only time after that grace period contributes to idle duration. For example,
input at 10:00 followed by input at 10:20 produces an idle period from 10:05 to
10:20. Key repeats, clicks, movement, and scrolling all reset the timer. At the
start of a recording, quiet observation starts the timer even if no earlier input
was recorded. The command reads a bounded lookback before midnight to establish
whether an existing quiet stretch already qualifies, then clips idle to today.
An ongoing idle period stops at the latest recorded interval, never extrapolating
through uncommitted time to the current clock.

Idle requires complete coverage of keyboard, buttons, relative motion, and wheel
scrolling (mask 15). A device lacking a category cannot establish overall idle;
per-category coverage is still reported. Missing rows, incomplete/unavailable
categories, run changes, discontinuous monotonic intervals, and UTC discontinuities
reset the quiet timer. Overlapping intervals from clock corrections or multiple
runs remain unknown for coverage and idle; their recorded counts are still shown.
Retrospective loss is reflected in persisted incomplete masks by the writer.

Complete coverage plus unknown duration accounts for the report window. Unknown
periods include time before collection began and after its newest committed row,
as well as partial or absent collection; they do not identify a cause such as
suspension. Input can still be observed in an incomplete interval, so observed
counts may undercount and can coexist with unknown coverage. Idle duration and
minutes containing input are different metrics and must not be added to estimate
working time. Reading, video playback, other unmonitored devices, or holding a key
without reported repeats cannot be distinguished from absence of monitored input.
