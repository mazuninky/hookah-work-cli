//! Output formats: the [`Reporter`] trait, one reporter per [`OutputFormat`], and
//! [`render_value`], which applies `--jq`/`--template` before the reporter.

mod console;
mod csv_out;
mod json;
mod toml_out;
mod toon;
pub mod transform;

pub use console::ConsoleReporter;
pub use csv_out::CsvReporter;
pub use json::JsonReporter;
pub use toml_out::TomlReporter;
pub use toon::ToonReporter;
pub use transform::{Transformed, Transforms};

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::io::Write;

use clap::ValueEnum;
use serde_json::Value;

use crate::error::Result;

/// Formats a JSON result into a writer.
pub trait Reporter {
    /// Writes `value` to `writer`.
    fn report(&self, value: &Value, writer: &mut dyn Write) -> Result<()>;
}

/// Output format selected with `-F/--format`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Tables and `key: value` lines for people
    #[default]
    Console,
    /// Pretty-printed JSON
    Json,
    /// TOON, a compact JSON encoding for LLMs
    Toon,
    /// TOML (a top-level list is wrapped as `items`)
    Toml,
    /// CSV with a header row
    Csv,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self
            .to_possible_value()
            .map(|value| value.get_name().to_owned())
            .unwrap_or_default();
        f.write_str(&name)
    }
}

/// The reporter for `format`.
#[must_use]
pub fn reporter_for_format(format: OutputFormat, use_color: bool) -> Box<dyn Reporter> {
    match format {
        OutputFormat::Console => Box::new(ConsoleReporter::new(use_color)),
        OutputFormat::Json => Box::new(JsonReporter),
        OutputFormat::Toon => Box::new(ToonReporter),
        OutputFormat::Toml => Box::new(TomlReporter),
        OutputFormat::Csv => Box::new(CsvReporter),
    }
}

/// Applies the transforms, then writes the result: template text verbatim, values through
/// the reporter. An empty `--jq` result prints nothing.
pub fn render_value(
    value: Value,
    format: OutputFormat,
    transforms: &Transforms<'_>,
    use_color: bool,
    writer: &mut dyn Write,
) -> Result<()> {
    match transform::apply(value, transforms)? {
        Transformed::Text(text) => write_text(writer, &text),
        Transformed::Json(Value::Null) if transforms.has_jq() => Ok(()),
        Transformed::Json(value) => reporter_for_format(format, use_color).report(&value, writer),
    }
}

/// Writes `text` and a trailing newline unless it already ends with one.
pub fn write_text(writer: &mut dyn Write, text: &str) -> Result<()> {
    writer.write_all(text.as_bytes())?;
    if !text.is_empty() && !text.ends_with('\n') {
        writer.write_all(b"\n")?;
    }
    Ok(())
}

/// Text from the API made safe for a terminal: control characters other than `\n` and `\t`
/// become caret notation (ESC → `^[`, C1 controls → `\u{FFFD}`) and `\r\n` becomes `\n`,
/// so data cannot emit escape sequences. For console output and error messages; data formats
/// and piped `hw api` text stay exact ([`server_text`]).
#[must_use]
pub fn terminal_text(text: &str) -> Cow<'_, str> {
    if !text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' | '\t' => out.push(c),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                out.push('^');
                out.push(char::from(u8::try_from(c).unwrap_or(b'?') ^ 0x40));
            }
            c if c.is_control() => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Server text for stdout: [`terminal_text`] on a terminal, the exact text otherwise.
#[must_use]
pub fn server_text(text: &str, terminal: bool) -> Cow<'_, str> {
    if terminal {
        terminal_text(text)
    } else {
        Cow::Borrowed(text)
    }
}

/// A cell for tabular formats: strings verbatim, `null` empty, anything else compact JSON.
fn cell_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Keys of all objects in first-seen order, so sparse rows still get every column.
fn union_headers(items: &[Value]) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|map| map.keys())
        .filter(|key| seen.insert(key.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn transforms<'a>(jq: Option<&str>, template: Option<&'a str>) -> Transforms<'a> {
        Transforms::compile(jq, template).unwrap()
    }

    fn render(value: Value, format: OutputFormat, transforms: &Transforms<'_>) -> String {
        let mut buf = Vec::new();
        render_value(value, format, transforms, false, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn every_format_renders_a_list() {
        let value = json!([{"id": 1, "name": "Стол 1"}]);
        for format in OutputFormat::value_variants() {
            let out = render(value.clone(), *format, &Transforms::none());
            assert!(out.contains("Стол 1"), "{format}: {out}");
        }
    }

    #[test]
    fn jq_runs_before_the_reporter() {
        let jq = transforms(Some(".[0].name"), None);
        assert_eq!(
            render(json!([{"name": "a"}]), OutputFormat::Json, &jq),
            "\"a\"\n"
        );
        let empty = transforms(Some("empty"), None);
        assert_eq!(render(json!([1]), OutputFormat::Json, &empty), "");
    }

    #[test]
    fn template_bypasses_the_reporter() {
        let t = transforms(None, Some("{{ name }}"));
        assert_eq!(render(json!({"name": "x"}), OutputFormat::Json, &t), "x\n");
    }

    #[test]
    fn terminal_text_neutralises_control_characters() {
        assert!(matches!(terminal_text("Стол 1\n\tok"), Cow::Borrowed(_)));
        assert_eq!(
            terminal_text("a\u{1b}[31mb\r\nc\rd\u{7f}\u{9b}e\u{0}"),
            "a^[[31mb\nc^Md^?\u{FFFD}e^@"
        );
    }

    #[test]
    fn server_text_is_escaped_only_on_a_terminal() {
        let text = "plain\r\ntext\u{1b}[2J";
        assert_eq!(server_text(text, true), "plain\ntext^[[2J");
        assert!(matches!(server_text(text, false), Cow::Borrowed(t) if t == text));
    }

    #[test]
    fn format_display_matches_cli_name() {
        assert_eq!(OutputFormat::Console.to_string(), "console");
        assert_eq!(OutputFormat::Csv.to_string(), "csv");
    }
}
