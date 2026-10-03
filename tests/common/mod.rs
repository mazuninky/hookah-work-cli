//! Shared integration-test helpers: [`TestEnv`] isolates the `hw` binary, [`App`] runs
//! `app::run` in-process with real HTTP, [`FakeCrm`] is a recording catch-all server, and
//! [`fixtures`] holds the response examples from the API documentation.
// Every test binary compiles this module but uses only part of it.
#![allow(dead_code)]

pub mod fixtures;

use std::sync::{Arc, Mutex, PoisonError};

use assert_cmd::Command;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser as _;
use httpmock::{HttpMockRequest, HttpMockResponse, MockServer};
use jiff::Timestamp;
use tempfile::TempDir;

use hw::app::{Deps, Env};
use hw::auth::{InMemoryStore, MockPrompter, MockResponse, Prompter, Secret};
use hw::cli::Cli;
use hw::client::{HttpConnector, NoSleep, RetryPolicy};
use hw::error::exit_code_for_error;
use hw::io::IoStreams;
use hw::test_util::{FakeInstaller, FakeReleases};
use hw::update::Version;

/// Token handed to commands by [`App`] and by `HW_TOKEN` in binary tests.
pub const TOKEN: &str = "tok-0123456789abcdef";

/// 2026-10-03 03:00 in Asia/Yekaterinburg (UTC+5): before the documented 06:00 business-day
/// start, so "today" is still 2026-10-02 for the CRM.
pub const NOW: &str = "2026-10-02T22:00:00Z";

fn utf8(path: std::path::PathBuf) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(path).expect("temp paths are UTF-8")
}

/// A tempdir sandbox for the `hw` binary. `HW_CONFIG` points at `<tmp>/config.toml` (created
/// empty, i.e. no profiles), `HOME`/`XDG_CONFIG_HOME` point into the tempdir, and every `HW_*`
/// plus colour/log/pager variable of the developer's shell is removed.
///
/// Binary tests never reach the OS keyring: tokens come from `HW_TOKEN` or a profile's
/// `api_token`, and no test runs `config delete`/`auth logout`, a keyring-storage login or
/// a login over a profile with `token_storage = "keyring"`.
pub struct TestEnv {
    dir: TempDir,
}

impl TestEnv {
    /// A fresh sandbox with an empty config file.
    pub fn new() -> Self {
        let env = Self {
            dir: TempDir::new().expect("create tempdir"),
        };
        env.write_config("");
        env
    }

    /// Root of the sandbox.
    pub fn path(&self) -> Utf8PathBuf {
        utf8(self.dir.path().to_path_buf())
    }

    /// The file `HW_CONFIG` points at.
    pub fn config_path(&self) -> Utf8PathBuf {
        self.path().join("config.toml")
    }

    /// Replaces the config file.
    pub fn write_config(&self, toml: &str) {
        std::fs::write(self.config_path(), toml).expect("write config");
    }

    /// The `hw` binary with the isolated environment.
    pub fn cmd(&self) -> Command {
        let mut cmd = Command::from_std(self.std_cmd());
        cmd.timeout(std::time::Duration::from_secs(60));
        cmd
    }

    /// Like [`TestEnv::cmd`], as a `std` command for tests that need `spawn`.
    pub fn std_cmd(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_hw"));
        cmd.current_dir(self.dir.path());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("HW_") {
                cmd.env_remove(&key);
            }
        }
        for key in [
            "NO_COLOR",
            "CLICOLOR_FORCE",
            "RUST_LOG",
            "PAGER",
            "XDG_CONFIG_HOME",
        ] {
            cmd.env_remove(key);
        }
        cmd.env("HOME", self.path().join("home"))
            .env("XDG_CONFIG_HOME", self.path().join("xdg"))
            .env("HW_CONFIG", self.config_path())
            .env("HW_NO_PAGER", "1");
        cmd
    }

    /// [`TestEnv::cmd`] aimed at `crm` with `HW_TOKEN` set and retries off.
    pub fn api_cmd(&self, crm: &str) -> Command {
        let mut cmd = self.cmd();
        cmd.env("HW_TOKEN", TOKEN)
            .args(["--crm", crm, "--retries", "0"]);
        cmd
    }
}

/// Result of one in-process [`App::run`].
pub struct Ran {
    /// What `app::run` returned.
    pub result: anyhow::Result<()>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

impl Ran {
    /// The process exit code this result maps to.
    pub fn exit_code(&self) -> i32 {
        self.result.as_ref().err().map_or(0, exit_code_for_error)
    }

    /// Panics with the error and stderr unless the command succeeded.
    pub fn assert_ok(&self, what: &str) {
        if let Err(err) = &self.result {
            panic!("{what} failed: {err:#}\nstderr: {}", self.stderr);
        }
    }

    /// stdout parsed as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", self.stdout))
    }
}

/// `hw::app::run` with the production [`HttpConnector`] (no retries, no sleeping), an
/// [`InMemoryStore`] keyring, a config file in a tempdir holding profile `test` → `base_url`,
/// and for `hw self` a [`FakeReleases`] whose latest release is this build plus a
/// [`FakeInstaller`] (nothing reaches GitHub, nothing is replaced).
///
/// Every invocation passes `--config`, `--profile test` and `--crm` explicitly, so the
/// `HW_CONFIG`/`HW_PROFILE`/`HW_CRM` clap reads from the test process cannot leak in.
pub struct App {
    dir: TempDir,
    base_url: String,
    /// The keyring.
    pub store: InMemoryStore,
    connector: HttpConnector,
    env: Env,
    now: Timestamp,
    /// Releases seen by `hw self`.
    pub releases: FakeReleases,
    /// Executable seen by `hw self update`.
    pub installer: FakeInstaller,
}

impl App {
    /// A runner against `base_url` with `HW_TOKEN` = [`TOKEN`] and the clock at [`NOW`].
    pub fn new(base_url: &str) -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let app = Self {
            dir,
            base_url: base_url.to_owned(),
            store: InMemoryStore::new(),
            connector: HttpConnector::new(RetryPolicy::with_sleeper(0, Arc::new(NoSleep))),
            env: Env {
                token: Some(Secret::new(TOKEN)),
                default_config_path: None,
            },
            now: NOW.parse().expect("valid timestamp"),
            releases: FakeReleases::new(Version::current().expect("release version")),
            installer: FakeInstaller::default(),
        };
        std::fs::write(
            app.config_path(),
            format!("[profiles.test]\ncrm = \"{base_url}\"\n"),
        )
        .expect("write config");
        app
    }

    /// Moves the injected clock.
    pub fn at(mut self, now: &str) -> Self {
        self.now = now.parse().expect("valid timestamp");
        self
    }

    /// Tempdir of this runner (for `generate-docs --output-dir`).
    pub fn dir(&self) -> Utf8PathBuf {
        utf8(self.dir.path().to_path_buf())
    }

    /// The config file every invocation uses.
    pub fn config_path(&self) -> Utf8PathBuf {
        self.dir().join("config.toml")
    }

    /// Runs `hw <args>` with empty stdin.
    pub fn run(&self, args: &[&str]) -> Ran {
        self.run_with_stdin(args, "")
    }

    /// Runs `hw <args>` with `stdin`.
    pub fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Ran {
        self.run_io(
            args,
            IoStreams::test_with_stdin(stdin),
            &MockPrompter::default(),
        )
    }

    /// Runs `hw <args>` as on a terminal (stdin and stderr are TTYs), answering the prompts
    /// from `script`.
    pub fn run_prompted(&self, args: &[&str], script: Vec<MockResponse>) -> Ran {
        let prompter = MockPrompter::new(script);
        let ran = self.run_io(args, IoStreams::test().with_tty(true, true), &prompter);
        assert_eq!(prompter.remaining(), 0, "unused answers for {args:?}");
        ran
    }

    fn run_io(&self, args: &[&str], mut io: IoStreams, prompter: &dyn Prompter) -> Ran {
        let config = self.config_path();
        let mut argv: Vec<&str> = vec![
            "hw",
            "--config",
            config.as_str(),
            "--profile",
            "test",
            "--crm",
            &self.base_url,
        ];
        argv.extend_from_slice(args);
        let cli = Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        let deps = Deps {
            store: &self.store,
            connector: &self.connector,
            issuer: &self.connector,
            prompter,
            env: &self.env,
            now: self.now,
            releases: &self.releases,
            installer: &self.installer,
        };
        let result = hw::app::run(&cli, &mut io, &deps);
        Ran {
            result,
            stdout: io.stdout_as_string(),
            stderr: io.stderr_as_string(),
        }
    }
}

/// One request as [`FakeCrm`] saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// HTTP method.
    pub method: String,
    /// Path without the query.
    pub path: String,
    /// Decoded query pairs in wire order.
    pub query: Vec<(String, String)>,
    /// Raw request body.
    pub body: String,
}

impl Seen {
    /// `"GET /api/tables"`.
    pub fn line(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

/// A catch-all httpmock server that answers every request plausibly (documented examples,
/// two-page pagination on paginated lists) and records it, whatever the method or path.
pub struct FakeCrm {
    server: MockServer,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeCrm {
    /// Starts the server.
    pub fn start() -> Self {
        let server = MockServer::start();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        server.mock(|when, then| {
            when.any_request();
            then.respond_with(move |req: &HttpMockRequest| {
                let seen = Seen {
                    method: req.method_str().to_owned(),
                    path: req.uri().path().to_owned(),
                    query: req.query_params(),
                    body: req.body_string(),
                };
                let response = fake_crm_reply(&seen);
                log.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(seen);
                response
            });
        });
        Self { server, seen }
    }

    /// Base URL for `--crm`.
    pub fn url(&self) -> String {
        self.server.base_url()
    }

    /// Requests received since the last call.
    pub fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

const PAGINATED: [&str; 3] = ["/api/products", "/api/clients", "/api/client/history/"];

fn fake_crm_reply(req: &Seen) -> HttpMockResponse {
    let json = |status: u16, body: &str| {
        HttpMockResponse::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(body.to_owned())
    };
    let path = req.path.as_str();
    match (req.method.as_str(), path) {
        ("POST", "/api/login") => json(200, fixtures::LOGIN).build(),
        ("POST", "/api/client/find") => json(200, fixtures::CLIENT_FIND).build(),
        ("GET", "/api/settings") => json(200, fixtures::SETTINGS).build(),
        ("GET", "/api/tables") => json(200, fixtures::TABLES).build(),
        ("GET", "/api/timetable") => json(200, "{}").build(),
        ("GET", _) if path.starts_with("/api/booking/") && path != "/api/booking/my" => {
            json(200, r#"{"id": 51}"#).build()
        }
        ("GET", _) if PAGINATED.iter().any(|p| path.starts_with(p)) => {
            let page = req
                .query
                .iter()
                .find(|(k, _)| k == "page")
                .map_or("1", |(_, v)| v.as_str());
            json(200, &format!(r#"[{{"id": {page}}}]"#))
                .header("X-Pagination-Total-Count", "2")
                .header("X-Pagination-Page-Count", "2")
                .header("X-Pagination-Current-Page", page)
                .header("X-Pagination-Per-Page", "1")
                .build()
        }
        ("GET", _) if path.starts_with("/api/") => json(200, "[]").build(),
        _ => json(405, r#"{"name":"Method Not Allowed","message":"not here"}"#).build(),
    }
}

/// Sorted lines of a multi-line text, for order-insensitive comparisons.
pub fn sorted_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    lines
}

/// A loopback port with nothing listening on it (bound, then released).
pub fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    listener.local_addr().expect("local addr").port()
}

/// Reads a UTF-8 file.
pub fn read(path: &Utf8Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}
