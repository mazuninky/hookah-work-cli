//! `hw sale` arguments.

use clap::{ArgGroup, Args, Subcommand, ValueEnum};

use super::ApiName;
use crate::dates::{DateArg, parse_date_arg};

/// Sales (needs the «Продажи» permission).
#[derive(Debug, Clone, Subcommand)]
pub enum SaleCommand {
    /// Sales of bookings and/or of one business day (deleted sales excluded)
    List(SaleListArgs),
}

/// `hw sale list`; give `--booking`, `--date` or both.
#[derive(Debug, Clone, Args)]
#[command(group(ArgGroup::new("filter").required(true).multiple(true).args(["booking", "date"])))]
pub struct SaleListArgs {
    /// Booking id; repeat or comma-separate for several bookings
    #[arg(long, value_name = "ID", value_delimiter = ',')]
    pub booking: Vec<u64>,

    /// Business day: today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: Option<DateArg>,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<SaleExpand>,
}

/// `--expand` values of `hw sale list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SaleExpand {
    /// The sold product
    Product,
    /// The booking the sale belongs to
    Booking,
    /// The employee who sold it
    Seller,
}

impl ApiName for SaleExpand {
    fn api_name(&self) -> &'static str {
        match self {
            SaleExpand::Product => "product",
            SaleExpand::Booking => "booking",
            SaleExpand::Seller => "seller",
        }
    }
}
