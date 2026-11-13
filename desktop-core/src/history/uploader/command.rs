use super::{install_flag, target_arguments, SharingMode, Target};

fn with(name: &[&str], target: &Target, tail: &[&str]) -> Vec<String> {
    name.iter()
        .map(|s| s.to_string())
        .chain(target_arguments(target))
        .chain(tail.iter().map(|s| s.to_string()))
        .collect()
}

pub fn installs() -> Vec<String> {
    vec!["installs".into(), "--json".into()]
}

pub fn reclaim() -> Vec<String> {
    vec!["reclaim".into(), "--json".into()]
}

/// The apps' sign-in: `mode` is the choice for a new install, and an
/// install already set up for the workspace keeps its saved choice
/// (`--keep-saved-sharing`), so signing in again reconnects it.
pub fn cloud_install(target: &Target, mode: SharingMode, acknowledge: bool) -> Vec<String> {
    let mut args = with(
        &["cloud", "install"],
        target,
        &["--json", install_flag(mode), "--keep-saved-sharing"],
    );
    if acknowledge {
        args.push("--acknowledge-uninspected-schedules".into());
    }
    args
}

pub fn simple(name: &str, target: &Target) -> Vec<String> {
    with(&[name], target, &["--json"])
}

pub fn sessions_preview(refresh: bool, limit: usize) -> Vec<String> {
    let mut args: Vec<String> = ["sessions", "preview", "--json", "--limit"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.push(limit.to_string());
    if refresh {
        args.push("--refresh".into());
    }
    args
}

pub fn sessions_list(target: &Target, limit: usize) -> Vec<String> {
    let limit = limit.to_string();
    with(
        &["sessions", "list"],
        target,
        &["--json", "--limit", &limit],
    )
}

pub fn sessions_catalog(target: &Target, limit: usize) -> Vec<String> {
    let mut args = sessions_list(target, limit);
    args.push("--catalog-only".into());
    args
}

pub fn sessions_reset(target: &Target, confirmation: &str) -> Vec<String> {
    with(
        &["sessions", "reset"],
        target,
        &["--json", "--confirmation", confirmation],
    )
}

pub fn sessions_delete(target: &Target, keys: &[String]) -> Vec<String> {
    let mut args = with(&["sessions", "delete"], target, &["--json"]);
    for key in keys {
        args.extend(["--session".to_string(), key.clone()]);
    }
    args
}

pub fn sessions_change(include: bool, target: &Target, keys: &[String]) -> Vec<String> {
    let verb = if include { "include" } else { "exclude" };
    let mut args = with(&["sessions", verb], target, &["--json"]);
    for key in keys {
        args.extend(["--session".to_string(), key.clone()]);
    }
    args
}

pub fn sharing_set(target: &Target, mode: SharingMode) -> Vec<String> {
    with(
        &["sharing", "set"],
        target,
        &["--mode", mode.as_str(), "--json"],
    )
}
