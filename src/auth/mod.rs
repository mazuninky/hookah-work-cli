//! Credentials: the [`Secret`] wrapper, the [`SecretStore`] keyring seam, the [`Prompter`]
//! of interactive login and the token resolution chain `HW_TOKEN` → profile `api_token` →
//! keyring.

pub mod keyring;
mod prompter;
mod secret;

#[cfg(any(test, feature = "test-util"))]
pub use keyring::InMemoryStore;
pub use keyring::{SecretStore, SystemKeyring};
pub use prompter::{InquirePrompter, PromptError, Prompter, Validator, non_blank};
#[cfg(any(test, feature = "test-util"))]
pub use prompter::{MockPrompter, MockResponse};
pub use secret::Secret;

use tracing::debug;

use crate::config::{Crm, SelectedProfile};
use crate::error::{Error, Result};

/// Keyring service under which a profile's token is stored; the account is the CRM host.
/// Changing the format strands tokens saved by earlier versions.
#[must_use]
pub fn service_name(profile: &str) -> String {
    format!("hw:{profile}")
}

/// Where a resolved token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// The `HW_TOKEN` environment variable.
    Env,
    /// The profile's `api_token` in the config file.
    Config,
    /// The OS keyring.
    Keyring,
}

impl TokenSource {
    /// Stable label shown by `hw auth status`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TokenSource::Env => "env",
            TokenSource::Config => "config",
            TokenSource::Keyring => "keyring",
        }
    }
}

/// The error for an authenticated command without any token, with a hint how to get one.
#[must_use]
pub fn missing_token_error(profile: Option<&str>) -> Error {
    let whose = profile.map_or_else(String::new, |name| format!(" for profile '{name}'"));
    Error::Auth(format!(
        "no API token{whose}; pipe one into `hw auth login --crm <NAME> --with-token` or set HW_TOKEN"
    ))
}

/// Resolves the API token for a request to `target`: a non-blank `env_token` wins (it is
/// global), then the profile's `api_token`, then the keyring entry `hw:<profile>` / host.
/// Profile tokens are scoped to the [origin](Crm::origin) of the profile's own CRM: for any
/// other `target` the result is an [`Error::Auth`], as it is when no token exists. Keyring
/// failures count as "no token".
pub fn resolve_token(
    env_token: Option<&Secret>,
    selected: Option<SelectedProfile<'_>>,
    target: Option<&Crm>,
    store: &dyn SecretStore,
) -> Result<(Secret, TokenSource)> {
    if let Some(token) = env_token.filter(|t| !t.is_blank()) {
        return Ok((token.clone(), TokenSource::Env));
    }
    let Some(selected) = selected else {
        return Err(missing_token_error(None));
    };
    let own = selected
        .profile
        .crm
        .as_deref()
        .and_then(|crm| Crm::parse(crm).ok());
    if own.as_ref().map(Crm::origin) != target.map(Crm::origin) {
        return Err(out_of_scope_error(selected.name, own.as_ref(), target));
    }
    if let Some(token) = selected
        .profile
        .api_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        return Ok((Secret::new(token), TokenSource::Config));
    }
    // `hw auth login` stores the entry under the profile's own host, which may spell the
    // shared origin differently from `target` (`:443`).
    let Some(own) = own else {
        return Err(missing_token_error(Some(selected.name)));
    };
    match store.get(&service_name(selected.name), own.host()) {
        Ok(Some(token)) if !token.trim().is_empty() => {
            Ok((Secret::new(token), TokenSource::Keyring))
        }
        Ok(_) => Err(missing_token_error(Some(selected.name))),
        Err(err) => {
            debug!("keyring lookup failed for profile {}: {err}", selected.name);
            Err(missing_token_error(Some(selected.name)))
        }
    }
}

fn out_of_scope_error(profile: &str, own: Option<&Crm>, target: Option<&Crm>) -> Error {
    const HOW: &str = "set HW_TOKEN or run `hw auth login --crm <CRM>`";
    let target = target.map_or("another CRM", Crm::origin);
    Error::Auth(match own {
        Some(own) => format!(
            "profile '{profile}' belongs to {}; its token is never sent to {target}; {HOW}",
            own.origin()
        ),
        None => format!(
            "profile '{profile}' has no `crm`, so its token is never sent to {target}; \
             add `crm` to the profile, or {HOW}"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;

    fn crm(input: &str) -> Crm {
        Crm::parse(input).unwrap()
    }

    fn profile(token: Option<&str>) -> Profile {
        Profile {
            crm: Some("demo".into()),
            api_token: token.map(String::from),
            ..Profile::default()
        }
    }

    fn selected(p: &Profile) -> Option<SelectedProfile<'_>> {
        Some(SelectedProfile {
            name: "demo",
            profile: p,
        })
    }

    #[test]
    fn service_name_format() {
        assert_eq!(service_name("demo"), "hw:demo");
    }

    #[test]
    fn env_token_wins() {
        let store = InMemoryStore::new();
        store.set("hw:demo", "demo.hookah.work", "kr").unwrap();
        let p = profile(Some("cfg"));
        let env = Secret::new("env");
        let (token, source) =
            resolve_token(Some(&env), selected(&p), Some(&crm("other")), &store).unwrap();
        assert_eq!((token.expose(), source), ("env", TokenSource::Env));
    }

    #[test]
    fn blank_env_falls_through_to_config() {
        let store = InMemoryStore::new();
        let p = profile(Some("cfg"));
        let env = Secret::new("  ");
        let (token, source) =
            resolve_token(Some(&env), selected(&p), Some(&crm("demo")), &store).unwrap();
        assert_eq!((token.expose(), source), ("cfg", TokenSource::Config));
    }

    #[test]
    fn keyring_is_last() {
        let store = InMemoryStore::new();
        store.set("hw:demo", "demo.hookah.work", "kr").unwrap();
        let p = profile(None);
        let (token, source) =
            resolve_token(None, selected(&p), Some(&crm("demo")), &store).unwrap();
        assert_eq!((token.expose(), source), ("kr", TokenSource::Keyring));
    }

    #[test]
    fn nothing_anywhere_is_an_auth_error() {
        let store = InMemoryStore::new();
        let p = profile(None);
        let target = crm("demo");
        for selected in [selected(&p), None] {
            let err = resolve_token(None, selected, Some(&target), &store).unwrap_err();
            assert!(matches!(err, Error::Auth(m) if m.contains("no API token")));
        }
    }

    #[test]
    fn profile_tokens_follow_the_normalised_origin() {
        let store = InMemoryStore::new();
        store.set("hw:demo", "demo.hookah.work", "kr").unwrap();
        let with_config = profile(Some("cfg"));
        let keyring_only = profile(None);
        for spelling in [
            "https://DEMO.hookah.work/api/",
            "https://demo.hookah.work/",
            "https://demo.hookah.work:443",
        ] {
            let target = crm(spelling);
            let (token, _) =
                resolve_token(None, selected(&with_config), Some(&target), &store).unwrap();
            assert_eq!(token.expose(), "cfg", "{spelling}");
            let (token, _) =
                resolve_token(None, selected(&keyring_only), Some(&target), &store).unwrap();
            assert_eq!(token.expose(), "kr", "{spelling}");
        }
    }

    #[test]
    fn profile_tokens_are_never_sent_to_another_origin() {
        let store = InMemoryStore::new();
        for (target, shown) in [
            ("other", "https://other.hookah.work"),
            ("http://demo.hookah.work", "http://demo.hookah.work"),
            (
                "https://demo.hookah.work:8443",
                "https://demo.hookah.work:8443",
            ),
        ] {
            let target = crm(target);
            store.set("hw:demo", target.host(), "kr").unwrap();
            for p in [profile(Some("cfg")), profile(None)] {
                let err = resolve_token(None, selected(&p), Some(&target), &store).unwrap_err();
                let expected = format!(
                    "profile 'demo' belongs to https://demo.hookah.work; its token is never sent to {shown}"
                );
                assert!(
                    matches!(&err, Error::Auth(m) if m.starts_with(&expected) && m.contains("HW_TOKEN")),
                    "{err:?}"
                );
            }
        }
        let no_crm = Profile {
            api_token: Some("cfg".into()),
            ..Profile::default()
        };
        let other = crm("other");
        let err = resolve_token(None, selected(&no_crm), Some(&other), &store).unwrap_err();
        assert!(
            matches!(&err, Error::Auth(m) if m.contains("has no `crm`")),
            "{err:?}"
        );
        let (token, _) = resolve_token(None, selected(&no_crm), None, &store).unwrap();
        assert_eq!(token.expose(), "cfg", "no CRM anywhere: nothing to leak to");
    }

    #[test]
    fn loopback_profiles_are_scoped_by_port() {
        let store = InMemoryStore::new();
        let local = Profile {
            crm: Some("http://127.0.0.1:4010".into()),
            api_token: Some("cfg".into()),
            ..Profile::default()
        };
        let same = crm("HTTP://127.0.0.1:4010/api/");
        let (token, _) = resolve_token(None, selected(&local), Some(&same), &store).unwrap();
        assert_eq!(token.expose(), "cfg");
        for other in [
            "http://127.0.0.1:4011",
            "http://localhost:4010",
            "https://127.0.0.1:4010",
        ] {
            let err = resolve_token(None, selected(&local), Some(&crm(other)), &store).unwrap_err();
            assert!(matches!(err, Error::Auth(_)), "{other}: {err:?}");
        }
    }

    #[test]
    fn labels_are_stable() {
        assert_eq!(TokenSource::Env.label(), "env");
        assert_eq!(TokenSource::Config.label(), "config");
        assert_eq!(TokenSource::Keyring.label(), "keyring");
    }
}
