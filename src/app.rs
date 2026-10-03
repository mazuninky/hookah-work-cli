//! Composition root: builds the real dependencies ([`run_system`]), resolves config,
//! profile, CRM and token, runs the handler and renders its [`Outcome`] once ([`run`]).

use std::io::Write as _;

use anyhow::Context as _;
use camino::Utf8PathBuf;
use clap::CommandFactory;
use jiff::Timestamp;
use tracing::debug;

use crate::auth::{InquirePrompter, Prompter, Secret, SecretStore, SystemKeyring, resolve_token};
use crate::cli::args::{Cli, Command};
use crate::cli::commands::{self, Outcome, Payload, auth::AuthContext};
use crate::client::{Api, Connector, HttpConnector, RetryPolicy, TokenIssuer};
use crate::config::{
    ConfigLocation, ConfigStore as _, FileConfigStore, platform_default_config_path, resolve_crm,
};
use crate::io::IoStreams;
use crate::output::{self, Transforms};

/// The parts of the process environment `hw` reads, captured once so tests can inject them.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// `HW_TOKEN`, when set and not blank.
    pub token: Option<Secret>,
    /// Config path used when neither `--config` nor `HW_CONFIG` is given.
    pub default_config_path: Option<Utf8PathBuf>,
}

impl Env {
    /// Reads `HW_TOKEN`, `XDG_CONFIG_HOME` and the home directory.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            token: std::env::var("HW_TOKEN")
                .ok()
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .map(Secret::new),
            default_config_path: platform_default_config_path(),
        }
    }
}

/// Everything [`run`] needs from the outside world.
pub struct Deps<'a> {
    /// Keyring.
    pub store: &'a dyn SecretStore,
    /// Builds API clients.
    pub connector: &'a dyn Connector,
    /// Exchanges email + password for a token.
    pub issuer: &'a dyn TokenIssuer,
    /// Asks the questions of an interactive `hw auth login`.
    pub prompter: &'a dyn Prompter,
    /// Process environment.
    pub env: &'a Env,
    /// "Now", for relative dates.
    pub now: Timestamp,
}

/// Production entry point: real streams, keyring, HTTP and clock.
pub fn run_system(cli: &Cli) -> anyhow::Result<()> {
    let mut io = IoStreams::system(cli.no_color, cli.no_pager);
    let http = HttpConnector::new(RetryPolicy::new(cli.retries));
    let env = Env::from_process();
    let prompter = InquirePrompter::new(cli.no_color, std::env::var_os("NO_COLOR").is_some());
    let deps = Deps {
        store: &SystemKeyring,
        connector: &http,
        issuer: &http,
        prompter: &prompter,
        env: &env,
        now: Timestamp::now(),
    };
    let result = run(cli, &mut io, &deps);
    io.stop_pager();
    result
}

/// Runs the parsed command against `deps` and renders the outcome into `io`. `--jq` and
/// `--template` are compiled first, so a syntax error fails before any request.
pub fn run(cli: &Cli, io: &mut IoStreams, deps: &Deps<'_>) -> anyhow::Result<()> {
    let transforms = Transforms::compile(cli.jq.as_deref(), cli.template.as_deref())?;
    let outcome = execute(cli, io, deps)?;
    render(outcome, cli, &transforms, io)
}

/// Resolves the command's dependencies and runs its handler.
pub fn execute(cli: &Cli, io: &mut IoStreams, deps: &Deps<'_>) -> anyhow::Result<Outcome> {
    let now = deps.now;
    match &cli.command {
        Command::Ref(cmd) => commands::reference::run(*cmd, connect(cli, deps, true)?.as_ref()),
        Command::Product(cmd) => commands::product::run(cmd, connect(cli, deps, true)?.as_ref()),
        Command::Storage(cmd) => {
            commands::storage::run(cmd, connect(cli, deps, true)?.as_ref(), now)
        }
        Command::Client(cmd) => commands::client::run(cmd, connect(cli, deps, true)?.as_ref()),
        Command::Booking(cmd) => commands::booking::run(cmd, connect(cli, deps, true)?.as_ref()),
        Command::Sale(cmd) => commands::sale::run(cmd, connect(cli, deps, true)?.as_ref(), now),
        Command::Hookah(cmd) => commands::hookah::run(cmd, connect(cli, deps, true)?.as_ref(), now),
        Command::Report(cmd) => commands::report::run(cmd, connect(cli, deps, true)?.as_ref(), now),
        Command::Analytics(cmd) => {
            commands::analytics::run(cmd, connect(cli, deps, true)?.as_ref(), now)
        }
        Command::Menu => commands::menu::run(connect(cli, deps, false)?.as_ref()),
        Command::Api(args) => {
            let request = commands::api::ApiRequest::from_args(args)?;
            commands::api::run(&request, connect(cli, deps, !args.no_auth)?.as_ref())
        }
        Command::Auth(cmd) => {
            let config = config_store(cli, deps)?;
            let ctx = AuthContext {
                config: &config,
                profile: cli.profile.as_deref(),
                crm: cli.crm.as_deref(),
                env_token: deps.env.token.as_ref(),
                store: deps.store,
                connector: deps.connector,
                issuer: deps.issuer,
                prompter: deps.prompter,
                interactive: io.is_stdin_tty() && io.is_stderr_tty(),
            };
            let (stdin, mut stderr) = io.stdin_and_stderr();
            commands::auth::run(cmd, &ctx, stdin, &mut *stderr)
        }
        Command::Config(cmd) => commands::config::run(
            cmd,
            &config_store(cli, deps)?,
            cli.profile.as_deref(),
            deps.store,
        ),
        Command::Completions(args) => {
            let mut script = Vec::new();
            clap_complete::generate(args.shell, &mut Cli::command(), "hw", &mut script);
            Ok(Outcome::text(String::from_utf8_lossy(&script).into_owned()))
        }
        Command::GenerateDocs(args) => commands::docs::run(args),
    }
}

fn config_store(cli: &Cli, deps: &Deps<'_>) -> anyhow::Result<FileConfigStore> {
    let location = ConfigLocation::resolve(
        cli.config.as_deref(),
        deps.env.default_config_path.as_deref(),
    )?;
    Ok(FileConfigStore::new(location))
}

/// A client for the selected CRM; with `authenticated`, a missing or out-of-scope token is an
/// auth error.
fn connect(cli: &Cli, deps: &Deps<'_>, authenticated: bool) -> anyhow::Result<Box<dyn Api>> {
    let config = config_store(cli, deps)?.load()?.unwrap_or_default();
    let selected = config.select(cli.profile.as_deref())?;
    let crm = resolve_crm(cli.crm.as_deref(), selected.map(|s| s.profile))?;
    debug!(
        "CRM {} (profile {})",
        crm.base_url(),
        selected.map_or("-", |s| s.name)
    );
    let token = if authenticated {
        let (token, source) =
            resolve_token(deps.env.token.as_ref(), selected, Some(&crm), deps.store)?;
        debug!("token from {}", source.label());
        crm.warn_if_cleartext();
        Some(token)
    } else {
        None
    };
    Ok(deps.connector.connect(crm.base_url(), token.as_ref()))
}

/// Writes the payload (JSON through `transforms` and `--format`, paged on a console
/// terminal), then the hints unless `-q`, then reports a deferred failure.
pub fn render(
    outcome: Outcome,
    cli: &Cli,
    transforms: &Transforms<'_>,
    io: &mut IoStreams,
) -> anyhow::Result<()> {
    let Outcome {
        payload,
        hints,
        failure,
    } = outcome;
    match payload {
        Payload::Json(value) => {
            let use_color = io.color_enabled();
            if cli.format == output::OutputFormat::Console {
                io.start_pager();
            }
            let written = {
                let mut stdout = io.stdout();
                output::render_value(value, cli.format, transforms, use_color, &mut *stdout)
                    .and_then(|()| stdout.flush().map_err(Into::into))
            };
            io.stop_pager();
            written?;
        }
        Payload::Text(text) => write_verbatim(io, &text)?,
        Payload::ServerText(text) => {
            let terminal = io.is_stdout_tty();
            write_verbatim(io, &output::server_text(&text, terminal))?;
        }
        Payload::Secret(secret) => write_verbatim(io, secret.expose())?,
    }
    if !cli.quiet {
        let mut stderr = io.stderr();
        for hint in &hints {
            writeln!(stderr, "hint: {hint}").context("writing hints to stderr")?;
        }
    }
    match failure {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

fn write_verbatim(io: &mut IoStreams, text: &str) -> anyhow::Result<()> {
    let mut stdout = io.stdout();
    output::write_text(&mut *stdout, text)?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{InMemoryStore, MockPrompter, MockResponse};
    use crate::error::{Error, exit_code, exit_code_for_error};
    use crate::test_util::{FakeApi, FakeConnector, FakeIssuer};
    use clap::FromArgMatches as _;
    use serde_json::json;

    struct Harness {
        _dir: tempfile::TempDir,
        env: Env,
        store: InMemoryStore,
        connector: FakeConnector,
        issuer: FakeIssuer,
        prompter: MockPrompter,
    }

    impl Harness {
        fn new(api: FakeApi, token: Option<&str>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = Utf8PathBuf::from_path_buf(dir.path().join("config.toml")).unwrap();
            Self {
                _dir: dir,
                env: Env {
                    token: token.map(Secret::new),
                    default_config_path: Some(path),
                },
                store: InMemoryStore::new(),
                connector: FakeConnector::new(api),
                issuer: FakeIssuer::status(401, "{}"),
                prompter: MockPrompter::default(),
            }
        }

        fn write_config(&self, toml: &str) {
            let path = self.env.default_config_path.as_ref().unwrap();
            std::fs::write(path, toml).unwrap();
        }

        fn run(&self, argv: &[&str], io: &mut IoStreams) -> anyhow::Result<()> {
            let cli = parse_without_env(argv);
            let deps = Deps {
                store: &self.store,
                connector: &self.connector,
                issuer: &self.issuer,
                prompter: &self.prompter,
                env: &self.env,
                now: Timestamp::UNIX_EPOCH,
            };
            run(&cli, io, &deps)
        }
    }

    /// Parses `argv` with clap's env fallbacks off, so the developer's `HW_CONFIG`,
    /// `HW_PROFILE` and `HW_CRM` cannot reach a test.
    fn parse_without_env(argv: &[&str]) -> Cli {
        let matches = Cli::command()
            .mut_args(|arg| arg.env(None))
            .try_get_matches_from(argv)
            .unwrap();
        Cli::from_arg_matches(&matches).unwrap()
    }

    #[test]
    fn harness_ignores_env_backed_flags() {
        let cli = parse_without_env(&["hw", "menu"]);
        assert_eq!((cli.config, cli.profile, cli.crm), (None, None, None));
        let argv = ["hw", "--config", "c.toml", "-p", "p", "--crm", "x", "menu"];
        let cli = parse_without_env(&argv);
        assert_eq!(cli.config, Some(Utf8PathBuf::from("c.toml")));
        assert_eq!(
            (cli.profile.as_deref(), cli.crm.as_deref()),
            (Some("p"), Some("x"))
        );
    }

    #[test]
    fn api_command_uses_env_token_and_crm_flag() {
        let harness = Harness::new(
            FakeApi::new().with_json("/api/tables", json!([{"id": 1}])),
            Some("tok"),
        );
        let mut io = IoStreams::test();
        harness
            .run(
                &["hw", "--crm", "demo", "-F", "json", "ref", "tables"],
                &mut io,
            )
            .unwrap();
        assert_eq!(io.stdout_as_string(), "[\n  {\n    \"id\": 1\n  }\n]\n");
        let (base, token) = &harness.connector.connections()[0];
        assert_eq!(base, "https://demo.hookah.work");
        assert_eq!(token.as_ref().map(Secret::expose), Some("tok"));
    }

    #[test]
    fn missing_token_is_auth_error_before_any_request() {
        let harness = Harness::new(FakeApi::new(), None);
        let err = harness
            .run(
                &["hw", "--crm", "demo", "ref", "tables"],
                &mut IoStreams::test(),
            )
            .unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::AUTH_ERROR);
        assert!(harness.connector.connections().is_empty());
    }

    #[test]
    fn jq_syntax_error_fails_before_any_request() {
        let harness = Harness::new(FakeApi::new(), Some("tok"));
        for argv in [
            ["hw", "--crm", "demo", "--jq", ".[", "ref", "tables"],
            ["hw", "--crm", "demo", "--template", "{{", "ref", "tables"],
        ] {
            let err = harness.run(&argv, &mut IoStreams::test()).unwrap_err();
            assert_eq!(exit_code_for_error(&err), exit_code::INPUT_ERROR, "{err:#}");
        }
        assert!(harness.connector.connections().is_empty());
    }

    #[test]
    fn profile_token_goes_only_to_the_profile_origin() {
        let harness = Harness::new(FakeApi::new().with_json("/api/tables", json!([])), None);
        harness.write_config(
            "[profiles.main]\ncrm = \"demo\"\ntoken_storage = \"config\"\napi_token = \"cfg\"\n",
        );

        for foreign in [
            "evil.example",
            "http://demo.hookah.work",
            "https://demo.hookah.work:8443",
        ] {
            let err = harness
                .run(
                    &["hw", "--crm", foreign, "ref", "tables"],
                    &mut IoStreams::test(),
                )
                .unwrap_err();
            assert_eq!(exit_code_for_error(&err), exit_code::AUTH_ERROR, "{err:#}");
            assert!(format!("{err:#}").contains("HW_TOKEN"), "{err:#}");
        }
        assert!(harness.connector.connections().is_empty());

        for same in [
            "https://demo.hookah.work/api",
            "https://demo.hookah.work:443/",
        ] {
            harness
                .run(
                    &["hw", "--crm", same, "ref", "tables"],
                    &mut IoStreams::test(),
                )
                .unwrap();
        }
        let connections = harness.connector.connections();
        assert_eq!(connections.len(), 2);
        for (base, token) in connections {
            assert!(base.starts_with("https://demo.hookah.work"), "{base}");
            assert_eq!(token.as_ref().map(Secret::expose), Some("cfg"));
        }
    }

    #[test]
    fn missing_crm_is_config_error() {
        let harness = Harness::new(FakeApi::new(), Some("tok"));
        let err = harness
            .run(&["hw", "menu"], &mut IoStreams::test())
            .unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::CONFIG_ERROR);
    }

    #[test]
    fn menu_connects_without_token() {
        let harness = Harness::new(
            FakeApi::new().with_json("/api/menu", json!([])),
            Some("tok"),
        );
        harness
            .run(&["hw", "--crm", "demo", "menu"], &mut IoStreams::test())
            .unwrap();
        assert_eq!(harness.connector.connections()[0].1, None);
    }

    #[test]
    fn pagination_hint_goes_to_stderr_unless_quiet() {
        let page = crate::client::Pagination {
            total_count: 432,
            page_count: 5,
            current_page: 1,
            per_page: 100,
        };
        let api = FakeApi::new().with_page("/api/clients", json!([]), page);
        let harness = Harness::new(api, Some("tok"));
        let mut io = IoStreams::test();
        harness
            .run(&["hw", "--crm", "demo", "client", "list"], &mut io)
            .unwrap();
        assert_eq!(
            io.stderr_as_string(),
            "hint: page 1 of 5 (432 total); use --all or --page <N>\n"
        );
        assert_eq!(io.stdout_as_string(), "");

        let mut quiet = IoStreams::test();
        harness
            .run(&["hw", "-q", "--crm", "demo", "client", "list"], &mut quiet)
            .unwrap();
        assert_eq!(quiet.stderr_as_string(), "");
    }

    #[test]
    fn auth_token_prints_raw_line() {
        let harness = Harness::new(FakeApi::new(), Some("raw-token"));
        let mut io = IoStreams::test();
        harness.run(&["hw", "auth", "token"], &mut io).unwrap();
        assert_eq!(io.stdout_as_string(), "raw-token\n");
    }

    #[test]
    fn auth_login_prompts_only_when_stdin_and_stderr_are_terminals() {
        let api = FakeApi::new().with_json("/api/settings", json!({}));
        let mut harness = Harness::new(api, None);
        harness.prompter = MockPrompter::new(vec![
            MockResponse::Select(0),
            MockResponse::Secret("key".into()),
            MockResponse::Select(1),
        ]);
        let argv = ["hw", "--crm", "demo", "-F", "json", "auth", "login"];

        for io in [
            IoStreams::test(),
            IoStreams::test().with_tty(true, false),
            IoStreams::test().with_tty(false, true),
        ] {
            let err = harness.run(&argv, &mut { io }).unwrap_err();
            assert_eq!(exit_code_for_error(&err), exit_code::INPUT_ERROR);
            assert!(
                format!("{err:#}").contains("https://demo.hookah.work/v2/settings/users"),
                "{err:#}"
            );
        }
        assert_eq!(harness.prompter.remaining(), 3);

        let mut io = IoStreams::test().with_tty(true, true);
        harness.run(&argv, &mut io).unwrap();
        assert_eq!(harness.prompter.remaining(), 0);
        let config_path = harness.env.default_config_path.as_ref().unwrap();
        assert_eq!(
            harness.prompter.asked().last(),
            Some(&(
                "Save the token to:".to_owned(),
                Some(format!("Config file ({config_path})"))
            ))
        );
        assert_eq!(
            io.stderr_as_string(),
            "Generate an API key at https://demo.hookah.work/v2/settings/users\n"
        );
        let report: serde_json::Value = serde_json::from_str(&io.stdout_as_string()).unwrap();
        assert_eq!(
            (&report["profile"], &report["storage"]),
            (&json!("demo"), &json!("keyring"))
        );
        assert_eq!(
            harness
                .store
                .get("hw:demo", "demo.hookah.work")
                .unwrap()
                .as_deref(),
            Some("key")
        );
    }

    #[test]
    fn auth_status_writes_report_then_fails() {
        let harness = Harness::new(FakeApi::new(), None);
        let mut io = IoStreams::test();
        let err = harness
            .run(
                &["hw", "--crm", "demo", "-F", "json", "auth", "status"],
                &mut io,
            )
            .unwrap_err();
        assert!(matches!(err.downcast_ref::<Error>(), Some(Error::Auth(_))));
        let report: serde_json::Value = serde_json::from_str(&io.stdout_as_string()).unwrap();
        assert_eq!(report["token_source"], "none");
    }

    #[test]
    fn api_text_is_written_exactly_when_piped() {
        let body = "plain\r\ntext\u{1b}[2J\n";
        let harness = Harness::new(FakeApi::new().with_text("/api/robots", body), Some("tok"));
        let mut io = IoStreams::test();
        harness
            .run(&["hw", "--crm", "demo", "api", "robots"], &mut io)
            .unwrap();
        assert_eq!(io.stdout_as_string(), body);
    }

    #[test]
    fn completions_are_text() {
        let harness = Harness::new(FakeApi::new(), None);
        let mut io = IoStreams::test();
        harness
            .run(&["hw", "completions", "bash"], &mut io)
            .unwrap();
        assert!(io.stdout_as_string().contains("_hw()"));
    }
}
