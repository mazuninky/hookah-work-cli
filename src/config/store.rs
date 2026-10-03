//! [`ConfigStore`]: the configuration file behind a trait, so command handlers never touch
//! the filesystem. [`FileConfigStore`] is built by the composition root (`crate::app`).

use camino::Utf8Path;

use super::{Config, ConfigLoader, ConfigLocation};
use crate::error::Result;

/// Where the configuration lives and how it is read and written.
pub trait ConfigStore: Send + Sync {
    /// Path of the backing file (it may not exist yet), as printed by `hw config path`.
    fn path(&self) -> &Utf8Path;

    /// The configuration; `None` when there is none yet. A missing explicit file is an error.
    fn load(&self) -> Result<Option<Config>>;

    /// The configuration for a read-modify-write; a missing file is an empty configuration.
    fn load_for_update(&self) -> Result<Config>;

    /// Replaces the stored configuration.
    fn save(&self, config: &Config) -> Result<()>;
}

/// Production [`ConfigStore`]: the TOML file at a [`ConfigLocation`].
#[derive(Debug, Clone)]
pub struct FileConfigStore {
    location: ConfigLocation,
}

impl FileConfigStore {
    /// A store for the file at `location`.
    #[must_use]
    pub fn new(location: ConfigLocation) -> Self {
        Self { location }
    }
}

impl ConfigStore for FileConfigStore {
    fn path(&self) -> &Utf8Path {
        &self.location.path
    }

    fn load(&self) -> Result<Option<Config>> {
        ConfigLoader::load(&self.location)
    }

    fn load_for_update(&self) -> Result<Config> {
        ConfigLoader::load_for_update(&self.location)
    }

    fn save(&self, config: &Config) -> Result<()> {
        ConfigLoader::save(config, &self.location.path)
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use memory::InMemoryConfigStore;

#[cfg(any(test, feature = "test-util"))]
mod memory {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use camino::Utf8Path;

    use super::ConfigStore;
    use crate::config::Config;
    use crate::error::Result;

    /// Test [`ConfigStore`] holding the configuration in memory and counting saves.
    #[derive(Debug, Default)]
    pub struct InMemoryConfigStore {
        state: Mutex<(Option<Config>, usize)>,
    }

    impl InMemoryConfigStore {
        /// The path reported by [`ConfigStore::path`].
        pub const PATH: &'static str = "/in-memory/hw/config.toml";

        /// A store holding `config` (`None`: no configuration yet).
        #[must_use]
        pub fn new(config: Option<Config>) -> Self {
            Self {
                state: Mutex::new((config, 0)),
            }
        }

        /// The stored configuration.
        #[must_use]
        pub fn config(&self) -> Option<Config> {
            self.lock().0.clone()
        }

        /// How many times [`ConfigStore::save`] was called.
        #[must_use]
        pub fn saves(&self) -> usize {
            self.lock().1
        }

        fn lock(&self) -> MutexGuard<'_, (Option<Config>, usize)> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl ConfigStore for InMemoryConfigStore {
        fn path(&self) -> &Utf8Path {
            Utf8Path::new(Self::PATH)
        }

        fn load(&self) -> Result<Option<Config>> {
            Ok(self.config())
        }

        fn load_for_update(&self) -> Result<Config> {
            Ok(self.config().unwrap_or_default())
        }

        fn save(&self, config: &Config) -> Result<()> {
            let mut state = self.lock();
            *state = (Some(config.clone()), state.1 + 1);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().join("config.toml")).unwrap();
        let store = FileConfigStore::new(ConfigLocation {
            path: path.clone(),
            explicit: false,
        });
        assert_eq!(store.path(), path);
        assert_eq!(store.load().unwrap(), None);
        let config = Config {
            default_profile: Some("a".into()),
            ..Config::default()
        };
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), Some(config.clone()));
        assert_eq!(store.load_for_update().unwrap(), config);
    }

    #[test]
    fn in_memory_store_counts_saves() {
        let store = InMemoryConfigStore::new(None);
        assert_eq!(store.load_for_update().unwrap(), Config::default());
        store.save(&Config::default()).unwrap();
        assert_eq!(
            (store.config(), store.saves()),
            (Some(Config::default()), 1)
        );
    }
}
