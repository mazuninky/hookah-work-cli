//! `hw api` — raw GET passthrough. There is deliberately no method flag.

use super::Outcome;
use crate::cli::args::ApiArgs;
use crate::client::{Api, MAX_PER_PAGE, Pagination, ResponseBody, fetch_all};
use crate::error::{Error, Result};

/// A validated `hw api` request; built before any config or network work so a bad PATH
/// fails as invalid input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    /// Normalised path starting with `/api`.
    pub path: String,
    /// Inline query pairs followed by `--query` pairs.
    pub query: Vec<(String, String)>,
    /// Whether to follow pagination.
    pub all: bool,
}

impl ApiRequest {
    /// Validates and normalises the arguments.
    pub fn from_args(args: &ApiArgs) -> Result<Self> {
        let (path, mut query) = split_path(&args.path)?;
        query.extend(args.query.iter().cloned());
        Ok(Self {
            path,
            query,
            all: args.all,
        })
    }
}

/// Runs `hw api <PATH>`: JSON bodies become a JSON payload, anything else a
/// [`Outcome::server_text`] payload (the renderer decides on escaping).
pub fn run(request: &ApiRequest, api: &dyn Api) -> anyhow::Result<Outcome> {
    let ApiRequest { path, query, all } = request;
    if *all {
        return Ok(Outcome::json(fetch_all(api, path, query, MAX_PER_PAGE)?));
    }
    let response = api.get(path, query)?;
    let hint = response
        .pagination
        .filter(Pagination::has_more)
        .map(|p| p.hint());
    let outcome = match response.body {
        ResponseBody::Json(value) => Outcome::json(value),
        ResponseBody::Text(text) => Outcome::server_text(text),
    };
    Ok(match hint {
        Some(hint) => outcome.with_hint(hint),
        None => outcome,
    })
}

/// Normalises `tables`, `api/tables`, `/api/tables?x=1` to `/api/tables` plus decoded query pairs.
///
/// Guarantees for the returned path: it is `/api` or starts with `/api/`, has no empty, `.` or
/// `..` segment, and holds only printable ASCII other than `%`, `\` and `#` — so no encoded or
/// backslash form of `..` can lead the request out of `/api/` of the selected CRM. Full URLs
/// are rejected. The query is not restricted: ureq re-encodes every pair.
pub fn split_path(input: &str) -> Result<(String, Vec<(String, String)>)> {
    let input = input.trim();
    if input.contains("://") {
        return Err(Error::InvalidInput(format!(
            "{input:?} is a URL; pass an API path such as `tables` and pick the CRM with --crm"
        )));
    }
    let (raw_path, raw_query) = input.split_once('?').unwrap_or((input, ""));
    if let Some(bad) = raw_path
        .chars()
        .find(|c| !c.is_ascii_graphic() || matches!(c, '%' | '\\' | '#'))
    {
        return Err(Error::InvalidInput(format!(
            "{raw_path:?} contains {bad:?}; an API path is printable ASCII without '%', '\\' or '#'"
        )));
    }
    let trimmed = raw_path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(Error::InvalidInput(
            "the API path is empty; try `hw api settings`".into(),
        ));
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(Error::InvalidInput(format!(
            "{raw_path:?} has an empty, '.' or '..' segment"
        )));
    }
    let path = if trimmed == "api" || trimmed.starts_with("api/") {
        format!("/{trimmed}")
    } else {
        format!("/api/{trimmed}")
    };

    let query = raw_query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect();
    Ok((path, query))
}

/// Decodes `%XX` escapes and `+` (as a space); malformed escapes are kept literally.
#[must_use]
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(byte) = hex {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::{Payload, pairs};
    use crate::test_util::FakeApi;
    use serde_json::json;

    fn args(path: &str, query: &[(&str, &str)], all: bool) -> ApiRequest {
        ApiRequest::from_args(&ApiArgs {
            path: path.into(),
            query: pairs(query),
            all,
            no_auth: false,
        })
        .unwrap()
    }

    #[test]
    fn path_forms_normalise() {
        for input in ["tables", "api/tables", "/api/tables", "/api/tables/"] {
            assert_eq!(split_path(input).unwrap().0, "/api/tables", "{input}");
        }
        assert_eq!(
            split_path("products?category_id=24&search=%D1%87%D0%B0%D0%B9&x").unwrap(),
            (
                "/api/products".to_owned(),
                pairs(&[("category_id", "24"), ("search", "чай"), ("x", "")])
            )
        );
    }

    #[test]
    fn rejects_urls_and_escapes() {
        for input in [
            "https://demo.hookah.work/api/tables",
            "",
            "/",
            "../settings",
            "a//b",
            "a b",
            "api/%2e%2e/login",
            "api/%2E%2E/login",
            "clients\\..\\login",
            "clients\\login",
            "clients/\u{1b}[2J",
            "clients/a\tb",
            "kl\u{0456}ents",
            "a#b",
        ] {
            assert!(
                matches!(split_path(input), Err(Error::InvalidInput(_))),
                "{input:?}"
            );
        }
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a+b%20c"), "a b c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn get_with_inline_and_extra_query() {
        let api = FakeApi::new().with_json("/api/products", json!([]));
        run(
            &args("products?type=dish", &[("expand", "components")], false),
            &api,
        )
        .unwrap();
        assert_eq!(
            api.requests(),
            vec![(
                "/api/products".to_owned(),
                pairs(&[("type", "dish"), ("expand", "components")])
            )]
        );
    }

    #[test]
    fn non_json_body_is_unescaped_server_text() {
        let api = FakeApi::new().with_text("/api/robots", "plain\r\ntext\u{1b}[2J");
        let outcome = run(&args("robots", &[], false), &api).unwrap();
        assert!(matches!(outcome.payload, Payload::ServerText(t) if t == "plain\r\ntext\u{1b}[2J"));
    }

    #[test]
    fn all_pages_through() {
        let api = FakeApi::new().with_page(
            "/api/clients",
            json!([{"id": 1}]),
            Pagination {
                total_count: 1,
                page_count: 1,
                current_page: 1,
                per_page: 100,
            },
        );
        let outcome = run(&args("clients?page=3", &[], true), &api).unwrap();
        assert_eq!(outcome.json_value(), Some(&json!([{"id": 1}])));
        assert_eq!(
            api.requests()[0].1,
            pairs(&[("page", "1"), ("per-page", "100")])
        );
    }
}
