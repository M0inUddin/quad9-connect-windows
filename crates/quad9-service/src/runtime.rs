use crate::protocol::*;
use quad9_core::*;

pub trait ServiceBackend: DnsBackend {
    fn adapters(&mut self) -> Result<Vec<AdapterSummary>, CoreError>;
    fn diagnostics(&mut self) -> Result<serde_json::Value, CoreError>;
    fn validate_scope(&mut self, targets: &[Target]) -> Result<(), CoreError>;
}

pub trait StateStore {
    fn load_state(&mut self) -> Result<ServiceState, CoreError>;
    fn save_state(&mut self, state: &ServiceState) -> Result<(), CoreError>;
}

pub struct Controller<B, S> {
    engine: Engine<B, S>,
    state: ServiceState,
    recovery_blocked: bool,
}

impl<B: ServiceBackend, S: JournalStore + StateStore> Controller<B, S> {
    pub fn start(backend: B, mut store: S) -> Result<Self, CoreError> {
        let mut state = store.load_state()?;
        state.validate().map_err(CoreError::Journal)?;
        let journal = store.load()?;
        if let Some(journal) = &journal {
            journal.validate()?;
        }
        let pending = state.receipts.iter().any(|r| r.response.is_none());
        let missing = (journal.is_none() && (state.journal_expected || pending))
            || (journal.is_some() && state.revision == 0);
        let mut engine = Engine { backend, store };
        let mut recovery_blocked = missing;
        if !missing {
            match engine.recover_interrupted() {
                Ok(report) => recovery_blocked = !report.is_complete(),
                Err(_) => recovery_blocked = true,
            }
        }
        for receipt in &mut state.receipts {
            if receipt.response.is_none() {
                receipt.response = Some(Response::error(&receipt.request.id, "interrupted", "The service restarted during this request. Inspect status before issuing a new operation."));
            }
        }
        if pending {
            engine.store.save_state(&state)?;
        }
        Ok(Self {
            engine,
            state,
            recovery_blocked,
        })
    }

    pub fn dispatch(&mut self, actor: &str, request: Request) -> Response {
        if let Err(error) = request.validate() {
            return Response::error(&request.id, "invalid_request", error);
        }
        if actor.is_empty() || actor.len() > 184 {
            return Response::error(
                &request.id,
                "unauthorized",
                "Caller identity is unavailable",
            );
        }
        if request.revision().is_none() {
            return self.read_only(request);
        }
        if let Some(receipt) = self
            .state
            .receipts
            .iter()
            .find(|r| r.actor == actor && r.request.id == request.id)
        {
            if receipt.request != request {
                return Response::error(
                    &request.id,
                    "id_reused",
                    "This request ID already identifies different parameters",
                );
            }
            return receipt.response.clone().unwrap_or_else(|| {
                Response::error(&request.id, "pending", "The original request is unresolved")
            });
        }
        if self.state.journal_expected {
            match self.engine.store.load() {
                Ok(Some(_)) => {}
                _ => self.recovery_blocked = true,
            }
        }
        if self.recovery_blocked {
            // Explicit restore may retry absent adapters, but a missing or
            // unreadable journal cannot be treated as an empty configuration.
            let can_retry_restore =
                matches!(request.command, Command::RestorePreviousSettings { .. })
                    && self
                        .engine
                        .store
                        .load()
                        .ok()
                        .flatten()
                        .is_some_and(|j| j.phase == TransactionPhase::RecoveryRequired);
            if !can_retry_restore {
                return Response::error(
                    &request.id,
                    "recovery_required",
                    "Recovery must be resolved before a new mutation",
                );
            }
        }
        if request.revision() != Some(self.state.revision) {
            return Response::error(
                &request.id,
                "stale_revision",
                "Read current status before issuing a new mutation",
            );
        }
        if let Command::EnableQuad9 { targets, .. } = &request.command {
            if let Err(error) = self.engine.backend.validate_scope(targets) {
                return Response::error(&request.id, "unsupported_scope", error.to_string());
            }
        }
        let Some(revision) = self.state.revision.checked_add(1) else {
            return Response::error(
                &request.id,
                "revision_exhausted",
                "Service revision exhausted",
            );
        };
        let mut next = self.state.clone();
        next.revision = revision;
        if next.receipts.len() == MAX_RECEIPTS {
            next.receipts.remove(0);
        }
        next.receipts.push(Receipt {
            actor: actor.into(),
            request: request.clone(),
            response: None,
        });
        if let Err(error) = self.engine.store.save_state(&next) {
            return Response::error(&request.id, "storage_unavailable", error.to_string());
        }
        self.state = next;
        let result = match &request.command {
            Command::EnableQuad9 { targets, .. } => {
                self.engine
                    .enable(targets)
                    .map(|outcome| ResultBody::Enabled {
                        revision,
                        already_enabled: outcome == EnableOutcome::AlreadyEnabled,
                    })
            }
            Command::RestorePreviousSettings { .. } => {
                self.engine.restore().map(|report| ResultBody::Restored {
                    revision,
                    complete: report.is_complete(),
                    issues: report.issues.into_iter().map(|i| i.reason).collect(),
                })
            }
            _ => unreachable!("Only mutation commands have a revision"),
        };
        let response = Response {
            version: PROTOCOL_VERSION,
            id: request.id.clone(),
            result: result.unwrap_or_else(|error| ResultBody::Error {
                code: "operation_failed".into(),
                message: error.to_string(),
            }),
        };
        match self.engine.store.load() {
            Ok(Some(journal)) => {
                self.state.journal_expected = true;
                self.recovery_blocked = !matches!(
                    journal.phase,
                    TransactionPhase::Committed | TransactionPhase::Restored
                );
            }
            Ok(None) => {}
            Err(_) => self.recovery_blocked = true,
        }
        self.state
            .receipts
            .last_mut()
            .expect("Reserved receipt")
            .response = Some(response.clone());
        if let Err(error) = self.engine.store.save_state(&self.state) {
            self.recovery_blocked = true;
            self.state
                .receipts
                .last_mut()
                .expect("Reserved receipt")
                .response = None;
            return Response::error(
                &request.id,
                "receipt_unavailable",
                format!("Result persistence failed; inspect status before retrying: {error}"),
            );
        }
        response
    }

    fn read_only(&mut self, request: Request) -> Response {
        let result = match request.command {
            Command::GetStatus => self.engine.store.load().and_then(|journal| {
                if self.state.journal_expected && journal.is_none() {
                    self.recovery_blocked = true;
                }
                if let Some(journal) = &journal {
                    journal.validate()?;
                }
                let mut observed_matches = journal
                    .as_ref()
                    .is_some_and(|j| j.phase == TransactionPhase::Committed);
                if let Some(journal) = &journal {
                    for entry in &journal.entries {
                        if self.engine.backend.read(&entry.target).ok().as_ref()
                            != Some(&entry.applied)
                        {
                            observed_matches = false;
                        }
                    }
                }
                let journal = journal.map(|j| JournalSummary {
                    phase: j.phase,
                    targets: j
                        .entries
                        .into_iter()
                        .map(|e| TargetSummary {
                            target: e.target,
                            phase: e.phase,
                        })
                        .collect(),
                });
                Ok(ResultBody::Status {
                    revision: self.state.revision,
                    recovery_blocked: self.recovery_blocked,
                    journal,
                    observed_matches,
                })
            }),
            Command::ListAdapters => self
                .engine
                .backend
                .adapters()
                .map(|adapters| ResultBody::Adapters { adapters }),
            Command::RunDiagnostics => self
                .engine
                .backend
                .diagnostics()
                .map(|report| ResultBody::Diagnostics { report }),
            _ => unreachable!("Mutations are handled separately"),
        };
        Response {
            version: PROTOCOL_VERSION,
            id: request.id,
            result: result.unwrap_or_else(|error| ResultBody::Error {
                code: "inspection_failed".into(),
                message: error.to_string(),
            }),
        }
    }
}
