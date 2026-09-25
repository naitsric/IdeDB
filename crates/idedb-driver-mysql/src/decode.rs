//! Text-protocol decoding of MySQL result columns into [`Value`].
//!
//! `query_iter` uses the text protocol, so every non-NULL cell arrives as
//! bytes. The column metadata decides how to read them: integers and floats
//! become native values, binary strings stay bytes, everything else (decimal,
//! temporal, JSON, enum, ...) keeps the exact text the server printed.

use idedb_core::Value;
use mysql_async::Column;
use mysql_async::Value as MyValue;
use mysql_async::consts::{ColumnFlags, ColumnType};

/// The `binary` pseudo charset: string and blob columns with it hold raw bytes.
const BINARY_CHARSET: u16 = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Int,
    UnsignedInt,
    Float,
    Bytes,
    Text,
}

impl Kind {
    pub(crate) fn of(column: &Column) -> Kind {
        use ColumnType::*;
        let unsigned = column.flags().contains(ColumnFlags::UNSIGNED_FLAG);
        match column.column_type() {
            MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG
            | MYSQL_TYPE_LONGLONG | MYSQL_TYPE_YEAR => {
                if unsigned {
                    Kind::UnsignedInt
                } else {
                    Kind::Int
                }
            }
            MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => Kind::Float,
            MYSQL_TYPE_BIT | MYSQL_TYPE_GEOMETRY | MYSQL_TYPE_VECTOR => Kind::Bytes,
            // Decimal, temporal and JSON columns also report the binary
            // charset, so the charset only decides for string and blob types.
            MYSQL_TYPE_VARCHAR
            | MYSQL_TYPE_VAR_STRING
            | MYSQL_TYPE_STRING
            | MYSQL_TYPE_TINY_BLOB
            | MYSQL_TYPE_MEDIUM_BLOB
            | MYSQL_TYPE_LONG_BLOB
            | MYSQL_TYPE_BLOB
                if column.character_set() == BINARY_CHARSET =>
            {
                Kind::Bytes
            }
            _ => Kind::Text,
        }
    }

    pub(crate) fn decode(self, value: MyValue) -> Value {
        match value {
            MyValue::NULL => Value::Null,
            MyValue::Bytes(bytes) => self.decode_bytes(bytes),
            // The text protocol only produces NULL and bytes; these cover
            // values that arrive already typed.
            MyValue::Int(i) => Value::Int(i),
            MyValue::UInt(u) => unsigned(u),
            MyValue::Float(f) => Value::Float(f.into()),
            MyValue::Double(d) => Value::Float(d),
            MyValue::Date(y, mo, d, h, mi, s, us) => {
                let mut text = format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}");
                push_micros(&mut text, us);
                Value::Text(text)
            }
            MyValue::Time(negative, days, h, mi, s, us) => {
                let hours = days * 24 + u32::from(h);
                let mut text = format!(
                    "{}{hours:02}:{mi:02}:{s:02}",
                    if negative { "-" } else { "" }
                );
                push_micros(&mut text, us);
                Value::Text(text)
            }
        }
    }

    fn decode_bytes(self, bytes: Vec<u8>) -> Value {
        let parsed = match self {
            Kind::Bytes => return Value::Bytes(bytes),
            Kind::Int => std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| s.parse().ok())
                .map(Value::Int),
            Kind::UnsignedInt => std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| s.parse().ok())
                .map(unsigned),
            Kind::Float => std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| s.parse().ok())
                .map(Value::Float),
            Kind::Text => None,
        };
        parsed.unwrap_or_else(|| match String::from_utf8(bytes) {
            Ok(text) => Value::Text(text),
            Err(e) => Value::Text(String::from_utf8_lossy(e.as_bytes()).into_owned()),
        })
    }
}

/// Decodes a binary-protocol cell (prepared statements), where temporal
/// values arrive typed. A DATE prints without a time part, as the text
/// protocol shows it.
pub(crate) fn binary(column: &Column, kind: Kind, value: MyValue) -> Value {
    match (column.column_type(), value) {
        (ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE, MyValue::Date(y, mo, d, ..)) => {
            Value::Text(format!("{y:04}-{mo:02}-{d:02}"))
        }
        (_, value) => kind.decode(value),
    }
}

/// Unsigned BIGINT values above `i64::MAX` travel as text to stay exact.
fn unsigned(u: u64) -> Value {
    i64::try_from(u).map_or_else(|_| Value::Text(u.to_string()), Value::Int)
}

fn push_micros(text: &mut String, micros: u32) {
    if micros != 0 {
        text.push('.');
        text.push_str(format!("{micros:06}").trim_end_matches('0'));
    }
}

/// Type name as the user would write it, e.g. `bigint unsigned`, `varchar`, `json`.
pub(crate) fn type_name(column: &Column) -> String {
    use ColumnType::*;
    let flags = column.flags();
    let binary = column.character_set() == BINARY_CHARSET;
    let name = match column.column_type() {
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_INT24 => "mediumint",
        MYSQL_TYPE_LONG => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_NULL => "null",
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "datetime",
        MYSQL_TYPE_YEAR => "year",
        MYSQL_TYPE_BIT => "bit",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_GEOMETRY => "geometry",
        MYSQL_TYPE_VECTOR => "vector",
        MYSQL_TYPE_ENUM => "enum",
        MYSQL_TYPE_SET => "set",
        _ if flags.contains(ColumnFlags::ENUM_FLAG) => "enum",
        _ if flags.contains(ColumnFlags::SET_FLAG) => "set",
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => {
            if binary {
                "varbinary"
            } else {
                "varchar"
            }
        }
        MYSQL_TYPE_STRING => {
            if binary {
                "binary"
            } else {
                "char"
            }
        }
        MYSQL_TYPE_TINY_BLOB => {
            if binary {
                "tinyblob"
            } else {
                "tinytext"
            }
        }
        MYSQL_TYPE_MEDIUM_BLOB => {
            if binary {
                "mediumblob"
            } else {
                "mediumtext"
            }
        }
        MYSQL_TYPE_LONG_BLOB => {
            if binary {
                "longblob"
            } else {
                "longtext"
            }
        }
        MYSQL_TYPE_BLOB => {
            if binary {
                "blob"
            } else {
                "text"
            }
        }
        MYSQL_TYPE_TYPED_ARRAY | MYSQL_TYPE_UNKNOWN => "unknown",
    };
    let numeric = column.column_type().is_numeric_type() || column.column_type() == MYSQL_TYPE_YEAR;
    if numeric && flags.contains(ColumnFlags::UNSIGNED_FLAG) && name != "year" {
        format!("{name} unsigned")
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_protocol_bytes_by_kind() {
        let bytes = |s: &str| MyValue::Bytes(s.as_bytes().to_vec());
        assert_eq!(Kind::Int.decode(bytes("-42")), Value::Int(-42));
        assert_eq!(Kind::UnsignedInt.decode(bytes("7")), Value::Int(7));
        assert_eq!(
            Kind::UnsignedInt.decode(bytes("18446744073709551615")),
            Value::Text("18446744073709551615".into())
        );
        assert_eq!(Kind::Float.decode(bytes("1.5")), Value::Float(1.5));
        assert_eq!(
            Kind::Text.decode(bytes("12.50")),
            Value::Text("12.50".into())
        );
        assert_eq!(
            Kind::Bytes.decode(MyValue::Bytes(vec![0, 255])),
            Value::Bytes(vec![0, 255])
        );
        assert_eq!(Kind::Int.decode(MyValue::NULL), Value::Null);
    }

    #[test]
    fn formats_typed_temporal_values() {
        assert_eq!(
            Kind::Text.decode(MyValue::Date(2026, 9, 25, 10, 11, 12, 500_000)),
            Value::Text("2026-09-25 10:11:12.5".into())
        );
        assert_eq!(
            Kind::Text.decode(MyValue::Time(true, 1, 2, 3, 4, 0)),
            Value::Text("-26:03:04".into())
        );
    }
}
