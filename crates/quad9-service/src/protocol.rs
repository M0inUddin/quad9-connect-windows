use quad9_core::{EntryPhase, Target, TransactionPhase};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_RECEIPTS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub id: String,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    GetStatus,
    ListAdapters,
    RunDiagnostics,
    EnableQuad9 {
        expected_revision: u64,
        targets: Vec<Target>,
    },
    RestorePreviousSettings {
        expected_revision: u64,
    },
}

impl Request {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PROTOCOL_VERSION {
            return Err("Unsupported protocol version".into());
        }
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(
                "Request ID must contain 1 to 64 ASCII letters, digits, hyphens, or underscores"
                    .into(),
            );
        }
        if let Command::EnableQuad9 { targets, .. } = &self.command {
            if targets.is_empty()
                || targets.len() > quad9_core::MAX_TARGETS
                || targets
                    .iter()
                    .enumerate()
                    .any(|(i, t)| targets[..i].contains(t))
            {
                return Err("Select distinct adapter families within the supported limit".into());
            }
        }
        Ok(())
    }

    pub fn revision(&self) -> Option<u64> {
        match self.command {
            Command::EnableQuad9 {
                expected_revision, ..
            }
            | Command::RestorePreviousSettings { expected_revision } => Some(expected_revision),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub id: String,
    pub result: ResultBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultBody {
    Status {
        revision: u64,
        recovery_blocked: bool,
        journal: Option<JournalSummary>,
        observed_matches: bool,
    },
    Adapters {
        adapters: Vec<AdapterSummary>,
    },
    Diagnostics {
        report: serde_json::Value,
    },
    Enabled {
        revision: u64,
        already_enabled: bool,
    },
    Restored {
        revision: u64,
        complete: bool,
        issues: Vec<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalSummary {
    pub phase: TransactionPhase,
    pub targets: Vec<TargetSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSummary {
    pub target: Target,
    pub phase: EntryPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterSummary {
    pub id: quad9_core::AdapterId,
    pub name: String,
    pub candidate: bool,
    pub exclusion_reason: Option<String>,
    pub ipv4_enabled: bool,
    pub ipv6_enabled: bool,
}

impl Response {
    pub fn error(id: &str, code: &str, message: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id: id.to_owned(),
            result: ResultBody::Error {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub actor: String,
    pub request: Request,
    pub response: Option<Response>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceState {
    pub revision: u64,
    pub journal_expected: bool,
    pub receipts: Vec<Receipt>,
}

impl ServiceState {
    pub fn validate(&self) -> Result<(), String> {
        if self.receipts.len() > MAX_RECEIPTS {
            return Err("Too many saved request receipts".into());
        }
        for (i, receipt) in self.receipts.iter().enumerate() {
            receipt.request.validate()?;
            if receipt.actor.is_empty()
                || receipt.actor.len() > 184
                || receipt.request.revision().is_none()
            {
                return Err("Invalid mutation receipt".into());
            }
            if self.receipts[..i]
                .iter()
                .any(|r| r.actor == receipt.actor && r.request.id == receipt.request.id)
            {
                return Err("Duplicate mutation receipt".into());
            }
            if !receipt
                .request
                .revision()
                .is_some_and(|r| r < self.revision)
            {
                return Err("Mutation receipt is ahead of the service revision".into());
            }
            if let Some(response) = &receipt.response {
                if response.id != receipt.request.id || response.version != PROTOCOL_VERSION {
                    return Err("Mismatched saved response".into());
                }
            }
        }
        Ok(())
    }
}
