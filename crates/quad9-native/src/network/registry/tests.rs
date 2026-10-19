use super::*;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    path: String,
    key: Option<Key>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = format!(
            r"Software\Quad9ConnectTests-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut key = ptr::null_mut();
        let mut disposition = 0;
        // SAFETY: Create only this test's unique HKCU key, never DNS settings.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&path).as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_ALL_ACCESS,
                ptr::null(),
                &mut key,
                &mut disposition,
            )
        };
        assert_eq!(code, 0);
        assert_eq!(
            disposition, REG_CREATED_NEW_KEY,
            "Do not reuse an existing test key"
        );
        Self {
            path,
            key: Some(Key(key)),
        }
    }
    fn child(&self, path: &str) -> Key {
        let mut key = ptr::null_mut();
        assert_eq!(
            // SAFETY: The parent is this test's exclusive HKCU namespace.
            unsafe {
                RegCreateKeyExW(
                    self.key.as_ref().unwrap().0,
                    wide(path).as_ptr(),
                    0,
                    ptr::null(),
                    0,
                    KEY_ALL_ACCESS,
                    ptr::null(),
                    &mut key,
                    ptr::null_mut(),
                )
            },
            0
        );
        Key(key)
    }
    fn delete_child(&self, path: &str) {
        assert_eq!(
            // SAFETY: Delete only a subtree below the unique key created above.
            unsafe { RegDeleteTreeW(self.key.as_ref().unwrap().0, wide(path).as_ptr()) },
            0
        );
    }
    fn spec(&self, child: &str) -> KeySpec {
        KeySpec {
            hive: Hive::User,
            path: if child.is_empty() {
                self.path.clone()
            } else {
                format!(r"{}\{child}", self.path)
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.key.take();
        // SAFETY: This exact unique path was confirmed newly created by this test.
        let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&self.path).as_ptr()) };
        assert_eq!(code, 0, "Could not remove the private registry test key");
    }
}
fn set(key: &Key, value: u32) {
    let bytes = value.to_le_bytes();
    assert_eq!(
        // SAFETY: The fixture owns this writable key; the DWORD buffer remains live.
        unsafe {
            RegSetValueExW(
                key.0,
                wide("Marker").as_ptr(),
                0,
                REG_DWORD,
                bytes.as_ptr(),
                4,
            )
        },
        0
    );
}
pub(super) fn signal() -> (Arc<Signal>, mpsc::Receiver<()>) {
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
#[track_caller]
pub(super) fn rearmed(signal: &Signal, receiver: &mpsc::Receiver<()>, before: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while signal.generation.load(Ordering::SeqCst) < before + 2 {
        receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        signal.health().unwrap();
    }
    // Creating a nested key can produce several coalesced notifications. Drain
    // the burst before starting the next independent edit in this API test.
    while receiver.recv_timeout(Duration::from_millis(50)).is_ok() {
        signal.health().unwrap();
        assert!(
            Instant::now() < deadline,
            "Notification burst did not settle"
        );
    }
}

#[test]
fn registry_value_changes_rearm_without_ip_notifications_or_polling() {
    let fixture = Fixture::new();
    let (signal, receiver) = signal();
    let watcher = RegistryWatcher::start_paths(signal.clone(), vec![fixture.spec("")]).unwrap();
    for value in 1..=5 {
        let before = signal.generation.load(Ordering::SeqCst);
        set(fixture.key.as_ref().unwrap(), value);
        rearmed(&signal, &receiver, before);
    }
    drop(watcher);
    let stopped = signal.generation.load(Ordering::SeqCst);
    set(fixture.key.as_ref().unwrap(), 6);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(signal.generation.load(Ordering::SeqCst), stopped);
}

#[test]
fn missing_policy_key_creation_deletion_and_recreation_remain_observed() {
    let fixture = Fixture::new();
    let (signal, receiver) = signal();
    let _watcher =
        RegistryWatcher::start_paths(signal.clone(), vec![fixture.spec(r"Policy\DNSClient")])
            .unwrap();
    let before = signal.generation.load(Ordering::SeqCst);
    drop(fixture.child(r"Policy\DNSClient"));
    rearmed(&signal, &receiver, before);
    let before = signal.generation.load(Ordering::SeqCst);
    fixture.delete_child("Policy");
    rearmed(&signal, &receiver, before);
    let before = signal.generation.load(Ordering::SeqCst);
    let recreated = fixture.child(r"Policy\DNSClient");
    rearmed(&signal, &receiver, before);
    let before = signal.generation.load(Ordering::SeqCst);
    set(&recreated, 1);
    rearmed(&signal, &receiver, before);
}

#[test]
fn startup_failure_does_not_leave_a_partial_subscription() {
    let fixture = Fixture::new();
    let (signal, _receiver) = signal();
    let result = RegistryWatcher::start_paths(
        signal.clone(),
        vec![
            fixture.spec(""),
            KeySpec {
                hive: Hive::User,
                path: format!("Quad9ConnectMissing-{}", std::process::id()),
            },
        ],
    );
    assert!(result.is_err());
    let before = signal.generation.load(Ordering::SeqCst);
    set(fixture.key.as_ref().unwrap(), 1);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(signal.generation.load(Ordering::SeqCst), before);
}
