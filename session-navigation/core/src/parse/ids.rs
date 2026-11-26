//! Session id validators. Every id that reaches a URL or argv passed one of
//! these first.
use crate::model::{Provider, SessionRef};

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn is_lower_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// `8-4-4-4-12` hex with hyphens at the right places.
pub fn is_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => is_hex(b),
        })
}

/// A Claude CLI session id: a UUID (the desktop's `rV` strict regex, facts B).
pub fn is_claude_cli_id(s: &str) -> bool {
    is_uuid(s)
}

/// A Claude desktop id: `^local_[A-Za-z0-9-]{1,64}$` (`cGr`, facts B).
pub fn is_claude_local_id(s: &str) -> bool {
    match s.strip_prefix("local_") {
        Some(rest) => {
            !rest.is_empty()
                && rest.len() <= 64
                && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }
        None => false,
    }
}

/// A Codex thread id: lowercase UUIDv7 (Codex facts A1).
pub fn is_codex_thread_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    is_uuid(s)
        && bytes.get(14) == Some(&b'7')
        && bytes.iter().all(|&b| b == b'-' || is_lower_hex(b))
}

/// Lowercase a Codex id if it is a valid one in any case.
pub fn normalize_codex_thread_id(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    is_codex_thread_id(&lower).then_some(lower)
}

/// An OpenCode session id: `ses_` + 12 lowercase hex + 14 base62 (facts B).
pub fn is_opencode_session_id(s: &str) -> bool {
    match s.strip_prefix("ses_") {
        Some(rest) => {
            let bytes = rest.as_bytes();
            bytes.len() == 26
                && bytes.iter().enumerate().all(|(i, &b)| {
                    if i < 12 {
                        is_lower_hex(b)
                    } else {
                        b.is_ascii_alphanumeric()
                    }
                })
        }
        None => false,
    }
}

/// A Grok session id: a UUID (relay `store.rs` `is_uuid`), compared in
/// lower case.
pub fn is_grok_session_id(s: &str) -> bool {
    is_uuid(s)
}

pub fn is_valid_session(session: &SessionRef) -> bool {
    match session.provider {
        Provider::Grok => is_grok_session_id(&session.id),
        Provider::Claude => is_claude_cli_id(&session.id),
        Provider::Codex => is_codex_thread_id(&session.id),
        Provider::OpenCode => is_opencode_session_id(&session.id),
    }
}
