//! Fixed operational events, separate from DNS recovery records.
use crate::storage::{reject_reparse, validate_file};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 512 * 1024;
const MAX_RECORDS: usize = 4096;
const RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;
const NAME: &str = "startup-events.v1.json";
const TEMPORARY: &str = "startup-events.v1.json.tmp";

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Event {
    ControllerStarting,
    ControllerStartFailed,
    ControllerStarted,
    PipeHostFailed,
    ServiceStopped,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    unix_seconds: u64,
    event: Event,
    windows_error: Option<u32>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Log {
    version: u32,
    records: Vec<Record>,
}

impl Log {
    fn retain(&mut self, now: u64) {
        // Discard future dates too, so a clock rollback cannot retain them forever.
        self.records.retain(|record| {
            record.unix_seconds <= now && now - record.unix_seconds < RETENTION_SECONDS
        });
        if self.records.len() > MAX_RECORDS {
            self.records.drain(..self.records.len() - MAX_RECORDS);
        }
    }

    fn next_expiry(&self, now: u64) -> Duration {
        let seconds = self
            .records
            .iter()
            .map(|record| {
                record
                    .unix_seconds
                    .saturating_add(RETENTION_SECONDS)
                    .saturating_sub(now)
            })
            .min()
            .unwrap_or(60);
        // Recheck wall-clock changes without polling adapters or service state.
        Duration::from_secs(seconds.clamp(1, 60))
    }
}

pub(crate) struct StartupLog {
    root: PathBuf,
    access: Mutex<()>,
    _lock: File,
}

fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(other)
}

impl StartupLog {
    /// Caller holds the validated machine directory handle.
    /// No IPC request can choose a path, filename, event text, or log contents.
    pub(crate) fn new(root: &Path) -> io::Result<Self> {
        let path = root.join("startup-events.v1.lock");
        reject_reparse(&path).map_err(other)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0).custom_flags(0x0020_0000);
        }
        let lock = options.open(path)?;
        validate_file(&lock).map_err(other)?;
        lock.try_lock()?;
        Ok(Self {
            root: root.into(),
            access: Mutex::new(()),
            _lock: lock,
        })
    }

    pub(crate) fn record(&self, event: Event, windows_error: Option<u32>) -> io::Result<()> {
        self.update(now()?, Some((event, windows_error)))
            .map(|_| ())
    }

    pub(crate) fn prune(&self) -> io::Result<Duration> {
        self.update(now()?, None)
    }

    fn open(&self, name: &str) -> io::Result<Option<File>> {
        let path = self.root.join(name);
        if !reject_reparse(&path).map_err(other)? {
            return Ok(None);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000); // OPEN_REPARSE_POINT
        }
        let file = options.open(path)?;
        validate_file(&file).map_err(other)?;
        Ok(Some(file))
    }

    fn update(&self, now: u64, event: Option<(Event, Option<u32>)>) -> io::Result<Duration> {
        let _guard = self.access.lock().map_err(other)?;
        let mut log = if let Some(file) = self.open(NAME)? {
            let mut bytes = Vec::new();
            file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > MAX_BYTES {
                return Err(other("Operational log exceeds its size limit"));
            }
            let log: Log = serde_json::from_slice(&bytes).map_err(other)?;
            if log.version != 1 {
                return Err(other("Unsupported operational log version"));
            }
            log
        } else {
            Log {
                version: 1,
                records: Vec::new(),
            }
        };
        let previous = log.records.len();
        log.retain(now);
        let changed = previous != log.records.len() || event.is_some();
        if let Some((event, windows_error)) = event {
            log.records.push(Record {
                unix_seconds: now,
                event,
                windows_error,
            });
            log.retain(now);
        }
        let next = log.next_expiry(now);
        if !changed {
            return Ok(next);
        }
        let bytes = serde_json::to_vec(&log).map_err(other)?;
        if bytes.len() > MAX_BYTES {
            return Err(other("Operational log exceeds its size limit"));
        }
        // A separate log lock owns cleanup even if recovery startup failed.
        if let Some(file) = self.open(TEMPORARY)? {
            drop(file);
            fs::remove_file(self.root.join(TEMPORARY))?;
        }
        let temporary = self.root.join(TEMPORARY);
        let mut file = File::create_new(&temporary)?;
        validate_file(&file).map_err(other)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        quad9_native::replace_file(&temporary, &self.root.join(NAME)).map_err(other)?;
        #[cfg(not(windows))]
        fs::rename(&temporary, self.root.join(NAME))?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "quad9-startup-log-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn records_only_fixed_events_and_numeric_codes() {
        let root = directory();
        let log = StartupLog::new(&root).unwrap();
        log.record(Event::PipeHostFailed, Some(5)).unwrap();
        log.prune().unwrap();
        let data: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(NAME)).unwrap()).unwrap();
        assert_eq!(data["version"], 1);
        let record = data["records"][0].as_object().unwrap();
        assert_eq!(record.len(), 3);
        assert_eq!(record["event"], "pipe_host_failed");
        assert_eq!(record["windows_error"], 5);
    }
    #[test]
    fn seven_day_expiry_and_clock_rollback_are_enforced_without_new_events() {
        let root = directory();
        let log = StartupLog::new(&root).unwrap();
        log.update(100, Some((Event::ControllerStartFailed, None)))
            .unwrap();
        log.update(100 + RETENTION_SECONDS - 1, None).unwrap();
        let read = || serde_json::from_slice::<Log>(&fs::read(root.join(NAME)).unwrap()).unwrap();
        assert_eq!(read().records.len(), 1);
        log.update(100 + RETENTION_SECONDS, None).unwrap();
        assert!(read().records.is_empty());
        log.update(200, Some((Event::ControllerStarting, None)))
            .unwrap();
        log.update(199, None).unwrap();
        assert!(read().records.is_empty());
    }
    #[test]
    fn record_count_bounds_serialized_size_and_retains_newest_events() {
        let mut log = Log {
            version: 1,
            records: (0..MAX_RECORDS + 1)
                .map(|i| Record {
                    unix_seconds: i as u64,
                    event: Event::ControllerStarted,
                    windows_error: Some(u32::MAX),
                })
                .collect(),
        };
        log.retain(MAX_RECORDS as u64);
        assert_eq!(log.records.len(), MAX_RECORDS);
        assert_eq!(log.records[0].unix_seconds, 1);
        assert!(serde_json::to_vec(&log).unwrap().len() < MAX_BYTES);
        for record in &mut log.records {
            record.unix_seconds = u64::MAX;
            record.event = Event::ControllerStartFailed;
        }
        assert!(serde_json::to_vec(&log).unwrap().len() < MAX_BYTES);
    }
    #[test]
    fn corrupt_oversized_and_unknown_format_logs_are_preserved_and_refused() {
        let root = directory();
        let log = StartupLog::new(&root).unwrap();
        for bytes in [
            b"invalid".to_vec(),
            vec![b'x'; MAX_BYTES + 1],
            br#"{"version":2,"records":[]}"#.to_vec(),
            br#"{"version":1,"records":[],"domain":"private.example"}"#.to_vec(),
        ] {
            fs::write(root.join(NAME), &bytes).unwrap();
            assert!(log
                .update(100, Some((Event::ServiceStopped, None)))
                .is_err());
            assert_eq!(fs::read(root.join(NAME)).unwrap(), bytes);
        }
    }
    #[test]
    fn stale_staging_file_is_replaced_without_touching_recovery_files() {
        let root = directory();
        fs::write(root.join(TEMPORARY), b"interrupted operational write").unwrap();
        fs::write(root.join("dns.journal.json"), b"recovery fixture").unwrap();
        let log = StartupLog::new(&root).unwrap();
        log.update(100, Some((Event::ServiceStopped, None)))
            .unwrap();
        assert!(!root.join(TEMPORARY).exists());
        assert_eq!(
            fs::read(root.join("dns.journal.json")).unwrap(),
            b"recovery fixture"
        );
    }
    #[test]
    fn concurrent_log_owner_is_refused_and_lock_releases_on_drop() {
        let root = directory();
        let log = StartupLog::new(&root).unwrap();
        assert!(StartupLog::new(&root).is_err());
        drop(log);
        assert!(StartupLog::new(&root).is_ok());
    }
}
