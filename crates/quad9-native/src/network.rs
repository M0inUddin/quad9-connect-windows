//! Read-only IP interface, address, and route notifications.
//!
//! Callbacks only advance an observation generation and coalesce a wakeup. They
//! never inspect adapters, perform IPC, or write DNS. DNS-only registry changes
//! are outside this subscription's coverage.

use crate::NativeError;
use std::{
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::HANDLE, NetworkManagement::IpHelper::*, Networking::WinSock::AF_UNSPEC,
};

struct Signal {
    generation: Arc<AtomicU64>,
    wake: SyncSender<()>,
}

unsafe fn changed(context: *const c_void) {
    // SAFETY: Each registration borrows the boxed Signal until cancellation has
    // drained its callbacks. No callback stores this reference beyond the call.
    let signal = unsafe { &*context.cast::<Signal>() };
    signal.generation.fetch_add(1, Ordering::SeqCst);
    // One pending wakeup covers any burst; the generation still counts changes.
    let _ = signal.wake.try_send(());
}

unsafe extern "system" fn interface_changed(
    context: *const c_void,
    _: *const MIB_IPINTERFACE_ROW,
    _: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: Windows returns the context supplied by this live subscription.
    unsafe { changed(context) };
}
unsafe extern "system" fn address_changed(
    context: *const c_void,
    _: *const MIB_UNICASTIPADDRESS_ROW,
    _: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: Windows returns the context supplied by this live subscription.
    unsafe { changed(context) };
}
unsafe extern "system" fn route_changed(
    context: *const c_void,
    _: *const MIB_IPFORWARD_ROW2,
    _: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: Windows returns the context supplied by this live subscription.
    unsafe { changed(context) };
}

/// Own and drop this on the observer thread, never inside a notification callback.
pub struct NetworkWatcher {
    registrations: Vec<HANDLE>,
    signal: Option<Box<Signal>>,
    receiver: Receiver<()>,
}

impl NetworkWatcher {
    pub fn start(generation: Arc<AtomicU64>) -> Result<Self, NativeError> {
        Self::register(generation, false)
    }

    fn register(generation: Arc<AtomicU64>, initial: bool) -> Result<Self, NativeError> {
        let (wake, receiver) = mpsc::sync_channel(1);
        let signal = Box::new(Signal { generation, wake });
        let context = (&*signal as *const Signal).cast();
        let mut watcher = Self {
            registrations: Vec::new(),
            signal: Some(signal),
            receiver,
        };
        for kind in 0..3 {
            let mut handle = ptr::null_mut();
            // SAFETY: The Signal has a stable address, even if watcher moves.
            // Windows owns each notification handle until explicit cancellation.
            let code = unsafe {
                match kind {
                    0 => NotifyIpInterfaceChange(
                        AF_UNSPEC,
                        Some(interface_changed),
                        context,
                        initial,
                        &mut handle,
                    ),
                    1 => NotifyUnicastIpAddressChange(
                        AF_UNSPEC,
                        Some(address_changed),
                        context,
                        initial,
                        &mut handle,
                    ),
                    _ => NotifyRouteChange2(
                        AF_UNSPEC,
                        Some(route_changed),
                        context,
                        initial,
                        &mut handle,
                    ),
                }
            };
            if code != 0 {
                return Err(NativeError::Windows {
                    operation: "Register network notifications",
                    code,
                });
            }
            watcher.registrations.push(handle);
        }
        Ok(watcher)
    }

    /// Wait for an event, with no adapter polling or DNS query.
    pub fn wait(&self, timeout: Duration) -> bool {
        self.receiver.recv_timeout(timeout).is_ok()
    }
}

impl Drop for NetworkWatcher {
    fn drop(&mut self) {
        let mut drained = true;
        for handle in self.registrations.drain(..) {
            // SAFETY: Drop runs off the callback thread and holds no resource a
            // callback needs. Cancellation waits for callbacks before returning.
            drained &= unsafe { CancelMibChangeNotify2(handle) } == 0;
        }
        if !drained {
            // Retain the callback context if Windows cannot confirm cancellation.
            // Leaking this small allocation is safer than freeing a live context.
            if let Some(signal) = self.signal.take() {
                std::mem::forget(signal);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_windows_callbacks_wake_and_can_be_cancelled_repeatedly() {
        for _ in 0..3 {
            let generation = Arc::new(AtomicU64::new(0));
            let watcher = NetworkWatcher::register(generation.clone(), true).unwrap();
            assert!(watcher.wait(Duration::from_secs(5)));
            assert!(generation.load(Ordering::SeqCst) > 0);
            drop(watcher);
            let after = generation.load(Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(generation.load(Ordering::SeqCst), after);
        }
    }

    #[test]
    fn event_bursts_coalesce_without_losing_generation_changes() {
        let generation = Arc::new(AtomicU64::new(0));
        let (wake, receiver) = mpsc::sync_channel(1);
        let signal = Signal {
            generation: generation.clone(),
            wake,
        };
        for _ in 0..100 {
            // SAFETY: The signal is live and each callback returns synchronously.
            unsafe { changed((&signal as *const Signal).cast()) };
        }
        assert_eq!(generation.load(Ordering::SeqCst), 100);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }
}
