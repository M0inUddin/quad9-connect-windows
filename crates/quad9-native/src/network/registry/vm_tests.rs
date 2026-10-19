//! Mutating tests run only when explicitly selected in disposable hosted VMs.

use super::{
    tests::{rearmed, signal},
    *,
};
use crate::{inspect_adapters, AdapterKind, WindowsDnsBackend};
use quad9_core::{AddressFamily, DnsBackend, DnsSettings, Target};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::{core::GUID, Win32::NetworkManagement::IpHelper::*};

fn require_disposable_vm() {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
}
fn target(family: AddressFamily) -> Target {
    let adapter = inspect_adapters()
        .unwrap()
        .into_iter()
        .find(|a| {
            a.connected && a.kind == AdapterKind::Ethernet && a.ipv4_enabled && a.has_ipv4_dns
        })
        .expect("Disposable VM needs a connected Ethernet resolver scope");
    Target {
        adapter_id: adapter.id,
        family,
    }
}

#[test]
#[ignore = "writes DNS; explicitly run only in disposable GitHub-hosted VMs"]
fn vm_dns_only_writes_and_exact_restores_notify_without_ip_subscriptions() {
    require_disposable_vm();
    let mut backend = WindowsDnsBackend::for_disposable_vm();
    for family in [AddressFamily::Ipv4, AddressFamily::Ipv6] {
        let target = target(family);
        let original = backend.read(&target).unwrap();
        let profile = DnsSettings::quad9(family);
        assert_ne!(
            original, profile,
            "The VM must start with its own DNS baseline"
        );
        let protocol = if family == AddressFamily::Ipv4 {
            "Tcpip"
        } else {
            "Tcpip6"
        };
        let (signal, receiver) = signal();
        // No IP Helper subscriptions exist in this observer. Only the selected
        // adapter's actual DNS registry path can produce the observed wakeup.
        let _watcher = RegistryWatcher::start_paths(
            signal.clone(),
            vec![KeySpec {
                hive: Hive::Machine,
                path: format!(
                    r"SYSTEM\CurrentControlSet\Services\{protocol}\Parameters\Interfaces\{{{}}}",
                    target.adapter_id.as_str()
                ),
            }],
        )
        .unwrap();
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            let before = signal.generation.load(Ordering::SeqCst);
            backend.write(&target, &profile).unwrap();
            rearmed(&signal, &receiver, before);
            assert_eq!(backend.read(&target).unwrap(), profile);
        }));
        // Restore even if the notification/readback assertion panics.
        let before = signal.generation.load(Ordering::SeqCst);
        backend
            .write(&target, &original)
            .expect("Restore the VM DNS baseline");
        assert_eq!(backend.read(&target).unwrap(), original);
        if let Err(panic) = attempt {
            std::panic::resume_unwind(panic);
        }
        rearmed(&signal, &receiver, before);
    }
}

#[test]
#[ignore = "writes DNS; explicitly run only in disposable GitHub-hosted VMs"]
fn vm_doh_property_changes_notify_from_dns_client_settings() {
    require_disposable_vm();
    let mut backend = WindowsDnsBackend::for_disposable_vm();
    for family in [AddressFamily::Ipv4, AddressFamily::Ipv6] {
        let target = target(family);
        let original = backend.read(&target).unwrap();
        let profile = DnsSettings::quad9(family);
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            backend.write(&target, &profile).unwrap();
            assert_eq!(backend.read(&target).unwrap(), profile);
            let (signal, receiver) = signal();
            let _watcher = RegistryWatcher::start_paths(
                signal.clone(),
                vec![KeySpec {
                    hive: Hive::Machine,
                    path: CONFIG_PATHS[2].into(),
                }],
            )
            .unwrap();
            let mut changed = profile.clone();
            // The explicit default TLS port keeps the same encrypted endpoint
            // while changing only the per-server DoH template text.
            changed.doh[0].template = Some("https://dns.quad9.net:443/dns-query".into());
            let before = signal.generation.load(Ordering::SeqCst);
            write_doh_only(&target, &changed);
            rearmed(&signal, &receiver, before);
            let observed = backend.read(&target).unwrap();
            assert_eq!(observed, changed);
            assert_eq!(observed.assignment, profile.assignment);
        }));
        backend
            .write(&target, &original)
            .expect("Restore the VM DNS baseline");
        assert_eq!(backend.read(&target).unwrap(), original);
        if let Err(panic) = attempt {
            std::panic::resume_unwind(panic);
        }
    }
}

fn write_doh_only(target: &Target, settings: &DnsSettings) {
    let templates: Vec<_> = settings
        .doh
        .iter()
        .map(|p| wide(p.template.as_ref().unwrap()))
        .collect();
    let mut doh: Vec<_> = settings
        .doh
        .iter()
        .enumerate()
        .map(|(i, p)| DNS_DOH_SERVER_SETTINGS {
            Template: templates[i].as_ptr().cast_mut(),
            Flags: p.flags,
        })
        .collect();
    let mut properties: Vec<_> = settings
        .doh
        .iter()
        .enumerate()
        .map(|(i, p)| DNS_SERVER_PROPERTY {
            Version: DNS_SERVER_PROPERTY_VERSION1,
            ServerIndex: p.server_index,
            Type: DnsServerDohProperty,
            Property: DNS_SERVER_PROPERTY_TYPES {
                DohSettings: &mut doh[i],
            },
        })
        .collect();
    let raw = DNS_INTERFACE_SETTINGS3 {
        Version: DNS_INTERFACE_SETTINGS_VERSION3,
        // Deliberately do not select DNS_SETTING_NAMESERVER. The test must
        // observe a real DoH-only edit, with no address field being rewritten.
        Flags: DNS_SETTING_DOH as u64
            | if target.family == AddressFamily::Ipv6 {
                DNS_SETTING_IPV6 as u64
            } else {
                0
            },
        cServerProperties: properties.len() as u32,
        ServerProperties: properties.as_mut_ptr(),
        ..Default::default()
    };
    let guid = GUID::from_u128(
        u128::from_str_radix(&target.adapter_id.as_str().replace('-', ""), 16).unwrap(),
    );
    // SAFETY: The validated target and all DoH buffers remain live through this
    // synchronous VM-only call. No NameServer pointer or address flag is passed.
    let code =
        unsafe { SetInterfaceDnsSettings(guid, (&raw as *const DNS_INTERFACE_SETTINGS3).cast()) };
    assert_eq!(code, 0, "DoH-only native write failed");
}

struct PolicyMarker {
    key: Key,
    name: String,
}
impl PolicyMarker {
    fn create() -> Self {
        let mut key = ptr::null_mut();
        // SAFETY: This ignored VM-only test opens the exact DNS policy key. The
        // unique marker below is not a Windows DNS policy and cannot change DNS.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                wide(CONFIG_PATHS[3]).as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_SET_VALUE,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        };
        assert_eq!(code, 0);
        Self {
            key: Key(key),
            name: format!(
                "Quad9ConnectValidation-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
        }
    }
    fn set(&self) {
        let bytes = 1u32.to_le_bytes();
        // SAFETY: The VM-only marker key grants value writes and bytes is live.
        let code = unsafe {
            RegSetValueExW(
                self.key.0,
                wide(&self.name).as_ptr(),
                0,
                REG_DWORD,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        assert_eq!(code, 0);
    }
    fn remove(&self) {
        // SAFETY: Remove only this VM test's uniquely named value.
        let code = unsafe { RegDeleteValueW(self.key.0, wide(&self.name).as_ptr()) };
        assert!(code == 0 || code == ERROR_FILE_NOT_FOUND);
    }
}
impl Drop for PolicyMarker {
    fn drop(&mut self) {
        self.remove();
    }
}

#[test]
#[ignore = "adds a policy marker; explicitly run only in disposable GitHub-hosted VMs"]
fn vm_policy_only_edits_notify_and_block_writes_until_the_marker_is_removed() {
    require_disposable_vm();
    let target = target(AddressFamily::Ipv4);
    let mut backend = WindowsDnsBackend::for_disposable_vm();
    let original = backend.read(&target).unwrap();
    let (signal, receiver) = signal();
    let _watcher = RegistryWatcher::start_paths(
        signal.clone(),
        vec![KeySpec {
            hive: Hive::Machine,
            path: CONFIG_PATHS[3].into(),
        }],
    )
    .unwrap();
    let marker = PolicyMarker::create();
    // Key creation may itself notify. Let it settle before the value-only edit.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let before = signal.generation.load(Ordering::SeqCst);
    marker.set();
    rearmed(&signal, &receiver, before);
    assert!(backend
        .read(&target)
        .unwrap_err()
        .to_string()
        .contains("Managed DNS"));
    assert!(backend
        .write(&target, &DnsSettings::quad9(AddressFamily::Ipv4))
        .is_err());
    let before = signal.generation.load(Ordering::SeqCst);
    marker.remove();
    rearmed(&signal, &receiver, before);
    assert_eq!(backend.read(&target).unwrap(), original);
}
