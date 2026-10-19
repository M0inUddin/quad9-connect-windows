//! DNS transaction rules shared by the future service and its recovery tool.
//!
//! This crate does not change Windows configuration. A production backend and
//! protected durable journal must pass VM validation before exposing mutations.

mod engine;
mod model;

pub use engine::*;
pub use model::*;
