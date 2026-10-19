//! In-process coordination for unprivileged clients. The service independently
//! authorizes requests and serializes all configuration transactions.

use crate::protocol::{Command, Request, Response};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
pub struct ClientCoordinator {
    gate: Mutex<()>,
    pub generation: Arc<AtomicU64>,
}

pub struct ObservedResponse {
    pub response: Response,
    pub generation: u64,
}

impl ClientCoordinator {
    pub fn execute(
        &self,
        request: &Request,
        send: impl FnOnce(&Request) -> Result<Response, String>,
    ) -> Result<ObservedResponse, String> {
        request.validate()?;
        let _guard = self
            .gate
            .lock()
            .map_err(|_| "The request coordinator is unavailable".to_owned())?;
        // Capture after acquiring the gate. A queued request may observe a
        // different network from the one present when the user clicked.
        let generation = self.generation.load(Ordering::SeqCst);
        let response = send(request)?;
        let observation = matches!(
            request.command,
            Command::GetStatus | Command::ListAdapters | Command::RunDiagnostics
        );
        if observation && generation != self.generation.load(Ordering::SeqCst) {
            return Err("The network changed during this check. Read current state and run diagnostics again.".into());
        }
        // A known mutation receipt remains authoritative even if the network
        // changed while it was running. The caller refreshes status separately.
        Ok(ObservedResponse {
            response,
            generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ResultBody;
    use std::sync::mpsc;

    fn diagnostic() -> Request {
        Request {
            version: 1,
            id: "diagnostic".into(),
            command: Command::RunDiagnostics,
        }
    }
    fn report(request: &Request) -> Response {
        Response {
            version: 1,
            id: request.id.clone(),
            result: ResultBody::Diagnostics {
                report: serde_json::json!({"outcome": "doh_observed"}),
            },
        }
    }

    #[test]
    fn queued_diagnostic_uses_the_generation_after_acquiring_the_gate() {
        let coordinator = Arc::new(ClientCoordinator::default());
        let gate = coordinator.gate.lock().unwrap();
        let queued = coordinator.clone();
        let (ready, waiting) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let request = diagnostic();
            let at_action = queued.generation.load(Ordering::SeqCst);
            ready.send(at_action).unwrap();
            queued
                .execute(&request, |request| Ok(report(request)))
                .unwrap()
        });
        assert_eq!(waiting.recv().unwrap(), 0);
        coordinator.generation.fetch_add(1, Ordering::SeqCst);
        drop(gate);
        let observed = thread.join().unwrap();
        assert_eq!(observed.generation, 1);
        assert_eq!(observed.response, report(&diagnostic()));
    }

    #[test]
    fn a_network_change_during_a_diagnostic_rejects_its_report() {
        let coordinator = ClientCoordinator::default();
        let result = coordinator.execute(&diagnostic(), |request| {
            coordinator.generation.fetch_add(1, Ordering::SeqCst);
            Ok(report(request))
        });
        assert!(result.err().unwrap().contains("network changed"));
    }

    #[test]
    fn a_network_change_does_not_discard_a_known_mutation_receipt() {
        let coordinator = ClientCoordinator::default();
        let request = Request {
            version: 1,
            id: "restore".into(),
            command: Command::RestorePreviousSettings {
                expected_revision: 1,
            },
        };
        let expected = Response {
            version: 1,
            id: request.id.clone(),
            result: ResultBody::Restored {
                revision: 2,
                complete: true,
                issues: vec![],
            },
        };
        let observed = coordinator
            .execute(&request, |_| {
                coordinator.generation.fetch_add(1, Ordering::SeqCst);
                Ok(expected.clone())
            })
            .unwrap();
        assert_eq!(observed.response, expected);
    }
}
