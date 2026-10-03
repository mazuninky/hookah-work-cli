//! `hw storage` arguments.

use clap::{Args, Subcommand, ValueEnum};

use super::ApiName;
use crate::dates::{DateArg, parse_date_arg};

/// Storages and purchases (needs the «Склад» permission).
#[derive(Debug, Clone, Subcommand)]
pub enum StorageCommand {
    /// Storages, in interface order
    List,
    /// Purchases of one business day, earliest first
    Purchases(PurchasesArgs),
}

/// `hw storage purchases`.
#[derive(Debug, Clone, Args)]
pub struct PurchasesArgs {
    /// Business day: today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: DateArg,

    /// Only purchases into this storage (ids: `hw storage list`)
    #[arg(long, value_name = "ID")]
    pub storage: Option<u64>,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<PurchaseExpand>,
}

/// `--expand` values of `hw storage purchases`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PurchaseExpand {
    /// The purchased product
    Product,
    /// The receiving storage
    Storage,
}

impl ApiName for PurchaseExpand {
    fn api_name(&self) -> &'static str {
        match self {
            PurchaseExpand::Product => "product",
            PurchaseExpand::Storage => "storage",
        }
    }
}
