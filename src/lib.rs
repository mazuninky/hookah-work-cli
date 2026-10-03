//! `hw` — a read-only command-line client for the HookahWork CRM API.
//!
//! The binary (`src/main.rs`) only parses arguments, sets up logging and calls
//! [`app::run_system`]; everything else lives in these modules so it can be tested
//! in-process.
#![warn(missing_docs)]

pub mod app;
pub mod auth;
pub mod cli;
pub mod client;
pub mod config;
pub mod dates;
pub mod error;
pub mod io;
pub mod output;
pub mod update;

#[cfg(any(test, feature = "test-util"))]
pub mod test_util;

pub use error::{Error, Result};
