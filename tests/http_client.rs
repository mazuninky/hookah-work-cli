//! Component tests of the real `HookahClient` against httpmock: headers, query and form
//! encoding, login, Yii error mapping, pagination headers, retries and secret hygiene.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use httpmock::prelude::*;
use httpmock::{HttpMockRequest, HttpMockResponse, Mock};
use pretty_assertions::assert_eq;
use serde_json::json;

use common::fixtures;
use hw::auth::Secret;
use hw::client::{
    Api, ApiResponse, HookahClient, HttpConnector, MESSAGE_LIMIT, Pagination, ResponseBody,
    RetryPolicy, TokenIssuer,
};
use hw::error::Error;
use hw::test_util::RecordingSleeper;

const SECRET_TOKEN: &str = "tok-SECRET-42";

struct Harness {
    client: HookahClient,
    sleeper: Arc<RecordingSleeper>,
}

fn client(base_url: &str, token: Option<&str>, retries: u32) -> Harness {
    let sleeper = Arc::new(RecordingSleeper::new());
    let policy = RetryPolicy::with_sleeper(retries, sleeper.clone());
    Harness {
        client: HookahClient::new(base_url, token.map(Secret::new), policy),
        sleeper,
    }
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn json_response(status: u16, body: &str) -> HttpMockResponse {
    HttpMockResponse::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(body.to_owned())
        .build()
}

/// A mock that answers `replies` in order and then repeats the last one.
fn sequence<'a>(
    server: &'a MockServer,
    method: Method,
    path: &str,
    replies: Vec<HttpMockResponse>,
) -> Mock<'a> {
    let next = AtomicUsize::new(0);
    server.mock(|when, then| {
        when.method(method).path(path);
        then.respond_with(move |_: &HttpMockRequest| {
            let i = next.fetch_add(1, Ordering::SeqCst).min(replies.len() - 1);
            replies[i].clone()
        });
    })
}

fn body_json(response: ApiResponse) -> serde_json::Value {
    match response.body {
        ResponseBody::Json(value) => value,
        ResponseBody::Text(text) => panic!("expected JSON, got text: {text}"),
    }
}

/// Display and Debug of an error: everything a user or a log line could show.
fn renderings(err: &Error) -> String {
    format!("{err} | {err:?}")
}

// --- Request shape -------------------------------------------------------------------------

#[test]
fn get_sends_bearer_user_agent_and_accept() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/tables")
            .header("authorization", format!("Bearer {SECRET_TOKEN}"))
            .header("user-agent", format!("hw/{}", env!("CARGO_PKG_VERSION")))
            .header("accept", "application/json");
        then.status(200).body(fixtures::TABLES);
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    let response = h.client.get("/api/tables", &[]).expect("tables");

    mock.assert();
    assert_eq!(body_json(response), fixtures::json(fixtures::TABLES));
}

#[test]
fn get_without_token_sends_no_authorization_header() {
    let server = MockServer::start();
    let anonymous = server.mock(|when, then| {
        when.method(GET)
            .path("/api/menu")
            .header_missing("authorization");
        then.status(200).body("[]");
    });
    let h = client(&server.base_url(), None, 0);

    h.client
        .get("/api/menu", &[])
        .expect("menu without a token");

    anonymous.assert();
}

#[test]
fn query_pairs_reach_the_wire_percent_encoded() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/products")
            .query_param("category_id", "24")
            .query_param("search", "чай & кофе=1")
            .query_param("per-page", "20")
            .query_param("expand", "components");
        then.status(200).body("[]");
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    let query = pairs(&[
        ("category_id", "24"),
        ("search", "чай & кофе=1"),
        ("per-page", "20"),
        ("expand", "components"),
    ]);
    h.client.get("/api/products", &query).expect("products");

    mock.assert();
}

#[test]
fn find_client_posts_a_form_body_with_expand_in_the_query() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/client/find")
            .header("authorization", format!("Bearer {SECRET_TOKEN}"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .query_param("expand", "bonusLevel,newBookings")
            .query_param_missing("phone")
            .form_urlencoded_tuple("phone", "+7 (917) 555-51-11")
            .form_urlencoded_tuple("card", "1");
        then.status(200).body(fixtures::CLIENT_FIND);
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    let response = h
        .client
        .find_client(
            &pairs(&[("card", "1"), ("phone", "+7 (917) 555-51-11")]),
            &pairs(&[("expand", "bonusLevel,newBookings")]),
        )
        .expect("client found");

    mock.assert();
    assert_eq!(body_json(response)["name"], "Тимофей Яшин");
}

#[test]
fn login_uses_basic_auth_and_returns_the_issued_token() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/login")
            // base64("integration@example.com:p@ss:wörd"), computed independently.
            .header(
                "authorization",
                "Basic aW50ZWdyYXRpb25AZXhhbXBsZS5jb206cEBzczp3w7ZyZA==",
            )
            .header("accept", "application/json");
        then.status(200).body(fixtures::LOGIN);
    });
    let connector = HttpConnector::new(RetryPolicy::with_sleeper(
        0,
        Arc::new(RecordingSleeper::new()),
    ));

    let login = connector
        .login(
            &server.base_url(),
            "integration@example.com",
            &Secret::new("p@ss:wörd"),
        )
        .expect("login");

    mock.assert();
    assert_eq!(login.token.expose(), "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
    assert_eq!(
        (login.email.as_str(), login.name.as_str()),
        ("integration@example.com", "Интеграция")
    );
}

#[test]
fn login_rejection_is_an_auth_error_without_the_password() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/login");
        then.status(401).body(fixtures::UNAUTHORIZED);
    });
    let connector = HttpConnector::new(RetryPolicy::with_sleeper(
        0,
        Arc::new(RecordingSleeper::new()),
    ));

    let err = connector
        .login(&server.base_url(), "a@b.c", &Secret::new("hunter2-pass"))
        .unwrap_err();

    assert!(
        matches!(&err, Error::Auth(m) if m == "Your request was made with invalid credentials."),
        "{err:?}"
    );
    assert!(!renderings(&err).contains("hunter2-pass"), "{err:?}");
}

// --- Response handling ---------------------------------------------------------------------

#[test]
fn yii_errors_map_to_error_kinds_with_the_server_message() {
    let html = format!("<html>{}</html>", "x".repeat(300));
    let truncated: String = html.chars().take(200).chain(['…']).collect();
    let cases: [(u16, &str, &str, String); 7] = [
        (
            401,
            fixtures::UNAUTHORIZED,
            "auth",
            "Your request was made with invalid credentials.".into(),
        ),
        (403, fixtures::FORBIDDEN, "auth", "Нет доступа".into()),
        (
            404,
            fixtures::CLIENT_NOT_FOUND,
            "not-found",
            "Клиент не найден".into(),
        ),
        (
            400,
            fixtures::EXPENSES_BAD_REQUEST,
            "invalid-input",
            "Параметр date обязателен (YYYY-MM-DD)".into(),
        ),
        (
            422,
            fixtures::VALIDATION_FAILED,
            "invalid-input",
            "name: Введите название".into(),
        ),
        (
            500,
            r#"{"name":"Internal Server Error","message":"An internal server error occurred.","code":0,"status":500}"#,
            "api 500",
            "An internal server error occurred.".into(),
        ),
        (502, &html, "api 502", truncated),
    ];
    let server = MockServer::start();
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    for (status, body, kind, message) in cases {
        let path = format!("/api/status/{status}");
        server.mock(|when, then| {
            when.method(GET).path(&path);
            then.status(status).body(body);
        });

        let err = h.client.get(&path, &[]).unwrap_err();

        let actual = match &err {
            Error::Auth(m) => ("auth".to_owned(), m.clone()),
            Error::NotFound(m) => ("not-found".to_owned(), m.clone()),
            Error::InvalidInput(m) => ("invalid-input".to_owned(), m.clone()),
            Error::Api { status, message } => (format!("api {status}"), message.clone()),
            other => (format!("{other:?}"), String::new()),
        };
        assert_eq!(actual, (kind.to_owned(), message), "HTTP {status}");
        assert!(!renderings(&err).contains(SECRET_TOKEN), "HTTP {status}");
    }
}

#[test]
fn pagination_headers_are_parsed_and_absent_ones_are_none() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/clients");
        then.status(200)
            .header("X-Pagination-Total-Count", "432")
            .header("X-Pagination-Page-Count", "5")
            .header("X-Pagination-Current-Page", "1")
            .header("X-Pagination-Per-Page", "100")
            .body("[]");
    });
    server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(200).body("[]");
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    let paged = h.client.get("/api/clients", &[]).expect("clients");
    let plain = h.client.get("/api/tables", &[]).expect("tables");

    assert_eq!(
        paged.pagination,
        Some(Pagination {
            total_count: 432,
            page_count: 5,
            current_page: 1,
            per_page: 100,
        })
    );
    assert_eq!(plain.pagination, None);
}

#[test]
fn non_json_success_is_text_and_empty_success_is_null() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/robots");
        then.status(200).body("User-agent: *");
    });
    server.mock(|when, then| {
        when.method(GET).path("/api/empty");
        then.status(204);
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    let text = h.client.get("/api/robots", &[]).expect("robots");
    let empty = h.client.get("/api/empty", &[]).expect("empty");

    assert_eq!(text.body, ResponseBody::Text("User-agent: *".into()));
    assert_eq!(empty.body, ResponseBody::Json(serde_json::Value::Null));
}

// --- Retries -------------------------------------------------------------------------------

#[test]
fn rate_limit_waits_for_the_reset_header_then_succeeds() {
    let server = MockServer::start();
    let mock = sequence(
        &server,
        GET,
        "/api/tables",
        vec![
            HttpMockResponse::builder()
                .status(429)
                .header("X-Rate-Limit-Reset", "7")
                .body("Too Many Requests")
                .build(),
            json_response(200, "[]"),
        ],
    );
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 3);

    let response = h.client.get("/api/tables", &[]).expect("retried");

    assert_eq!(body_json(response), json!([]));
    mock.assert_calls(2);
    assert_eq!(h.sleeper.slept(), vec![Duration::from_secs(7)]);
}

#[test]
fn server_errors_back_off_exponentially_and_give_up_after_the_budget() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(503).body("unavailable");
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 3);

    let err = h.client.get("/api/tables", &[]).unwrap_err();

    assert!(
        matches!(&err, Error::Api { status: 503, message } if message == "unavailable"),
        "{err:?}"
    );
    mock.assert_calls(4);
    assert_eq!(
        h.sleeper.slept(),
        vec![
            Duration::from_millis(200),
            Duration::from_millis(400),
            Duration::from_millis(800),
        ]
    );
}

#[test]
fn zero_retries_means_exactly_one_attempt() {
    let server = MockServer::start();
    let unavailable = server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(503);
    });
    let limited = server.mock(|when, then| {
        when.method(GET).path("/api/employees");
        then.status(429).header("X-Rate-Limit-Reset", "1");
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    let err_503 = h.client.get("/api/tables", &[]).unwrap_err();
    let err_429 = h.client.get("/api/employees", &[]).unwrap_err();

    assert!(
        matches!(err_503, Error::Api { status: 503, .. }),
        "{err_503:?}"
    );
    assert!(
        matches!(err_429, Error::Api { status: 429, .. }),
        "{err_429:?}"
    );
    unavailable.assert_calls(1);
    limited.assert_calls(1);
    assert_eq!(h.sleeper.slept(), Vec::<Duration>::new());
}

#[test]
fn client_errors_are_never_retried() {
    let server = MockServer::start();
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 3);

    for status in [400_u16, 401, 403, 404, 422] {
        let path = format!("/api/status/{status}");
        let mock = server.mock(|when, then| {
            when.method(GET).path(&path);
            then.status(status).body("{}");
        });

        h.client.get(&path, &[]).unwrap_err();

        assert_eq!(mock.calls(), 1, "HTTP {status} must not be retried");
    }
    assert_eq!(h.sleeper.slept(), Vec::<Duration>::new());
}

#[test]
fn lookups_are_retried_like_gets() {
    let server = MockServer::start();
    let mock = sequence(
        &server,
        POST,
        "/api/client/find",
        vec![
            json_response(502, "{}"),
            json_response(200, fixtures::CLIENT_FIND),
        ],
    );
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 1);

    let response = h
        .client
        .find_client(&pairs(&[("id", "1001")]), &[])
        .expect("retried lookup");

    assert_eq!(body_json(response)["id"], 1001);
    mock.assert_calls(2);
    assert_eq!(h.sleeper.slept(), vec![Duration::from_millis(200)]);
}

#[test]
fn transport_failures_back_off_then_surface_as_http_errors() {
    let base = format!("http://127.0.0.1:{}", common::closed_port());
    let h = client(&base, Some(SECRET_TOKEN), 2);

    let err = h.client.get("/api/tables", &[]).unwrap_err();

    assert!(matches!(err, Error::Http(_)), "{err:?}");
    assert_eq!(
        h.sleeper.slept(),
        vec![Duration::from_millis(200), Duration::from_millis(400)]
    );
    assert!(!renderings(&err).contains(SECRET_TOKEN), "{err:?}");
}

// --- Secret hygiene ------------------------------------------------------------------------

#[test]
fn a_token_echoed_by_the_server_is_masked_in_errors() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/settings");
        then.status(401).body(format!(
            r#"{{"name":"Unauthorized","message":"token {SECRET_TOKEN} is revoked","status":401}}"#
        ));
    });
    server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(500)
            .body(format!("echo: Authorization: Bearer {SECRET_TOKEN}"));
    });
    let h = client(&server.base_url(), Some(SECRET_TOKEN), 0);

    let auth = h.client.get("/api/settings", &[]).unwrap_err();
    let api = h.client.get("/api/tables", &[]).unwrap_err();

    assert!(
        matches!(&auth, Error::Auth(m) if m == "token *** is revoked"),
        "{auth:?}"
    );
    assert!(
        matches!(&api, Error::Api { message, .. } if message == "echo: Authorization: Bearer ***"),
        "{api:?}"
    );
    for err in [&auth, &api] {
        assert!(!renderings(err).contains(SECRET_TOKEN), "{err:?}");
    }
}

#[test]
fn a_password_echoed_by_the_server_is_masked_in_login_errors() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/login");
        then.status(401)
            .body(r#"{"message":"wrong password hunter2-pass for a@b.c"}"#);
    });
    let connector = HttpConnector::new(RetryPolicy::with_sleeper(
        0,
        Arc::new(RecordingSleeper::new()),
    ));

    let err = connector
        .login(&server.base_url(), "a@b.c", &Secret::new("hunter2-pass"))
        .unwrap_err();

    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(!renderings(&err).contains("hunter2-pass"), "{err:?}");
}

/// `secret` as a server echoes it inside a JSON string.
fn json_escaped(secret: &str) -> String {
    let quoted = serde_json::to_string(secret).expect("serialise");
    quoted[1..quoted.len() - 1].to_owned()
}

#[test]
fn a_json_escaped_password_echo_is_masked_in_login_errors() {
    let filler = "x".repeat(MESSAGE_LIMIT - 3);
    for password in [r#"p"a\ss"#, r"back\slash"] {
        let server = MockServer::start();
        let echo = |message: String| json_response(401, &json!({ "message": message }).to_string());
        sequence(
            &server,
            POST,
            "/api/login",
            vec![
                echo(format!("wrong password {password} for a@b.c")),
                echo(format!("{filler}{password} is wrong")),
            ],
        );
        let connector = HttpConnector::new(RetryPolicy::with_sleeper(
            0,
            Arc::new(RecordingSleeper::new()),
        ));
        let login = || {
            connector
                .login(&server.base_url(), "a@b.c", &Secret::new(password))
                .unwrap_err()
        };

        let plain = login();
        let cut = login();

        assert!(
            matches!(&plain, Error::Auth(m) if m == "wrong password *** for a@b.c"),
            "{plain:?}"
        );
        assert!(
            matches!(&cut, Error::Auth(m) if m.contains("***")),
            "{cut:?}"
        );
        let prefix = &password[..3];
        for err in [&plain, &cut] {
            let shown = renderings(err);
            assert!(
                !shown.contains(prefix) && !shown.contains(&json_escaped(prefix)),
                "{err:?}"
            );
        }
    }
}

#[test]
fn a_token_cut_by_the_excerpt_limit_leaks_no_prefix() {
    const TOKEN: &str = r#"tok"SECRET\42"#;
    let echoed = format!("{}{TOKEN} is revoked", "x".repeat(MESSAGE_LIMIT - 3));
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/settings");
        then.status(403)
            .body(json!({ "message": echoed.clone() }).to_string());
    });
    server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(500).body(echoed.clone());
    });
    let h = client(&server.base_url(), Some(TOKEN), 0);

    let auth = h.client.get("/api/settings", &[]).unwrap_err();
    let api = h.client.get("/api/tables", &[]).unwrap_err();

    assert!(matches!(&auth, Error::Auth(_)), "{auth:?}");
    assert!(matches!(&api, Error::Api { status: 500, .. }), "{api:?}");
    for err in [&auth, &api] {
        let shown = renderings(err);
        assert!(shown.contains("***") && !shown.contains("tok"), "{err:?}");
    }
}
