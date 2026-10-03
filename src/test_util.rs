//! Hand-written test doubles for unit tests and, through the `test-util` feature,
//! integration tests: a scripted [`FakeApi`], [`FakeConnector`], [`FakeIssuer`] and
//! [`RecordingSleeper`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::Value;

use crate::auth::Secret;
use crate::client::{
    Api, ApiResponse, Connector, Login, Pagination, ResponseBody, Sleeper, TokenIssuer,
    error_for_status,
};
use crate::error::{Error, Result};

/// Which [`Api`] method a call went through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// [`Api::get`].
    Get,
    /// [`Api::find_client`].
    FindClient,
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The method used.
    pub kind: CallKind,
    /// Request path (`/api/client/find` for lookups).
    pub path: String,
    /// Query pairs in order.
    pub query: Vec<(String, String)>,
    /// Form pairs (lookups only).
    pub form: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
enum Reply {
    Ok(ApiResponse),
    Status(u16, String),
}

/// Scripted [`Api`]: replies are queued per path; the last reply of a path repeats.
/// A path without replies answers 404.
#[derive(Debug, Default)]
pub struct FakeApi {
    replies: Mutex<HashMap<String, VecDeque<Reply>>>,
    calls: Mutex<Vec<Call>>,
}

impl FakeApi {
    /// An API with no scripted replies.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn with_reply(self, path: &str, reply: Reply) -> Self {
        lock(&self.replies)
            .entry(path.to_owned())
            .or_default()
            .push_back(reply);
        self
    }

    /// Queues a JSON reply for `path`.
    #[must_use]
    pub fn with_json(self, path: &str, value: Value) -> Self {
        self.with_reply(path, Reply::Ok(ApiResponse::json(value)))
    }

    /// Queues a JSON reply with pagination headers for `path`.
    #[must_use]
    pub fn with_page(self, path: &str, value: Value, pagination: Pagination) -> Self {
        self.with_reply(
            path,
            Reply::Ok(ApiResponse {
                body: ResponseBody::Json(value),
                pagination: Some(pagination),
            }),
        )
    }

    /// Queues a non-JSON reply for `path`.
    #[must_use]
    pub fn with_text(self, path: &str, text: &str) -> Self {
        self.with_reply(
            path,
            Reply::Ok(ApiResponse {
                body: ResponseBody::Text(text.to_owned()),
                pagination: None,
            }),
        )
    }

    /// Queues an error status for `path`, mapped exactly like a real response with this body.
    #[must_use]
    pub fn with_status(self, path: &str, status: u16, body: &str) -> Self {
        self.with_reply(path, Reply::Status(status, body.to_owned()))
    }

    /// Every call so far.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    /// `(path, query)` of every call so far — the usual assertion target.
    #[must_use]
    pub fn requests(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.calls()
            .into_iter()
            .map(|call| (call.path, call.query))
            .collect()
    }

    fn answer(&self, call: Call) -> Result<ApiResponse> {
        let path = call.path.clone();
        lock(&self.calls).push(call);
        let reply = {
            let mut replies = lock(&self.replies);
            match replies.get_mut(&path) {
                Some(queue) if queue.len() > 1 => queue.pop_front(),
                Some(queue) => queue.front().cloned(),
                None => None,
            }
        };
        match reply {
            Some(Reply::Ok(response)) => Ok(response),
            Some(Reply::Status(status, body)) => Err(error_for_status(status, &body, &[])),
            None => Err(Error::NotFound(format!("FakeApi has no reply for {path}"))),
        }
    }
}

impl Api for FakeApi {
    fn get(&self, path: &str, query: &[(String, String)]) -> Result<ApiResponse> {
        self.answer(Call {
            kind: CallKind::Get,
            path: path.to_owned(),
            query: query.to_vec(),
            form: Vec::new(),
        })
    }

    fn find_client(
        &self,
        form: &[(String, String)],
        query: &[(String, String)],
    ) -> Result<ApiResponse> {
        self.answer(Call {
            kind: CallKind::FindClient,
            path: "/api/client/find".to_owned(),
            query: query.to_vec(),
            form: form.to_vec(),
        })
    }
}

/// Lets several boxed clients share one [`FakeApi`].
struct SharedApi(Arc<FakeApi>);

impl Api for SharedApi {
    fn get(&self, path: &str, query: &[(String, String)]) -> Result<ApiResponse> {
        self.0.get(path, query)
    }

    fn find_client(
        &self,
        form: &[(String, String)],
        query: &[(String, String)],
    ) -> Result<ApiResponse> {
        self.0.find_client(form, query)
    }
}

/// [`Connector`] handing out one shared [`FakeApi`] and recording `(base_url, token)`.
#[derive(Debug, Default)]
pub struct FakeConnector {
    api: Arc<FakeApi>,
    connections: Mutex<Vec<(String, Option<Secret>)>>,
}

impl FakeConnector {
    /// A connector serving `api`.
    #[must_use]
    pub fn new(api: FakeApi) -> Self {
        Self {
            api: Arc::new(api),
            connections: Mutex::default(),
        }
    }

    /// The shared fake, for call assertions.
    #[must_use]
    pub fn api(&self) -> &FakeApi {
        &self.api
    }

    /// Every `(base_url, token)` a client was built for.
    #[must_use]
    pub fn connections(&self) -> Vec<(String, Option<Secret>)> {
        lock(&self.connections).clone()
    }
}

impl Connector for FakeConnector {
    fn connect(&self, base_url: &str, token: Option<&Secret>) -> Box<dyn Api> {
        lock(&self.connections).push((base_url.to_owned(), token.cloned()));
        Box::new(SharedApi(Arc::clone(&self.api)))
    }
}

/// [`TokenIssuer`] returning a fixed login or status error and recording
/// `(base_url, email, password)`.
#[derive(Debug)]
pub struct FakeIssuer {
    reply: std::result::Result<Login, (u16, String)>,
    calls: Mutex<Vec<(String, String, Secret)>>,
}

impl FakeIssuer {
    /// Succeeds with `login`.
    #[must_use]
    pub fn ok(login: Login) -> Self {
        Self {
            reply: Ok(login),
            calls: Mutex::default(),
        }
    }

    /// Fails like a response with `status` and `body`.
    #[must_use]
    pub fn status(status: u16, body: &str) -> Self {
        Self {
            reply: Err((status, body.to_owned())),
            calls: Mutex::default(),
        }
    }

    /// Every login attempt so far.
    #[must_use]
    pub fn calls(&self) -> Vec<(String, String, Secret)> {
        lock(&self.calls).clone()
    }
}

impl TokenIssuer for FakeIssuer {
    fn login(&self, base_url: &str, email: &str, password: &Secret) -> Result<Login> {
        lock(&self.calls).push((base_url.to_owned(), email.to_owned(), password.clone()));
        match &self.reply {
            Ok(login) => Ok(login.clone()),
            Err((status, body)) => Err(error_for_status(*status, body, &[])),
        }
    }
}

/// [`Sleeper`] that records requested delays instead of sleeping.
#[derive(Debug, Default)]
pub struct RecordingSleeper {
    slept: Mutex<Vec<Duration>>,
}

impl RecordingSleeper {
    /// A sleeper with no recorded delays.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every delay requested so far.
    #[must_use]
    pub fn slept(&self) -> Vec<Duration> {
        lock(&self.slept).clone()
    }
}

impl Sleeper for RecordingSleeper {
    fn sleep(&self, duration: Duration) {
        lock(&self.slept).push(duration);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fake_api_replays_queue_and_repeats_last() {
        let api = FakeApi::new()
            .with_json("/api/a", json!(1))
            .with_json("/api/a", json!(2));
        let body = |api: &FakeApi| api.get("/api/a", &[]).unwrap().into_json().unwrap();
        assert_eq!(body(&api), json!(1));
        assert_eq!(body(&api), json!(2));
        assert_eq!(body(&api), json!(2));
        assert!(matches!(api.get("/api/b", &[]), Err(Error::NotFound(_))));
        assert_eq!(api.requests().len(), 4);
    }

    #[test]
    fn fake_api_maps_status_like_the_client() {
        let api = FakeApi::new().with_status("/api/x", 403, r#"{"message":"Нет доступа"}"#);
        assert!(matches!(api.get("/api/x", &[]), Err(Error::Auth(m)) if m == "Нет доступа"));
    }

    #[test]
    fn recording_sleeper_records() {
        let sleeper = RecordingSleeper::new();
        sleeper.sleep(Duration::from_millis(5));
        assert_eq!(sleeper.slept(), vec![Duration::from_millis(5)]);
    }
}
