//! Pretty-printed JSON output.

use std::io::Write;

use serde_json::Value;

use super::Reporter;
use crate::error::Result;

/// JSON reporter: the value as pretty JSON plus a trailing newline.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonReporter;

impl Reporter for JsonReporter {
    fn report(&self, value: &Value, writer: &mut dyn Write) -> Result<()> {
        serde_json::to_writer_pretty(&mut *writer, value)?;
        writeln!(writer)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_with_trailing_newline() {
        let value = json!({"b": 1, "a": [1, 2]});
        let mut buf = Vec::new();
        JsonReporter.report(&value, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.ends_with("}\n"));
        assert!(
            text.find("\"b\"") < text.find("\"a\""),
            "key order preserved: {text}"
        );
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
    }
}
