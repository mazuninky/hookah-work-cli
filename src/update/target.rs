//! Platforms with prebuilt release archives and the names of their assets; mirrors the build
//! matrix and the packaging steps of `.github/workflows/release.yml`.

use super::Version;

/// How a release archive is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// Gzip-compressed tarball (Linux, macOS).
    TarGz,
    /// Zip (Windows).
    Zip,
}

impl ArchiveKind {
    /// File extension of the archive, without the dot.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

/// A platform `hw` publishes release archives for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Rust target triple, part of the asset name.
    pub triple: &'static str,
    /// Archive format.
    pub archive: ArchiveKind,
    /// File name of the executable inside the archive.
    pub binary: &'static str,
}

impl Target {
    /// x86_64 Linux (glibc).
    pub const LINUX_X86_64: Self = Self {
        triple: "x86_64-unknown-linux-gnu",
        archive: ArchiveKind::TarGz,
        binary: "hw",
    };

    /// Apple Silicon macOS.
    pub const MACOS_AARCH64: Self = Self {
        triple: "aarch64-apple-darwin",
        archive: ArchiveKind::TarGz,
        binary: "hw",
    };

    /// x86_64 Windows (MSVC).
    pub const WINDOWS_X86_64: Self = Self {
        triple: "x86_64-pc-windows-msvc",
        archive: ArchiveKind::Zip,
        binary: "hw.exe",
    };

    /// Every published target.
    pub const ALL: [Self; 3] = [
        Self::LINUX_X86_64,
        Self::MACOS_AARCH64,
        Self::WINDOWS_X86_64,
    ];

    /// The target this binary was compiled for, if releases are published for it.
    #[must_use]
    pub const fn current() -> Option<Self> {
        if cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) {
            Some(Self::LINUX_X86_64)
        } else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
            Some(Self::MACOS_AARCH64)
        } else if cfg!(all(
            target_arch = "x86_64",
            target_os = "windows",
            target_env = "msvc"
        )) {
            Some(Self::WINDOWS_X86_64)
        } else {
            None
        }
    }

    /// `hw-<version>-<triple>`: the archive's stem and its only top-level directory.
    fn stem(self, version: &Version) -> String {
        format!("hw-{version}-{}", self.triple)
    }

    /// The release asset, e.g. `hw-2026.40.1-aarch64-apple-darwin.tar.gz`.
    #[must_use]
    pub fn asset_name(self, version: &Version) -> String {
        format!("{}.{}", self.stem(version), self.archive.extension())
    }

    /// The checksum sidecar published next to [`Target::asset_name`].
    #[must_use]
    pub fn checksum_name(self, version: &Version) -> String {
        format!("{}.sha256", self.asset_name(version))
    }

    /// Path of the executable inside the archive, e.g. `hw-2026.40.1-x86_64-pc-windows-msvc/hw.exe`.
    #[must_use]
    pub fn binary_path(self, version: &Version) -> String {
        format!("{}/{}", self.stem(version), self.binary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: Version = Version::new(2026, 41, 2);

    #[test]
    fn asset_names_follow_the_release_workflow() {
        let names: Vec<(String, String, String)> = Target::ALL
            .iter()
            .map(|t| (t.asset_name(&V), t.checksum_name(&V), t.binary_path(&V)))
            .collect();
        assert_eq!(
            names,
            [
                (
                    "hw-2026.41.2-x86_64-unknown-linux-gnu.tar.gz",
                    "hw-2026.41.2-x86_64-unknown-linux-gnu.tar.gz.sha256",
                    "hw-2026.41.2-x86_64-unknown-linux-gnu/hw",
                ),
                (
                    "hw-2026.41.2-aarch64-apple-darwin.tar.gz",
                    "hw-2026.41.2-aarch64-apple-darwin.tar.gz.sha256",
                    "hw-2026.41.2-aarch64-apple-darwin/hw",
                ),
                (
                    "hw-2026.41.2-x86_64-pc-windows-msvc.zip",
                    "hw-2026.41.2-x86_64-pc-windows-msvc.zip.sha256",
                    "hw-2026.41.2-x86_64-pc-windows-msvc/hw.exe",
                ),
            ]
            .map(|(a, c, b)| (a.to_owned(), c.to_owned(), b.to_owned()))
        );
    }

    #[test]
    fn current_target_matches_the_build() {
        let expected = match (
            std::env::consts::ARCH,
            std::env::consts::OS,
            cfg!(target_env = "gnu"),
            cfg!(target_env = "msvc"),
        ) {
            ("x86_64", "linux", true, _) => Some(Target::LINUX_X86_64),
            ("aarch64", "macos", _, _) => Some(Target::MACOS_AARCH64),
            ("x86_64", "windows", _, true) => Some(Target::WINDOWS_X86_64),
            _ => None,
        };
        assert_eq!(Target::current(), expected);
    }
}
