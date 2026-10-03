//! Yii pagination: `X-Pagination-*` response headers, the "more pages" hint and `--all`.

use serde_json::Value;
use tracing::debug;

use super::Api;
use crate::error::{Error, Result};

/// Largest page size the API accepts; `--all` always uses it.
pub const MAX_PER_PAGE: u32 = 100;

/// Safety stop for `--all` against a server whose page count never converges.
const MAX_PAGES: u64 = 10_000;

/// Pagination metadata of one response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    /// `X-Pagination-Total-Count`.
    pub total_count: u64,
    /// `X-Pagination-Page-Count`.
    pub page_count: u64,
    /// `X-Pagination-Current-Page` (1-based).
    pub current_page: u64,
    /// `X-Pagination-Per-Page`.
    pub per_page: u64,
}

impl Pagination {
    /// Header carrying the total number of items.
    pub const TOTAL_COUNT: &'static str = "X-Pagination-Total-Count";
    /// Header carrying the number of pages.
    pub const PAGE_COUNT: &'static str = "X-Pagination-Page-Count";
    /// Header carrying the 1-based current page.
    pub const CURRENT_PAGE: &'static str = "X-Pagination-Current-Page";
    /// Header carrying the page size.
    pub const PER_PAGE: &'static str = "X-Pagination-Per-Page";

    /// Reads the headers through `header`; `None` unless page count and current page are present.
    #[must_use]
    pub fn from_headers(header: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let number = |name: &str| header(name).and_then(|v| v.trim().parse::<u64>().ok());
        Some(Self {
            page_count: number(Self::PAGE_COUNT)?,
            current_page: number(Self::CURRENT_PAGE)?,
            total_count: number(Self::TOTAL_COUNT).unwrap_or_default(),
            per_page: number(Self::PER_PAGE).unwrap_or_default(),
        })
    }

    /// Whether pages after the current one exist.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.current_page < self.page_count
    }

    /// The stderr hint shown when more pages exist (without the `hint: ` prefix).
    #[must_use]
    pub fn hint(&self) -> String {
        format!(
            "page {} of {} ({} total); use --all or --page <N>",
            self.current_page, self.page_count, self.total_count
        )
    }
}

/// Fetches every page of a list endpoint and concatenates the arrays. Any `page`/`per-page`
/// in `query` is replaced; pages are requested until the current page reaches the page count.
pub fn fetch_all(
    api: &dyn Api,
    path: &str,
    query: &[(String, String)],
    per_page: u32,
) -> Result<Value> {
    let base: Vec<(String, String)> = query
        .iter()
        .filter(|(key, _)| key != "page" && key != "per-page")
        .cloned()
        .collect();
    let mut items = Vec::new();
    let mut page: u64 = 1;
    loop {
        let mut page_query = base.clone();
        page_query.push(("page".into(), page.to_string()));
        page_query.push(("per-page".into(), per_page.to_string()));
        let response = api.get(path, &page_query)?;
        let pagination = response.pagination;
        let Value::Array(batch) = response.into_json()? else {
            return Err(Error::InvalidResponse(format!(
                "{path} did not return a list, so --all cannot page through it"
            )));
        };
        let received = batch.len();
        items.extend(batch);
        debug!("{path}: page {page} returned {received} item(s)");

        match pagination {
            Some(p) if p.has_more() && received > 0 => page = p.current_page + 1,
            _ => break,
        }
        if page > MAX_PAGES {
            return Err(Error::InvalidResponse(format!(
                "{path} kept reporting more pages after {MAX_PAGES} pages"
            )));
        }
    }
    Ok(Value::Array(items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::FakeApi;
    use serde_json::json;

    fn pages(current: u64, count: u64) -> Pagination {
        Pagination {
            total_count: 3,
            page_count: count,
            current_page: current,
            per_page: 100,
        }
    }

    #[test]
    fn parses_headers() {
        let headers = |name: &str| match name {
            "X-Pagination-Total-Count" => Some("432".to_owned()),
            "X-Pagination-Page-Count" => Some("5".to_owned()),
            "X-Pagination-Current-Page" => Some("1".to_owned()),
            "X-Pagination-Per-Page" => Some("100".to_owned()),
            _ => None,
        };
        let p = Pagination::from_headers(headers).unwrap();
        assert_eq!(p.hint(), "page 1 of 5 (432 total); use --all or --page <N>");
        assert!(p.has_more());
        assert!(Pagination::from_headers(|_| None).is_none());
    }

    #[test]
    fn fetch_all_walks_pages_and_concatenates() {
        let api = FakeApi::new()
            .with_page("/api/clients", json!([{"id": 1}, {"id": 2}]), pages(1, 2))
            .with_page("/api/clients", json!([{"id": 3}]), pages(2, 2));
        let query = vec![
            ("search".to_owned(), "a".to_owned()),
            ("page".to_owned(), "7".to_owned()),
        ];
        let all = fetch_all(&api, "/api/clients", &query, 100).unwrap();
        assert_eq!(all, json!([{"id": 1}, {"id": 2}, {"id": 3}]));
        let pages_requested: Vec<Vec<(String, String)>> =
            api.requests().into_iter().map(|(_, q)| q).collect();
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        assert_eq!(
            pages_requested,
            vec![
                vec![
                    pair("search", "a"),
                    pair("page", "1"),
                    pair("per-page", "100")
                ],
                vec![
                    pair("search", "a"),
                    pair("page", "2"),
                    pair("per-page", "100")
                ],
            ]
        );
    }

    #[test]
    fn fetch_all_rejects_non_lists() {
        let api = FakeApi::new().with_json("/api/settings", json!({"a": 1}));
        let err = fetch_all(&api, "/api/settings", &[], 100).unwrap_err();
        assert!(matches!(err, Error::InvalidResponse(_)));
    }
}
