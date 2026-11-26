//! Running the constant PowerShell scripts and reading what they print.
//!
//! Every script is a constant from [`super::scripts`], handed to Windows
//! PowerShell 5.1 as `-EncodedCommand` (Base64 of its UTF-16LE text), so no
//! value is ever spliced into script source. Values travel as one JSON
//! document on stdin, which the script reads with `[Console]::In`
//! (UNVERIFIED: `-EncodedCommand` leaves redirected stdin unread).
//!
//! Output decoding is defensive: the scripts set a UTF-8 output encoding,
//! but a UTF-8 BOM, or UTF-16LE (with or without its BOM, as `wsl.exe` writes
//! its own messages), are both accepted.
use std::time::Duration;

use super::config::WindowsConfig;
use crate::system::{Output, Run, Secret, System};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard Base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let sym = |i: u32| char::from(*ALPHABET.get((i & 63) as usize).unwrap_or(&b'A'));
    for chunk in bytes.chunks(3) {
        let b = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        out.push(sym(n >> 18));
        out.push(sym(n >> 12));
        out.push(if chunk.len() > 1 { sym(n >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(n) } else { '=' });
    }
    out
}

/// `-EncodedCommand`'s argument: Base64 of the UTF-16LE script.
pub fn encode_script(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

/// The arguments before the encoded script, in order.
pub const FLAGS: [&str; 5] = [
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-EncodedCommand",
];

/// The argv of `powershell.exe` for `script`.
pub fn args(script: &str) -> Vec<String> {
    let mut args: Vec<String> = FLAGS.iter().map(|f| (*f).to_owned()).collect();
    args.push(encode_script(script));
    args
}

/// One run of `script`, with `input` (if any) as JSON on stdin.
pub fn run(
    cfg: &WindowsConfig,
    script: &str,
    input: Option<&serde_json::Value>,
    timeout: Duration,
) -> Run {
    let mut run = Run::with_args(&cfg.powershell(), args(script))
        .timeout(timeout)
        .max_output(32 * 1024 * 1024);
    if let Some(input) = input {
        run = run.stdin(Secret::new(input.to_string().into_bytes()));
    }
    run
}

/// Text from a helper's stdout: UTF-8 (BOM stripped) or UTF-16LE (BOM or
/// the ASCII-in-UTF-16 shape). `None` for anything undecodable.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec()).ok();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16le(rest);
    }
    let looks_utf16 = bytes.len() >= 2
        && bytes.len().is_multiple_of(2)
        && bytes.get(1) == Some(&0)
        && bytes.first().is_some_and(|b| *b != 0);
    if looks_utf16 {
        return utf16le(bytes);
    }
    String::from_utf8(bytes.to_vec()).ok()
}

fn utf16le(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16(&units).ok()
}

/// Run `script` and parse its stdout as JSON of type `T`. Any failure,
/// non-zero exit or unparsable output is `None`.
pub fn json<S: System, T: serde::de::DeserializeOwned>(sys: &S, run: &Run) -> Option<T> {
    let output: Output = sys.run(run).ok().filter(Output::success)?;
    let text = decode_text(&output.stdout)?;
    serde_json::from_str(text.trim()).ok()
}
