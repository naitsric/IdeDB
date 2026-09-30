//! What results look like to the model: JSON cells a model can read and a
//! JSON parser keeps exact, and results small enough for its context.

use idedb_core::{Column, QueryEvent, Value};
use serde_json::Value as Json;

/// The MCP server's `instructions`: how to use the tools, for the model.
pub const INSTRUCTIONS: &str = "\
IdeDB is a database IDE. Through it you can use the databases the user connected in IdeDB, as far as the user allows \
this client. IdeDB records every call in an audit log the user reviews.

- list_connections lists the connections this client may use. Pass a connection's id (or its name, when unique) as \
`connection` to the other tools.
- list_schemas, list_tables and describe_table show a connection's structure. Look before writing SQL.
- query runs one read-only statement (SELECT, WITH, SHOW, EXPLAIN...) on a read-only session. It returns at most 200 \
rows unless you pass maxRows (up to 1000), and refuses anything that writes.
- execute runs one statement that changes data or schema. The user must approve each one in IdeDB, so pass `reason` \
to say why; it may be rejected, and it waits a limited time (2 minutes by default) for an answer. It is refused \
without asking where this client may only read, or on connections the user marked never-write.
- One statement per call: no scripts of several statements separated by `;`, and no transaction control or session \
settings (BEGIN, COMMIT, SET, USE...). Each call runs on its own.
- Statements are cancelled after a timeout (30 seconds by default). Results are cut to a row limit and about 256 KB, \
and flagged `truncated` when cut: prefer WHERE, LIMIT and aggregates to reading whole tables.
- Values come back as JSON. Integers too large for a JSON number, NaN and infinities come back as strings; long text \
is cut, and binary values are summarized.";

/// Longest text value returned whole, in characters.
const MAX_TEXT_CHARS: usize = 2000;
/// Bytes of a binary value shown, in hex.
const SHOWN_BYTES: usize = 32;
/// Once the rows serialized so far reach this size, no more are added.
pub(crate) const MAX_RESULT_BYTES: usize = 256 * 1024;
/// The largest integer a JSON parser reads exactly into a double (2^53 - 1).
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// A cell as JSON for the model:
/// - integers beyond ±(2^53 - 1), and NaN or infinite floats, as strings;
/// - text over 2000 characters cut, with `…[truncated, N chars]` saying how
///   long it was;
/// - bytes as `<binary N bytes: 0x…>` with the first 32 in hex.
pub(crate) fn cell(value: Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(b),
        Value::Int(i) if i.unsigned_abs() > MAX_SAFE_INTEGER => Json::String(i.to_string()),
        Value::Int(i) => Json::from(i),
        Value::Float(f) => serde_json::Number::from_f64(f).map_or_else(|| Json::String(float_text(f)), Json::Number),
        Value::Text(text) => Json::String(cut_text(text)),
        Value::Bytes(bytes) => Json::String(binary(&bytes)),
    }
}

fn float_text(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f > 0.0 {
        "Infinity".into()
    } else {
        "-Infinity".into()
    }
}

fn cut_text(text: String) -> String {
    // Most values are short: counting stops early for those.
    if text.chars().nth(MAX_TEXT_CHARS).is_none() {
        return text;
    }
    let total = text.chars().count();
    let end = text.char_indices().nth(MAX_TEXT_CHARS).map_or(text.len(), |(i, _)| i);
    format!("{}…[truncated, {total} chars]", &text[..end])
}

fn binary(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().take(SHOWN_BYTES).map(|b| format!("{b:02x}")).collect();
    let more = if bytes.len() > SHOWN_BYTES { "…" } else { "" };
    format!("<binary {} bytes: 0x{hex}{more}>", bytes.len())
}

/// Collects what a statement emits into a result for the model: at most
/// `max_rows` rows, and none past [`MAX_RESULT_BYTES`] of serialized rows.
pub(crate) struct Collector {
    max_rows: usize,
    bytes: usize,
    pub columns: Option<Vec<Column>>,
    pub rows: Vec<Vec<Json>>,
    /// Rows arrived that the result has no room for.
    pub dropped: bool,
    /// The final `Done` or `Error`.
    pub last: Option<QueryEvent>,
}

impl Collector {
    pub fn new(max_rows: usize) -> Self {
        Self { max_rows, bytes: 0, columns: None, rows: Vec::new(), dropped: false, last: None }
    }

    pub fn push(&mut self, event: QueryEvent) {
        match event {
            QueryEvent::Columns { columns } => self.columns = Some(columns),
            QueryEvent::Rows { rows } => {
                for row in rows {
                    if self.rows.len() >= self.max_rows || self.bytes >= MAX_RESULT_BYTES {
                        self.dropped = true;
                        continue;
                    }
                    let row: Vec<Json> = row.into_iter().map(cell).collect();
                    // The row and the comma after it.
                    self.bytes += serde_json::to_vec(&row).map_or(0, |json| json.len()) + 1;
                    self.rows.push(row);
                }
            }
            last => self.last = Some(last),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_integers_json_parsers_read_exactly() {
        assert_eq!(cell(Value::Int(42)), json!(42));
        assert_eq!(cell(Value::Int(-(1 << 53) + 1)), json!(-9007199254740991i64));
        assert_eq!(cell(Value::Int(1 << 53)), json!("9007199254740992"));
        assert_eq!(cell(Value::Int(i64::MIN)), json!("-9223372036854775808"));
        assert_eq!(cell(Value::Int(i64::MAX)), json!("9223372036854775807"));
    }

    #[test]
    fn floats_without_a_json_number_are_strings() {
        assert_eq!(cell(Value::Float(1.5)), json!(1.5));
        assert_eq!(cell(Value::Float(f64::NAN)), json!("NaN"));
        assert_eq!(cell(Value::Float(f64::INFINITY)), json!("Infinity"));
        assert_eq!(cell(Value::Float(f64::NEG_INFINITY)), json!("-Infinity"));
    }

    #[test]
    fn cuts_long_text_on_a_char_boundary() {
        assert_eq!(cell(Value::Text("ñ".repeat(2000))), json!("ñ".repeat(2000)));
        let long = cell(Value::Text("ñ".repeat(2500)));
        assert_eq!(long, json!(format!("{}…[truncated, 2500 chars]", "ñ".repeat(2000))));
        assert_eq!((cell(Value::Null), cell(Value::Bool(true))), (Json::Null, json!(true)));
    }

    #[test]
    fn summarizes_bytes() {
        assert_eq!(cell(Value::Bytes(vec![0, 1, 0xab, 0xff])), json!("<binary 4 bytes: 0x0001abff>"));
        let big = cell(Value::Bytes((0..=255).collect()));
        let expected = format!("<binary 256 bytes: 0x{}…>", (0..32).map(|b| format!("{b:02x}")).collect::<String>());
        assert_eq!(big, json!(expected));
        assert_eq!(cell(Value::Bytes(Vec::new())), json!("<binary 0 bytes: 0x>"));
    }

    fn rows(n: usize, width: usize) -> QueryEvent {
        QueryEvent::Rows { rows: (0..n).map(|_| vec![Value::Text("x".repeat(width))]).collect() }
    }

    #[test]
    fn stops_at_the_row_limit() {
        let mut collector = Collector::new(3);
        collector.push(rows(2, 1));
        assert!(!collector.dropped);
        collector.push(rows(2, 1));
        assert_eq!(collector.rows.len(), 3);
        assert!(collector.dropped);
    }

    #[test]
    fn stops_at_about_256_kb() {
        // Rows of about 1 KB: the one that reaches the cap is the last kept.
        let mut collector = Collector::new(1000);
        collector.push(rows(1000, 1000));
        assert!(collector.dropped);
        let kept = collector.rows.len();
        assert_eq!(kept, 261);
        let size = serde_json::to_vec(&collector.rows).unwrap().len();
        assert!((MAX_RESULT_BYTES..MAX_RESULT_BYTES + 1100).contains(&size), "{size}");
    }
}
