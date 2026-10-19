//! Canonical JSON (ARCHITECTURE.md §1.1.1), a strict subset of RFC 8785.
//!
//! [`to_canonical`] turns a `serde_json::Value` into the one byte sequence that
//! hashing and signing use: no whitespace, object keys ASCII and sorted by
//! byte, integers within ±(2^53-1), no floats, JCS string escaping. It rejects
//! values outside that subset instead of normalizing them.

use serde_json::{Map, Number, Value};

/// Largest integer magnitude allowed in canonical JSON: 2^53 - 1.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    #[error("floating point number {0} is not allowed in canonical JSON")]
    Float(String),
    #[error("integer {0} is outside ±(2^53-1)")]
    IntegerOutOfRange(String),
    #[error("object key {0:?} is not ASCII")]
    NonAsciiKey(String),
}

/// Serializes `value` as canonical JSON.
pub fn to_canonical(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut out = Vec::new();
    write_value(&mut out, value)?;
    Ok(out)
}

fn write_value(out: &mut Vec<u8>, value: &Value) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => write_number(out, n)?,
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(out, item)?;
            }
            out.push(b']');
        }
        Value::Object(map) => write_object(out, map)?,
    }
    Ok(())
}

fn write_number(out: &mut Vec<u8>, n: &Number) -> Result<(), CanonicalError> {
    let in_range = if let Some(i) = n.as_i64() {
        i.unsigned_abs() <= MAX_SAFE_INTEGER
    } else if let Some(u) = n.as_u64() {
        u <= MAX_SAFE_INTEGER
    } else {
        return Err(CanonicalError::Float(n.to_string()));
    };
    if !in_range {
        return Err(CanonicalError::IntegerOutOfRange(n.to_string()));
    }
    out.extend_from_slice(n.to_string().as_bytes());
    Ok(())
}

// Sorting here rather than relying on Map iteration order keeps the output
// canonical even if a dependency enables serde_json's `preserve_order`.
fn write_object(out: &mut Vec<u8>, map: &Map<String, Value>) -> Result<(), CanonicalError> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    if let Some((key, _)) = entries.iter().find(|(k, _)| !k.is_ascii()) {
        return Err(CanonicalError::NonAsciiKey((*key).clone()));
    }
    entries.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    out.push(b'{');
    for (i, (key, value)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write_string(out, key);
        out.push(b':');
        write_value(out, value)?;
    }
    out.push(b'}');
    Ok(())
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for &b in s.as_bytes() {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x0a => out.extend_from_slice(b"\\n"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0d => out.extend_from_slice(b"\\r"),
            0x00..=0x1f => {
                out.extend_from_slice(b"\\u00");
                out.push(HEX[(b >> 4) as usize]);
                out.push(HEX[(b & 0xf) as usize]);
            }
            _ => out.push(b),
        }
    }
    out.push(b'"');
}
