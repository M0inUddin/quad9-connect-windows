use crate::{
    protocol::*,
    runtime::*,
    startup_log::{Event, StartupLog},
    storage::AtomicFiles,
};
use quad9_core::{CoreError, Target};
use quad9_native::{
    service::{self, MachineDirectory, Ready},
    NativeError, WindowsDnsBackend,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

impl ServiceBackend for WindowsDnsBackend {
    fn adapters(&mut self) -> Result<Vec<AdapterSummary>, CoreError> {
        quad9_native::inspect_adapters()
            .map(|adapters| {
                adapters
                    .into_iter()
                    .map(|a| AdapterSummary {
                        id: a.id,
                        name: a.name,
                        candidate: a.candidate,
                        exclusion_reason: a.exclusion_reason,
                        ipv4_enabled: a.ipv4_enabled,
                        ipv6_enabled: a.ipv6_enabled,
                        ipv6_usable: a.ipv6_usable,
                        has_ipv4_dns: a.has_ipv4_dns,
                        has_ipv6_dns: a.has_ipv6_dns,
                    })
                    .collect()
            })
            .map_err(|e| CoreError::Backend(e.to_string()))
    }
    fn diagnostics(&mut self) -> Result<serde_json::Value, CoreError> {
        let probe =
            quad9_native::probe_system_resolver().map_err(|e| CoreError::Backend(e.to_string()))?;
        serde_json::to_value(probe).map_err(|e| CoreError::Backend(e.to_string()))
    }
    fn validate_scope(&mut self, targets: &[Target]) -> Result<(), CoreError> {
        WindowsDnsBackend::validate_scope(self, targets)
    }
}

fn run(stop: &AtomicBool, ready: Ready, backend: WindowsDnsBackend) -> Result<(), NativeError> {
    // Keep the protected directory handle open to prevent its replacement.
    let directory = MachineDirectory::open()?;
    let store = AtomicFiles::open_protected_directory(directory.path())
        .map_err(|e| NativeError::InvalidData(e.to_string()))?;
    // Logging is best effort and has its own lock. It never substitutes for a
    // recovery record or changes whether controller startup is safe.
    let log = StartupLog::new(directory.path()).ok().map(Arc::new);
    let record = |event, code| {
        if let Some(log) = &log {
            let _ = log.record(event, code);
        }
    };
    record(Event::ControllerStarting, None);
    let mut controller = match Controller::start(backend, store) {
        Ok(controller) => controller,
        Err(error) => {
            record(Event::ControllerStartFailed, None);
            return Err(NativeError::InvalidData(error.to_string()));
        }
    };
    record(Event::ControllerStarted, None);
    std::thread::scope(|scope| {
        let (finished, finish) = mpsc::channel::<()>();
        let _cleanup = log.as_ref().and_then(|log| {
            let log = Arc::clone(log);
            std::thread::Builder::new()
                .name("quad9-log-retention".into())
                .spawn_scoped(scope, move || loop {
                    let wait = log.prune().unwrap_or(std::time::Duration::from_secs(60));
                    match finish.recv_timeout(wait) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                })
                .ok()
        });
        let result = service::serve(stop, ready, |actor, bytes| {
            let response = match serde_json::from_slice::<Request>(bytes) {
            Ok(request) if request.revision().is_some()
                && directory.path().join("installer-maintenance").try_exists().unwrap_or(true) =>
                Response::error(&request.id, "maintenance", "Installation or recovery is in progress. Retry after the administrator finishes repair."),
            Ok(request) => controller.dispatch(actor, request),
            Err(_) => Response::error(
                "",
                "invalid_request",
                "Malformed request or unsupported command fields",
            ),
        };
            serde_json::to_vec(&response).expect("Response fields are serializable")
        });
        drop(finished);
        match &result {
            Err(error) if !stop.load(Ordering::Acquire) => record(
                Event::PipeHostFailed,
                match error {
                    NativeError::Windows { code, .. } => Some(*code),
                    _ => None,
                },
            ),
            _ => record(Event::ServiceStopped, None),
        }
        result
    })
}

pub fn entry(stop: &AtomicBool, ready: Ready) -> Result<(), NativeError> {
    run(stop, ready, WindowsDnsBackend::new())
}

#[cfg(feature = "vm-validation")]
pub fn validation_entry(stop: &AtomicBool, ready: Ready) -> Result<(), NativeError> {
    run(stop, ready, WindowsDnsBackend::for_disposable_vm())
}

pub fn send(request: &Request) -> Result<Response, String> {
    request.validate()?;
    let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    let bytes = service::transact(&bytes).map_err(|e| e.to_string())?;
    let response: Response = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if response.version != PROTOCOL_VERSION || response.id != request.id {
        return Err("The service response does not match this request".into());
    }
    Ok(response)
}

/// The exclusive store lock prevents racing either the service or another repair.
/// This command accepts neither an Enable operation nor a caller-selected path.
pub fn maintenance(restore: bool) -> Result<(), String> {
    let actor = service::require_administrator().map_err(|e| e.to_string())?;
    let directory = MachineDirectory::open().map_err(|e| e.to_string())?;
    let mut store =
        AtomicFiles::open_protected_directory(directory.path()).map_err(|e| e.to_string())?;
    if !restore {
        crate::maintenance::check(&mut store).map_err(|e| e.to_string())?;
        println!(r#"{{"compatible":true,"protocol_version":1,"storage_version":1}}"#);
        return Ok(());
    }
    let mut controller =
        Controller::start(WindowsDnsBackend::new(), store).map_err(|e| e.to_string())?;
    let status = controller.dispatch(
        &actor,
        Request {
            version: PROTOCOL_VERSION,
            id: "maintenance-status".into(),
            command: Command::GetStatus,
        },
    );
    let ResultBody::Status { revision, .. } = status.result else {
        return Err("Recovery status is unavailable; retain the service and journal".into());
    };
    let response = controller.dispatch(
        &actor,
        Request {
            version: PROTOCOL_VERSION,
            // Offline invocations must not replay an earlier restoration receipt.
            id: format!("maintenance-{}-{}", std::process::id(), revision),
            command: Command::RestorePreviousSettings {
                expected_revision: revision,
            },
        },
    );
    println!(
        "{}",
        serde_json::to_string(&response).map_err(|e| e.to_string())?
    );
    match response.result {
        ResultBody::Restored { complete: true, .. } => Ok(()),
        _ => Err(
            "DNS restoration is incomplete. Retain the installed service and recovery records."
                .into(),
        ),
    }
}
