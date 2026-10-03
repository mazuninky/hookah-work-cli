//! clap definitions: the root [`Cli`], the [`Command`] tree and arguments shared by
//! several commands. Doc comments here are the `--help` text.

mod analytics;
mod api;
mod auth;
mod booking;
mod client;
mod config;
mod hookah;
mod product;
mod reference;
mod report;
mod sale;
mod storage;

pub use analytics::{AnalyticsCommand, PeriodArgs};
pub use api::{ApiArgs, parse_key_value};
pub use auth::{AuthCommand, AuthLoginArgs};
pub use booking::{
    BookingCommand, BookingExpand, BookingListArgs, BookingMyArgs, BookingViewArgs, TimetableArgs,
};
pub use client::{
    ClientCommand, ClientFindArgs, ClientFindExpand, ClientHistoryArgs, ClientListArgs,
};
pub use config::{ConfigCommand, ConfigNameArgs, ConfigShowArgs};
pub use hookah::{HookahCommand, HookahExpand, HookahListArgs};
pub use product::{ProductCommand, ProductExpand, ProductListArgs, ProductType};
pub use reference::RefCommand;
pub use report::{
    BonusPointType, BonusPointsArgs, BonusPointsExpand, CreditType, CreditsArgs, CreditsExpand,
    ExpenseExpand, ExpensesArgs, ReportCommand,
};
pub use sale::{SaleCommand, SaleExpand, SaleListArgs};
pub use storage::{PurchaseExpand, PurchasesArgs, StorageCommand};

use camino::Utf8PathBuf;
use clap::{ArgAction, Args, Parser, Subcommand};

use crate::client::MAX_PER_PAGE;
use crate::output::OutputFormat;

/// Help heading of the flags accepted by every command.
const GLOBAL: &str = "Global Options";

const ABOUT: &str = "Read-only command-line client for the HookahWork CRM API";

const LONG_ABOUT: &str = "Read-only command-line client for the HookahWork CRM API.

hw only reads: every command is a GET request, plus the two non-mutating POSTs the API
needs for logging in and for searching a client. Nothing in the CRM is created, changed
or deleted.";

const AFTER_HELP: &str = "Getting started:
  hw auth login
  hw booking list --date today
  hw product list --search чай -F json

Exit codes: 0 ok, 1 runtime/API/network error, 2 not found, 3 configuration error,
4 authentication error, 5 invalid input.";

/// Read-only command-line client for the HookahWork CRM API.
#[derive(Debug, Parser)]
#[command(name = "hw", version, about = ABOUT, long_about = LONG_ABOUT, after_help = AFTER_HELP)]
pub struct Cli {
    /// Increase logging verbosity (-v, -vv, -vvv); logs go to stderr
    #[arg(short, long, action = ArgAction::Count, global = true, help_heading = GLOBAL)]
    pub verbose: u8,

    /// Print only errors to stderr: no hints, no warnings
    #[arg(short, long, global = true, help_heading = GLOBAL, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Configuration file [default: $XDG_CONFIG_HOME/hw/config.toml or ~/.config/hw/config.toml]
    #[arg(long, global = true, help_heading = GLOBAL, value_name = "PATH", env = "HW_CONFIG")]
    pub config: Option<Utf8PathBuf>,

    /// Profile from the configuration file to use
    #[arg(short, long, global = true, help_heading = GLOBAL, value_name = "NAME", env = "HW_PROFILE")]
    pub profile: Option<String>,

    /// CRM to talk to: subdomain (demo), host (demo.hookah.work) or base URL
    #[arg(long, global = true, help_heading = GLOBAL, value_name = "CRM", env = "HW_CRM")]
    pub crm: Option<String>,

    /// Output format
    #[arg(
        short = 'F',
        long,
        global = true, help_heading = GLOBAL,
        value_name = "FORMAT",
        default_value_t
    )]
    pub format: OutputFormat,

    /// Disable colored output (NO_COLOR is honoured too)
    #[arg(long, global = true, help_heading = GLOBAL)]
    pub no_color: bool,

    /// Do not page console output (also HW_NO_PAGER=1)
    #[arg(long, global = true, help_heading = GLOBAL)]
    pub no_pager: bool,

    /// Filter the JSON result with a jq expression before formatting
    #[arg(long, global = true, help_heading = GLOBAL, value_name = "EXPR")]
    pub jq: Option<String>,

    /// Render the result with a minijinja template instead of --format
    #[arg(long, global = true, help_heading = GLOBAL, value_name = "TPL")]
    pub template: Option<String>,

    /// Retries for rate limiting (429), server errors (5xx) and network failures; 0 disables
    #[arg(long, global = true, help_heading = GLOBAL, value_name = "N", default_value_t = 3)]
    pub retries: u32,

    /// Command to run
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Reference data: settings, tables, payment methods, employees, services, bonus levels
    #[command(subcommand)]
    Ref(RefCommand),

    /// Products and product categories
    #[command(subcommand)]
    Product(ProductCommand),

    /// Storages and purchases
    #[command(subcommand)]
    Storage(StorageCommand),

    /// Clients: list, groups, visit history, lookup
    #[command(subcommand)]
    Client(ClientCommand),

    /// Bookings: daily list, timetable, single booking, device bookings
    #[command(subcommand)]
    Booking(BookingCommand),

    /// Sales at tables or over a business day
    #[command(subcommand)]
    Sale(SaleCommand),

    /// Hookahs served at tables or over a business day
    #[command(subcommand)]
    Hookah(HookahCommand),

    /// Daily reports: expenses, credits, bonus points
    #[command(subcommand)]
    Report(ReportCommand),

    /// Period summaries of hookahs and sales
    #[command(subcommand)]
    Analytics(AnalyticsCommand),

    /// Public electronic menu (works without a token)
    Menu,

    /// Send a raw GET request to any API path
    Api(ApiArgs),

    /// Log in and manage the stored API token
    #[command(subcommand)]
    Auth(AuthCommand),

    /// Inspect and manage configuration profiles
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Print a shell completion script
    Completions(CompletionsArgs),

    /// Write man pages, completions and the markdown reference into a directory
    #[command(hide = true)]
    GenerateDocs(GenerateDocsArgs),
}

/// `hw completions`.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to generate the completion script for
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

/// `hw generate-docs`.
#[derive(Debug, Args)]
pub struct GenerateDocsArgs {
    /// Output directory (created if missing)
    #[arg(long, value_name = "DIR")]
    pub output_dir: Utf8PathBuf,
}

/// Pagination flags of list endpoints.
#[derive(Debug, Clone, Default, Args)]
pub struct PageArgs {
    /// Page to fetch, starting at 1
    #[arg(long, value_name = "N", conflicts_with = "all", value_parser = clap::value_parser!(u32).range(1..))]
    pub page: Option<u32>,

    /// Items per page, 1-100 [server default: 100]
    #[arg(long = "per-page", value_name = "N", value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_PER_PAGE)))]
    pub per_page: Option<u32>,

    /// Fetch every page and print them as one list
    #[arg(long)]
    pub all: bool,
}

/// A CLI value with a distinct spelling in the API (e.g. `bonus-level` → `bonusLevel`).
pub trait ApiName {
    /// The API spelling.
    fn api_name(&self) -> &'static str;
}

/// Joins API names with `separator`, dropping repeats; `None` for an empty list.
#[must_use]
pub fn join_api_names<T: ApiName>(values: &[T], separator: &str) -> Option<String> {
    let mut names: Vec<&'static str> = Vec::new();
    for name in values.iter().map(ApiName::api_name) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    (!names.is_empty()).then(|| names.join(separator))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn per_page_help_names_each_server_default() {
        let mut cli = Cli::command();
        let mut help = |path: &[&str]| {
            let cmd = path
                .iter()
                .fold(&mut cli, |cmd, name| cmd.find_subcommand_mut(name).unwrap());
            cmd.render_help().to_string()
        };
        assert!(help(&["client", "history"]).contains("[server default: 20]"));
        for path in [["client", "list"], ["product", "list"]] {
            assert!(help(&path).contains("[server default: 100]"), "{path:?}");
        }
    }

    #[test]
    fn join_api_names_dedupes_and_maps() {
        let expand = [
            BookingExpand::BonusOperations,
            BookingExpand::Client,
            BookingExpand::BonusOperations,
        ];
        assert_eq!(
            join_api_names(&expand, ",").as_deref(),
            Some("bonusOperations,client")
        );
        assert_eq!(join_api_names::<BookingExpand>(&[], ","), None);
    }
}
