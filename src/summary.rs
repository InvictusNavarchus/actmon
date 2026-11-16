//! Read-only daily reporting. Idle is derived from continuous complete coverage.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    ops::Bound::{Excluded, Unbounded},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Local, Offset, TimeZone, Timelike, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;

use crate::model::{BUTTON, KEYBOARD, MOTION, SECOND, WHEEL};

const ALL_INPUT: u8 = KEYBOARD | BUTTON | MOTION | WHEEL;

#[derive(Debug, Serialize)]
pub struct Period {
    pub start: String,
    pub end: String,
    pub duration_us: i64,
}

#[derive(Debug, Default, Serialize)]
pub struct InputCounts {
    pub key_presses: i64,
    pub observed_key_repeats: i64,
    pub mouse_clicks: i64,
    pub intervals_with_motion: i64,
    pub intervals_with_scroll: i64,
}

#[derive(Debug, Serialize)]
pub struct Hour {
    pub start: String,
    pub counts: InputCounts,
    pub minutes_containing_input: usize,
    pub complete_coverage_us: i64,
    pub idle_us: i64,
}

#[derive(Debug, Serialize)]
pub struct Coverage {
    pub category: &'static str,
    pub complete_us: i64,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub database: String,
    pub start: String,
    pub end: String,
    pub idle_after_seconds: u64,
    pub recorded_intervals: usize,
    pub counts: InputCounts,
    pub repeat_reporting_unknown_intervals: usize,
    pub first_input: Option<String>,
    pub last_input: Option<String>,
    pub minutes_containing_input: usize,
    pub complete_coverage_us: i64,
    pub unknown_us: i64,
    pub coverage: Vec<Coverage>,
    pub idle_us: i64,
    pub longest_idle_us: i64,
    pub idle_periods: Vec<Period>,
    pub unknown_periods: Vec<Period>,
    pub hours: Vec<Hour>,
}

struct Row {
    run: i64,
    mono: i64,
    start: i64,
    duration: i64,
    available: u8,
    incomplete: u8,
    keys: i64,
    repeats: Option<i64>,
    clicks: i64,
    moved: bool,
    scrolled: bool,
    first: Option<i64>,
    last: Option<i64>,
}

impl Row {
    fn end(&self) -> i64 {
        self.start + self.duration
    }

    fn has_input(&self) -> bool {
        self.keys > 0
            || self.repeats.unwrap_or(0) > 0
            || self.clicks > 0
            || self.moved
            || self.scrolled
    }

    fn complete(&self, mask: u8) -> bool {
        self.available & mask == mask && self.incomplete & mask == 0
    }
}

impl InputCounts {
    fn add(&mut self, row: &Row) {
        self.key_presses += row.keys;
        self.observed_key_repeats += row.repeats.unwrap_or(0);
        self.mouse_clicks += row.clicks;
        self.intervals_with_motion += i64::from(row.moved);
        self.intervals_with_scroll += i64::from(row.scrolled);
    }
}

fn timestamp<Tz: TimeZone>(us: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    DateTime::from_timestamp_micros(us)
        .expect("validated timestamp")
        .with_timezone(tz)
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}

fn period<Tz: TimeZone>(start: i64, end: i64, tz: &Tz) -> Period
where
    Tz::Offset: std::fmt::Display,
{
    Period {
        start: timestamp(start, tz),
        end: timestamp(end, tz),
        duration_us: end - start,
    }
}

fn add_duration(
    hours: &mut BTreeMap<i64, Hour>,
    from: i64,
    to: i64,
    field: fn(&mut Hour) -> &mut i64,
) {
    let mut cursor = from;
    while cursor < to {
        let stop = hours
            .range((Excluded(cursor), Unbounded))
            .next()
            .map_or(to, |(boundary, _)| to.min(*boundary));
        let (_, hour) = hours
            .range_mut(..=cursor)
            .next_back()
            .expect("hourly buckets cover the report window");
        *field(hour) += stop - cursor;
        cursor = stop;
    }
}

pub fn today(path: &Path, idle_after_seconds: u64) -> Result<Summary> {
    let now = Local::now();
    let midnight = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let start = Local
        .from_local_datetime(&midnight)
        .earliest()
        .context("local midnight does not exist in this timezone")?;
    summarize(
        path,
        start.timestamp_micros(),
        now.timestamp_micros(),
        idle_after_seconds,
        &Local,
    )
}

fn summarize<Tz: TimeZone>(
    path: &Path,
    start: i64,
    end: i64,
    idle_after_seconds: u64,
    tz: &Tz,
) -> Result<Summary>
where
    Tz::Offset: std::fmt::Display,
{
    ensure!(
        (1..=86400).contains(&idle_after_seconds),
        "idle threshold must be 1..=86400 seconds"
    );
    ensure!(start <= end, "summary window ends before it starts");
    let threshold = idle_after_seconds as i64 * SECOND;
    let lookback = start
        .checked_sub(threshold + SECOND)
        .context("window underflow")?;
    ensure!(
        DateTime::<Utc>::from_timestamp_micros(lookback).is_some()
            && DateTime::<Utc>::from_timestamp_micros(end).is_some(),
        "invalid summary timestamps"
    );
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open existing database {}", path.display()))?;
    connection.busy_timeout(Duration::from_millis(250))?;
    // One snapshot prevents a concurrent collector commit from mixing coverage states.
    let tx = connection.transaction()?;
    let application: i64 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        application == 0x4143544d && version == 1,
        "unsupported Actmon database"
    );
    let mut stmt = tx.prepare(
        "SELECT run_id,start_mono_us,start_utc_us,duration_us,available_mask,incomplete_mask,
         key_downs,key_repeats,
         button_left+button_right+button_middle+button_side+button_extra+button_forward+button_back+button_task,
         moved, wheel_up+wheel_down+wheel_right+wheel_left,
         key_first_us,button_first_us,motion_first_us,scroll_first_us,
         key_last_us,button_last_us,motion_last_us,scroll_last_us
         FROM intervals WHERE start_utc_us >= ?1 AND start_utc_us < ?2
         ORDER BY start_utc_us,run_id,start_mono_us",
    )?;
    let rows: Vec<Row> = stmt
        .query_map(params![lookback, end], |r| {
            let mut first = Vec::new();
            let mut last = Vec::new();
            for index in 11..15 {
                first.extend(r.get::<_, Option<i64>>(index)?);
                last.extend(r.get::<_, Option<i64>>(index + 4)?);
            }
            Ok(Row {
                run: r.get(0)?,
                mono: r.get(1)?,
                start: r.get(2)?,
                duration: r.get(3)?,
                available: r.get(4)?,
                incomplete: r.get(5)?,
                keys: r.get(6)?,
                repeats: r.get(7)?,
                clicks: r.get(8)?,
                moved: r.get(9)?,
                scrolled: r.get::<_, i64>(10)? > 0,
                first: first.into_iter().min(),
                last: last.into_iter().max(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    tx.commit()?;
    build(path, start, end, idle_after_seconds, &rows, tz)
}

fn build<Tz: TimeZone>(
    path: &Path,
    start: i64,
    end: i64,
    idle_after_seconds: u64,
    rows: &[Row],
    tz: &Tz,
) -> Result<Summary>
where
    Tz::Offset: std::fmt::Display,
{
    let mut report = Summary {
        database: path.display().to_string(),
        start: timestamp(start, tz),
        end: timestamp(end, tz),
        idle_after_seconds,
        recorded_intervals: 0,
        counts: InputCounts::default(),
        repeat_reporting_unknown_intervals: 0,
        first_input: None,
        last_input: None,
        minutes_containing_input: 0,
        complete_coverage_us: 0,
        unknown_us: 0,
        coverage: [
            (KEYBOARD, "keyboard"),
            (BUTTON, "buttons"),
            (MOTION, "motion"),
            (WHEEL, "scroll"),
        ]
        .map(|(_, category)| Coverage {
            category,
            complete_us: 0,
        })
        .into(),
        idle_us: 0,
        longest_idle_us: 0,
        idle_periods: Vec::new(),
        unknown_periods: Vec::new(),
        hours: Vec::new(),
    };
    let mut hours = BTreeMap::new();
    let mut minute = start.div_euclid(60 * SECOND) * 60 * SECOND;
    let mut previous_hour = None;
    while minute < end {
        let local = DateTime::from_timestamp_micros(minute)
            .expect("validated timestamp")
            .with_timezone(tz);
        let identity = (local.date_naive(), local.hour(), local.offset().fix());
        if previous_hour != Some(identity) {
            // Keep real UTC boundaries. Rounding back a local minute count can
            // collide when an offset transition happens partway through an hour.
            hours.insert(
                minute,
                Hour {
                    start: timestamp(minute, tz),
                    counts: InputCounts::default(),
                    minutes_containing_input: 0,
                    complete_coverage_us: 0,
                    idle_us: 0,
                },
            );
            previous_hour = Some(identity);
        }
        minute += 60 * SECOND;
    }
    let mut minutes = BTreeSet::new();
    let mut quiet_start = None;
    let mut idle_ranges = Vec::new();
    let mut coverage_end = start;
    let mut first_input = None;
    let mut last_input = None;
    let mut previous_end = i64::MIN;
    for (index, row) in rows.iter().enumerate() {
        ensure!(
            row.duration > 0 && row.duration <= SECOND,
            "invalid recorded interval duration"
        );
        let row_end = row
            .start
            .checked_add(row.duration)
            .context("interval timestamp overflow")?;
        ensure!(
            DateTime::<Utc>::from_timestamp_micros(row_end).is_some(),
            "invalid recorded timestamp"
        );
        // Intervals are at most one second. Overlaps from clock corrections or
        // concurrent runs cannot establish a unique wall-clock coverage timeline.
        let overlaps = previous_end > row.start
            || rows.get(index + 1).is_some_and(|next| next.start < row_end);
        previous_end = previous_end.max(row_end);
        let continuous = index > 0
            && rows[index - 1].run == row.run
            && rows[index - 1].mono + rows[index - 1].duration == row.mono
            && rows[index - 1].end() == row.start;
        if !continuous {
            quiet_start = None;
        }
        let complete = row.complete(ALL_INPUT) && !overlaps;
        // Repeats have counts but no first/last offsets. Use the entire bucket
        // as potentially occupied, preventing optimistic idle estimates.
        let repeats = row.repeats.unwrap_or(0) > 0;
        let has_input = row.has_input();
        let first = row.start + if repeats { 0 } else { row.first.unwrap_or(0) };
        let last = row.start
            + if repeats {
                row.duration
            } else {
                row.last.unwrap_or(row.duration)
            };
        if complete {
            let quiet_end = (if has_input { first } else { row_end }).min(end);
            let since = *quiet_start.get_or_insert(row.start);
            let idle_start = (since + idle_after_seconds as i64 * SECOND).max(start);
            if quiet_end > idle_start {
                let from = idle_start.max(row.start).max(start);
                if quiet_end > from {
                    idle_ranges.push((from, quiet_end));
                }
            }
            if has_input {
                quiet_start = Some(last);
            }
        } else {
            quiet_start = None;
        }

        if row_end <= start {
            continue;
        }
        let clipped_start = row.start.max(start);
        let clipped_end = row_end.min(end);
        let clipped_duration = clipped_end - clipped_start;
        // Coverage duration can be clipped, but aggregate counts cannot.
        // Counts from boundary-straddling buckets are omitted.
        report.recorded_intervals += 1;
        let inside = row.start >= start && row_end <= end;
        if inside {
            report.counts.add(row);
        }
        report.repeat_reporting_unknown_intervals += usize::from(row.repeats.is_none());
        let hour = hours
            .range_mut(..=clipped_start)
            .next_back()
            .map(|(_, hour)| hour)
            .context("missing hourly bucket")?;
        if inside {
            hour.counts.add(row);
        }
        if has_input && inside {
            first_input = Some(first_input.map_or(first, |old: i64| old.min(first)));
            last_input = Some(last_input.map_or(last, |old: i64| old.max(last)));
            let minute = row.start.div_euclid(60 * SECOND);
            if minutes.insert(minute) {
                hour.minutes_containing_input += 1;
            }
        }
        if !overlaps {
            for (coverage, mask) in report
                .coverage
                .iter_mut()
                .zip([KEYBOARD, BUTTON, MOTION, WHEEL])
            {
                if row.complete(mask) {
                    coverage.complete_us += clipped_duration;
                }
            }
        }
        if complete {
            if clipped_start > coverage_end {
                report
                    .unknown_periods
                    .push(period(coverage_end, clipped_start, tz));
            }
            coverage_end = clipped_end;
            report.complete_coverage_us += clipped_duration;
            add_duration(&mut hours, clipped_start, clipped_end, |hour| {
                &mut hour.complete_coverage_us
            });
        }
    }
    if coverage_end < end {
        report.unknown_periods.push(period(coverage_end, end, tz));
    }
    report.unknown_us = end - start - report.complete_coverage_us;
    report.first_input = first_input.map(|time| timestamp(time, tz));
    report.last_input = last_input.map(|time| timestamp(time, tz));
    report.minutes_containing_input = minutes.len();
    // Each contribution is already threshold-qualified; interruptions reset
    // the quiet timer and therefore cannot produce adjacent idle contributions.
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (from, to) in idle_ranges {
        if let Some(previous) = merged.last_mut().filter(|previous| previous.1 == from) {
            previous.1 = to;
        } else {
            merged.push((from, to));
        }
        add_duration(&mut hours, from, to, |hour| &mut hour.idle_us);
    }
    for (from, to) in merged {
        report.idle_us += to - from;
        report.longest_idle_us = report.longest_idle_us.max(to - from);
        report.idle_periods.push(period(from, to, tz));
    }
    report.hours = hours.into_values().collect();
    Ok(report)
}

fn duration(us: i64) -> String {
    let seconds = us / SECOND;
    format!(
        "{}h {:02}m {:02}s",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn display_time(value: &str, format: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .expect("report timestamp")
        .format(format)
        .to_string()
}

impl Summary {
    pub fn render(&self, details: bool) -> String {
        let start_offset = display_time(&self.start, "%:z");
        let end_offset = display_time(&self.end, "%:z");
        let timezone = if start_offset == end_offset {
            format!("UTC{start_offset}")
        } else {
            format!("UTC{start_offset} to UTC{end_offset}")
        };
        let mut text = format!(
            "Activity summary: {}, {} to {}\nTimezone: {}\nDatabase: {}\n",
            display_time(&self.start, "%Y-%m-%d"),
            display_time(&self.start, "%H:%M:%S"),
            display_time(&self.end, "%H:%M:%S"),
            timezone,
            self.database
        );
        if self.recorded_intervals == 0 {
            text.push_str("No recorded intervals inside this window.\n");
        }
        writeln!(
            text,
            "Observed: {} key presses, {} key repeats, {} mouse clicks",
            self.counts.key_presses, self.counts.observed_key_repeats, self.counts.mouse_clicks
        )
        .unwrap();
        writeln!(
            text,
            "Input in {} minutes; {} intervals with motion, {} with scroll",
            self.minutes_containing_input,
            self.counts.intervals_with_motion,
            self.counts.intervals_with_scroll
        )
        .unwrap();
        writeln!(
            text,
            "First input: {}\nLast input: {}",
            self.first_input
                .as_deref()
                .map(|value| display_time(value, "%H:%M:%S"))
                .unwrap_or_else(|| "none recorded".into()),
            self.last_input
                .as_deref()
                .map(|value| display_time(value, "%H:%M:%S"))
                .unwrap_or_else(|| "none recorded".into())
        )
        .unwrap();
        writeln!(
            text,
            "Complete coverage: {}\nUnknown: {} across {} gap{}; longest: {}",
            duration(self.complete_coverage_us),
            duration(self.unknown_us),
            self.unknown_periods.len(),
            if self.unknown_periods.len() == 1 {
                ""
            } else {
                "s"
            },
            duration(
                self.unknown_periods
                    .iter()
                    .map(|gap| gap.duration_us)
                    .max()
                    .unwrap_or(0)
            )
        )
        .unwrap();
        for coverage in &self.coverage {
            writeln!(
                text,
                "  {} coverage: {}",
                coverage.category,
                duration(coverage.complete_us)
            )
            .unwrap();
        }
        writeln!(
            text,
            "Idle after {}s: {}; longest: {}",
            self.idle_after_seconds,
            duration(self.idle_us),
            duration(self.longest_idle_us)
        )
        .unwrap();
        if details {
            for idle in &self.idle_periods {
                writeln!(
                    text,
                    "  Idle {} to {} ({})",
                    display_time(&idle.start, "%H:%M:%S"),
                    display_time(&idle.end, "%H:%M:%S"),
                    duration(idle.duration_us)
                )
                .unwrap();
            }
            for gap in &self.unknown_periods {
                writeln!(
                    text,
                    "  Unknown {} to {} ({})",
                    display_time(&gap.start, "%H:%M:%S"),
                    display_time(&gap.end, "%H:%M:%S"),
                    duration(gap.duration_us)
                )
                .unwrap();
            }
        } else if !self.idle_periods.is_empty() || !self.unknown_periods.is_empty() {
            text.push_str("Use --details to list individual idle and unknown periods.\n");
        }
        writeln!(
            text,
            "\n{:<5}  {:>6} {:>9} {:>8} {:>11}   {:>12}   {:>12}",
            "Hour", "Keys", "Repeats", "Clicks", "Input min", "Coverage", "Idle"
        )
        .unwrap();
        for hour in &self.hours {
            writeln!(
                text,
                "{}  {:>6} {:>9} {:>8} {:>11}   {:>12}   {:>12}",
                display_time(&hour.start, "%H:%M"),
                hour.counts.key_presses,
                hour.counts.observed_key_repeats,
                hour.counts.mouse_clicks,
                hour.minutes_containing_input,
                duration(hour.complete_coverage_us),
                duration(hour.idle_us)
            )
            .unwrap();
        }
        writeln!(text, "\nCounts may undercount with incomplete coverage; boundary-straddling counts are omitted.\nInput minutes and idle measure input activity, not time working.\nTiming is approximate within one-second buckets; repeats use bucket endpoints.\nRepeat reporting unavailable in {} intervals.", self.repeat_reporting_unknown_intervals).unwrap();
        text
    }
}

#[cfg(test)]
mod tests;
