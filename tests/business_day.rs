//! Relative dates end-to-end: `app::run` with the production HTTP client, an injected clock
//! and httpmock. Reports resolve `today`/`yesterday`/`tomorrow` from `GET /api/settings`;
//! booking lists pass them through for the CRM to resolve.

mod common;

use httpmock::Mock;
use httpmock::prelude::*;
use pretty_assertions::assert_eq;

use common::{App, NOW, fixtures};
use hw::error::exit_code;

fn settings<'a>(server: &'a MockServer, body: &str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(GET).path("/api/settings");
        then.status(200).body(body);
    })
}

#[test]
fn relative_report_dates_follow_the_crm_business_day() {
    let auckland = r#"{"midnight": "0", "timezone": "Pacific/Auckland"}"#;
    // (settings, now, --date, the date the API must receive)
    let cases = [
        // 03:00 local, before the documented 06:00 start: still the previous business day.
        (fixtures::SETTINGS, NOW, "today", "2026-10-02"),
        (fixtures::SETTINGS, NOW, "yesterday", "2026-10-01"),
        // 07:00 local, after the start.
        (
            fixtures::SETTINGS,
            "2026-10-03T02:00:00Z",
            "today",
            "2026-10-03",
        ),
        // 01:30 on 10-04 in Auckland while UTC is still on 10-03.
        (auckland, "2026-10-03T12:30:00Z", "today", "2026-10-04"),
    ];

    for (settings_body, now, arg, expected) in cases {
        let label = format!("--date {arg} at {now} with {settings_body}");
        let server = MockServer::start();
        let settings = settings(&server, settings_body);
        let expenses = server.mock(|when, then| {
            when.method(GET)
                .path("/api/expenses")
                .query_param("date", expected)
                .query_param("expand", "category,paymentMethod");
            then.status(200).body(fixtures::EXPENSES);
        });

        let ran = App::new(&server.base_url()).at(now).run(&[
            "-F",
            "json",
            "report",
            "expenses",
            "--date",
            arg,
            "--expand",
            "category,payment-method",
        ]);

        ran.assert_ok(&label);
        assert_eq!(settings.calls(), 1, "{label}");
        assert_eq!(expenses.calls(), 1, "{label}");
        assert_eq!(ran.json(), fixtures::json(fixtures::EXPENSES), "{label}");
    }
}

#[test]
fn a_period_resolves_both_ends_with_one_settings_request() {
    let server = MockServer::start();
    let settings = settings(&server, fixtures::SETTINGS);
    let analytics = server.mock(|when, then| {
        when.method(GET)
            .path("/api/analytic/hookah")
            .query_param("from", "2026-10-01")
            .query_param("till", "2026-10-02");
        then.status(200).body("[]");
    });

    let ran = App::new(&server.base_url()).run(&[
        "analytics",
        "hookah",
        "--from",
        "yesterday",
        "--till",
        "today",
    ]);

    ran.assert_ok("analytics hookah");
    settings.assert_calls(1);
    analytics.assert_calls(1);
}

#[test]
fn booking_lists_send_relative_dates_literally() {
    let server = MockServer::start();
    let settings = settings(&server, fixtures::SETTINGS);
    let list = server.mock(|when, then| {
        when.method(GET)
            .path("/api/timetable/list")
            .query_param("date", "tomorrow");
        then.status(200).body("[]");
    });
    let timetable = server.mock(|when, then| {
        when.method(GET)
            .path("/api/timetable")
            .query_param("date", "yesterday");
        then.status(200).body("{}");
    });
    let app = App::new(&server.base_url());

    app.run(&["booking", "list", "--date", "tomorrow"])
        .assert_ok("booking list");
    app.run(&["booking", "timetable", "--date", "yesterday"])
        .assert_ok("booking timetable");

    list.assert_calls(1);
    timetable.assert_calls(1);
    settings.assert_calls(0);
}

#[test]
fn calendar_dates_never_fetch_settings() {
    let server = MockServer::start();
    let settings = settings(&server, fixtures::SETTINGS);
    let credits = server.mock(|when, then| {
        when.method(GET)
            .path("/api/credits")
            .query_param("date", "2019-03-05")
            .query_param("type", "2");
        then.status(200).body("[]");
    });

    App::new(&server.base_url())
        .run(&[
            "report",
            "credits",
            "--date",
            "2019-03-05",
            "--type",
            "return",
        ])
        .assert_ok("report credits");

    credits.assert_calls(1);
    settings.assert_calls(0);
}

#[test]
fn a_rejected_settings_request_stops_before_the_report() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/settings");
        then.status(403).body(fixtures::FORBIDDEN);
    });
    let expenses = server.mock(|when, then| {
        when.method(GET).path("/api/expenses");
        then.status(200).body("[]");
    });

    let ran = App::new(&server.base_url()).run(&["report", "expenses", "--date", "today"]);

    assert_eq!(ran.exit_code(), exit_code::AUTH_ERROR);
    assert_eq!(ran.stdout, "");
    expenses.assert_calls(0);
}
