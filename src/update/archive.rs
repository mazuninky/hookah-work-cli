//! Integrity check and unpacking of a downloaded release archive, both in memory: only the
//! executable's bytes are read out, so no archive path ever reaches the file system.

use std::fmt::Write as _;
use std::io::{Cursor, Read};

use sha2::{Digest as _, Sha256};

use super::ArchiveKind;
use crate::error::{Error, Result};

/// Upper bound for the unpacked executable (release builds are a few MB).
pub const MAX_BINARY_SIZE: u64 = 256 * 1024 * 1024;

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The digest from a `.sha256` sidecar for `asset`: `<64 hex digits>  <file name>` as written by
/// `shasum -a 256` (uppercase hex, a `*` before the name, a directory prefix and CRLF are
/// accepted). The name is required and must be `asset`: otherwise the digest may belong to a
/// different archive.
pub fn parse_sha256_sidecar(text: &str, asset: &str) -> Result<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| bad_sidecar(asset, "it is empty"))?;
    let mut fields = line.split_whitespace();
    let digest = fields.next().unwrap_or_default();
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad_sidecar(
            asset,
            "it does not start with a SHA-256 hex digest",
        ));
    }
    let name = fields
        .next()
        .map(|name| name.trim_start_matches('*'))
        .filter(|name| !name.is_empty())
        .ok_or_else(|| bad_sidecar(asset, "it names no file"))?;
    let file_name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if file_name != asset {
        return Err(bad_sidecar(asset, &format!("it is for {name}")));
    }
    Ok(digest.to_ascii_lowercase())
}

fn bad_sidecar(asset: &str, why: &str) -> Error {
    Error::SelfUpdate(format!("invalid checksum file for {asset}: {why}"))
}

/// Fails unless `bytes` hash to `expected` (hex, any case); runs before anything is unpacked.
pub fn verify_sha256(bytes: &[u8], expected: &str, asset: &str) -> Result<()> {
    let actual = sha256_hex(bytes);
    let expected = expected.to_ascii_lowercase();
    if actual == expected {
        Ok(())
    } else {
        Err(Error::SelfUpdate(format!(
            "checksum mismatch for {asset}: expected {expected}, got {actual}; nothing was replaced"
        )))
    }
}

/// Reads the regular file at `path` (e.g. `hw-2026.40.1-aarch64-apple-darwin/hw`) out of `archive`.
pub fn extract_binary(archive: &[u8], kind: ArchiveKind, path: &str) -> Result<Vec<u8>> {
    let found = match kind {
        ArchiveKind::TarGz => from_tar_gz(archive, path),
        ArchiveKind::Zip => from_zip(archive, path),
    }?;
    match found {
        Some(binary) if !binary.is_empty() => Ok(binary),
        Some(_) => Err(Error::SelfUpdate(format!(
            "{path} in the release archive is empty"
        ))),
        None => Err(Error::SelfUpdate(format!(
            "the release archive has no {path}"
        ))),
    }
}

fn from_tar_gz(archive: &[u8], path: &str) -> Result<Option<Vec<u8>>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(unreadable)? {
        let mut entry = entry.map_err(unreadable)?;
        if entry.header().entry_type().is_file()
            && normalize(&String::from_utf8_lossy(&entry.path_bytes())) == path
        {
            return read_limited(&mut entry, path).map(Some);
        }
    }
    Ok(None)
}

fn from_zip(archive: &[u8], path: &str) -> Result<Option<Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(unreadable)?;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(unreadable)?;
        if file.is_file() && normalize(file.name()) == path {
            return read_limited(&mut file, path).map(Some);
        }
    }
    Ok(None)
}

/// Windows PowerShell may store `\` separators; tar may prefix `./`.
fn normalize(name: &str) -> String {
    let name = name.replace('\\', "/");
    name.strip_prefix("./").unwrap_or(&name).to_owned()
}

fn read_limited(reader: &mut dyn Read, path: &str) -> Result<Vec<u8>> {
    let mut binary = Vec::new();
    reader
        .take(MAX_BINARY_SIZE + 1)
        .read_to_end(&mut binary)
        .map_err(unreadable)?;
    if binary.len() as u64 > MAX_BINARY_SIZE {
        return Err(Error::SelfUpdate(format!(
            "{path} in the release archive is larger than {MAX_BINARY_SIZE} bytes"
        )));
    }
    Ok(binary)
}

fn unreadable(err: impl std::fmt::Display) -> Error {
    Error::SelfUpdate(format!("cannot read the release archive: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{tar_gz_archive, zip_archive};

    /// SHA-256 of `abc`.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const ASSET: &str = "hw-2026.41.1-aarch64-apple-darwin.tar.gz";
    const BIN: &str = "hw-2026.41.1-aarch64-apple-darwin/hw";

    #[test]
    fn sha256_hex_is_lowercase_hex() {
        assert_eq!(sha256_hex(b"abc"), ABC);
    }

    #[test]
    fn sidecar_accepts_shasum_and_powershell_forms() {
        let upper = ABC.to_ascii_uppercase();
        for text in [
            format!("{ABC}  {ASSET}\n"),
            format!("{ABC}  {ASSET}"),
            format!("{ABC} *{ASSET}\r\n"),
            format!("\n{upper}\t{ASSET}\n"),
            format!("{ABC}  dist/{ASSET}\n"),
        ] {
            assert_eq!(parse_sha256_sidecar(&text, ASSET).unwrap(), ABC, "{text:?}");
        }
    }

    #[test]
    fn sidecar_rejects_garbage_and_other_files() {
        for (text, why) in [
            (String::new(), "empty"),
            (" \n\t\n".to_owned(), "empty"),
            (format!("{ABC}\n"), "names no file"),
            (format!("{ABC}  *\n"), "names no file"),
            (format!("{}  {ASSET}", &ABC[1..]), "digest"),
            (format!("{ABC}0  {ASSET}"), "digest"),
            (format!("{}  {ASSET}", "z".repeat(64)), "digest"),
            ("<html>Not Found</html>".to_owned(), "digest"),
            (
                format!("{ABC}  hw-2026.41.1-x86_64-unknown-linux-gnu.tar.gz"),
                "it is for",
            ),
        ] {
            let err = parse_sha256_sidecar(&text, ASSET).unwrap_err();
            assert!(
                matches!(&err, Error::SelfUpdate(m) if m.contains(why) && m.contains(ASSET)),
                "{text:?}: {err}"
            );
        }
    }

    #[test]
    fn verify_accepts_matching_digest_in_any_case() {
        verify_sha256(b"abc", ABC, ASSET).unwrap();
        verify_sha256(b"abc", &ABC.to_ascii_uppercase(), ASSET).unwrap();
    }

    #[test]
    fn verify_rejects_mismatch_naming_both_digests() {
        let err = verify_sha256(b"xyz", ABC, ASSET).unwrap_err();
        let message = err.to_string();
        assert!(matches!(err, Error::SelfUpdate(_)));
        assert!(
            message.contains("checksum mismatch") && message.contains(ASSET),
            "{message}"
        );
        assert!(
            message.contains(ABC) && message.contains(&sha256_hex(b"xyz")),
            "{message}"
        );
    }

    #[test]
    fn extracts_the_binary_from_a_tarball() {
        let archive = tar_gz_archive(&[
            (
                "hw-2026.41.1-aarch64-apple-darwin/LICENSE",
                b"MIT".as_slice(),
            ),
            (BIN, b"new-binary".as_slice()),
            (
                "hw-2026.41.1-aarch64-apple-darwin/man/hw.1",
                b".TH".as_slice(),
            ),
        ]);
        assert_eq!(
            extract_binary(&archive, ArchiveKind::TarGz, BIN).unwrap(),
            b"new-binary"
        );
        let dotted = tar_gz_archive(&[(&format!("./{BIN}"), b"dotted".as_slice())]);
        assert_eq!(
            extract_binary(&dotted, ArchiveKind::TarGz, BIN).unwrap(),
            b"dotted"
        );
    }

    #[test]
    fn extracts_the_binary_from_a_zip_with_either_separator() {
        let path = "hw-2026.41.1-x86_64-pc-windows-msvc/hw.exe";
        for name in [path.to_owned(), path.replace('/', "\\")] {
            let archive = zip_archive(&[
                (
                    "hw-2026.41.1-x86_64-pc-windows-msvc/LICENSE",
                    b"MIT".as_slice(),
                ),
                (&name, b"MZ-binary".as_slice()),
            ]);
            assert_eq!(
                extract_binary(&archive, ArchiveKind::Zip, path).unwrap(),
                b"MZ-binary",
                "{name}"
            );
        }
    }

    #[test]
    fn missing_or_empty_binary_is_an_error() {
        let wrong_version =
            tar_gz_archive(&[("hw-2026.40.1-aarch64-apple-darwin/hw", b"x".as_slice())]);
        let empty = tar_gz_archive(&[(BIN, b"".as_slice())]);
        for (archive, why) in [(wrong_version, "has no"), (empty, "empty")] {
            let err = extract_binary(&archive, ArchiveKind::TarGz, BIN).unwrap_err();
            assert!(
                matches!(&err, Error::SelfUpdate(m) if m.contains(why) && m.contains(BIN)),
                "{err}"
            );
        }
    }

    #[test]
    fn corrupt_archive_is_an_error() {
        for kind in [ArchiveKind::TarGz, ArchiveKind::Zip] {
            let err = extract_binary(b"definitely not an archive", kind, BIN).unwrap_err();
            assert!(
                matches!(&err, Error::SelfUpdate(m) if m.starts_with("cannot read the release archive")),
                "{kind:?}: {err}"
            );
        }
    }
}
