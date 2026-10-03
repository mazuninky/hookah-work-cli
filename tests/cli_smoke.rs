//! One happy path per command group through the `hw` binary against httpmock: the request
//! reaches the documented endpoint with the documented parameters and the result is printed.

mod common;

use httpmock::Mock;
use httpmock::prelude::*;
use predicates::prelude::*;
use pretty_assertions::assert_eq;

use common::{TOKEN, TestEnv, fixtures};

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

/// Runs `hw <args>` against `server`, asserts success with a clean stderr and returns stdout.
fn run_ok(server: &MockServer, args: &[&str]) -> String {
    let env = TestEnv::new();
    let output = env
        .api_cmd(&server.base_url())
        .args(args)
        .output()
        .expect("run hw");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "hw {args:?}: {stderr}");
    assert_eq!(stderr, "", "hw {args:?}");
    String::from_utf8(output.stdout).expect("UTF-8 stdout")
}

fn json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

fn get<'a>(server: &'a MockServer, path: &str, body: &str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path(path)
            .header("authorization", bearer());
        then.status(200).body(body);
    })
}

#[test]
fn ref_prints_reference_data() {
    let server = MockServer::start();
    let tables = get(&server, "/api/tables", fixtures::TABLES);

    let out = run_ok(&server, &["-F", "json", "ref", "tables"]);

    tables.assert();
    assert_eq!(json(&out), fixtures::json(fixtures::TABLES));
}

#[test]
fn product_list_maps_filters_to_query_params() {
    let server = MockServer::start();
    let products = server.mock(|when, then| {
        when.method(GET)
            .path("/api/products")
            .header("authorization", bearer())
            .query_param("category_id", "24")
            .query_param("type", "dish")
            .query_param("search", "чай")
            .query_param("expand", "components")
            .query_param("page", "1")
            .query_param("per-page", "20");
        then.status(200).body(fixtures::PRODUCTS);
    });

    let out = run_ok(
        &server,
        &[
            "-F",
            "json",
            "product",
            "list",
            "--category",
            "24",
            "--type",
            "dish",
            "--search",
            "чай",
            "--expand",
            "components",
            "--page",
            "1",
            "--per-page",
            "20",
        ],
    );

    products.assert();
    assert_eq!(json(&out), fixtures::json(fixtures::PRODUCTS));
}

#[test]
fn storage_purchases_send_date_storage_and_expand() {
    let server = MockServer::start();
    let purchases = server.mock(|when, then| {
        when.method(GET)
            .path("/api/purchases")
            .header("authorization", bearer())
            .query_param("date", "2026-09-30")
            .query_param("storage_id", "2")
            .query_param("expand", "product,storage");
        then.status(200).body(r#"[{"id": 52, "storage_id": 2}]"#);
    });

    let out = run_ok(
        &server,
        &[
            "-F",
            "json",
            "storage",
            "purchases",
            "--date",
            "2026-09-30",
            "--storage",
            "2",
            "--expand",
            "product,storage",
        ],
    );

    purchases.assert();
    assert_eq!(json(&out)[0]["id"], 52);
}

#[test]
fn client_find_posts_the_lookup_form() {
    let server = MockServer::start();
    let find = server.mock(|when, then| {
        when.method(POST)
            .path("/api/client/find")
            .header("authorization", bearer())
            .query_param("expand", "bonusLevel")
            .form_urlencoded_tuple("phone", "9175555111");
        then.status(200).body(fixtures::CLIENT_FIND);
    });

    let out = run_ok(
        &server,
        &[
            "-F",
            "json",
            "client",
            "find",
            "--phone",
            "9175555111",
            "--expand",
            "bonus-level",
        ],
    );

    find.assert();
    assert_eq!(json(&out), fixtures::json(fixtures::CLIENT_FIND));
}

#[test]
fn booking_view_prints_one_booking() {
    let server = MockServer::start();
    let view = server.mock(|when, then| {
        when.method(GET)
            .path("/api/booking/51")
            .header("authorization", bearer())
            .query_param("expand", "table");
        then.status(200)
            .body(r#"{"id": 51, "table": {"id": 2, "name": "Стол 2"}}"#);
    });

    let out = run_ok(&server, &["booking", "view", "51", "--expand", "table"]);

    view.assert();
    assert!(out.contains("id: 51"), "{out}");
}

#[test]
fn sale_list_joins_booking_ids_with_dashes() {
    let server = MockServer::start();
    let sales = server.mock(|when, then| {
        when.method(GET)
            .path("/api/sale")
            .header("authorization", bearer())
            .query_param("booking_id", "496-497")
            .query_param("expand", "product");
        then.status(200).body(r#"[{"id": 196, "booking_id": 496}]"#);
    });

    let out = run_ok(
        &server,
        &[
            "-F",
            "json",
            "sale",
            "list",
            "--booking",
            "496,497",
            "--expand",
            "product",
        ],
    );

    sales.assert();
    assert_eq!(json(&out)[0]["id"], 196);
}

#[test]
fn hookah_list_filters_by_booking() {
    let server = MockServer::start();
    let hookahs = server.mock(|when, then| {
        when.method(GET)
            .path("/api/hookah")
            .header("authorization", bearer())
            .query_param("booking_id", "496")
            .query_param("expand", "service");
        then.status(200).body(r#"[{"id": 215}]"#);
    });

    run_ok(
        &server,
        &["hookah", "list", "--booking", "496", "--expand", "service"],
    );

    hookahs.assert();
}

#[test]
fn report_bonus_points_maps_the_type_to_its_number() {
    let server = MockServer::start();
    let report = server.mock(|when, then| {
        when.method(GET)
            .path("/api/bonus-points")
            .header("authorization", bearer())
            .query_param("date", "2026-08-23")
            .query_param("type", "1")
            .query_param("expand", "client");
        then.status(200).body("[]");
    });

    run_ok(
        &server,
        &[
            "report",
            "bonus-points",
            "--date",
            "2026-08-23",
            "--type",
            "visit",
            "--expand",
            "client",
        ],
    );

    report.assert();
}

#[test]
fn analytics_sale_sends_the_period() {
    let server = MockServer::start();
    let analytics = server.mock(|when, then| {
        when.method(GET)
            .path("/api/analytic/sale")
            .header("authorization", bearer())
            .query_param("from", "2026-09-01")
            .query_param("till", "2026-09-16");
        then.status(200)
            .body(r#"[{"product_id": 172, "count": 3}]"#);
    });

    let out = run_ok(
        &server,
        &[
            "-F",
            "csv",
            "analytics",
            "sale",
            "--from",
            "2026-09-01",
            "--till",
            "2026-09-16",
        ],
    );

    analytics.assert();
    assert_eq!(out, "product_id,count\n172,3\n");
}

#[test]
fn menu_is_fetched_without_a_token_even_when_one_is_set() {
    let server = MockServer::start();
    let menu = server.mock(|when, then| {
        when.method(GET)
            .path("/api/menu")
            .header_missing("authorization");
        then.status(200)
            .body(r#"[{"id": 1, "name": "Классический чай"}]"#);
    });

    let out = run_ok(&server, &["-F", "json", "--jq", ".[0].name", "menu"]);

    menu.assert();
    assert_eq!(out, "\"Классический чай\"\n");
}

#[test]
fn api_passes_through_get_requests_and_raw_text() {
    let server = MockServer::start();
    let products = server.mock(|when, then| {
        when.method(GET)
            .path("/api/products")
            .header("authorization", bearer())
            .query_param("type", "dish")
            .query_param("expand", "components");
        then.status(200).body("[]");
    });
    let robots = get(&server, "/api/robots", "User-agent: *");

    run_ok(
        &server,
        &["api", "products?type=dish", "--query", "expand=components"],
    );
    let text = run_ok(&server, &["-F", "json", "api", "/api/robots"]);

    products.assert();
    robots.assert();
    assert_eq!(text, "User-agent: *\n");
}

#[test]
fn completions_print_a_shell_script() {
    TestEnv::new()
        .cmd()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_hw()"));
}
