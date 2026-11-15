use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Result, anyhow};

use crate::{
    clock,
    db::Store,
    model::{Batch, Event, KEYBOARD, MOUSE},
};

pub const QUEUE_CAPACITY: usize = 120;

pub struct Writer {
    sender: Option<SyncSender<Batch>>,
    thread: Option<JoinHandle<Result<()>>>,
    stopping: Arc<AtomicBool>,
    lost_since: Option<i64>,
}

impl Writer {
    pub fn start(path: PathBuf, boot_id: String, configuration: String) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let thread = thread::spawn(move || {
            let store = Store::open(&path, &boot_id, &configuration);
            match store {
                Ok(store) => {
                    let _ = ready_tx.send(Ok(()));
                    run(store, receiver, stop)
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    Err(error)
                }
            }
        });
        ready_rx.recv()?.map_err(|e| anyhow!(e))?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            stopping,
            lost_since: None,
        })
    }

    pub fn submit(&mut self, mut batch: Batch) -> Result<()> {
        let now = clock::now();
        let start = batch
            .intervals
            .iter()
            .map(|r| r.start_mono_us)
            .chain(
                batch
                    .events
                    .iter()
                    .map(|e| e.loss.map_or(e.mono_us, |loss| loss.0)),
            )
            .min()
            .unwrap_or(now.mono_us);
        if let Some(start) = self.lost_since {
            let mut event = Event::new("writer_queue_loss", now, "{}".into());
            event.loss = Some((start, now.mono_us, KEYBOARD | MOUSE));
            batch.events.push(event);
        }
        match self.sender.as_ref().unwrap().try_send(batch) {
            Ok(()) => self.lost_since = None,
            Err(TrySendError::Full(_)) => {
                if self.lost_since.is_none() {
                    tracing::error!("writer queue full; recording coverage loss");
                }
                self.lost_since = Some(self.lost_since.map_or(start, |old| old.min(start)));
            }
            Err(TrySendError::Disconnected(_)) => return Err(anyhow!("database writer stopped")),
        }
        Ok(())
    }

    pub fn finish(mut self, mut batch: Batch) -> Result<()> {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(start) = self.lost_since {
            let now = clock::now();
            let mut event = Event::new("writer_queue_loss", now, "{}".into());
            event.loss = Some((start, now.mono_us, KEYBOARD | MOUSE));
            batch.events.push(event);
        }
        self.sender
            .take()
            .unwrap()
            .send(batch)
            .map_err(|_| anyhow!("database writer stopped before shutdown"))?;
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| anyhow!("database writer panicked"))?
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(mut store: Store, receiver: Receiver<Batch>, stopping: Arc<AtomicBool>) -> Result<()> {
    for batch in receiver {
        let mut warned = false;
        loop {
            match store.write(&batch) {
                Ok(()) => break,
                Err(error) => {
                    if !warned {
                        tracing::error!(%error,"database write failed; retrying with bounded capture queue");
                        warned = true;
                    }
                    if stopping.load(Ordering::Relaxed) {
                        return Err(error);
                    }
                    thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
