//! Whether Claude's desktop app (Claude.app) is installed here, so a missing
//! Claude Code login can say that signing in to the desktop app alone is
//! not enough for cloud sessions.
//!
//! The desktop app's sign-in is never read. Nothing here opens the app's
//! `config.json`, token cache, cookies or Keychain items, or anything else
//! in its data folder that holds its sign-in: only whether the app itself
//! is installed, by the existence of its bundle (macOS) or install folder
//! (Windows). Claude Code's own sign-in (`claude auth login`, with the
//! Claude Code the desktop app includes or one installed separately) is
//! what cloud sessions use; whether the desktop app is signed in does not
//! change what the person has to do.
use std::path::{Path, PathBuf};

/// The desktop app's data folder on macOS for the person whose home is
/// `home`: `~/Library/Application Support/Claude`. Only its `claude-code`
/// folder, the copies of Claude Code the app keeps for its Code tab, is
/// ever looked at ([`crate::claude_sign_in::desktop_app_copy`]).
pub fn app_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Claude")
}

/// Where the desktop app is installed when it is, for the person whose home
/// is `home`: `/Applications/Claude.app` or `~/Applications/Claude.app` on
/// macOS, `%LOCALAPPDATA%\AnthropicClaude` on Windows. Linux has no build
/// of the desktop app, so nothing is listed there. Tests look only under
/// `home` (no system `/Applications`, no `%LOCALAPPDATA%`), so they never
/// see this computer's app.
pub fn install_locations(home: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut locations = vec![home.join("Applications/Claude.app")];
        if !cfg!(test) {
            locations.insert(0, PathBuf::from("/Applications/Claude.app"));
        }
        locations
    }
    #[cfg(windows)]
    {
        vec![std::env::var_os("LOCALAPPDATA")
            .filter(|dir| !dir.is_empty() && !cfg!(test))
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("AnthropicClaude")]
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = home;
        Vec::new()
    }
}

/// Whether Claude's desktop app is installed for the person whose home is
/// `home`. Only checks that its bundle or install folder exists.
pub fn installed(home: &Path) -> bool {
    install_locations(home)
        .iter()
        .any(|location| std::fs::metadata(location).is_ok_and(|meta| meta.is_dir()))
}

/// Makes this platform's desktop app install under `home`, as a test sees
/// it; `false` on Linux, which has none.
#[cfg(test)]
pub(crate) fn install_for_test(home: &Path) -> bool {
    match install_locations(home).pop() {
        Some(location) => {
            std::fs::create_dir_all(location).unwrap();
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_app_is_found_by_its_bundle_alone() {
        let home = tempfile::tempdir().unwrap();
        assert!(!installed(home.path()));
        // The app's data folder, sign-in or not, is not an install.
        std::fs::create_dir_all(app_dir(home.path())).unwrap();
        std::fs::write(app_dir(home.path()).join("config.json"), "{}").unwrap();
        assert!(!installed(home.path()));
        std::fs::create_dir_all(home.path().join("Applications/Claude.app/Contents")).unwrap();
        assert!(installed(home.path()));
        assert_eq!(
            app_dir(Path::new("/Users/me")),
            Path::new("/Users/me/Library/Application Support/Claude")
        );
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn linux_has_no_desktop_app() {
        let home = tempfile::tempdir().unwrap();
        assert!(install_locations(home.path()).is_empty());
        assert!(!installed(home.path()));
    }
}
