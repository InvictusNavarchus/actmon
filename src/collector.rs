use std::{
    fs::{self, OpenOptions},
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rustix::event::{PollFd, PollFlags, poll};
use serde::{Deserialize, Serialize};

use crate::{
    clock::{self, Stamp},
    input::{self, Decoded, Reader, Role},
    model::{Aggregator, Event, KEYBOARD, MOUSE, SECOND},
    writer::Writer,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub database: PathBuf,
    pub keyboard_name: String,
    pub mouse_name: String,
    pub sysfs_root: PathBuf,
    pub device_root: PathBuf,
}

struct Source {
    role: Role,
    name: String,
    reader: Option<Reader>,
    fault: Option<String>,
    loss_start: Option<i64>,
    last_drained_us: i64,
}

impl Source {
    fn mask(&self) -> u8 {
        if self.role == Role::Keyboard {
            KEYBOARD
        } else {
            MOUSE
        }
    }

    fn report(&mut self, agg: &mut Aggregator, time: Stamp, kind: &str, details: String) {
        agg.event(Event::new(
            kind,
            time,
            serde_json::json!({"role":self.role,"name":self.name,"detail":details}).to_string(),
        ));
    }

    fn disconnect(&mut self, agg: &mut Aggregator, time: Stamp, reason: String) {
        if let Some(start) = self.loss_start.take() {
            self.record_loss(agg, time, start);
        }
        self.reader = None;
        agg.coverage(time, self.mask(), false, false);
        let mut loss = Event::new(
            "source_unavailable",
            time,
            serde_json::json!({"role":self.role,"reason":reason}).to_string(),
        );
        loss.loss = Some((self.last_drained_us, time.mono_us + 1, self.mask()));
        agg.event(loss);
    }

    fn record_loss(&mut self, agg: &mut Aggregator, time: Stamp, start: i64) {
        let mut event = Event::new(
            "kernel_event_loss",
            time,
            serde_json::json!({"role":self.role}).to_string(),
        );
        event.loss = Some((start, time.mono_us + 1, self.mask()));
        agg.event(event);
    }

    fn refresh(&mut self, agg: &mut Aggregator, time: Stamp, devices: &[input::DeviceInfo]) {
        let selected = match input::select(devices, &self.name) {
            Ok(value) => value,
            Err(error) => {
                self.fault(agg, time, error.to_string());
                return;
            }
        };
        if self.reader.as_ref().is_some_and(|r| {
            selected.is_some_and(|d| d.sysfs == r.info.sysfs && d.node == r.info.node)
        }) {
            return;
        }
        if self.reader.is_some() {
            self.disconnect(agg, time, "device removed or replaced".into());
        }
        let Some(info) = selected else {
            self.fault(agg, time, "no matching device".into());
            return;
        };
        match Reader::open(info.clone(), self.role) {
            Ok(reader) => {
                let time = clock::now();
                agg.coverage(time, reader.mask, true, reader.repeats_supported);
                self.report(
                    agg,
                    time,
                    "source_connected",
                    serde_json::to_string(&reader.info).unwrap(),
                );
                self.reader = Some(reader);
                self.fault = None;
                self.last_drained_us = time.mono_us;
            }
            Err(error) => self.fault(agg, time, format!("{error:#}")),
        }
    }

    fn fault(&mut self, agg: &mut Aggregator, time: Stamp, message: String) {
        if self.reader.is_some() {
            self.disconnect(agg, time, message.clone());
        }
        if self.fault.as_ref() != Some(&message) {
            tracing::warn!(role=?self.role,error=%message,"input source unavailable");
            self.report(agg, time, "source_error", message.clone());
            self.fault = Some(message);
        }
    }

    fn drain(&mut self, agg: &mut Aggregator) {
        // Bound work per pass so a very busy device cannot starve intervals,
        // discovery, signals, or the other reader.
        for _ in 0..32 {
            let Some(reader) = self.reader.as_mut() else {
                return;
            };
            let events = match reader.read() {
                Ok(events) => events,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.last_drained_us = clock::now().mono_us;
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.disconnect(agg, clock::now(), error.to_string());
                    return;
                }
            };
            if events.is_empty() {
                self.disconnect(agg, clock::now(), "end of input stream".into());
                return;
            }
            for event in events {
                let time = clock::now();
                let decoded = self.reader.as_mut().unwrap().decoder.decode(&event);
                match decoded {
                    Decoded::Action(action) => match input::event_time(&event) {
                        Ok(stamp) => agg.input(time, stamp, action),
                        Err(error) => {
                            self.fault(agg, time, error.to_string());
                            return;
                        }
                    },
                    Decoded::Dropped => {
                        // The overrun can have discarded events older than the
                        // marker. Conservatively invalidate back to connection.
                        let start = self.loss_start.unwrap_or(self.last_drained_us);
                        self.loss_start = Some(start);
                        self.record_loss(agg, time, start);
                    }
                    Decoded::Recovered => {
                        if let Some(start) = self.loss_start.take() {
                            self.record_loss(agg, time, start);
                        }
                    }
                    Decoded::Ignore => {}
                }
            }
        }
    }
}

pub fn collect(mut config: Config, stop: Arc<AtomicBool>) -> Result<()> {
    if config.keyboard_name == config.mouse_name {
        bail!("keyboard and mouse must select distinct sources");
    }
    if let Some(parent) = config
        .database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    config.database = if config.database.exists() {
        fs::canonicalize(&config.database)?
    } else {
        let parent = config
            .database
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        fs::canonicalize(parent)?.join(
            config
                .database
                .file_name()
                .context("database path needs a filename")?,
        )
    };
    let mut lock_name = config.database.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(lock_name))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .context("another collector holds the database lock")?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    let mut writer = Writer::start(
        config.database.clone(),
        boot,
        serde_json::to_string(&config)?,
    )?;
    let mut previous = clock::now();
    let mut agg = Aggregator::new(previous);
    agg.event(Event::new("collector_started", previous, "{}".into()));
    let mut sources = [
        Source {
            role: Role::Keyboard,
            name: config.keyboard_name.clone(),
            reader: None,
            fault: None,
            loss_start: None,
            last_drained_us: previous.mono_us,
        },
        Source {
            role: Role::Mouse,
            name: config.mouse_name.clone(),
            reader: None,
            fault: None,
            loss_start: None,
            last_drained_us: previous.mono_us,
        },
    ];
    let mut next_scan = previous.mono_us;
    let mut next_flush = previous.mono_us + SECOND;
    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            let time = clock::now();
            if discontinuous(previous, time) {
                agg.discontinuity(time);
                next_scan = time.mono_us;
                next_flush = time.mono_us + SECOND;
            }
            previous = time;
            if time.mono_us >= next_scan {
                match input::discover(&config.sysfs_root, &config.device_root) {
                    Ok(devices) => {
                        for source in &mut sources {
                            source.refresh(&mut agg, time, &devices);
                        }
                    }
                    Err(error) => {
                        for source in &mut sources {
                            source.fault(&mut agg, time, format!("{error:#}"));
                        }
                    }
                }
                next_scan = time.mono_us + 2 * SECOND;
            }
            let readers: Vec<_> = sources.iter().filter_map(|s| s.reader.as_ref()).collect();
            let mut fds: Vec<_> = readers
                .iter()
                .map(|r| PollFd::new(r.fd(), PollFlags::IN))
                .collect();
            let timeout = rustix::time::Timespec::try_from(Duration::from_millis(100))?;
            match poll(&mut fds, Some(&timeout)) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
            drop(fds);
            drop(readers);
            let after_poll = clock::now();
            if discontinuous(previous, after_poll) {
                agg.discontinuity(after_poll);
                next_scan = after_poll.mono_us;
                next_flush = after_poll.mono_us + SECOND;
            }
            previous = after_poll;
            for source in &mut sources {
                source.drain(&mut agg);
            }
            let time = clock::now();
            if time.mono_us >= next_flush {
                // Keep an unresolved overrun incomplete across interval boundaries.
                for source in &mut sources {
                    if let Some(start) = source.loss_start {
                        source.record_loss(&mut agg, time, start);
                    }
                }
                writer.submit(agg.finish(time, false))?;
                next_flush = time.mono_us + SECOND;
            }
        }
        Ok(())
    })();
    let time = clock::now();
    for source in &mut sources {
        if let Some(start) = source.loss_start {
            source.record_loss(&mut agg, time, start);
        }
    }
    agg.event(Event::new(
        if result.is_ok() {
            "collector_stopped"
        } else {
            "collector_failed"
        },
        time,
        "{}".into(),
    ));
    let mut final_batch = agg.finish(time, true);
    final_batch.stopped = result.is_ok();
    writer.finish(final_batch)?;
    result
}

fn discontinuous(before: Stamp, after: Stamp) -> bool {
    let mono = after.mono_us - before.mono_us;
    mono > 2 * SECOND
        || ((after.utc_us - before.utc_us) - mono).abs() > 100_000
        || ((after.boot_us - before.boot_us) - mono).abs() > 100_000
}
