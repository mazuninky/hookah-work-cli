//! `hw booking` — daily lists, timetable, single bookings and device bookings.
//!
//! Relative dates are sent literally: these endpoints apply the CRM's night shift themselves.

use super::{Outcome, fetch};
use crate::cli::args::{BookingCommand, join_api_names};
use crate::client::{Api, Query};

/// Runs `hw booking <command>`.
pub fn run(command: &BookingCommand, api: &dyn Api) -> anyhow::Result<Outcome> {
    let mut query = Query::new();
    match command {
        BookingCommand::List(args) => {
            query
                .push_opt("date", args.date.map(|d| d.literal()))
                .push_opt("expand", join_api_names(&args.expand, ","));
            fetch(api, "/api/timetable/list", &query)
        }
        BookingCommand::Timetable(args) => {
            query.push_opt("date", args.date.map(|d| d.literal()));
            fetch(api, "/api/timetable", &query)
        }
        BookingCommand::View(args) => {
            query.push_opt("expand", join_api_names(&args.expand, ","));
            fetch(api, &format!("/api/booking/{}", args.id), &query)
        }
        BookingCommand::My(args) => {
            query.push("device", &args.device);
            fetch(api, "/api/booking/my", &query)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{
        BookingExpand, BookingListArgs, BookingMyArgs, BookingViewArgs, TimetableArgs,
    };
    use crate::cli::commands::pairs;
    use crate::dates::DateArg;
    use crate::test_util::FakeApi;
    use jiff::civil::date;
    use serde_json::json;

    #[test]
    fn list_sends_relative_date_literally_and_maps_expand() {
        let api = FakeApi::new().with_json("/api/timetable/list", json!([]));
        let args = BookingListArgs {
            date: Some(DateArg::Tomorrow),
            expand: vec![
                BookingExpand::Client,
                BookingExpand::Table,
                BookingExpand::BonusOperations,
            ],
        };
        run(&BookingCommand::List(args), &api).unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/timetable/list".to_owned(),
                pairs(&[
                    ("date", "tomorrow"),
                    ("expand", "client,table,bonusOperations")
                ])
            )]
        );
    }

    #[test]
    fn list_without_date_omits_it() {
        let api = FakeApi::new().with_json("/api/timetable/list", json!([]));
        let args = BookingListArgs {
            date: None,
            expand: vec![],
        };
        run(&BookingCommand::List(args), &api).unwrap();
        assert_eq!(
            api.requests(),
            vec![("/api/timetable/list".to_owned(), vec![])]
        );
    }

    #[test]
    fn timetable_keeps_table_keyed_object() {
        let grouped = json!({"13": {"707": {"id": 707}}, "1": {"708": {"id": 708}}});
        let api = FakeApi::new().with_json("/api/timetable", grouped.clone());
        let args = TimetableArgs {
            date: Some(DateArg::On(date(2018, 10, 13))),
        };
        let outcome = run(&BookingCommand::Timetable(args), &api).unwrap();
        assert_eq!(outcome.json_value(), Some(&grouped));
        assert_eq!(
            api.requests(),
            vec![(
                "/api/timetable".to_owned(),
                pairs(&[("date", "2018-10-13")])
            )]
        );
    }

    #[test]
    fn view_and_my_paths() {
        let api = FakeApi::new()
            .with_json("/api/booking/707", json!({"id": 707}))
            .with_json("/api/booking/my", json!([]));
        let view = BookingViewArgs {
            id: 707,
            expand: vec![BookingExpand::Total],
        };
        run(&BookingCommand::View(view), &api).unwrap();
        let my = BookingMyArgs {
            device: "doc-test-device-2026".into(),
        };
        run(&BookingCommand::My(my), &api).unwrap();
        assert_eq!(
            api.requests(),
            vec![
                ("/api/booking/707".to_owned(), pairs(&[("expand", "total")])),
                (
                    "/api/booking/my".to_owned(),
                    pairs(&[("device", "doc-test-device-2026")])
                ),
            ]
        );
    }
}
