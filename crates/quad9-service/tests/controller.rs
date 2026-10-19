use quad9_core::*;
use quad9_service::{protocol::*, runtime::*};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Default)]
struct State(Rc<RefCell<Saved>>);
#[derive(Default)]
struct Saved {
    journal: Option<Journal>,
    requests: ServiceState,
    fail_reservation: bool,
    fail_completion: bool,
}
impl JournalStore for State {
    fn load(&mut self) -> Result<Option<Journal>, CoreError> {
        Ok(self.0.borrow().journal.clone())
    }
    fn save(&mut self, journal: &Journal) -> Result<(), CoreError> {
        journal.validate()?;
        self.0.borrow_mut().journal = Some(journal.clone());
        Ok(())
    }
}
impl StateStore for State {
    fn load_state(&mut self) -> Result<ServiceState, CoreError> {
        Ok(self.0.borrow().requests.clone())
    }
    fn save_state(&mut self, state: &ServiceState) -> Result<(), CoreError> {
        state.validate().map_err(CoreError::Journal)?;
        let mut saved = self.0.borrow_mut();
        let pending = state.receipts.last().is_some_and(|r| r.response.is_none());
        if (pending && saved.fail_reservation) || (!pending && saved.fail_completion) {
            return Err(CoreError::Journal(
                "injected request storage failure".into(),
            ));
        }
        // Round trip to exercise durable state rather than sharing references.
        saved.requests = serde_json::from_slice(&serde_json::to_vec(state).unwrap()).unwrap();
        Ok(())
    }
}

#[derive(Clone)]
struct Backend(Rc<RefCell<Machine>>);
struct Machine {
    settings: DnsSettings,
    writes: usize,
    missing: bool,
}
impl Default for Backend {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(Machine {
            settings: original(),
            writes: 0,
            missing: false,
        })))
    }
}
impl DnsBackend for Backend {
    fn read(&mut self, _: &Target) -> Result<DnsSettings, CoreError> {
        let machine = self.0.borrow();
        if machine.missing {
            return Err(CoreError::Backend("adapter absent".into()));
        }
        Ok(machine.settings.clone())
    }
    fn write(&mut self, _: &Target, settings: &DnsSettings) -> Result<(), CoreError> {
        let mut machine = self.0.borrow_mut();
        machine.writes += 1;
        machine.settings = settings.clone();
        Ok(())
    }
    fn verify_system_doh(&mut self, _: &[Target]) -> Result<(), CoreError> {
        Ok(())
    }
}
impl ServiceBackend for Backend {
    fn adapters(&mut self) -> Result<Vec<AdapterSummary>, CoreError> {
        Ok(vec![])
    }
    fn diagnostics(&mut self) -> Result<serde_json::Value, CoreError> {
        Ok(serde_json::json!({"outcome":"doh_observed"}))
    }
    fn validate_scope(&mut self, _: &[Target]) -> Result<(), CoreError> {
        Ok(())
    }
}

fn original() -> DnsSettings {
    DnsSettings {
        assignment: ServerAssignment::Automatic,
        doh: vec![],
    }
}
fn target() -> Target {
    Target {
        adapter_id: AdapterId::parse("01234567-89ab-cdef-0123-456789abcdef").unwrap(),
        family: AddressFamily::Ipv4,
    }
}
fn request(id: &str, command: Command) -> Request {
    Request {
        version: 1,
        id: id.into(),
        command,
    }
}
fn enable(id: &str, revision: u64) -> Request {
    request(
        id,
        Command::EnableQuad9 {
            expected_revision: revision,
            targets: vec![target()],
        },
    )
}
fn restore(id: &str, revision: u64) -> Request {
    request(
        id,
        Command::RestorePreviousSettings {
            expected_revision: revision,
        },
    )
}
fn error(response: Response, expected: &str) {
    match response.result {
        ResultBody::Error { code, .. } => assert_eq!(code, expected),
        other => panic!("Expected {expected}, got {other:?}"),
    }
}
const ACTOR: &str = "S-1-5-21-1000";

#[test]
fn upgrade_preflight_preserves_active_and_restored_state_without_writes() {
    let backend = Backend::default();
    let mut state = State::default();
    quad9_service::maintenance::check(&mut state).unwrap();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    controller.dispatch(ACTOR, enable("enable", 0));
    let before = serde_json::to_vec(&state.0.borrow().journal).unwrap();
    quad9_service::maintenance::check(&mut state).unwrap();
    assert_eq!(
        serde_json::to_vec(&state.0.borrow().journal).unwrap(),
        before
    );
    assert_eq!(backend.0.borrow().writes, 1);
    controller.dispatch(ACTOR, restore("restore", 1));
    quad9_service::maintenance::check(&mut state).unwrap();
    state.0.borrow_mut().journal = None;
    assert!(quad9_service::maintenance::check(&mut state).is_err());
}

#[test]
fn upgrade_preflight_refuses_interrupted_or_untracked_recovery() {
    let backend = Backend::default();
    let mut state = State::default();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    controller.dispatch(ACTOR, enable("enable", 0));
    state.0.borrow_mut().requests.receipts[0].response = None;
    assert!(quad9_service::maintenance::check(&mut state).is_err());
    state.0.borrow_mut().requests.receipts.clear();
    state.0.borrow_mut().journal.as_mut().unwrap().phase = TransactionPhase::RecoveryRequired;
    assert!(quad9_service::maintenance::check(&mut state).is_err());
    state.0.borrow_mut().journal.as_mut().unwrap().phase = TransactionPhase::Committed;
    state.0.borrow_mut().requests.journal_expected = false;
    assert!(quad9_service::maintenance::check(&mut state).is_err());
}

#[test]
fn original_request_retry_never_reenables_after_restore_or_restart() {
    let backend = Backend::default();
    let state = State::default();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    let first = controller.dispatch(ACTOR, enable("enable-1", 0));
    assert!(matches!(first.result, ResultBody::Enabled { .. }));
    assert!(matches!(
        controller.dispatch(ACTOR, restore("restore-1", 1)).result,
        ResultBody::Restored { complete: true, .. }
    ));
    let writes = backend.0.borrow().writes;
    assert_eq!(controller.dispatch(ACTOR, enable("enable-1", 0)), first);
    drop(controller);
    let mut restarted = Controller::start(backend.clone(), state).unwrap();
    assert_eq!(restarted.dispatch(ACTOR, enable("enable-1", 0)), first);
    assert_eq!(backend.0.borrow().writes, writes);
    assert_eq!(backend.0.borrow().settings, original());
}

#[test]
fn different_payload_cannot_reuse_an_accepted_request_id() {
    let mut controller = Controller::start(Backend::default(), State::default()).unwrap();
    controller.dispatch(ACTOR, enable("one", 0));
    error(controller.dispatch(ACTOR, restore("one", 1)), "id_reused");
}

#[test]
fn receipts_are_scoped_to_the_authenticated_caller() {
    let mut controller = Controller::start(Backend::default(), State::default()).unwrap();
    controller.dispatch(ACTOR, enable("one", 0));
    assert!(matches!(
        controller
            .dispatch("S-1-5-21-2000", enable("one", 1))
            .result,
        ResultBody::Enabled {
            revision: 2,
            already_enabled: true
        }
    ));
}

#[test]
fn evicted_receipts_cannot_replay_an_old_revision() {
    let backend = Backend::default();
    let mut controller = Controller::start(backend.clone(), State::default()).unwrap();
    controller.dispatch(ACTOR, enable("old", 0));
    for revision in 1..=MAX_RECEIPTS as u64 + 1 {
        controller.dispatch(ACTOR, restore(&format!("restore-{revision}"), revision));
    }
    let writes = backend.0.borrow().writes;
    error(
        controller.dispatch(ACTOR, enable("old", 0)),
        "stale_revision",
    );
    assert_eq!(backend.0.borrow().writes, writes);
}

#[test]
fn reserving_a_request_must_succeed_before_any_dns_write() {
    let backend = Backend::default();
    let state = State::default();
    state.0.borrow_mut().fail_reservation = true;
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    error(
        controller.dispatch(ACTOR, enable("one", 0)),
        "storage_unavailable",
    );
    assert_eq!(backend.0.borrow().writes, 0);
    assert!(state.0.borrow().journal.is_none());
}

#[test]
fn lost_result_persistence_blocks_further_mutations() {
    let backend = Backend::default();
    let state = State::default();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    state.0.borrow_mut().fail_completion = true;
    error(
        controller.dispatch(ACTOR, enable("one", 0)),
        "receipt_unavailable",
    );
    let writes = backend.0.borrow().writes;
    error(controller.dispatch(ACTOR, enable("one", 0)), "pending");
    error(
        controller.dispatch(ACTOR, enable("two", 1)),
        "recovery_required",
    );
    assert_eq!(backend.0.borrow().writes, writes);
    drop(controller);
    state.0.borrow_mut().fail_completion = false;
    let mut restarted = Controller::start(backend.clone(), state).unwrap();
    error(restarted.dispatch(ACTOR, enable("one", 0)), "interrupted");
    assert_eq!(backend.0.borrow().writes, writes);
    assert!(matches!(
        restarted.dispatch(ACTOR, restore("restore", 1)).result,
        ResultBody::Restored { complete: true, .. }
    ));
}

#[test]
fn interrupted_receipt_without_a_journal_is_not_a_fresh_install() {
    let backend = Backend::default();
    let state = State::default();
    state.0.borrow_mut().requests = ServiceState {
        revision: 1,
        journal_expected: false,
        receipts: vec![Receipt {
            actor: ACTOR.into(),
            request: enable("one", 0),
            response: None,
        }],
    };
    let mut controller = Controller::start(backend.clone(), state).unwrap();
    error(controller.dispatch(ACTOR, enable("one", 0)), "interrupted");
    error(
        controller.dispatch(ACTOR, enable("two", 1)),
        "recovery_required",
    );
    assert_eq!(backend.0.borrow().writes, 0);
}

#[test]
fn discovered_journal_becomes_required_before_repairing_a_pending_receipt() {
    let backend = Backend::default();
    let state = State::default();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    state.0.borrow_mut().fail_completion = true;
    error(
        controller.dispatch(ACTOR, enable("one", 0)),
        "receipt_unavailable",
    );
    drop(controller);
    {
        let mut saved = state.0.borrow_mut();
        saved.fail_completion = false;
        assert!(!saved.requests.journal_expected);
        assert_eq!(
            saved.journal.as_ref().unwrap().phase,
            TransactionPhase::Committed
        );
    }
    let mut restarted = Controller::start(backend.clone(), state.clone()).unwrap();
    assert!(state.0.borrow().requests.journal_expected);
    error(restarted.dispatch(ACTOR, enable("one", 0)), "interrupted");
    state.0.borrow_mut().journal = None;
    error(
        restarted.dispatch(ACTOR, enable("two", 1)),
        "recovery_required",
    );
    error(
        restarted.dispatch(ACTOR, restore("three", 1)),
        "recovery_required",
    );
    assert_eq!(backend.0.borrow().writes, 1);
    drop(restarted);
    let mut again = Controller::start(backend.clone(), state).unwrap();
    error(
        again.dispatch(ACTOR, enable("four", 1)),
        "recovery_required",
    );
    assert_eq!(backend.0.borrow().writes, 1);
}

#[test]
fn journal_disappearance_during_service_lifetime_blocks_enable_and_restore() {
    let backend = Backend::default();
    let state = State::default();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    controller.dispatch(ACTOR, enable("one", 0));
    state.0.borrow_mut().journal = None;
    let writes = backend.0.borrow().writes;
    let status = controller.dispatch(
        ACTOR,
        Request {
            version: 1,
            id: "inspect-missing".into(),
            command: Command::GetStatus,
        },
    );
    assert!(matches!(
        status.result,
        ResultBody::Status {
            recovery_blocked: true,
            ..
        }
    ));
    error(
        controller.dispatch(ACTOR, enable("two", 1)),
        "recovery_required",
    );
    error(
        controller.dispatch(ACTOR, restore("three", 1)),
        "recovery_required",
    );
    assert_eq!(backend.0.borrow().writes, writes);
}

#[test]
fn losing_request_state_cannot_reset_the_revision_while_a_journal_exists() {
    let backend = Backend::default();
    let state = State::default();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    controller.dispatch(ACTOR, enable("one", 0));
    drop(controller);
    state.0.borrow_mut().requests = ServiceState::default();
    let mut restarted = Controller::start(backend.clone(), state).unwrap();
    error(
        restarted.dispatch(ACTOR, restore("two", 0)),
        "recovery_required",
    );
    assert_eq!(backend.0.borrow().writes, 1);
}

#[test]
fn explicit_restore_can_retry_a_temporarily_absent_adapter() {
    let backend = Backend::default();
    let state = State::default();
    let mut controller = Controller::start(backend.clone(), state).unwrap();
    controller.dispatch(ACTOR, enable("one", 0));
    backend.0.borrow_mut().missing = true;
    match controller.dispatch(ACTOR, restore("two", 1)).result {
        ResultBody::Restored {
            complete, issues, ..
        } => {
            assert!(!complete);
            assert_eq!(issues.len(), 1);
            assert!(issues[0].contains(target().adapter_id.as_str()));
            assert!(issues[0].contains("IPv4"));
            assert!(issues[0].contains("adapter absent"));
        }
        other => panic!("Expected incomplete restoration, got {other:?}"),
    }
    backend.0.borrow_mut().missing = false;
    assert!(matches!(
        controller.dispatch(ACTOR, restore("three", 2)).result,
        ResultBody::Restored { complete: true, .. }
    ));
    assert_eq!(backend.0.borrow().settings, original());
}

#[test]
fn malformed_unknown_and_versioned_commands_never_reserve_mutations() {
    let backend = Backend::default();
    let state = State::default();
    let mut controller = Controller::start(backend.clone(), state.clone()).unwrap();
    let mut bad = enable("one", 0);
    bad.version = 999;
    error(controller.dispatch(ACTOR, bad), "invalid_request");
    error(controller.dispatch("", enable("two", 0)), "unauthorized");
    assert!(serde_json::from_str::<Request>(
        r#"{"version":1,"id":"one","command":{"type":"run_shell","path":"anything"}}"#
    )
    .is_err());
    assert!(state.0.borrow().requests.receipts.is_empty());
    assert_eq!(backend.0.borrow().writes, 0);
}
