//! [`Secret`]: a token or password that never shows up in `Debug` output.

use std::fmt;

/// A credential string. It has no `Display`/`Serialize`, and `Debug` prints `Secret(***)`,
/// so it can only leave the process through an explicit [`Secret::expose`] call.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a credential.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value — for the HTTP header, the keyring, the config file and `hw auth token` only.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the value is empty or whitespace only.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let secret = Secret::new("s3cr3t-token");
        let debug = format!("{secret:?} {:?}", Some(&secret));
        assert!(!debug.contains("s3cr3t"), "{debug}");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
    }

    #[test]
    fn expose_returns_the_value() {
        assert_eq!(Secret::new("abc").expose(), "abc");
    }

    #[test]
    fn blank_detects_whitespace() {
        assert!(Secret::new("  \n").is_blank());
        assert!(!Secret::new("x").is_blank());
    }
}
