//! `hw auth` — login, status, token and logout. Login prompts only when the composition
//! root reports a terminal; otherwise secrets come from stdin via `--with-token` or
//! `--password-stdin`.

use std::io::{BufRead, Write};

use anyhow::Context as _;
use serde_json::{Map, Value, json};

use super::Outcome;
use crate::auth::{
    PromptError, Prompter, Secret, SecretStore, non_blank, resolve_token, service_name,
};
use crate::cli::args::{AuthCommand, AuthLoginArgs};
use crate::client::{Connector, Login, TokenIssuer};
use crate::config::{
    API_KEY_URL_TEMPLATE, Config, ConfigStore, Crm, TokenStorage, missing_crm, optional_crm,
    resolve_crm,
};
use crate::error::Error;

/// Everything the auth handlers depend on.
pub struct AuthContext<'a> {
    /// The configuration file.
    pub config: &'a dyn ConfigStore,
    /// `--profile`/`HW_PROFILE`.
    pub profile: Option<&'a str>,
    /// `--crm`/`HW_CRM`.
    pub crm: Option<&'a str>,
    /// `HW_TOKEN`.
    pub env_token: Option<&'a Secret>,
    /// Keyring.
    pub store: &'a dyn SecretStore,
    /// Builds clients for token checks.
    pub connector: &'a dyn Connector,
    /// Exchanges email + password for a token.
    pub issuer: &'a dyn TokenIssuer,
    /// Asks the interactive login questions.
    pub prompter: &'a dyn Prompter,
    /// Whether stdin and stderr are terminals, so `login` without flags may prompt.
    pub interactive: bool,
}

const CRM_PROMPT: &str = "CRM (demo, demo.hookah.work or a URL):";
const METHOD_PROMPT: &str = "Log in with:";
const METHODS: [&str; 2] = ["API key", "Email and password"];
const API_KEY_PROMPT: &str = "API key:";
const EMAIL_PROMPT: &str = "Email:";
const PASSWORD_PROMPT: &str = "Password:";
const STORAGE_PROMPT: &str = "Save the token to:";
const KEYRING_OPTION: &str = "OS keyring";

/// Runs `hw auth <command>`; only `login` reads `stdin` or writes `stderr`.
pub fn run(
    command: &AuthCommand,
    ctx: &AuthContext<'_>,
    stdin: &mut dyn BufRead,
    stderr: &mut dyn Write,
) -> anyhow::Result<Outcome> {
    match command {
        AuthCommand::Login(args) => login(args, ctx, stdin, stderr),
        AuthCommand::Status => status(ctx),
        AuthCommand::Token => token(ctx),
        AuthCommand::Logout => logout(ctx),
    }
}

/// Where `login` gets the secret from.
#[derive(Clone, Copy)]
enum Method<'a> {
    /// `--with-token`: a token line on stdin.
    StdinToken,
    /// `--email <EMAIL> --password-stdin`: a password line on stdin.
    StdinPassword(&'a str),
    /// Questions on the terminal.
    Prompt,
}

fn login(
    args: &AuthLoginArgs,
    ctx: &AuthContext<'_>,
    stdin: &mut dyn BufRead,
    stderr: &mut dyn Write,
) -> anyhow::Result<Outcome> {
    // A prompt that finds no terminal fails like a login started without one (exit 5).
    run_login(args, ctx, stdin, stderr).map_err(|err| match err.downcast_ref::<PromptError>() {
        Some(PromptError::NotTerminal) => no_method_error(ctx).into(),
        _ => err,
    })
}

fn run_login(
    args: &AuthLoginArgs,
    ctx: &AuthContext<'_>,
    stdin: &mut dyn BufRead,
    stderr: &mut dyn Write,
) -> anyhow::Result<Outcome> {
    let method = match (args.with_token, args.email.as_deref(), args.password_stdin) {
        (true, _, _) => Method::StdinToken,
        (false, Some(email), true) => Method::StdinPassword(email),
        _ if ctx.interactive => Method::Prompt,
        _ => return Err(no_method_error(ctx).into()),
    };

    let mut config = ctx.config.load_for_update()?;
    let (name, known_crm) = login_target(&config, ctx)?;
    let (crm, crm_input) = match known_crm {
        Some(crm) => (crm, crm_flag(ctx).map(str::to_owned)),
        None if matches!(method, Method::Prompt) => {
            let input = ctx.prompter.text(CRM_PROMPT, None, valid_crm)?;
            let input = input.trim().to_owned();
            (Crm::parse(&input)?, Some(input))
        }
        None => return Err(missing_crm().into()),
    };
    let name = name.unwrap_or_else(|| crm.label().to_owned());
    crm.warn_if_cleartext();

    let (token, login) = match method {
        Method::StdinToken => (read_line(stdin, "token", true)?, None),
        Method::StdinPassword(email) => {
            let password = read_line(stdin, "password", false)?;
            exchange(ctx, &crm, email, &password)?
        }
        Method::Prompt => {
            let stored_email = config.profiles.get(&name).and_then(|p| p.email.as_deref());
            prompt_credentials(ctx, &crm, stored_email, stderr)?
        }
    };

    ctx.connector
        .connect(crm.base_url(), Some(&token))
        .get("/api/settings", &[])
        .with_context(|| format!("checking the token against {}", crm.base_url()))?;

    let previous = config.profiles.get(&name);
    let current = previous.map(|p| p.token_storage).unwrap_or_default();
    let storage = match (args.storage, method) {
        (Some(storage), _) => storage,
        (None, Method::Prompt) => prompt_storage(ctx, current)?,
        (None, _) => current,
    };
    // The entry `hw:<profile>` lives under the host of the CRM the profile had then. Only a
    // profile that recorded keyring storage has one, so other logins never reach the keyring.
    let stale_keyring_host = previous
        .filter(|p| storage == TokenStorage::Config && p.token_storage == TokenStorage::Keyring)
        .map(|p| match p.crm.as_deref().map(Crm::parse) {
            Some(Ok(old)) => old.host().to_owned(),
            _ => crm.host().to_owned(),
        });

    if storage == TokenStorage::Keyring {
        ctx.store
            .set(&service_name(&name), crm.host(), token.expose())
            .map_err(|err| {
                Error::Keyring(format!(
                    "cannot store the token in the OS keyring ({err}); retry with --storage config"
                ))
            })?;
    }

    let profile = config.profiles.entry(name.clone()).or_default();
    if let Some(input) = crm_input {
        profile.crm = Some(input);
    }
    if let Some(email) = login.as_ref().map(|l| &l.email).filter(|e| !e.is_empty()) {
        profile.email = Some(email.clone());
    }
    profile.token_storage = storage;
    profile.api_token = match storage {
        TokenStorage::Config => Some(token.expose().to_owned()),
        TokenStorage::Keyring => None,
    };
    let email = profile.email.clone();
    if config.default_profile_name().is_none() {
        config.default_profile = Some(name.clone());
    }
    ctx.config.save(&config)?;

    let mut report = Map::new();
    report.insert("profile".into(), json!(name));
    report.insert("crm".into(), json!(crm.base_url()));
    report.insert("storage".into(), json!(storage.label()));
    if let Some(email) = email {
        report.insert("email".into(), json!(email));
    }
    if let Some(login) = login.filter(|l| !l.name.is_empty()) {
        report.insert("name".into(), json!(login.name));
    }
    let mut outcome = Outcome::json(Value::Object(report));
    // After the save, so a failed cleanup never costs the new token; the config copy wins.
    if let Some(host) = stale_keyring_host
        && let Err(err) = ctx.store.delete(&service_name(&name), &host)
    {
        outcome = outcome.with_hint(format!(
            "could not remove the profile's old token from the OS keyring ({err}); the token \
             in the config file takes precedence"
        ));
    }
    Ok(outcome)
}

/// Asks where to save the token, the config file first, with the cursor on `current`.
fn prompt_storage(ctx: &AuthContext<'_>, current: TokenStorage) -> anyhow::Result<TokenStorage> {
    let config_file = format!("Config file ({})", ctx.config.path());
    let options = [config_file.as_str(), KEYRING_OPTION];
    let preselected = usize::from(current == TokenStorage::Keyring);
    let index = ctx.prompter.select(STORAGE_PROMPT, &options, preselected)?;
    Ok(if index == 0 {
        TokenStorage::Config
    } else {
        TokenStorage::Keyring
    })
}

/// Asks for the method, then for an API key (after pointing at the page that issues it) or
/// for an email and password.
fn prompt_credentials(
    ctx: &AuthContext<'_>,
    crm: &Crm,
    stored_email: Option<&str>,
    stderr: &mut dyn Write,
) -> anyhow::Result<(Secret, Option<Login>)> {
    if ctx.prompter.select(METHOD_PROMPT, &METHODS, 0)? == 0 {
        writeln!(stderr, "Generate an API key at {}", crm.api_key_url())
            .and_then(|()| stderr.flush())
            .context("writing to stderr")?;
        let key = ctx.prompter.secret(API_KEY_PROMPT)?;
        return Ok((Secret::new(key.expose().trim()), None));
    }
    let email = ctx.prompter.text(EMAIL_PROMPT, stored_email, non_blank)?;
    let password = ctx.prompter.secret(PASSWORD_PROMPT)?;
    exchange(ctx, crm, email.trim(), &password)
}

/// `POST /api/login`; the returned [`Login`] carries `email` when the response has none.
fn exchange(
    ctx: &AuthContext<'_>,
    crm: &Crm,
    email: &str,
    password: &Secret,
) -> anyhow::Result<(Secret, Option<Login>)> {
    let mut login = ctx
        .issuer
        .login(crm.base_url(), email, password)
        .with_context(|| format!("logging in to {} as {email}", crm.base_url()))?;
    if login.email.is_empty() {
        email.clone_into(&mut login.email);
    }
    Ok((login.token.clone(), Some(login)))
}

fn valid_crm(input: &str) -> Result<(), String> {
    match Crm::parse(input) {
        Ok(_) => Ok(()),
        Err(Error::InvalidInput(why)) => Err(why),
        Err(other) => Err(other.to_string()),
    }
}

/// The error for `login` without flags and without a terminal, naming the API key page of
/// the CRM when it is already known.
fn no_method_error(ctx: &AuthContext<'_>) -> Error {
    let config = ctx.config.load().ok().flatten().unwrap_or_default();
    let url = login_target(&config, ctx)
        .ok()
        .and_then(|(_, crm)| crm)
        .map_or_else(|| API_KEY_URL_TEMPLATE.to_owned(), |crm| crm.api_key_url());
    Error::InvalidInput(format!(
        "not a terminal, so hw cannot ask for credentials; pipe an API key on stdin with \
         --with-token (generate one at {url}), or pass --email <EMAIL> --password-stdin and \
         pipe the password"
    ))
}

fn crm_flag<'a>(ctx: &AuthContext<'a>) -> Option<&'a str> {
    ctx.crm.map(str::trim).filter(|c| !c.is_empty())
}

/// Profile name and CRM for `login`: `--profile` (CRM from `--crm` or that profile), else
/// the `--crm` label, else the selected existing profile. `None` when nothing names it.
fn login_target(
    config: &Config,
    ctx: &AuthContext<'_>,
) -> anyhow::Result<(Option<String>, Option<Crm>)> {
    let requested = ctx.profile.map(str::trim).filter(|p| !p.is_empty());
    if let Some(name) = requested {
        let crm = optional_crm(crm_flag(ctx), config.profiles.get(name))?;
        return Ok((Some(name.to_owned()), crm));
    }
    if let Some(flag) = crm_flag(ctx) {
        let crm = Crm::parse(flag)?;
        return Ok((Some(crm.label().to_owned()), Some(crm)));
    }
    let selected = config.select(None)?;
    let crm = optional_crm(None, selected.map(|s| s.profile))?;
    Ok((selected.map(|s| s.name.to_owned()), crm))
}

fn status(ctx: &AuthContext<'_>) -> anyhow::Result<Outcome> {
    let config = ctx.config.load()?.unwrap_or_default();
    let selected = config.select(ctx.profile)?;
    let crm = resolve_crm(ctx.crm, selected.map(|s| s.profile))?;
    let resolved = resolve_token(ctx.env_token, selected, Some(&crm), ctx.store);

    let mut report = Map::new();
    report.insert("profile".into(), json!(selected.map(|s| s.name)));
    report.insert("crm".into(), json!(crm.base_url()));
    if let Some(email) = selected.and_then(|s| s.profile.email.as_deref()) {
        report.insert("email".into(), json!(email));
    }

    let (token, source) = match resolved {
        Ok(found) => found,
        Err(err) => {
            report.insert("token_source".into(), json!("none"));
            report.insert("token_status".into(), json!("unchecked"));
            return Ok(Outcome::json(Value::Object(report)).then_fail(err));
        }
    };
    report.insert("token_source".into(), json!(source.label()));

    crm.warn_if_cleartext();
    let check = ctx
        .connector
        .connect(crm.base_url(), Some(&token))
        .get("/api/settings", &[]);
    // Only a rejection says anything about the token; other failures leave it unchecked.
    let (verdict, failure) = match check {
        Ok(_) => ("valid", None),
        Err(Error::Auth(message)) => (
            "invalid",
            Some(Error::Auth(format!(
                "the {} token was rejected by {}: {message}",
                source.label(),
                crm.base_url()
            ))),
        ),
        Err(other) => ("unchecked", Some(other)),
    };
    report.insert("token_status".into(), json!(verdict));
    let outcome = Outcome::json(Value::Object(report));
    Ok(match failure {
        Some(err) => outcome.then_fail(err),
        None => outcome,
    })
}

fn token(ctx: &AuthContext<'_>) -> anyhow::Result<Outcome> {
    let config = ctx.config.load()?.unwrap_or_default();
    let selected = config.select(ctx.profile)?;
    let crm = optional_crm(ctx.crm, selected.map(|s| s.profile))?;
    let (token, _) = resolve_token(ctx.env_token, selected, crm.as_ref(), ctx.store)?;
    Ok(Outcome::secret(token))
}

/// Forgets the selected profile's token: the config `api_token` and the keyring entry under
/// the profile's own CRM host, plus the `--crm` host when that differs.
fn logout(ctx: &AuthContext<'_>) -> anyhow::Result<Outcome> {
    let mut config = ctx.config.load()?.unwrap_or_default();
    let name = config
        .select(ctx.profile)?
        .map(|s| s.name.to_owned())
        .ok_or_else(|| Error::Config("no profile selected; pass --profile <NAME>".into()))?;
    let Some(profile) = config.profiles.get_mut(&name) else {
        return Err(crate::config::profile_not_found(&name).into());
    };
    let override_crm = ctx
        .crm
        .filter(|c| !c.trim().is_empty())
        .map(Crm::parse)
        .transpose()?;
    // A profile CRM that no longer parses has no keyring entry worth chasing.
    let own_crm = profile.crm.as_deref().and_then(|c| Crm::parse(c).ok());
    let mut hosts: Vec<&str> = own_crm.iter().chain(&override_crm).map(Crm::host).collect();
    hosts.dedup();
    for host in hosts {
        ctx.store.delete(&service_name(&name), host)?;
    }
    if profile.api_token.take().is_some() {
        ctx.config.save(&config)?;
    }
    Ok(Outcome::json(json!({"profile": name, "logged_out": true})))
}

/// One line of stdin; tokens are trimmed, passwords only lose the line terminator.
fn read_line(stdin: &mut dyn BufRead, what: &str, trim: bool) -> anyhow::Result<Secret> {
    let mut line = String::new();
    stdin
        .read_line(&mut line)
        .with_context(|| format!("reading the {what} from stdin"))?;
    let value = if trim {
        line.trim()
    } else {
        line.trim_end_matches(['\n', '\r'])
    };
    if value.trim().is_empty() {
        return Err(Error::InvalidInput(format!(
            "stdin is empty; pipe the {what} in, e.g. `echo \"$TOKEN\" | hw auth login --with-token`"
        ))
        .into());
    }
    Ok(Secret::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{InMemoryStore, MockPrompter, MockResponse};
    use crate::config::{InMemoryConfigStore, Profile};
    use crate::error::{exit_code, exit_code_for_error};
    use crate::test_util::{FakeApi, FakeConnector, FakeIssuer};
    use std::io::Cursor;

    const SETTINGS: &str = "/api/settings";

    struct Env {
        config: InMemoryConfigStore,
        store: InMemoryStore,
        connector: FakeConnector,
        issuer: FakeIssuer,
        prompter: MockPrompter,
        interactive: bool,
    }

    impl Env {
        fn new(api: FakeApi) -> Self {
            Self {
                config: InMemoryConfigStore::new(None),
                store: InMemoryStore::new(),
                connector: FakeConnector::new(api),
                issuer: FakeIssuer::ok(Login {
                    email: "integration@example.com".into(),
                    name: "Интеграция".into(),
                    token: Secret::new("issued-token"),
                }),
                prompter: MockPrompter::default(),
                interactive: false,
            }
        }

        /// A terminal session answering from `script`.
        fn interactive(api: FakeApi, script: Vec<MockResponse>) -> Self {
            Self {
                prompter: MockPrompter::new(script),
                interactive: true,
                ..Self::new(api)
            }
        }

        fn ctx<'a>(
            &'a self,
            profile: Option<&'a str>,
            crm: Option<&'a str>,
            env: Option<&'a Secret>,
        ) -> AuthContext<'a> {
            AuthContext {
                config: &self.config,
                profile,
                crm,
                env_token: env,
                store: &self.store,
                connector: &self.connector,
                issuer: &self.issuer,
                prompter: &self.prompter,
                interactive: self.interactive,
            }
        }

        fn config(&self) -> Config {
            self.config.config().unwrap_or_default()
        }

        /// An existing default profile `demo` whose `old-token` is in `storage`.
        fn with_profile(mut self, storage: TokenStorage) -> Self {
            let mut config = Config {
                default_profile: Some("demo".into()),
                ..Config::default()
            };
            let profile = Profile {
                crm: Some("demo".into()),
                token_storage: storage,
                api_token: (storage == TokenStorage::Config).then(|| "old-token".into()),
                ..Profile::default()
            };
            config.profiles.insert("demo".into(), profile);
            if storage == TokenStorage::Keyring {
                self.store
                    .set("hw:demo", "demo.hookah.work", "old-token")
                    .unwrap();
            }
            self.config = InMemoryConfigStore::new(Some(config));
            self
        }

        /// Profile `demo`'s `token_storage`, `api_token` and keyring entry.
        fn demo_token(&self) -> (TokenStorage, Option<String>, Option<String>) {
            let profile = self.config().profiles.remove("demo").unwrap();
            let keyring = self.store.get("hw:demo", "demo.hookah.work").unwrap();
            (profile.token_storage, profile.api_token, keyring)
        }
    }

    /// What [`Env::demo_token`] holds after `token` was saved to `storage`.
    fn saved_to(
        storage: TokenStorage,
        token: &str,
    ) -> (TokenStorage, Option<String>, Option<String>) {
        match storage {
            TokenStorage::Config => (storage, Some(token.to_owned()), None),
            TokenStorage::Keyring => (storage, None, Some(token.to_owned())),
        }
    }

    /// The prompted API key `token`, then `storage` picked at the storage prompt.
    fn key_then_storage(token: &str, storage: TokenStorage) -> Vec<MockResponse> {
        vec![
            MockResponse::Select(0),
            MockResponse::Secret(token.into()),
            MockResponse::Select(usize::from(storage == TokenStorage::Keyring)),
        ]
    }

    /// A keyring where every write and delete fails.
    struct BrokenKeyring;

    impl SecretStore for BrokenKeyring {
        fn get(&self, _: &str, _: &str) -> crate::error::Result<Option<String>> {
            Ok(None)
        }

        fn set(&self, _: &str, _: &str, _: &str) -> crate::error::Result<()> {
            Err(Error::Keyring("locked".into()))
        }

        fn delete(&self, _: &str, _: &str) -> crate::error::Result<()> {
            Err(Error::Keyring("locked".into()))
        }
    }

    fn ok_api() -> FakeApi {
        FakeApi::new().with_json(SETTINGS, serde_json::json!({"midnight": "6"}))
    }

    fn login_args(
        with_token: bool,
        email: Option<&str>,
        storage: Option<TokenStorage>,
    ) -> AuthLoginArgs {
        AuthLoginArgs {
            with_token,
            email: email.map(String::from),
            password_stdin: email.is_some(),
            storage,
        }
    }

    /// The storage prompt's first option, for the in-memory config store.
    fn config_option() -> String {
        format!("Config file ({})", InMemoryConfigStore::PATH)
    }

    fn stdin(text: &str) -> Cursor<Vec<u8>> {
        Cursor::new(text.as_bytes().to_vec())
    }

    /// `login` reading `stdin`, with stderr discarded.
    fn login_piped(
        args: &AuthLoginArgs,
        ctx: &AuthContext<'_>,
        stdin: &mut Cursor<Vec<u8>>,
    ) -> anyhow::Result<Outcome> {
        login(args, ctx, stdin, &mut std::io::sink())
    }

    /// `login` on a terminal: empty stdin, stderr captured.
    fn login_prompted(
        args: &AuthLoginArgs,
        ctx: &AuthContext<'_>,
    ) -> (anyhow::Result<Outcome>, String) {
        let mut stderr = Vec::new();
        let result = login(args, ctx, &mut stdin(""), &mut stderr);
        (result, String::from_utf8(stderr).unwrap())
    }

    #[test]
    fn login_with_token_validates_and_stores_in_keyring() {
        let env = Env::new(ok_api());
        let args = login_args(true, None, Some(TokenStorage::Keyring));
        let outcome = login_piped(
            &args,
            &env.ctx(None, Some("demo"), None),
            &mut stdin(" tok-1 \n"),
        )
        .unwrap();

        assert_eq!(
            outcome.json_value().unwrap(),
            &serde_json::json!({"profile": "demo", "crm": "https://demo.hookah.work", "storage": "keyring"})
        );
        assert_eq!(
            env.store
                .get("hw:demo", "demo.hookah.work")
                .unwrap()
                .as_deref(),
            Some("tok-1")
        );
        let (base, token) = &env.connector.connections()[0];
        assert_eq!(
            (base.as_str(), token.as_ref().unwrap().expose()),
            ("https://demo.hookah.work", "tok-1")
        );
        assert_eq!(
            env.connector.api().requests(),
            vec![(SETTINGS.to_owned(), vec![])]
        );
        let config = env.config();
        assert_eq!(config.default_profile.as_deref(), Some("demo"));
        assert_eq!(config.profiles["demo"].api_token, None);
    }

    #[test]
    fn login_with_password_uses_issuer_and_config_storage() {
        let env = Env::new(ok_api());
        let args = login_args(false, Some("integration@example.com"), None);
        let outcome = login_piped(
            &args,
            &env.ctx(Some("main"), Some("demo"), None),
            &mut stdin("p@ss word\n"),
        )
        .unwrap();

        let calls = env.issuer.calls();
        assert_eq!(calls[0].2.expose(), "p@ss word");
        assert_eq!(outcome.json_value().unwrap()["name"], "Интеграция");
        assert_eq!(outcome.json_value().unwrap()["storage"], "config");
        let profile = &env.config().profiles["main"];
        assert_eq!(profile.token_storage, TokenStorage::Config);
        assert_eq!(profile.api_token.as_deref(), Some("issued-token"));
        assert_eq!(profile.email.as_deref(), Some("integration@example.com"));
        assert!(env.store.is_empty());
    }

    fn invalid_input_message(err: &anyhow::Error) -> &str {
        match err.downcast_ref::<Error>() {
            Some(Error::InvalidInput(message)) => message,
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn login_without_method_or_terminal_names_flags_and_key_page() {
        let env = Env::new(ok_api());
        let args = login_args(false, None, None);
        let err =
            login_piped(&args, &env.ctx(None, Some("demo"), None), &mut stdin("")).unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::INPUT_ERROR);
        let message = invalid_input_message(&err);
        for part in [
            "--with-token",
            "--password-stdin",
            "https://demo.hookah.work/v2/settings/users",
        ] {
            assert!(message.contains(part), "{message}");
        }

        let err = login_piped(&args, &env.ctx(None, None, None), &mut stdin("")).unwrap_err();
        assert!(
            invalid_input_message(&err).contains(API_KEY_URL_TEMPLATE),
            "{err:#}"
        );
        assert!(env.connector.connections().is_empty());
        assert_eq!(env.config.saves(), 0);
    }

    #[test]
    fn prompted_api_key_is_announced_checked_and_saved() {
        let env = Env::interactive(
            ok_api(),
            vec![
                MockResponse::Select(0),
                MockResponse::Secret(" key-1 ".into()),
                MockResponse::Select(1),
            ],
        );
        let args = login_args(false, None, None);
        let (result, stderr) = login_prompted(&args, &env.ctx(None, Some("demo"), None));

        assert_eq!(
            result.unwrap().json_value().unwrap(),
            &serde_json::json!({"profile": "demo", "crm": "https://demo.hookah.work", "storage": "keyring"})
        );
        assert_eq!(
            stderr,
            "Generate an API key at https://demo.hookah.work/v2/settings/users\n"
        );
        assert_eq!(
            env.prompter.asked(),
            vec![
                (METHOD_PROMPT.to_owned(), Some(METHODS[0].to_owned())),
                (API_KEY_PROMPT.to_owned(), None),
                (STORAGE_PROMPT.to_owned(), Some(config_option())),
            ]
        );
        let (base, token) = &env.connector.connections()[0];
        assert_eq!(
            (base.as_str(), token.as_ref().unwrap().expose()),
            ("https://demo.hookah.work", "key-1")
        );
        assert_eq!(
            env.store
                .get("hw:demo", "demo.hookah.work")
                .unwrap()
                .as_deref(),
            Some("key-1")
        );
        assert!(env.issuer.calls().is_empty());
        assert_eq!(env.config().profiles["demo"].api_token, None);
    }

    #[test]
    fn prompted_password_defaults_to_the_stored_email() {
        let mut env = Env::interactive(
            ok_api(),
            vec![
                MockResponse::Select(1),
                MockResponse::Text(String::new()),
                MockResponse::Secret("p@ss word".into()),
            ],
        );
        let mut config = Config::default();
        config.profiles.insert(
            "main".into(),
            Profile {
                crm: Some("demo".into()),
                email: Some("stored@example.com".into()),
                ..Profile::default()
            },
        );
        env.config = InMemoryConfigStore::new(Some(config));
        let args = login_args(false, None, Some(TokenStorage::Config));
        let (result, stderr) = login_prompted(&args, &env.ctx(None, None, None));

        assert_eq!(result.unwrap().json_value().unwrap()["profile"], "main");
        assert_eq!(stderr, "", "no API key hint on the password path");
        assert_eq!(
            env.prompter.asked()[1],
            (
                EMAIL_PROMPT.to_owned(),
                Some("stored@example.com".to_owned())
            )
        );
        let calls = env.issuer.calls();
        assert_eq!(
            (
                calls[0].0.as_str(),
                calls[0].1.as_str(),
                calls[0].2.expose()
            ),
            (
                "https://demo.hookah.work",
                "stored@example.com",
                "p@ss word"
            )
        );
        let profile = &env.config().profiles["main"];
        assert_eq!(profile.api_token.as_deref(), Some("issued-token"));
        assert_eq!(profile.email.as_deref(), Some("integration@example.com"));
        assert_eq!(profile.crm.as_deref(), Some("demo"));
    }

    #[test]
    fn missing_crm_is_prompted_and_reasked_until_valid() {
        let env = Env::interactive(
            ok_api(),
            vec![
                MockResponse::Text("bad crm!".into()),
                MockResponse::Text(" Demo ".into()),
                MockResponse::Select(0),
                MockResponse::Secret("key-2".into()),
            ],
        );
        let args = login_args(false, None, Some(TokenStorage::Config));
        let (result, stderr) = login_prompted(&args, &env.ctx(None, None, None));

        assert_eq!(result.unwrap().json_value().unwrap()["profile"], "demo");
        assert_eq!(env.prompter.asked()[0], (CRM_PROMPT.to_owned(), None));
        let rejections = env.prompter.rejections();
        assert_eq!(rejections.len(), 1);
        assert!(
            rejections[0].starts_with("invalid CRM 'bad crm!'"),
            "{rejections:?}"
        );
        assert!(stderr.contains("https://demo.hookah.work/v2/settings/users"));
        let config = env.config();
        let profile = &config.profiles["demo"];
        assert_eq!(profile.crm.as_deref(), Some("Demo"));
        assert_eq!(profile.api_token.as_deref(), Some("key-2"));
        assert_eq!(config.default_profile.as_deref(), Some("demo"));
    }

    #[test]
    fn cancelled_prompt_saves_and_sends_nothing() {
        for script in [
            vec![MockResponse::Cancel],
            vec![MockResponse::Select(0), MockResponse::Cancel],
            vec![
                MockResponse::Select(1),
                MockResponse::Text("a@b.c".into()),
                MockResponse::Cancel,
            ],
        ] {
            let env = Env::interactive(ok_api(), script);
            let args = login_args(false, None, Some(TokenStorage::Config));
            let (result, _) = login_prompted(&args, &env.ctx(None, Some("demo"), None));
            let err = result.unwrap_err();
            assert_eq!(format!("{err:#}"), "login cancelled");
            assert_eq!(exit_code_for_error(&err), exit_code::RUNTIME_ERROR);
            assert_eq!(env.config.saves(), 0);
            assert!(env.store.is_empty());
            assert!(env.connector.connections().is_empty());
            assert!(env.issuer.calls().is_empty());
        }
    }

    /// Nothing was written to the config or keyring, and nothing went to the CRM.
    fn assert_untouched(env: &Env) {
        assert_eq!(env.config.saves(), 0);
        assert!(env.store.is_empty());
        assert!(env.connector.connections().is_empty());
        assert!(env.issuer.calls().is_empty());
    }

    #[test]
    fn cancel_at_the_crm_prompt_saves_and_sends_nothing() {
        let env = Env::interactive(ok_api(), vec![MockResponse::Cancel]);
        let args = login_args(false, None, Some(TokenStorage::Config));
        let (result, stderr) = login_prompted(&args, &env.ctx(None, None, None));
        let err = result.unwrap_err();
        assert_eq!(format!("{err:#}"), "login cancelled");
        assert_eq!(exit_code_for_error(&err), exit_code::RUNTIME_ERROR);
        assert_eq!(env.prompter.asked(), vec![(CRM_PROMPT.to_owned(), None)]);
        assert_eq!(stderr, "");
        assert_untouched(&env);
    }

    #[test]
    fn failed_prompt_saves_and_sends_nothing() {
        for (crm, script) in [
            (None, vec![]),
            (Some("demo"), vec![MockResponse::Select(0)]),
            (
                Some("demo"),
                vec![MockResponse::Select(1), MockResponse::Text("a@b.c".into())],
            ),
        ] {
            let env = Env::interactive(ok_api(), script);
            let args = login_args(false, None, Some(TokenStorage::Config));
            let (result, _) = login_prompted(&args, &env.ctx(None, crm, None));
            let err = result.unwrap_err();
            assert!(
                matches!(
                    err.downcast_ref::<PromptError>(),
                    Some(PromptError::Failed(_))
                ),
                "{err:#}"
            );
            assert!(format!("{err:#}").contains("no answer scripted"), "{err:#}");
            assert_eq!(exit_code_for_error(&err), exit_code::RUNTIME_ERROR);
            assert_untouched(&env);
        }
    }

    #[test]
    fn lost_terminal_is_the_no_terminal_input_error() {
        for (crm, script, url) in [
            (None, vec![MockResponse::NotTerminal], API_KEY_URL_TEMPLATE),
            (
                Some("demo"),
                vec![MockResponse::NotTerminal],
                "https://demo.hookah.work/v2/settings/users",
            ),
            (
                Some("demo"),
                vec![MockResponse::Select(0), MockResponse::NotTerminal],
                "https://demo.hookah.work/v2/settings/users",
            ),
        ] {
            let env = Env::interactive(ok_api(), script);
            let args = login_args(false, None, Some(TokenStorage::Config));
            let (result, _) = login_prompted(&args, &env.ctx(None, crm, None));
            let err = result.unwrap_err();
            assert_eq!(exit_code_for_error(&err), exit_code::INPUT_ERROR);
            let message = invalid_input_message(&err);
            for part in ["--with-token", "--password-stdin", url] {
                assert!(message.contains(part), "{message}");
            }
            assert_untouched(&env);
        }
    }

    #[test]
    fn control_characters_in_the_crm_are_invalid_input() {
        let env = Env::new(ok_api());
        let args = login_args(true, None, Some(TokenStorage::Config));
        let crm = Some("https://demo.hookah.work/\u{1b}]8;;x\u{7}");
        let err = login_piped(&args, &env.ctx(None, crm, None), &mut stdin("tok\n")).unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::INPUT_ERROR);
        assert!(!format!("{err:#}").chars().any(char::is_control), "{err:#}");
        assert_untouched(&env);

        let env = Env::interactive(
            ok_api(),
            vec![
                MockResponse::Text("demo.hookah.work/\u{1b}[2J".into()),
                MockResponse::Text("demo".into()),
                MockResponse::Select(0),
                MockResponse::Secret("key".into()),
            ],
        );
        let args = login_args(false, None, Some(TokenStorage::Config));
        let (result, stderr) = login_prompted(&args, &env.ctx(None, None, None));
        result.unwrap();
        let rejections = env.prompter.rejections();
        assert_eq!(rejections.len(), 1);
        assert!(
            rejections[0].contains("control characters"),
            "{rejections:?}"
        );
        assert!(
            !rejections[0].chars().any(char::is_control),
            "{rejections:?}"
        );
        assert!(
            !stderr.chars().any(|c| c.is_control() && c != '\n'),
            "{stderr:?}"
        );
    }

    #[test]
    fn flags_never_prompt_on_a_terminal() {
        let env = Env::interactive(ok_api(), vec![]);
        let args = login_args(true, None, None);
        login_piped(
            &args,
            &env.ctx(None, Some("demo"), None),
            &mut stdin("tok\n"),
        )
        .unwrap();
        assert!(env.prompter.asked().is_empty());

        let env = Env::interactive(ok_api(), vec![]);
        let err = login_piped(&args, &env.ctx(None, None, None), &mut stdin("tok\n"));
        assert!(
            matches!(
                err.unwrap_err().downcast_ref::<Error>(),
                Some(Error::Config(_))
            ),
            "a missing CRM is still an error with --with-token"
        );
        assert!(env.prompter.asked().is_empty());
    }

    #[test]
    fn prompted_storage_is_honoured() {
        for storage in [TokenStorage::Config, TokenStorage::Keyring] {
            let env = Env::interactive(ok_api(), key_then_storage("key", storage));
            let args = login_args(false, None, None);
            let (result, _) = login_prompted(&args, &env.ctx(None, Some("demo"), None));

            let outcome = result.unwrap();
            assert_eq!(outcome.json_value().unwrap()["storage"], storage.label());
            assert_eq!(
                env.prompter.asked().last(),
                Some(&(STORAGE_PROMPT.to_owned(), Some(config_option())))
            );
            assert_eq!(env.demo_token(), saved_to(storage, "key"));
            assert_eq!(env.prompter.remaining(), 0);
        }
    }

    #[test]
    fn rejected_prompted_token_is_not_followed_by_the_storage_prompt() {
        let env = Env::interactive(
            FakeApi::new().with_status(SETTINGS, 401, "{}"),
            key_then_storage("key", TokenStorage::Config),
        );
        let (result, _) = login_prompted(
            &login_args(false, None, None),
            &env.ctx(None, Some("demo"), None),
        );
        assert!(matches!(
            result.unwrap_err().downcast_ref::<Error>(),
            Some(Error::Auth(_))
        ));
        assert_eq!(env.prompter.asked().len(), 2, "method and key only");
        assert_eq!(env.config.saves(), 0);
    }

    #[test]
    fn prompted_storage_preselects_the_profile_storage() {
        for (current, preselected) in [
            (TokenStorage::Config, config_option()),
            (TokenStorage::Keyring, KEYRING_OPTION.to_owned()),
        ] {
            let env =
                Env::interactive(ok_api(), key_then_storage("new", current)).with_profile(current);
            let args = login_args(false, None, None);
            let (result, _) = login_prompted(&args, &env.ctx(None, None, None));

            result.unwrap();
            assert_eq!(
                env.prompter.asked().last(),
                Some(&(STORAGE_PROMPT.to_owned(), Some(preselected)))
            );
            assert_eq!(env.demo_token(), saved_to(current, "new"));
        }
    }

    #[test]
    fn storage_flag_skips_the_storage_prompt() {
        for storage in [TokenStorage::Config, TokenStorage::Keyring] {
            let env = Env::interactive(
                ok_api(),
                vec![MockResponse::Select(0), MockResponse::Secret("key".into())],
            );
            let args = login_args(false, None, Some(storage));
            let (result, _) = login_prompted(&args, &env.ctx(None, Some("demo"), None));

            assert_eq!(
                result.unwrap().json_value().unwrap()["storage"],
                storage.label()
            );
            assert!(
                env.prompter
                    .asked()
                    .iter()
                    .all(|(m, _)| m != STORAGE_PROMPT),
                "{:?}",
                env.prompter.asked()
            );
            assert_eq!(env.demo_token(), saved_to(storage, "key"));
        }
    }

    #[test]
    fn piped_login_keeps_the_profile_storage_or_uses_config() {
        let args = login_args(true, None, None);
        let env = Env::new(ok_api());
        login_piped(
            &args,
            &env.ctx(None, Some("demo"), None),
            &mut stdin("new\n"),
        )
        .unwrap();
        assert_eq!(env.demo_token(), saved_to(TokenStorage::Config, "new"));

        for storage in [TokenStorage::Config, TokenStorage::Keyring] {
            let env = Env::new(ok_api()).with_profile(storage);
            let outcome =
                login_piped(&args, &env.ctx(None, None, None), &mut stdin("new\n")).unwrap();
            assert_eq!(outcome.json_value().unwrap()["storage"], storage.label());
            assert_eq!(env.demo_token(), saved_to(storage, "new"));
        }
    }

    #[test]
    fn switching_storage_removes_the_other_copy() {
        for (from, to) in [
            (TokenStorage::Keyring, TokenStorage::Config),
            (TokenStorage::Config, TokenStorage::Keyring),
        ] {
            let env = Env::new(ok_api()).with_profile(from);
            let outcome = login_piped(
                &login_args(true, None, Some(to)),
                &env.ctx(None, None, None),
                &mut stdin("new\n"),
            )
            .unwrap();
            assert!(outcome.hints.is_empty(), "{:?}", outcome.hints);
            assert_eq!(env.demo_token(), saved_to(to, "new"), "--storage {to}");

            let env = Env::interactive(ok_api(), key_then_storage("new", to)).with_profile(from);
            let (result, _) =
                login_prompted(&login_args(false, None, None), &env.ctx(None, None, None));
            result.unwrap();
            assert_eq!(env.demo_token(), saved_to(to, "new"), "prompted {to}");
        }

        // The old entry sits under the host of the CRM the profile had before `--crm`.
        let env = Env::new(ok_api()).with_profile(TokenStorage::Keyring);
        login_piped(
            &login_args(true, None, Some(TokenStorage::Config)),
            &env.ctx(Some("demo"), Some("other"), None),
            &mut stdin("new\n"),
        )
        .unwrap();
        assert!(env.store.is_empty());
    }

    #[test]
    fn cancel_at_the_storage_prompt_saves_nothing() {
        let env = Env::interactive(
            ok_api(),
            vec![
                MockResponse::Select(0),
                MockResponse::Secret("key".into()),
                MockResponse::Cancel,
            ],
        )
        .with_profile(TokenStorage::Keyring);
        let (result, _) =
            login_prompted(&login_args(false, None, None), &env.ctx(None, None, None));

        let err = result.unwrap_err();
        assert_eq!(format!("{err:#}"), "login cancelled");
        assert_eq!(exit_code_for_error(&err), exit_code::RUNTIME_ERROR);
        assert_eq!(env.config.saves(), 0);
        assert_eq!(
            env.demo_token(),
            saved_to(TokenStorage::Keyring, "old-token")
        );
    }

    #[test]
    fn keyring_write_failure_saves_nothing_and_suggests_config() {
        let env = Env::new(ok_api());
        let mut ctx = env.ctx(None, Some("demo"), None);
        ctx.store = &BrokenKeyring;
        let args = login_args(true, None, Some(TokenStorage::Keyring));
        let err = login_piped(&args, &ctx, &mut stdin("tok\n")).unwrap_err();
        assert!(
            matches!(err.downcast_ref::<Error>(), Some(Error::Keyring(m)) if m.contains("retry with --storage config")),
            "{err:#}"
        );
        assert_eq!(env.config.saves(), 0);
    }

    #[test]
    fn keyring_cleanup_runs_only_when_leaving_the_keyring() {
        let env = Env::new(ok_api()).with_profile(TokenStorage::Keyring);
        let mut ctx = env.ctx(None, None, None);
        ctx.store = &BrokenKeyring;
        let args = login_args(true, None, Some(TokenStorage::Config));
        let outcome = login_piped(&args, &ctx, &mut stdin("new\n")).unwrap();
        assert_eq!(
            outcome.hints,
            vec![
                "could not remove the profile's old token from the OS keyring (keyring error: \
                 locked); the token in the config file takes precedence"
            ]
        );
        assert_eq!(
            env.config().profiles["demo"].api_token.as_deref(),
            Some("new")
        );

        // A failing delete would leave a hint, so these logins never touched the keyring.
        for env in [
            Env::new(ok_api()),
            Env::new(ok_api()).with_profile(TokenStorage::Config),
        ] {
            let mut ctx = env.ctx(None, Some("demo"), None);
            ctx.store = &BrokenKeyring;
            let outcome =
                login_piped(&login_args(true, None, None), &ctx, &mut stdin("new\n")).unwrap();
            assert!(outcome.hints.is_empty(), "{:?}", outcome.hints);
        }
    }

    /// The rule `hw auth login --help` states: `--profile`; without `--crm`, the default
    /// (or only) existing profile; otherwise the CRM's first host label.
    #[test]
    fn login_profile_name_follows_the_help_rule() {
        let existing = || {
            let mut config = Config {
                default_profile: Some("main".into()),
                ..Config::default()
            };
            config.profiles.insert(
                "main".into(),
                Profile {
                    crm: Some("demo".into()),
                    ..Profile::default()
                },
            );
            InMemoryConfigStore::new(Some(config))
        };
        for (profile, crm, config, expected) in [
            (Some("x"), Some("demo"), existing(), "x"),
            (None, None, existing(), "main"),
            (None, Some("demo"), existing(), "demo"),
            (None, Some("https://other.example.com"), existing(), "other"),
            (None, Some("demo"), InMemoryConfigStore::new(None), "demo"),
        ] {
            let mut env = Env::new(ok_api());
            env.config = config;
            let args = login_args(true, None, Some(TokenStorage::Config));
            let outcome =
                login_piped(&args, &env.ctx(profile, crm, None), &mut stdin("tok\n")).unwrap();
            assert_eq!(
                outcome.json_value().unwrap()["profile"],
                expected,
                "{profile:?} {crm:?}"
            );
        }
    }

    #[test]
    fn rejected_token_saves_nothing() {
        let env = Env::new(FakeApi::new().with_status(SETTINGS, 401, r#"{"message":"bad"}"#));
        let args = login_args(true, None, Some(TokenStorage::Config));
        let err = login_piped(
            &args,
            &env.ctx(None, Some("demo"), None),
            &mut stdin("tok\n"),
        )
        .unwrap_err();
        assert!(matches!(err.downcast_ref::<Error>(), Some(Error::Auth(_))));
        assert_eq!(env.config.saves(), 0);
        assert!(env.store.is_empty());
    }

    #[test]
    fn empty_stdin_is_invalid_input() {
        let env = Env::new(ok_api());
        let args = login_args(true, None, Some(TokenStorage::Keyring));
        let err =
            login_piped(&args, &env.ctx(None, Some("demo"), None), &mut stdin("\n")).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn login_without_crm_is_config_error() {
        let env = Env::new(ok_api());
        let args = login_args(true, None, Some(TokenStorage::Keyring));
        let err = login_piped(&args, &env.ctx(None, None, None), &mut stdin("tok\n")).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::Config(_))
        ));
    }

    #[test]
    fn status_reports_valid_env_token() {
        let env = Env::new(ok_api());
        let token = Secret::new("env-token");
        let outcome = status(&env.ctx(None, Some("demo"), Some(&token))).unwrap();
        assert_eq!(
            outcome.json_value().unwrap(),
            &serde_json::json!({"profile": null, "crm": "https://demo.hookah.work", "token_source": "env", "token_status": "valid"})
        );
        assert!(outcome.failure.is_none());
    }

    #[test]
    fn status_without_token_reports_then_fails_with_auth() {
        let env = Env::new(ok_api());
        let outcome = status(&env.ctx(None, Some("demo"), None)).unwrap();
        assert_eq!(outcome.json_value().unwrap()["token_status"], "unchecked");
        assert!(matches!(outcome.failure, Some(Error::Auth(_))));
        assert!(env.connector.connections().is_empty());
    }

    #[test]
    fn status_with_rejected_token_is_invalid() {
        let env = Env::new(FakeApi::new().with_status(SETTINGS, 401, "{}"));
        let token = Secret::new("t");
        let outcome = status(&env.ctx(None, Some("demo"), Some(&token))).unwrap();
        assert_eq!(outcome.json_value().unwrap()["token_status"], "invalid");
        assert!(matches!(outcome.failure, Some(Error::Auth(_))));
    }

    #[test]
    fn status_with_unreachable_crm_reports_unchecked_then_fails() {
        let env = Env::new(FakeApi::new().with_status(SETTINGS, 503, "{}"));
        let token = Secret::new("t");
        let outcome = status(&env.ctx(None, Some("demo"), Some(&token))).unwrap();
        assert_eq!(
            outcome.json_value().unwrap(),
            &serde_json::json!({"profile": null, "crm": "https://demo.hookah.work", "token_source": "env", "token_status": "unchecked"})
        );
        assert!(matches!(
            outcome.failure,
            Some(Error::Api { status: 503, .. })
        ));
    }

    #[test]
    fn profile_token_is_not_used_for_another_crm() {
        let env = Env::new(ok_api());
        let args = login_args(true, None, Some(TokenStorage::Config));
        login_piped(
            &args,
            &env.ctx(None, Some("demo"), None),
            &mut stdin("cfg-token\n"),
        )
        .unwrap();

        let err = token(&env.ctx(None, Some("other"), None)).unwrap_err();
        assert!(matches!(err.downcast_ref::<Error>(), Some(Error::Auth(_))));
        let outcome = status(&env.ctx(None, Some("other"), None)).unwrap();
        assert_eq!(outcome.json_value().unwrap()["token_source"], "none");
        assert!(matches!(outcome.failure, Some(Error::Auth(_))));
        assert_eq!(env.connector.connections().len(), 1, "only the login check");
    }

    #[test]
    fn logout_with_crm_override_clears_the_profile_host_too() {
        let env = Env::new(ok_api());
        let args = login_args(true, None, Some(TokenStorage::Keyring));
        login_piped(
            &args,
            &env.ctx(None, Some("demo"), None),
            &mut stdin("kr-token\n"),
        )
        .unwrap();
        env.store
            .set("hw:demo", "other.hookah.work", "stale")
            .unwrap();

        let out = logout(&env.ctx(None, Some("other"), None)).unwrap();
        assert_eq!(out.json_value().unwrap()["logged_out"], true);
        assert!(env.store.is_empty());
    }

    #[test]
    fn token_prints_keyring_token_and_logout_removes_it() {
        let env = Env::new(ok_api());
        let args = login_args(true, None, Some(TokenStorage::Keyring));
        login_piped(
            &args,
            &env.ctx(None, Some("demo"), None),
            &mut stdin("kr-token\n"),
        )
        .unwrap();

        let outcome = token(&env.ctx(None, None, None)).unwrap();
        assert!(
            matches!(&outcome.payload, super::super::Payload::Secret(s) if s.expose() == "kr-token")
        );

        let out = logout(&env.ctx(None, None, None)).unwrap();
        assert_eq!(out.json_value().unwrap()["logged_out"], true);
        assert!(env.store.is_empty());
        assert!(
            env.config().profiles.contains_key("demo"),
            "profile is kept"
        );
        logout(&env.ctx(None, None, None)).unwrap();

        let err = token(&env.ctx(None, None, None)).unwrap_err();
        assert!(matches!(err.downcast_ref::<Error>(), Some(Error::Auth(_))));
    }
}
