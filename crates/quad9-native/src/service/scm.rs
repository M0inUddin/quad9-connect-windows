use super::*;
use std::sync::{
    atomic::{AtomicPtr, Ordering},
    OnceLock,
};
use windows_sys::Win32::{Foundation::*, System::Services::*};

static ENTRY: OnceLock<ServiceEntry> = OnceLock::new();
static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(ptr::null_mut());

fn ready() -> Result<(), NativeError> {
    if STOP.load(Ordering::Acquire) {
        return Err(invalid("Service stop requested during startup"));
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: SERVICE_RUNNING,
        dwControlsAccepted: SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
        ..Default::default()
    };
    // SAFETY: The status handle is registered before the service entry runs.
    if unsafe { SetServiceStatus(STATUS.load(Ordering::Acquire), &status) } == 0 {
        return Err(last_error("SetServiceStatus"));
    }
    Ok(())
}

unsafe extern "system" fn handler(
    control: u32,
    _: u32,
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOP.store(true, Ordering::Release);
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

unsafe extern "system" fn main(_: u32, _: *mut *mut u16) {
    // SAFETY: The fixed name is valid and handler references process-lifetime state.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(wide(SERVICE_NAME).as_ptr(), Some(handler), ptr::null())
    };
    if handle.is_null() {
        return;
    }
    STATUS.store(handle, Ordering::Release);
    let mut status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: SERVICE_START_PENDING,
        dwWaitHint: 30_000,
        dwCheckPoint: 1,
        ..Default::default()
    };
    // SAFETY: handle is registered and status is a correctly initialized structure.
    unsafe {
        SetServiceStatus(handle, &status);
    }
    // The entry reports readiness only after storage recovery and pipe creation.
    let failed = ENTRY.get().is_none_or(|entry| entry(&STOP, ready).is_err())
        && !STOP.load(Ordering::Acquire);
    status.dwCurrentState = SERVICE_STOPPED;
    status.dwControlsAccepted = 0;
    status.dwCheckPoint = 0;
    status.dwWaitHint = 0;
    status.dwWin32ExitCode = if failed {
        ERROR_SERVICE_SPECIFIC_ERROR
    } else {
        NO_ERROR
    };
    status.dwServiceSpecificExitCode = u32::from(failed);
    // SAFETY: The SCM owns this status handle for the lifetime of service main.
    unsafe {
        SetServiceStatus(handle, &status);
    }
}

pub fn run_service(entry: ServiceEntry) -> Result<(), NativeError> {
    ENTRY
        .set(entry)
        .map_err(|_| invalid("Service dispatcher was already initialized"))?;
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: The terminated table and name remain alive until the dispatcher
    // returns after service main stops. No user-controlled entrypoint is accepted.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(last_error("StartServiceCtrlDispatcherW"));
    }
    Ok(())
}
