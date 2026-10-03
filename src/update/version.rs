//! Release versions: calendar `YEAR.WEEK.BUILD` (`docs/releasing.md`), compared numerically.

use std::fmt;
use std::str::FromStr;

/// A release version such as `2026.40.1`, ordered as a numeric `(year, week, build)` triple.
///
/// A week with a leading zero parses to the same value (`2027.01.1` == `2027.1.1`); it always
/// prints unpadded, which is the spelling of release tags and asset names (Cargo forbids
/// leading zeros in a package version).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    year: u16,
    week: u8,
    build: u32,
}

/// Text that is not a `YEAR.WEEK.BUILD` release version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected YEAR.WEEK.BUILD, e.g. 2026.40.1")]
pub struct InvalidVersion;

impl Version {
    /// A version from its parts, unchecked (user input goes through [`str::parse`]).
    #[must_use]
    pub const fn new(year: u16, week: u8, build: u32) -> Self {
        Self { year, week, build }
    }

    /// The version of this binary (`CARGO_PKG_VERSION`).
    pub fn current() -> Result<Self, InvalidVersion> {
        env!("CARGO_PKG_VERSION").parse()
    }
}

impl FromStr for Version {
    type Err = InvalidVersion;

    /// `YEAR.WEEK.BUILD` with an optional `v` prefix: a four-digit year, week 1-53 (one or two
    /// digits), any build number.
    fn from_str(text: &str) -> Result<Self, InvalidVersion> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let (Some(year), Some(week), Some(build), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(InvalidVersion);
        };
        let year = number(year, 4)
            .filter(|_| year.len() == 4)
            .and_then(|y| u16::try_from(y).ok());
        let week = number(week, 2)
            .filter(|w| (1..=53).contains(w))
            .and_then(|w| u8::try_from(w).ok());
        let build = number(build, 10);
        match (year, week, build) {
            (Some(year), Some(week), Some(build)) => Ok(Self::new(year, week, build)),
            _ => Err(InvalidVersion),
        }
    }
}

/// ASCII digits only (no sign, no spaces), at most `max_len` of them.
fn number(part: &str, max_len: usize) -> Option<u32> {
    let digits =
        !part.is_empty() && part.len() <= max_len && part.bytes().all(|b| b.is_ascii_digit());
    digits.then(|| part.parse().ok()).flatten()
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.year, self.week, self.build)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        text.parse().unwrap()
    }

    #[test]
    fn parses_with_and_without_v_prefix() {
        assert_eq!(v("2026.40.1"), Version::new(2026, 40, 1));
        assert_eq!(v("v2026.40.1"), Version::new(2026, 40, 1));
        assert_eq!(v("2026.40.1").to_string(), "2026.40.1");
    }

    #[test]
    fn padded_week_equals_unpadded_and_prints_unpadded() {
        assert_eq!(v("2027.01.1"), v("2027.1.1"));
        assert_eq!(v("v2027.01.3").to_string(), "2027.1.3");
    }

    #[test]
    fn orders_numerically() {
        assert!(v("2026.9.1") < v("2026.10.1"));
        assert!(v("2026.40.2") > v("2026.40.1"));
        assert!(v("2026.40.10") > v("2026.40.9"));
        assert!(v("2027.1.1") > v("2026.52.99"));
    }

    #[test]
    fn rejects_malformed() {
        for text in [
            "",
            "v",
            "2026.40",
            "2026.40.1.2",
            "2026..1",
            "2026.40.",
            "26.40.1",
            "20260.40.1",
            "2026.0.1",
            "2026.54.1",
            "2026.001.1",
            "2026.40.-1",
            "2026.+4.1",
            " 2026.40.1",
            "2026.40.1 ",
            "V2026.40.1",
            "vv2026.40.1",
            "2026.40.99999999999",
            "latest",
        ] {
            assert_eq!(text.parse::<Version>(), Err(InvalidVersion), "{text:?}");
        }
    }

    #[test]
    fn current_build_version_parses() {
        assert_eq!(
            Version::current().unwrap().to_string(),
            env!("CARGO_PKG_VERSION")
        );
    }
}
