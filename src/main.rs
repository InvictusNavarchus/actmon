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
