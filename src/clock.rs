use std::os::fd::AsFd;

use rustix::time::{ClockId, clock_gettime};

#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    pub mono_us: i64,
    pub utc_us: i64,
    pub boot_us: i64,
}

pub fn now() -> Stamp {
    // Bracket the UTC read so the mapping does not include an entire syscall's
    // delay on one side. Capture latency is tracked by the collector separately.
    let before = micros(ClockId::Monotonic);
    let utc_us = micros(ClockId::Realtime);
    let after = micros(ClockId::Monotonic);
    Stamp {
        mono_us: before + (after - before) / 2,
        utc_us,
        boot_us: micros(ClockId::Boottime),
    }
}

fn micros(clock: ClockId) -> i64 {
    let value = clock_gettime(clock);
    value.tv_sec * 1_000_000 + value.tv_nsec / 1_000
}

/// Switch this reader's kernel timestamps to CLOCK_MONOTONIC. This does not
/// change timestamps or access mode for other readers of the device.
pub fn set_input_clock(fd: impl AsFd) -> rustix::io::Result<()> {
    const EVIOCSCLOCKID: rustix::ioctl::Opcode = rustix::ioctl::opcode::write::<i32>(b'E', 0xa0);
    // SAFETY: Linux EVIOCSCLOCKID is _IOW('E', 0xa0, int), expecting a pointer
    // to a C int clock ID. i32 matches Linux int; CLOCK_MONOTONIC is 1.
    unsafe {
        let request = rustix::ioctl::Setter::<EVIOCSCLOCKID, i32>::new(1);
        rustix::ioctl::ioctl(fd, request)
    }
}
