use quad9_core::*;
use std::{collections::BTreeMap, net::IpAddr};

#[derive(Default)]
struct MemoryStore {
    saved: Option<Journal>,
    saves: usize,
    fail_on: Option<usize>,
    fail_from: Option<usize>,
    unavailable: bool,
}

impl JournalStore for MemoryStore {
    fn load(&mut self) -> Result<Option<Journal>, CoreError> {
        if self.unavailable {
            return Err(CoreError::Journal("unreadable".into()));
        }
        Ok(self.saved.clone())
    }
    fn save(&mut self, journal: &Journal) -> Result<(), CoreError> {
        self.saves += 1;
        if self.fail_on == Some(self.saves) || self.fail_from.is_some_and(|n| self.saves >= n) {
            return Err(CoreError::Journal("disk failure".into()));
        }
        // Exercise the format across the same persistence boundary as a service restart.
        self.saved = Some(serde_json::from_slice(&serde_json::to_vec(journal).unwrap()).unwrap());
        Ok(())
    }
}

#[derive(Default)]
struct FakeDns {
    settings: BTreeMap<Target, DnsSettings>,
    writes: usize,
    fail_write: Option<usize>,
    fail_after_write: bool,
    verify_fails: bool,
    missing: Option<Target>,
    change_during_verify: bool,
    external_change_on_success: Option<Target>,
}

impl DnsBackend for FakeDns {
    fn read(&mut self, target: &Target) -> Result<DnsSettings, CoreError> {
        if self.missing.as_ref() == Some(target) {
            return Err(CoreError::Backend("adapter absent".into()));
        }
        self.settings
            .get(target)
            .cloned()
            .ok_or_else(|| CoreError::Backend("unknown adapter".into()))
    }
    fn write(&mut self, target: &Target, settings: &DnsSettings) -> Result<(), CoreError> {
        self.writes += 1;
        let fail = self.fail_write == Some(self.writes);
        if fail && !self.fail_after_write {
            return Err(CoreError::Backend("write rejected".into()));
        }
        self.settings.insert(target.clone(), settings.clone());
        if fail {
            return Err(CoreError::Backend("ambiguous write completion".into()));
        }
        Ok(())
    }
    fn verify_system_doh(&mut self, targets: &[Target]) -> Result<(), CoreError> {
        if let Some(target) = &self.external_change_on_success {
            let address = if target.family == AddressFamily::Ipv4 {
                "192.0.2.99"
            } else {
                "2001:db8::99"
            };
            self.settings.insert(target.clone(), static_dns(address));
        }
        if self.change_during_verify {
            self.settings
                .insert(targets[0].clone(), static_dns("192.0.2.99"));
        }
        if self.verify_fails || self.change_during_verify {
            return Err(CoreError::Backend("verification failed".into()));
        }
        Ok(())
    }
}

fn target(family: AddressFamily) -> Target {
    Target {
        adapter_id: AdapterId::parse("01234567-89ab-cdef-0123-456789abcdef").unwrap(),
        family,
    }
}
fn automatic() -> DnsSettings {
    DnsSettings {
        assignment: ServerAssignment::Automatic,
        doh: vec![],
    }
}
fn static_dns(address: &str) -> DnsSettings {
    DnsSettings {
        assignment: ServerAssignment::Static {
            servers: vec![address.parse::<IpAddr>().unwrap()],
        },
        doh: vec![],
    }
}
fn setup() -> Engine<FakeDns, MemoryStore> {
    let mut backend = FakeDns::default();
    backend
        .settings
        .insert(target(AddressFamily::Ipv4), automatic());
    backend
        .settings
        .insert(target(AddressFamily::Ipv6), static_dns("2001:db8::53"));
    Engine {
        backend,
        store: MemoryStore::default(),
    }
}
fn both() -> Vec<Target> {
    vec![target(AddressFamily::Ipv4), target(AddressFamily::Ipv6)]
}

#[test]
fn enable_and_restore_preserve_assignment_mode_and_static_order() {
    let mut engine = setup();
    let before = engine.backend.settings.clone();
    assert_eq!(engine.enable(&both()).unwrap(), EnableOutcome::Enabled);
    assert_eq!(
        engine.store.saved.as_ref().unwrap().phase,
        TransactionPhase::Committed
    );
    assert!(engine.restore().unwrap().is_complete());
    assert_eq!(engine.backend.settings, before);
}

#[test]
fn restores_existing_encryption_properties_exactly() {
    let mut engine = setup();
    let ipv4 = target(AddressFamily::Ipv4);
    let mut original = static_dns("192.0.2.53");
    if let ServerAssignment::Static { servers } = &mut original.assignment {
        servers.push("192.0.2.54".parse().unwrap());
    }
    original.doh.push(DohProperty {
        server_index: 0,
        template: Some("https://resolver.example/dns-query".into()),
        flags: 6,
    });
    engine
        .backend
        .settings
        .insert(ipv4.clone(), original.clone());
    engine.enable(std::slice::from_ref(&ipv4)).unwrap();
    engine.restore().unwrap();
    assert_eq!(engine.backend.settings[&ipv4], original);
}

#[test]
fn repeated_enable_keeps_first_snapshot_and_does_not_write() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    let journal = engine.store.saved.clone();
    let writes = engine.backend.writes;
    assert_eq!(
        engine.enable(&both()).unwrap(),
        EnableOutcome::AlreadyEnabled
    );
    assert_eq!(engine.store.saved, journal);
    assert_eq!(engine.backend.writes, writes);
}

#[test]
fn repeated_enable_rechecks_every_target_after_a_successful_probe() {
    for changed in both() {
        let mut engine = setup();
        engine.enable(&both()).unwrap();
        let journal = engine.store.saved.clone();
        let writes = engine.backend.writes;
        engine.backend.external_change_on_success = Some(changed.clone());
        assert!(matches!(
            engine.enable(&both()),
            Err(CoreError::Conflict(_))
        ));
        assert_eq!(engine.backend.writes, writes);
        assert_eq!(engine.store.saved, journal);
        assert_ne!(
            engine.backend.settings[&changed],
            DnsSettings::quad9(changed.family)
        );
    }
}

#[test]
fn rejects_changed_target_scope_while_active() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    assert!(matches!(
        engine.enable(&both()[..1]),
        Err(CoreError::ActiveTransaction)
    ));
}

#[test]
fn failure_on_second_family_restores_first() {
    let mut engine = setup();
    let before = engine.backend.settings.clone();
    engine.backend.fail_write = Some(2);
    assert!(matches!(
        engine.enable(&both()),
        Err(CoreError::EnableFailed {
            recovered: true,
            ..
        })
    ));
    assert_eq!(engine.backend.settings, before);
}

#[test]
fn failed_write_that_already_applied_is_recovered_from_intent() {
    let mut engine = setup();
    let before = engine.backend.settings.clone();
    engine.backend.fail_write = Some(1);
    engine.backend.fail_after_write = true;
    assert!(matches!(
        engine.enable(&both()),
        Err(CoreError::EnableFailed {
            recovered: true,
            ..
        })
    ));
    assert_eq!(engine.backend.settings, before);
}

#[test]
fn initial_verification_failure_rolls_back() {
    let mut engine = setup();
    let before = engine.backend.settings.clone();
    engine.backend.verify_fails = true;
    assert!(matches!(
        engine.enable(&both()),
        Err(CoreError::EnableFailed {
            recovered: true,
            ..
        })
    ));
    assert_eq!(engine.backend.settings, before);
}

#[test]
fn later_verification_failure_preserves_committed_policy() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    let enabled = engine.backend.settings.clone();
    engine.backend.verify_fails = true;
    assert!(engine.enable(&both()).is_err());
    assert_eq!(engine.backend.settings, enabled);
    assert_eq!(
        engine.store.saved.as_ref().unwrap().phase,
        TransactionPhase::Committed
    );
}

#[test]
fn external_change_during_failed_enable_is_preserved() {
    let mut engine = setup();
    engine.backend.change_during_verify = true;
    assert!(matches!(
        engine.enable(&both()),
        Err(CoreError::EnableFailed {
            recovered: false,
            ..
        })
    ));
    assert_eq!(
        engine.backend.settings[&target(AddressFamily::Ipv4)],
        static_dns("192.0.2.99")
    );
    assert_eq!(
        engine.store.saved.as_ref().unwrap().phase,
        TransactionPhase::RecoveryRequired
    );
}

#[test]
fn restore_preserves_external_edits_and_restores_other_family() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    engine
        .backend
        .settings
        .insert(target(AddressFamily::Ipv4), static_dns("192.0.2.44"));
    let report = engine.restore().unwrap();
    assert_eq!(report.issues.len(), 1);
    assert_eq!(
        engine.backend.settings[&target(AddressFamily::Ipv4)],
        static_dns("192.0.2.44")
    );
    assert_eq!(
        engine.backend.settings[&target(AddressFamily::Ipv6)],
        static_dns("2001:db8::53")
    );
}

#[test]
fn an_absent_adapter_can_be_recovered_when_it_returns() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    engine.backend.missing = Some(target(AddressFamily::Ipv4));
    assert!(!engine.restore().unwrap().is_complete());
    engine.backend.missing = None;
    assert!(engine.recover_interrupted().unwrap().is_complete());
    assert_eq!(
        engine.backend.settings[&target(AddressFamily::Ipv4)],
        automatic()
    );
}

#[test]
fn restoration_write_can_be_retried_after_transient_failure() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    engine.backend.fail_write = Some(3);
    assert!(!engine.restore().unwrap().is_complete());
    assert!(engine.recover_interrupted().unwrap().is_complete());
}

#[test]
fn committed_configuration_survives_startup_recovery() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    let writes = engine.backend.writes;
    assert!(engine.recover_interrupted().unwrap().is_complete());
    assert_eq!(engine.backend.writes, writes);
}

#[test]
fn interrupted_apply_after_native_write_is_restored() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    let journal = engine.store.saved.as_mut().unwrap();
    journal.phase = TransactionPhase::Applying;
    journal.entries[1].phase = EntryPhase::WriteIntent;
    assert!(engine.recover_interrupted().unwrap().is_complete());
    assert_eq!(
        engine.backend.settings[&target(AddressFamily::Ipv4)],
        automatic()
    );
}

#[test]
fn corrupt_or_unknown_journal_never_writes() {
    for version in [0, 2, u32::MAX] {
        let mut engine = setup();
        engine.enable(&both()).unwrap();
        engine.store.saved.as_mut().unwrap().schema_version = version;
        let writes = engine.backend.writes;
        assert!(engine.restore().is_err());
        assert_eq!(engine.backend.writes, writes);
    }
}

#[test]
fn journal_load_failure_never_writes() {
    let mut engine = setup();
    engine.store.unavailable = true;
    assert!(engine.enable(&both()).is_err());
    assert_eq!(engine.backend.writes, 0);
}

#[test]
fn every_enable_journal_boundary_recovers_without_losing_baseline() {
    // Initial plan, two intent+completion pairs, then commit.
    for save in 1..=6 {
        let mut engine = setup();
        let before = engine.backend.settings.clone();
        engine.store.fail_on = Some(save);
        assert!(engine.enable(&both()).is_err());
        assert_eq!(engine.backend.settings, before, "save {save}");
    }
}

#[test]
fn every_restore_journal_boundary_remains_recoverable() {
    for offset in 1..=6 {
        let mut engine = setup();
        let before = engine.backend.settings.clone();
        engine.enable(&both()).unwrap();
        engine.store.fail_on = Some(engine.store.saves + offset);
        assert!(engine.restore().is_err());
        engine.store.fail_on = None;
        if offset == 1 {
            // The restore intent never became durable. Startup must preserve
            // the committed configuration; the caller can explicitly retry.
            let writes = engine.backend.writes;
            engine.recover_interrupted().unwrap();
            assert_eq!(engine.backend.writes, writes);
            assert!(engine.restore().unwrap().is_complete());
        } else {
            assert!(engine.recover_interrupted().unwrap().is_complete());
        }
        assert_eq!(engine.backend.settings, before, "restore save {offset}");
    }
}

#[test]
fn duplicate_or_empty_selection_never_writes() {
    for targets in [vec![], vec![target(AddressFamily::Ipv4); 2]] {
        let mut engine = setup();
        assert!(engine.enable(&targets).is_err());
        assert_eq!(engine.backend.writes, 0);
        assert!(engine.store.saved.is_none());
    }
}

#[test]
fn profile_is_encrypted_for_both_families_without_fallback() {
    for family in [AddressFamily::Ipv4, AddressFamily::Ipv6] {
        let settings = DnsSettings::quad9(family);
        settings.validate(family).unwrap();
        assert_eq!(settings.doh.len(), 2);
        assert!(settings
            .doh
            .iter()
            .all(|p| p.flags == 2 && p.template.as_deref() == Some(QUAD9_DOH_TEMPLATE)));
    }
}

#[test]
fn reject_wrong_family_and_unknown_encryption_properties() {
    assert!(static_dns("192.0.2.1")
        .validate(AddressFamily::Ipv6)
        .is_err());
    let mut settings = DnsSettings::quad9(AddressFamily::Ipv4);
    settings.doh[0].flags |= 1 << 20;
    assert!(settings.validate(AddressFamily::Ipv4).is_err());
}

#[test]
fn adapter_identifiers_are_canonical_and_not_paths() {
    assert_eq!(
        AdapterId::parse("{01234567-89AB-CDEF-0123-456789ABCDEF}").unwrap(),
        target(AddressFamily::Ipv4).adapter_id
    );
    assert!(AdapterId::parse("../../Interfaces").is_err());
    assert!(AdapterId::parse("{01234567-89ab-cdef-0123-456789abcdef").is_err());
}

#[test]
fn journal_deserialization_rejects_unknown_fields() {
    assert!(serde_json::from_str::<DnsSettings>(
        r#"{"assignment":{"mode":"automatic"},"doh":[],"command":"anything"}"#
    )
    .is_err());
}

#[test]
fn permanent_disk_failure_retains_intent_for_later_recovery() {
    let mut engine = setup();
    let before = engine.backend.settings.clone();
    engine.store.fail_from = Some(3); // After the first native write.
    assert!(matches!(
        engine.enable(&both()),
        Err(CoreError::EnableFailed {
            recovered: false,
            ..
        })
    ));
    assert_eq!(
        engine.store.saved.as_ref().unwrap().entries[0].phase,
        EntryPhase::WriteIntent
    );
    assert_eq!(engine.backend.writes, 1);
    engine.store.fail_from = None;
    assert!(engine.recover_interrupted().unwrap().is_complete());
    assert_eq!(engine.backend.settings, before);
}

#[test]
fn planned_entry_does_not_claim_an_external_write_to_the_same_profile() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    let journal = engine.store.saved.as_mut().unwrap();
    journal.phase = TransactionPhase::Applying;
    journal.entries[0].phase = EntryPhase::Planned;
    // Treat this as a competing writer's value, since no write intent exists.
    assert!(!engine.recover_interrupted().unwrap().is_complete());
    assert_eq!(
        engine.backend.settings[&target(AddressFamily::Ipv4)],
        DnsSettings::quad9(AddressFamily::Ipv4)
    );
}

#[test]
fn tampered_profile_and_inconsistent_terminal_states_are_rejected() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    let valid = engine.store.saved.clone().unwrap();
    let mut altered = valid.clone();
    altered.entries[0].applied = static_dns("192.0.2.123");
    assert!(altered.validate().is_err());
    let mut inconsistent = valid;
    inconsistent.entries[0].phase = EntryPhase::Planned;
    assert!(inconsistent.validate().is_err());
}

#[test]
fn unfinished_recovery_blocks_new_enable_without_writes() {
    let mut engine = setup();
    engine.enable(&both()).unwrap();
    engine.backend.missing = Some(target(AddressFamily::Ipv4));
    assert!(!engine.restore().unwrap().is_complete());
    let writes = engine.backend.writes;
    assert!(matches!(
        engine.enable(&both()),
        Err(CoreError::ActiveTransaction)
    ));
    assert_eq!(engine.backend.writes, writes);
}
