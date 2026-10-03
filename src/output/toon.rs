//! TOON output (token-efficient JSON for LLM consumers).

use std::io::Write;

use serde_json::Value;

use super::Reporter;
use crate::error::{Error, Result};

/// TOON reporter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToonReporter;

impl Reporter for ToonReporter {
    fn report(&self, value: &Value, writer: &mut dyn Write) -> Result<()> {
        let encoded =
            toon_format::encode_default(value).map_err(|e| Error::Render(format!("toon: {e}")))?;
        writer.write_all(encoded.as_bytes())?;
        if !encoded.ends_with('\n') {
            writeln!(writer)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_list_of_objects() {
        let mut buf = Vec::new();
        ToonReporter
            .report(
                &json!([{"id": 1, "name": "a"}, {"id": 2, "name": "b"}]),
                &mut buf,
            )
            .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("id") && text.contains('b'), "{text}");
        assert!(text.ends_with('\n'));
    }
}
