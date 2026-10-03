//! `hw auth` arguments.

use clap::{Args, Subcommand};

use crate::config::TokenStorage;

const LOGIN_LONG_ABOUT: &str =
    "Store an API token for a CRM and make it usable by every other command.

On a terminal, hw asks for the CRM (unless --crm, HW_CRM or the profile names it), then
for an API key or for an email and password, and last where to save the token (unless
--storage is given). A CRM director generates the API key at
https://<crm>.hookah.work/v2/settings/users (hw prints the exact address).

Scripts and CI pass a flag and pipe the secret on stdin, never on the command line:
  --with-token                  one line of stdin is the API key
  --email E --password-stdin    one line of stdin is the password of CRM user E;
                                hw exchanges it for the token via POST /api/login
Without a flag and without a terminal (stdin and stderr), login fails with exit 5.

The token is checked with GET /api/settings before it is saved where --storage says.
Without --storage, a terminal login asks (preselecting the profile's current storage);
otherwise an existing profile keeps its storage and a new one uses the config file
(api_token, 0600 permissions). Saving to one place removes the profile's token from the
other.

The profile name is --profile; without --crm, the default (or only) existing profile;
otherwise the CRM's first host label (demo for demo.hookah.work).";

const LOGIN_AFTER_HELP: &str = "Examples:
  hw auth login
  hw auth login --crm demo --storage keyring
  echo \"$TOKEN\" | hw auth login --crm demo --with-token
  printf '%s\\n' \"$PASSWORD\" | hw auth login --crm demo --email api@example.com --password-stdin
  hw auth login --crm demo --with-token --storage keyring < token.txt";

/// Authentication.
#[derive(Debug, Clone, Subcommand)]
pub enum AuthCommand {
    /// Store an API token for a CRM (prompts on a terminal; flags read stdin)
    #[command(long_about = LOGIN_LONG_ABOUT, after_help = LOGIN_AFTER_HELP)]
    Login(AuthLoginArgs),
    /// Show the profile, CRM, token source and whether the token works (exit 4 if not)
    Status,
    /// Print the resolved API token, for scripts
    Token,
    /// Remove the profile's stored token (the profile itself stays)
    Logout,
}

/// `hw auth login`.
#[derive(Debug, Clone, Args)]
pub struct AuthLoginArgs {
    /// Read the API token from the first line of stdin instead of prompting
    #[arg(long, conflicts_with_all = ["email", "password_stdin"])]
    pub with_token: bool,

    /// Email of the CRM user to log in as (with --password-stdin)
    #[arg(long, value_name = "EMAIL", requires = "password_stdin")]
    pub email: Option<String>,

    /// Read the password from the first line of stdin instead of prompting (with --email)
    #[arg(long, requires = "email")]
    pub password_stdin: bool,

    /// Where to save the token [default: the profile's storage, else config; a terminal login asks]
    #[arg(long, value_name = "STORAGE")]
    pub storage: Option<TokenStorage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::API_KEY_URL_TEMPLATE;

    #[test]
    fn login_help_names_the_api_key_page() {
        assert!(LOGIN_LONG_ABOUT.contains(API_KEY_URL_TEMPLATE));
    }
}
