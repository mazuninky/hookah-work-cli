//! Read-only access to the HookahWork API.
//!
//! [`Api`] is the seam command handlers depend on. [`HookahClient`] is the real ureq-based
//! implementation; besides `GET` it can send exactly two non-mutating `POST`s —
//! `/api/login` ([`HookahClient::login`]) and `/api/client/find` ([`Api::find_client`]) —
//! and deliberately has no generic request method.

mod pagination;
mod retry;
mod status;

pub use pagination::{MAX_PER_PAGE, Pagination, fetch_all};
pub use retry::{
    BACKOFF_BASE, BACKOFF_CAP, NoSleep, RATE_LIMIT_CAP, RATE_LIMIT_DEFAULT, RetryPolicy, Sleeper,
    ThreadSleeper, backoff_delay, is_retryable_status, is_retryable_transport, rate_limit_delay,
};
pub use status::{MESSAGE_LIMIT, error_for_status, error_message};

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::auth::Secret;
use crate::error::{Error, Result};

/// `User-Agent` sent with every request.
pub const USER_AGENT: &str = concat!("hw/", env!("CARGO_PKG_VERSION"));

/// End-to-end timeout of one attempt (DNS through the last body byte).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A rate-limit (429) wait at least this long is logged as a warning: the user sees a stall.
pub const RATE_LIMIT_WARN_AFTER: Duration = Duration::from_secs(1);

const ACCEPT_JSON: &str = "application/json";

/// The body of a successful response.
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseBody {
    /// Parsed JSON; an empty body is `Value::Null`.
    Json(Value),
    /// A body that is not JSON, kept verbatim.
    Text(String),
}

/// A successful (2xx) response.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiResponse {
    /// Response body.
    pub body: ResponseBody,
    /// Pagination headers, when the endpoint is paginated.
    pub pagination: Option<Pagination>,
}

impl ApiResponse {
    /// A JSON response without pagination.
    #[must_use]
    pub fn json(value: Value) -> Self {
        Self {
            body: ResponseBody::Json(value),
            pagination: None,
        }
    }

    /// The JSON body; a non-JSON body is [`Error::InvalidResponse`].
    pub fn into_json(self) -> Result<Value> {
        match self.body {
            ResponseBody::Json(value) => Ok(value),
            ResponseBody::Text(text) => Err(Error::InvalidResponse(format!(
                "expected JSON, got: {}",
                status::excerpt(&text)
            ))),
        }
    }
}

/// Query-string builder that keeps parameter order (so requests are deterministic).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query(Vec<(String, String)>);

impl Query {
    /// An empty query.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `key=value`.
    pub fn push(&mut self, key: &str, value: impl ToString) -> &mut Self {
        self.0.push((key.to_owned(), value.to_string()));
        self
    }

    /// Appends `key=value` when `value` is present.
    pub fn push_opt<V: ToString>(&mut self, key: &str, value: Option<V>) -> &mut Self {
        if let Some(value) = value {
            self.push(key, value);
        }
        self
    }

    /// The pairs in insertion order.
    #[must_use]
    pub fn pairs(&self) -> &[(String, String)] {
        &self.0
    }

    /// Consumes the builder into its pairs.
    #[must_use]
    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.0
    }
}

/// The read-only API surface command handlers depend on.
///
/// `path` is relative to the CRM base URL and starts with `/api/`; query pairs are sent
/// percent-encoded in the given order.
pub trait Api {
    /// `GET <path>?<query>`.
    fn get(&self, path: &str, query: &[(String, String)]) -> Result<ApiResponse>;

    /// `POST /api/client/find?<query>` with `form` as an `application/x-www-form-urlencoded`
    /// body — a lookup, not a write.
    fn find_client(
        &self,
        form: &[(String, String)],
        query: &[(String, String)],
    ) -> Result<ApiResponse>;
}

/// Builds [`Api`] clients for a CRM; the seam between the composition root and HTTP.
pub trait Connector {
    /// A client for `base_url` that sends `token` as a Bearer header when present.
    fn connect(&self, base_url: &str, token: Option<&Secret>) -> Box<dyn Api>;
}

/// Exchanges CRM user credentials for an API token (`POST /api/login`).
pub trait TokenIssuer {
    /// Logs in with Basic auth and returns the issued token.
    fn login(&self, base_url: &str, email: &str, password: &Secret) -> Result<Login>;
}

/// The result of `POST /api/login`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    /// Email of the CRM user.
    pub email: String,
    /// Display name of the CRM user.
    pub name: String,
    /// The issued API token (`auth_token`).
    pub token: Secret,
}

impl Login {
    /// Parses the login response body.
    pub fn from_json(value: &Value) -> Result<Self> {
        let field = |name: &str| value.get(name).and_then(Value::as_str);
        let token = field("auth_token")
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| Error::InvalidResponse("login response has no auth_token".into()))?;
        Ok(Self {
            email: field("email").unwrap_or_default().to_owned(),
            name: field("name").unwrap_or_default().to_owned(),
            token: Secret::new(token),
        })
    }
}

/// Status, the headers `hw` cares about and the body text of one attempt.
struct RawResponse {
    status: u16,
    rate_limit_reset: Option<String>,
    pagination: Option<Pagination>,
    body: String,
}

/// The real HookahWork client (sync ureq, rustls, 30 s timeout, retries per [`RetryPolicy`]).
pub struct HookahClient {
    agent: ureq::Agent,
    base_url: String,
    token: Option<Secret>,
    policy: RetryPolicy,
}

impl fmt::Debug for HookahClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookahClient")
            .field("base_url", &self.base_url)
            .field("token", &self.token)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl HookahClient {
    /// A client for `base_url` (no trailing slash), optionally authenticated with `token`.
    #[must_use]
    pub fn new(base_url: impl Into<String>, token: Option<Secret>, policy: RetryPolicy) -> Self {
        let agent = ureq::Agent::config_builder()
            // Non-2xx must come back as responses so the Yii error body can be read.
            .http_status_as_error(false)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            agent,
            base_url: base_url.into(),
            token,
            policy,
        }
    }

    /// The CRM base URL this client talks to.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `POST /api/login` with Basic auth (`email:password`). The password and its Basic
    /// encoding are masked in error messages like the token.
    pub fn login(&self, email: &str, password: &Secret) -> Result<Login> {
        let url = self.url("/api/login");
        let credentials = Secret::new(
            base64::engine::general_purpose::STANDARD
                .encode(format!("{email}:{}", password.expose())),
        );
        let authorization = format!("Basic {}", credentials.expose());
        let response = self.execute("POST", "/api/login", &[password, &credentials], || {
            self.agent
                .post(&url)
                .header("Accept", ACCEPT_JSON)
                .header("Authorization", authorization.as_str())
                .send_empty()
        })?;
        match response.body {
            ResponseBody::Json(value) => Login::from_json(&value),
            ResponseBody::Text(_) => Err(Error::InvalidResponse(
                "the login response is not JSON".into(),
            )),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn bearer(&self) -> Option<String> {
        self.token
            .as_ref()
            .map(|token| format!("Bearer {}", token.expose()))
    }

    /// Runs one logical request, retrying per the policy; `send` performs a single attempt.
    /// `secrets` are masked in error bodies on top of the token.
    fn execute<F>(
        &self,
        method: &str,
        path: &str,
        secrets: &[&Secret],
        send: F,
    ) -> Result<ApiResponse>
    where
        F: Fn() -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    {
        let mut attempt = 0;
        loop {
            debug!("{method} {path} (attempt {})", attempt + 1);
            match send().and_then(read_raw) {
                Ok(raw) => {
                    if let Some(delay) = self.policy.delay_after_status(
                        raw.status,
                        raw.rate_limit_reset.as_deref(),
                        attempt,
                    ) {
                        if is_long_rate_limit_wait(raw.status, delay) {
                            warn!(
                                "{method} {path}: rate limited (HTTP 429), retrying in {delay:?}"
                            );
                        } else {
                            info!(
                                "{method} {path}: HTTP {}, retrying in {delay:?}",
                                raw.status
                            );
                        }
                        self.policy.sleep(delay);
                        attempt += 1;
                        continue;
                    }
                    return self.finish(raw, secrets);
                }
                Err(err) => {
                    if let Some(delay) = self.policy.delay_after_transport(&err, attempt) {
                        info!("{method} {path}: {err}, retrying in {delay:?}");
                        self.policy.sleep(delay);
                        attempt += 1;
                        continue;
                    }
                    return Err(Error::Http(err));
                }
            }
        }
    }

    fn finish(&self, raw: RawResponse, secrets: &[&Secret]) -> Result<ApiResponse> {
        if !(200..300).contains(&raw.status) {
            let secrets: Vec<&Secret> = self.token.iter().chain(secrets.iter().copied()).collect();
            return Err(error_for_status(raw.status, &raw.body, &secrets));
        }
        let body = if raw.body.trim().is_empty() {
            ResponseBody::Json(Value::Null)
        } else {
            match serde_json::from_str(&raw.body) {
                Ok(value) => ResponseBody::Json(value),
                Err(_) => ResponseBody::Text(raw.body),
            }
        };
        Ok(ApiResponse {
            body,
            pagination: raw.pagination,
        })
    }
}

/// A 429 wait long enough to show up as a stall deserves a warning rather than an info line.
fn is_long_rate_limit_wait(status: u16, delay: Duration) -> bool {
    status == 429 && delay >= RATE_LIMIT_WARN_AFTER
}

fn read_raw(
    mut response: ureq::http::Response<ureq::Body>,
) -> std::result::Result<RawResponse, ureq::Error> {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let rate_limit_reset = header("X-Rate-Limit-Reset");
    let pagination = Pagination::from_headers(header);
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string()?;
    Ok(RawResponse {
        status,
        rate_limit_reset,
        pagination,
        body,
    })
}

impl Api for HookahClient {
    fn get(&self, path: &str, query: &[(String, String)]) -> Result<ApiResponse> {
        let url = self.url(path);
        let bearer = self.bearer();
        self.execute("GET", path, &[], || {
            let mut request = self.agent.get(&url).header("Accept", ACCEPT_JSON);
            for (key, value) in query {
                request = request.query(key, value);
            }
            if let Some(bearer) = &bearer {
                request = request.header("Authorization", bearer.as_str());
            }
            request.call()
        })
    }

    fn find_client(
        &self,
        form: &[(String, String)],
        query: &[(String, String)],
    ) -> Result<ApiResponse> {
        let path = "/api/client/find";
        let url = self.url(path);
        let bearer = self.bearer();
        self.execute("POST", path, &[], || {
            let mut request = self.agent.post(&url).header("Accept", ACCEPT_JSON);
            for (key, value) in query {
                request = request.query(key, value);
            }
            if let Some(bearer) = &bearer {
                request = request.header("Authorization", bearer.as_str());
            }
            request.send_form(form.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        })
    }
}

/// Production [`Connector`] and [`TokenIssuer`]: builds [`HookahClient`]s with one retry policy.
#[derive(Debug, Clone)]
pub struct HttpConnector {
    policy: RetryPolicy,
}

impl HttpConnector {
    /// A connector whose clients retry per `policy`.
    #[must_use]
    pub fn new(policy: RetryPolicy) -> Self {
        Self { policy }
    }
}

impl Connector for HttpConnector {
    fn connect(&self, base_url: &str, token: Option<&Secret>) -> Box<dyn Api> {
        Box::new(HookahClient::new(
            base_url,
            token.cloned(),
            self.policy.clone(),
        ))
    }
}

impl TokenIssuer for HttpConnector {
    fn login(&self, base_url: &str, email: &str, password: &Secret) -> Result<Login> {
        HookahClient::new(base_url, None, self.policy.clone()).login(email, password)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_keeps_order_and_skips_none() {
        let mut query = Query::new();
        query
            .push("date", "2026-09-30")
            .push_opt("storage_id", Some(2))
            .push_opt::<u32>("page", None)
            .push("expand", "product,storage");
        assert_eq!(
            query.into_pairs(),
            vec![
                ("date".to_owned(), "2026-09-30".to_owned()),
                ("storage_id".to_owned(), "2".to_owned()),
                ("expand".to_owned(), "product,storage".to_owned()),
            ]
        );
    }

    #[test]
    fn login_parses_documented_response() {
        let login = Login::from_json(&json!({
            "email": "integration@example.com",
            "name": "Интеграция",
            "auth_token": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        }))
        .unwrap();
        assert_eq!(login.email, "integration@example.com");
        assert_eq!(login.token.expose(), "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        assert!(!format!("{login:?}").contains("xxxx"));
    }

    #[test]
    fn login_without_token_is_invalid_response() {
        let err = Login::from_json(&json!({"email": "a"})).unwrap_err();
        assert!(matches!(err, Error::InvalidResponse(_)));
    }

    #[test]
    fn into_json_rejects_text() {
        let response = ApiResponse {
            body: ResponseBody::Text("<html>".into()),
            pagination: None,
        };
        assert!(matches!(
            response.into_json(),
            Err(Error::InvalidResponse(_))
        ));
        assert_eq!(
            ApiResponse::json(json!([1])).into_json().unwrap(),
            json!([1])
        );
    }

    #[test]
    fn client_debug_hides_token() {
        let client = HookahClient::new(
            "https://demo.hookah.work",
            Some(Secret::new("topsecret")),
            RetryPolicy::new(0),
        );
        let debug = format!("{client:?}");
        assert!(!debug.contains("topsecret"), "{debug}");
        assert_eq!(client.base_url(), "https://demo.hookah.work");
    }

    #[test]
    fn error_bodies_mask_the_token_and_extra_secrets() {
        let client = HookahClient::new(
            "https://demo.hookah.work",
            Some(Secret::new("topsecret")),
            RetryPolicy::new(0),
        );
        let password = Secret::new("hunter2");
        let raw = RawResponse {
            status: 401,
            rate_limit_reset: None,
            pagination: None,
            body: r#"{"message":"topsecret hunter2"}"#.into(),
        };
        let err = client.finish(raw, &[&password]).unwrap_err();
        assert!(matches!(&err, Error::Auth(m) if m == "*** ***"), "{err:?}");
    }

    #[test]
    fn only_long_rate_limit_waits_warn() {
        assert!(is_long_rate_limit_wait(429, Duration::from_secs(1)));
        assert!(!is_long_rate_limit_wait(429, Duration::from_millis(999)));
        assert!(!is_long_rate_limit_wait(503, Duration::from_secs(5)));
    }
}
