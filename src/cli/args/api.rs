//! `hw api` arguments.

use clap::Args;

const LONG_ABOUT: &str = "Send a raw GET request to any API path and print the response.

PATH may be `tables`, `api/tables` or `/api/tables`, optionally with `?key=value`.
JSON responses go through --format/--jq/--template; anything else is printed as text:
unchanged when piped, with control characters escaped (ESC as ^[) on a terminal.
There is no method flag: hw api only ever sends GET.";

const AFTER_HELP: &str = "Examples:
  hw api settings
  hw api 'products?type=dish' --query expand=components
  hw api clients --all -F json
  hw api menu --no-auth";

/// `hw api`.
#[derive(Debug, Clone, Args)]
#[command(long_about = LONG_ABOUT, after_help = AFTER_HELP)]
pub struct ApiArgs {
    /// API path: `tables`, `api/tables` or `/api/tables`, optionally with `?key=value`
    #[arg(value_name = "PATH")]
    pub path: String,

    /// Extra query parameter KEY=VALUE (repeatable)
    #[arg(long = "query", value_name = "KEY=VALUE", value_parser = parse_key_value)]
    pub query: Vec<(String, String)>,

    /// Follow pagination and print every page as one list
    #[arg(long)]
    pub all: bool,

    /// Send the request without a token (public endpoints such as `menu`)
    #[arg(long)]
    pub no_auth: bool,
}

/// clap value parser for `KEY=VALUE`.
pub fn parse_key_value(raw: &str) -> Result<(String, String), String> {
    match raw.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => {
            Ok((key.trim().to_owned(), value.to_owned()))
        }
        _ => Err(format!("'{raw}' is not KEY=VALUE")),
    }
}
