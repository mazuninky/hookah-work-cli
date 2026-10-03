//! Command handlers. Each receives its collaborators (`&dyn Api`, stores, the clock) and
//! returns an [`Outcome`]; the composition root (`crate::app`) renders it exactly once.

pub mod analytics;
pub mod api;
pub mod auth;
pub mod booking;
pub mod client;
pub mod config;
pub mod docs;
pub mod hookah;
pub mod menu;
pub mod product;
pub mod reference;
pub mod report;
pub mod sale;
pub mod self_update;
pub mod storage;

use serde_json::Value;

use crate::auth::Secret;
use crate::cli::args::PageArgs;
use crate::client::{Api, MAX_PER_PAGE, Pagination, Query, fetch_all};
use crate::error::Error;

/// What a command produced.
#[derive(Debug)]
pub struct Outcome {
    /// What goes to stdout.
    pub payload: Payload,
    /// One-line hints for stderr (suppressed by `-q`), without the `hint: ` prefix.
    pub hints: Vec<String>,
    /// An error to report after the payload was written (e.g. `hw auth status` with a bad token).
    pub failure: Option<Error>,
}

/// The stdout part of an [`Outcome`].
#[derive(Debug)]
pub enum Payload {
    /// A value rendered through `--jq`/`--template`/`--format`.
    Json(Value),
    /// Text written verbatim.
    Text(String),
    /// Text from the server (`hw api`): exact bytes to a pipe or file, control characters
    /// escaped on a terminal.
    ServerText(String),
    /// A credential written verbatim (`hw auth token`); redacted in `Debug`.
    Secret(Secret),
}

impl Outcome {
    /// A JSON payload.
    #[must_use]
    pub fn json(value: Value) -> Self {
        Self::from_payload(Payload::Json(value))
    }

    /// A verbatim text payload.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::from_payload(Payload::Text(text.into()))
    }

    /// A server-text payload.
    #[must_use]
    pub fn server_text(text: impl Into<String>) -> Self {
        Self::from_payload(Payload::ServerText(text.into()))
    }

    /// A credential payload.
    #[must_use]
    pub fn secret(secret: Secret) -> Self {
        Self::from_payload(Payload::Secret(secret))
    }

    fn from_payload(payload: Payload) -> Self {
        Self {
            payload,
            hints: Vec::new(),
            failure: None,
        }
    }

    /// Adds a stderr hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    /// Fails the command after the payload has been written.
    #[must_use]
    pub fn then_fail(mut self, error: Error) -> Self {
        self.failure = Some(error);
        self
    }

    /// The JSON payload, if that is what this outcome carries.
    #[must_use]
    pub fn json_value(&self) -> Option<&Value> {
        match &self.payload {
            Payload::Json(value) => Some(value),
            _ => None,
        }
    }
}

/// A single GET whose JSON body is the result.
pub(crate) fn fetch(api: &dyn Api, path: &str, query: &Query) -> anyhow::Result<Outcome> {
    Ok(Outcome::json(api.get(path, query.pairs())?.into_json()?))
}

/// A GET of a paginated list: one page (with a hint when more exist) or, with `--all`,
/// every page concatenated.
pub(crate) fn fetch_paged(
    api: &dyn Api,
    path: &str,
    mut query: Query,
    page: &PageArgs,
) -> anyhow::Result<Outcome> {
    if page.all {
        let per_page = page.per_page.unwrap_or(MAX_PER_PAGE);
        return Ok(Outcome::json(fetch_all(
            api,
            path,
            query.pairs(),
            per_page,
        )?));
    }
    query
        .push_opt("page", page.page)
        .push_opt("per-page", page.per_page);
    let response = api.get(path, query.pairs())?;
    let hint = response
        .pagination
        .filter(Pagination::has_more)
        .map(|p| p.hint());
    let outcome = Outcome::json(response.into_json()?);
    Ok(match hint {
        Some(hint) => outcome.with_hint(hint),
        None => outcome,
    })
}

/// Shorthand for building expected query pairs in handler tests.
#[cfg(test)]
pub(crate) fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::FakeApi;
    use serde_json::json;

    fn page_info(current: u64, count: u64) -> Pagination {
        Pagination {
            total_count: 432,
            page_count: count,
            current_page: current,
            per_page: 100,
        }
    }

    #[test]
    fn single_page_with_more_pages_adds_hint() {
        let api = FakeApi::new().with_page("/api/clients", json!([{"id": 1}]), page_info(1, 5));
        let outcome =
            fetch_paged(&api, "/api/clients", Query::new(), &PageArgs::default()).unwrap();
        assert_eq!(
            outcome.hints,
            vec!["page 1 of 5 (432 total); use --all or --page <N>".to_owned()]
        );
        assert_eq!(api.requests(), vec![("/api/clients".to_owned(), vec![])]);
    }

    #[test]
    fn last_page_has_no_hint_and_sends_page_params() {
        let api = FakeApi::new().with_page("/api/clients", json!([]), page_info(5, 5));
        let page = PageArgs {
            page: Some(5),
            per_page: Some(10),
            all: false,
        };
        let outcome = fetch_paged(&api, "/api/clients", Query::new(), &page).unwrap();
        assert!(outcome.hints.is_empty());
        assert_eq!(
            api.requests()[0].1,
            pairs(&[("page", "5"), ("per-page", "10")])
        );
    }

    #[test]
    fn outcome_builders() {
        let outcome = Outcome::secret(Secret::new("tok")).then_fail(Error::Auth("x".into()));
        assert!(format!("{outcome:?}").contains("Secret(***)"));
        assert!(outcome.json_value().is_none());
        assert!(matches!(outcome.failure, Some(Error::Auth(_))));
        assert!(matches!(Outcome::text("t").payload, Payload::Text(t) if t == "t"));
        assert!(
            matches!(Outcome::server_text("\u{1b}").payload, Payload::ServerText(t) if t == "\u{1b}")
        );
    }
}
