//! Power callbacks use opaque IDs rather than borrowed pointers. Removing an ID
//! makes late callbacks harmless without relying on undocumented callback-drain
//! behavior from PowerUnregisterSuspendResumeNotification.

use super::Signal;
use crate::NativeError;
use std::{
    collections::HashMap,
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
};
use windows_sys::Win32::{
    System::Power::{
        PowerRegisterSuspendResumeNotification, PowerUnregisterSuspendResumeNotification,
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
    },
    UI::WindowsAndMessaging::{
        DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    },
};

fn signals() -> &'static Mutex<HashMap<usize, Arc<Signal>>> {
    static SIGNALS: OnceLock<Mutex<HashMap<usize, Arc<Signal>>>> = OnceLock::new();
    SIGNALS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remove_signal(id: usize) {
    // The lock also waits for an already-running callback's bounded mark().
    signals()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
}

unsafe extern "system" fn power_changed(
    context: *const c_void,
    event: u32,
    _: *const c_void,
) -> u32 {
    if matches!(
        event,
        PBT_APMSUSPEND | PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND
    ) {
        // Context is an opaque never-reused ID, not an address to dereference.
        // Keep the lock through mark() so removal waits for callbacks that have
        // found their signal. No adapter read, IPC, DNS write, or blocking send.
        let signals = signals().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(signal) = signals.get(&(context as usize)) {
            signal.mark();
        }
    }
    0
}

pub(super) struct PowerWatcher {
    id: usize,
    handle: isize,
    parameters: Option<Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>>,
}

impl PowerWatcher {
    pub(super) fn start(signal: Arc<Signal>) -> Result<Self, NativeError> {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let id = NEXT_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| NativeError::InvalidData("Power notification IDs exhausted".into()))?;
        signals()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, signal);
        let mut parameters = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power_changed),
            Context: id as *mut c_void,
        });
        let mut handle = ptr::null_mut();
        // SAFETY: The boxed parameters stay at a stable address for the entire
        // registration. The callback interprets Context only as an opaque ID.
        let code = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                (&mut *parameters as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast(),
                &mut handle,
            )
        };
        if code != 0 || handle.is_null() {
            remove_signal(id);
            return Err(if code != 0 {
                NativeError::Windows {
                    operation: "Register power notifications",
                    code,
                }
            } else {
                NativeError::InvalidData(
                    "Power notification registration returned no handle".into(),
                )
            });
        }
        Ok(Self {
            id,
            handle: handle as isize,
            parameters: Some(parameters),
        })
    }
}

impl Drop for PowerWatcher {
    fn drop(&mut self) {
        remove_signal(self.id);
        // SAFETY: This is the sole owner of the successful registration. Its
        // signal is already removed and any late callback will do nothing.
        if unsafe { PowerUnregisterSuspendResumeNotification(self.handle) } != 0 {
            // Preserve the callback descriptor if cancellation fails. Its ID
            // stays invalid, so it cannot access any released application state.
            if let Some(parameters) = self.parameters.take() {
                std::mem::forget(parameters);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicU64, mpsc};

    fn signal() -> (Arc<Signal>, mpsc::Receiver<()>) {
        let (wake, receiver) = mpsc::sync_channel(1);
        (
            Arc::new(Signal {
                generation: Arc::new(AtomicU64::new(0)),
                wake,
                fault: Mutex::new(None),
            }),
            receiver,
        )
    }

    fn notify(id: usize, event: u32) {
        // SAFETY: The callback never dereferences Context or Setting. This
        // exercises the production callback without changing system power state.
        let code = unsafe { power_changed(id as *const c_void, event, ptr::null()) };
        assert_eq!(code, 0);
    }

    #[test]
    fn power_events_invalidate_without_ip_or_registry_changes() {
        let (signal, receiver) = signal();
        let watcher = PowerWatcher::start(signal.clone()).unwrap();
        for event in [PBT_APMSUSPEND, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND] {
            notify(watcher.id, event);
        }
        assert_eq!(signal.generation.load(Ordering::SeqCst), 3);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
        notify(watcher.id, u32::MAX);
        assert_eq!(signal.generation.load(Ordering::SeqCst), 3);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn repeated_registration_releases_signals_and_ignores_late_callbacks() {
        for _ in 0..3 {
            let (signal, receiver) = signal();
            let weak = Arc::downgrade(&signal);
            let generation = signal.generation.clone();
            let watcher = PowerWatcher::start(signal.clone()).unwrap();
            let id = watcher.id;
            drop(signal);
            drop(watcher);
            assert!(weak.upgrade().is_none());
            let before = generation.load(Ordering::SeqCst);
            notify(id, PBT_APMRESUMEAUTOMATIC);
            assert_eq!(generation.load(Ordering::SeqCst), before);
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn callbacks_racing_cancellation_cannot_mark_after_drop_returns() {
        let (signal, _) = signal();
        let watcher = PowerWatcher::start(signal.clone()).unwrap();
        let id = watcher.id;
        let (started, running) = mpsc::sync_channel(0);
        let (next, resume) = mpsc::sync_channel(0);
        let callbacks = std::thread::spawn(move || {
            notify(id, PBT_APMRESUMEAUTOMATIC);
            started.send(()).unwrap();
            resume.recv().unwrap();
            for _ in 0..1000 {
                notify(id, PBT_APMRESUMEAUTOMATIC);
            }
        });
        running.recv().unwrap();
        next.send(()).unwrap();
        drop(watcher);
        let after = signal.generation.load(Ordering::SeqCst);
        callbacks.join().unwrap();
        assert_eq!(signal.generation.load(Ordering::SeqCst), after);
    }
}
