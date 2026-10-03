//! Command-line surface: clap definitions in [`args`], handlers in [`commands`].

pub mod args;
pub mod commands;

pub use args::{Cli, Command};
