//! `hw config` — list, show, set-default, delete and path. Tokens are never printed.

use serde_json::{Value, json};

use super::Outcome;
use crate::auth::{SecretStore, service_name};
use crate::cli::args::ConfigCommand;
use crate::config::{Config, ConfigStore, Crm, profile_not_found};
use crate::error::Error;

/// Runs `hw config <command>`; `selected` is `--profile`/`HW_PROFILE`, `secrets` the keyring.
pub fn run(
    command: &ConfigCommand,
    config_store: &dyn ConfigStore,
    selected: Option<&str>,
    secrets: &dyn SecretStore,
) -> anyhow::Result<Outcome> {
    match command {
        ConfigCommand::Path => Ok(Outcome::json(json!(config_store.path().as_str()))),
        ConfigCommand::List => Ok(Outcome::json(list(&load(config_store)?))),
        ConfigCommand::Show(args) => {
            let config = load(config_store)?;
            let name = match args.name.as_deref() {
                Some(name) => name.to_owned(),
                None => config
                    .select(selected)?
                    .map(|s| s.name.to_owned())
                    .ok_or_else(|| {
                        Error::Config(
                            "no profile selected; pass a NAME (see `hw config list`)".into(),
                        )
                    })?,
            };
            Ok(Outcome::json(show(&config, &name)?))
        }
        ConfigCommand::SetDefault(args) => {
            let mut config = load(config_store)?;
            if !config.profiles.contains_key(&args.name) {
                return Err(profile_not_found(&args.name).into());
            }
            config.default_profile = Some(args.name.clone());
            config_store.save(&config)?;
            Ok(Outcome::json(json!({"default_profile": args.name})))
        }
        ConfigCommand::Delete(args) => {
            let mut config = load(config_store)?;
            let profile = config
                .profiles
                .remove(&args.name)
                .ok_or_else(|| profile_not_found(&args.name))?;
            // A CRM that no longer parses has no keyring entry worth chasing.
            if let Some(crm) = profile.crm.as_deref().and_then(|c| Crm::parse(c).ok()) {
                secrets.delete(&service_name(&args.name), crm.host())?;
            }
            if config.default_profile_name() == Some(args.name.as_str()) {
                config.default_profile = None;
            }
            config_store.save(&config)?;
            Ok(Outcome::json(json!({"deleted": args.name})))
        }
    }
}

fn load(config_store: &dyn ConfigStore) -> anyhow::Result<Config> {
    Ok(config_store.load()?.unwrap_or_default())
}

fn list(config: &Config) -> Value {
    let default = config.default_profile_name();
    config
        .profiles
        .iter()
        .map(|(name, profile)| {
            json!({
                "name": name,
                "crm": profile.crm,
                "default": Some(name.as_str()) == default,
                "token_storage": profile.token_storage.label(),
            })
        })
        .collect()
}

fn show(config: &Config, name: &str) -> anyhow::Result<Value> {
    let profile = config
        .profiles
        .get(name)
        .ok_or_else(|| profile_not_found(name))?;
    let mut value = json!({
        "name": name,
        "default": config.default_profile_name() == Some(name),
        "crm": profile.crm,
        "email": profile.email,
        "token_storage": profile.token_storage.label(),
    });
    if profile.api_token.is_some()
        && let Some(map) = value.as_object_mut()
    {
        map.insert("api_token".into(), json!("***"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::InMemoryStore;
    use crate::cli::args::{ConfigNameArgs, ConfigShowArgs};
    use crate::config::InMemoryConfigStore;

    const SAMPLE: &str = r#"
default_profile = "demo"
[profiles.demo]
crm = "demo"
token_storage = "config"
api_token = "very-secret"
[profiles.another]
crm = "another.hookah.work"
"#;

    fn setup(contents: Option<&str>) -> InMemoryConfigStore {
        InMemoryConfigStore::new(contents.map(|text| toml::from_str(text).unwrap()))
    }

    fn run_json(
        command: &ConfigCommand,
        config_store: &InMemoryConfigStore,
        secrets: &InMemoryStore,
    ) -> Value {
        run(command, config_store, None, secrets)
            .unwrap()
            .json_value()
            .cloned()
            .unwrap()
    }

    #[test]
    fn list_is_sorted_with_default_marker() {
        let config_store = setup(Some(SAMPLE));
        let value = run_json(&ConfigCommand::List, &config_store, &InMemoryStore::new());
        assert_eq!(
            value,
            json!([
                {"name": "another", "crm": "another.hookah.work", "default": false, "token_storage": "config"},
                {"name": "demo", "crm": "demo", "default": true, "token_storage": "config"},
            ])
        );
    }

    #[test]
    fn list_without_file_is_empty_and_path_is_reported() {
        let config_store = setup(None);
        let secrets = InMemoryStore::new();
        assert_eq!(
            run_json(&ConfigCommand::List, &config_store, &secrets),
            json!([])
        );
        assert_eq!(
            run_json(&ConfigCommand::Path, &config_store, &secrets),
            json!(InMemoryConfigStore::PATH)
        );
    }

    #[test]
    fn show_redacts_token() {
        let config_store = setup(Some(SAMPLE));
        let value = run_json(
            &ConfigCommand::Show(ConfigShowArgs { name: None }),
            &config_store,
            &InMemoryStore::new(),
        );
        assert_eq!(value["name"], "demo");
        assert_eq!(value["api_token"], "***");
        assert!(!value.to_string().contains("very-secret"));
    }

    #[test]
    fn unknown_profile_is_config_error() {
        let config_store = setup(Some(SAMPLE));
        let command = ConfigCommand::SetDefault(ConfigNameArgs {
            name: "nope".into(),
        });
        let err = run(&command, &config_store, None, &InMemoryStore::new()).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<Error>(),
            Some(Error::Config(_))
        ));
        assert_eq!(config_store.saves(), 0);
    }

    #[test]
    fn set_default_then_delete_clears_default_and_keyring() {
        let config_store = setup(Some(SAMPLE));
        let secrets = InMemoryStore::new();
        secrets
            .set("hw:another", "another.hookah.work", "kr")
            .unwrap();

        let set = ConfigCommand::SetDefault(ConfigNameArgs {
            name: "another".into(),
        });
        run(&set, &config_store, None, &secrets).unwrap();
        let delete = ConfigCommand::Delete(ConfigNameArgs {
            name: "another".into(),
        });
        run(&delete, &config_store, None, &secrets).unwrap();

        let config = config_store.config().unwrap();
        assert!(!config.profiles.contains_key("another"));
        assert_eq!(config.default_profile, None);
        assert!(secrets.is_empty());
        assert_eq!(config_store.saves(), 2);
    }
}
