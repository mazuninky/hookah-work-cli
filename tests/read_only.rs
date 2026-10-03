//! The core product promise as a test: every command, run end-to-end through `app::run` with
//! the production HTTP connector, sends only GET requests — plus exactly `POST /api/login`
//! and `POST /api/client/find`. A new command fails `every_command_is_covered` until it is
//! added to the table below.

mod common;

use std::collections::BTreeSet;

use clap::CommandFactory as _;
use pretty_assertions::assert_eq;

use common::{App, FakeCrm, Seen};
use hw::auth::MockResponse;
use hw::cli::Cli;

/// The only non-GET requests `hw` may ever send.
const ALLOWED_POSTS: [&str; 2] = ["POST /api/login", "POST /api/client/find"];

struct Case {
    /// Leaf command path, e.g. `"client find"`.
    command: &'static str,
    /// Arguments after the command path; `{dir}` is replaced by a tempdir.
    args: &'static [&'static str],
    stdin: &'static str,
    /// Expected `METHOD /path` lines in order; empty for offline commands.
    requests: &'static [&'static str],
}

const fn case(
    command: &'static str,
    args: &'static [&'static str],
    requests: &'static [&'static str],
) -> Case {
    Case {
        command,
        args,
        stdin: "",
        requests,
    }
}

const SETTINGS: &str = "GET /api/settings";

const CASES: &[Case] = &[
    case("ref settings", &[], &[SETTINGS]),
    case("ref tables", &[], &["GET /api/tables"]),
    case("ref payment-methods", &[], &["GET /api/payment-methods"]),
    case("ref employees", &[], &["GET /api/employees"]),
    case("ref services", &[], &["GET /api/services"]),
    case("ref bonus-levels", &[], &["GET /api/bonus-levels"]),
    case(
        "product list",
        &["--type", "dish", "--expand", "components", "--all"],
        &["GET /api/products", "GET /api/products"],
    ),
    case("product categories", &[], &["GET /api/product/categories"]),
    case("storage list", &[], &["GET /api/storages"]),
    case(
        "storage purchases",
        &[
            "--date",
            "today",
            "--storage",
            "2",
            "--expand",
            "product,storage",
        ],
        &[SETTINGS, "GET /api/purchases"],
    ),
    case(
        "client list",
        &["--search", "Яшин", "--group", "1", "--page", "2"],
        &["GET /api/clients"],
    ),
    case("client groups", &[], &["GET /api/client/groups"]),
    case(
        "client history",
        &["1001", "--all"],
        &[
            "GET /api/client/history/1001",
            "GET /api/client/history/1001",
        ],
    ),
    case(
        "client find",
        &[
            "--phone",
            "9175555111",
            "--card",
            "1",
            "--expand",
            "bonus-level",
        ],
        &["POST /api/client/find"],
    ),
    case(
        "booking list",
        &["--date", "today", "--expand", "client,table,total"],
        &["GET /api/timetable/list"],
    ),
    case(
        "booking timetable",
        &["--date", "2018-10-13"],
        &["GET /api/timetable"],
    ),
    case(
        "booking view",
        &["51", "--expand", "table"],
        &["GET /api/booking/51"],
    ),
    case(
        "booking my",
        &["--device", "q1w2e3r4t5y6u7i8o9"],
        &["GET /api/booking/my"],
    ),
    case(
        "sale list",
        &[
            "--booking",
            "496,497",
            "--date",
            "yesterday",
            "--expand",
            "product",
        ],
        &[SETTINGS, "GET /api/sale"],
    ),
    case(
        "hookah list",
        &["--date", "2026-08-23", "--expand", "service"],
        &["GET /api/hookah"],
    ),
    case(
        "report expenses",
        &[
            "--date",
            "2021-10-01",
            "--expand",
            "category,payment-method",
        ],
        &["GET /api/expenses"],
    ),
    case(
        "report credits",
        &["--date", "today", "--type", "debt"],
        &[SETTINGS, "GET /api/credits"],
    ),
    case(
        "report bonus-points",
        &[
            "--date",
            "2026-08-23",
            "--type",
            "visit",
            "--expand",
            "client",
        ],
        &["GET /api/bonus-points"],
    ),
    case(
        "analytics hookah",
        &["--from", "2023-01-01", "--till", "2023-12-31"],
        &["GET /api/analytic/hookah"],
    ),
    case(
        "analytics sale",
        &["--from", "yesterday", "--till", "today"],
        &[SETTINGS, "GET /api/analytic/sale"],
    ),
    case("menu", &[], &["GET /api/menu"]),
    case(
        "api",
        &["clients?search=a", "--all"],
        &["GET /api/clients", "GET /api/clients"],
    ),
    case("api", &["/api/menu", "--no-auth"], &["GET /api/menu"]),
    Case {
        command: "auth login",
        args: &["--with-token"],
        stdin: "pasted-token\n",
        requests: &[SETTINGS],
    },
    Case {
        command: "auth login",
        args: &[
            "--email",
            "integration@example.com",
            "--password-stdin",
            "--storage",
            "config",
        ],
        stdin: "p@ss\n",
        requests: &["POST /api/login", SETTINGS],
    },
    case("auth status", &[], &[SETTINGS]),
    case("auth token", &[], &[]),
    case("auth logout", &[], &[]),
    case("config list", &[], &[]),
    case("config show", &[], &[]),
    case("config set-default", &["test"], &[]),
    case("config delete", &["test"], &[]),
    case("config path", &[], &[]),
    case("completions", &["bash"], &[]),
    // `hw self` talks to GitHub (a fake here), never to the CRM.
    case("self check", &[], &[]),
    case("self update", &[], &[]),
    case("generate-docs", &["--output-dir", "{dir}"], &[]),
];

fn is_allowed(request: &Seen) -> bool {
    request.method == "GET" || ALLOWED_POSTS.contains(&request.line().as_str())
}

#[test]
fn every_request_is_a_get_or_one_of_the_two_lookups() {
    let crm = FakeCrm::start();
    let mut methods = BTreeSet::new();
    let mut posts = BTreeSet::new();

    for case in CASES {
        let app = App::new(&crm.url());
        let dir = app.dir();
        let mut argv: Vec<&str> = case.command.split_whitespace().collect();
        argv.extend(
            case.args
                .iter()
                .map(|arg| if *arg == "{dir}" { dir.as_str() } else { arg }),
        );

        let label = format!("hw {}", argv.join(" "));
        let ran = app.run_with_stdin(&argv, case.stdin);
        let seen = crm.take();

        let forbidden: Vec<String> = seen
            .iter()
            .filter(|r| !is_allowed(r))
            .map(Seen::line)
            .collect();
        assert!(forbidden.is_empty(), "{label} sent {forbidden:?}");
        ran.assert_ok(&label);
        for request in seen.iter().filter(|r| r.method == "GET") {
            assert_eq!(request.body, "", "{label}: GET with a body");
        }
        let lines: Vec<String> = seen.iter().map(Seen::line).collect();
        assert_eq!(lines, case.requests, "{label}");

        methods.extend(seen.iter().map(|r| r.method.clone()));
        posts.extend(seen.iter().filter(|r| r.method == "POST").map(Seen::line));
    }

    assert_eq!(
        methods,
        BTreeSet::from(["GET".to_owned(), "POST".to_owned()])
    );
    assert_eq!(
        posts,
        ALLOWED_POSTS
            .iter()
            .map(|p| (*p).to_owned())
            .collect::<BTreeSet<_>>(),
        "the table must exercise both allowed POSTs"
    );
}

#[test]
fn interactive_login_sends_only_the_check_and_the_login_post() {
    let crm = FakeCrm::start();
    let api_key = vec![
        MockResponse::Select(0),
        MockResponse::Secret("pasted-key".into()),
    ];
    let password = vec![
        MockResponse::Select(1),
        MockResponse::Text("integration@example.com".into()),
        MockResponse::Secret("p@ss".into()),
    ];
    for (script, expected) in [
        (api_key, vec![SETTINGS]),
        (password, vec!["POST /api/login", SETTINGS]),
    ] {
        let app = App::new(&crm.url());
        let ran = app.run_prompted(&["auth", "login", "--storage", "config"], script);
        ran.assert_ok("interactive auth login");
        let lines: Vec<String> = crm.take().iter().map(Seen::line).collect();
        assert_eq!(lines, expected);
    }
}

fn leaf_commands(cmd: &clap::Command, prefix: &str, out: &mut BTreeSet<String>) {
    for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
        let path = format!("{prefix}{}", sub.get_name());
        if sub.has_subcommands() {
            leaf_commands(sub, &format!("{path} "), out);
        } else {
            out.insert(path);
        }
    }
}

#[test]
fn every_command_is_covered() {
    let mut all = BTreeSet::new();
    leaf_commands(&Cli::command(), "", &mut all);
    let covered: BTreeSet<String> = CASES.iter().map(|c| c.command.to_owned()).collect();

    assert_eq!(
        covered, all,
        "add every new command to CASES in tests/read_only.rs"
    );
}
