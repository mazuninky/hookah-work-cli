//! `hw analytics` — period summaries (both ends resolved to `YYYY-MM-DD`).

use jiff::Timestamp;

use super::{Outcome, fetch};
use crate::cli::args::{AnalyticsCommand, PeriodArgs};
use crate::client::{Api, Query};
use crate::dates::DateResolver;

/// Runs `hw analytics <command>`; `now` resolves relative dates.
pub fn run(command: &AnalyticsCommand, api: &dyn Api, now: Timestamp) -> anyhow::Result<Outcome> {
    let (path, period) = match command {
        AnalyticsCommand::Hookah(period) => ("/api/analytic/hookah", period),
        AnalyticsCommand::Sale(period) => ("/api/analytic/sale", period),
    };
    fetch(api, path, &period_query(period, api, now)?)
}

fn period_query(period: &PeriodArgs, api: &dyn Api, now: Timestamp) -> anyhow::Result<Query> {
    let dates = DateResolver::new(api, now);
    let mut query = Query::new();
    query
        .push("from", dates.resolve(period.from)?)
        .push("till", dates.resolve(period.till)?);
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::pairs;
    use crate::dates::DateArg;
    use crate::test_util::FakeApi;
    use jiff::civil::date;
    use serde_json::json;

    #[test]
    fn hookah_period() {
        let api = FakeApi::new().with_json("/api/analytic/hookah", json!([]));
        let period = PeriodArgs {
            from: DateArg::On(date(2023, 1, 1)),
            till: DateArg::On(date(2023, 12, 31)),
        };
        run(
            &AnalyticsCommand::Hookah(period),
            &api,
            Timestamp::UNIX_EPOCH,
        )
        .unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/analytic/hookah".to_owned(),
                pairs(&[("from", "2023-01-01"), ("till", "2023-12-31")])
            )]
        );
    }

    #[test]
    fn sale_period_fetches_settings_once_for_two_relative_ends() {
        let api = FakeApi::new()
            .with_json(
                "/api/settings",
                json!({"midnight": "6", "timezone": "Asia/Yekaterinburg"}),
            )
            .with_json("/api/analytic/sale", json!([]));
        let now: Timestamp = "2026-10-03T02:00:00Z".parse().unwrap();
        let period = PeriodArgs {
            from: DateArg::Yesterday,
            till: DateArg::Today,
        };
        run(&AnalyticsCommand::Sale(period), &api, now).unwrap();
        assert_eq!(
            api.requests(),
            vec![
                ("/api/settings".to_owned(), vec![]),
                (
                    "/api/analytic/sale".to_owned(),
                    pairs(&[("from", "2026-10-02"), ("till", "2026-10-03")])
                ),
            ]
        );
    }
}
