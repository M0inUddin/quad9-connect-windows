use crate::*;
use quad9_core::*;
use std::{net::IpAddr, ptr};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::ERROR_FILE_NOT_FOUND, NetworkManagement::IpHelper::*, System::Registry::*,
    },
};

pub struct WindowsDnsBackend {
    validation_vm: bool,
}

fn failure(error: impl std::fmt::Display) -> CoreError {
    CoreError::Backend(error.to_string())
}

fn guid(id: &AdapterId) -> GUID {
    GUID::from_u128(
        u128::from_str_radix(&id.as_str().replace('-', ""), 16).expect("Validated adapter GUID"),
    )
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn implicit_ipv6_discovery(id: &AdapterId, loopback: bool) -> bool {
    let path = format!(
        r"SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters\Interfaces\{{{}}}",
        id.as_str()
    );
    if !registry_string(&path, "NameServer")
        .is_ok_and(|value| value.is_none_or(|s| s.trim().is_empty()))
    {
        return false;
    }
    match crate::windows::read_dns_family(guid(id), true) {
        Ok(raw) => {
            raw.name_servers.is_none_or(|s| s.trim().is_empty())
                && raw.profile_name_servers.is_none_or(|s| s.trim().is_empty())
                && raw.server_properties.is_empty()
                && raw.profile_server_properties.is_empty()
        }
        // The software loopback can enumerate discovery defaults despite having
        // no IPv6 DNS settings object. Static settings were checked above, and
        // the caller also requires that no route can carry these addresses.
        Err(NativeError::Windows {
            code: ERROR_FILE_NOT_FOUND,
            ..
        }) if loopback => true,
        _ => false,
    }
}

fn registry_string(path: &str, value: &str) -> Result<Option<String>, CoreError> {
    let path = wide(path);
    let value = wide(value);
    let mut bytes = 0u32;
    // SAFETY: Fixed HKLM paths and value names are valid UTF-16. The first call
    // only asks Windows for the required size of a string value.
    let code = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != 0 {
        return Err(failure(format!("Static DNS registry read failed: {code}")));
    }
    if bytes > 65_536 || !bytes.is_multiple_of(2) {
        return Err(failure("Invalid static DNS string size"));
    }
    let mut buffer = vec![0u16; bytes as usize / 2];
    // SAFETY: The buffer is aligned and large enough for the byte count.
    let code = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if code != 0 {
        return Err(failure(format!("Static DNS registry read failed: {code}")));
    }
    while buffer.last() == Some(&0) {
        buffer.pop();
    }
    String::from_utf16(&buffer).map(Some).map_err(failure)
}

fn policy_key_populated(path: &str) -> Result<bool, CoreError> {
    let mut key = ptr::null_mut();
    // SAFETY: path is fixed, NUL-terminated, and the out-pointer is valid.
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(path).as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if code != 0 {
        return Err(failure(format!("DNS policy inspection failed: {code}")));
    }
    let mut subkeys = 0;
    let mut values = 0;
    // SAFETY: key is a live read-only registry handle; optional outputs are null.
    let code = unsafe {
        RegQueryInfoKeyW(
            key,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
            &mut subkeys,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut values,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    // SAFETY: key was opened above and is released exactly once.
    unsafe {
        RegCloseKey(key);
    }
    if code != 0 {
        return Err(failure(format!("DNS policy inspection failed: {code}")));
    }
    Ok(subkeys != 0 || values != 0)
}

fn check_policies() -> Result<(), CoreError> {
    for path in [
        r"SOFTWARE\Policies\Microsoft\Windows NT\DNSClient",
        r"SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig",
    ] {
        if policy_key_populated(path)? {
            return Err(failure(
                "Managed DNS or namespace policy requires separate compatibility validation",
            ));
        }
    }
    Ok(())
}

fn addresses(value: Option<&str>) -> Result<Vec<IpAddr>, CoreError> {
    value
        .unwrap_or("")
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<IpAddr>().map_err(failure))
        .collect()
}

impl Default for WindowsDnsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsDnsBackend {
    pub fn new() -> Self {
        Self {
            validation_vm: false,
        }
    }

    #[cfg(feature = "vm-validation")]
    pub fn for_disposable_vm() -> Self {
        Self {
            validation_vm: true,
        }
    }

    pub fn validate_scope(&mut self, targets: &[Target]) -> Result<(), CoreError> {
        check_policies()?;
        let adapters = inspect_adapters().map_err(failure)?;
        if !self.validation_vm {
            for adapter in adapters.iter().filter(|a| a.connected) {
                for (family, active) in [
                    (AddressFamily::Ipv4, adapter.has_ipv4_dns),
                    (AddressFamily::Ipv6, adapter.has_ipv6_dns),
                ] {
                    if active
                        && !targets
                            .iter()
                            .any(|t| t.adapter_id == adapter.id && t.family == family)
                    {
                        return Err(failure("Another active resolver scope is outside the selection; inspect adapters and VPNs before enabling"));
                    }
                }
            }
        }
        for target in targets {
            let adapter = adapters
                .iter()
                .find(|a| a.id == target.adapter_id)
                .ok_or_else(|| failure("Selected adapter is absent"))?;
            let allowed = adapter.candidate
                || (self.validation_vm
                    && adapter.connected
                    && matches!(adapter.kind, AdapterKind::Ethernet | AdapterKind::Wifi));
            if !allowed {
                return Err(failure(
                    "Selected adapter is not an eligible connected physical adapter",
                ));
            }
            if (target.family == AddressFamily::Ipv4 && !adapter.ipv4_enabled)
                || (target.family == AddressFamily::Ipv6 && !adapter.ipv6_enabled)
            {
                return Err(failure("Selected address family is not enabled"));
            }
            // IPv6 requires a preferred global address and an IPv6 gateway.
            if !self.validation_vm && target.family == AddressFamily::Ipv6 && !adapter.ipv6_usable {
                return Err(failure(
                    "Selected IPv6 scope has no usable global IPv6 address and gateway",
                ));
            }
            self.read(target)?;
        }
        Ok(())
    }
}

impl DnsBackend for WindowsDnsBackend {
    fn read(&mut self, target: &Target) -> Result<DnsSettings, CoreError> {
        check_policies()?;
        let raw = crate::windows::read_dns_family(
            guid(&target.adapter_id),
            target.family == AddressFamily::Ipv6,
        )
        .map_err(failure)?;
        if raw
            .profile_name_servers
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || !raw.profile_server_properties.is_empty()
        {
            return Err(failure(
                "Profile-specific DNS requires separate restore validation",
            ));
        }
        let protocol = if target.family == AddressFamily::Ipv6 {
            "Tcpip6"
        } else {
            "Tcpip"
        };
        let path = format!(
            r"SYSTEM\CurrentControlSet\Services\{protocol}\Parameters\Interfaces\{{{}}}",
            target.adapter_id.as_str()
        );
        let registry = registry_string(&path, "NameServer")?;
        let static_servers = addresses(registry.as_deref())?;
        if static_servers != addresses(raw.name_servers.as_deref())? {
            return Err(failure(
                "Native DNS readback and static assignment registry disagree",
            ));
        }
        let mut doh = Vec::new();
        for property in raw.server_properties {
            if property.version != 1 || property.property_type != 1 {
                return Err(failure("Unsupported DNS server property"));
            }
            doh.push(DohProperty {
                server_index: property.server_index,
                template: property.doh_template,
                flags: property
                    .doh_flags
                    .ok_or_else(|| failure("Missing DoH flags"))?,
            });
        }
        doh.sort_by_key(|p| p.server_index);
        let settings = DnsSettings {
            assignment: if static_servers.is_empty() {
                ServerAssignment::Automatic
            } else {
                ServerAssignment::Static {
                    servers: static_servers,
                }
            },
            doh,
        };
        settings.validate(target.family)?;
        Ok(settings)
    }

    fn write(&mut self, target: &Target, settings: &DnsSettings) -> Result<(), CoreError> {
        settings.validate(target.family)?;
        check_policies()?;
        let servers = match &settings.assignment {
            ServerAssignment::Automatic => String::new(),
            ServerAssignment::Static { servers } => servers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        };
        let mut server_string = wide(&servers);
        let templates: Vec<Option<Vec<u16>>> = settings
            .doh
            .iter()
            .map(|p| p.template.as_ref().map(|t| wide(t)))
            .collect();
        let mut doh: Vec<DNS_DOH_SERVER_SETTINGS> = settings
            .doh
            .iter()
            .enumerate()
            .map(|(i, p)| DNS_DOH_SERVER_SETTINGS {
                Template: templates[i]
                    .as_ref()
                    .map_or(ptr::null_mut(), |s| s.as_ptr().cast_mut()),
                Flags: p.flags,
            })
            .collect();
        let mut properties: Vec<DNS_SERVER_PROPERTY> = settings
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
        let mut raw = DNS_INTERFACE_SETTINGS3 {
            Version: DNS_INTERFACE_SETTINGS_VERSION3,
            Flags: DNS_SETTING_NAMESERVER as u64
                | if target.family == AddressFamily::Ipv6 {
                    DNS_SETTING_IPV6 as u64
                } else {
                    0
                },
            NameServer: server_string.as_mut_ptr(),
            ..Default::default()
        };
        if !servers.is_empty() {
            raw.Flags |= DNS_SETTING_DOH as u64;
            raw.cServerProperties = properties.len() as u32;
            raw.ServerProperties = if properties.is_empty() {
                ptr::null_mut()
            } else {
                properties.as_mut_ptr()
            };
        }
        // SAFETY: Only DNS server and DoH fields are selected. All pointees
        // remain owned by the vectors above until this synchronous call returns.
        let code = unsafe {
            SetInterfaceDnsSettings(
                guid(&target.adapter_id),
                (&raw as *const DNS_INTERFACE_SETTINGS3).cast(),
            )
        };
        if code != 0 {
            return Err(failure(format!("SetInterfaceDnsSettings returned {code}")));
        }
        Ok(())
    }

    fn verify_system_doh(&mut self, targets: &[Target]) -> Result<(), CoreError> {
        self.validate_scope(targets)?;
        let report = probe_system_resolver().map_err(failure)?;
        if report.outcome != ProbeOutcome::DohObserved {
            return Err(failure("A fresh system lookup did not verify Quad9 DoH"));
        }
        Ok(())
    }
}
