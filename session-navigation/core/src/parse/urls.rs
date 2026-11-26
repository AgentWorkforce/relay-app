//! Provider URL builders. Each validates its id first, then percent-encodes it
//! anyway, so a builder can never emit an unescaped or invalid id.
use super::ids;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidId;

/// RFC 3986 unreserved characters pass; everything else is `%XX`.
pub fn encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0x0f));
        }
    }
    out
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        _ => char::from(b'A' + (nibble & 0x0f).saturating_sub(10)),
    }
}

/// `claude://code/continue?session=<local id>`: selects a NON-archived desktop
/// session. On an archived or unknown id the app silently shows Code home
/// (Claude facts B).
pub fn claude_continue(local_id: &str) -> Result<String, InvalidId> {
    if !ids::is_claude_local_id(local_id) {
        return Err(InvalidId);
    }
    Ok(format!(
        "claude://code/continue?session={}",
        encode_component(local_id)
    ))
}

/// `claude://resume?session=<cli uuid>`: unarchives the owning desktop session
/// or imports the transcript as a new one (Claude facts B).
pub fn claude_resume(cli_id: &str) -> Result<String, InvalidId> {
    if !ids::is_claude_cli_id(cli_id) {
        return Err(InvalidId);
    }
    Ok(format!(
        "claude://resume?session={}",
        encode_component(cli_id)
    ))
}

/// `codex://threads/<uuid>`. The `prompt` parameter is never used (Codex
/// facts B8: it pre-fills the composer).
pub fn codex_thread(thread_id: &str) -> Result<String, InvalidId> {
    if !ids::is_codex_thread_id(thread_id) {
        return Err(InvalidId);
    }
    Ok(format!("codex://threads/{}", encode_component(thread_id)))
}
