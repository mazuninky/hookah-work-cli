//! `hw auth` and `hw config` through the binary: login with config storage, token checks,
//! token precedence and redaction. No test here reaches the OS keyring (see `TestEnv`).

mod common;

use httpmock::prelude::*;
use predicates::prelude::*;
use pretty_assertions::assert_eq;
use serde_json::json;

use common::{TOKEN, TestEnv, fixtures};
use hw::config::{Config, TokenStorage};
use hw::error::exit_code;

fn parse_config(path: &camino::Utf8Path) -> Config {
    toml::from_str(&common::read(path)).expect("valid config")
}

fn settings_for<'a>(server: &'a MockServer, token: &str) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path("/api/settings")
            .header("authorization", format!("Bearer {token}"));
        then.status(200).body(fixtures::SETTINGS);
    })
}

fn reject_everything_else(server: &MockServer) {
    server.mock(|when, then| {
        when.any_request();
        then.status(401).body(fixtures::UNAUTHORIZED);
    });
}

#[test]
fn login_with_token_writes_a_private_config_that_later_commands_use() {
    let server = MockServer::start();
    let crm = server.base_url();
    let check = settings_for(&server, "pasted-token");
    let tables = server.mock(|when, then| {
        when.method(GET)
            .path("/api/tables")
            .header("authorization", "Bearer pasted-token");
        then.status(200).body(fixtures::TABLES);
    });
    let env = TestEnv::new();
    let config = env.path().join("nested/hw/config.toml");

    let login = env
        .cmd()
        .env("HW_CONFIG", &config)
        .args(["--crm", &crm, "--profile", "local", "-F", "json"])
        .args(["auth", "login", "--with-token", "--storage", "config"])
        .write_stdin("  pasted-token \n")
        .output()
        .unwrap();

    assert!(
        login.status.success(),
        "{}",
        String::from_utf8_lossy(&login.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&login.stdout).unwrap();
    assert_eq!(
        report,
        json!({"profile": "local", "crm": crm, "storage": "config"})
    );
    check.assert_calls(1);
    let saved = parse_config(&config);
    assert_eq!(saved.default_profile.as_deref(), Some("local"));
    let profile = &saved.profiles["local"];
    assert_eq!(profile.crm.as_deref(), Some(crm.as_str()));
    assert_eq!(profile.token_storage, TokenStorage::Config);
    assert_eq!(profile.api_token.as_deref(), Some("pasted-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&config).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "config holds a token: {mode:o}");
    }

    env.cmd()
        .env("HW_CONFIG", &config)
        .args(["-F", "json", "ref", "tables"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Барная стойка"));
    tables.assert_calls(1);
}

#[test]
fn login_with_a_rejected_token_saves_nothing() {
    let server = MockServer::start();
    reject_everything_else(&server);
    let env = TestEnv::new();
    let config = env.path().join("fresh/config.toml");

    env.cmd()
        .env("HW_CONFIG", &config)
        .args(["--crm", &server.base_url()])
        .args(["auth", "login", "--with-token", "--storage", "config"])
        .write_stdin("bad-token\n")
        .assert()
        .code(exit_code::AUTH_ERROR)
        .stdout("")
        .stderr(predicate::str::contains("invalid credentials"))
        .stderr(predicate::str::contains("bad-token").not());

    assert!(
        !config.exists(),
        "nothing may be written for a rejected token"
    );
}

#[test]
fn login_with_password_exchanges_credentials_for_a_token() {
    let server = MockServer::start();
    let login = server.mock(|when, then| {
        when.method(POST)
            .path("/api/login")
            // base64("integration@example.com:s3cret pass")
            .header(
                "authorization",
                "Basic aW50ZWdyYXRpb25AZXhhbXBsZS5jb206czNjcmV0IHBhc3M=",
            );
        then.status(200).body(fixtures::LOGIN);
    });
    let check = settings_for(&server, "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
    let env = TestEnv::new();

    let output = env
        .cmd()
        .args(["--crm", &server.base_url(), "-F", "json", "auth", "login"])
        .args(["--email", "integration@example.com", "--password-stdin"])
        .write_stdin("s3cret pass\n")
        .output()
        .unwrap();

    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.status.success(), "{stderr}");
    login.assert_calls(1);
    check.assert_calls(1);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["email"], "integration@example.com");
    assert_eq!(report["name"], "Интеграция");
    assert_eq!(report["profile"], "127");
    assert_eq!(report["storage"], "config", "the default for a new profile");
    for text in [&stdout, &stderr] {
        assert!(!text.contains("s3cret pass"), "{text}");
        assert!(!text.contains("xxxxxxxxxxxxxxxx"), "{text}");
    }
    let profile = &parse_config(&env.config_path()).profiles["127"];
    assert_eq!(profile.token_storage, TokenStorage::Config);
    assert_eq!(
        profile.api_token.as_deref(),
        Some("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")
    );
    assert_eq!(profile.email.as_deref(), Some("integration@example.com"));
}

#[test]
fn login_without_a_method_explains_both_ways() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.any_request();
        then.status(200);
    });

    let env = TestEnv::new();
    env.cmd()
        .args(["--crm", &server.base_url(), "auth", "login"])
        .assert()
        .code(exit_code::INPUT_ERROR)
        .stderr(predicate::str::contains("--with-token"))
        .stderr(predicate::str::contains("--password-stdin"))
        .stderr(predicate::str::contains(format!(
            "{}/v2/settings/users",
            server.base_url()
        )));
    env.cmd()
        .args(["auth", "login"])
        .assert()
        .code(exit_code::INPUT_ERROR)
        .stderr(predicate::str::contains(
            "https://<crm>.hookah.work/v2/settings/users",
        ));

    any.assert_calls(0);
}

#[test]
fn status_reports_source_and_validity_and_fails_on_a_bad_token() {
    let server = MockServer::start();
    settings_for(&server, TOKEN);
    reject_everything_else(&server);
    let crm = server.base_url();
    let env = TestEnv::new();
    let status = |token: Option<&str>| {
        let mut cmd = env.cmd();
        if let Some(token) = token {
            cmd.env("HW_TOKEN", token);
        }
        let output = cmd
            .args(["--crm", &crm, "-F", "json", "auth", "status"])
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
        (output.status.code(), report)
    };

    let (code, valid) = status(Some(TOKEN));
    assert_eq!(code, Some(exit_code::SUCCESS));
    assert_eq!(
        valid,
        json!({"profile": null, "crm": crm, "token_source": "env", "token_status": "valid"})
    );

    let (code, invalid) = status(Some("revoked-token"));
    assert_eq!(code, Some(exit_code::AUTH_ERROR));
    assert_eq!(invalid["token_status"], "invalid");

    let (code, missing) = status(None);
    assert_eq!(code, Some(exit_code::AUTH_ERROR));
    assert_eq!(
        (&missing["token_source"], &missing["token_status"]),
        (&json!("none"), &json!("unchecked"))
    );
}

#[test]
fn env_token_takes_precedence_over_the_config_token() {
    let server = MockServer::start();
    let with_env = settings_for(&server, "env-token");
    let with_config = settings_for(&server, "config-token");
    reject_everything_else(&server);
    let env = TestEnv::new();
    env.write_config(&format!(
        "[profiles.main]\ncrm = \"{}\"\ntoken_storage = \"config\"\napi_token = \"config-token\"\n",
        server.base_url()
    ));

    env.cmd()
        .env("HW_TOKEN", "env-token")
        .args(["ref", "settings"])
        .assert()
        .success();
    env.cmd().args(["ref", "settings"]).assert().success();

    with_env.assert_calls(1);
    with_config.assert_calls(1);
}

#[test]
fn a_profile_token_is_sent_only_to_its_own_origin() {
    let server = MockServer::start();
    let accepted = settings_for(&server, "config-token");
    let leaked = server.mock(|when, then| {
        when.any_request();
        then.status(401).body(fixtures::UNAUTHORIZED);
    });
    let port = server.port();
    let env = TestEnv::new();
    let profile = |crm: &str| {
        format!(
            "[profiles.main]\ncrm = \"{crm}\"\ntoken_storage = \"config\"\napi_token = \"config-token\"\n"
        )
    };

    for own in [
        format!("https://127.0.0.1:{port}"),
        format!("http://127.0.0.1:{}", port.wrapping_add(1)),
    ] {
        env.write_config(&profile(&own));
        env.cmd()
            .args(["--crm", &server.base_url(), "ref", "settings"])
            .assert()
            .code(exit_code::AUTH_ERROR)
            .stderr(predicate::str::contains(format!(
                "its token is never sent to http://127.0.0.1:{port}"
            )));
    }
    env.write_config(&profile(&server.base_url()));
    env.cmd()
        .args([
            "--crm",
            &format!("HTTP://127.0.0.1:{port}/api/"),
            "ref",
            "settings",
        ])
        .assert()
        .success();

    accepted.assert_calls(1);
    leaked.assert_calls(0);
}

#[test]
fn auth_token_prints_only_the_raw_token() {
    TestEnv::new()
        .cmd()
        .env("HW_TOKEN", TOKEN)
        .args(["-F", "json", "auth", "token"])
        .assert()
        .success()
        .stdout(format!("{TOKEN}\n"))
        .stderr("");
}

#[test]
fn config_list_and_show_never_print_the_token() {
    let env = TestEnv::new();
    env.write_config(
        r#"default_profile = "demo"
[profiles.demo]
crm = "demo"
email = "integration@example.com"
token_storage = "config"
api_token = "very-secret-token"
[profiles.another]
crm = "another.hookah.work"
"#,
    );
    let stdout = |args: &[&str]| {
        let output = env.cmd().args(args).output().unwrap();
        assert!(output.status.success(), "{args:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("very-secret-token"), "{args:?}: {text}");
        text
    };

    let list: serde_json::Value =
        serde_json::from_str(&stdout(&["-F", "json", "config", "list"])).expect("JSON list");
    let show: serde_json::Value =
        serde_json::from_str(&stdout(&["-F", "json", "config", "show"])).expect("JSON show");
    stdout(&["config", "list"]);
    let console_show = stdout(&["config", "show", "demo"]);

    assert_eq!(
        list,
        json!([
            {"name": "another", "crm": "another.hookah.work", "default": false, "token_storage": "config"},
            {"name": "demo", "crm": "demo", "default": true, "token_storage": "config"},
        ])
    );
    assert_eq!(show["name"], "demo");
    assert_eq!(show["api_token"], "***");
    assert!(console_show.contains("api_token: ***"), "{console_show}");
}

#[test]
fn set_default_switches_the_selected_profile() {
    let env = TestEnv::new();
    env.write_config("[profiles.a]\ncrm = \"a\"\n[profiles.b]\ncrm = \"b\"\n");

    env.cmd()
        .args(["config", "set-default", "b"])
        .assert()
        .success();
    env.cmd()
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("name: b"));
    env.cmd()
        .args(["config", "set-default", "zzz"])
        .assert()
        .code(exit_code::CONFIG_ERROR);
}

#[test]
fn config_path_follows_hw_config() {
    let env = TestEnv::new();
    env.cmd()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(format!("{}\n", env.config_path()));
}

#[cfg(unix)]
#[test]
fn config_is_discovered_under_xdg_config_home_without_hw_config() {
    let env = TestEnv::new();
    let xdg_config = env.path().join("xdg/hw/config.toml");
    std::fs::create_dir_all(xdg_config.parent().unwrap()).unwrap();
    std::fs::write(&xdg_config, "[profiles.found]\ncrm = \"found\"\n").unwrap();

    env.cmd()
        .env_remove("HW_CONFIG")
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(format!("{xdg_config}\n"));
    env.cmd()
        .env_remove("HW_CONFIG")
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("found"));
}
