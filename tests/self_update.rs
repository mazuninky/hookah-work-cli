//! `hw self check` / `hw self update` end to end: in-process through `app::run` with the real
//! `GitHubReleases` against a fake github.com (httpmock) and a `FakeInstaller`, plus the
//! contract of the binary for everything that fails or finishes before a request.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use camino::Utf8PathBuf;
use clap::{CommandFactory as _, FromArgMatches as _};
use httpmock::{HttpMockRequest, HttpMockResponse, MockServer};
use predicates::prelude::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use common::{Ran, TOKEN, TestEnv, closed_port};
use hw::app::{Deps, Env};
use hw::auth::{InMemoryStore, MockPrompter, Secret};
use hw::cli::Cli;
use hw::client::{NoSleep, RetryPolicy};
use hw::error::exit_code;
use hw::io::IoStreams;
use hw::test_util::{FakeApi, FakeConnector, FakeInstaller, FakeIssuer, release_archive};
use hw::update::{GitHubReleases, Target, Version, sha256_hex};

/// Releases of the repository on github.com, below the web root.
const RELEASES: &str = "/mazuninky/hookah-work-cli/releases";
const LATEST: &str = "/mazuninky/hookah-work-cli/releases/latest";

/// Version of the `hw` under test.
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Where the [`FakeInstaller`] says the running executable is.
const EXE: &str = "/home/user/.local/bin/hw";
/// The profile's own API token; like `HW_TOKEN` it must never reach GitHub.
const PROFILE_TOKEN: &str = "profile-tok-fedcba9876543210";

const NEW_BINARY: &[u8] = b"\x7fELF the next hw build";
const OLD_BINARY: &[u8] = b"\x7fELF an earlier hw build";

/// This build's version with `years` added to the year and `builds` to the build number.
fn version(years: i32, builds: i32) -> Version {
    let parts: Vec<i32> = CURRENT
        .split('.')
        .map(|part| part.parse().expect("numeric version part"))
        .collect();
    format!("{}.{}.{}", parts[0] + years, parts[1], parts[2] + builds)
        .parse()
        .unwrap_or_else(|e| panic!("{CURRENT} {years:+} years {builds:+} builds: {e}"))
}

fn current() -> Version {
    version(0, 0)
}

fn newer() -> Version {
    version(0, 1)
}

fn older() -> Version {
    version(-1, 0)
}

fn tag_path(version: &Version) -> String {
    format!("{RELEASES}/tag/v{version}")
}

fn download_path(version: &Version, name: &str) -> String {
    format!("{RELEASES}/download/v{version}/{name}")
}

/// Where release downloads redirect to, as github.com sends them to its asset CDN.
fn cdn_path(name: &str) -> String {
    format!("/cdn/{name}")
}

// --- Fake github.com ---------------------------------------------------------------------

/// The answer of [`FakeGitHub`] on one path.
#[derive(Debug, Clone)]
enum Reply {
    /// `302 Found` to this `Location`.
    Redirect(String),
    /// `200 OK` with this body.
    Body(Vec<u8>),
    /// This status and an empty body.
    Status(u16),
}

/// One request as [`FakeGitHub`] saw it.
#[derive(Debug, Clone)]
struct Hit {
    /// `"GET /path"`.
    line: String,
    authorization: bool,
}

/// A catch-all httpmock server standing in for github.com: answers the routes given to
/// [`FakeGitHub::serve`] (any other path is 404) and records every request, whatever its
/// method or path.
struct FakeGitHub {
    server: MockServer,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl FakeGitHub {
    fn start() -> Self {
        Self {
            server: MockServer::start(),
            hits: Arc::default(),
        }
    }

    /// Stands in for `https://github.com`.
    fn url(&self) -> String {
        self.server.base_url()
    }

    /// Answers `routes` (path → reply) from now on; call once.
    fn serve(&self, routes: Vec<(String, Reply)>) {
        let routes: HashMap<String, Reply> = routes.into_iter().collect();
        let hits = Arc::clone(&self.hits);
        self.server.mock(|when, then| {
            when.any_request();
            then.respond_with(move |req: &HttpMockRequest| {
                let path = req.uri().path().to_owned();
                hits.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Hit {
                        line: format!("{} {path}", req.method_str()),
                        authorization: req.headers().contains_key("authorization"),
                    });
                let reply = HttpMockResponse::builder();
                match routes.get(&path) {
                    Some(Reply::Redirect(location)) => {
                        reply.status(302).header("Location", location.as_str())
                    }
                    Some(Reply::Body(bytes)) => reply.status(200).body(bytes.clone()),
                    Some(Reply::Status(status)) => reply.status(*status),
                    None => reply.status(404).body("Not Found"),
                }
                .build()
            });
        });
    }

    /// `METHOD /path` of every request so far, in order.
    fn requests(&self) -> Vec<String> {
        self.hits().into_iter().map(|hit| hit.line).collect()
    }

    /// Requests so far that carried an `Authorization` header.
    fn authorized(&self) -> Vec<String> {
        self.hits()
            .into_iter()
            .filter(|hit| hit.authorization)
            .map(|hit| hit.line)
            .collect()
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// `/releases/latest` redirecting to the tag page of `version` (path-only `Location`).
fn latest_is(version: &Version) -> (String, Reply) {
    (LATEST.to_owned(), Reply::Redirect(tag_path(version)))
}

/// Release `version` for `target` as published: `archive` behind a CDN redirect and the
/// `.sha256` sidecar holding `sidecar`.
fn assets(
    version: &Version,
    target: Target,
    archive: Vec<u8>,
    sidecar: &str,
) -> Vec<(String, Reply)> {
    let asset = target.asset_name(version);
    vec![
        (
            download_path(version, &target.checksum_name(version)),
            Reply::Body(sidecar.as_bytes().to_vec()),
        ),
        (
            download_path(version, &asset),
            Reply::Redirect(cdn_path(&asset)),
        ),
        (cdn_path(&asset), Reply::Body(archive)),
    ]
}

/// Release `version` for `target` packing `binary`, with a matching checksum.
fn release(version: &Version, target: Target, binary: &[u8]) -> Vec<(String, Reply)> {
    let archive = release_archive(version, target, binary);
    let sidecar = format!("{}  {}\n", sha256_hex(&archive), target.asset_name(version));
    assets(version, target, archive, &sidecar)
}

/// The requests that fetch release `version` for `target`, sorted.
fn release_requests(version: &Version, target: Target) -> Vec<String> {
    let asset = target.asset_name(version);
    let mut lines = vec![
        format!(
            "GET {}",
            download_path(version, &target.checksum_name(version))
        ),
        format!("GET {}", download_path(version, &asset)),
        format!("GET {}", cdn_path(&asset)),
    ];
    lines.sort();
    lines
}

fn sorted(mut lines: Vec<String>) -> Vec<String> {
    lines.sort();
    lines
}

// --- In-process runner -------------------------------------------------------------------

/// `hw::app::run` with the production [`GitHubReleases`] aimed at a [`FakeGitHub`], a
/// [`FakeInstaller`] at [`EXE`], `HW_TOKEN` set and a profile holding a token for the fake
/// GitHub's own origin. argv is parsed with clap's env fallbacks off (no `HW_*` leaks in).
struct Runner {
    _dir: TempDir,
    env: Env,
    store: InMemoryStore,
    connector: FakeConnector,
    issuer: FakeIssuer,
    releases: GitHubReleases,
    installer: FakeInstaller,
}

impl Runner {
    fn new(gh: &FakeGitHub) -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let config = Utf8PathBuf::from_path_buf(dir.path().join("config.toml")).expect("UTF-8");
        std::fs::write(
            &config,
            format!(
                "[profiles.test]\ncrm = \"{}\"\ntoken_storage = \"config\"\napi_token = \"{PROFILE_TOKEN}\"\n",
                gh.url()
            ),
        )
        .expect("write config");
        Self {
            _dir: dir,
            env: Env {
                token: Some(Secret::new(TOKEN)),
                default_config_path: Some(config),
            },
            store: InMemoryStore::new(),
            connector: FakeConnector::new(FakeApi::new()),
            issuer: FakeIssuer::status(401, "{}"),
            releases: GitHubReleases::with_base_url(
                &gh.url(),
                RetryPolicy::with_sleeper(0, Arc::new(NoSleep)),
            ),
            installer: FakeInstaller::new(EXE),
        }
    }

    /// Runs `hw <args>`.
    fn run(&self, args: &[&str]) -> Ran {
        let argv = std::iter::once("hw").chain(args.iter().copied());
        let matches = Cli::command()
            .mut_args(|arg| arg.env(None))
            .try_get_matches_from(argv)
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        let prompter = MockPrompter::default();
        let deps = Deps {
            store: &self.store,
            connector: &self.connector,
            issuer: &self.issuer,
            prompter: &prompter,
            env: &self.env,
            now: common::NOW.parse().expect("valid timestamp"),
            releases: &self.releases,
            installer: &self.installer,
        };
        let mut io = IoStreams::test();
        let result = hw::app::run(&cli, &mut io, &deps);
        Ran {
            result,
            stdout: io.stdout_as_string(),
            stderr: io.stderr_as_string(),
        }
    }
}

/// The error of a failed run, with its context chain.
fn error(ran: &Ran) -> String {
    match &ran.result {
        Err(err) => format!("{err:#}"),
        Ok(()) => panic!("expected a failure, got stdout {:?}", ran.stdout),
    }
}

fn update_report(installed: &Version, path: serde_json::Value, updated: bool) -> serde_json::Value {
    json!({
        "previous": CURRENT,
        "installed": installed.to_string(),
        "path": path,
        "updated": updated,
    })
}

// --- hw self check -----------------------------------------------------------------------

#[test]
fn check_reports_a_newer_release_with_a_hint_that_quiet_hides() {
    let gh = FakeGitHub::start();
    let newer = newer();
    // The absolute form of the redirect, as github.com sends it.
    gh.serve(vec![(
        LATEST.to_owned(),
        Reply::Redirect(format!("{}{}", gh.url(), tag_path(&newer))),
    )]);
    let runner = Runner::new(&gh);
    let expected = json!({
        "current": CURRENT,
        "latest": newer.to_string(),
        "update_available": true,
        "release_url": format!("https://github.com/mazuninky/hookah-work-cli/releases/tag/v{newer}"),
    });

    let ran = runner.run(&["-F", "json", "self", "check"]);
    ran.assert_ok("self check");
    assert_eq!(ran.json(), expected);
    assert_eq!(
        ran.stderr,
        format!("hint: hw {newer} is available; install it with `hw self update`\n")
    );

    let quiet = runner.run(&["-q", "-F", "json", "self", "check"]);
    quiet.assert_ok("self check -q");
    assert_eq!(quiet.json(), expected);
    assert_eq!(quiet.stderr, "");

    assert_eq!(
        gh.requests(),
        [format!("GET {LATEST}"), format!("GET {LATEST}")]
    );
    assert_eq!(runner.installer.located(), 0);
}

#[test]
fn check_when_up_to_date_reports_no_update_and_no_hint() {
    let gh = FakeGitHub::start();
    gh.serve(vec![latest_is(&current())]);
    let runner = Runner::new(&gh);

    let ran = runner.run(&["-F", "json", "self", "check"]);

    ran.assert_ok("self check");
    assert_eq!(
        ran.json(),
        json!({
            "current": CURRENT,
            "latest": CURRENT,
            "update_available": false,
            "release_url": format!("https://github.com/mazuninky/hookah-work-cli/releases/tag/v{CURRENT}"),
        })
    );
    assert_eq!(ran.stderr, "");
}

// --- hw self update ----------------------------------------------------------------------

#[test]
fn update_installs_the_verified_binary_of_the_latest_release() {
    let Some(target) = Target::current() else {
        return; // no prebuilt release for this platform; the handler's unit tests cover it
    };
    let gh = FakeGitHub::start();
    let newer = newer();
    let mut routes = vec![latest_is(&newer)];
    routes.extend(release(&newer, target, NEW_BINARY));
    gh.serve(routes);
    let runner = Runner::new(&gh);

    let ran = runner.run(&["-F", "json", "self", "update"]);

    ran.assert_ok("self update");
    assert_eq!(ran.json(), update_report(&newer, json!(EXE), true));
    assert_eq!(
        ran.stderr,
        format!("hint: hw updated from {CURRENT} to {newer} at {EXE}\n")
    );
    assert_eq!(
        runner.installer.replaced(),
        [(Utf8PathBuf::from(EXE), NEW_BINARY.to_vec())]
    );
    let mut expected = release_requests(&newer, target);
    expected.push(format!("GET {LATEST}"));
    assert_eq!(sorted(gh.requests()), sorted(expected));
}

#[test]
fn checksum_mismatch_fails_and_replaces_nothing() {
    let Some(target) = Target::current() else {
        return;
    };
    let gh = FakeGitHub::start();
    let newer = newer();
    let archive = release_archive(&newer, target, NEW_BINARY);
    let forged = format!(
        "{}  {}\n",
        sha256_hex(b"a different archive"),
        target.asset_name(&newer)
    );
    let mut routes = vec![latest_is(&newer)];
    routes.extend(assets(&newer, target, archive, &forged));
    gh.serve(routes);
    let runner = Runner::new(&gh);

    let ran = runner.run(&["-F", "json", "self", "update"]);

    assert_eq!(ran.exit_code(), exit_code::RUNTIME_ERROR, "{}", error(&ran));
    let message = error(&ran);
    assert!(
        message.contains(&format!(
            "checksum mismatch for {}",
            target.asset_name(&newer)
        )) && message.contains("nothing was replaced"),
        "{message}"
    );
    assert_eq!(runner.installer.replaced(), []);
    assert_eq!(ran.stdout, "");
}

#[test]
fn checksum_file_for_another_asset_fails_and_replaces_nothing() {
    let Some(target) = Target::current() else {
        return;
    };
    let gh = FakeGitHub::start();
    let newer = newer();
    let archive = release_archive(&newer, target, NEW_BINARY);
    // The right digest, but filed under the previous release's archive.
    let stale = format!(
        "{}  {}\n",
        sha256_hex(&archive),
        target.asset_name(&current())
    );
    let mut routes = vec![latest_is(&newer)];
    routes.extend(assets(&newer, target, archive, &stale));
    gh.serve(routes);
    let runner = Runner::new(&gh);

    let ran = runner.run(&["-F", "json", "self", "update"]);

    assert_eq!(ran.exit_code(), exit_code::RUNTIME_ERROR, "{}", error(&ran));
    let message = error(&ran);
    assert!(
        message.contains(&format!(
            "invalid checksum file for {}: it is for {}",
            target.asset_name(&newer),
            target.asset_name(&current())
        )),
        "{message}"
    );
    assert_eq!(runner.installer.replaced(), []);
}

#[test]
fn update_to_a_release_without_assets_is_not_found() {
    if Target::current().is_none() {
        return;
    }
    let gh = FakeGitHub::start();
    gh.serve(vec![latest_is(&current())]);
    let runner = Runner::new(&gh);
    let missing = newer();

    let ran = runner.run(&["self", "update", "--to", &missing.to_string()]);

    assert_eq!(ran.exit_code(), exit_code::NOT_FOUND, "{}", error(&ran));
    let message = error(&ran);
    assert!(
        message.contains(&format!("in release v{missing}")),
        "{message}"
    );
    assert_eq!(runner.installer.replaced(), []);
    let requests = gh.requests();
    assert!(
        !requests.is_empty()
            && requests
                .iter()
                .all(|line| line.starts_with(&format!("GET {RELEASES}/download/v{missing}/"))),
        "--to must not consult the latest release: {requests:?}"
    );
}

#[test]
fn downgrade_needs_allow_downgrade_and_downloads_nothing_without_it() {
    let Some(target) = Target::current() else {
        return;
    };
    let gh = FakeGitHub::start();
    let older = older();
    let mut routes = vec![latest_is(&newer())];
    routes.extend(release(&older, target, OLD_BINARY));
    gh.serve(routes);
    let runner = Runner::new(&gh);
    let to = older.to_string();

    let refused = runner.run(&["-F", "json", "self", "update", "--to", &to]);

    assert_eq!(
        refused.exit_code(),
        exit_code::INPUT_ERROR,
        "{}",
        error(&refused)
    );
    let message = error(&refused);
    assert!(
        message.contains(&format!("{older} is older than the running hw {CURRENT}"))
            && message.contains("--allow-downgrade"),
        "{message}"
    );
    assert_eq!(gh.requests(), Vec::<String>::new());
    assert_eq!(runner.installer.located(), 0);

    let ran = runner.run(&[
        "-F",
        "json",
        "self",
        "update",
        "--to",
        &to,
        "--allow-downgrade",
    ]);

    ran.assert_ok("self update --allow-downgrade");
    assert_eq!(ran.json(), update_report(&older, json!(EXE), true));
    assert_eq!(
        runner.installer.replaced(),
        [(Utf8PathBuf::from(EXE), OLD_BINARY.to_vec())]
    );
    assert_eq!(sorted(gh.requests()), release_requests(&older, target));
}

#[test]
fn update_when_up_to_date_downloads_and_replaces_nothing() {
    let gh = FakeGitHub::start();
    let mut routes = vec![latest_is(&current())];
    if let Some(target) = Target::current() {
        // Published, so only the version check can keep it from being installed.
        routes.extend(release(&current(), target, NEW_BINARY));
    }
    gh.serve(routes);
    let runner = Runner::new(&gh);

    for args in [
        &["-F", "json", "self", "update"][..],
        &["-F", "json", "self", "update", "--to", CURRENT],
    ] {
        let ran = runner.run(args);
        ran.assert_ok(&format!("{args:?}"));
        assert_eq!(
            ran.json(),
            update_report(&current(), serde_json::Value::Null, false),
            "{args:?}"
        );
        assert_eq!(
            ran.stderr,
            format!("hint: hw {CURRENT} is up to date\n"),
            "{args:?}"
        );
    }

    assert_eq!(gh.requests(), [format!("GET {LATEST}")]);
    assert_eq!(runner.installer.located(), 0);
    assert_eq!(runner.installer.replaced(), []);
}

#[test]
fn no_published_release_is_not_found() {
    for (name, reply) in [
        (
            "redirect to the release list",
            Reply::Redirect(RELEASES.to_owned()),
        ),
        ("404", Reply::Status(404)),
    ] {
        let gh = FakeGitHub::start();
        gh.serve(vec![(LATEST.to_owned(), reply)]);
        let runner = Runner::new(&gh);

        for args in [&["self", "check"][..], &["self", "update"]] {
            let ran = runner.run(args);
            assert_eq!(
                ran.exit_code(),
                exit_code::NOT_FOUND,
                "{name}, {args:?}: {}",
                error(&ran)
            );
            let message = error(&ran);
            assert!(
                message.contains("no releases of mazuninky/hookah-work-cli published yet"),
                "{name}, {args:?}: {message}"
            );
            assert_eq!(ran.stdout, "", "{name}, {args:?}");
        }
        assert_eq!(
            gh.requests(),
            [format!("GET {LATEST}"), format!("GET {LATEST}")],
            "{name}"
        );
        assert_eq!(runner.installer.located(), 0, "{name}");
    }
}

#[test]
fn github_never_receives_an_authorization_header_nor_the_crm_a_request() {
    let gh = FakeGitHub::start();
    let newer = newer();
    let mut routes = vec![latest_is(&newer)];
    if let Some(target) = Target::current() {
        routes.extend(release(&newer, target, NEW_BINARY));
    }
    gh.serve(routes);
    let runner = Runner::new(&gh);
    // HW_TOKEN is set and profile `test` holds a token for this very origin.
    let crm = gh.url();
    let global = ["--crm", crm.as_str(), "-p", "test", "-F", "json"];

    let check = runner.run(&[&global[..], &["self", "check"]].concat());
    check.assert_ok("self check");
    if Target::current().is_some() {
        let update = runner.run(&[&global[..], &["self", "update"]].concat());
        update.assert_ok("self update");
        assert_eq!(update.json()["updated"], true);
    }

    let requests = gh.requests();
    assert!(requests.len() > 1, "{requests:?}");
    assert!(
        requests.iter().all(|line| line.starts_with("GET ")),
        "hw self must only read from GitHub: {requests:?}"
    );
    assert_eq!(gh.authorized(), Vec::<String>::new());
    assert_eq!(runner.connector.connections(), []);
}

// --- The binary --------------------------------------------------------------------------

/// `hw` in a [`TestEnv`] that cannot reach the network: every proxy variable ureq reads points
/// at a closed loopback port and retries are off, so a request a test does not expect fails
/// fast and locally (exit 1) instead of reaching github.com.
fn offline_cmd(env: &TestEnv) -> assert_cmd::Command {
    let proxy = format!("http://127.0.0.1:{}", closed_port());
    let mut cmd = env.cmd();
    for key in ["ALL_PROXY", "HTTPS_PROXY", "HTTP_PROXY"] {
        cmd.env(key, &proxy).env(key.to_lowercase(), &proxy);
    }
    cmd.env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .args(["--retries", "0"]);
    cmd
}

#[test]
fn offline_guard_turns_a_github_request_into_exit_1() {
    // The guard every other binary test relies on: a request that does happen fails locally.
    offline_cmd(&TestEnv::new())
        .args(["self", "check"])
        .assert()
        .code(exit_code::RUNTIME_ERROR)
        .stdout("")
        .stderr(predicate::str::contains("Error: HTTP error"));
}

#[test]
fn self_help_describes_check_and_the_verified_update() {
    let env = TestEnv::new();
    offline_cmd(&env)
        .args(["self", "--help"])
        .assert()
        .code(exit_code::SUCCESS)
        .stdout(
            predicate::str::contains("check")
                .and(predicate::str::contains("update"))
                .and(predicate::str::contains("latest release on GitHub")),
        )
        .stderr("");
    offline_cmd(&env)
        .args(["self", "update", "--help"])
        .assert()
        .code(exit_code::SUCCESS)
        .stdout(
            predicate::str::contains("SHA-256 checksum")
                .and(predicate::str::contains("before anything is unpacked"))
                .and(predicate::str::contains("replace the running hw binary"))
                .and(predicate::str::contains("--to <VERSION>"))
                .and(predicate::str::contains("--allow-downgrade")),
        )
        .stderr("");
}

#[test]
fn self_usage_errors_exit_5_before_any_request() {
    let env = TestEnv::new();
    for (args, message) in [
        (
            &["self", "update", "--to", "2026.x.1"][..],
            "expected YEAR.WEEK.BUILD",
        ),
        (
            &["self", "update", "--to", "2026.54.1"],
            "expected YEAR.WEEK.BUILD",
        ),
        (
            &["self", "update", "--to", "latest"],
            "expected YEAR.WEEK.BUILD",
        ),
        (&["self", "update", "--allow-downgrade"], "--to <VERSION>"),
        (&["self", "check", "--to", CURRENT], "--to"),
        (&["self"], "Usage:"),
    ] {
        let output = offline_cmd(&env).args(args).output().expect("run hw");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(exit_code::INPUT_ERROR),
            "{args:?}: {stderr}"
        );
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn downgrade_without_allow_downgrade_exits_5_before_any_request() {
    let older = older().to_string();
    offline_cmd(&TestEnv::new())
        .args(["self", "update", "--to", &older])
        .assert()
        .code(exit_code::INPUT_ERROR)
        .stdout("")
        .stderr(predicate::str::contains(format!(
            "{older} is older than the running hw {CURRENT}; pass --allow-downgrade"
        )));
}

#[test]
fn update_to_the_running_version_needs_no_config_crm_or_token() {
    let env = TestEnv::new();
    env.write_config("this is = = not toml");
    let hostile = |cmd: &mut assert_cmd::Command| {
        cmd.env("HW_PROFILE", "nosuch")
            .env("HW_CRM", "https://crm.invalid")
            .env("HW_TOKEN", TOKEN);
    };

    let mut cmd = offline_cmd(&env);
    hostile(&mut cmd);
    let output = cmd
        .args(["-F", "json", "self", "update", "--to", CURRENT])
        .output()
        .expect("run hw");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(exit_code::SUCCESS), "{stderr}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON stdout");
    assert_eq!(
        report,
        update_report(&current(), serde_json::Value::Null, false)
    );
    assert_eq!(stderr, format!("hint: hw {CURRENT} is up to date\n"));

    // The same environment breaks any CRM command: the config is never read for `hw self`.
    let mut cmd = offline_cmd(&env);
    hostile(&mut cmd);
    cmd.arg("menu").assert().code(exit_code::CONFIG_ERROR);
}
