//! The app registry on the fast path (a system with kernel reads):
//!
//! - What cannot change within one click — installed apps, their versions
//!   and executables, scheme handlers, the default terminal — comes from the
//!   JXA registry query and is reused for at most [`REUSE`] inside one
//!   observer.
//! - What is evidence of now is read every look: the frontmost app with
//!   `lsappinfo` (two runs, ~5 ms each, against ~60 ms for the JXA query;
//!   both ask LaunchServices), and the running apps from the kernel's
//!   process table: an installed app runs when a process of the person
//!   executes its bundle executable.
//!
//! Anything this cannot read falls back to a fresh JXA query, whose `front`
//! and `running` are then used as on the tool path.
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use super::config::MacConfig;
use super::machine;
use super::parse::AppsReport;
use crate::common::{par, probe};
use crate::system::{Run, System};

pub const LSAPPINFO: &str = "/usr/bin/lsappinfo";

/// How long the slow-changing registry facts are reused.
pub const REUSE: Duration = Duration::from_secs(2);

/// The last registry report and when it was read, with each installed
/// app's executable canonicalized.
#[derive(Clone, Debug, Default)]
pub struct RegistryCache {
    entry: Option<(Instant, AppsReport, BTreeMap<String, String>)>,
    pub reuse: Option<Duration>,
}

impl RegistryCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn window(&self) -> Duration {
        self.reuse.unwrap_or(REUSE)
    }
}

/// `lsappinfo front`: `ASN:0x0-0x7c47c4:`.
pub fn front_asn(stdout: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?.trim();
    let body = text.strip_prefix("ASN:0x")?.strip_suffix(':')?;
    let (high, low) = body.split_once("-0x")?;
    let hex = |s: &str| !s.is_empty() && s.len() <= 16 && s.bytes().all(|b| b.is_ascii_hexdigit());
    (hex(high) && hex(low)).then(|| text.to_owned())
}

/// `lsappinfo info -only bundleID <asn>`: the app must still be the front
/// one (`<asn> (in front)`); `bundleID="x"` → `Some(x)`, `bundleID=[ NULL ]`
/// → `None` (a front app without a bundle id). Anything else: unreadable.
pub fn front_bundle(stdout: &[u8], asn: &str) -> Option<Option<String>> {
    if stdout.len() > 64 * 1024 {
        return None;
    }
    let text = std::str::from_utf8(stdout).ok()?;
    if !text.contains(&format!("{asn} (in front)")) {
        return None;
    }
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("bundleID="))?;
    let value = line.strip_prefix("bundleID=")?.trim();
    if value == "[ NULL ]" {
        return Some(None);
    }
    let id = value.strip_prefix('"')?.strip_suffix('"')?;
    let ok = !id.is_empty()
        && id.len() <= 255
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    ok.then(|| Some(id.to_owned()))
}

pub fn front_run(cfg: &MacConfig) -> Run {
    Run::new(LSAPPINFO, &["front"]).timeout(cfg.common.timeouts.probe)
}

pub fn front_info_run(cfg: &MacConfig, asn: &str) -> Run {
    Run::new(LSAPPINFO, &["info", "-only", "bundleID", asn]).timeout(cfg.common.timeouts.probe)
}

/// The frontmost app's bundle id (`Some(None)`: a front app without one),
/// `None` when `lsappinfo` could not say.
pub fn front<S: System>(sys: &S, cfg: &MacConfig) -> Option<Option<String>> {
    let asn = front_asn(&probe(sys, &front_run(cfg))?.stdout)?;
    front_bundle(&probe(sys, &front_info_run(cfg, &asn))?.stdout, &asn)
}

/// What one look at the registry gives.
pub struct Look {
    pub report: Option<AppsReport>,
    /// Each installed app's executable, canonical (empty for a report
    /// without them).
    pub executables: BTreeMap<String, String>,
    /// The report was read by this look (its `front` and `running` are
    /// fresh).
    pub fresh: bool,
}

fn executables<S: System>(sys: &S, report: &AppsReport) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (bundle, entry) in &report.apps {
        let Some(entry) = entry else { continue };
        let exe = entry.executable.as_deref()?;
        if !session_navigation::parse::is_absolute_path(exe) {
            return None;
        }
        out.insert(bundle.clone(), sys.canonicalize(exe).ok()?);
    }
    Some(out)
}

/// The registry report: reused when younger than the reuse window, else
/// queried; with the front app read fresh by `lsappinfo` (the report's own
/// when that fails, querying afresh if it was a reused one).
pub fn look<S: System + Sync>(sys: &S, cfg: &MacConfig, cache: &mut RegistryCache) -> Look {
    let window = cache.window();
    let reused = cache
        .entry
        .as_ref()
        .filter(|(at, _, _)| at.elapsed() < window)
        .map(|(_, report, exes)| (report.clone(), exes.clone()));
    // Nothing to reuse: ask the registry while `lsappinfo` answers.
    let (lsappinfo, queried) = match reused {
        Some(_) => (front(sys, cfg), None),
        None => {
            let (front, report) = par::join(sys, || front(sys, cfg), || machine::apps(sys, cfg));
            (front, Some(report))
        }
    };
    let (mut report, executables, fresh) = match (reused, &lsappinfo) {
        (Some((report, exes)), Some(_)) => (Some(report), exes, false),
        _ => {
            let report = queried.unwrap_or_else(|| machine::apps(sys, cfg));
            let exes = report.as_ref().and_then(|r| executables(sys, r));
            cache.entry = match (&report, &exes) {
                (Some(r), Some(e)) => Some((Instant::now(), r.clone(), e.clone())),
                _ => None,
            };
            (report, exes.unwrap_or_default(), true)
        }
    };
    if let (Some(report), Some(front)) = (report.as_mut(), lsappinfo) {
        report.front = front;
    }
    Look {
        report,
        executables,
        fresh,
    }
}

/// The installed apps that run as `uid`: their canonical executable is in
/// the kernel's table. `None` when that cannot be told (an installed app
/// without a known executable, or no kernel executables at all).
pub fn running(
    executables: &BTreeMap<String, String>,
    report: &AppsReport,
    table: &BTreeSet<(u32, String)>,
    uid: u32,
) -> Option<Vec<String>> {
    if table.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (bundle, entry) in &report.apps {
        if entry.is_none() {
            continue;
        }
        let exe = executables.get(bundle)?;
        if table.contains(&(uid, exe.clone())) {
            out.push(bundle.clone());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsappinfo_front_and_bundle() {
        let asn = front_asn(b"ASN:0x0-0x7c47c4:\n");
        assert_eq!(asn.as_deref(), Some("ASN:0x0-0x7c47c4:"));
        assert_eq!(front_asn(b"[ NULL ]\n"), None);
        assert_eq!(front_asn(b"ASN:0x0-0xzz:"), None);
        let info = b"[ NULL ]  ASN:0x0-0x7c47c4: (in front) \n    bundleID=\"com.google.Chrome\"\n    bundle path=[ NULL ] \n";
        assert_eq!(
            front_bundle(info, "ASN:0x0-0x7c47c4:"),
            Some(Some("com.google.Chrome".to_owned()))
        );
        // No longer in front, another app, or a bundle-less app.
        let moved = b"[ NULL ]  ASN:0x0-0x7c47c4: \n    bundleID=\"com.google.Chrome\"\n";
        assert_eq!(front_bundle(moved, "ASN:0x0-0x7c47c4:"), None);
        assert_eq!(front_bundle(info, "ASN:0x0-0x1:"), None);
        let none = b"x ASN:0x0-0x5: (in front) \n    bundleID=[ NULL ] \n";
        assert_eq!(front_bundle(none, "ASN:0x0-0x5:"), Some(None));
        let hostile = b"ASN:0x0-0x5: (in front) \n    bundleID=\"a b\"\n";
        assert_eq!(front_bundle(hostile, "ASN:0x0-0x5:"), None);
    }
}
