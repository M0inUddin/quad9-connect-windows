use crate::{protocol::ServiceState, runtime::StateStore};
use quad9_core::{CoreError, Journal, JournalStore};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_STATE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    version: u32,
    payload: T,
}

/// The host supplies an already validated, protected machine directory.
/// No directory or filename is accepted from IPC requests.
pub struct AtomicFiles {
    root: PathBuf,
    _lock: File,
}

fn storage_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Journal(error.to_string())
}

fn validate_file(file: &File) -> Result<(), CoreError> {
    #[cfg(all(windows, not(test)))]
    quad9_native::service::validate_private_file(file).map_err(storage_error)?;
    #[cfg(any(not(windows), test))]
    let _ = file;
    Ok(())
}

fn reject_reparse(path: &Path) -> Result<bool, CoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(storage_error("Recovery files must not be reparse points"));
                }
            }
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(storage_error("Recovery path is not a regular file"));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage_error(error)),
    }
}

impl AtomicFiles {
    pub fn open_protected_directory(root: &Path) -> Result<Self, CoreError> {
        let lock_path = root.join("service.journal.lock");
        reject_reparse(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0).custom_flags(0x0020_0000); // OPEN_REPARSE_POINT
        }
        let lock = options.open(lock_path).map_err(storage_error)?;
        validate_file(&lock)?;
        lock.try_lock().map_err(storage_error)?;
        Ok(Self {
            root: root.into(),
            _lock: lock,
        })
    }

    fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, CoreError> {
        let path = self.root.join(name);
        if !reject_reparse(&path)? {
            return Ok(None);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000);
        }
        let file = options.open(path).map_err(storage_error)?;
        validate_file(&file)?;
        let mut bytes = Vec::new();
        file.take(MAX_STATE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(storage_error("Recovery file exceeds its size limit"));
        }
        let envelope: Envelope<T> = serde_json::from_slice(&bytes).map_err(storage_error)?;
        if envelope.version != 1 {
            return Err(storage_error("Unsupported recovery envelope version"));
        }
        Ok(Some(envelope.payload))
    }

    fn write<T: Serialize>(&self, name: &str, payload: &T) -> Result<(), CoreError> {
        let destination = self.root.join(name);
        reject_reparse(&destination)?;
        let temporary = self.root.join(format!("{name}.tmp"));
        if reject_reparse(&temporary)? {
            fs::remove_file(&temporary).map_err(storage_error)?;
        }
        let bytes = serde_json::to_vec(&Envelope {
            version: 1,
            payload,
        })
        .map_err(storage_error)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(storage_error("Recovery state exceeds its size limit"));
        }
        let mut file = File::create_new(&temporary).map_err(storage_error)?;
        validate_file(&file)?;
        file.write_all(&bytes).map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
        drop(file);
        #[cfg(windows)]
        quad9_native::replace_file(&temporary, &destination).map_err(storage_error)?;
        #[cfg(not(windows))]
        {
            fs::rename(&temporary, &destination).map_err(storage_error)?;
            File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(storage_error)?;
        }
        Ok(())
    }
}

impl JournalStore for AtomicFiles {
    fn load(&mut self) -> Result<Option<Journal>, CoreError> {
        let journal: Option<Journal> = self.read("dns.journal.json")?;
        if let Some(journal) = &journal {
            journal.validate()?;
        }
        Ok(journal)
    }
    fn save(&mut self, journal: &Journal) -> Result<(), CoreError> {
        journal.validate()?;
        self.write("dns.journal.json", journal)
    }
}

impl StateStore for AtomicFiles {
    fn load_state(&mut self) -> Result<ServiceState, CoreError> {
        let state: ServiceState = self.read("requests.journal.json")?.unwrap_or_default();
        state.validate().map_err(storage_error)?;
        Ok(state)
    }
    fn save_state(&mut self, state: &ServiceState) -> Result<(), CoreError> {
        state.validate().map_err(storage_error)?;
        self.write("requests.journal.json", state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "quad9-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn cleanup(path: &Path) {
        for name in [
            "dns.journal.json",
            "requests.journal.json",
            "requests.journal.json.tmp",
            "service.journal.lock",
        ] {
            let _ = fs::remove_file(path.join(name));
        }
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn exclusive_owner_and_state_survive_reopen() {
        let path = directory();
        let mut store = AtomicFiles::open_protected_directory(&path).unwrap();
        assert!(AtomicFiles::open_protected_directory(&path).is_err());
        store
            .save_state(&ServiceState {
                revision: 7,
                ..Default::default()
            })
            .unwrap();
        drop(store);
        let mut reopened = AtomicFiles::open_protected_directory(&path).unwrap();
        assert_eq!(reopened.load_state().unwrap().revision, 7);
        drop(reopened);
        cleanup(&path);
    }

    #[test]
    fn unfinished_temporary_file_does_not_replace_committed_state() {
        let path = directory();
        let mut store = AtomicFiles::open_protected_directory(&path).unwrap();
        store
            .save_state(&ServiceState {
                revision: 1,
                ..Default::default()
            })
            .unwrap();
        fs::write(path.join("requests.journal.json.tmp"), "incomplete").unwrap();
        assert_eq!(store.load_state().unwrap().revision, 1);
        store
            .save_state(&ServiceState {
                revision: 2,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.load_state().unwrap().revision, 2);
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn corrupt_unknown_or_oversized_files_fail_closed() {
        let path = directory();
        let mut store = AtomicFiles::open_protected_directory(&path).unwrap();
        for bytes in [
            b"broken".to_vec(),
            br#"{"version":999,"payload":{}}"#.to_vec(),
            vec![b' '; MAX_STATE_BYTES + 1],
        ] {
            fs::write(path.join("requests.journal.json"), bytes).unwrap();
            assert!(store.load_state().is_err());
        }
        drop(store);
        cleanup(&path);
    }
}
