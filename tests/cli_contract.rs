//! The public contract of the `hw` binary: exit codes, `--help`/`--version`, the stdout/stderr
//! split, colour control, SIGPIPE and secrets in diagnostics.

mod common;

use httpmock::prelude::*;
use predicates::prelude::*;
use pretty_assertions::assert_eq;

use common::{TOKEN, TestEnv, fixtures};
use hw::error::exit_code;

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

// --- Help, version, usage errors -----------------------------------------------------------

#[test]
fn version_prints_the_crate_version() {
    TestEnv::new()
        .cmd()
        .arg("--version")
        .assert()
        .code(exit_code::SUCCESS)
        .stdout(format!("hw {}\n", env!("CARGO_PKG_VERSION")))
        .stderr("");
}

#[test]
fn help_goes_to_stdout_with_exit_zero() {
    let env = TestEnv::new();
    for args in [&["--help"][..], &["-h"], &["booking", "list", "--help"]] {
        env.cmd()
            .args(args)
            .assert()
            .code(exit_code::SUCCESS)
            .stdout(predicate::str::contains("Usage:"))
            .stderr("");
    }
}

#[test]
fn usage_errors_exit_5_with_the_message_on_stderr() {
    let env = TestEnv::new();
    let cases: [&[&str]; 6] = [
        &[],
        &["--bogus", "menu"],
        &["report", "expenses"],
        &["report", "expenses", "--date", "31.12.2026"],
        &["sale", "list"],
        &["-q", "-v", "menu"],
    ];
    for args in cases {
        env.cmd()
            .args(args)
            .assert()
            .code(exit_code::INPUT_ERROR)
            .stdout("")
            .stderr(predicate::str::starts_with("error:").or(predicate::str::contains("Usage:")));
    }
}

// --- Exit codes ----------------------------------------------------------------------------

#[test]
fn api_responses_map_to_exit_codes() {
    let server = MockServer::start();
    let reply = |method: Method, path: &str, status: u16, body: &str| {
        server.mock(|when, then| {
            when.method(method).path(path);
            then.status(status).body(body);
        });
    };
    reply(GET, "/api/tables", 200, fixtures::TABLES);
    reply(GET, "/api/employees", 500, r#"{"message":"boom"}"#);
    reply(
        GET,
        "/api/booking/999",
        404,
        r#"{"name":"Not Found","message":"Бронь не найдена","code":0,"status":404}"#,
    );
    reply(GET, "/api/services", 401, fixtures::UNAUTHORIZED);
    reply(GET, "/api/bonus-levels", 403, fixtures::FORBIDDEN);
    reply(GET, "/api/expenses", 400, fixtures::EXPENSES_BAD_REQUEST);
    reply(POST, "/api/client/find", 422, fixtures::VALIDATION_FAILED);
    let crm = server.base_url();
    let env = TestEnv::new();

    let cases: [(&[&str], i32, &str); 7] = [
        (&["ref", "tables"], exit_code::SUCCESS, ""),
        (
            &["ref", "employees"],
            exit_code::RUNTIME_ERROR,
            "API error 500: boom",
        ),
        (
            &["booking", "view", "999"],
            exit_code::NOT_FOUND,
            "Бронь не найдена",
        ),
        (
            &["ref", "services"],
            exit_code::AUTH_ERROR,
            "invalid credentials",
        ),
        (
            &["ref", "bonus-levels"],
            exit_code::AUTH_ERROR,
            "Нет доступа",
        ),
        (
            &["report", "expenses", "--date", "2021-10-01"],
            exit_code::INPUT_ERROR,
            "Параметр date обязателен",
        ),
        (
            &["client", "find", "--id", "1"],
            exit_code::INPUT_ERROR,
            "name: Введите название",
        ),
    ];
    for (args, code, message) in cases {
        let output = env.api_cmd(&crm).args(args).output().expect("run hw");
        let (stdout, stderr) = (stdout_of(&output), stderr_of(&output));
        assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
        if code == exit_code::SUCCESS {
            assert_eq!(stderr, "", "{args:?}");
        } else {
            assert_eq!(stdout, "", "{args:?}: errors never go to stdout");
            assert!(
                stderr.starts_with("Error: ") && stderr.contains(message),
                "{args:?}: {stderr}"
            );
        }
    }
}

#[test]
fn network_failure_exits_1() {
    let crm = format!("http://127.0.0.1:{}", common::closed_port());
    TestEnv::new()
        .api_cmd(&crm)
        .args(["ref", "tables"])
        .assert()
        .code(exit_code::RUNTIME_ERROR)
        .stdout("")
        .stderr(predicate::str::contains("HTTP error"));
}

#[test]
fn configuration_problems_exit_3() {
    let env = TestEnv::new();
    let missing = env.path().join("missing.toml");

    env.cmd()
        .env("HW_TOKEN", TOKEN)
        .args(["ref", "tables"])
        .assert()
        .code(exit_code::CONFIG_ERROR)
        .stderr(predicate::str::contains("no CRM selected"));
    env.cmd()
        .env("HW_TOKEN", TOKEN)
        .env("HW_CONFIG", &missing)
        .args(["--crm", "demo", "ref", "tables"])
        .assert()
        .code(exit_code::CONFIG_ERROR)
        .stderr(predicate::str::contains("config file not found"));
    env.cmd()
        .env("HW_TOKEN", TOKEN)
        .args(["--crm", "demo", "--profile", "nope", "ref", "tables"])
        .assert()
        .code(exit_code::CONFIG_ERROR)
        .stderr(predicate::str::contains("profile 'nope' not found"));

    env.write_config("default_profile = [");
    env.cmd()
        .env("HW_TOKEN", TOKEN)
        .args(["--crm", "demo", "ref", "tables"])
        .assert()
        .code(exit_code::CONFIG_ERROR)
        .stderr(predicate::str::contains(env.config_path().as_str()));
}

#[test]
fn a_missing_token_exits_4_before_any_request() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.any_request();
        then.status(200).body("[]");
    });

    TestEnv::new()
        .cmd()
        .args(["--crm", &server.base_url(), "ref", "tables"])
        .assert()
        .code(exit_code::AUTH_ERROR)
        .stdout("")
        .stderr(predicate::str::contains("hw auth login"));

    any.assert_calls(0);
}

#[test]
fn local_validation_errors_exit_5_before_any_request() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.any_request();
        then.status(200).body("[]");
    });
    let env = TestEnv::new();

    env.api_cmd(&server.base_url())
        .args(["api", "https://example.com/api/tables"])
        .assert()
        .code(exit_code::INPUT_ERROR)
        .stderr(predicate::str::contains("is a URL"));
    env.cmd()
        .env("HW_TOKEN", TOKEN)
        .args(["--crm", "got cha", "ref", "tables"])
        .assert()
        .code(exit_code::INPUT_ERROR)
        .stderr(predicate::str::contains("invalid CRM"));

    any.assert_calls(0);
}

// --- Streams -------------------------------------------------------------------------------

#[test]
fn pagination_hint_goes_to_stderr_and_quiet_silences_it() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/api/products")
            .query_param("per-page", "3");
        then.status(200)
            .header("X-Pagination-Total-Count", "13")
            .header("X-Pagination-Page-Count", "5")
            .header("X-Pagination-Current-Page", "1")
            .header("X-Pagination-Per-Page", "3")
            .body(fixtures::PRODUCTS);
    });
    let env = TestEnv::new();
    let args = ["-F", "json", "product", "list", "--per-page", "3"];

    let loud = env.api_cmd(&server.base_url()).args(args).output().unwrap();
    let quiet = env
        .api_cmd(&server.base_url())
        .arg("-q")
        .args(args)
        .output()
        .unwrap();

    assert!(loud.status.success() && quiet.status.success());
    let parsed: serde_json::Value =
        serde_json::from_slice(&loud.stdout).expect("stdout is pure JSON");
    assert_eq!(parsed, fixtures::json(fixtures::PRODUCTS));
    assert_eq!(
        stderr_of(&loud),
        "hint: page 1 of 5 (13 total); use --all or --page <N>\n"
    );
    assert_eq!(stderr_of(&quiet), "");
    assert_eq!(quiet.stdout, loud.stdout);
}

#[test]
fn console_colour_follows_no_color_flag_and_env() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/settings");
        then.status(200).body(fixtures::SETTINGS);
    });
    let env = TestEnv::new();
    let run = |extra_env: &[(&str, &str)], extra_args: &[&str]| {
        let mut cmd = env.api_cmd(&server.base_url());
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        let output = cmd
            .args(extra_args)
            .args(["ref", "settings"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr_of(&output));
        stdout_of(&output)
    };

    // CLICOLOR_FORCE colours a pipe, which is what makes the negative cases meaningful.
    let forced = run(&[("CLICOLOR_FORCE", "1")], &[]);
    assert!(forced.contains('\x1b'), "forced colour: {forced:?}");

    for (label, out) in [
        ("piped", run(&[], &[])),
        (
            "--no-color",
            run(&[("CLICOLOR_FORCE", "1")], &["--no-color"]),
        ),
        (
            "NO_COLOR",
            run(&[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")], &[]),
        ),
    ] {
        assert!(!out.contains('\x1b'), "{label}: {out:?}");
        assert!(
            out.contains("timezone: Asia/Yekaterinburg"),
            "{label}: {out}"
        );
    }
}

#[cfg(unix)]
#[test]
fn closing_stdout_early_ends_quietly() {
    use std::io::{BufRead as _, BufReader};
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::Stdio;

    // Far more than a pipe buffer, so `hw` is still writing when the reader goes away.
    let tables: Vec<serde_json::Value> = (0..5000)
        .map(|id| serde_json::json!({"id": id, "name": format!("Стол {id}"), "seats": "4-5 чел"}))
        .collect();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(200)
            .body(serde_json::to_string(&tables).unwrap());
    });

    let env = TestEnv::new();
    let mut child = env
        .std_cmd()
        .env("HW_TOKEN", TOKEN)
        .args(["--crm", &server.base_url(), "-F", "json", "ref", "tables"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hw");
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    stdout.read_line(&mut first).unwrap();
    drop(stdout);
    let output = child.wait_with_output().unwrap();

    let stderr = stderr_of(&output);
    assert_eq!(first, "[\n", "{stderr}");
    assert!(
        output.status.code() == Some(0) || output.status.signal() == Some(libc::SIGPIPE),
        "{:?}: {stderr}",
        output.status
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "{stderr}"
    );
}

// --- Secrets -------------------------------------------------------------------------------

#[test]
fn the_token_never_reaches_stdout_or_stderr_even_at_trace_level() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/settings");
        then.status(200).body(fixtures::SETTINGS);
    });
    server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(401)
            .body(format!(r#"{{"message":"token {TOKEN} is revoked"}}"#));
    });
    let env = TestEnv::new();

    for (args, code) in [
        (["ref", "settings"], exit_code::SUCCESS),
        (["ref", "tables"], exit_code::AUTH_ERROR),
    ] {
        let output = env
            .api_cmd(&server.base_url())
            .env("RUST_LOG", "trace")
            .arg("-vvv")
            .args(args)
            .output()
            .unwrap();
        let (stdout, stderr) = (stdout_of(&output), stderr_of(&output));
        assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(!stderr.is_empty(), "{args:?}: trace logging must be on");
        assert!(!stdout.contains(TOKEN), "{args:?} stdout: {stdout}");
        assert!(!stderr.contains(TOKEN), "{args:?} stderr: {stderr}");
    }
}
