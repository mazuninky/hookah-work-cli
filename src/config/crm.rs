//! CRM address normalisation: `demo`, `demo.hookah.work` and full base URLs all
//! resolve to one base URL, the host used as the keyring account and the origin that scopes a
//! profile's token.

use std::net::{IpAddr, Ipv6Addr};

use tracing::warn;

use crate::error::{Error, Result};

/// Domain under which every CRM gets its own subdomain.
pub const CRM_DOMAIN: &str = "hookah.work";

/// CRM page where a director generates an API key, relative to the base URL.
pub const API_KEY_PATH: &str = "/v2/settings/users";

/// [`Crm::api_key_url`] for a CRM that is not known yet.
pub const API_KEY_URL_TEMPLATE: &str = "https://<crm>.hookah.work/v2/settings/users";

/// A normalised CRM address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crm {
    base_url: String,
    host: String,
    origin: String,
}

impl Crm {
    /// Normalises a subdomain (`demo`), host (`demo.hookah.work`) or URL
    /// (`https://…`, `http://127.0.0.1:1234`). Hosts are lowercased; a trailing `/` and a
    /// trailing `/api` are stripped; bare subdomains and hosts get `https://`.
    pub fn parse(input: &str) -> Result<Self> {
        let raw = input.trim();
        if raw.is_empty() {
            return Err(invalid(input, "it is empty"));
        }
        if raw.chars().any(char::is_control) {
            return Err(invalid(input, "control characters are not allowed"));
        }

        let (scheme, rest) = match raw.split_once("://") {
            Some((scheme, rest)) => {
                let scheme = scheme.to_ascii_lowercase();
                if scheme != "https" && scheme != "http" {
                    return Err(invalid(
                        input,
                        "only http:// and https:// URLs are supported",
                    ));
                }
                (scheme, rest)
            }
            None => ("https".to_owned(), raw),
        };

        let rest = strip_api_suffix(rest);
        let (host, path) = match rest.find('/') {
            Some(idx) => rest.split_at(idx),
            None => (rest, ""),
        };
        if host.is_empty() {
            return Err(invalid(input, "the host is missing"));
        }
        if let Some(bad) = host.chars().find(|c| !is_host_char(*c)) {
            return Err(invalid(input, &format!("'{bad}' is not allowed in a host")));
        }
        if path
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '?' | '#'))
        {
            return Err(invalid(input, "the URL must not carry a query or fragment"));
        }

        let mut host = host.to_ascii_lowercase();
        let is_bare_subdomain = !raw.contains("://") && !host.contains(['.', ':', '[']);
        if is_bare_subdomain {
            if host.starts_with('-') || host.ends_with('-') {
                return Err(invalid(input, "a subdomain cannot start or end with '-'"));
            }
            host = format!("{host}.{CRM_DOMAIN}");
        }

        Ok(Self {
            base_url: format!("{scheme}://{host}{path}"),
            origin: format!("{scheme}://{}", without_default_port(&host, &scheme)),
            host,
        })
    }

    /// Base URL without a trailing slash, e.g. `https://demo.hookah.work`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Host (with port, if any), e.g. `demo.hookah.work` — the keyring account.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Scheme, host and port, with a default port (`:443`, `:80`) dropped, e.g.
    /// `https://demo.hookah.work`. A profile's token is only sent to its own origin.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Whether credentials sent to this CRM would cross the network unencrypted:
    /// `http://` to anything but a loopback host (`localhost`, `127.0.0.0/8`, `[::1]`).
    #[must_use]
    pub fn is_cleartext(&self) -> bool {
        self.base_url.starts_with("http://") && !is_loopback(&self.host)
    }

    /// Logs a warning when [`Crm::is_cleartext`]; call it before a credential is sent.
    pub fn warn_if_cleartext(&self) {
        if self.is_cleartext() {
            warn!(
                "{} uses plain http://: the token or password travels unencrypted",
                self.base_url
            );
        }
    }

    /// The API key page, e.g. `https://demo.hookah.work/v2/settings/users`.
    #[must_use]
    pub fn api_key_url(&self) -> String {
        format!("{}{API_KEY_PATH}", self.base_url)
    }

    /// First host label (`demo`), the default profile name for `hw auth login`.
    #[must_use]
    pub fn label(&self) -> &str {
        let without_port = self.host.split(':').next().unwrap_or(&self.host);
        without_port.split('.').next().unwrap_or(without_port)
    }
}

/// `host` is `name[:port]` or `[v6][:port]`, as produced by [`Crm::parse`].
fn is_loopback(host: &str) -> bool {
    if let Some(v6) = host.strip_prefix('[') {
        let addr = v6.split(']').next().unwrap_or_default();
        return addr.parse::<Ipv6Addr>().is_ok_and(|ip| ip.is_loopback());
    }
    let name = host.split(':').next().unwrap_or(host);
    name == "localhost"
        || name.ends_with(".localhost")
        || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `host` without an explicit port that equals the scheme's default (or is empty).
fn without_default_port<'a>(host: &'a str, scheme: &str) -> &'a str {
    let (name, port) = if host.starts_with('[') {
        match host.split_once("]:") {
            Some((v6, port)) => (&host[..=v6.len()], port),
            None => return host,
        }
    } else {
        match host.split_once(':') {
            Some((name, port)) if !port.contains(':') => (name, port),
            _ => return host,
        }
    };
    let default = if scheme == "https" { 443 } else { 80 };
    if port.is_empty() || port.parse::<u16>() == Ok(default) {
        name
    } else {
        host
    }
}

fn strip_api_suffix(mut rest: &str) -> &str {
    rest = rest.trim_end_matches('/');
    if let Some(stripped) = rest.strip_suffix("/api") {
        rest = stripped.trim_end_matches('/');
    }
    rest
}

/// Letters, digits, `-`, `.`, plus `:` and brackets for ports and IPv6 literals.
/// Rejecting `@` keeps credentials out of the URL.
fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '[' | ']')
}

fn invalid(input: &str, why: &str) -> Error {
    // The input is echoed to the terminal (also as a prompt's validation message), so control
    // characters are shown escaped rather than interpreted.
    let mut shown = String::with_capacity(input.len());
    for c in input.trim().chars() {
        if c.is_control() {
            shown.extend(c.escape_default());
        } else {
            shown.push(c);
        }
    }
    Error::InvalidInput(format!(
        "invalid CRM '{shown}': {why}; pass a subdomain (demo), a host (demo.hookah.work) or a base URL"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(input: &str) -> String {
        Crm::parse(input).unwrap().base_url().to_owned()
    }

    #[test]
    fn subdomain_expands_to_hookah_work() {
        assert_eq!(base("demo"), "https://demo.hookah.work");
        assert_eq!(base("  DEMO "), "https://demo.hookah.work");
    }

    #[test]
    fn api_key_url_hangs_off_the_base_url() {
        let url = |input: &str| Crm::parse(input).unwrap().api_key_url();
        assert_eq!(url("demo"), "https://demo.hookah.work/v2/settings/users");
        assert_eq!(
            url("http://127.0.0.1:4010/api/"),
            "http://127.0.0.1:4010/v2/settings/users"
        );
        assert_eq!(
            API_KEY_URL_TEMPLATE,
            format!("https://<crm>.{CRM_DOMAIN}{API_KEY_PATH}")
        );
    }

    #[test]
    fn host_gets_https() {
        assert_eq!(base("demo.hookah.work"), "https://demo.hookah.work");
        assert_eq!(base("demo.hookah.work/api/"), "https://demo.hookah.work");
    }

    #[test]
    fn urls_are_kept_minus_trailing_slash_and_api() {
        assert_eq!(
            base("https://demo.hookah.work/"),
            "https://demo.hookah.work"
        );
        assert_eq!(
            base("https://demo.hookah.work/api"),
            "https://demo.hookah.work"
        );
        assert_eq!(base("http://127.0.0.1:1234"), "http://127.0.0.1:1234");
        assert_eq!(base("http://127.0.0.1:1234/api/"), "http://127.0.0.1:1234");
    }

    #[test]
    fn host_and_label() {
        let crm = Crm::parse("demo").unwrap();
        assert_eq!(crm.host(), "demo.hookah.work");
        assert_eq!(crm.label(), "demo");
        let local = Crm::parse("http://127.0.0.1:1234").unwrap();
        assert_eq!(local.host(), "127.0.0.1:1234");
    }

    #[test]
    fn origin_is_scheme_host_and_non_default_port() {
        let origin = |input: &str| Crm::parse(input).unwrap().origin().to_owned();
        for same in [
            "demo",
            "DEMO.hookah.work",
            "https://demo.hookah.work/",
            "https://demo.hookah.work:443/api",
            "HTTPS://demo.hookah.work/sub",
        ] {
            assert_eq!(origin(same), "https://demo.hookah.work", "{same}");
        }
        assert_eq!(
            origin("http://demo.hookah.work:80"),
            "http://demo.hookah.work"
        );
        assert_eq!(
            origin("http://demo.hookah.work:443"),
            "http://demo.hookah.work:443"
        );
        assert_eq!(
            origin("https://demo.hookah.work:8443"),
            "https://demo.hookah.work:8443"
        );
        assert_eq!(origin("http://127.0.0.1:1234/api"), "http://127.0.0.1:1234");
        assert_eq!(origin("https://[::1]:443"), "https://[::1]");
        assert_eq!(origin("http://[::1]:8080"), "http://[::1]:8080");
    }

    #[test]
    fn cleartext_is_http_off_loopback() {
        for remote in [
            "http://demo.hookah.work",
            "http://10.0.0.5:8080",
            "http://[::2]",
        ] {
            assert!(Crm::parse(remote).unwrap().is_cleartext(), "{remote}");
        }
        for safe in [
            "demo",
            "https://10.0.0.5",
            "http://127.0.0.1:1234",
            "http://localhost:8080",
            "http://api.localhost",
            "http://[::1]:8080",
        ] {
            assert!(!Crm::parse(safe).unwrap().is_cleartext(), "{safe}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        for input in [
            "",
            "   ",
            "got cha",
            "got_cha",
            "-demo",
            "ftp://x.y",
            "https://",
            "user@host",
        ] {
            let err = Crm::parse(input).unwrap_err();
            assert!(matches!(err, Error::InvalidInput(_)), "{input:?}: {err:?}");
        }
    }

    #[test]
    fn rejects_control_characters_and_echoes_them_escaped() {
        for (input, shown) in [
            (
                "https://demo.hookah.work/\u{1b}[31m",
                "https://demo.hookah.work/\\u{1b}[31m",
            ),
            ("demo.hookah.work/a\u{7f}", "demo.hookah.work/a\\u{7f}"),
            ("got\u{0}cha", "got\\u{0}cha"),
            ("demo.hookah.work/a\tb", "demo.hookah.work/a\\tb"),
            (
                "https://demo.hookah.work/\u{9b}2J",
                "https://demo.hookah.work/\\u{9b}2J",
            ),
        ] {
            let Err(Error::InvalidInput(message)) = Crm::parse(input) else {
                panic!("{input:?} was accepted");
            };
            assert!(
                message.starts_with(&format!(
                    "invalid CRM '{shown}': control characters are not allowed"
                )),
                "{message}"
            );
            assert!(!message.chars().any(char::is_control), "{message:?}");
        }
        assert_eq!(
            base(" demo\n"),
            "https://demo.hookah.work",
            "trimmed, not inside"
        );
    }
}
