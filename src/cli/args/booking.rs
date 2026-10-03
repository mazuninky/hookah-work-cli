//! `hw booking` arguments.

use clap::{Args, Subcommand, ValueEnum};

use super::ApiName;
use crate::dates::{DateArg, parse_date_arg};

/// Bookings (needs the «Столы» permission).
#[derive(Debug, Clone, Subcommand)]
pub enum BookingCommand {
    /// Bookings of one business day (deleted bookings excluded)
    List(BookingListArgs),
    /// Bookings of one business day, grouped by table id
    Timetable(TimetableArgs),
    /// One booking by id (deleted bookings included, status 0)
    View(BookingViewArgs),
    /// Upcoming new bookings made from one device
    My(BookingMyArgs),
}

/// `hw booking list`.
#[derive(Debug, Clone, Args)]
pub struct BookingListArgs {
    /// Business day: today (default), yesterday, tomorrow or YYYY-MM-DD; the CRM applies
    /// its night shift to relative days
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: Option<DateArg>,

    /// Nested data to include (comma-separated or repeated); `total` costs one query per booking
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<BookingExpand>,
}

/// `hw booking timetable`.
#[derive(Debug, Clone, Args)]
pub struct TimetableArgs {
    /// Business day: today (default), yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: Option<DateArg>,
}

/// `hw booking view`.
#[derive(Debug, Clone, Args)]
pub struct BookingViewArgs {
    /// Booking id
    #[arg(value_name = "ID")]
    pub id: u64,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<BookingExpand>,
}

/// `hw booking my`.
#[derive(Debug, Clone, Args)]
pub struct BookingMyArgs {
    /// Device id the bookings were created with
    #[arg(long, value_name = "ID")]
    pub device: String,
}

/// `--expand` values of booking commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BookingExpand {
    /// The linked client
    Client,
    /// The table
    Table,
    /// Sales at the table
    Sales,
    /// Hookahs served
    Hookahs,
    /// Payments
    Payments,
    /// Debt operations
    Credit,
    /// Bonus point operations
    BonusOperations,
    /// Bill total
    Total,
}

impl ApiName for BookingExpand {
    fn api_name(&self) -> &'static str {
        match self {
            BookingExpand::Client => "client",
            BookingExpand::Table => "table",
            BookingExpand::Sales => "sales",
            BookingExpand::Hookahs => "hookahs",
            BookingExpand::Payments => "payments",
            BookingExpand::Credit => "credit",
            BookingExpand::BonusOperations => "bonusOperations",
            BookingExpand::Total => "total",
        }
    }
}
