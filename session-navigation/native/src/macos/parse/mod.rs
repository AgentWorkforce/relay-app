//! Pure parsers for macOS command output (`sysctl`, `osascript`, the JXA
//! app registry) and the `ps` process table. Each returns `None` for
//! anything malformed, oversized or hostile; none of them panics.
pub mod ps;

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::common::parse::trim_ascii;

/// `sysctl -n kern.boottime`: `{ sec = 1794291866, usec = 486968 } Mon Nov …`.
pub fn boot_time(bytes: &[u8]) -> Option<u64> {
    if bytes.len() > 256 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let rest = text.trim_start().strip_prefix("{ sec = ")?;
    let end = rest.find(',')?;
    let digits = rest.get(..end)?;
    if digits.is_empty() || digits.len() > 12 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sec: u64 = digits.parse().ok()?;
    rest.get(end..)?.starts_with(", usec = ").then_some(sec)
}

/// The AppleScript error number at the end of `osascript`'s stderr:
/// `… execution error: Not authorized to send Apple events to Ghostty. (-1743)`.
pub fn osascript_error(stderr: &[u8]) -> Option<i32> {
    let text = std::str::from_utf8(stderr.get(..stderr.len().min(64 * 1024))?).ok()?;
    let trimmed = text.trim_end();
    let inner = trimmed.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let digits = inner.get(open + 1..)?;
    let body = digits.strip_prefix('-').unwrap_or(digits);
    if body.is_empty() || body.len() > 6 || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AppEntry {
    pub path: String,
    pub version: Option<String>,
    /// The bundle's main executable (`NSBundle.executablePath`), from which
    /// a running instance is told in the kernel's process table.
    #[serde(default)]
    pub executable: Option<String>,
}

/// The default handler of `public.unix-executable` (what "Make iTerm2
/// Default Term" sets): `bundle` is `None` when no handler is registered or
/// it has no bundle id.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TerminalHandler {
    #[serde(default)]
    pub bundle: Option<String>,
}

/// What the JXA registry query reports.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AppsReport {
    pub schemes: BTreeMap<String, Option<String>>,
    pub apps: BTreeMap<String, Option<AppEntry>>,
    pub front: Option<String>,
    pub running: Vec<String>,
    /// `None` (or absent) when the handler could not be read.
    #[serde(default)]
    pub terminal: Option<TerminalHandler>,
}

/// The JXA output, which must answer for exactly the bundles asked about.
pub fn apps_report(bytes: &[u8], asked: &[&str]) -> Option<AppsReport> {
    if bytes.len() > 256 * 1024 {
        return None;
    }
    let report: AppsReport = serde_json::from_slice(trim_ascii(bytes)).ok()?;
    let keys: Vec<&str> = report.apps.keys().map(String::as_str).collect();
    let mut wanted: Vec<&str> = asked.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    (keys == wanted).then_some(report)
}
