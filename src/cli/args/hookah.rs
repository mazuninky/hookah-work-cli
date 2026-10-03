//! `hw hookah` arguments.

use clap::{ArgGroup, Args, Subcommand, ValueEnum};

use super::ApiName;
use crate::dates::{DateArg, parse_date_arg};

/// Hookahs (needs the «Столы» permission).
#[derive(Debug, Clone, Subcommand)]
pub enum HookahCommand {
    /// Hookahs of bookings and/or of one business day (deleted hookahs excluded)
    List(HookahListArgs),
}

/// `hw hookah list`; give `--booking`, `--date` or both.
#[derive(Debug, Clone, Args)]
#[command(group(ArgGroup::new("filter").required(true).multiple(true).args(["booking", "date"])))]
pub struct HookahListArgs {
    /// Booking id; repeat or comma-separate for several bookings
    #[arg(long, value_name = "ID", value_delimiter = ',')]
    pub booking: Vec<u64>,

    /// Business day: today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: Option<DateArg>,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<HookahExpand>,
}

/// `--expand` values of `hw hookah list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookahExpand {
    /// The service (hookah type)
    Service,
    /// The booking
    Booking,
}

impl ApiName for HookahExpand {
    fn api_name(&self) -> &'static str {
        match self {
            HookahExpand::Service => "service",
            HookahExpand::Booking => "booking",
        }
    }
}
