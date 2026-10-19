use crate::{protocol::*, runtime::*, storage::AtomicFiles};
use quad9_core::{CoreError, Target};
use quad9_native::{
    service::{self, MachineDirectory, Ready},
    NativeError, WindowsDnsBackend,
};
use std::sync::atomic::AtomicBool;

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
    let mut controller =
        Controller::start(backend, store).map_err(|e| NativeError::InvalidData(e.to_string()))?;
    service::serve(stop, ready, |actor, bytes| {
        let response = match serde_json::from_slice::<Request>(bytes) {
            Ok(request) => controller.dispatch(actor, request),
            Err(_) => Response::error(
                "",
                "invalid_request",
                "Malformed request or unsupported command fields",
            ),
        };
        serde_json::to_vec(&response).expect("Response fields are serializable")
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
