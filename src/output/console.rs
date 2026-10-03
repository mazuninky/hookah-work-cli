//! Human-readable output: tables for lists of objects, `key: value` lines for objects.
//! Every key and value passes through [`terminal_text`]: API data must not drive the terminal.

use std::io::Write;

use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use serde_json::Value;

use super::{Reporter, cell_text, terminal_text, union_headers};
use crate::error::Result;

/// Console reporter; bold keys when color is enabled.
#[derive(Debug, Clone, Copy)]
pub struct ConsoleReporter {
    use_color: bool,
}

impl ConsoleReporter {
    /// A reporter that emits ANSI bold only when `use_color` is set.
    #[must_use]
    pub fn new(use_color: bool) -> Self {
        Self { use_color }
    }

    fn table(items: &[Value], writer: &mut dyn Write) -> Result<()> {
        let headers = union_headers(items);
        let mut table = Table::new();
        table
            .load_style(UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(headers.iter().map(|h| table_cell(h)));
        for item in items {
            table.add_row(headers.iter().map(|h| {
                item.get(h.as_str())
                    .map(|v| table_cell(&cell_text(v)))
                    .unwrap_or_default()
            }));
        }
        writeln!(writer, "{table}")?;
        Ok(())
    }
}

impl Reporter for ConsoleReporter {
    fn report(&self, value: &Value, writer: &mut dyn Write) -> Result<()> {
        match value {
            Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_object) => {
                Self::table(items, writer)?;
            }
            Value::Array(items) => {
                for item in items {
                    writeln!(writer, "- {}", terminal_text(&cell_text(item)))?;
                }
            }
            Value::Object(map) => {
                for (key, val) in map {
                    let key = terminal_text(key);
                    let text = if val.is_null() {
                        "null".to_owned()
                    } else {
                        terminal_text(&cell_text(val)).into_owned()
                    };
                    if self.use_color {
                        writeln!(writer, "\x1b[1m{key}\x1b[0m: {text}")?;
                    } else {
                        writeln!(writer, "{key}: {text}")?;
                    }
                }
            }
            Value::Null => {}
            scalar => writeln!(writer, "{}", terminal_text(&cell_text(scalar)))?,
        }
        Ok(())
    }
}

/// Tabs become spaces in a table: a terminal expands them past the computed column width.
fn table_cell(text: &str) -> String {
    terminal_text(text).replace('\t', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(value: &Value, color: bool) -> String {
        let mut buf = Vec::new();
        ConsoleReporter::new(color).report(value, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn list_of_objects_is_a_table() {
        let out = render(&json!([{"id": 1, "name": "Стол 1", "seats": null}]), false);
        assert!(out.contains("Стол 1") && out.contains("name"), "{out}");
    }

    #[test]
    fn object_is_key_value_lines() {
        let out = render(
            &json!({"timezone": "Asia/Yekaterinburg", "x": null, "n": [1]}),
            false,
        );
        assert_eq!(out, "timezone: Asia/Yekaterinburg\nx: null\nn: [1]\n");
        assert!(render(&json!({"a": 1}), true).contains("\x1b[1ma\x1b[0m"));
    }

    #[test]
    fn api_text_cannot_emit_escape_sequences() {
        let hostile = "Яшин\u{1b}]52;c;cHduZWQ=\u{7}";
        let outputs = [
            render(&json!([{"name": hostile, "x\u{1b}[2J": 1}]), false),
            render(&json!({"name": hostile}), true),
            render(&json!([hostile]), false),
            render(&json!(hostile), false),
        ];
        for out in outputs {
            assert!(!out.contains('\u{7}'), "{out:?}");
            assert!(out.contains("Яшин^[]52;c;cHduZWQ=^G"), "{out:?}");
        }
        let table = render(&json!([{"a\tb": "c\td"}]), false);
        assert!(table.contains("a b") && table.contains("c d"), "{table}");
    }

    #[test]
    fn scalars_and_plain_lists() {
        assert_eq!(
            render(&json!("/tmp/config.toml"), false),
            "/tmp/config.toml\n"
        );
        assert_eq!(render(&json!(["a", 2]), false), "- a\n- 2\n");
        assert_eq!(render(&Value::Null, false), "");
        assert_eq!(render(&json!([]), false), "");
    }
}
