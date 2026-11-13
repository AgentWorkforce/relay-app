//! Capture over the running WSL distributions' homes ([`super::wsl`]).
//!
//! A home's Claude folder is one more extra Claude folder, under the person's
//! sharing choice for it; every other provider's history under the home is
//! read as that provider's, laid out as on Linux. These folders are polled
//! (sync passes, discovery, the retained reconciliation and inventory
//! rounds), never watched, and only while their distribution is still
//! listed as running: a stopped distribution's share is never touched, so
//! never booted. A WSL folder the person named is the exception: it is read
//! whenever it can be ([`readable`]). A WSL root's error is logged and
//! skipped, never failing the pass for the other roots.
use super::*;
use agent_relay_node::relay::claude_dirs::Recorded;

/// The running distributions' Claude folders the person has not stopped
/// sharing (`recorded`'s choice for each). No file system call: the folders
/// were found off this thread.
pub(super) fn claude_dirs(recorded: &Recorded) -> Vec<PathBuf> {
    wsl::claude_dirs()
        .into_iter()
        .filter(|claude| recorded.choice(claude).shared)
        .collect()
}

/// Whether `extra` is a WSL distribution's folder: one that can vanish when
/// the distribution stops, so its errors are logged and skipped.
pub(super) fn tolerated(extra: &ProviderRoots) -> bool {
    wsl::is_wsl_path(&extra.claude)
}

/// Whether `root` (a folder, or a file in one) may be read now. A WSL root
/// found automatically may while its distribution is still listed as
/// running. One inside a WSL folder the person named
/// (`AGENT_RELAY_CLAUDE_CONFIG_DIRS`, or added in Settings) always may, in
/// the preview and in the collector, whatever the running list and
/// `AGENT_RELAY_WSL_HISTORY` (which governs automatic discovery only) say:
/// reading it is what the person asked for, so touching an explicit
/// `\\wsl.localhost\<distro>` path may start its distribution. Any other
/// root always may.
pub(super) fn readable(root: &Path) -> bool {
    readable_with(root, wsl::still_running, named_dirs)
}

/// [`readable`] over `running` (the running-list check) and `named` (the
/// person's WSL folders, asked only when the list does not answer).
fn readable_with(
    root: &Path,
    running: impl Fn(&Path) -> bool,
    named: impl FnOnce() -> Vec<PathBuf>,
) -> bool {
    !wsl::is_wsl_path(root) || running(root) || named().iter().any(|dir| wsl::is_within(root, dir))
}

/// The WSL folders the person named, as they are configured now.
fn named_dirs() -> Vec<PathBuf> {
    let env = |key: &str| std::env::var(key).ok();
    let Some(home) = user_home() else {
        return Vec::new();
    };
    named_in(&ClaudeDirs::from_env(&env, &home, &relay_home(&env, &home)))
}

/// The WSL folders among `dirs`' `AGENT_RELAY_CLAUDE_CONFIG_DIRS` and the
/// folders added in Settings: the ones the person named, not ones found.
pub(super) fn named_in(dirs: &ClaudeDirs) -> Vec<PathBuf> {
    dirs.environment
        .iter()
        .cloned()
        .chain(dirs.recorded().added)
        .filter(|dir| wsl::is_wsl_path(dir))
        .collect()
}

/// The sources a WSL home is read for: each of `wanted` (every one when
/// `None`) but Claude, whose WSL folders are extra Claude folders already.
pub(super) fn sources(wanted: Option<&Vec<Source>>) -> Vec<Source> {
    Source::ALL
        .iter()
        .copied()
        .filter(|source| *source != Source::Claude)
        .filter(|source| wanted.is_none_or(|wanted| wanted.contains(source)))
        .collect()
}

/// Every provider's roots under a WSL `home`, laid out as on Linux.
pub(super) fn roots(home: &Path) -> ProviderRoots {
    let opencode = home.join(".local/share/opencode/opencode.db");
    ProviderRoots::from_home(home.to_path_buf(), opencode)
}

/// One sweep of each running WSL home into `db`, for every provider but
/// Claude, as [`SessionStore::sync`] with `options` otherwise. Returns the
/// sessions the sweeps changed.
pub(super) fn sweep_homes(db: &Path, options: &SyncOptions) -> Vec<SessionRef> {
    let sources = sources(options.sources.as_ref());
    let mut changed = Vec::new();
    if sources.is_empty() {
        return changed;
    }
    for home in wsl::homes() {
        let mut options = options.clone().sources(sources.clone());
        options.progress = None;
        let swept = kept_with(db, roots(&home.home)).and_then(|store| match store.sync(options) {
            Ok(report) => Ok(report.changed),
            Err(ai_hist::Error::SyncLocked { .. }) => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        });
        match swept {
            Ok(found) => changed.extend(found),
            Err(error) => skipped(&home.home, &error),
        }
    }
    changed
}

/// One discovery of each running WSL home into `db`, added to `report`.
pub(super) fn discover_homes(
    db: &Path,
    options: &DiscoveryOptions,
    report: &mut ai_hist::DiscoveryReport,
) {
    let sources = sources(options.sources.as_ref());
    if sources.is_empty() {
        return;
    }
    for home in wsl::homes() {
        let discovered = open_with(db, roots(&home.home))
            .and_then(|store| Ok(store.discover(options.clone().sources(sources.clone()))?));
        match discovered {
            Ok(found) => {
                report.discovered += found.discovered;
                report.skipped_unchanged += found.skipped_unchanged;
                report.diagnostics.extend(found.diagnostics);
            }
            Err(error) => skipped(&home.home, &error),
        }
    }
}

/// The reconciliation rounds' roots for the running WSL homes: each home's
/// path (the round's key) with its roots and sources.
pub(super) fn round_roots() -> Vec<(PathBuf, ProviderRoots, Vec<Source>)> {
    let sources = sources(None);
    wsl::homes()
        .into_iter()
        .map(|home| (home.home.clone(), roots(&home.home), sources.clone()))
        .collect()
}

/// Add a retained round for each running WSL root not yet in `keys`: each
/// WSL Claude folder in `claude` (Claude only) and each running home (every
/// other provider). A root that cannot be opened is logged and skipped.
pub(super) fn add_rounds(
    store: &SessionStore,
    claude: Vec<ProviderRoots>,
    keys: &mut std::collections::HashSet<Option<PathBuf>>,
    added: &mut Vec<incremental::Reconciliation>,
) {
    let db = store.db_path();
    let claude = claude.into_iter().map(|roots| {
        (
            roots.claude.clone(),
            Some(roots.claude.clone()),
            roots,
            vec![Source::Claude],
        )
    });
    let homes = round_roots()
        .into_iter()
        .map(|(home, roots, sources)| (home, None, roots, sources));
    for (root, extra_claude, roots, sources) in claude.chain(homes) {
        if !keys.insert(Some(root.clone())) {
            continue;
        }
        let opened = open_with(db, roots).and_then(|store| Ok(store.reconcile()?.sources(sources)));
        match opened {
            Ok(inner) => added.push(incremental::Reconciliation {
                db: db.to_owned(),
                extra_claude,
                wsl: Some(root),
                inner,
            }),
            Err(error) => skipped(&root, &error),
        }
    }
}

/// A finished step for a WSL round whose distribution stopped: it is not
/// read, so the share is not touched; `None` for any other round.
pub(super) fn stopped_round(
    round: &incremental::Reconciliation,
) -> Option<ai_hist::ReconciliationStep> {
    let root = round.wsl.as_deref()?;
    (!readable(root)).then(finished)
}

/// A WSL round's error, logged and turned into a finished step so it never
/// fails the pass; any other round's result unchanged.
pub(super) fn tolerate_step(
    round: &incremental::Reconciliation,
    step: Result<ai_hist::ReconciliationStep>,
) -> Result<ai_hist::ReconciliationStep> {
    match (step, round.wsl.as_deref()) {
        (Err(error), Some(root)) => {
            skipped(root, &error);
            Ok(finished())
        }
        (step, _) => step,
    }
}

fn finished() -> ai_hist::ReconciliationStep {
    let mut step = ai_hist::ReconciliationStep::default();
    step.complete = true;
    step
}

/// Log a WSL root's error before it is skipped.
pub(super) fn skipped(root: &Path, error: &anyhow::Error) {
    eprintln!("wsl history root {} skipped: {error:#}", root.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WSL home's sweep and the Windows home's sweep into one database
    /// each only add: neither drops the Codex session the other holds.
    #[test]
    fn sweeping_a_wsl_home_keeps_the_windows_homes_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let codex = |home: &Path, id: &str| {
            let day = home.join(".codex/sessions/2026/11/12");
            std::fs::create_dir_all(&day).unwrap();
            let at = chrono::Utc::now().to_rfc3339();
            std::fs::write(
                day.join(format!("rollout-{id}.jsonl")),
                format!(
                    "{}\n{}\n",
                    serde_json::json!({"timestamp": at, "type": "session_meta",
                        "payload": {"id": id, "cwd": "/home/ana/project"}}),
                    serde_json::json!({"timestamp": at, "type": "event_msg",
                        "payload": {"type": "user_message", "message": "synthetic"}}),
                ),
            )
            .unwrap();
        };
        let windows = dir.path().join("windows");
        let wsl = dir.path().join("wsl/home/ana");
        codex(&windows, "on-windows");
        codex(&wsl, "in-wsl");
        let db = dir.path().join("history.db");
        let sync = |roots: ProviderRoots| {
            open_with(&db, roots)
                .unwrap()
                .sync(SyncOptions::default().sources([Source::Codex]))
                .unwrap();
        };
        let windows_roots = || ProviderRoots::from_home(windows.clone(), windows.join("none.db"));
        sync(windows_roots());
        sync(roots(&wsl));
        sync(windows_roots());
        let store = open_with(&db, windows_roots()).unwrap();
        let mut ids: Vec<String> = store
            .sessions(CatalogQuery::default())
            .map(|row| row.unwrap().session_id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["in-wsl", "on-windows"]);
    }

    #[test]
    fn a_wsl_home_is_read_for_every_wanted_source_but_claude() {
        let all = sources(None);
        assert!(!all.contains(&Source::Claude));
        assert!(all.contains(&Source::Codex) && all.contains(&Source::Copilot));
        assert_eq!(all.len(), Source::ALL.len() - 1);
        assert_eq!(
            sources(Some(&vec![Source::Claude, Source::Codex])),
            [Source::Codex]
        );
        assert!(sources(Some(&vec![Source::Claude])).is_empty());
        let roots = roots(Path::new("/share/home/ana"));
        assert_eq!(roots.codex, Path::new("/share/home/ana/.codex"));
        assert_eq!(roots.claude, Path::new("/share/home/ana/.claude"));
    }

    /// A WSL folder the person named is read with its distribution not
    /// listed as running, as in the preview, and with
    /// `AGENT_RELAY_WSL_HISTORY=0`, which leaves the running list empty; a
    /// found one is not.
    #[test]
    fn a_named_wsl_folder_is_read_whatever_the_running_list() {
        let named = r"\\wsl.localhost\Ubuntu\home\x\.claude";
        let not_running = |_: &Path| false;
        for path in [
            named.to_owned(),
            format!(r"{named}\projects\p\s.jsonl"),
            r"\\wsl$\Ubuntu\home\x\.claude\projects".to_owned(),
        ] {
            assert!(
                readable_with(Path::new(&path), not_running, || vec![named.into()]),
                "{path}"
            );
        }
        let found = Path::new(r"\\wsl.localhost\Ubuntu\home\ana\.claude");
        assert!(!readable_with(found, not_running, || vec![named.into()]));
        assert!(readable_with(found, |_| true, Vec::new));
        // A folder off the share never asks for the named folders.
        let asked = std::cell::Cell::new(false);
        let named_asked = || {
            asked.set(true);
            Vec::new()
        };
        assert!(readable_with(
            Path::new("/home/ana/.claude"),
            not_running,
            named_asked
        ));
        assert!(!asked.get());
    }

    /// The named folders are the environment's and the Settings ones on the
    /// share; a local one needs no exception.
    #[test]
    fn the_named_folders_come_from_the_environment_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("claude-config-dirs.json");
        let added = r"\\wsl.localhost\Debian\root\.claude";
        std::fs::write(
            &file,
            serde_json::json!({"added": [added, "/home/ana/.claude-work"]}).to_string(),
        )
        .unwrap();
        let dirs = ClaudeDirs {
            environment: vec![
                PathBuf::from(r"\\wsl.localhost\Ubuntu\home\x\.claude"),
                PathBuf::from("/home/ana/.claude-other"),
            ],
            file: Some(file),
            private_root: None,
        };
        assert_eq!(
            named_in(&dirs),
            [
                PathBuf::from(r"\\wsl.localhost\Ubuntu\home\x\.claude"),
                PathBuf::from(added),
            ]
        );
    }

    #[test]
    fn a_folder_off_the_share_is_always_readable() {
        assert!(readable(Path::new("/home/ana/.claude")));
        assert!(readable(Path::new(r"C:\Users\ana\.codex")));
        // Off Windows no distribution is ever listed as running.
        if !cfg!(windows) {
            assert!(!readable(Path::new(
                r"\\wsl.localhost\Ubuntu\home\ana\.claude"
            )));
        }
    }
}
