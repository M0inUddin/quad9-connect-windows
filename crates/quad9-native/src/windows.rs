use crate::*;
use std::{ffi::c_void, mem, ptr};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA},
        NetworkManagement::{Dns::*, IpHelper::*, Ndis::IfOperStatusUp},
        Networking::WinSock::AF_UNSPEC,
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
    },
};

fn win_error(operation: &'static str, code: u32) -> NativeError {
    NativeError::Windows { operation, code }
}

// SAFETY: Callers pass strings owned by Windows for the duration of the API
// allocation. The cap bounds scanning of an unexpectedly unterminated string.
unsafe fn wide_string(value: *const u16) -> Result<Option<String>, NativeError> {
    if value.is_null() {
        return Ok(None);
    }
    for len in 0..32_768 {
        // SAFETY: Pointer comes from a live Windows-owned UTF-16 string.
        if unsafe { *value.add(len) } == 0 {
            // SAFETY: The preceding scan found the initialized string length.
            return String::from_utf16(unsafe { std::slice::from_raw_parts(value, len) })
                .map(Some)
                .map_err(|_| NativeError::InvalidData("Windows returned invalid UTF-16".into()));
        }
    }
    Err(NativeError::InvalidData(
        "Windows returned an overlong string".into(),
    ))
}

fn format_guid(guid: GUID) -> Result<AdapterId, NativeError> {
    AdapterId::parse(&format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    ))
    .map_err(|e| NativeError::InvalidData(e.to_string()))
}

pub fn inspect_adapters() -> Result<Vec<AdapterInspection>, NativeError> {
    // u128 storage guarantees the alignment required by the Windows structures.
    let mut size = 16 * 1024u32;
    for _ in 0..4 {
        if size > 1024 * 1024 {
            return Err(NativeError::InvalidData(
                "Adapter buffer exceeded 1 MiB".into(),
            ));
        }
        let mut buffer = vec![0u128; (size as usize).div_ceil(16)];
        size = (buffer.len() * 16) as u32;
        let head = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: head is aligned and writable for size bytes; Windows reports
        // buffer growth rather than writing beyond this allocation.
        let code = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_INCLUDE_ALL_INTERFACES,
                ptr::null(),
                head,
                &mut size,
            )
        };
        if code == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if code == ERROR_NO_DATA {
            return Ok(vec![]);
        }
        if code != 0 {
            return Err(win_error("GetAdaptersAddresses", code));
        }
        let mut current = head;
        let mut result = Vec::new();
        while !current.is_null() {
            if result.len() >= 512 {
                return Err(NativeError::InvalidData("Too many adapter records".into()));
            }
            // SAFETY: current belongs to the linked list in the live buffer.
            let adapter = unsafe { &*current };
            let mut row = MIB_IF_ROW2 {
                InterfaceLuid: adapter.Luid,
                ..Default::default()
            };
            // SAFETY: row is initialized and writable for the full structure.
            let row_code = unsafe { GetIfEntry2(&mut row) };
            let mut guid = GUID::from_u128(0);
            // SAFETY: Both pointers refer to valid, correctly sized objects.
            let guid_code = unsafe { ConvertInterfaceLuidToGuid(&adapter.Luid, &mut guid) };
            if guid_code != 0 {
                return Err(win_error("ConvertInterfaceLuidToGuid", guid_code));
            }
            let id = format_guid(guid)?;
            let kind = match adapter.IfType {
                IF_TYPE_ETHERNET_CSMACD => AdapterKind::Ethernet,
                IF_TYPE_IEEE80211 => AdapterKind::Wifi,
                _ => AdapterKind::Other,
            };
            let hardware =
                (row_code == 0).then_some(row.InterfaceAndOperStatusFlags._bitfield & 1 != 0);
            let connected = adapter.OperStatus == IfOperStatusUp;
            let reason = exclusion_reason(kind, connected, hardware);
            // SAFETY: Flags is the bitfield representation of this initialized union.
            let flags = unsafe { adapter.Anonymous2.Flags };
            // SAFETY: FriendlyName points into the live adapter buffer.
            let name =
                unsafe { wide_string(adapter.FriendlyName) }?.unwrap_or_else(|| id.as_str().into());
            let (dns_readback, dns_readback_error) = match read_dns_settings(guid) {
                Ok(settings) => (Some(settings), None),
                Err(error) => (None, Some(error.to_string())),
            };
            result.push(AdapterInspection {
                id,
                name,
                kind,
                connected,
                hardware_interface: hardware,
                ipv4_enabled: flags & IP_ADAPTER_IPV4_ENABLED != 0,
                ipv6_enabled: flags & IP_ADAPTER_IPV6_ENABLED != 0,
                candidate: reason.is_none(),
                exclusion_reason: reason.map(String::from),
                dns_readback,
                dns_readback_error,
            });
            current = adapter.Next;
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        return Ok(result);
    }
    Err(NativeError::InvalidData(
        "Adapter list kept changing; retry inspection".into(),
    ))
}

type GetDns = unsafe extern "system" fn(GUID, *mut DNS_INTERFACE_SETTINGS) -> u32;
type FreeDns = unsafe extern "system" fn(*mut DNS_INTERFACE_SETTINGS);

struct SettingsAllocation {
    value: DNS_INTERFACE_SETTINGS3,
    free: FreeDns,
}

impl Drop for SettingsAllocation {
    fn drop(&mut self) {
        // SAFETY: This allocation came from the paired GetInterfaceDnsSettings
        // call and is freed once after all returned values have been copied.
        unsafe {
            (self.free)((&mut self.value as *mut DNS_INTERFACE_SETTINGS3).cast());
        }
    }
}

fn read_dns_settings(guid: GUID) -> Result<RawDnsReadback, NativeError> {
    let module_name: Vec<u16> = "iphlpapi.dll\0".encode_utf16().collect();
    // SAFETY: The module is already loaded by the statically imported adapter
    // APIs. GetModuleHandle does not create an owned reference to release.
    let module = unsafe { GetModuleHandleW(module_name.as_ptr()) };
    if module.is_null() {
        return Err(NativeError::InvalidData(
            "IP Helper module unavailable".into(),
        ));
    }
    // SAFETY: Module and export names are fixed and NUL-terminated.
    let get = unsafe { GetProcAddress(module, c"GetInterfaceDnsSettings".as_ptr().cast()) };
    // SAFETY: Same module and fixed export name as above.
    let free = unsafe { GetProcAddress(module, c"FreeInterfaceDnsSettings".as_ptr().cast()) };
    let (Some(get), Some(free)) = (get, free) else {
        return Err(NativeError::InvalidData(
            "Per-interface DNS settings API unavailable".into(),
        ));
    };
    // SAFETY: Signatures exactly match the documented Windows exports.
    let get: GetDns = unsafe { mem::transmute(get) };
    // SAFETY: Signature exactly matches FreeInterfaceDnsSettings.
    let free: FreeDns = unsafe { mem::transmute(free) };
    let mut settings = DNS_INTERFACE_SETTINGS3 {
        Version: DNS_INTERFACE_SETTINGS_VERSION3,
        ..Default::default()
    };
    // SAFETY: Version 3 identifies the allocated structure size. Flags stays
    // zero as required by the getter, and all fields start initialized.
    let code = unsafe { get(guid, (&mut settings as *mut DNS_INTERFACE_SETTINGS3).cast()) };
    if code != 0 {
        return Err(win_error("GetInterfaceDnsSettings", code));
    }
    let allocation = SettingsAllocation {
        value: settings,
        free,
    };
    let value = &allocation.value;
    // SAFETY: All pointers remain owned by allocation throughout these copies.
    unsafe {
        Ok(RawDnsReadback {
            version: value.Version,
            flags: value.Flags,
            name_servers: wide_string(value.NameServer)?,
            profile_name_servers: wide_string(value.ProfileNameServer)?,
            server_properties: copy_properties(value.ServerProperties, value.cServerProperties)?,
            profile_server_properties: copy_properties(
                value.ProfileServerProperties,
                value.cProfileServerProperties,
            )?,
        })
    }
}

unsafe fn copy_properties(
    properties: *const DNS_SERVER_PROPERTY,
    count: u32,
) -> Result<Vec<RawServerProperty>, NativeError> {
    if count > 128 || (count != 0 && properties.is_null()) {
        return Err(NativeError::InvalidData(
            "Invalid DNS property array".into(),
        ));
    }
    let mut result = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        // SAFETY: The API owns count initialized properties in this allocation.
        let property = unsafe { &*properties.add(index) };
        let mut record = RawServerProperty {
            version: property.Version,
            server_index: property.ServerIndex,
            property_type: property.Type,
            doh_flags: None,
            doh_template: None,
        };
        if property.Version == DNS_SERVER_PROPERTY_VERSION1 && property.Type == DnsServerDohProperty
        {
            // SAFETY: The version and type identify the active union member.
            let doh = unsafe { property.Property.DohSettings };
            if doh.is_null() {
                return Err(NativeError::InvalidData("Null DoH property".into()));
            }
            // SAFETY: The DoH object and template are live until settings is freed.
            record.doh_flags = Some(unsafe { (*doh).Flags });
            // SAFETY: The same live DoH allocation owns this UTF-16 string.
            record.doh_template = unsafe { wide_string((*doh).Template) }?;
        }
        result.push(record);
    }
    Ok(result)
}

struct QueryRecords(*mut DNS_RECORDW);

impl Drop for QueryRecords {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: DnsFree with DnsFreeRecordList is the matching release for
            // the DnsQuery_W record list, and this guard owns it exactly once.
            unsafe {
                DnsFree(self.0.cast::<c_void>(), DnsFreeRecordList);
            }
        }
    }
}

pub fn query_protocol() -> Result<Vec<String>, NativeError> {
    let name: Vec<u16> = format!("{PROBE_NAME}\0").encode_utf16().collect();
    let mut records = ptr::null_mut();
    // SAFETY: Inputs are fixed and valid. DnsQuery_W returns wide-character
    // records despite the windows-sys declaration using DNS_RECORDA pointers.
    let code = unsafe {
        DnsQuery_W(
            name.as_ptr(),
            DNS_TYPE_TEXT,
            DNS_QUERY_BYPASS_CACHE | DNS_QUERY_NO_HOSTS_FILE | DNS_QUERY_TREAT_AS_FQDN,
            ptr::null_mut(),
            &mut records,
            ptr::null_mut(),
        )
    };
    let allocation = QueryRecords(records.cast());
    if code != 0 {
        return Err(win_error("DnsQuery_W", code));
    }
    let mut values = Vec::new();
    let mut current = allocation.0;
    let mut count = 0;
    while !current.is_null() {
        count += 1;
        if count > 128 {
            return Err(NativeError::InvalidData(
                "Too many DNS response records".into(),
            ));
        }
        // SAFETY: current points into the query's live record list.
        let record = unsafe { &*current };
        if record.wType == DNS_TYPE_TEXT {
            // SAFETY: wType identifies the active TXT union member.
            let txt = unsafe { &record.Data.TXT };
            if txt.dwStringCount > 128 {
                return Err(NativeError::InvalidData("Too many DNS TXT strings".into()));
            }
            let mut value = String::new();
            // TXT fragments in one record concatenate; separate records do not.
            for index in 0..txt.dwStringCount as usize {
                // SAFETY: Windows allocates the flexible pointer array with the
                // declared string count; the Rust member represents its first slot.
                let part = unsafe { *txt.pStringArray.as_ptr().add(index) };
                // SAFETY: Each fragment is a live Windows-owned UTF-16 string.
                value.push_str(&unsafe { wide_string(part) }?.unwrap_or_default());
            }
            values.push(value);
        }
        current = record.pNext;
    }
    Ok(values)
}
