use crate::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Invalid(#[from] InvalidSettings),
    #[error("DNS backend failed: {0}")]
    Backend(String),
    #[error("DNS settings conflict: {0}")]
    Conflict(String),
    #[error("Recovery journal unavailable: {0}")]
    Journal(String),
    #[error("An active configuration or unfinished recovery already exists")]
    ActiveTransaction,
    #[error("Enable failed: {cause}. Recovery complete: {recovered}")]
    EnableFailed { cause: String, recovered: bool },
}

/// The implementation must reject unsafe/unsupported adapters and policy
/// contexts before returning a restorable snapshot. Write affects one family.
pub trait DnsBackend {
    fn read(&mut self, target: &Target) -> Result<DnsSettings, CoreError>;
    fn write(&mut self, target: &Target, settings: &DnsSettings) -> Result<(), CoreError>;
    fn verify_system_doh(&mut self, targets: &[Target]) -> Result<(), CoreError>;
}

/// A production implementation must serialize access and durably persist a
/// protected atomic replacement before save returns. Failure forbids a write.
pub trait JournalStore {
    fn load(&mut self) -> Result<Option<Journal>, CoreError>;
    fn save(&mut self, journal: &Journal) -> Result<(), CoreError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnableOutcome {
    Enabled,
    AlreadyEnabled,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RecoveryIssue {
    pub target: Target,
    pub reason: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub issues: Vec<RecoveryIssue>,
}

impl RestoreReport {
    pub fn is_complete(&self) -> bool {
        self.issues.is_empty()
    }
}

pub struct Engine<B, S> {
    pub backend: B,
    pub store: S,
}

impl<B: DnsBackend, S: JournalStore> Engine<B, S> {
    fn load(&mut self) -> Result<Option<Journal>, CoreError> {
        let journal = self.store.load()?;
        if let Some(journal) = &journal {
            journal.validate()?;
        }
        Ok(journal)
    }

    pub fn enable(&mut self, targets: &[Target]) -> Result<EnableOutcome, CoreError> {
        if targets.is_empty()
            || targets.len() > MAX_TARGETS
            || targets
                .iter()
                .enumerate()
                .any(|(i, t)| targets[..i].contains(t))
        {
            return Err(InvalidSettings(
                "Select between 1 and 32 distinct adapter families".into(),
            )
            .into());
        }
        if let Some(journal) = self.load()? {
            if journal.phase != TransactionPhase::Restored {
                if journal.phase == TransactionPhase::Committed
                    && journal.entries.len() == targets.len()
                    && journal.entries.iter().all(|e| targets.contains(&e.target))
                {
                    for entry in &journal.entries {
                        if self.backend.read(&entry.target)? != entry.applied {
                            return Err(CoreError::ActiveTransaction);
                        }
                    }
                    self.backend.verify_system_doh(targets)?;
                    for entry in &journal.entries {
                        if self.backend.read(&entry.target)? != entry.applied {
                            return Err(CoreError::Conflict(
                                "Settings changed during verification".into(),
                            ));
                        }
                    }
                    return Ok(EnableOutcome::AlreadyEnabled);
                }
                return Err(CoreError::ActiveTransaction);
            }
        }
        let mut entries = Vec::with_capacity(targets.len());
        for target in targets {
            let before = self.backend.read(target)?;
            before.validate(target.family)?;
            entries.push(JournalEntry {
                target: target.clone(),
                before,
                applied: DnsSettings::quad9(target.family),
                phase: EntryPhase::Planned,
            });
        }
        let mut journal = Journal {
            schema_version: JOURNAL_VERSION,
            phase: TransactionPhase::Applying,
            entries,
        };
        self.store.save(&journal)?;
        let attempt = self.apply(&mut journal, targets);
        if let Err(cause) = attempt {
            // Reload durable state. The in-memory state may be ahead of a failed
            // save, while a failed native write may already have taken effect.
            let recovered = self.restore().map(|r| r.is_complete()).unwrap_or(false);
            return Err(CoreError::EnableFailed {
                cause: cause.to_string(),
                recovered,
            });
        }
        Ok(EnableOutcome::Enabled)
    }

    fn apply(&mut self, journal: &mut Journal, targets: &[Target]) -> Result<(), CoreError> {
        for index in 0..journal.entries.len() {
            let entry = &journal.entries[index];
            if self.backend.read(&entry.target)? != entry.before {
                return Err(CoreError::Backend(
                    "Settings changed before application".into(),
                ));
            }
            journal.entries[index].phase = EntryPhase::WriteIntent;
            self.store.save(journal)?;
            let entry = &journal.entries[index];
            if self.backend.read(&entry.target)? != entry.before {
                return Err(CoreError::Backend(
                    "Settings changed after intent was saved".into(),
                ));
            }
            self.backend.write(&entry.target, &entry.applied)?;
            if self.backend.read(&entry.target)? != entry.applied {
                return Err(CoreError::Backend("DNS readback did not match".into()));
            }
            journal.entries[index].phase = EntryPhase::Applied;
            self.store.save(journal)?;
        }
        self.backend.verify_system_doh(targets)?;
        for entry in &journal.entries {
            if self.backend.read(&entry.target)? != entry.applied {
                return Err(CoreError::Conflict(
                    "Settings changed during verification".into(),
                ));
            }
        }
        journal.phase = TransactionPhase::Committed;
        self.store.save(journal)
    }

    pub fn restore(&mut self) -> Result<RestoreReport, CoreError> {
        let Some(mut journal) = self.load()? else {
            return Ok(RestoreReport::default());
        };
        if journal.phase == TransactionPhase::Restored {
            return Ok(RestoreReport::default());
        }
        journal.phase = TransactionPhase::Restoring;
        self.store.save(&journal)?;
        let mut report = RestoreReport::default();
        for index in (0..journal.entries.len()).rev() {
            let result = self.restore_entry(&mut journal, index);
            if let Err(error) = result {
                // Persistence failure stops all subsequent native writes.
                if matches!(error, CoreError::Journal(_)) {
                    return Err(error);
                }
                report.issues.push(RecoveryIssue {
                    target: journal.entries[index].target.clone(),
                    reason: error.to_string(),
                });
                if matches!(error, CoreError::Conflict(_)) {
                    journal.entries[index].phase = EntryPhase::Conflict;
                }
                self.store.save(&journal)?;
            }
        }
        journal.phase = if report.is_complete() {
            TransactionPhase::Restored
        } else {
            TransactionPhase::RecoveryRequired
        };
        self.store.save(&journal)?;
        Ok(report)
    }

    fn restore_entry(&mut self, journal: &mut Journal, index: usize) -> Result<(), CoreError> {
        let entry = &journal.entries[index];
        let current = self.backend.read(&entry.target)?;
        if current == entry.before {
            journal.entries[index].phase = EntryPhase::Restored;
            return self.store.save(journal);
        }
        if matches!(
            entry.phase,
            EntryPhase::Planned | EntryPhase::Restored | EntryPhase::Conflict
        ) || current != entry.applied
        {
            return Err(CoreError::Conflict(
                "Current settings are not owned by this transaction".into(),
            ));
        }
        journal.entries[index].phase = EntryPhase::RestoreIntent;
        self.store.save(journal)?;
        let entry = &journal.entries[index];
        if self.backend.read(&entry.target)? != entry.applied {
            return Err(CoreError::Conflict(
                "Settings changed immediately before restoration".into(),
            ));
        }
        self.backend.write(&entry.target, &entry.before)?;
        if self.backend.read(&entry.target)? != entry.before {
            return Err(CoreError::Backend(
                "Restored settings could not be verified".into(),
            ));
        }
        journal.entries[index].phase = EntryPhase::Restored;
        self.store.save(journal)
    }

    /// Startup recovery never undoes a committed configuration or re-enables it.
    pub fn recover_interrupted(&mut self) -> Result<RestoreReport, CoreError> {
        match self.load()? {
            Some(journal)
                if !matches!(
                    journal.phase,
                    TransactionPhase::Committed | TransactionPhase::Restored
                ) =>
            {
                self.restore()
            }
            _ => Ok(RestoreReport::default()),
        }
    }
}
