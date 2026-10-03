//! Configuration file schema plus profile and CRM selection.
//!
//! Precedence: `--profile`/`HW_PROFILE` > `default_profile` > the only profile;
//! `--crm`/`HW_CRM` > the profile's `crm`.

mod crm;
mod loader;
mod store;

pub use crm::{API_KEY_PATH, API_KEY_URL_TEMPLATE, CRM_DOMAIN, Crm};
pub use loader::{ConfigLoader, ConfigLocation, default_config_path, platform_default_config_path};
#[cfg(any(test, feature = "test-util"))]
pub use store::InMemoryConfigStore;
pub use store::{ConfigStore, FileConfigStore};

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Directory under the user config dir that holds `hw`'s files.
pub const CONFIG_DIR_NAME: &str = "hw";
/// Configuration file name.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The whole configuration file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Profile used when `--profile` is not given; an empty string counts as unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    /// Profiles by name (sorted, so output and file order are deterministic).
    pub profiles: BTreeMap<String, Profile>,
}

/// One CRM connection.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// CRM subdomain, host or base URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crm: Option<String>,
    /// Who the token belongs to (recorded by `hw auth login --email`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Where `hw auth login` stored the token.
    pub token_storage: TokenStorage,
    /// The token itself, only with `token_storage = "config"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
}

impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profile")
            .field("crm", &self.crm)
            .field("email", &self.email)
            .field("token_storage", &self.token_storage)
            .field("api_token", &self.api_token.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Where the API token of a profile lives; a profile without `token_storage` means config.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TokenStorage {
    /// The `api_token` field of the config file (written with 0600 permissions)
    #[default]
    Config,
    /// The OS keyring (Keychain, Credential Manager, keyutils)
    Keyring,
}

impl TokenStorage {
    /// Stable lowercase label, identical to the config-file spelling.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TokenStorage::Keyring => "keyring",
            TokenStorage::Config => "config",
        }
    }
}

impl fmt::Display for TokenStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A profile chosen for this invocation, together with its name.
#[derive(Debug, Clone, Copy)]
pub struct SelectedProfile<'a> {
    /// Profile name (the key under `[profiles]`).
    pub name: &'a str,
    /// Profile settings.
    pub profile: &'a Profile,
}

impl Config {
    /// `default_profile`, ignoring an empty value.
    #[must_use]
    pub fn default_profile_name(&self) -> Option<&str> {
        self.default_profile
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    /// Picks the profile: `requested` > `default_profile` > the only profile.
    /// A named profile (requested or default) that does not exist is an error.
    pub fn select(&self, requested: Option<&str>) -> Result<Option<SelectedProfile<'_>>> {
        if let Some(name) = requested.map(str::trim).filter(|n| !n.is_empty()) {
            return self
                .lookup(name)
                .map(Some)
                .ok_or_else(|| profile_not_found(name));
        }
        if let Some(name) = self.default_profile_name() {
            return self.lookup(name).map(Some).ok_or_else(|| {
                Error::Config(format!(
                    "default_profile '{name}' does not exist; run `hw config set-default <NAME>`"
                ))
            });
        }
        if self.profiles.len() == 1 {
            return Ok(self
                .profiles
                .iter()
                .next()
                .map(|(name, profile)| SelectedProfile { name, profile }));
        }
        Ok(None)
    }

    fn lookup(&self, name: &str) -> Option<SelectedProfile<'_>> {
        self.profiles
            .get_key_value(name)
            .map(|(name, profile)| SelectedProfile { name, profile })
    }
}

/// The error for an explicitly named profile that is not in the config file.
#[must_use]
pub fn profile_not_found(name: &str) -> Error {
    Error::Config(format!("profile '{name}' not found; see `hw config list`"))
}

/// The CRM for this invocation, if any: `flag` > the profile's `crm`. Invalid values are errors.
pub fn optional_crm(flag: Option<&str>, profile: Option<&Profile>) -> Result<Option<Crm>> {
    let raw = flag
        .filter(|f| !f.trim().is_empty())
        .or_else(|| profile.and_then(|p| p.crm.as_deref()));
    raw.map(Crm::parse).transpose()
}

/// Like [`optional_crm`], but a missing CRM is a configuration error with a hint.
pub fn resolve_crm(flag: Option<&str>, profile: Option<&Profile>) -> Result<Crm> {
    optional_crm(flag, profile)?.ok_or_else(missing_crm)
}

/// The error for a command that needs a CRM when neither a flag nor a profile names one.
#[must_use]
pub fn missing_crm() -> Error {
    Error::Config(
        "no CRM selected; pass --crm <NAME> (or set HW_CRM), or run `hw auth login --crm <NAME>`"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(default: Option<&str>, names: &[&str]) -> Config {
        Config {
            default_profile: default.map(String::from),
            profiles: names
                .iter()
                .map(|n| {
                    (
                        (*n).to_owned(),
                        Profile {
                            crm: Some((*n).to_owned()),
                            ..Profile::default()
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn requested_profile_wins() {
        let cfg = config(Some("a"), &["a", "b"]);
        assert_eq!(cfg.select(Some("b")).unwrap().unwrap().name, "b");
    }

    #[test]
    fn missing_requested_profile_is_config_error() {
        let err = config(None, &["a"]).select(Some("zzz")).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }

    #[test]
    fn default_profile_then_single_profile() {
        assert_eq!(
            config(Some("b"), &["a", "b"])
                .select(None)
                .unwrap()
                .unwrap()
                .name,
            "b"
        );
        assert_eq!(
            config(None, &["only"]).select(None).unwrap().unwrap().name,
            "only"
        );
        assert!(config(None, &["a", "b"]).select(None).unwrap().is_none());
        assert!(
            config(Some(""), &["a", "b"])
                .select(None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn dangling_default_profile_is_config_error() {
        let err = config(Some("gone"), &["a"]).select(None).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }

    #[test]
    fn crm_flag_beats_profile() {
        let profile = Profile {
            crm: Some("demo".into()),
            ..Profile::default()
        };
        let crm = resolve_crm(Some("other"), Some(&profile)).unwrap();
        assert_eq!(crm.base_url(), "https://other.hookah.work");
        let crm = resolve_crm(None, Some(&profile)).unwrap();
        assert_eq!(crm.base_url(), "https://demo.hookah.work");
    }

    #[test]
    fn missing_crm_is_config_error() {
        let err = resolve_crm(None, None).unwrap_err();
        assert!(matches!(err, Error::Config(msg) if msg.contains("--crm")));
    }

    #[test]
    fn profile_debug_redacts_token() {
        let profile = Profile {
            api_token: Some("very-secret".into()),
            ..Profile::default()
        };
        let debug = format!("{profile:?}");
        assert!(!debug.contains("very-secret"), "{debug}");
        assert!(debug.contains("***"));
    }

    #[test]
    fn parses_documented_schema() {
        let cfg: Config = toml::from_str(
            r#"
default_profile = "demo"
[profiles.demo]
crm = "demo"
email = "integration@example.com"
token_storage = "config"
api_token = "abc"
"#,
        )
        .unwrap();
        let profile = &cfg.profiles["demo"];
        assert_eq!(profile.token_storage, TokenStorage::Config);
        assert_eq!(profile.api_token.as_deref(), Some("abc"));
    }

    #[test]
    fn token_storage_defaults_to_config() {
        let cfg: Config =
            toml::from_str("[profiles.a]\n[profiles.b]\ntoken_storage = \"keyring\"\n").unwrap();
        assert_eq!(cfg.profiles["a"].token_storage, TokenStorage::Config);
        assert_eq!(cfg.profiles["b"].token_storage, TokenStorage::Keyring);
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = toml::from_str::<Config>("[profiles.a]\ncrn = \"x\"\n").unwrap_err();
        assert!(err.to_string().contains("crn"), "{err}");
    }
}
