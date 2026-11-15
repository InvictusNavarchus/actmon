//! Verified snapshots, kept separate from both collection and external upload.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DATABASE: &str = "activity.db";
const MANIFEST: &str = "manifest.json";
const TARGET: usize = 7;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Coverage {
    pub intervals: i64,
    pub first_interval_utc_us: Option<i64>,
    pub last_interval_end_utc_us: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub actmon_version: String,
    pub sqlite_version: String,
    pub schema_version: i64,
    pub source_database: PathBuf,
    pub verified_utc_us: i64,
    pub database_bytes: u64,
    pub database_sha256: String,
    pub coverage: Coverage,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Success {
    pub snapshot: PathBuf,
    pub published_utc_us: i64,
    pub coverage: Coverage,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub source_database: PathBuf,
    pub last_attempt_utc_us: Option<i64>,
    pub attempt_finished_utc_us: Option<i64>,
    pub last_error: Option<String>,
    pub last_success: Option<Success>,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub state: State,
    pub last_success_age_seconds: Option<f64>,
    pub snapshot_directories: usize,
    pub retention_target: usize,
    pub excess_snapshots: usize,
    pub automatic_deletion: bool,
}

#[derive(Clone)]
struct Budget {
    deadline: Instant,
    stop: Arc<AtomicBool>,
}
impl Budget {
    fn expired(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }
    fn check(&self) -> Result<()> {
        ensure!(
            !self.expired(),
            "backup cancelled or completion deadline exceeded"
        );
        Ok(())
    }
}

// A sibling control directory keeps mutable state and partial copies outside
// the uploader's root. Each destination belongs to one canonical database path.
fn control_path(destination: &Path) -> Result<PathBuf> {
    let name = destination
        .file_name()
        .context("destination must not be filesystem root")?;
    let mut control = std::ffi::OsString::from(".");
    control.push(name);
    control.push(".actmon-backup");
    Ok(destination.with_file_name(control))
}

fn private_dir(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => sync_dir(path.parent().context("directory has no parent")?)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let m = fs::symlink_metadata(path)?;
            ensure!(
                m.is_dir() && !m.file_type().is_symlink(),
                "expected a real directory: {}",
                path.display()
            );
            ensure!(
                m.mode() & 0o077 == 0,
                "backup directory must be private (mode 0700): {}",
                path.display()
            );
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}

fn read_state(control: &Path) -> Result<State> {
    match fs::read(control.join("state.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}

fn write_state(control: &Path, state: &State) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(control)?;
    serde_json::to_writer_pretty(&mut temp, state)?;
    temp.as_file().sync_all()?;
    temp.persist(control.join("state.json"))?;
    sync_dir(control)
}

pub fn status(destination: &Path) -> Result<Status> {
    let destination = fs::canonicalize(destination)?;
    let state = read_state(&control_path(&destination)?)?;
    let count = fs::read_dir(&destination)?.try_fold(0, |count, item| -> Result<usize> {
        let item = item?;
        Ok(count
            + usize::from(
                item.file_type()?.is_dir()
                    && item.file_name().to_string_lossy().starts_with("snapshot-"),
            ))
    })?;
    Ok(Status {
        last_success_age_seconds: state
            .last_success
            .as_ref()
            .map(|s| (crate::clock::now().utc_us - s.published_utc_us) as f64 / 1_000_000.0),
        state,
        snapshot_directories: count,
        retention_target: TARGET,
        excess_snapshots: count.saturating_sub(TARGET),
        automatic_deletion: false,
    })
}

pub fn create(
    database: &Path,
    destination: &Path,
    timeout: Duration,
    stop: Arc<AtomicBool>,
) -> Result<Status> {
    let budget = Budget {
        deadline: Instant::now()
            .checked_add(timeout)
            .context("timeout too large")?,
        stop,
    };
    let database = fs::canonicalize(database).context("open existing source database")?;
    // Require an existing parent; do not silently build an uploader directory tree.
    let parent = fs::canonicalize(
        destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let destination = parent.join(
        destination
            .file_name()
            .context("destination needs a directory name")?,
    );
    private_dir(&destination)?;
    let destination = fs::canonicalize(destination)?;
    ensure!(
        !database.starts_with(&destination),
        "published directory must not contain the live database"
    );
    let control = control_path(&destination)?;
    private_dir(&control)?;
    ensure!(
        !database.starts_with(&control),
        "control directory must not contain the live database"
    );
    ensure!(
        fs::metadata(&control)?.dev() == fs::metadata(&destination)?.dev(),
        "staging and published directory must be on the same filesystem"
    );
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(control.join("lock"))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .context("another backup holds this destination lock")?;
    let mut state = read_state(&control)?;
    ensure!(
        state.source_database.as_os_str().is_empty() || state.source_database == database,
        "destination belongs to a different database"
    );
    state.source_database = database.clone();
    state.last_attempt_utc_us = Some(crate::clock::now().utc_us);
    state.attempt_finished_utc_us = None;
    state.last_error = None;
    write_state(&control, &state)?;
    tracing::info!(source=%database.display(), destination=%destination.display(), "backup started");
    let result = publish(&database, &destination, &control, &budget);
    state.attempt_finished_utc_us = Some(crate::clock::now().utc_us);
    let outcome = match result {
        Ok(success) => {
            tracing::info!(snapshot=%success.snapshot.display(), "verified local snapshot published");
            state.last_success = Some(success);
            Ok(())
        }
        Err(error) => {
            state.last_error = Some(format!("{error:#}"));
            tracing::error!(error=%error, "backup failed; previous snapshots preserved");
            Err(error)
        }
    };
    write_state(&control, &state)
        .context("save backup outcome; inspect published directory if this fails")?;
    outcome?;
    let report = status(&destination)?;
    if report.excess_snapshots > 0 {
        tracing::warn!(
            excess = report.excess_snapshots,
            "retention target exceeded; automatic deletion disabled without reader coordination"
        );
    }
    Ok(report)
}

fn publish(
    database: &Path,
    destination: &Path,
    control: &Path,
    budget: &Budget,
) -> Result<Success> {
    budget.check()?;
    let source = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_millis(100))?;
    identity(&source)?;
    let staging = tempfile::Builder::new()
        .prefix("staging-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(control)?;
    let copy = staging.path().join(DATABASE);
    let mut target = Connection::open(&copy)?;
    target.execute_batch("PRAGMA synchronous=FULL;")?;
    {
        let backup = Backup::new(&source, &mut target)?;
        loop {
            budget.check()?;
            match backup.step(256)? {
                StepResult::Done => break,
                StepResult::More => thread::sleep(Duration::from_millis(1)),
                StepResult::Busy | StepResult::Locked => thread::sleep(Duration::from_millis(25)),
                _ => bail!("unsupported backup step result"),
            }
        }
    }
    // The backup copies WAL header flags too. Convert only the private copy to
    // DELETE mode, ensuring it is independently restorable without sidecars.
    target.execute_batch("PRAGMA journal_mode=DELETE;")?;
    target.close().map_err(|(_, e)| e)?;
    budget.check()?;
    let coverage = validate(&copy, Some(budget))?;
    let manifest = Manifest {
        format_version: 1,
        actmon_version: env!("CARGO_PKG_VERSION").into(),
        sqlite_version: rusqlite::version().into(),
        schema_version: 1,
        source_database: database.into(),
        verified_utc_us: crate::clock::now().utc_us,
        database_bytes: fs::metadata(&copy)?.len(),
        database_sha256: hash(&copy, Some(budget))?,
        coverage,
    };
    let mut file = File::create(staging.path().join(MANIFEST))?;
    serde_json::to_writer_pretty(&mut file, &manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(&copy)?.sync_all()?;
    sync_dir(staging.path())?;
    budget.check()?;
    let name = format!(
        "snapshot-{}-{}",
        manifest.verified_utc_us,
        staging.path().file_name().unwrap().to_string_lossy()
    );
    let published = destination.join(name);
    publish_directory(staging.path(), &published)?;
    // Once visible, never remove the artifact on a later sync/state error.
    sync_dir(destination)
        .and_then(|()| sync_dir(control))
        .with_context(|| {
            format!(
                "snapshot visible at {} but publication durability is uncertain",
                published.display()
            )
        })?;
    Ok(Success {
        snapshot: published,
        published_utc_us: crate::clock::now().utc_us,
        coverage: manifest.coverage,
    })
}

fn publish_directory(staging: &Path, published: &Path) -> Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        staging,
        rustix::fs::CWD,
        published,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .context("atomically publish snapshot without replacing an existing artifact")?;
    Ok(())
}

fn identity(c: &Connection) -> Result<()> {
    let app: i64 = c.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        app == 0x4143544d && version == 1,
        "unsupported Actmon database identity or schema"
    );
    Ok(())
}

fn regular_file(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "expected regular file: {}",
        path.display()
    );
    Ok(())
}

fn validate(path: &Path, budget: Option<&Budget>) -> Result<Coverage> {
    regular_file(path)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        ensure!(
            !Path::new(&sidecar).try_exists()?,
            "snapshot contains SQLite sidecar {suffix}"
        );
    }
    let mut c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if let Some(budget) = budget {
        let budget = budget.clone();
        c.progress_handler(1000, Some(move || budget.expired()))?;
    }
    identity(&c)?;
    let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    ensure!(mode == "delete", "snapshot must use DELETE journal mode");
    let results = c
        .prepare("PRAGMA integrity_check")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        results == ["ok"],
        "snapshot integrity check failed: {results:?}"
    );
    ensure!(
        c.prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_none(),
        "snapshot foreign key check failed"
    );
    // Exercise the application's normal read path as well as structural checks.
    crate::db::status(path)?;
    Ok(c.query_row(
        "SELECT count(*),min(start_utc_us),max(start_utc_us+duration_us) FROM intervals",
        [],
        |r| {
            Ok(Coverage {
                intervals: r.get(0)?,
                first_interval_utc_us: r.get(1)?,
                last_interval_end_utc_us: r.get(2)?,
            })
        },
    )?)
}

fn hash(path: &Path, budget: Option<&Budget>) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        if let Some(budget) = budget {
            budget.check()?;
        }
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Verify fixed artifact filenames, never paths supplied by a manifest.
/// Published snapshots are immutable by contract; callers must not edit them
/// concurrently with verification or restore from a changing directory.
pub fn verify(snapshot: &Path) -> Result<Manifest> {
    regular_file(&snapshot.join(MANIFEST))?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(snapshot.join(MANIFEST))?)?;
    ensure!(
        manifest.format_version == 1 && manifest.schema_version == 1,
        "unsupported snapshot format or schema"
    );
    let database = snapshot.join(DATABASE);
    regular_file(&database)?;
    ensure!(
        fs::metadata(&database)?.len() == manifest.database_bytes
            && hash(&database, None)? == manifest.database_sha256,
        "snapshot checksum or length mismatch"
    );
    ensure!(
        validate(&database, None)? == manifest.coverage,
        "snapshot coverage differs from manifest"
    );
    Ok(manifest)
}

#[cfg(test)]
mod tests;
