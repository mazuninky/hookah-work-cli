//! Maps non-2xx responses to [`Error`]. The API answers with Yii error bodies
//! (`{"name","message","code","status","type"}`; 422 is a list of `{field, message}`).

use serde_json::{Map, Value};

use crate::auth::Secret;
use crate::error::Error;

/// Longest response-body excerpt (in characters) placed into an error message.
pub const MESSAGE_LIMIT: usize = 200;

/// 401/403 → [`Error::Auth`], 404 → [`Error::NotFound`], 400/422 → [`Error::InvalidInput`],
/// anything else → [`Error::Api`]; the message is built by [`error_message`].
#[must_use]
pub fn error_for_status(status: u16, body: &str, secrets: &[&Secret]) -> Error {
    let message = error_message(status, body, secrets);
    match status {
        401 | 403 => Error::Auth(message),
        404 => Error::NotFound(message),
        400 | 422 => Error::InvalidInput(message),
        _ => Error::Api { status, message },
    }
}

/// The user-facing message: the JSON `message`, the joined 422 field messages, or the body
/// itself, with `secrets` masked, then sanitised and truncated to [`MESSAGE_LIMIT`] characters.
///
/// Masking runs on decoded JSON text (a JSON-escaped echo of a secret differs from the secret)
/// and before truncation (a secret cut at the limit would leave its prefix).
#[must_use]
pub fn error_message(status: u16, body: &str, secrets: &[&Secret]) -> String {
    let text = match serde_json::from_str::<Value>(body) {
        Ok(value) => match yii_message(&value) {
            Some(message) => mask(&message, secrets),
            None => mask_strings(value, secrets).to_string(),
        },
        Err(_) => mask(body, secrets),
    };
    let text = text.trim();
    if text.is_empty() {
        return format!("HTTP {status}");
    }
    excerpt(text)
}

/// `text` with every non-blank secret replaced by `***`. Longer secrets go first, so a secret
/// that is a substring of another cannot leave the rest of the longer one visible.
#[must_use]
pub(crate) fn mask(text: &str, secrets: &[&Secret]) -> String {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .filter(|secret| !secret.is_blank())
        .map(|secret| secret.expose())
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
        .into_iter()
        .fold(text.to_owned(), |text, secret| text.replace(secret, "***"))
}

/// [`mask`] applied to every string and key of a JSON value.
fn mask_strings(value: Value, secrets: &[&Secret]) -> Value {
    match value {
        Value::String(text) => Value::String(mask(&text, secrets)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| mask_strings(item, secrets))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| (mask(&key, secrets), mask_strings(item, secrets)))
                .collect::<Map<_, _>>(),
        ),
        other => other,
    }
}

fn yii_message(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned),
        Value::Array(items) => field_messages(items),
        _ => None,
    }
}

fn field_messages(items: &[Value]) -> Option<String> {
    let parts: Vec<String> = items
        .iter()
        .filter_map(|item| {
            let message = item.get("message")?.as_str()?;
            Some(match item.get("field").and_then(Value::as_str) {
                Some(field) => format!("{field}: {message}"),
                None => message.to_owned(),
            })
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// Server text for an error message: truncated to [`MESSAGE_LIMIT`] characters, control
/// characters (ANSI escapes included) replaced so a hostile body cannot drive the terminal.
pub(crate) fn excerpt(text: &str) -> String {
    let mut out: String = text
        .chars()
        .take(MESSAGE_LIMIT)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.chars().nth(MESSAGE_LIMIT).is_some() {
        out.push('…');
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const YII_401: &str = r#"{"name":"Unauthorized","message":"Your request was made with invalid credentials.","code":0,"status":401,"type":"yii\\web\\UnauthorizedHttpException"}"#;

    #[test]
    fn maps_statuses_to_variants() {
        assert!(
            matches!(error_for_status(401, YII_401, &[]), Error::Auth(m) if m == "Your request was made with invalid credentials.")
        );
        assert!(
            matches!(error_for_status(403, "{\"message\":\"Нет доступа\"}", &[]), Error::Auth(m) if m == "Нет доступа")
        );
        assert!(matches!(
            error_for_status(404, "{\"message\":\"Бронь не найдена\"}", &[]),
            Error::NotFound(_)
        ));
        assert!(matches!(error_for_status(400, "{}", &[]), Error::InvalidInput(m) if m == "{}"));
        assert!(matches!(
            error_for_status(422, "[]", &[]),
            Error::InvalidInput(_)
        ));
        assert!(matches!(
            error_for_status(503, "", &[]),
            Error::Api { status: 503, message } if message == "HTTP 503"
        ));
    }

    #[test]
    fn joins_validation_messages() {
        let body =
            r#"[{"field":"date","message":"Неверная дата"},{"field":"id","message":"Нужен id"}]"#;
        assert_eq!(
            error_message(422, body, &[]),
            "date: Неверная дата; id: Нужен id"
        );
    }

    #[test]
    fn truncates_and_sanitises_plain_bodies() {
        let long = "x".repeat(500);
        let message = error_message(500, &long, &[]);
        assert_eq!(message.chars().count(), MESSAGE_LIMIT + 1);
        assert!(message.ends_with('…'));
        assert_eq!(
            error_message(502, "bad\u{1b}[31mgateway", &[]),
            "bad [31mgateway"
        );
    }

    /// `secret` as a server echoes it inside a JSON string.
    fn json_escaped(secret: &str) -> String {
        let quoted = serde_json::to_string(secret).unwrap();
        quoted[1..quoted.len() - 1].to_owned()
    }

    #[test]
    fn masks_json_escaped_secrets_in_every_body_shape() {
        for raw in [r#"p"a\ss"#, r"back\slash", "q\"uote"] {
            let secret = Secret::new(raw);
            let escaped = json_escaped(raw);
            assert_ne!(escaped, raw);
            let message = json!({"message": format!("wrong password {raw}")}).to_string();
            assert_eq!(
                error_message(401, &message, &[&secret]),
                "wrong password ***"
            );
            let fields = json!([{"field": "password", "message": format!("{raw} is wrong")}]);
            assert_eq!(
                error_message(422, &fields.to_string(), &[&secret]),
                "password: *** is wrong"
            );
            let other = json!({"errors": {"password": [format!("{raw} is wrong")]}});
            assert_eq!(
                error_message(400, &other.to_string(), &[&secret]),
                r#"{"errors":{"password":["*** is wrong"]}}"#
            );
            assert_eq!(
                error_message(500, &format!("echo {raw}"), &[&secret]),
                "echo ***"
            );
        }
    }

    #[test]
    fn masks_before_truncating() {
        let secret = Secret::new(r#"tok"SECRET\42"#);
        let echoed = format!(
            "{}{} and more",
            "x".repeat(MESSAGE_LIMIT - 4),
            secret.expose()
        );
        for body in [json!({"message": echoed}).to_string(), echoed.clone()] {
            let message = error_message(500, &body, &[&secret]);
            assert!(message.ends_with("*** …"), "{message}");
            assert!(!message.contains("tok"), "{message}");
        }
    }

    #[test]
    fn longer_secrets_are_masked_first() {
        let token = Secret::new("abc");
        let password = Secret::new("abcdef");
        assert_eq!(mask("abcdef abc", &[&token, &password]), "*** ***");
        assert_eq!(mask("keep", &[&Secret::new("  ")]), "keep");
    }
}
