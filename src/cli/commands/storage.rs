//! `hw storage` — storages and daily purchases.

use jiff::Timestamp;

use super::{Outcome, fetch};
use crate::cli::args::{PurchasesArgs, StorageCommand, join_api_names};
use crate::client::{Api, Query};
use crate::dates::DateResolver;

/// Runs `hw storage <command>`; `now` resolves relative dates.
pub fn run(command: &StorageCommand, api: &dyn Api, now: Timestamp) -> anyhow::Result<Outcome> {
    match command {
        StorageCommand::List => fetch(api, "/api/storages", &Query::new()),
        StorageCommand::Purchases(args) => purchases(args, api, now),
    }
}

fn purchases(args: &PurchasesArgs, api: &dyn Api, now: Timestamp) -> anyhow::Result<Outcome> {
    let dates = DateResolver::new(api, now);
    let mut query = Query::new();
    query
        .push("date", dates.resolve(args.date)?)
        .push_opt("storage_id", args.storage)
        .push_opt("expand", join_api_names(&args.expand, ","));
    fetch(api, "/api/purchases", &query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::PurchaseExpand;
    use crate::cli::commands::pairs;
    use crate::dates::DateArg;
    use crate::test_util::FakeApi;
    use jiff::civil::date;
    use serde_json::json;

    #[test]
    fn list_hits_storages() {
        let api = FakeApi::new().with_json("/api/storages", json!([]));
        run(&StorageCommand::List, &api, Timestamp::UNIX_EPOCH).unwrap();
        assert_eq!(api.requests(), vec![("/api/storages".to_owned(), vec![])]);
    }

    #[test]
    fn purchases_with_absolute_date_skips_settings() {
        let api = FakeApi::new().with_json("/api/purchases", json!([]));
        let args = PurchasesArgs {
            date: DateArg::On(date(2026, 9, 30)),
            storage: Some(1),
            expand: vec![PurchaseExpand::Product, PurchaseExpand::Storage],
        };
        run(
            &StorageCommand::Purchases(args),
            &api,
            Timestamp::UNIX_EPOCH,
        )
        .unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/purchases".to_owned(),
                pairs(&[
                    ("date", "2026-09-30"),
                    ("storage_id", "1"),
                    ("expand", "product,storage")
                ])
            )]
        );
    }

    #[test]
    fn purchases_resolves_relative_date_via_settings() {
        let api = FakeApi::new()
            .with_json(
                "/api/settings",
                json!({"midnight": "6", "timezone": "Asia/Yekaterinburg"}),
            )
            .with_json("/api/purchases", json!([]));
        let now: Timestamp = "2026-10-02T22:00:00Z".parse().unwrap();
        let args = PurchasesArgs {
            date: DateArg::Today,
            storage: None,
            expand: vec![],
        };
        run(&StorageCommand::Purchases(args), &api, now).unwrap();
        assert_eq!(
            api.requests(),
            vec![
                ("/api/settings".to_owned(), vec![]),
                (
                    "/api/purchases".to_owned(),
                    pairs(&[("date", "2026-10-02")])
                ),
            ]
        );
    }
}
