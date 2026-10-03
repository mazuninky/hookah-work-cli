//! `hw client` arguments.

use clap::{ArgGroup, Args, Subcommand, ValueEnum};

use super::{ApiName, PageArgs};

/// Clients (needs the «Клиенты» permission).
#[derive(Debug, Clone, Subcommand)]
pub enum ClientCommand {
    /// Clients, oldest first (paginated)
    List(ClientListArgs),
    /// Client groups, sorted by name
    Groups,
    /// Bookings of one client, newest first (paginated, 20 per page by default)
    History(ClientHistoryArgs),
    /// Find one client by id, card number and/or phone (first match)
    Find(ClientFindArgs),
}

/// `hw client list`.
#[derive(Debug, Clone, Args)]
pub struct ClientListArgs {
    /// Only clients whose name or card number contains TEXT
    #[arg(long, value_name = "TEXT")]
    pub search: Option<String>,

    /// Only clients of this group (ids: `hw client groups`)
    #[arg(long, value_name = "ID")]
    pub group: Option<u64>,

    #[command(flatten)]
    #[allow(missing_docs)]
    pub page: PageArgs,
}

/// `hw client history`; the endpoint pages by 20, not by the 100 of other lists.
#[derive(Debug, Clone, Args)]
#[command(mut_arg("per_page", |arg| arg.help("Items per page, 1-100 [server default: 20]")))]
pub struct ClientHistoryArgs {
    /// Client id
    #[arg(value_name = "ID")]
    pub id: u64,

    #[command(flatten)]
    #[allow(missing_docs)]
    pub page: PageArgs,
}

/// `hw client find`; the criteria combine (all must match).
#[derive(Debug, Clone, Args)]
#[command(group(ArgGroup::new("criteria").required(true).multiple(true).args(["id", "card", "phone"])))]
pub struct ClientFindArgs {
    /// Client id
    #[arg(long, value_name = "ID")]
    pub id: Option<u64>,

    /// Card number
    #[arg(long, value_name = "NUMBER")]
    pub card: Option<u64>,

    /// Phone number or a part of it (country code optional)
    #[arg(long, value_name = "PHONE")]
    pub phone: Option<String>,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<ClientFindExpand>,
}

/// `--expand` values of `hw client find`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ClientFindExpand {
    /// The client's bonus program level
    BonusLevel,
    /// The client's upcoming bookings
    NewBookings,
}

impl ApiName for ClientFindExpand {
    fn api_name(&self) -> &'static str {
        match self {
            ClientFindExpand::BonusLevel => "bonusLevel",
            ClientFindExpand::NewBookings => "newBookings",
        }
    }
}
