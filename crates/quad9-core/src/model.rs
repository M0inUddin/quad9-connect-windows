use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use thiserror::Error;

pub const JOURNAL_VERSION: u32 = 1;
pub const QUAD9_DOH_TEMPLATE: &str = "https://dns.quad9.net/dns-query";
pub const MAX_TARGETS: usize = 32;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct InvalidSettings(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AdapterId(String);

impl AdapterId {
    pub fn parse(value: &str) -> Result<Self, InvalidSettings> {
        let value = value
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(value);
        if value.len() != 36
            || !value.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return Err(InvalidSettings(
                "Adapter ID must be a Windows adapter GUID".into(),
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AdapterId {
    type Error = InvalidSettings;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AdapterId> for String {
    fn from(value: AdapterId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub adapter_id: AdapterId,
    pub family: AddressFamily,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerAssignment {
    Automatic,
    Static { servers: Vec<IpAddr> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DohProperty {
    pub server_index: u32,
    pub template: Option<String>,
    pub flags: u64,
}

/// Only settings owned by the app. Suffixes and registration are never written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsSettings {
    pub assignment: ServerAssignment,
    pub doh: Vec<DohProperty>,
}

impl DnsSettings {
    pub fn quad9(family: AddressFamily) -> Self {
        let addresses = match family {
            AddressFamily::Ipv4 => ["9.9.9.9", "149.112.112.112"],
            AddressFamily::Ipv6 => ["2620:fe::fe", "2620:fe::9"],
        };
        Self {
            assignment: ServerAssignment::Static {
                servers: addresses
                    .iter()
                    .map(|s| s.parse().expect("constant IP address"))
                    .collect(),
            },
            doh: (0..2)
                .map(|server_index| DohProperty {
                    server_index,
                    template: Some(QUAD9_DOH_TEMPLATE.into()),
                    // ENABLE only. FALLBACK_TO_UDP is deliberately absent.
                    flags: 0x0002,
                })
                .collect(),
        }
    }

    pub fn validate(&self, family: AddressFamily) -> Result<(), InvalidSettings> {
        let servers = match &self.assignment {
            ServerAssignment::Automatic => {
                if !self.doh.is_empty() {
                    return Err(InvalidSettings("Automatic assignment with adapter DoH properties needs a proven restore strategy".into()));
                }
                return Ok(());
            }
            ServerAssignment::Static { servers } => servers,
        };
        if servers.is_empty() || servers.len() > MAX_TARGETS {
            return Err(InvalidSettings(
                "Static DNS requires between 1 and 32 servers".into(),
            ));
        }
        for (i, server) in servers.iter().enumerate() {
            if server.is_unspecified()
                || server.is_multicast()
                || server.is_ipv4() != (family == AddressFamily::Ipv4)
                || servers[..i].contains(server)
            {
                return Err(InvalidSettings(
                    "Invalid, duplicate, or wrong-family DNS server".into(),
                ));
            }
        }
        for (i, property) in self.doh.iter().enumerate() {
            if property.server_index as usize >= servers.len()
                || self.doh[..i]
                    .iter()
                    .any(|p| p.server_index == property.server_index)
            {
                return Err(InvalidSettings(
                    "DoH property index does not identify one unique server".into(),
                ));
            }
            if property.flags & !0x000f != 0 || property.flags & 3 == 3 {
                return Err(InvalidSettings("Unsupported DoH flags".into()));
            }
            let explicit = property.flags & 2 != 0;
            let auto = property.flags & 1 != 0;
            if (property.flags & 0x000c != 0 && !explicit && !auto)
                || (auto && property.template.is_some())
                || (explicit
                    && !property
                        .template
                        .as_ref()
                        .is_some_and(|s| s.starts_with("https://") && !s.contains('\0')))
                || (!explicit && !auto && property.template.is_some())
            {
                return Err(InvalidSettings(
                    "Inconsistent DoH template and flags".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionPhase {
    Applying,
    Committed,
    Restoring,
    RecoveryRequired,
    Restored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryPhase {
    Planned,
    WriteIntent,
    Applied,
    RestoreIntent,
    Restored,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEntry {
    pub target: Target,
    pub before: DnsSettings,
    pub applied: DnsSettings,
    pub phase: EntryPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub schema_version: u32,
    pub phase: TransactionPhase,
    pub entries: Vec<JournalEntry>,
}

impl Journal {
    pub fn validate(&self) -> Result<(), InvalidSettings> {
        if self.schema_version != JOURNAL_VERSION
            || self.entries.is_empty()
            || self.entries.len() > MAX_TARGETS
        {
            return Err(InvalidSettings(
                "Unsupported or invalid recovery journal".into(),
            ));
        }
        for (i, entry) in self.entries.iter().enumerate() {
            if self.entries[..i].iter().any(|e| e.target == entry.target) {
                return Err(InvalidSettings(
                    "Duplicate target in recovery journal".into(),
                ));
            }
            entry.before.validate(entry.target.family)?;
            if entry.applied != DnsSettings::quad9(entry.target.family) {
                return Err(InvalidSettings(
                    "Journal contains a profile the app does not manage".into(),
                ));
            }
            if (self.phase == TransactionPhase::Committed && entry.phase != EntryPhase::Applied)
                || (self.phase == TransactionPhase::Restored && entry.phase != EntryPhase::Restored)
            {
                return Err(InvalidSettings(
                    "Journal phase contradicts its entries".into(),
                ));
            }
        }
        Ok(())
    }
}
