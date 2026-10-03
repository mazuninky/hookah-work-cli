//! `hw report` arguments.

use clap::{Args, Subcommand, ValueEnum};

use super::ApiName;
use crate::dates::{DateArg, parse_date_arg};

/// Daily reports; each covers one business day.
#[derive(Debug, Clone, Subcommand)]
pub enum ReportCommand {
    /// Expenses of one business day (needs «Расходы»)
    Expenses(ExpensesArgs),
    /// Debt and repayment operations of one business day (needs «Бухгалтерия»)
    Credits(CreditsArgs),
    /// Bonus point operations of one business day (needs «Бухгалтерия»)
    BonusPoints(BonusPointsArgs),
}

/// `hw report expenses`.
#[derive(Debug, Clone, Args)]
pub struct ExpensesArgs {
    /// Business day: today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: DateArg,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<ExpenseExpand>,
}

/// `--expand` values of `hw report expenses`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExpenseExpand {
    /// The expense category
    Category,
    /// The payment method
    PaymentMethod,
}

impl ApiName for ExpenseExpand {
    fn api_name(&self) -> &'static str {
        match self {
            ExpenseExpand::Category => "category",
            ExpenseExpand::PaymentMethod => "paymentMethod",
        }
    }
}

/// `hw report credits`.
#[derive(Debug, Clone, Args)]
pub struct CreditsArgs {
    /// Business day: today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: DateArg,

    /// Only operations of this type [server default: debt and return]
    #[arg(long = "type", value_name = "TYPE")]
    pub kind: Option<CreditType>,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<CreditsExpand>,
}

/// Credit operation types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CreditType {
    /// Debt taken (1)
    Debt,
    /// Repayment or deposit (2)
    Return,
    /// Debt outside the cash register (3)
    DebtOffCash,
    /// Deposit outside the cash register (4)
    DepositOffCash,
}

impl ApiName for CreditType {
    fn api_name(&self) -> &'static str {
        match self {
            CreditType::Debt => "1",
            CreditType::Return => "2",
            CreditType::DebtOffCash => "3",
            CreditType::DepositOffCash => "4",
        }
    }
}

/// `--expand` values of `hw report credits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CreditsExpand {
    /// The client
    Client,
    /// The payment method
    PaymentMethod,
}

impl ApiName for CreditsExpand {
    fn api_name(&self) -> &'static str {
        match self {
            CreditsExpand::Client => "client",
            CreditsExpand::PaymentMethod => "paymentMethod",
        }
    }
}

/// `hw report bonus-points`.
#[derive(Debug, Clone, Args)]
pub struct BonusPointsArgs {
    /// Business day: today, yesterday, tomorrow or YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub date: DateArg,

    /// Only operations of this type
    #[arg(long = "type", value_name = "TYPE")]
    pub kind: Option<BonusPointType>,

    /// Nested data to include (comma-separated or repeated)
    #[arg(long, value_name = "FIELD", value_delimiter = ',')]
    pub expand: Vec<BonusPointsExpand>,
}

/// Bonus point operation types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BonusPointType {
    /// Earned for a visit (1)
    Visit,
    /// Spent on a bill (2)
    Spend,
    /// Granted manually (3)
    Gift,
    /// Taken away manually (4)
    Confiscation,
    /// Expired (5)
    Expiry,
}

impl ApiName for BonusPointType {
    fn api_name(&self) -> &'static str {
        match self {
            BonusPointType::Visit => "1",
            BonusPointType::Spend => "2",
            BonusPointType::Gift => "3",
            BonusPointType::Confiscation => "4",
            BonusPointType::Expiry => "5",
        }
    }
}

/// `--expand` values of `hw report bonus-points`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BonusPointsExpand {
    /// The client
    Client,
}

impl ApiName for BonusPointsExpand {
    fn api_name(&self) -> &'static str {
        match self {
            BonusPointsExpand::Client => "client",
        }
    }
}
