use super::*;
use agent_relay_node::relay::harness::opencode::{parse_version, system_version, VersionCache};

#[derive(Default)]
pub(super) struct Versions {
    opencode: VersionCache,
    devin: VersionCache,
}
impl Versions {
    pub(super) fn opencode(&self, executables: &BTreeMap<String, PathBuf>) -> Option<u64> {
        let version = self
            .opencode
            .get(executables.get("opencode")?, &system_version)?;
        parse_version(&version).map(|version| version.0)
    }
    pub(super) fn devin(&self, executables: &BTreeMap<String, PathBuf>) -> Option<bool> {
        let version = self.devin.get(executables.get("devin")?, &system_version)?;
        let version = version.split_whitespace().find(|word| {
            word.trim_start_matches('v')
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })?;
        let mut parts = version
            .trim_start_matches('v')
            .split('.')
            .map(str::parse::<u64>);
        let major = parts.next()?.ok()?;
        let minor = parts.next()?.ok()?;
        Some((major, minor) >= (3000, 3))
    }
}

const IDS: &[&str] = &[
    "grok", "opencode", "devin", "muse", "pi", "openclaw", "hermes", "droid",
];

/// On Windows these commands are only looked up on PATH: their installers
/// have no fixed per-user directory there, so nothing else is guessed.
const WINDOWS_PATH_ONLY: &[&str] = &["droid"];

#[derive(Default)]
pub(super) struct Search {
    pub(super) directories: Vec<PathBuf>,
    path_directories: Vec<PathBuf>,
    overrides: BTreeMap<String, PathBuf>,
    provider_directories: BTreeMap<String, Vec<PathBuf>>,
}

impl Search {
    pub(super) fn new(home: &Path, env: &impl Fn(&str) -> Option<String>) -> Self {
        Self::with_applications(home, env, Path::new("/Applications"))
    }

    fn with_applications(
        home: &Path,
        env: &impl Fn(&str) -> Option<String>,
        applications: &Path,
    ) -> Self {
        let mut directories = vec![
            home.join(".local/bin"),
            home.join(".bun/bin"),
            home.join(".opencode/bin"),
            home.join(".grok/bin"),
        ];
        #[cfg(unix)]
        directories.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
        let path_directories: Vec<PathBuf> = env("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .filter(|path| path.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        directories.extend(path_directories.iter().cloned());
        let overrides = IDS
            .iter()
            .filter_map(|id| {
                let key = format!("RELAY_DESKTOP_{}", id.to_uppercase());
                env(&key)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| (id.to_string(), PathBuf::from(value)))
            })
            .collect();
        let mut provider_directories = BTreeMap::new();
        if let Some(grok_home) = Locations::from_env(home, env).grok.parent() {
            provider_directories.insert("grok".into(), vec![grok_home.join("bin")]);
        }
        if cfg!(target_os = "macos") {
            provider_directories.insert(
                "devin".into(),
                [applications.to_path_buf(), home.join("Applications")]
                    .iter()
                    .map(|root| {
                        root.join("Devin.app/Contents/Resources/app/extensions/windsurf/devin/bin")
                    })
                    .collect(),
            );
        }
        Self {
            directories,
            path_directories,
            overrides,
            provider_directories,
        }
    }

    pub(super) fn resolve(&self, id: &str) -> Option<PathBuf> {
        use agent_relay_node::platform::exe;
        if let Some(path) = self.overrides.get(id) {
            return path.is_absolute().then(|| exe::runnable(path)).flatten();
        }
        if cfg!(windows) && WINDOWS_PATH_ONLY.contains(&id) {
            return exe::find_in(&self.path_directories, id);
        }
        self.provider_directories
            .get(id)
            .and_then(|directories| exe::find_in(directories, id))
            .or_else(|| exe::find_in(&self.directories, id))
    }

    pub(super) fn all(&self) -> BTreeMap<String, PathBuf> {
        IDS.iter()
            .filter_map(|id| self.resolve(id).map(|path| (id.to_string(), path)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_detection_finds_custom_grok_home_and_bundled_devin() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let grok_home = home.join("custom-grok");
        let applications = root.path().join("Applications");
        let bin = grok_home.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let command = bin.join(if cfg!(windows) { "grok.exe" } else { "grok" });
        fs::write(&command, "fixture executable").unwrap();
        agent_relay_node::platform::fs::set_mode(&command, 0o700).unwrap();
        for value in [
            grok_home.to_string_lossy().into_owned(),
            " ~/custom-grok ".into(),
        ] {
            let env = |key: &str| (key == "GROK_HOME").then(|| value.clone());
            let search = Search::with_applications(&home, &env, &applications);
            assert_eq!(search.resolve("grok").as_deref(), Some(command.as_path()));
        }
        let search = Search::with_applications(&home, &|_| None, &applications);
        if cfg!(target_os = "macos") {
            let command = applications
                .join("Devin.app/Contents/Resources/app/extensions/windsurf/devin/bin/devin");
            fs::create_dir_all(command.parent().unwrap()).unwrap();
            fs::write(&command, "fixture executable").unwrap();
            agent_relay_node::platform::fs::set_mode(&command, 0o700).unwrap();
            assert_eq!(search.resolve("devin").as_deref(), Some(command.as_path()));
            fs::remove_file(&command).unwrap();
            let personal = home.join(
                "Applications/Devin.app/Contents/Resources/app/extensions/windsurf/devin/bin/devin",
            );
            fs::create_dir_all(personal.parent().unwrap()).unwrap();
            fs::write(&personal, "fixture executable").unwrap();
            agent_relay_node::platform::fs::set_mode(&personal, 0o700).unwrap();
            assert_eq!(search.resolve("devin").as_deref(), Some(personal.as_path()));
        }
    }

    #[test]
    fn droid_resolves_from_user_bin_on_unix_and_only_from_path_on_windows() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let applications = root.path().join("Applications");
        let name = if cfg!(windows) { "droid.exe" } else { "droid" };
        let local = home.join(".local/bin").join(name);
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::write(&local, "fixture executable").unwrap();
        agent_relay_node::platform::fs::set_mode(&local, 0o700).unwrap();
        let search = Search::with_applications(&home, &|_| None, &applications);
        if cfg!(windows) {
            // No guessed per-user path: absent from PATH means not installed.
            assert_eq!(search.resolve("droid"), None);
        } else {
            assert_eq!(search.resolve("droid").as_deref(), Some(local.as_path()));
        }
        let on_path = root.path().join("path-bin");
        fs::create_dir_all(&on_path).unwrap();
        let command = on_path.join(name);
        fs::write(&command, "fixture executable").unwrap();
        agent_relay_node::platform::fs::set_mode(&command, 0o700).unwrap();
        fs::remove_file(&local).unwrap();
        let path = on_path.to_string_lossy().into_owned();
        let env = |key: &str| (key == "PATH").then(|| path.clone());
        let search = Search::with_applications(&home, &env, &applications);
        assert_eq!(search.resolve("droid").as_deref(), Some(command.as_path()));
        assert_eq!(
            search.all().get("droid").map(PathBuf::as_path),
            Some(command.as_path())
        );
    }
}
