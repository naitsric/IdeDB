//! Binary-protocol decoding of arbitrary Postgres columns into [`Value`].
//!
//! A generic client cannot know result types at compile time, so every cell
//! goes through [`Cell`], a `FromSql` that accepts any type and dispatches on
//! the column's runtime type. Types without a native `Value` variant are
//! rendered as the same text `psql` would print.

use std::error::Error as StdError;
use std::fmt::Write as _;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use idedb_core::Value;
use fallible_iterator::FallibleIterator;
use postgres_protocol::types as proto;
use tokio_postgres::types::{FromSql, Kind, Type};

type BoxError = Box<dyn StdError + Sync + Send>;

pub(crate) struct Cell(pub Value);

impl<'a> FromSql<'a> for Cell {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        // A single undecodable cell must not fail the whole result set.
        Ok(Cell(decode(ty, raw).unwrap_or_else(|e| {
            Value::Text(format!("<{}: {e}>", ty.name()))
        })))
    }

    fn from_sql_null(_: &Type) -> Result<Self, BoxError> {
        Ok(Cell(Value::Null))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

fn decode(ty: &Type, raw: &[u8]) -> Result<Value, BoxError> {
    Ok(match *ty {
        Type::BOOL => Value::Bool(bool::from_sql(ty, raw)?),
        Type::INT2 => Value::Int(i16::from_sql(ty, raw)?.into()),
        Type::INT4 => Value::Int(i32::from_sql(ty, raw)?.into()),
        Type::INT8 => Value::Int(i64::from_sql(ty, raw)?),
        Type::OID => Value::Int(u32::from_sql(ty, raw)?.into()),
        Type::FLOAT4 => Value::Float(f32::from_sql(ty, raw)?.into()),
        Type::FLOAT8 => Value::Float(f64::from_sql(ty, raw)?),
        Type::NUMERIC => Value::Text(numeric_to_string(raw)?),
        Type::BYTEA => Value::Bytes(raw.to_vec()),
        Type::JSON | Type::JSONB => Value::Text(serde_json::Value::from_sql(ty, raw)?.to_string()),
        Type::UUID => Value::Text(uuid::Uuid::from_sql(ty, raw)?.to_string()),
        Type::DATE => Value::Text(NaiveDate::from_sql(ty, raw)?.to_string()),
        Type::TIME => Value::Text(NaiveTime::from_sql(ty, raw)?.to_string()),
        Type::TIMESTAMP => Value::Text(timestamp_to_string(ty, raw)?),
        Type::TIMESTAMPTZ => Value::Text(timestamptz_to_string(ty, raw)?),
        Type::INTERVAL => Value::Text(interval_to_string(raw)?),
        _ => match ty.kind() {
            Kind::Domain(inner) => decode(inner, raw)?,
            Kind::Array(element) => Value::Text(array_to_string(element, raw)?),
            // Text-like types (text, varchar, name, citext, enums, ...) send
            // their UTF-8 representation in binary format too.
            Kind::Enum(_) => Value::Text(std::str::from_utf8(raw)?.to_owned()),
            _ if is_textual(ty) => Value::Text(std::str::from_utf8(raw)?.to_owned()),
            _ => Value::Bytes(raw.to_vec()),
        },
    })
}

fn is_textual(ty: &Type) -> bool {
    matches!(
        *ty,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN | Type::XML
    ) || matches!(ty.name(), "citext" | "ltree" | "lquery")
}

fn timestamp_to_string(ty: &Type, raw: &[u8]) -> Result<String, BoxError> {
    match proto::timestamp_from_sql(raw)? {
        i64::MAX => Ok("infinity".into()),
        i64::MIN => Ok("-infinity".into()),
        _ => Ok(NaiveDateTime::from_sql(ty, raw)?.to_string()),
    }
}

fn timestamptz_to_string(ty: &Type, raw: &[u8]) -> Result<String, BoxError> {
    match proto::timestamp_from_sql(raw)? {
        i64::MAX => Ok("infinity".into()),
        i64::MIN => Ok("-infinity".into()),
        _ => Ok(DateTime::<Utc>::from_sql(ty, raw)?.format("%Y-%m-%d %H:%M:%S%.f%:z").to_string()),
    }
}

/// Postgres `numeric` wire format: ndigits, weight, sign, dscale, then
/// `ndigits` base-10000 digits where digit `i` has weight `weight - i`.
pub(crate) fn numeric_to_string(raw: &[u8]) -> Result<String, BoxError> {
    const NEG: u16 = 0x4000;
    const NAN: u16 = 0xC000;
    const PINF: u16 = 0xD000;
    const NINF: u16 = 0xF000;

    let word = |i: usize| -> Result<u16, BoxError> {
        raw.get(i * 2..i * 2 + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .ok_or_else(|| "truncated numeric".into())
    };
    let ndigits = word(0)? as usize;
    let weight = word(1)? as i16 as i32;
    let sign = word(2)?;
    let dscale = word(3)? as usize;
    let digits = (0..ndigits).map(|i| word(4 + i)).collect::<Result<Vec<_>, _>>()?;
    let digit = |i: i32| -> u16 {
        usize::try_from(i).ok().and_then(|i| digits.get(i).copied()).unwrap_or(0)
    };

    match sign {
        NAN => return Ok("NaN".into()),
        PINF => return Ok("Infinity".into()),
        NINF => return Ok("-Infinity".into()),
        _ => {}
    }

    let mut out = String::new();
    if sign == NEG {
        out.push('-');
    }
    if weight < 0 {
        out.push('0');
    } else {
        write!(out, "{}", digit(0))?;
        for i in 1..=weight {
            write!(out, "{:04}", digit(i))?;
        }
    }
    if dscale > 0 {
        let mut frac = String::with_capacity(dscale + 4);
        let mut i = weight + 1;
        while frac.len() < dscale {
            write!(frac, "{:04}", digit(i))?;
            i += 1;
        }
        frac.truncate(dscale);
        out.push('.');
        out.push_str(&frac);
    }
    Ok(out)
}

/// Formats like Postgres' default `IntervalStyle = postgres`.
pub(crate) fn interval_to_string(raw: &[u8]) -> Result<String, BoxError> {
    if raw.len() != 16 {
        return Err("invalid interval length".into());
    }
    let micros = i64::from_be_bytes(raw[0..8].try_into()?);
    let days = i32::from_be_bytes(raw[8..12].try_into()?);
    let months = i32::from_be_bytes(raw[12..16].try_into()?);

    let mut parts = Vec::new();
    let (years, mons) = (months / 12, months % 12);
    let plural = |n: i32, one: &str, many: &str| format!("{n} {}", if n.abs() == 1 { one } else { many });
    if years != 0 {
        parts.push(plural(years, "year", "years"));
    }
    if mons != 0 {
        parts.push(plural(mons, "mon", "mons"));
    }
    if days != 0 {
        parts.push(plural(days, "day", "days"));
    }
    if micros != 0 || parts.is_empty() {
        let sign = if micros < 0 { "-" } else { "" };
        let us = micros.unsigned_abs();
        let (h, m, s, frac) = (us / 3_600_000_000, us / 60_000_000 % 60, us / 1_000_000 % 60, us % 1_000_000);
        let mut time = format!("{sign}{h:02}:{m:02}:{s:02}");
        if frac != 0 {
            write!(time, ".{}", format!("{frac:06}").trim_end_matches('0'))?;
        }
        parts.push(time);
    }
    Ok(parts.join(" "))
}

/// Renders an array as a Postgres array literal, e.g. `{1,2,NULL}` or
/// `{{a,b},{c,d}}`, decoding each element with its element type.
fn array_to_string(element: &Type, raw: &[u8]) -> Result<String, BoxError> {
    let array = proto::array_from_sql(raw)?;
    let dims: Vec<usize> = array
        .dimensions()
        .map(|d| Ok(usize::try_from(d.len).unwrap_or(0)))
        .collect()?;
    let items: Vec<String> = array
        .values()
        .map(|v| {
            Ok(match v {
                None => "NULL".to_owned(),
                Some(bytes) => {
                    array_item_literal(&decode(element, bytes).unwrap_or(Value::Text("?".into())))
                }
            })
        })
        .collect()?;

    fn nest(out: &mut String, dims: &[usize], items: &mut std::slice::Iter<'_, String>) {
        out.push('{');
        for i in 0..dims[0] {
            if i > 0 {
                out.push(',');
            }
            if dims.len() > 1 {
                nest(out, &dims[1..], items);
            } else if let Some(item) = items.next() {
                out.push_str(item);
            }
        }
        out.push('}');
    }

    let mut out = String::new();
    if dims.is_empty() {
        out.push_str("{}");
    } else {
        nest(&mut out, &dims, &mut items.iter());
    }
    Ok(out)
}

fn array_item_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(b) => if *b { "t" } else { "f" }.into(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bytes(b) => {
            let mut s = String::from("\"\\\\x");
            for byte in b {
                let _ = write!(s, "{byte:02x}");
            }
            s.push('"');
            s
        }
        Value::Text(s) => {
            let needs_quotes = s.is_empty()
                || s.eq_ignore_ascii_case("null")
                || s.chars().any(|c| matches!(c, ',' | '{' | '}' | '"' | '\\') || c.is_whitespace());
            if needs_quotes {
                format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                s.clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numeric(ndigits: u16, weight: i16, sign: u16, dscale: u16, digits: &[u16]) -> Vec<u8> {
        let mut v = Vec::new();
        for w in [ndigits, weight as u16, sign, dscale].into_iter().chain(digits.iter().copied()) {
            v.extend_from_slice(&w.to_be_bytes());
        }
        v
    }

    #[test]
    fn numeric_formats_like_postgres() {
        assert_eq!(numeric_to_string(&numeric(2, 0, 0, 2, &[123, 4500])).unwrap(), "123.45");
        assert_eq!(numeric_to_string(&numeric(1, -1, 0, 3, &[10])).unwrap(), "0.001");
        assert_eq!(numeric_to_string(&numeric(1, 1, 0, 0, &[1])).unwrap(), "10000");
        assert_eq!(numeric_to_string(&numeric(0, 0, 0, 2, &[])).unwrap(), "0.00");
        assert_eq!(numeric_to_string(&numeric(1, 0, 0x4000, 0, &[7])).unwrap(), "-7");
        assert_eq!(numeric_to_string(&numeric(0, 0, 0xC000, 0, &[])).unwrap(), "NaN");
    }

    #[test]
    fn interval_formats_like_postgres() {
        let iv = |micros: i64, days: i32, months: i32| {
            let mut v = micros.to_be_bytes().to_vec();
            v.extend_from_slice(&days.to_be_bytes());
            v.extend_from_slice(&months.to_be_bytes());
            v
        };
        assert_eq!(interval_to_string(&iv(0, 0, 0)).unwrap(), "00:00:00");
        assert_eq!(
            interval_to_string(&iv(4 * 3_600_000_000 + 5 * 60_000_000 + 6_500_000, 3, 14)).unwrap(),
            "1 year 2 mons 3 days 04:05:06.5"
        );
        assert_eq!(interval_to_string(&iv(-60_000_000, 1, 0)).unwrap(), "1 day -00:01:00");
    }
}
