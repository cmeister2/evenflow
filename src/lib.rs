//! Structural, source-only Rust linting with shared rule infrastructure.
//!
//! The library is implementation-facing; its Rust API is not yet stable.

pub mod analyzer;
pub mod cli;
pub mod config;
mod flow;
mod logging;
pub mod model;
pub mod registry;
mod resolution;
pub mod runner;
pub mod source;
