//! `hw self` arguments.

use clap::{Args, Subcommand};

use crate::update::Version;

const UPDATE_LONG_ABOUT: &str =
    "Download a hw release from GitHub and replace the running hw binary with it.

Installs the latest release, or the one --to names. The SHA-256 checksum of the release
archive is verified against the .sha256 file published with it before anything is unpacked;
on a mismatch nothing is replaced. The new binary takes the old one's place in a single step
(on macOS it is ad-hoc signed first, so keychain access keeps working).

Prebuilt releases exist for x86_64 Linux (glibc), Apple Silicon macOS and x86_64 Windows;
elsewhere build from source with cargo install. A hw installed by Homebrew, Nix or the system
package manager is left alone: update it there. Nothing is sent to the CRM.";

const UPDATE_AFTER_HELP: &str = "Examples:
  hw self update
  hw self update --to 2026.41.1
  hw self update --to 2026.38.2 --allow-downgrade";

/// `hw self`: the hw binary itself.
#[derive(Debug, Clone, Subcommand)]
pub enum SelfCommand {
    /// Compare this hw with the latest release on GitHub (exit 0 with or without an update)
    Check,
    /// Replace this hw binary with a release from GitHub, verifying its SHA-256 checksum
    #[command(long_about = UPDATE_LONG_ABOUT, after_help = UPDATE_AFTER_HELP)]
    Update(SelfUpdateArgs),
}

/// `hw self update`.
#[derive(Debug, Clone, Args)]
pub struct SelfUpdateArgs {
    /// Version to install instead of the latest release: YEAR.WEEK.BUILD, e.g. 2026.41.1 or v2026.41.1
    #[arg(long, value_name = "VERSION")]
    pub to: Option<Version>,

    /// Let --to install a version older than this hw
    #[arg(long, requires = "to")]
    pub allow_downgrade: bool,
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::args::{Cli, Command};
    use crate::error::{exit_code, exit_code_for_clap_error};

    fn update_args(argv: &[&str]) -> Result<SelfUpdateArgs, clap::Error> {
        let cli = Cli::try_parse_from(["hw", "self", "update"].iter().chain(argv))?;
        match cli.command {
            Command::SelfCmd(SelfCommand::Update(args)) => Ok(args),
            other => panic!("parsed {other:?}"),
        }
    }

    #[test]
    fn to_accepts_a_v_prefix_and_a_padded_week() {
        for to in ["2027.1.1", "v2027.1.1", "2027.01.1", "v2027.01.1"] {
            let args = update_args(&["--to", to]).unwrap();
            assert_eq!(args.to, Some(Version::new(2027, 1, 1)), "{to}");
        }
    }

    #[test]
    fn malformed_to_is_a_usage_error() {
        let err = update_args(&["--to", "2026.4x.1"]).unwrap_err();
        assert_eq!(exit_code_for_clap_error(&err), exit_code::INPUT_ERROR);
        assert!(
            err.to_string().contains("expected YEAR.WEEK.BUILD"),
            "{err}"
        );
    }

    #[test]
    fn allow_downgrade_requires_to() {
        let err = update_args(&["--allow-downgrade"]).unwrap_err();
        assert_eq!(exit_code_for_clap_error(&err), exit_code::INPUT_ERROR);
        assert!(
            update_args(&["--to", "2026.38.1", "--allow-downgrade"])
                .unwrap()
                .allow_downgrade
        );
    }
}
