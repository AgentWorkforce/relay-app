//! The extra Claude Code folders read beside the store's own: sibling
//! logins, folders added in Settings or the environment (a WSL one
//! included, read whenever it can be), running sessions'
//! `CLAUDE_CONFIG_DIR` and the running WSL distributions' (`wsl_capture`).
use super::*;

/// The roots of each Claude Code config folder beyond `roots.claude`: sibling
/// `.claude-<name>` logins, `AGENT_RELAY_CLAUDE_CONFIG_DIRS`, folders added
/// in Settings, running sessions' `CLAUDE_CONFIG_DIR` (see
/// `relay::claude_dirs`) and, on Windows, the running WSL distributions'
/// (`wsl`). `ai-hist` reads one Claude folder per store, so
/// each other one is swept as Claude alone into the same database: a sweep
/// only adds and updates sessions, never drops one another folder holds.
/// A folder whose `projects/` is one already read (a link) is left out.
pub fn extra_claude_roots(roots: &ProviderRoots) -> Vec<ProviderRoots> {
    let env = |key: &str| std::env::var(key).ok();
    let dirs = ClaudeDirs::from_env(&env, &roots.home, &relay_home(&env, &roots.home));
    dirs.remember_running(&SystemProcesses::default());
    extra_claude_roots_recorded(roots, &dirs.recorded())
}

pub(super) fn extra_claude_roots_recorded(
    roots: &ProviderRoots,
    recorded: &claude_dirs::Recorded,
) -> Vec<ProviderRoots> {
    let env = |key: &str| std::env::var(key).ok();
    let dirs = ClaudeDirs::from_env(&env, &roots.home, &relay_home(&env, &roots.home));
    // The desktop's own folder, even when `base_roots` replaced it.
    let default = claude_dirs::default_home(&env, &roots.home);
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut seen = std::collections::HashSet::from([real(&default.join("projects"))]);
    // A WSL folder named here is read whatever the running list says
    // (`wsl_capture::readable`), so may start its distribution.
    let named: Vec<PathBuf> = dirs
        .watched_recorded(&default, recorded)
        .into_iter()
        .skip(1)
        .filter(|dir| dir.shared)
        .map(|dir| dir.path)
        .filter(|claude| claude.join("projects").is_dir())
        .filter(|claude| seen.insert(real(&claude.join("projects"))))
        .collect();
    // The running WSL distributions' Claude folders the person still
    // shares (Windows only), found off this thread: no file system call
    // here, since each would be a round trip to the distribution. One also
    // named above is read once.
    let found: Vec<PathBuf> = wsl_capture::claude_dirs(recorded)
        .into_iter()
        .filter(|claude| {
            !named
                .iter()
                .any(|dir| wsl::is_within(claude, dir) && wsl::is_within(dir, claude))
        })
        .collect();
    named
        .into_iter()
        .chain(found)
        .map(|claude| {
            let mut extra = roots.clone();
            extra.claude = claude;
            extra
        })
        .collect()
}

/// One Claude-only sweep of each extra Claude folder into `db`, as
/// [`SessionStore::sync`] with `options` otherwise. A folder whose sweep
/// found the lock held is left to the next one. Returns the sessions the
/// sweeps changed.
pub(super) fn sweep_extra_claude(
    db: &Path,
    roots: &ProviderRoots,
    options: &SyncOptions,
) -> Result<Vec<SessionRef>> {
    sweep_extra_claude_roots(db, extra_claude_roots(roots), options)
}

pub(super) fn sweep_extra_claude_roots(
    db: &Path,
    extra: Vec<ProviderRoots>,
    options: &SyncOptions,
) -> Result<Vec<SessionRef>> {
    let mut changed = Vec::new();
    if options
        .sources
        .as_ref()
        .is_some_and(|sources| !sources.contains(&Source::Claude))
    {
        return Ok(changed);
    }
    for extra in extra {
        let mut options = options.clone().sources([Source::Claude]);
        options.progress = None;
        let tolerated = wsl_capture::tolerated(&extra);
        if !wsl_capture::readable(&extra.claude) {
            continue;
        }
        let swept = kept_with(db, extra.clone()).and_then(|store| match store.sync(options) {
            Ok(report) => Ok(report.changed),
            Err(ai_hist::Error::SyncLocked { .. }) => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        });
        match swept {
            Ok(found) => changed.extend(found),
            Err(error) if tolerated => wsl_capture::skipped(&extra.claude, &error),
            Err(error) => return Err(error),
        }
    }
    Ok(changed)
}
