//! The serialized service controller and durable request-retry contract.
pub mod maintenance;
pub mod protocol;
pub mod runtime;
#[cfg(any(windows, test))]
mod startup_log;
pub mod storage;

pub mod client;
#[cfg(windows)]
pub mod host;
