//! TOML output.

use std::io::Write;

use serde_json::{Map, Value};

use super::Reporter;
use crate::error::{Error, Result};

/// TOML reporter. TOML has no `null` and needs a table at the root, so nulls are dropped,
/// a root list is wrapped as `items = [...]` and a root scalar as `value = ...`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TomlReporter;

impl Reporter for TomlReporter {
    fn report(&self, value: &Value, writer: &mut dyn Write) -> Result<()> {
        let root = match strip_nulls(value) {
            Value::Object(map) => Value::Object(map),
            Value::Array(items) => wrap("items", Value::Array(items)),
            Value::Null => Value::Object(Map::new()),
            scalar => wrap("value", scalar),
        };
        let text =
            toml::to_string_pretty(&root).map_err(|e| Error::Render(format!("toml: {e}")))?;
        writer.write_all(text.as_bytes())?;
        Ok(())
    }
}

fn wrap(key: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(key.to_owned(), value);
    Value::Object(map)
}

fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), strip_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|v| !v.is_null())
                .map(strip_nulls)
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(value: &Value) -> String {
        let mut buf = Vec::new();
        TomlReporter.report(value, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn object_without_nulls() {
        let out = render(&json!({"interval": "30", "gone": null}));
        assert!(out.contains("interval = \"30\""), "{out}");
        assert!(!out.contains("gone"), "{out}");
    }

    #[test]
    fn root_list_is_wrapped() {
        let out = render(&json!([{"id": 1, "seats": null}]));
        assert!(out.contains("[[items]]") && out.contains("id = 1"), "{out}");
    }

    #[test]
    fn root_scalar_is_wrapped() {
        assert_eq!(render(&json!("x")), "value = \"x\"\n");
    }
}
