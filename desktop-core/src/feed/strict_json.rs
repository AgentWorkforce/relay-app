//! A strict I-JSON (RFC 7493) acceptance pass over a feed envelope's raw
//! bytes, run before anything parses them.
//!
//! RFC 8785 requires its input to be I-JSON, but `serde_json` keeps the last
//! of two members with the same name. A text with a duplicate would then
//! canonicalise to whichever value survived, and a signature over one
//! reading would verify a text other parsers read differently. This pass
//! refuses, at any depth: a duplicate member name (compared after escape
//! decoding), a lone surrogate, a leading byte order mark, invalid UTF-8,
//! characters after the top-level value, and anything outside the RFC 8259
//! grammar. The manifest carries only safe integers, so a fraction, an
//! exponent, `-0` or an integer beyond ±(2^53−1) is refused too.

use std::collections::HashSet;
use std::fmt;

/// The largest integer the feed accepts: 2^53 − 1.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// How deeply values may nest. The envelope nests four levels; this bounds
/// the recursion of a hostile text.
const MAX_DEPTH: usize = 32;

/// Why a text is not strict I-JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrictJsonError {
    /// The bytes are not UTF-8.
    InvalidUtf8,
    /// The text starts with a byte order mark.
    ByteOrderMark,
    /// An object names a member twice.
    DuplicateName,
    /// A `\u` escape is an unpaired surrogate.
    LoneSurrogate,
    /// A number is not a safe integer.
    Number,
    /// Something follows the top-level value.
    TrailingCharacters,
    /// Values nest deeper than the feed ever needs.
    TooDeep,
    /// Anything else outside the RFC 8259 grammar.
    Syntax,
}

impl fmt::Display for StrictJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidUtf8 => "the text is not UTF-8",
            Self::ByteOrderMark => "the text starts with a byte order mark",
            Self::DuplicateName => "an object names a member twice",
            Self::LoneSurrogate => "a string holds a lone surrogate",
            Self::Number => "a number is not a safe integer",
            Self::TrailingCharacters => "characters follow the value",
            Self::TooDeep => "values nest too deeply",
            Self::Syntax => "the text is not JSON",
        })
    }
}

impl std::error::Error for StrictJsonError {}

/// Accepts `raw` only if it is one strict I-JSON value, as above.
pub fn check(raw: &[u8]) -> Result<(), StrictJsonError> {
    let text = std::str::from_utf8(raw).map_err(|_| StrictJsonError::InvalidUtf8)?;
    if text.starts_with('\u{feff}') {
        return Err(StrictJsonError::ByteOrderMark);
    }
    let mut parser = Parser {
        bytes: text.as_bytes(),
        at: 0,
    };
    parser.whitespace();
    parser.value(0)?;
    parser.whitespace();
    if parser.at != parser.bytes.len() {
        return Err(StrictJsonError::TrailingCharacters);
    }
    Ok(())
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> Result<u8, StrictJsonError> {
        let byte = self.peek().ok_or(StrictJsonError::Syntax)?;
        self.at += 1;
        Ok(byte)
    }

    fn consume(&mut self, wanted: u8) -> Result<(), StrictJsonError> {
        match self.next()? {
            byte if byte == wanted => Ok(()),
            _ => Err(StrictJsonError::Syntax),
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<(), StrictJsonError> {
        if depth > MAX_DEPTH {
            return Err(StrictJsonError::TooDeep);
        }
        match self.peek().ok_or(StrictJsonError::Syntax)? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(drop),
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            b'n' => self.literal(b"null"),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(StrictJsonError::Syntax),
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), StrictJsonError> {
        word.iter().try_for_each(|&byte| self.consume(byte))
    }

    fn object(&mut self, depth: usize) -> Result<(), StrictJsonError> {
        self.consume(b'{')?;
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(());
        }
        let mut names = HashSet::new();
        loop {
            if self.peek() != Some(b'"') {
                return Err(StrictJsonError::Syntax);
            }
            if !names.insert(self.string()?) {
                return Err(StrictJsonError::DuplicateName);
            }
            self.whitespace();
            self.consume(b':')?;
            self.whitespace();
            self.value(depth + 1)?;
            self.whitespace();
            match self.next()? {
                b',' => self.whitespace(),
                b'}' => return Ok(()),
                _ => return Err(StrictJsonError::Syntax),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<(), StrictJsonError> {
        self.consume(b'[')?;
        self.whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(());
        }
        loop {
            self.value(depth + 1)?;
            self.whitespace();
            match self.next()? {
                b',' => self.whitespace(),
                b']' => return Ok(()),
                _ => return Err(StrictJsonError::Syntax),
            }
        }
    }

    /// An integer: `-?(0|[1-9][0-9]*)`, at most 2^53 − 1 in magnitude, and
    /// not `-0`. A fraction or exponent is refused as a non-integer.
    fn number(&mut self) -> Result<(), StrictJsonError> {
        let negative = self.peek() == Some(b'-');
        if negative {
            self.at += 1;
        }
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        let digits = self.bytes.get(start..self.at).unwrap_or_default();
        match digits {
            [] => return Err(StrictJsonError::Syntax),
            [b'0', _, ..] => return Err(StrictJsonError::Syntax),
            [b'0'] if negative => return Err(StrictJsonError::Number),
            _ => {}
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err(StrictJsonError::Number);
        }
        let magnitude = digits.iter().try_fold(0u64, |total, &digit| {
            total
                .checked_mul(10)
                .and_then(|total| total.checked_add(u64::from(digit - b'0')))
                .filter(|&total| total <= MAX_SAFE_INTEGER)
        });
        magnitude.map(drop).ok_or(StrictJsonError::Number)
    }

    /// A string, returned decoded so names compare after escape decoding.
    fn string(&mut self) -> Result<String, StrictJsonError> {
        self.consume(b'"')?;
        let mut decoded = String::new();
        loop {
            let start = self.at;
            while matches!(self.peek(), Some(byte) if byte != b'"' && byte != b'\\' && byte >= 0x20)
            {
                self.at += 1;
            }
            let run = self.bytes.get(start..self.at).unwrap_or_default();
            // The text is UTF-8 and the run stops only at ASCII bytes.
            decoded.push_str(std::str::from_utf8(run).map_err(|_| StrictJsonError::InvalidUtf8)?);
            match self.next()? {
                b'"' => return Ok(decoded),
                b'\\' => decoded.push(self.escape()?),
                _ => return Err(StrictJsonError::Syntax),
            }
        }
    }

    fn escape(&mut self) -> Result<char, StrictJsonError> {
        Ok(match self.next()? {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode_escape(),
            _ => return Err(StrictJsonError::Syntax),
        })
    }

    /// The character of a `\uXXXX` escape, joining a high surrogate with the
    /// `\uXXXX` low surrogate that must follow it.
    fn unicode_escape(&mut self) -> Result<char, StrictJsonError> {
        let unit = self.hex4()?;
        let code = match unit {
            0xD800..=0xDBFF => {
                if self.peek() != Some(b'\\') || self.bytes.get(self.at + 1) != Some(&b'u') {
                    return Err(StrictJsonError::LoneSurrogate);
                }
                self.at += 2;
                let low = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return Err(StrictJsonError::LoneSurrogate);
                }
                0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
            }
            0xDC00..=0xDFFF => return Err(StrictJsonError::LoneSurrogate),
            unit => u32::from(unit),
        };
        char::from_u32(code).ok_or(StrictJsonError::LoneSurrogate)
    }

    fn hex4(&mut self) -> Result<u16, StrictJsonError> {
        (0..4).try_fold(0u16, |total, _| {
            let digit = char::from(self.next()?)
                .to_digit(16)
                .ok_or(StrictJsonError::Syntax)?;
            Ok((total << 4) | digit as u16)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(text: &str) -> StrictJsonError {
        check(text.as_bytes()).expect_err(text)
    }

    #[test]
    fn accepts_rfc_8259_values_with_any_whitespace() {
        for text in [
            "{}",
            "[]",
            " \t\r\n{\"a\" : [1, -2, 0, true, false, null, \"x\"]}\r\n",
            "{\"a\":{\"b\":[{}]},\"c\":\"\\u00e9\\ud83d\\ude00\\\"\\/\"}",
            "9007199254740991",
            "-9007199254740991",
            "\"pro\\u0066ile\"",
        ] {
            assert_eq!(check(text.as_bytes()), Ok(()), "{text}");
        }
    }

    #[test]
    fn refuses_duplicate_names_at_any_depth() {
        assert_eq!(refused(r#"{"a":1,"a":1}"#), StrictJsonError::DuplicateName);
        assert_eq!(
            refused(r#"{"m":{"p":1,"q":2,"p":3}}"#),
            StrictJsonError::DuplicateName
        );
        assert_eq!(
            refused(r#"{"m":[{"x":1},{"y":2,"y":2}]}"#),
            StrictJsonError::DuplicateName
        );
        assert_eq!(
            refused(r#"{"profile":"a","pro\u0066ile":"b"}"#),
            StrictJsonError::DuplicateName
        );
        // The same name in two sibling objects is not a duplicate.
        assert_eq!(check(br#"[{"a":1},{"a":2}]"#), Ok(()));
    }

    #[test]
    fn refuses_lone_surrogates() {
        assert_eq!(refused(r#""\ud800""#), StrictJsonError::LoneSurrogate);
        assert_eq!(refused(r#""\ud800x""#), StrictJsonError::LoneSurrogate);
        assert_eq!(refused(r#""\ud800\u0041""#), StrictJsonError::LoneSurrogate);
        assert_eq!(refused(r#""\udc00""#), StrictJsonError::LoneSurrogate);
        assert_eq!(refused(r#"{"\udfff":1}"#), StrictJsonError::LoneSurrogate);
    }

    #[test]
    fn refuses_a_bom_trailing_characters_and_invalid_utf8() {
        assert_eq!(refused("\u{feff}{}"), StrictJsonError::ByteOrderMark);
        assert_eq!(refused("{} {}"), StrictJsonError::TrailingCharacters);
        assert_eq!(refused("{}x"), StrictJsonError::TrailingCharacters);
        assert_eq!(check(b"\"\xff\""), Err(StrictJsonError::InvalidUtf8));
        assert_eq!(
            check(b"\"\xed\xa0\x80\""),
            Err(StrictJsonError::InvalidUtf8)
        );
    }

    #[test]
    fn refuses_numbers_that_are_not_safe_integers() {
        for text in [
            "1.5",
            "1.0",
            "1e3",
            "1E3",
            "-0",
            "9007199254740992",
            "-9007199254740992",
        ] {
            assert_eq!(refused(text), StrictJsonError::Number, "{text}");
        }
        assert_eq!(
            refused("123456789012345678901234567890"),
            StrictJsonError::Number
        );
    }

    #[test]
    fn refuses_text_outside_the_grammar() {
        for text in [
            "",
            " ",
            "{",
            "{\"a\"}",
            "{\"a\":1,}",
            "[1,]",
            "[1 2]",
            "{a:1}",
            "{'a':1}",
            "01",
            "-",
            "+1",
            "NaN",
            "Infinity",
            "tru",
            "\"\\x\"",
            "\"\\u12\"",
            "\"a\nb\"",
            "\"\u{1}\"",
            "\"open",
            "/* c */ {}",
            "\u{a0}{}",
        ] {
            assert_eq!(refused(text), StrictJsonError::Syntax, "{text:?}");
        }
    }

    #[test]
    fn refuses_deep_nesting() {
        let deep = format!("{}{}", "[".repeat(40), "]".repeat(40));
        assert_eq!(refused(&deep), StrictJsonError::TooDeep);
    }
}
