//! [`GitHubReleases`]: the production [`ReleaseSource`], github.com without its REST API (and
//! so without the API rate limit): the latest release is read from the `/releases/latest`
//! redirect, as `scripts/install.sh` does, and assets from release downloads. No request
//! carries an `Authorization` header: neither the CRM token nor any GitHub token is sent.

use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::{ReleaseSource, Version};
use crate::client::{RetryPolicy, USER_AGENT};
use crate::error::{Error, Result};

/// `owner/name` of the repository `hw` releases are published in.
pub const REPOSITORY: &str = "mazuninky/hookah-work-cli";

const WEB_BASE: &str = "https://github.com";

/// Largest release asset `hw` downloads (release archives are a few MB).
pub const MAX_DOWNLOAD_SIZE: u64 = 256 * 1024 * 1024;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// Total time of one lookup or download, retries and their waits included: each attempt may
/// use what is left, so a stalled transfer ends the request instead of starting another one.
const REQUEST_BUDGET: Duration = Duration::from_secs(300);

/// The GitHub page of release `version`.
#[must_use]
pub fn release_page_url(version: &Version) -> String {
    format!("{WEB_BASE}/{REPOSITORY}/releases/tag/v{version}")
}

/// Release lookups and downloads over ureq, retried per a [`RetryPolicy`] (429 waits
/// `Retry-After`).
pub struct GitHubReleases {
    agent: ureq::Agent,
    base: String,
    policy: RetryPolicy,
    budget: Duration,
}

impl std::fmt::Debug for GitHubReleases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubReleases")
            .field("base", &self.base)
            .field("policy", &self.policy)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

/// What one request is for.
#[derive(Debug, Clone, Copy)]
enum Want {
    /// The `Location` of a redirect, which is not followed; the body is not read.
    Location,
    /// The body of the final response, redirects followed (assets live on a CDN).
    Bytes,
}

/// The final answer to a request.
struct Answer {
    status: u16,
    location: Option<String>,
    /// The body of a 2xx [`Want::Bytes`] answer; empty otherwise.
    body: Vec<u8>,
}

impl GitHubReleases {
    /// The real github.com, HTTPS only (redirects to the asset CDN included).
    #[must_use]
    pub fn new(policy: RetryPolicy) -> Self {
        Self::build(WEB_BASE, true, policy)
    }

    /// `base` stands in for `https://github.com` (paths below it are unchanged) and plain HTTP
    /// is allowed, so tests can point this at a local mock server.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn with_base_url(base: &str, policy: RetryPolicy) -> Self {
        Self::build(base, false, policy)
    }

    fn build(base: &str, https_only: bool, policy: RetryPolicy) -> Self {
        let agent = ureq::Agent::config_builder()
            // Non-2xx must come back as responses so a 404 can become "not found".
            .http_status_as_error(false)
            .https_only(https_only)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(RESPONSE_TIMEOUT))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            agent,
            base: base.trim_end_matches('/').to_owned(),
            policy,
            budget: REQUEST_BUDGET,
        }
    }

    /// `GET url` with retries for 429, 5xx and network failures, all within the budget.
    fn get(&self, url: &str, want: Want) -> Result<Answer> {
        let deadline = Instant::now() + self.budget;
        let left = || deadline.saturating_duration_since(Instant::now());
        let mut attempt = 0;
        loop {
            debug!("GET {url} (attempt {})", attempt + 1);
            let delay = match self.attempt(url, want, left()) {
                Ok((answer, retry_after)) => {
                    let delay = self.policy.delay_after_status(
                        answer.status,
                        retry_after.as_deref(),
                        attempt,
                    );
                    match delay {
                        Some(delay) if delay < left() => {
                            info!("GET {url}: HTTP {}, retrying in {delay:?}", answer.status);
                            delay
                        }
                        _ => return Ok(answer),
                    }
                }
                Err(err) => match self.policy.delay_after_transport(&err, attempt) {
                    Some(delay) if delay < left() => {
                        info!("GET {url}: {err}, retrying in {delay:?}");
                        delay
                    }
                    _ => return Err(Error::Http(err)),
                },
            };
            self.policy.sleep(delay);
            attempt += 1;
        }
    }

    /// One request, ending after `budget` at the latest: the answer and its `Retry-After`.
    fn attempt(
        &self,
        url: &str,
        want: Want,
        budget: Duration,
    ) -> std::result::Result<(Answer, Option<String>), ureq::Error> {
        let request = self.agent.get(url).config().timeout_global(Some(budget));
        let mut response = match want {
            Want::Location => request.max_redirects(0).build().call()?,
            Want::Bytes => request
                .build()
                .header("Accept", "application/octet-stream")
                .call()?,
        };
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let (location, retry_after) = (header("Location"), header("Retry-After"));
        let body = match want {
            Want::Bytes if (200..300).contains(&status) => response
                .body_mut()
                .with_config()
                .limit(MAX_DOWNLOAD_SIZE)
                .read_to_vec()?,
            _ => Vec::new(),
        };
        let answer = Answer {
            status,
            location,
            body,
        };
        Ok((answer, retry_after))
    }
}

impl ReleaseSource for GitHubReleases {
    fn latest(&self) -> Result<Version> {
        let url = format!("{}/{REPOSITORY}/releases/latest", self.base);
        let answer = self.get(&url, Want::Location)?;
        match answer.status {
            300..=399 => {}
            200..=299 | 404 => return Err(no_releases()),
            status => return Err(refused(status, "the latest release")),
        }
        let location = answer.location.ok_or_else(|| {
            Error::SelfUpdate(format!("GitHub redirected {url} without a Location header"))
        })?;
        version_from_location(&self.base, &location)
    }

    fn download(&self, version: &Version, name: &str) -> Result<Vec<u8>> {
        let url = format!(
            "{}/{REPOSITORY}/releases/download/v{version}/{name}",
            self.base
        );
        let answer = self.get(&url, Want::Bytes)?;
        match answer.status {
            200..=299 => Ok(answer.body),
            404 => Err(Error::NotFound(format!(
                "{name} in release v{version} (see {WEB_BASE}/{REPOSITORY}/releases)"
            ))),
            status => Err(refused(status, name)),
        }
    }
}

/// The release a `/releases/latest` redirect points at. Only
/// `<base>/<repository>/releases/tag/v<version>` (or that path without the origin) is a
/// release; a redirect to the release list means none is published.
fn version_from_location(base: &str, location: &str) -> Result<Version> {
    let unexpected = || {
        let shown: String = location.chars().take(200).collect();
        Error::SelfUpdate(format!(
            "unexpected redirect for the latest release: {shown:?}"
        ))
    };
    let path = location.strip_prefix(base).unwrap_or(location);
    let rest = path
        .strip_prefix(&format!("/{REPOSITORY}/releases"))
        .ok_or_else(unexpected)?;
    if rest.is_empty() || rest == "/" {
        return Err(no_releases());
    }
    let tag = rest
        .strip_prefix("/tag/")
        .filter(|tag| tag.starts_with('v'))
        .ok_or_else(unexpected)?;
    tag.parse().map_err(|e| {
        let tag: String = tag.chars().take(64).collect();
        Error::SelfUpdate(format!(
            "the latest release tag {tag:?} is not a hw version: {e}"
        ))
    })
}

fn no_releases() -> Error {
    Error::NotFound(format!("no releases of {REPOSITORY} published yet"))
}

/// A non-2xx answer other than 404; the body (an HTML or XML page) adds nothing.
fn refused(status: u16, what: &str) -> Error {
    Error::SelfUpdate(format!("GitHub answered HTTP {status} for {what}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use httpmock::prelude::*;

    use super::*;
    use crate::client::NoSleep;

    const LATEST: &str = "/mazuninky/hookah-work-cli/releases/latest";
    const TAG: &str = "/mazuninky/hookah-work-cli/releases/tag/v2026.41.2";

    fn source(server: &MockServer, retries: u32) -> GitHubReleases {
        GitHubReleases::with_base_url(
            &server.base_url(),
            RetryPolicy::with_sleeper(retries, Arc::new(NoSleep)),
        )
    }

    #[test]
    fn latest_reads_the_redirect_without_following_it_or_authorization() {
        for absolute in [false, true] {
            let server = MockServer::start();
            let origin = if absolute {
                server.base_url()
            } else {
                String::new()
            };
            let location = format!("{origin}{TAG}");
            let latest = server.mock(|when, then| {
                when.method(GET)
                    .path(LATEST)
                    .header("User-Agent", USER_AGENT)
                    .header_missing("Authorization");
                then.status(302).header("Location", location.as_str());
            });
            let page = server.mock(|when, then| {
                when.path(TAG);
                then.status(200).body("<html>");
            });
            assert_eq!(
                source(&server, 0).latest().unwrap(),
                Version::new(2026, 41, 2),
                "{location}"
            );
            latest.assert();
            page.assert_calls(0);
        }
    }

    #[test]
    fn latest_without_a_release_redirect_is_not_found_or_refused() {
        for (status, location, check) in [
            (
                404,
                None,
                "not found: no releases of mazuninky/hookah-work-cli",
            ),
            (200, None, "not found: no releases"),
            (
                302,
                Some("/mazuninky/hookah-work-cli/releases"),
                "not found: no releases",
            ),
            (302, None, "without a Location header"),
            (503, None, "GitHub answered HTTP 503 for the latest release"),
            (429, None, "GitHub answered HTTP 429 for the latest release"),
        ] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(GET).path(LATEST);
                let then = then.status(status);
                if let Some(location) = location {
                    let _ = then.header("Location", location);
                }
            });
            let err = source(&server, 0).latest().unwrap_err();
            assert!(
                err.to_string().contains(check),
                "{status} {location:?}: {err}"
            );
        }
    }

    #[test]
    fn location_must_be_a_v_tag_of_this_repository() {
        let base = "https://github.com";
        for (location, version) in [
            (
                "https://github.com/mazuninky/hookah-work-cli/releases/tag/v2026.40.1",
                Version::new(2026, 40, 1),
            ),
            (
                "/mazuninky/hookah-work-cli/releases/tag/v2026.40.1",
                Version::new(2026, 40, 1),
            ),
            (
                "/mazuninky/hookah-work-cli/releases/tag/v2027.01.1",
                Version::new(2027, 1, 1),
            ),
        ] {
            assert_eq!(version_from_location(base, location).unwrap(), version);
        }

        for location in [
            "https://github.com/mazuninky/hookah-work-cli/releases",
            "/mazuninky/hookah-work-cli/releases/",
        ] {
            assert!(
                matches!(
                    version_from_location(base, location),
                    Err(Error::NotFound(_))
                ),
                "{location}"
            );
        }

        for (location, why) in [
            (
                "https://evil.example/mazuninky/hookah-work-cli/releases/tag/v2026.40.1",
                "unexpected redirect",
            ),
            (
                "https://github.com.evil.example/mazuninky/hookah-work-cli/releases/tag/v2026.40.1",
                "unexpected redirect",
            ),
            (
                "//evil.example/mazuninky/hookah-work-cli/releases/tag/v2026.40.1",
                "unexpected redirect",
            ),
            (
                "http://github.com/mazuninky/hookah-work-cli/releases/tag/v2026.40.1",
                "unexpected redirect",
            ),
            (
                "/mazuninky/atl/releases/tag/v2026.38.1",
                "unexpected redirect",
            ),
            (
                "/mazuninky/hookah-work-cli/releases/tag/2026.40.1",
                "unexpected redirect",
            ),
            (
                "/mazuninky/hookah-work-cli/releases/download/v2026.40.1",
                "unexpected redirect",
            ),
            (
                "/mazuninky/hookah-work-cli/releases/tag/vv2026.40.1",
                "is not a hw version",
            ),
            (
                "/mazuninky/hookah-work-cli/releases/tag/nightly",
                "unexpected redirect",
            ),
            (
                "/mazuninky/hookah-work-cli/releases/tag/v2026.40.1?x=1",
                "is not a hw version",
            ),
            (
                "/mazuninky/hookah-work-cli/releases/tag/v2026.40.1/extra",
                "is not a hw version",
            ),
        ] {
            let err = version_from_location(base, location).unwrap_err();
            assert!(
                matches!(&err, Error::SelfUpdate(m) if m.contains(why)),
                "{location}: {err}"
            );
        }
    }

    #[test]
    fn download_fetches_the_asset_path_and_retries_server_errors() {
        let server = MockServer::start();
        let path = "/mazuninky/hookah-work-cli/releases/download/v2026.41.2/hw.tar.gz";
        let failing = server.mock(|when, then| {
            when.method(GET).path(path);
            then.status(503);
        });
        let releases = source(&server, 2);
        assert!(matches!(
            releases.download(&Version::new(2026, 41, 2), "hw.tar.gz"),
            Err(Error::SelfUpdate(m)) if m == "GitHub answered HTTP 503 for hw.tar.gz"
        ));
        failing.assert_calls(3);
    }

    #[test]
    fn a_stalled_download_ends_with_the_budget_instead_of_being_retried() {
        let server = MockServer::start();
        let stalled = server.mock(|when, then| {
            when.method(GET)
                .path("/mazuninky/hookah-work-cli/releases/download/v2026.41.2/hw.tar.gz");
            then.status(200).body("late").delay(Duration::from_secs(2));
        });
        let releases = GitHubReleases {
            budget: Duration::from_millis(300),
            ..source(&server, 3)
        };
        let started = Instant::now();
        let result = releases.download(&Version::new(2026, 41, 2), "hw.tar.gz");
        assert!(
            matches!(result, Err(Error::Http(ureq::Error::Timeout(_)))),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_millis(1500));
        stalled.assert_calls(1);
    }

    #[test]
    fn download_follows_redirects_returns_bytes_and_maps_404_to_not_found() {
        let server = MockServer::start();
        let asset = server.mock(|when, then| {
            when.method(GET)
                .path("/mazuninky/hookah-work-cli/releases/download/v2026.41.2/a.sha256")
                .header_missing("Authorization");
            then.status(302).header("Location", "/cdn/a.sha256");
        });
        let cdn = server.mock(|when, then| {
            when.method(GET).path("/cdn/a.sha256");
            then.status(200).body("digest  a");
        });
        let releases = source(&server, 0);
        let version = Version::new(2026, 41, 2);
        assert_eq!(
            releases.download(&version, "a.sha256").unwrap(),
            b"digest  a"
        );
        asset.assert();
        cdn.assert();
        assert!(matches!(
            releases.download(&version, "missing.zip"),
            Err(Error::NotFound(m)) if m.starts_with("missing.zip in release v2026.41.2")
        ));
    }

    #[test]
    fn production_source_refuses_plain_http() {
        let server = MockServer::start();
        let releases = GitHubReleases {
            base: server.base_url(),
            ..GitHubReleases::new(RetryPolicy::with_sleeper(0, Arc::new(NoSleep)))
        };
        assert!(matches!(releases.latest(), Err(Error::Http(_))));
    }

    #[test]
    fn release_page_points_at_the_tag() {
        assert_eq!(
            release_page_url(&Version::new(2026, 41, 2)),
            "https://github.com/mazuninky/hookah-work-cli/releases/tag/v2026.41.2"
        );
    }
}
