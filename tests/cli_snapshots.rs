//! Snapshots of user-facing output: `--help` text and command output built from the
//! documented API examples. Review changes with `cargo insta review`.

mod common;

use httpmock::prelude::*;

use common::{TestEnv, fixtures};

/// Runs `hw <args>` and returns stdout, failing on a non-zero exit.
fn stdout(cmd: &mut assert_cmd::Command, args: &[&str]) -> String {
    let output = cmd.args(args).output().expect("run hw");
    assert!(
        output.status.success(),
        "hw {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 stdout")
}

/// `hw.exe` in usage lines on Windows.
fn help_filters() -> Vec<(&'static str, &'static str)> {
    vec![(r"\bhw\.exe\b", "hw")]
}

/// The binary without `HW_CONFIG`: clap prints env values into `--help`.
fn help_cmd(env: &TestEnv) -> assert_cmd::Command {
    let mut cmd = env.cmd();
    cmd.env_remove("HW_CONFIG");
    cmd
}

#[test]
fn top_level_help() {
    let help = stdout(&mut help_cmd(&TestEnv::new()), &["--help"]);
    insta::with_settings!({ filters => help_filters() }, {
        insta::assert_snapshot!(help);
    });
}

#[test]
fn booking_list_help() {
    let help = stdout(
        &mut help_cmd(&TestEnv::new()),
        &["booking", "list", "--help"],
    );
    insta::with_settings!({ filters => help_filters() }, {
        insta::assert_snapshot!(help);
    });
}

#[test]
fn auth_login_help() {
    let help = stdout(&mut help_cmd(&TestEnv::new()), &["auth", "login", "--help"]);
    assert!(help.contains("https://<crm>.hookah.work/v2/settings/users"));
    insta::with_settings!({ filters => help_filters() }, {
        insta::assert_snapshot!(help);
    });
}

#[test]
fn self_update_help() {
    let help = stdout(
        &mut help_cmd(&TestEnv::new()),
        &["self", "update", "--help"],
    );
    assert!(help.contains("SHA-256 checksum"), "{help}");
    insta::with_settings!({ filters => help_filters() }, {
        insta::assert_snapshot!(help);
    });
}

#[test]
fn client_find_json() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/client/find");
        then.status(200).body(fixtures::CLIENT_FIND);
    });
    let env = TestEnv::new();

    let out = stdout(
        &mut env.api_cmd(&server.base_url()),
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

    insta::assert_snapshot!(out);
}

#[test]
fn auth_status_json() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/settings");
        then.status(200).body(fixtures::SETTINGS);
    });
    let env = TestEnv::new();

    let out = stdout(
        &mut env.api_cmd(&server.base_url()),
        &["-F", "json", "auth", "status"],
    );

    insta::with_settings!({ filters => vec![(r"127\.0\.0\.1:\d+", "127.0.0.1:[PORT]")] }, {
        insta::assert_snapshot!(out);
    });
}

#[test]
fn ref_tables_console() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/tables");
        then.status(200).body(fixtures::TABLES);
    });
    let env = TestEnv::new();

    let out = stdout(&mut env.api_cmd(&server.base_url()), &["ref", "tables"]);

    insta::assert_snapshot!(out);
}
