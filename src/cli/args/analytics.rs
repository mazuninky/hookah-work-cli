//! `hw analytics` arguments.

use clap::{Args, Subcommand};

use crate::dates::{DateArg, parse_date_arg};

/// Period summaries over closed tables (needs the «Аналитика» permission).
#[derive(Debug, Clone, Subcommand)]
pub enum AnalyticsCommand {
    /// Hookahs served per service, by revenue
    Hookah(PeriodArgs),
    /// Products sold per product, by turnover
    Sale(PeriodArgs),
}

/// A period of business days, both ends included.
#[derive(Debug, Clone, Args)]
pub struct PeriodArgs {
    /// First business day: today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub from: DateArg,

    /// Last business day (included): today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub till: DateArg,
}
