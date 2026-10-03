//! `hw config` arguments.

use clap::{Args, Subcommand};

/// Configuration profiles.
#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    /// List profiles with their CRM, token storage and default marker
    List,
    /// Show one profile; the token is redacted
    Show(ConfigShowArgs),
    /// Make a profile the default
    SetDefault(ConfigNameArgs),
    /// Delete a profile and its keyring token
    Delete(ConfigNameArgs),
    /// Print the configuration file path (even if the file does not exist yet)
    Path,
}

/// `hw config show`.
#[derive(Debug, Clone, Args)]
pub struct ConfigShowArgs {
    /// Profile name [default: the selected profile]
    #[arg(value_name = "NAME")]
    pub name: Option<String>,
}

/// A command that needs a profile name.
#[derive(Debug, Clone, Args)]
pub struct ConfigNameArgs {
    /// Profile name
    #[arg(value_name = "NAME")]
    pub name: String,
}
