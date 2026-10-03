//! Locating, reading and writing the configuration file.
//!
//! Location: `--config` > `HW_CONFIG` > `$XDG_CONFIG_HOME/hw/config.toml` > `~/.config/hw/config.toml`
//! (Unix, macOS included) or `%APPDATA%\hw\config.toml` (Windows).

use std::io::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize as _;
use tracing::debug;

use super::{CONFIG_DIR_NAME, CONFIG_FILE_NAME, Config};
use crate::error::{Error, Result};

/// The config file path for this invocation and whether the user named it explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLocation {
    /// Path of the file (it may not exist yet).
    pub path: Utf8PathBuf,
    /// `true` for `--config`/`HW_CONFIG`; an explicit path must exist for reads.
    pub explicit: bool,
}

impl ConfigLocation {
    /// Chooses `explicit` when given, otherwise the platform default path.
    pub fn resolve(explicit: Option<&Utf8Path>, default_path: Option<&Utf8Path>) -> Result<Self> {
        match (explicit, default_path) {
            (Some(path), _) => Ok(Self {
                path: path.to_owned(),
                explicit: true,
            }),
            (None, Some(path)) => Ok(Self {
                path: path.to_owned(),
                explicit: false,
            }),
            (None, None) => Err(Error::Config(
                "cannot determine the config file location; pass --config <PATH> or set HW_CONFIG"
                    .into(),
            )),
        }
    }
}

/// Unix default path: `$XDG_CONFIG_HOME/hw/config.toml` when set (non-empty), else
/// `<home>/.config/hw/config.toml`. Pure, so the precedence is testable without env mutation.
#[must_use]
pub fn default_config_path(
    xdg_config_home: Option<&str>,
    home: Option<&Utf8Path>,
) -> Option<Utf8PathBuf> {
    let base = match xdg_config_home.map(str::trim).filter(|v| !v.is_empty()) {
        Some(xdg) => Utf8PathBuf::from(xdg),
        None => home?.join(".config"),
    };
    Some(base.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME))
}

/// The default path for this platform, read from the process environment.
#[must_use]
pub fn platform_default_config_path() -> Option<Utf8PathBuf> {
    if cfg!(windows) {
        let dir = Utf8PathBuf::try_from(dirs::config_dir()?).ok()?;
        return Some(dir.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME));
    }
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let home = dirs::home_dir().and_then(|h| Utf8PathBuf::try_from(h).ok());
    default_config_path(xdg.as_deref(), home.as_deref())
}

/// Reads and writes [`Config`] files.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConfigLoader;

impl ConfigLoader {
    /// Loads the file. A missing default file is `Ok(None)` (no profiles); a missing
    /// explicit file is a configuration error.
    pub fn load(location: &ConfigLocation) -> Result<Option<Config>> {
        if !location.path.exists() {
            if location.explicit {
                return Err(Error::Config(format!(
                    "config file not found: {}",
                    location.path
                )));
            }
            debug!("no config file at {}", location.path);
            return Ok(None);
        }
        Self::read(&location.path).map(Some)
    }

    /// Loads the file for a read-modify-write: a missing file (explicit or not) is an empty config.
    pub fn load_for_update(location: &ConfigLocation) -> Result<Config> {
        if location.path.exists() {
            Self::read(&location.path)
        } else {
            Ok(Config::default())
        }
    }

    fn read(path: &Utf8Path) -> Result<Config> {
        debug!("loading config from {path}");
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("cannot read {path}: {e}")))?;
        let document = toml::de::Deserializer::parse(&text)
            .map_err(|e| parse_error(path, &text, &e, false))?;
        Config::deserialize(document).map_err(|e| parse_error(path, &text, &e, true))
    }

    /// Writes `config` to `path`, creating parent directories. The file is replaced via a
    /// temporary sibling so a crash never leaves half a file, and is owner-only on Unix
    /// because it may hold `api_token`.
    pub fn save(config: &Config, path: &Utf8Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(config)
            .map_err(|e| Error::Config(format!("cannot serialise config: {e}")))?;
        let tmp = path.with_extension("toml.tmp");
        write_private(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        debug!("wrote config to {path}");
        Ok(())
    }
}

/// A config error without source text: toml's `Display` quotes the offending line, which may
/// hold `api_token`, so only the message and a `line:column` position are kept. A data
/// (serde) message also quotes the offending input; `mask_input` hides it.
fn parse_error(path: &Utf8Path, text: &str, err: &toml::de::Error, mask_input: bool) -> Error {
    let message = err
        .message()
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("; ");
    let message = if mask_input {
        mask_first_quoted(&message)
    } else {
        message
    };
    let position = err
        .span()
        .and_then(|span| text.get(..span.start))
        .map(|before| {
            let line = before.matches('\n').count() + 1;
            let column = before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .count()
                + 1;
            format!(":{line}:{column}")
        })
        .unwrap_or_default();
    Error::Config(format!("{path}{position}: {message}"))
}

/// Replaces the first `` `…` `` or `"…"` part of a serde message with `***`: it is always the
/// offending input (`unknown variant `…``, `invalid type: string "…"`), never the schema.
fn mask_first_quoted(message: &str) -> String {
    let Some((start, quote)) = message.char_indices().find(|(_, c)| matches!(c, '`' | '"')) else {
        return message.to_owned();
    };
    let rest = &message[start + quote.len_utf8()..];
    let mut escaped = false;
    let end = rest.char_indices().find_map(|(i, c)| {
        let closes = c == quote && !escaped;
        escaped = c == '\\' && !escaped;
        closes.then_some(i)
    });
    let tail = end.map_or("", |i| &rest[i + quote.len_utf8()..]);
    format!("{}{quote}***{quote}{tail}", &message[..start])
}

fn write_private(path: &Utf8Path, content: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // `mode` only applies on creation; a leftover temp file keeps its old bits otherwise.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(content)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Profile, TokenStorage};

    fn temp_path(dir: &tempfile::TempDir, name: &str) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(dir.path().join(name)).unwrap()
    }

    #[test]
    fn xdg_wins_over_home() {
        let home = Utf8Path::new("/home/u");
        assert_eq!(
            default_config_path(Some("/xdg"), Some(home)).unwrap(),
            "/xdg/hw/config.toml"
        );
        assert_eq!(
            default_config_path(Some(""), Some(home)).unwrap(),
            "/home/u/.config/hw/config.toml"
        );
        assert_eq!(
            default_config_path(None, Some(home)).unwrap(),
            "/home/u/.config/hw/config.toml"
        );
        assert!(default_config_path(None, None).is_none());
    }

    #[test]
    fn resolve_prefers_explicit() {
        let loc = ConfigLocation::resolve(
            Some(Utf8Path::new("/a.toml")),
            Some(Utf8Path::new("/b.toml")),
        )
        .unwrap();
        assert_eq!((loc.path.as_str(), loc.explicit), ("/a.toml", true));
        let loc = ConfigLocation::resolve(None, Some(Utf8Path::new("/b.toml"))).unwrap();
        assert_eq!((loc.path.as_str(), loc.explicit), ("/b.toml", false));
        assert!(matches!(
            ConfigLocation::resolve(None, None),
            Err(Error::Config(_))
        ));
    }

    #[test]
    fn missing_default_file_is_none_missing_explicit_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_path(&dir, "absent.toml");
        let implicit = ConfigLocation {
            path: path.clone(),
            explicit: false,
        };
        assert!(ConfigLoader::load(&implicit).unwrap().is_none());
        let explicit = ConfigLocation {
            path,
            explicit: true,
        };
        assert!(matches!(
            ConfigLoader::load(&explicit),
            Err(Error::Config(_))
        ));
        assert_eq!(
            ConfigLoader::load_for_update(&explicit).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn parse_error_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_path(&dir, "bad.toml");
        std::fs::write(&path, "default_profile = [").unwrap();
        let loc = ConfigLocation {
            path: path.clone(),
            explicit: true,
        };
        let err = ConfigLoader::load(&loc).unwrap_err();
        assert!(
            matches!(&err, Error::Config(msg) if msg.contains(path.as_str())),
            "{err:?}"
        );
    }

    #[test]
    fn parse_errors_never_quote_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_path(&dir, "bad.toml");
        for text in [
            "[profiles.a]\napi_tokn = \"SECRET-1\"\n",
            "[profiles.a]\napi_token = SECRET-1\n",
            "[profiles.a]\ntoken_storage = \"SECRET-1\"\n",
            "[profiles.a]\napi_token = 123456789\n",
            "[profiles.a]\napi_token = \"SECRET-1\n",
            "[profiles]\napi_token = \"SECRET-1\"\n",
            "[profiles]\napi_token = \"SEC\\\"RET-1\"\n",
        ] {
            std::fs::write(&path, text).unwrap();
            let loc = ConfigLocation {
                path: path.clone(),
                explicit: true,
            };
            let err = ConfigLoader::load(&loc).unwrap_err().to_string();
            assert!(
                !["SECRET-1", "SEC", "123456789"]
                    .iter()
                    .any(|secret| err.contains(secret)),
                "{err}"
            );
            assert!(err.contains(&format!("{path}:2:")), "{err}");
        }
    }

    #[test]
    fn data_errors_keep_the_schema_part() {
        assert_eq!(
            mask_first_quoted("unknown variant `tok`, expected `keyring` or `config`"),
            "unknown variant `***`, expected `keyring` or `config`"
        );
        assert_eq!(
            mask_first_quoted(r#"invalid type: string "a\"b", expected struct Profile"#),
            r#"invalid type: string "***", expected struct Profile"#
        );
        assert_eq!(mask_first_quoted("no quotes"), "no quotes");
    }

    #[test]
    fn save_round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_path(&dir, "nested/hw/config.toml");
        let mut config = Config {
            default_profile: Some("demo".into()),
            ..Config::default()
        };
        config.profiles.insert(
            "demo".into(),
            Profile {
                crm: Some("demo".into()),
                token_storage: TokenStorage::Config,
                api_token: Some("tok".into()),
                ..Profile::default()
            },
        );
        ConfigLoader::save(&config, &path).unwrap();
        let loc = ConfigLocation {
            path: path.clone(),
            explicit: true,
        };
        assert_eq!(ConfigLoader::load(&loc).unwrap().unwrap(), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
