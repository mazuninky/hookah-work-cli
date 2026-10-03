//! Self-update machinery behind `hw self`: release [`Version`]s, the published [`Target`]s,
//! checksum and archive helpers, and the two seams the handlers depend on — [`ReleaseSource`]
//! (GitHub in production) and [`Installer`] (the running executable). Nothing here talks to
//! the CRM.

mod archive;
mod github;
mod install;
mod target;
mod version;

pub use archive::{
    MAX_BINARY_SIZE, extract_binary, parse_sha256_sidecar, sha256_hex, verify_sha256,
};
pub use github::{GitHubReleases, MAX_DOWNLOAD_SIZE, REPOSITORY, release_page_url};
pub use install::{SystemInstaller, check_location};
pub use target::{ArchiveKind, Target};
pub use version::{InvalidVersion, Version};

use camino::{Utf8Path, Utf8PathBuf};

use crate::error::Result;

/// Where `hw` releases come from.
pub trait ReleaseSource {
    /// The newest published release; none at all is [`Error::NotFound`](crate::Error::NotFound).
    fn latest(&self) -> Result<Version>;

    /// The bytes of asset `name` of release `version`; a missing release or asset is
    /// [`Error::NotFound`](crate::Error::NotFound).
    fn download(&self, version: &Version, name: &str) -> Result<Vec<u8>>;
}

/// The running executable and how to swap it.
pub trait Installer {
    /// The executable to replace, once it is known to be replaceable: not owned by a package
    /// manager, in a writable directory. Runs before anything is downloaded.
    fn locate(&self) -> Result<Utf8PathBuf>;

    /// Replaces `exe` (as returned by [`Installer::locate`]) with `binary` in one step; on
    /// failure the old executable stays in place.
    fn replace(&self, exe: &Utf8Path, binary: &[u8]) -> Result<()>;
}
