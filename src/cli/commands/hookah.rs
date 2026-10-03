//! `hw hookah` — hookahs of bookings and/or a business day.

use jiff::Timestamp;

use super::sale::booking_day_query;
use super::{Outcome, fetch};
use crate::cli::args::{HookahCommand, join_api_names};
use crate::client::Api;

/// Runs `hw hookah <command>`; `now` resolves relative dates.
pub fn run(command: &HookahCommand, api: &dyn Api, now: Timestamp) -> anyhow::Result<Outcome> {
    let HookahCommand::List(args) = command;
    let query = booking_day_query(
        api,
        now,
        &args.booking,
        args.date,
        join_api_names(&args.expand, ","),
    )?;
    fetch(api, "/api/hookah", &query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{HookahExpand, HookahListArgs};
    use crate::cli::commands::pairs;
    use crate::test_util::FakeApi;
    use serde_json::json;

    #[test]
    fn single_booking_with_expand() {
        let api = FakeApi::new().with_json(
            "/api/hookah",
            json!([{"id": 214, "booking_id": 496, "service_id": 1, "price": 700}]),
        );
        let args = HookahListArgs {
            booking: vec![496],
            date: None,
            expand: vec![HookahExpand::Service, HookahExpand::Booking],
        };
        run(&HookahCommand::List(args), &api, Timestamp::UNIX_EPOCH).unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/hookah".to_owned(),
                pairs(&[("booking_id", "496"), ("expand", "service,booking")])
            )]
        );
    }
}
