//! `hw sale` — sales of bookings and/or a business day.

use jiff::Timestamp;

use super::{Outcome, fetch};
use crate::cli::args::{SaleCommand, join_api_names};
use crate::client::{Api, Query};
use crate::dates::{DateArg, DateResolver};

/// Runs `hw sale <command>`; `now` resolves relative dates.
pub fn run(command: &SaleCommand, api: &dyn Api, now: Timestamp) -> anyhow::Result<Outcome> {
    let SaleCommand::List(args) = command;
    let query = booking_day_query(
        api,
        now,
        &args.booking,
        args.date,
        join_api_names(&args.expand, ","),
    )?;
    fetch(api, "/api/sale", &query)
}

/// `booking_id` (ids joined with `-`, an enumeration rather than a range), `date` and `expand`,
/// shared by the sale and hookah lists.
pub(crate) fn booking_day_query(
    api: &dyn Api,
    now: Timestamp,
    bookings: &[u64],
    date: Option<DateArg>,
    expand: Option<String>,
) -> anyhow::Result<Query> {
    let mut query = Query::new();
    if !bookings.is_empty() {
        let ids: Vec<String> = bookings.iter().map(u64::to_string).collect();
        query.push("booking_id", ids.join("-"));
    }
    if let Some(date) = date {
        query.push("date", DateResolver::new(api, now).resolve(date)?);
    }
    query.push_opt("expand", expand);
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{SaleExpand, SaleListArgs};
    use crate::cli::commands::pairs;
    use crate::test_util::FakeApi;
    use jiff::civil::date;
    use serde_json::json;

    #[test]
    fn bookings_are_joined_with_dash() {
        let api = FakeApi::new().with_json("/api/sale", json!([]));
        let args = SaleListArgs {
            booking: vec![495, 496],
            date: Some(DateArg::On(date(2026, 8, 23))),
            expand: vec![SaleExpand::Product, SaleExpand::Booking, SaleExpand::Seller],
        };
        run(&SaleCommand::List(args), &api, Timestamp::UNIX_EPOCH).unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/sale".to_owned(),
                pairs(&[
                    ("booking_id", "495-496"),
                    ("date", "2026-08-23"),
                    ("expand", "product,booking,seller")
                ])
            )]
        );
    }

    #[test]
    fn relative_date_is_resolved() {
        let api = FakeApi::new()
            .with_json(
                "/api/settings",
                json!({"midnight": "6", "timezone": "Asia/Yekaterinburg"}),
            )
            .with_json("/api/sale", json!([]));
        let now: Timestamp = "2026-10-03T02:00:00Z".parse().unwrap();
        let args = SaleListArgs {
            booking: vec![],
            date: Some(DateArg::Yesterday),
            expand: vec![],
        };
        run(&SaleCommand::List(args), &api, now).unwrap();
        assert_eq!(
            api.requests()[1],
            ("/api/sale".to_owned(), pairs(&[("date", "2026-10-02")]))
        );
    }
}
