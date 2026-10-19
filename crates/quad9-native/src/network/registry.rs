//! One-shot registry notifications, rearmed on a dedicated thread before the
//! final invalidation. Only KEY_NOTIFY access is requested in production.

use super::Signal;
use crate::NativeError;
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::{mpsc, Arc},
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Registry::*, Threading::*},
};

const CONFIG_PATHS: [&str; 4] = [
    r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
    r"SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters",
    // Includes both Parameters and InterfaceSpecificParameters, so DoH-only
    // changes are covered even when the DNS server addresses stay identical.
    r"SYSTEM\CurrentControlSet\Services\Dnscache",
    r"SOFTWARE\Policies\Microsoft\Windows NT\DNSClient",
];

#[derive(Clone, Copy)]
enum Hive {
    Machine,
    #[cfg(test)]
    User,
}
impl Hive {
    fn handle(self) -> HKEY {
        match self {
            Self::Machine => HKEY_LOCAL_MACHINE,
            #[cfg(test)]
            Self::User => HKEY_CURRENT_USER,
        }
    }
}
struct KeySpec {
    hive: Hive,
    path: String,
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: This is the single owner of a key opened by RegOpenKeyExW.
        unsafe { RegCloseKey(self.0) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn error(operation: &'static str, code: u32) -> NativeError {
    NativeError::Windows { operation, code }
}
fn last_error(operation: &'static str) -> NativeError {
    // SAFETY: Read the calling thread's last error immediately after failure.
    error(operation, unsafe { GetLastError() })
}
fn event(manual: bool) -> Result<OwnedHandle, NativeError> {
    // SAFETY: An unnamed event has one owned handle and no borrowed attributes.
    let raw = unsafe { CreateEventW(ptr::null(), i32::from(manual), 0, ptr::null()) };
    if raw.is_null() {
        return Err(last_error("CreateEventW"));
    }
    // SAFETY: The valid event handle is transferred to its sole Rust owner.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn open_nearest(spec: &KeySpec) -> Result<Key, NativeError> {
    let mut path = spec.path.as_str();
    loop {
        let mut key = ptr::null_mut();
        // SAFETY: A fixed local hive and a live NUL-terminated path are supplied.
        // KEY_NOTIFY does not grant value reads or any registry mutation rights.
        let code = unsafe {
            RegOpenKeyExW(
                spec.hive.handle(),
                wide(path).as_ptr(),
                0,
                KEY_NOTIFY | KEY_WOW64_64KEY,
                &mut key,
            )
        };
        if code == 0 {
            return Ok(Key(key));
        }
        if !matches!(
            code,
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_KEY_DELETED
        ) {
            return Err(error("Open DNS notification key", code));
        }
        // An optional policy key may not exist yet. Watch its closest existing
        // ancestor, including subtree names, so later creation is also observed.
        let Some((parent, _)) = path.rsplit_once('\\') else {
            return Err(error("Find DNS notification ancestor", code));
        };
        path = parent;
    }
}

struct Entry {
    spec: KeySpec,
    event: OwnedHandle,
    key: Option<Key>,
}
impl Entry {
    fn new(spec: KeySpec) -> Result<Self, NativeError> {
        let mut entry = Self {
            spec,
            event: event(false)?,
            key: None,
        };
        entry.arm()?;
        Ok(entry)
    }
    fn arm(&mut self) -> Result<(), NativeError> {
        // Cancel the old handle before reopening. Reusing a deleted key handle
        // would never notice a new key created at the same registry path.
        self.key.take();
        for _ in 0..4 {
            // SAFETY: Clear only the completed/cancelled registration's event.
            // No new registration exists until the following RegNotify call.
            if unsafe { ResetEvent(self.event.as_raw_handle()) } == 0 {
                return Err(last_error("Reset DNS notification event"));
            }
            let key = open_nearest(&self.spec)?;
            // SAFETY: Both handles remain owned by this entry. Each registration
            // is one-shot and is rearmed only after its event has been consumed.
            let code = unsafe {
                RegNotifyChangeKeyValue(
                    key.0,
                    1,
                    REG_NOTIFY_CHANGE_NAME
                        | REG_NOTIFY_CHANGE_LAST_SET
                        | REG_NOTIFY_CHANGE_SECURITY
                        | REG_NOTIFY_THREAD_AGNOSTIC,
                    self.event.as_raw_handle(),
                    1,
                )
            };
            if code == ERROR_KEY_DELETED {
                continue;
            }
            if code != 0 {
                return Err(error("Arm DNS notification", code));
            }
            self.key = Some(key);
            return Ok(());
        }
        Err(NativeError::InvalidData(
            "DNS notification key kept changing during registration".into(),
        ))
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        // RegCloseKey cancels a pending notification and can signal its event.
        // Close the key before Rust closes the event handle.
        self.key.take();
    }
}

pub(super) struct RegistryWatcher {
    stop: Arc<OwnedHandle>,
    worker: Option<JoinHandle<()>>,
}
impl RegistryWatcher {
    pub(super) fn start(signal: Arc<Signal>) -> Result<Self, NativeError> {
        Self::start_paths(
            signal,
            CONFIG_PATHS
                .into_iter()
                .map(|path| KeySpec {
                    hive: Hive::Machine,
                    path: path.into(),
                })
                .collect(),
        )
    }
    fn start_paths(signal: Arc<Signal>, paths: Vec<KeySpec>) -> Result<Self, NativeError> {
        let stop = Arc::new(event(true)?);
        let thread_stop = stop.clone();
        let (ready, started) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("quad9-dns-notifications".into())
            .spawn(move || {
                let entries = paths
                    .into_iter()
                    .map(Entry::new)
                    .collect::<Result<Vec<_>, _>>();
                let entries = match entries {
                    Ok(entries) => entries,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = ready.send(Ok(()));
                if let Err(error) = Self::run(&thread_stop, entries, &signal) {
                    signal.fail(error.to_string());
                }
            })
            .map_err(|e| {
                NativeError::InvalidData(format!("Cannot start DNS notification thread: {e}"))
            })?;
        let watcher = Self {
            stop,
            worker: Some(worker),
        };
        started
            .recv()
            .map_err(|_| {
                NativeError::InvalidData("DNS notification thread failed during startup".into())
            })?
            .map_err(NativeError::InvalidData)?;
        Ok(watcher)
    }
    fn run(
        stop: &OwnedHandle,
        mut entries: Vec<Entry>,
        signal: &Signal,
    ) -> Result<(), NativeError> {
        let mut handles = vec![stop.as_raw_handle()];
        handles.extend(entries.iter().map(|e| e.event.as_raw_handle()));
        loop {
            // SAFETY: All handles remain live on this thread. The fixed set has
            // four registry events and one stop event, below the Windows limit.
            let code = unsafe {
                WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE)
            };
            if code == WAIT_OBJECT_0 {
                return Ok(());
            }
            if code == WAIT_FAILED {
                return Err(last_error("Wait for DNS notification"));
            }
            let index = code.wrapping_sub(WAIT_OBJECT_0 + 1) as usize;
            let Some(entry) = entries.get_mut(index) else {
                return Err(NativeError::InvalidData(
                    "Unexpected DNS notification wait result".into(),
                ));
            };
            // Invalidate both before and after rearming. A read begun while
            // registrations are being replaced cannot publish an old result.
            signal.mark();
            entry.arm()?;
            signal.mark();
        }
    }
}
impl Drop for RegistryWatcher {
    fn drop(&mut self) {
        // SAFETY: The owned manual-reset event is shared with the worker and
        // remains alive until after join has drained all registry work.
        unsafe { SetEvent(self.stop.as_raw_handle()) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "vm-validation"))]
mod vm_tests;
