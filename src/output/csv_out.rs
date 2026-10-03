//! CSV output.

use std::io::Write;

use serde_json::Value;

use super::{Reporter, cell_text, union_headers};
use crate::error::{Error, Result};

/// CSV reporter: a list of objects becomes a header (union of keys) plus one row per item;
/// an object becomes one row; other lists become a single `value` column. Nested values are
/// compact JSON cells.
#[derive(Debug, Clone, Copy, Default)]
pub struct CsvReporter;

impl Reporter for CsvReporter {
    fn report(&self, value: &Value, writer: &mut dyn Write) -> Result<()> {
        let mut csv = csv::Writer::from_writer(writer);
        match value {
            Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_object) => {
                let headers = union_headers(items);
                csv.write_record(&headers).map_err(csv_error)?;
                for item in items {
                    csv.write_record(
                        headers
                            .iter()
                            .map(|h| item.get(h.as_str()).map(cell_text).unwrap_or_default()),
                    )
                    .map_err(csv_error)?;
                }
            }
            Value::Array(items) => {
                if !items.is_empty() {
                    csv.write_record(["value"]).map_err(csv_error)?;
                }
                for item in items {
                    csv.write_record([cell_text(item)]).map_err(csv_error)?;
                }
            }
            Value::Object(map) => {
                csv.write_record(map.keys()).map_err(csv_error)?;
                csv.write_record(map.values().map(cell_text))
                    .map_err(csv_error)?;
            }
            Value::Null => {}
            scalar => csv.write_record([cell_text(scalar)]).map_err(csv_error)?,
        }
        csv.flush()?;
        Ok(())
    }
}

fn csv_error(err: csv::Error) -> Error {
    Error::Render(format!("csv: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(value: &Value) -> String {
        let mut buf = Vec::new();
        CsvReporter.report(value, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn list_of_objects_with_sparse_keys() {
        let out = render(&json!([{"id": 1, "name": "a"}, {"id": 2, "extra": {"k": 1}}]));
        assert_eq!(out, "id,name,extra\n1,a,\n2,,\"{\"\"k\"\":1}\"\n");
    }

    #[test]
    fn object_is_one_row_and_scalars_one_column() {
        assert_eq!(render(&json!({"a": 1, "b": null})), "a,b\n1,\n");
        assert_eq!(render(&json!([1, "x"])), "value\n1\nx\n");
        assert_eq!(render(&json!([])), "");
    }
}
