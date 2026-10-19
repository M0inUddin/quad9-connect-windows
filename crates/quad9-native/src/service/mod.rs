//! LocalSystem service host, protected storage, and authenticated local IPC.
mod pipe;
mod scm;
mod security;

pub use pipe::{serve, transact};
pub use scm::run_service;
pub use security::{require_administrator, validate_private_file, MachineDirectory};

use crate::NativeError;
use std::{ptr, sync::atomic::AtomicBool};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

pub const SERVICE_NAME: &str = "Quad9Connect";
pub const OPERATORS_GROUP: &str = "Quad9ConnectOperators";
pub const PIPE_NAME: &str = r"\\.\pipe\Quad9Connect.v1";
pub const MAX_FRAME: usize = 64 * 1024;

pub(super) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
pub(super) fn last_error(operation: &'static str) -> NativeError {
    NativeError::Windows {
        operation,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(1) as u32,
    }
}

pub(super) struct Handle(pub HANDLE);
impl Handle {
    pub fn checked(value: HANDLE, operation: &'static str) -> Result<Self, NativeError> {
        if value.is_null() || value == INVALID_HANDLE_VALUE {
            Err(last_error(operation))
        } else {
            Ok(Self(value))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: Handle owns one valid Windows kernel handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct LocalAllocation(pub *mut std::ffi::c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: The allocation came from an API documented to use LocalFree.
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }
}

pub(super) fn invalid(message: &str) -> NativeError {
    NativeError::InvalidData(message.into())
}
pub(super) fn null_handle() -> HANDLE {
    ptr::null_mut()
}
pub type Ready = fn() -> Result<(), NativeError>;
pub type ServiceEntry = fn(&AtomicBool, Ready) -> Result<(), NativeError>;
