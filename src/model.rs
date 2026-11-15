use std::collections::BTreeMap;

use crate::clock::Stamp;

pub const SECOND: i64 = 1_000_000;
pub const KEYBOARD: u8 = 1;
pub const BUTTON: u8 = 2;
pub const MOTION: u8 = 4;
pub const WHEEL: u8 = 8;
pub const MOUSE: u8 = BUTTON | MOTION | WHEEL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Key,
    Repeat,
    Button(usize),
    Motion,
    Wheel { axis: usize, amount: i64 },
}

impl Action {
    pub fn mask(self) -> u8 {
        match self {
            Self::Key | Self::Repeat => KEYBOARD,
            Self::Button(_) => BUTTON,
            Self::Motion => MOTION,
            Self::Wheel { .. } => WHEEL,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Span {
    pub first: Option<i64>,
    pub last: Option<i64>,
}

impl Span {
    fn observe(&mut self, time: i64) {
        self.first = Some(self.first.map_or(time, |old| old.min(time)));
        self.last = Some(self.last.map_or(time, |old| old.max(time)));
    }
}

#[derive(Clone, Debug)]
pub struct Interval {
    pub start_mono_us: i64,
    pub start_utc_us: i64,
    pub duration_us: i64,
    /// Categories available for the entire interval, not just at its end.
    pub available: u8,
    pub incomplete: u8,
    pub repeats_supported: bool,
    pub keys: i64,
    pub repeats: i64,
    pub buttons: [i64; 8],
    pub moved: bool,
    /// up, down, right, left; units of 1/120 nominal wheel detent.
    pub wheel: [i64; 4],
    /// key-down, button-down, movement, scroll. Repeats do not extend key span.
    pub spans: [Span; 4],
}

impl Interval {
    fn new(start: i64, mapping: Stamp, available: u8, repeats_supported: bool) -> Self {
        Self {
            start_mono_us: start,
            start_utc_us: mapping.utc_us + start - mapping.mono_us,
            duration_us: SECOND,
            available,
            incomplete: 0,
            repeats_supported,
            keys: 0,
            repeats: 0,
            buttons: [0; 8],
            moved: false,
            wheel: [0; 4],
            spans: Default::default(),
        }
    }

    fn observe(&mut self, stamp: i64, action: Action) {
        let offset = stamp - self.start_mono_us;
        let category = match action {
            Action::Key => {
                self.keys += 1;
                0
            }
            Action::Repeat => {
                self.repeats += 1;
                return;
            }
            Action::Button(index) => {
                self.buttons[index] += 1;
                1
            }
            Action::Motion => {
                self.moved = true;
                2
            }
            Action::Wheel { axis, amount } => {
                self.wheel[axis * 2 + usize::from(amount < 0)] += amount.abs();
                3
            }
        };
        self.spans[category].observe(offset);
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub kind: String,
    pub mono_us: i64,
    pub utc_us: i64,
    /// When present, invalidate persisted intervals overlapping this range.
    pub loss: Option<(i64, i64, u8)>,
    pub details: String,
}

impl Event {
    pub fn new(kind: &str, time: Stamp, details: String) -> Self {
        Self {
            kind: kind.into(),
            mono_us: time.mono_us,
            utc_us: time.utc_us,
            loss: None,
            details,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub intervals: Vec<Interval>,
    pub events: Vec<Event>,
    pub stopped: bool,
}

/// A one-second grace period lets the reader drain boundary events before
/// finalization. Older arrivals explicitly invalidate previously recorded rows.
pub struct Aggregator {
    origin: i64,
    finalized: i64,
    mapping: Stamp,
    available: u8,
    repeats_supported: bool,
    buckets: BTreeMap<i64, Interval>,
    events: Vec<Event>,
    pending_losses: Vec<(i64, i64, u8)>,
}

impl Aggregator {
    pub fn new(time: Stamp) -> Self {
        Self {
            origin: time.mono_us,
            finalized: time.mono_us,
            mapping: time,
            available: 0,
            repeats_supported: false,
            buckets: BTreeMap::new(),
            events: Vec::new(),
            pending_losses: Vec::new(),
        }
    }

    fn bucket_start(&self, stamp: i64) -> i64 {
        self.origin + (stamp - self.origin).div_euclid(SECOND) * SECOND
    }

    fn ensure_through(&mut self, stamp: i64) {
        let end = self.bucket_start(stamp);
        let mut next = self
            .buckets
            .last_key_value()
            .map_or(self.finalized, |(key, _)| key + SECOND);
        while next <= end {
            let mut bucket =
                Interval::new(next, self.mapping, self.available, self.repeats_supported);
            for &(start, end, mask) in &self.pending_losses {
                if next < end && next + SECOND > start {
                    bucket.incomplete |= mask;
                }
            }
            self.buckets.insert(next, bucket);
            next += SECOND;
        }
    }

    pub fn event(&mut self, event: Event) {
        if let Some((start, end, mask)) = event.loss {
            // A loss can precede bucket creation or survive across batches.
            // Coalesce overlapping ranges of the same mask; prune only after
            // every affected interval has been finalized.
            if end > self.finalized && start < end {
                let mut merged_start = start.max(self.finalized);
                let mut merged_end = end;
                self.pending_losses
                    .retain(|&(old_start, old_end, old_mask)| {
                        if old_mask == mask && old_start <= merged_end && old_end >= merged_start {
                            merged_start = merged_start.min(old_start);
                            merged_end = merged_end.max(old_end);
                            false
                        } else {
                            true
                        }
                    });
                self.pending_losses.push((merged_start, merged_end, mask));
            }
            for bucket in self.buckets.values_mut() {
                if bucket.start_mono_us < end && bucket.start_mono_us + bucket.duration_us > start {
                    bucket.incomplete |= mask;
                }
            }
        }
        self.events.push(event);
    }

    pub fn coverage(&mut self, time: Stamp, mask: u8, available: bool, repeats: bool) {
        self.ensure_through(time.mono_us);
        self.available = if available {
            self.available | mask
        } else {
            self.available & !mask
        };
        if mask & KEYBOARD != 0 {
            self.repeats_supported = repeats && available;
        }
        let start = self.bucket_start(time.mono_us);
        if let Some(bucket) = self.buckets.get_mut(&start) {
            if time.mono_us == start {
                bucket.available = (bucket.available & !mask) | (self.available & mask);
                if mask & KEYBOARD != 0 {
                    bucket.repeats_supported = self.repeats_supported;
                }
            } else {
                bucket.available &= !mask;
                bucket.incomplete |= mask;
                if mask & KEYBOARD != 0 {
                    bucket.repeats_supported = false;
                }
            }
        }
    }

    pub fn input(&mut self, time: Stamp, stamp: i64, action: Action) {
        if stamp < self.origin {
            return;
        } // Event queued before this run opened its source.
        if stamp < self.finalized || stamp > time.mono_us + 10_000 {
            let mut event = Event::new("late_or_invalid_input", time, "{}".into());
            event.loss = Some((
                stamp.min(time.mono_us),
                stamp.max(time.mono_us) + 1,
                action.mask(),
            ));
            self.event(event);
            return;
        }
        self.ensure_through(stamp);
        let start = self.bucket_start(stamp);
        if let Some(bucket) = self.buckets.get_mut(&start) {
            bucket.observe(stamp, action);
        }
    }

    pub fn finish(&mut self, time: Stamp, stop: bool) -> Batch {
        self.ensure_through(time.mono_us);
        let cutoff = if stop {
            time.mono_us
        } else {
            time.mono_us - SECOND
        };
        let ready: Vec<_> = self
            .buckets
            .keys()
            .copied()
            .take_while(|start| *start + SECOND <= cutoff || (stop && *start < cutoff))
            .collect();
        let mut intervals = Vec::new();
        for start in ready {
            let mut bucket = self.buckets.remove(&start).unwrap();
            if start + SECOND > time.mono_us {
                bucket.duration_us = time.mono_us - start;
                bucket.incomplete |= KEYBOARD | MOUSE;
            }
            self.finalized = start + bucket.duration_us;
            intervals.push(bucket);
        }
        self.pending_losses
            .retain(|&(_, end, _)| end > self.finalized);
        Batch {
            intervals,
            events: std::mem::take(&mut self.events),
            stopped: stop,
        }
    }

    /// No fabricated idle rows across suspension, a long scheduling stall, or
    /// UTC movement. The old pending intervals become an explicit coverage gap.
    pub fn discontinuity(&mut self, time: Stamp) {
        let start = self.finalized;
        self.buckets.clear();
        let mut event = Event::new("clock_or_capture_gap", time, "{}".into());
        event.loss = Some((start, time.mono_us.max(start + 1), KEYBOARD | MOUSE));
        self.event(event);
        self.origin = time.mono_us;
        self.finalized = time.mono_us;
        self.mapping = time;
    }
}

#[cfg(test)]
mod tests;
