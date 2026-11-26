//! Pure parsers from provider bytes to evidence fragments. Malformed,
//! oversized or hostile input yields an error, never a guess and never a panic.
pub mod claude;
pub mod codex;
pub mod grok;
pub mod ids;
pub mod lstart;
pub mod opencode;
pub mod urls;

use serde::de::DeserializeOwned;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    TooLarge,
    Empty,
    Json,
    MissingField(&'static str),
    InvalidId(&'static str),
    InvalidValue(&'static str),
}

/// Parse the first JSON value in `bytes`, ignoring a garbage suffix (Claude
/// rewrites registry files in place, facts F). Rejects inputs over `limit`.
pub(crate) fn first_json<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, ParseError> {
    if bytes.len() > limit {
        return Err(ParseError::TooLarge);
    }
    let mut values = serde_json::Deserializer::from_slice(bytes).into_iter::<T>();
    match values.next() {
        Some(Ok(value)) => Ok(value),
        Some(Err(_)) => Err(ParseError::Json),
        None => Err(ParseError::Empty),
    }
}

/// Parse exactly one JSON value.
pub(crate) fn whole_json<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, ParseError> {
    if bytes.len() > limit {
        return Err(ParseError::TooLarge);
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(ParseError::Empty);
    }
    serde_json::from_slice(bytes).map_err(|_| ParseError::Json)
}

/// An absolute POSIX path without any control character.
pub fn is_absolute_path(path: &str) -> bool {
    path.starts_with('/') && !path.chars().any(char::is_control) && path.len() <= 4096
}

/// An absolute path on either kind of system: provider records are read on
/// the platform that wrote them; `decide` checks the right kind for its
/// runtime.
pub fn is_any_absolute_path(path: &str) -> bool {
    is_absolute_path(path) || is_windows_absolute_path(path)
}

/// An absolute Windows drive path (`C:\\…`) without any control character,
/// `"`, `<`, `>`, `|`, `?` or `*`. UNC and device paths are refused.
pub fn is_windows_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes.len() <= 4096
        && bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(1) == Some(&b':')
        && bytes.get(2) == Some(&b'\\')
        && !path.chars().any(char::is_control)
        && !path.contains(['"', '<', '>', '|', '?', '*'])
}
