//! `hw report` — daily expenses, credits and bonus points (dates resolved to `YYYY-MM-DD`).

use jiff::Timestamp;

use super::{Outcome, fetch};
use crate::cli::args::{ApiName, ReportCommand, join_api_names};
use crate::client::{Api, Query};
use crate::dates::DateResolver;

/// Runs `hw report <command>`; `now` resolves relative dates.
pub fn run(command: &ReportCommand, api: &dyn Api, now: Timestamp) -> anyhow::Result<Outcome> {
    let dates = DateResolver::new(api, now);
    let mut query = Query::new();
    let path = match command {
        ReportCommand::Expenses(args) => {
            query
                .push("date", dates.resolve(args.date)?)
                .push_opt("expand", join_api_names(&args.expand, ","));
            "/api/expenses"
        }
        ReportCommand::Credits(args) => {
            query
                .push("date", dates.resolve(args.date)?)
                .push_opt("type", args.kind.map(|k| k.api_name()))
                .push_opt("expand", join_api_names(&args.expand, ","));
            "/api/credits"
        }
        ReportCommand::BonusPoints(args) => {
            query
                .push("date", dates.resolve(args.date)?)
                .push_opt("type", args.kind.map(|k| k.api_name()))
                .push_opt("expand", join_api_names(&args.expand, ","));
            "/api/bonus-points"
        }
    };
    fetch(api, path, &query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{
        BonusPointType, BonusPointsArgs, BonusPointsExpand, CreditType, CreditsArgs, CreditsExpand,
        ExpenseExpand, ExpensesArgs,
    };
    use crate::cli::commands::pairs;
    use crate::dates::DateArg;
    use crate::test_util::FakeApi;
    use jiff::civil::date;
    use serde_json::json;

    fn day() -> DateArg {
        DateArg::On(date(2021, 10, 1))
    }

    fn requested(command: &ReportCommand, path: &str) -> Vec<(String, String)> {
        let api = FakeApi::new().with_json(path, json!([]));
        run(command, &api, Timestamp::UNIX_EPOCH).unwrap();
        let requests = api.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, path);
        requests[0].1.clone()
    }

    #[test]
    fn expenses_maps_payment_method() {
        let command = ReportCommand::Expenses(ExpensesArgs {
            date: day(),
            expand: vec![ExpenseExpand::Category, ExpenseExpand::PaymentMethod],
        });
        assert_eq!(
            requested(&command, "/api/expenses"),
            pairs(&[("date", "2021-10-01"), ("expand", "category,paymentMethod")])
        );
    }

    #[test]
    fn credits_maps_type_to_number() {
        let command = ReportCommand::Credits(CreditsArgs {
            date: day(),
            kind: Some(CreditType::DepositOffCash),
            expand: vec![CreditsExpand::Client, CreditsExpand::PaymentMethod],
        });
        assert_eq!(
            requested(&command, "/api/credits"),
            pairs(&[
                ("date", "2021-10-01"),
                ("type", "4"),
                ("expand", "client,paymentMethod")
            ])
        );
    }

    #[test]
    fn bonus_points_maps_type_to_number() {
        let command = ReportCommand::BonusPoints(BonusPointsArgs {
            date: day(),
            kind: Some(BonusPointType::Confiscation),
            expand: vec![BonusPointsExpand::Client],
        });
        assert_eq!(
            requested(&command, "/api/bonus-points"),
            pairs(&[("date", "2021-10-01"), ("type", "4"), ("expand", "client")])
        );
    }
}
