use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;

use crate::{
    clock,
    model::{Batch, Interval},
};

const APPLICATION_ID: i64 = 0x4143544d;
const SCHEMA: &str = include_str!("schema.sql");

pub struct Store {
    connection: Connection,
    run: i64,
    previous_commit_latency_us: Option<i64>,
    next_batch: i64,
}

#[derive(Serialize)]
pub struct Status {
    pub schema_version: i64,
    pub run_id: Option<i64>,
    pub latest_commit_utc_us: Option<i64>,
    pub orderly_stop_utc_us: Option<i64>,
    pub latest_interval_utc_us: Option<i64>,
    pub available_mask: Option<u8>,
    pub incomplete_mask: Option<u8>,
    pub previous_commit_latency_us: Option<i64>,
    pub latest_capture_to_commit_us: Option<i64>,
}

impl Store {
    pub fn open(path: &Path, boot_id: &str, configuration: &str) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(250))?;
        let application: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if !((version == 0 && application == 0) || (version == 1 && application == APPLICATION_ID))
        {
            bail!("unsupported database identity or schema version");
        }
        if application == 0 {
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if tables != 0 {
                bail!("refusing to initialize a non-Actmon database");
            }
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        if version == 0 {
            let tx = connection.transaction()?;
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        // An additive index also upgrades already-created v1 databases. It
        // keeps per-interval loss lookups bounded to ranges ending after it.
        connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS events_loss ON events(run_id,loss_end_mono_us);",
        )?;
        let time = clock::now();
        connection.execute("INSERT INTO runs(boot_id,version,configuration,started_utc_us,started_mono_us) VALUES(?1,?2,?3,?4,?5)",
            params![boot_id, env!("CARGO_PKG_VERSION"), configuration, time.utc_us, time.mono_us])?;
        let run = connection.last_insert_rowid();
        Ok(Self {
            connection,
            run,
            previous_commit_latency_us: None,
            next_batch: 1,
        })
    }

    /// Each batch is a transaction: retrying cannot expose half-written coverage.
    pub fn write(&mut self, batch: &Batch) -> Result<()> {
        let begin = Instant::now();
        let time = clock::now();
        let tx = self.connection.transaction()?;
        let last_batch: i64 =
            tx.query_row("SELECT last_batch FROM runs WHERE id=?1", [self.run], |r| {
                r.get(0)
            })?;
        if last_batch >= self.next_batch {
            // A commit error can have an uncertain outcome. The sequence marker
            // is atomic with the rows, so retrying an already committed batch
            // does not duplicate measurements or wedge on a uniqueness error.
            tx.rollback()?;
            self.next_batch += 1;
            return Ok(());
        }
        for row in &batch.intervals {
            insert_interval(&tx, self.run, row)?;
        }
        for event in &batch.events {
            let loss = event.loss;
            tx.execute("INSERT INTO events(run_id,utc_us,mono_us,kind,details,loss_start_mono_us,loss_end_mono_us,loss_mask) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![self.run, event.utc_us, event.mono_us, event.kind, event.details,
                    loss.map(|v|v.0), loss.map(|v|v.1), loss.map(|v|v.2)])?;
            if let Some((start, end, mask)) = loss {
                tx.execute("UPDATE intervals SET incomplete_mask=incomplete_mask | ?4 WHERE run_id=?1 AND start_mono_us < ?3 AND start_mono_us+duration_us > ?2",
                    params![self.run,start,end,mask])?;
            }
        }
        let lag = batch
            .intervals
            .last()
            .map(|r| time.mono_us - r.start_mono_us - r.duration_us);
        tx.execute("UPDATE runs SET latest_commit_utc_us=?2, stopped_utc_us=CASE WHEN ?3 THEN ?2 ELSE stopped_utc_us END, capture_to_commit_us=?4,previous_commit_latency_us=?5,last_batch=?6 WHERE id=?1",
            params![self.run,time.utc_us,batch.stopped,lag,self.previous_commit_latency_us,self.next_batch])?;
        tx.commit()?;
        self.next_batch += 1;
        // Updating this in another transaction would add an unnecessary sync.
        // Latency is logged and retained on the NEXT batch's transaction.
        let elapsed = i64::try_from(begin.elapsed().as_micros()).unwrap_or(i64::MAX);
        self.previous_commit_latency_us = Some(elapsed);
        tracing::debug!(commit_latency_us=elapsed, capture_to_commit_us=?lag, "batch committed");
        Ok(())
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

fn insert_interval(tx: &rusqlite::Transaction<'_>, run: i64, r: &Interval) -> Result<()> {
    // A loss event can be committed before all affected intervals exist,
    // especially when a queue-overflow marker reaches the writer first.
    let mut incomplete = r.incomplete;
    let mut losses = tx.prepare_cached("SELECT loss_mask FROM events WHERE run_id=?1 AND loss_end_mono_us>?2 AND loss_start_mono_us<?3")?;
    let masks = losses.query_map(
        params![run, r.start_mono_us, r.start_mono_us + r.duration_us],
        |row| row.get::<_, u8>(0),
    )?;
    for mask in masks {
        incomplete |= mask?;
    }
    tx.execute("INSERT INTO intervals(run_id,start_mono_us,start_utc_us,duration_us,available_mask,incomplete_mask,key_downs,key_repeats,button_left,button_right,button_middle,button_side,button_extra,button_forward,button_back,button_task,moved,wheel_up,wheel_down,wheel_right,wheel_left,key_first_us,key_last_us,button_first_us,button_last_us,motion_first_us,motion_last_us,scroll_first_us,scroll_last_us)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29)",
        params![run,r.start_mono_us,r.start_utc_us,r.duration_us,r.available,incomplete,r.keys,
            if r.repeats_supported || r.repeats>0 { Some(r.repeats) } else { None },
            r.buttons[0],r.buttons[1],r.buttons[2],r.buttons[3],r.buttons[4],r.buttons[5],r.buttons[6],r.buttons[7],
            r.moved,r.wheel[0],r.wheel[1],r.wheel[2],r.wheel[3],
            r.spans[0].first,r.spans[0].last,r.spans[1].first,r.spans[1].last,
            r.spans[2].first,r.spans[2].last,r.spans[3].first,r.spans[3].last])?;
    Ok(())
}

pub fn status(path: &Path) -> Result<Status> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open existing database {}", path.display()))?;
    let application: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if application != APPLICATION_ID || version != 1 {
        bail!("unsupported Actmon database");
    }
    let mut status = Status {
        schema_version: version,
        run_id: None,
        latest_commit_utc_us: None,
        orderly_stop_utc_us: None,
        latest_interval_utc_us: None,
        available_mask: None,
        incomplete_mask: None,
        previous_commit_latency_us: None,
        latest_capture_to_commit_us: None,
    };
    let mut stmt = connection.prepare("SELECT id,latest_commit_utc_us,stopped_utc_us,capture_to_commit_us,previous_commit_latency_us FROM runs ORDER BY id DESC LIMIT 1")?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        status.run_id = Some(row.get(0)?);
        status.latest_commit_utc_us = row.get(1)?;
        status.orderly_stop_utc_us = row.get(2)?;
        status.latest_capture_to_commit_us = row.get(3)?;
        status.previous_commit_latency_us = row.get(4)?;
    }
    let mut stmt=connection.prepare("SELECT start_utc_us,available_mask,incomplete_mask FROM intervals WHERE run_id=?1 ORDER BY start_mono_us DESC LIMIT 1")?;
    let mut rows = stmt.query([status.run_id])?;
    if let Some(row) = rows.next()? {
        status.latest_interval_utc_us = Some(row.get(0)?);
        status.available_mask = Some(row.get(1)?);
        status.incomplete_mask = Some(row.get(2)?);
    }
    Ok(status)
}

#[cfg(test)]
mod tests;
