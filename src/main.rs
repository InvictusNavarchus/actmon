use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarize today's recorded input and idle periods in the local timezone.
    Summary {
        #[arg(long)]
        database: PathBuf,
        /// Idle starts after this many seconds of continuously observed quiet.
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=86400))]
        idle_after_seconds: u64,
        /// Print structured timestamps and microsecond durations instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Publish a verified local snapshot. Does not schedule jobs or delete old snapshots.
    Backup {
        #[arg(long)]
        database: PathBuf,
        /// Dedicated published directory; upload this directory, not its parent.
        #[arg(long)]
        destination: PathBuf,
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=86400))]
        timeout_seconds: u64,
    },
    /// Read local backup attempt/success metadata (does not reverify artifacts).
    BackupStatus {
        #[arg(long)]
        destination: PathBuf,
    },
    /// Verify a copied snapshot before restoration, without modifying it.
    VerifyBackup {
        #[arg(long)]
        snapshot: PathBuf,
    },
    /// List input metadata without opening devices or recording input.
    Devices {
        #[arg(long, default_value = "/sys/class/input")]
        sysfs_root: PathBuf,
        #[arg(long, default_value = "/dev/input")]
        device_root: PathBuf,
    },
    /// Collect until SIGINT or SIGTERM. Requires read access to selected nodes.
    Collect {
        /// Explicit path prevents accidentally writing to an installed database.
        #[arg(long)]
        database: PathBuf,
        #[arg(long, default_value = "keyboard-debouncer")]
        keyboard_name: String,
        /// Exact name, including trailing spaces; see devices output.
        #[arg(long)]
        mouse_name: String,
        #[arg(long, default_value = "/sys/class/input")]
        sysfs_root: PathBuf,
        #[arg(long, default_value = "/dev/input")]
        device_root: PathBuf,
    },
    /// Read latest persisted coverage and commit metadata as JSON.
    Status {
        #[arg(long)]
        database: PathBuf,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_ansi(false)
        .init();
    match Cli::parse().command {
        Command::Summary {
            database,
            idle_after_seconds,
            json,
        } => {
            let report = actmon::summary::today(&database, idle_after_seconds)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render());
            }
        }
        Command::Backup {
            database,
            destination,
            timeout_seconds,
        } => {
            let stop = Arc::new(AtomicBool::new(false));
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&stop))?;
            signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&stop))?;
            let result = actmon::backup::create(
                &database,
                &destination,
                std::time::Duration::from_secs(timeout_seconds),
                stop,
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::BackupStatus { destination } => println!(
            "{}",
            serde_json::to_string_pretty(&actmon::backup::status(&destination)?)?
        ),
        Command::VerifyBackup { snapshot } => println!(
            "{}",
            serde_json::to_string_pretty(&actmon::backup::verify(&snapshot)?)?
        ),
        Command::Devices {
            sysfs_root,
            device_root,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&actmon::input::discover(&sysfs_root, &device_root)?)?
            );
        }
        Command::Status { database } => println!(
            "{}",
            serde_json::to_string_pretty(&actmon::db::status(&database)?)?
        ),
        Command::Collect {
            database,
            keyboard_name,
            mouse_name,
            sysfs_root,
            device_root,
        } => {
            let stop = Arc::new(AtomicBool::new(false));
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&stop))?;
            signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&stop))?;
            actmon::collector::collect(
                actmon::collector::Config {
                    database,
                    keyboard_name,
                    mouse_name,
                    sysfs_root,
                    device_root,
                },
                stop,
            )?;
        }
    }
    Ok(())
}
