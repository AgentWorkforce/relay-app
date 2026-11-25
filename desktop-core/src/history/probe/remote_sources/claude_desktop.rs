//! Whether Claude's desktop app (Claude.app) is signed in here, so a
//! missing Claude Code login can say what to do on a Mac (or Windows PC)
//! that has only the desktop app.
//!
//! The desktop app's sign-in is never used. It keeps its claude.ai OAuth
//! token in `config.json`, encrypted with Electron's safe storage, whose key
//! is the app's own Keychain item (`Claude Safe Storage`) on macOS. Reading
//! that key would raise macOS's "allow access" prompt for another app's
//! item and, once allowed, hand Agent Relay the key to everything the
//! desktop app encrypts (its claude.ai cookies included), for a private
//! format Anthropic does not offer to other apps. Refreshing that token
//! would also rotate the desktop app's own login. So only the presence of
//! the app's token cache is read here: never its value, never the Keychain.
//! Claude Code's own sign-in (`claude auth login`, with the Claude Code the
//! desktop app includes or one installed separately) is what cloud sessions
//! use.
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The desktop app's token cache entries in its `config.json`, newest first.
const TOKEN_CACHE_KEYS: [&str; 2] = ["oauth:tokenCacheV2", "oauth:tokenCache"];

/// A `config.json` larger than this is not the desktop app's settings file.
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

/// The desktop app's data folder for the person whose home is `home`:
/// `~/Library/Application Support/Claude` on macOS, `%APPDATA%\Claude` on
/// Windows, and `$XDG_CONFIG_HOME/Claude` (else `~/.config/Claude`) where
/// Linux builds of the app keep it.
pub fn app_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Claude")
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"))
            .join("Claude")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("Claude")
    }
}

/// Whether the desktop app under `app_dir` holds a sign-in: its
/// `config.json` carries a token cache. Reads only whether the entry is
/// there; the (encrypted) value is never kept, decoded or logged.
pub fn signed_in(app_dir: &Path) -> bool {
    let path = app_dir.join("config.json");
    if std::fs::metadata(&path).map_or(true, |meta| meta.len() > MAX_CONFIG_BYTES) {
        return false;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(Value::Object(config)) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    TOKEN_CACHE_KEYS.iter().any(|key| {
        config
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|cache| !cache.trim().is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(config: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), config).unwrap();
        dir
    }

    #[test]
    fn a_token_cache_is_a_sign_in() {
        let app = app_with(r#"{"locale":"en-US","oauth:tokenCacheV2":"djEwAAAA"}"#);
        assert!(signed_in(app.path()));
        let legacy = app_with(r#"{"oauth:tokenCache":"djEwAAAA"}"#);
        assert!(signed_in(legacy.path()));
    }

    #[test]
    fn no_token_cache_is_no_sign_in() {
        let signed_out = app_with(r#"{"locale":"en-US","lastKnownAccountUuid":"x"}"#);
        assert!(!signed_in(signed_out.path()));
        let emptied = app_with(r#"{"oauth:tokenCacheV2":""}"#);
        assert!(!signed_in(emptied.path()));
        let not_json = app_with("not json");
        assert!(!signed_in(not_json.path()));
        let absent = tempfile::tempdir().unwrap();
        assert!(!signed_in(absent.path()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_app_keeps_its_data_in_application_support() {
        assert_eq!(
            app_dir(Path::new("/Users/me")),
            Path::new("/Users/me/Library/Application Support/Claude")
        );
    }
}
