//! Offline upgrade preflight. This reads persisted formats without recovery writes.
use crate::runtime::StateStore;
use quad9_core::{CoreError, JournalStore, TransactionPhase};

pub fn check(store: &mut (impl JournalStore + StateStore)) -> Result<(), CoreError> {
    let state = store.load_state()?;
    state.validate().map_err(CoreError::Journal)?;
    let journal = store.load()?;
    if let Some(journal) = &journal {
        journal.validate()?;
    }
    if state.receipts.iter().any(|r| r.response.is_none())
        || (state.journal_expected && journal.is_none())
        || (journal.is_some() && (!state.journal_expected || state.revision == 0))
        || journal.as_ref().is_some_and(|j| {
            !matches!(
                j.phase,
                TransactionPhase::Committed | TransactionPhase::Restored
            )
        })
    {
        return Err(CoreError::Journal(
            "Resolve pending recovery with the installed version before upgrading".into(),
        ));
    }
    Ok(())
}
