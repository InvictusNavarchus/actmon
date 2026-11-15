//! Input measurements, coverage accounting, and local persistence for Actmon.

#[cfg(not(target_os = "linux"))]
compile_error!("Actmon currently supports Linux only");

pub mod clock;
pub mod collector;
pub mod db;
pub mod input;
pub mod model;
pub mod writer;
