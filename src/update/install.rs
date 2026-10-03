//! [`SystemInstaller`]: finds the running executable and swaps a new build in for it.
//!
//! The new file is staged in a temporary directory next to the executable (same file system),
//! is ad-hoc signed on macOS, gets the old file's permissions, is flushed to disk, and only
//! then takes the executable's name in one step: a failure leaves an executable in place.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use tracing::debug;

use super::{Installer, REPOSITORY};
use crate::error::{Error, Result};

/// Prefix of the staging directory and of the write probe next to the executable.
const STAGING_PREFIX: &str = ".hw-update-";

const HOMEBREW: [&str; 3] = [
    "/opt/homebrew/Cellar/",
    "/usr/local/Cellar/",
    "/home/linuxbrew/.linuxbrew/Cellar/",
];
const NIX: [&str; 1] = ["/nix/store/"];
const SYSTEM: [&str; 4] = ["/usr/bin/", "/usr/sbin/", "/bin/", "/sbin/"];

/// Tries, and the pause between them, to put the new binary under the executable's name after
/// a failed swap left that name empty (a virus scanner may hold the file for a moment).
const RESTORE_ATTEMPTS: u32 = 5;
const RESTORE_PAUSE: Duration = Duration::from_millis(200);

/// The production [`Installer`] for the running executable.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemInstaller;

impl Installer for SystemInstaller {
    fn locate(&self) -> Result<Utf8PathBuf> {
        let exe = std::env::current_exe()
            .map_err(|e| Error::SelfUpdate(format!("cannot find the running hw binary: {e}")))?;
        // A symlink resolves to its target: that file is the one replaced, and a Homebrew or
        // Nix install behind a symlink is recognised. Windows would get a `\\?\` path instead.
        #[cfg(unix)]
        let exe = exe
            .canonicalize()
            .map_err(|e| Error::SelfUpdate(format!("cannot resolve the hw binary path: {e}")))?;
        let exe = Utf8PathBuf::from_path_buf(exe).map_err(|path| {
            Error::SelfUpdate(format!(
                "the hw binary path is not valid UTF-8: {}",
                path.display()
            ))
        })?;
        check_location(&exe)?;
        probe_writable(parent(&exe)?)?;
        Ok(exe)
    }

    fn replace(&self, exe: &Utf8Path, binary: &[u8]) -> Result<()> {
        let dir = parent(exe)?;
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(dir)
            .map_err(|e| Error::SelfUpdate(format!("cannot stage the new binary in {dir}: {e}")))?;
        let staged = staging.path().join(exe.file_name().unwrap_or("hw"));
        stage(&staged, binary, exe)
            .map_err(|e| Error::SelfUpdate(format!("cannot stage the new binary in {dir}: {e}")))?;
        swap(&staged, exe)
            .map_err(|err| keep_an_executable(&staged, exe, err))
            .map_err(|e| Error::SelfUpdate(format!("cannot replace {exe}: {e}")))?;
        #[cfg(unix)]
        sync_dir(dir);
        debug!("replaced {exe}");
        Ok(())
    }
}

/// Refuses an executable a package manager owns: replacing it behind the manager's back
/// would leave the manager's records wrong (or fail, in the read-only Nix store).
pub fn check_location(exe: &Utf8Path) -> Result<()> {
    let path = exe.as_str();
    let under = |prefixes: &[&str]| prefixes.iter().any(|prefix| path.starts_with(prefix));
    let owner = if under(&HOMEBREW) {
        "Homebrew; update it with brew"
    } else if under(&NIX) {
        "Nix; update it through Nix"
    } else if under(&SYSTEM) {
        "the system package manager; update it through the package manager"
    } else {
        return Ok(());
    };
    Err(Error::SelfUpdate(format!(
        "hw at {exe} is managed by {owner}"
    )))
}

/// Creates and removes a file in `dir`, so an unwritable directory fails before any download.
fn probe_writable(dir: &Utf8Path) -> Result<()> {
    tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempfile_in(dir)
        .map(drop)
        .map_err(|e| {
            Error::SelfUpdate(format!(
                "cannot write to {dir} ({e}); rerun with permission to write there or reinstall hw into a directory you own"
            ))
        })
}

/// Writes the new binary, signs it (codesign may rewrite the file, hence the reopen), gives it
/// the old file's permissions and flushes it, so the rename never exposes a partial file.
fn stage(staged: &Path, binary: &[u8], exe: &Utf8Path) -> io::Result<()> {
    std::fs::write(staged, binary)?;
    #[cfg(target_os = "macos")]
    ad_hoc_sign(staged);
    let file = OpenOptions::new().write(true).open(staged)?;
    file.set_permissions(std::fs::metadata(exe)?.permissions())?;
    file.sync_all()
}

/// Persists the rename itself; best effort, as the executable is already replaced.
#[cfg(unix)]
fn sync_dir(dir: &Utf8Path) {
    if let Err(e) = std::fs::File::open(dir).and_then(|dir| dir.sync_all()) {
        debug!("cannot flush {dir}: {e}");
    }
}

fn parent(exe: &Utf8Path) -> Result<&Utf8Path> {
    exe.parent()
        .filter(|dir| !dir.as_str().is_empty())
        .ok_or_else(|| Error::SelfUpdate(format!("{exe} has no parent directory")))
}

/// A rename within one directory is atomic on Unix, and a running binary may be renamed over.
#[cfg(not(windows))]
fn swap(staged: &Path, exe: &Utf8Path) -> io::Result<()> {
    std::fs::rename(staged, exe)
}

/// Windows cannot rename over a running `.exe`: self-replace moves it aside (deleted on exit)
/// and copies the new one in. Any other file is renamed over as on Unix.
#[cfg(windows)]
fn swap(staged: &Path, exe: &Utf8Path) -> io::Result<()> {
    let running = std::env::current_exe()?.canonicalize()?;
    if exe.as_std_path().canonicalize()? == running {
        self_replace::self_replace(staged)
    } else {
        std::fs::rename(staged, exe)
    }
}

/// After a failed swap, `err` as is while `exe` still exists. A swap that failed after moving
/// the old file aside (self-replace on Windows) gets the verified new binary copied under the
/// empty name instead, and the error says so; if even that fails, the error says `exe` is gone.
fn keep_an_executable(staged: &Path, exe: &Utf8Path, err: io::Error) -> io::Error {
    if exe.exists() {
        return err;
    }
    for attempt in 1..=RESTORE_ATTEMPTS {
        match std::fs::copy(staged, exe) {
            Ok(_) => {
                return io::Error::new(
                    err.kind(),
                    format!("{err}; the new binary was copied to {exe} instead"),
                );
            }
            Err(e) if attempt < RESTORE_ATTEMPTS => {
                debug!("cannot copy the new binary to {exe} yet: {e}");
                std::thread::sleep(RESTORE_PAUSE);
            }
            Err(e) => debug!("cannot copy the new binary to {exe}: {e}"),
        }
    }
    io::Error::new(
        err.kind(),
        format!(
            "{err}; {exe} is missing now, reinstall hw from https://github.com/{REPOSITORY}/releases"
        ),
    )
}

/// Re-signs ad hoc, as `scripts/install.sh` does, so the keychain keeps trusting `hw` without
/// a password prompt. Best effort: a failure is a warning. An absolute path, so `PATH` cannot
/// substitute another program.
#[cfg(target_os = "macos")]
fn ad_hoc_sign(path: &Path) {
    let result = std::process::Command::new("/usr/bin/codesign")
        .args(["-s", "-", "-f"])
        .arg(path)
        .output();
    let problem = match result {
        Ok(output) if output.status.success() => {
            debug!("ad-hoc signed {}", path.display());
            return;
        }
        Ok(output) => format!(
            "codesign failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(e) => format!("cannot run codesign: {e}"),
    };
    tracing::warn!(
        "{problem}; the updated hw is not re-signed and macOS may ask for the keychain password"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf8(path: &Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap()
    }

    #[test]
    fn package_managed_locations_are_refused() {
        for (path, owner) in [
            ("/opt/homebrew/Cellar/hw/2026.40.1/bin/hw", "Homebrew"),
            ("/usr/local/Cellar/hw/2026.40.1/bin/hw", "Homebrew"),
            (
                "/home/linuxbrew/.linuxbrew/Cellar/hw/2026.40.1/bin/hw",
                "Homebrew",
            ),
            ("/nix/store/abc-hw-2026.40.1/bin/hw", "Nix"),
            ("/usr/bin/hw", "system package manager"),
            ("/usr/sbin/hw", "system package manager"),
        ] {
            let err = check_location(Utf8Path::new(path)).unwrap_err();
            assert!(
                matches!(&err, Error::SelfUpdate(m) if m.contains(owner) && m.contains(path)),
                "{path}: {err}"
            );
        }
    }

    #[test]
    fn user_locations_are_accepted() {
        for path in [
            "/usr/local/bin/hw",
            "/home/me/.local/bin/hw",
            "/home/me/.cargo/bin/hw",
            "/Users/me/bin/hw",
            "/opt/tools/hw",
            "/srv/Cellar/hw",
            "/home/me/homebrew-tools/hw",
            "C:\\Users\\me\\bin\\hw.exe",
        ] {
            check_location(Utf8Path::new(path)).unwrap();
        }
    }

    #[test]
    fn probe_detects_missing_directory_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        probe_writable(&root).unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        let err = probe_writable(&root.join("missing")).unwrap_err();
        assert!(
            matches!(&err, Error::SelfUpdate(m) if m.contains("cannot write to")),
            "{err}"
        );
    }

    #[test]
    fn replace_swaps_the_file_keeps_permissions_and_leaves_no_staging() {
        let dir = tempfile::tempdir().unwrap();
        let exe = utf8(dir.path()).join("hw");
        std::fs::write(&exe, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o750)).unwrap();
        }

        SystemInstaller.replace(&exe, b"new").unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o750);
        }
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["hw"]);
    }

    #[test]
    fn failed_swap_with_the_executable_in_place_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        let (staged, exe) = (root.join("staged"), root.join("hw"));
        std::fs::write(&staged, b"new").unwrap();
        std::fs::write(&exe, b"old").unwrap();

        let err = keep_an_executable(staged.as_std_path(), &exe, io::Error::other("locked"));

        assert_eq!(err.to_string(), "locked");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[test]
    fn failed_swap_that_moved_the_executable_aside_puts_the_new_binary_there() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        let (staged, exe) = (root.join("staged"), root.join("hw"));
        std::fs::write(&staged, b"new").unwrap();

        let err = keep_an_executable(staged.as_std_path(), &exe, io::Error::other("locked"));

        assert_eq!(
            err.to_string(),
            format!("locked; the new binary was copied to {exe} instead")
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    }

    #[test]
    fn failed_restore_reports_the_missing_executable() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        let exe = root.join("hw");

        let missing = root.join("no-such-staged-file");
        let err = keep_an_executable(missing.as_std_path(), &exe, io::Error::other("locked"));

        assert!(
            err.to_string()
                .starts_with(&format!("locked; {exe} is missing now, reinstall hw")),
            "{err}"
        );
    }

    #[test]
    fn replace_without_the_old_file_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = utf8(dir.path()).join("hw");
        let err = SystemInstaller.replace(&exe, b"new").unwrap_err();
        assert!(matches!(err, Error::SelfUpdate(_)), "{err}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
