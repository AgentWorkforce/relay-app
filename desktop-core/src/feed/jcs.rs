//! The JSON Canonicalization Scheme (RFC 8785) for a feed manifest.
//!
//! The signature covers these bytes of the `manifest` member as received,
//! so a manifest re-serialised with other whitespace or member order still
//! verifies. Members are sorted by the UTF-16 code units of their names,
//! nothing is padded, strings are escaped as section 3.2.2.2 requires, and
//! numbers are written as section 3.2.2.3 does for an integer. The manifest
//! carries only safe integers, so any other number is refused rather than
//! formatted as an ECMAScript double.

use std::fmt;

use serde_json::{Map, Number, Value};

use super::strict_json::MAX_SAFE_INTEGER;

/// Why a value has no canonical form here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JcsError {
    /// A number is not an integer within ±(2^53 − 1).
    Number,
}

impl fmt::Display for JcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a number is not a safe integer")
    }
}

impl std::error::Error for JcsError {}

/// The canonical UTF-8 bytes of `value`.
pub fn canonicalize(value: &Value) -> Result<Vec<u8>, JcsError> {
    let mut out = Vec::new();
    write_value(value, &mut out)?;
    Ok(out)
}

fn write_value(value: &Value, out: &mut Vec<u8>) -> Result<(), JcsError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => write_number(number, out)?,
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_value(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(members) => write_object(members, out)?,
    }
    Ok(())
}

fn write_object(members: &Map<String, Value>, out: &mut Vec<u8>) -> Result<(), JcsError> {
    let mut sorted: Vec<(&String, &Value)> = members.iter().collect();
    sorted.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
    out.push(b'{');
    for (index, (name, value)) in sorted.into_iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        write_string(name, out);
        out.push(b':');
        write_value(value, out)?;
    }
    out.push(b'}');
    Ok(())
}

fn write_number(number: &Number, out: &mut Vec<u8>) -> Result<(), JcsError> {
    let safe = |magnitude: u64| magnitude <= MAX_SAFE_INTEGER;
    let text = if let Some(value) = number.as_u64().filter(|&value| safe(value)) {
        value.to_string()
    } else if let Some(value) = number.as_i64().filter(|value| safe(value.unsigned_abs())) {
        value.to_string()
    } else {
        return Err(JcsError::Number);
    };
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

fn write_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    let mut buffer = [0u8; 4];
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\r' => out.extend_from_slice(b"\\r"),
            control if u32::from(control) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", u32::from(control)).as_bytes());
            }
            other => out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes()),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn canonical(value: &Value) -> String {
        String::from_utf8(canonicalize(value).expect("canonical")).expect("UTF-8")
    }

    #[test]
    fn sorts_names_by_utf16_code_units() {
        // U+1F600 is the surrogate pair D83D DE00 and sorts before U+FB01,
        // although its code point and its UTF-8 bytes sort after it.
        let value = json!({"\u{fb01}": 1, "\u{1f600}": 2, "b": 3, "a": 4, "\u{e9}": 5, "A": 6});
        assert_eq!(
            canonicalize(&value).expect("canonical"),
            b"{\"A\":6,\"a\":4,\"b\":3,\"\xc3\xa9\":5,\"\xf0\x9f\x98\x80\":2,\"\xef\xac\x81\":1}"
        );
    }

    #[test]
    fn escapes_strings_as_jcs_requires() {
        let value = json!([
            "\u{8}\t\n\u{c}\r\"\\",
            "\u{0}\u{1f}\u{1}",
            "/\u{7f}\u{e9}\u{20ac}"
        ]);
        assert_eq!(
            canonicalize(&value).expect("canonical"),
            b"[\"\\b\\t\\n\\f\\r\\\"\\\\\",\"\\u0000\\u001f\\u0001\",\"/\x7f\xc3\xa9\xe2\x82\xac\"]"
        );
    }

    #[test]
    fn writes_nested_values_without_whitespace() {
        let value = json!({
            "z": {"y": [1, -2, 9007199254740991_u64], "x": null},
            "a": [{"c": true, "b": false}, "s"]
        });
        assert_eq!(
            canonical(&value),
            r#"{"a":[{"b":false,"c":true},"s"],"z":{"x":null,"y":[1,-2,9007199254740991]}}"#
        );
    }

    #[test]
    fn writes_empty_objects_and_arrays() {
        let value = json!({"o": {}, "a": [], "n": [{}, []], "": 0});
        assert_eq!(canonical(&value), r#"{"":0,"a":[],"n":[{},[]],"o":{}}"#);
        assert_eq!(canonical(&json!({})), "{}");
        assert_eq!(canonical(&json!([])), "[]");
    }

    #[test]
    fn refuses_numbers_that_are_not_safe_integers() {
        for value in [
            json!(1.5),
            json!(1.0),
            json!(9007199254740992_u64),
            json!(-9007199254740992_i64),
            json!(u64::MAX),
            json!({"nested": [0.25]}),
        ] {
            assert_eq!(canonicalize(&value), Err(JcsError::Number), "{value}");
        }
        assert_eq!(
            canonical(&json!(-9007199254740991_i64)),
            "-9007199254740991"
        );
    }
}
