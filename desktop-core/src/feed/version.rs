//! Release version order, shared by the GTK updater and the feed verifier:
//! `major.minor.patch[-prerelease][+build]`, a release after its own
//! prereleases, prerelease labels in Foundation's
//! `localizedStandardCompare` order.

/// `major.minor.patch[-prerelease][+build]`: the numbers and the prerelease.
pub fn version(value: &str) -> Option<([u64; 3], Option<&str>)> {
    let (value, build) = match value.trim().split_once('+') {
        Some((value, build)) => (value, Some(build)),
        None => (value.trim(), None),
    };
    if build.is_some_and(|build| build.contains('\n')) {
        return None;
    }
    let (numbers, prerelease) = match value.split_once('-') {
        Some((numbers, prerelease)) => (numbers, Some(prerelease)),
        None => (value, None),
    };
    if let Some(prerelease) = prerelease {
        let allowed = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
        if prerelease.is_empty() || !prerelease.chars().all(allowed) {
            return None;
        }
    }
    let mut parts = [0; 3];
    let mut pieces = numbers.split('.');
    for part in &mut parts {
        let piece = pieces.next()?;
        if piece.is_empty() || !piece.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *part = piece.parse().ok()?;
    }
    pieces.next().is_none().then_some((parts, prerelease))
}

/// Orders prerelease labels as Foundation's `localizedStandardCompare`
/// does: runs of digits by their value (`rc.10` after `rc.9`), everything
/// else as case-insensitive text.
fn prerelease_order(left: &str, right: &str) -> std::cmp::Ordering {
    fn runs(text: &str) -> Vec<(bool, String)> {
        let mut runs: Vec<(bool, String)> = Vec::new();
        for c in text.chars() {
            let digit = c.is_ascii_digit();
            match runs.last_mut() {
                Some((kind, run)) if *kind == digit => run.push(c),
                _ => runs.push((digit, c.to_string())),
            }
        }
        runs
    }
    let (left, right) = (runs(left), runs(right));
    for ((left_digit, left), (right_digit, right)) in left.iter().zip(&right) {
        let order = if *left_digit && *right_digit {
            let (left, right) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
            left.len().cmp(&right.len()).then_with(|| left.cmp(right))
        } else {
            left.to_lowercase().cmp(&right.to_lowercase())
        };
        if order.is_ne() {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// Whether `candidate` is a later version than `installed`. A release
/// outranks its own prereleases; anything unrecognized is never newer.
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    let (Some(new), Some(old)) = (version(candidate), version(installed)) else {
        return false;
    };
    if new.0 != old.0 {
        return new.0 > old.0;
    }
    match (new.1, old.1) {
        (Some(new), Some(old)) => prerelease_order(new, old).is_gt(),
        (new, old) => new.is_none() && old.is_some(),
    }
}
