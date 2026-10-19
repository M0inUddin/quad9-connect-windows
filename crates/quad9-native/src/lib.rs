//! Windows DNS integration and privileged service primitives.

use quad9_core::AdapterId;
use serde::Serialize;
use std::time::Duration;
#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[cfg(any(windows, test))]
mod dns_scope;

#[cfg(windows)]
mod dns_backend;
#[cfg(windows)]
pub mod network;
#[cfg(windows)]
pub mod service;
mod upstream;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use dns_backend::WindowsDnsBackend;
pub use upstream::{probe_upstream, Outcome as UpstreamOutcome, UpstreamReport};

#[cfg(windows)]
pub fn replace_file(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), NativeError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: Both paths are initialized NUL-terminated UTF-16 and remain live.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(NativeError::Windows {
            operation: "MoveFileExW",
            code: std::io::Error::last_os_error().raw_os_error().unwrap_or(1) as u32,
        });
    }
    Ok(())
}

pub const PROBE_NAME: &str = "proto.on.quad9.net.";
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Error)]
pub enum NativeError {
    #[error("This command requires Windows")]
    UnsupportedPlatform,
    #[error("{operation} failed with Windows error {code}")]
    Windows { operation: &'static str, code: u32 },
    #[error("{0}")]
    InvalidData(String),
    #[error("System DNS check exceeded its deadline")]
    Deadline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Ethernet,
    Wifi,
    Other,
}

#[derive(Debug, Serialize)]
pub struct AdapterInspection {
    pub id: AdapterId,
    pub name: String,
    pub kind: AdapterKind,
    pub connected: bool,
    pub hardware_interface: Option<bool>,
    pub ipv4_enabled: bool,
    pub ipv6_enabled: bool,
    pub ipv6_usable: bool,
    pub has_ipv4_dns: bool,
    pub has_ipv6_dns: bool,
    pub candidate: bool,
    pub exclusion_reason: Option<String>,
    pub dns_readback: Option<RawDnsReadback>,
    pub dns_readback_error: Option<String>,
}

/// An API observation, deliberately not a restorable DNS configuration.
/// Family/assignment-mode semantics still need native write/restore VM proof.
#[derive(Debug, Serialize)]
pub struct RawDnsReadback {
    pub version: u32,
    pub flags: u64,
    pub name_servers: Option<String>,
    pub profile_name_servers: Option<String>,
    pub server_properties: Vec<RawServerProperty>,
    pub profile_server_properties: Vec<RawServerProperty>,
}

#[derive(Debug, Serialize)]
pub struct RawServerProperty {
    pub version: u32,
    pub server_index: u32,
    pub property_type: i32,
    pub doh_flags: Option<u64>,
    pub doh_template: Option<String>,
}

pub fn exclusion_reason(
    kind: AdapterKind,
    connected: bool,
    hardware: Option<bool>,
) -> Option<&'static str> {
    if !matches!(kind, AdapterKind::Ethernet | AdapterKind::Wifi) {
        Some("Only physical Wi-Fi and Ethernet are initial candidates")
    } else if hardware != Some(true) {
        Some("Windows did not identify this as a hardware interface")
    } else if !connected {
        Some("Adapter is disconnected")
    } else {
        None
    }
}

pub fn inspect_adapters() -> Result<Vec<AdapterInspection>, NativeError> {
    #[cfg(windows)]
    {
        windows::inspect_adapters()
    }
    #[cfg(not(windows))]
    {
        Err(NativeError::UnsupportedPlatform)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedTransport {
    Doh,
    Dot,
    PlaintextUdp,
    PlaintextTcp,
    DnscryptUdp,
    DnscryptTcp,
}

pub fn parse_transport(text: &str) -> Option<ObservedTransport> {
    match text.trim().trim_matches('"').trim_end_matches('.') {
        "doh" => Some(ObservedTransport::Doh),
        "dot" => Some(ObservedTransport::Dot),
        "do53-udp" => Some(ObservedTransport::PlaintextUdp),
        "do53-tcp" => Some(ObservedTransport::PlaintextTcp),
        "dnscrypt-udp" => Some(ObservedTransport::DnscryptUdp),
        "dnscrypt-tcp" => Some(ObservedTransport::DnscryptTcp),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
pub struct ProbeReport {
    pub query: &'static str,
    pub checked_at_unix_seconds: u64,
    pub elapsed_ms: u64,
    pub outcome: ProbeOutcome,
    pub transport: Option<ObservedTransport>,
    pub error: Option<String>,
    pub scope: &'static str,
}

/// Independent checks. Direct HTTPS never changes the system DNS observation.
#[derive(Debug, Serialize)]
pub struct DiagnosticsReport {
    #[serde(flatten)]
    pub system: ProbeReport,
    pub direct_upstream: UpstreamReport,
}

pub fn probe_diagnostics() -> Result<DiagnosticsReport, NativeError> {
    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    std::thread::scope(|scope| {
        let direct = scope.spawn(|| upstream::probe_until(deadline));
        let system = probe_system_resolver_with_timeout(
            deadline.saturating_duration_since(std::time::Instant::now()),
        );
        let direct_upstream = direct
            .join()
            .map_err(|_| NativeError::InvalidData("Direct check worker failed".into()))??;
        Ok(DiagnosticsReport {
            system: system?,
            direct_upstream,
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    DohObserved,
    OtherTransportObserved,
    UnknownResponse,
    Failed,
    TimedOut,
}

pub fn classify_responses(values: &[String]) -> (ProbeOutcome, Option<ObservedTransport>) {
    let transports: Vec<_> = values.iter().filter_map(|v| parse_transport(v)).collect();
    // Refuse to collapse conflicting records into an optimistic success.
    match transports.first().copied() {
        Some(first)
            if transports.iter().all(|t| *t == first) && transports.len() == values.len() =>
        {
            (
                if first == ObservedTransport::Doh {
                    ProbeOutcome::DohObserved
                } else {
                    ProbeOutcome::OtherTransportObserved
                },
                Some(first),
            )
        }
        _ => (ProbeOutcome::UnknownResponse, None),
    }
}

/// A cancellable system-resolver diagnostic with at most one outstanding query.
pub fn probe_system_resolver() -> Result<ProbeReport, NativeError> {
    probe_system_resolver_with_timeout(PROBE_TIMEOUT)
}

fn probe_system_resolver_with_timeout(timeout: Duration) -> Result<ProbeReport, NativeError> {
    #[cfg(not(windows))]
    {
        let _ = timeout;
        Err(NativeError::UnsupportedPlatform)
    }
    #[cfg(windows)]
    {
        use std::time::Instant;
        let checked_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let start = Instant::now();
        let (outcome, transport, error) = match windows::query_protocol(timeout) {
            Ok(values) => {
                let (outcome, transport) = classify_responses(&values);
                (outcome, transport, None)
            }
            Err(NativeError::Deadline) => (
                ProbeOutcome::TimedOut,
                None,
                Some("System DNS check exceeded 15 seconds".into()),
            ),
            Err(error) => (ProbeOutcome::Failed, None, Some(error.to_string())),
        };
        Ok(ProbeReport {
            query: PROBE_NAME,
            checked_at_unix_seconds: checked_at,
            elapsed_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            outcome,
            transport,
            error,
            scope: "This system-resolver lookup only; not proof for every adapter or application",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_and_unknown_hardware_are_excluded() {
        assert!(exclusion_reason(AdapterKind::Ethernet, true, Some(false)).is_some());
        assert!(exclusion_reason(AdapterKind::Wifi, true, None).is_some());
        assert!(exclusion_reason(AdapterKind::Other, true, Some(true)).is_some());
        assert!(exclusion_reason(AdapterKind::Wifi, false, Some(true)).is_some());
        assert!(exclusion_reason(AdapterKind::Wifi, true, Some(true)).is_none());
    }

    #[test]
    fn distinguishes_all_documented_protocol_responses() {
        for (value, expected) in [
            ("doh", ObservedTransport::Doh),
            ("dot", ObservedTransport::Dot),
            ("do53-udp", ObservedTransport::PlaintextUdp),
            ("do53-tcp", ObservedTransport::PlaintextTcp),
            ("dnscrypt-udp", ObservedTransport::DnscryptUdp),
            ("dnscrypt-tcp", ObservedTransport::DnscryptTcp),
        ] {
            assert_eq!(parse_transport(value), Some(expected));
        }
        assert_eq!(parse_transport("\"doh.\""), Some(ObservedTransport::Doh));
        assert_eq!(parse_transport("not-doh"), None);
    }

    #[test]
    fn empty_unknown_or_conflicting_answers_never_report_doh() {
        for values in [
            vec![],
            vec!["doh".into(), "unexpected".into()],
            vec!["doh".into(), "do53-udp".into()],
        ] {
            assert_eq!(
                classify_responses(&values),
                (ProbeOutcome::UnknownResponse, None)
            );
        }
        assert_eq!(
            classify_responses(&["doh".into()]),
            (ProbeOutcome::DohObserved, Some(ObservedTransport::Doh))
        );
    }
}
