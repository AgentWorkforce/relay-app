//! GitHub Copilot runs inside VS Code: it needs VS Code itself and the GitHub
//! Copilot Chat extension. Both checks report what was found or every place
//! checked, so macOS and GTK show the same detection text.
use super::*;
mod bundled;

/// Copilot uses the account-bound provider grant and native proof hooks.
/// Cloud capability negotiation still gates verified messaging per account.
pub(crate) const COPILOT_CONNECTION_READY: bool = true;

const EXTENSION_PREFIX: &str = "github.copilot-chat-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Mac,
    Linux,
    Windows,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Linux
        }
    }
}

/// Where VS Code and its extensions are looked for. Building it reads no files.
#[derive(Clone, Debug, Default)]
pub(crate) struct Search {
    installs: Vec<PathBuf>,
    command: Option<&'static str>,
    path: Vec<PathBuf>,
    extensions: PathBuf,
    snap_bundle: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Detection {
    pub vscode: Option<PathBuf>,
    pub extension: Option<PathBuf>,
    pub bundled_checked: Vec<PathBuf>,
}

impl Detection {
    pub(crate) fn ready(&self) -> bool {
        self.vscode.is_some() && self.extension.is_some()
    }
}

impl Search {
    /// `AGENT_RELAY_PROVIDER_DETECTION_ROOT`, when absolute, replaces `/` as
    /// the root of the fixed macOS and Linux install locations, the way
    /// `AGENT_RELAY_PROVIDER_DETECTION_HOME` replaces the home directory.
    pub(crate) fn new(home: &Path, env: &impl Fn(&str) -> Option<String>) -> Self {
        let path = env("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        let system = env("AGENT_RELAY_PROVIDER_DETECTION_ROOT")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| PathBuf::from("/"));
        Self::for_platform(Platform::current(), home, env, path, &system)
    }

    /// `system` is the root the fixed macOS and Linux install locations sit
    /// under; Windows locations come from the environment.
    pub(crate) fn for_platform(
        platform: Platform,
        home: &Path,
        env: &impl Fn(&str) -> Option<String>,
        path: Vec<PathBuf>,
        system: &Path,
    ) -> Self {
        let var = |key: &str| {
            env(key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute() || platform == Platform::Windows)
        };
        let (installs, command, profile) = match platform {
            Platform::Mac => (
                [system.join("Applications"), home.join("Applications")]
                    .into_iter()
                    .map(|root| root.join("Visual Studio Code.app"))
                    .collect(),
                "code",
                home.to_path_buf(),
            ),
            Platform::Linux => (
                vec![
                    system.join("usr/share/code/code"),
                    system.join("snap/bin/code"),
                ],
                "code",
                home.to_path_buf(),
            ),
            Platform::Windows => {
                let local =
                    var("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
                let mut installs = vec![local.join("Programs")];
                installs.push(
                    var("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files")),
                );
                installs.extend(var("ProgramFiles(x86)"));
                (
                    installs
                        .into_iter()
                        .map(|root| root.join("Microsoft VS Code").join("Code.exe"))
                        .collect(),
                    "code.cmd",
                    var("USERPROFILE").unwrap_or_else(|| home.to_path_buf()),
                )
            }
        };
        Self {
            installs,
            command: Some(command),
            // Relative PATH entries resolve against the app's working
            // directory, so they are dropped on every platform.
            path: path.into_iter().filter(|dir| dir.is_absolute()).collect(),
            extensions: profile.join(".vscode").join("extensions"),
            snap_bundle: (platform == Platform::Linux).then(|| {
                system.join("snap/code/current/usr/share/code/resources/app/extensions/copilot")
            }),
        }
    }

    pub(crate) fn detect(&self) -> Detection {
        let on_path = self
            .command
            .into_iter()
            .flat_map(|command| self.path.iter().map(move |dir| dir.join(command)))
            .find(|candidate| candidate.is_file() && !flatpak_export(candidate));
        let vscode = self
            .installs
            .iter()
            .find(|candidate| candidate.exists())
            .cloned()
            .or(on_path);
        let extension = fs::read_dir(&self.extensions)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(EXTENSION_PREFIX)
                    && entry.path().is_dir()
            })
            .map(|entry| entry.path())
            .max_by(|left, right| {
                extension_version(left)
                    .cmp(&extension_version(right))
                    .then_with(|| left.cmp(right))
            });
        let mut bundled_checked = Vec::new();
        let extension = extension.or_else(|| {
            let path = vscode.as_deref()?;
            bundled::candidates(path, self.snap_bundle.as_deref())
                .into_iter()
                .find(|candidate| {
                    bundled_checked.push(candidate.clone());
                    bundled::is_chat_extension(candidate)
                })
        });
        Detection {
            vscode,
            extension,
            bundled_checked,
        }
    }

    /// The text both apps show under the provider: what Copilot needs, then
    /// each check with the location found or the locations checked.
    pub(crate) fn detail(&self, detection: &Detection) -> String {
        let vscode = match &detection.vscode {
            Some(path) => format!("VS Code: detected at {}.", path.display()),
            None => {
                let mut checked: Vec<String> = self
                    .installs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                checked.extend(self.command.map(|command| format!("{command} on PATH")));
                format!("VS Code: not detected. Checked {}.", checked.join(", "))
            }
        };
        let extension = match &detection.extension {
            Some(path) => format!(
                "GitHub Copilot Chat extension: detected at {}.",
                path.display()
            ),
            None => {
                let mut checked = vec![self
                    .extensions
                    .join(format!("{EXTENSION_PREFIX}*"))
                    .display()
                    .to_string()];
                checked.extend(
                    detection
                        .bundled_checked
                        .iter()
                        .map(|path| path.display().to_string()),
                );
                format!(
                    "GitHub Copilot Chat extension: not detected. Checked {}.",
                    checked.join(", ")
                )
            }
        };
        format!("{vscode} {extension}")
    }
}

/// Flatpak's sandbox uses different profile paths. Until setup targets those
/// coherently, do not claim a host installation from its launcher/export.
fn flatpak_export(path: &Path) -> bool {
    path.components().any(|p| p.as_os_str() == "flatpak")
        || fs::canonicalize(path)
            .ok()
            .is_some_and(|p| p.file_name().is_some_and(|name| name == "flatpak"))
}

/// The numeric version after the extension prefix (`0.22.4` in
/// `github.copilot-chat-0.22.4`), up to the first part that is not a number;
/// `None` when it starts with none, so such a directory ranks below any
/// versioned one and ties fall back to path order.
fn extension_version(path: &Path) -> Option<Vec<u64>> {
    let name = path.file_name()?.to_str()?;
    let version: Vec<u64> = name
        .strip_prefix(EXTENSION_PREFIX)?
        .split(['.', '-'])
        .map_while(|part| part.parse().ok())
        .collect();
    (!version.is_empty()).then_some(version)
}

pub(crate) const REQUIREMENTS: &str = "GitHub Copilot needs VS Code with the GitHub Copilot Chat extension, signed in to GitHub inside VS Code.";

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "fixture executable").unwrap();
    }

    #[test]
    fn copilot_needs_vscode_and_the_chat_extension() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let bin = root.path().join("bin");
        let search = Search::for_platform(
            Platform::Linux,
            &home,
            &|_| None,
            vec![bin.clone()],
            root.path(),
        );
        let missing = search.detect();
        assert_eq!(
            missing,
            Detection {
                vscode: None,
                extension: None,
                bundled_checked: Vec::new(),
            }
        );
        let detail = search.detail(&missing);
        assert!(
            detail.contains("VS Code: not detected. Checked "),
            "{detail}"
        );
        assert!(
            detail.contains(
                &root
                    .path()
                    .join("usr/share/code/code")
                    .display()
                    .to_string()
            ),
            "{detail}"
        );
        assert!(detail.contains("code on PATH"), "{detail}");
        let pattern = home
            .join(".vscode")
            .join("extensions")
            .join("github.copilot-chat-*");
        assert!(
            detail.contains(&format!(
                "GitHub Copilot Chat extension: not detected. Checked {}.",
                pattern.display()
            )),
            "{detail}"
        );

        executable(&bin.join("code"));
        let older = home
            .join(".vscode")
            .join("extensions")
            .join("github.copilot-chat-0.21.0");
        let newer = home
            .join(".vscode")
            .join("extensions")
            .join("github.copilot-chat-0.22.4");
        fs::create_dir_all(&older).unwrap();
        fs::create_dir_all(&newer).unwrap();
        fs::create_dir_all(
            home.join(".vscode")
                .join("extensions")
                .join("github.copilot-1.250.0"),
        )
        .unwrap();
        let found = search.detect();
        assert!(found.ready());
        assert_eq!(found.vscode, Some(bin.join("code")));
        assert_eq!(found.extension, Some(newer.clone()));
        assert_eq!(
            search.detail(&found),
            format!(
                "VS Code: detected at {}. GitHub Copilot Chat extension: detected at {}.",
                bin.join("code").display(),
                newer.display()
            )
        );

        // VS Code alone, or a lookalike extension alone, is not enough.
        fs::remove_dir_all(home.join(".vscode").join("extensions")).unwrap();
        fs::create_dir_all(
            home.join(".vscode")
                .join("extensions")
                .join("github.copilot-1.250.0"),
        )
        .unwrap();
        assert!(!search.detect().ready());
    }

    #[test]
    fn copilot_picks_the_newest_chat_extension_by_version() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let extensions = home.join(".vscode").join("extensions");
        let search =
            Search::for_platform(Platform::Linux, &home, &|_| None, Vec::new(), root.path());
        let newest = |names: &[&str]| {
            fs::remove_dir_all(&extensions).ok();
            for name in names {
                fs::create_dir_all(extensions.join(name)).unwrap();
            }
            search.detect().extension
        };
        assert_eq!(
            newest(&["github.copilot-chat-0.9.0", "github.copilot-chat-0.22.4"]),
            Some(extensions.join("github.copilot-chat-0.22.4"))
        );
        assert_eq!(
            newest(&["github.copilot-chat-0.99", "github.copilot-chat-0.100"]),
            Some(extensions.join("github.copilot-chat-0.100"))
        );
        // A platform suffix does not hide the version; an unversioned
        // directory ranks below a versioned one, and ties keep path order.
        assert_eq!(
            newest(&[
                "github.copilot-chat-0.22.4-darwin-arm64",
                "github.copilot-chat-0.3.0",
                "github.copilot-chat-next",
            ]),
            Some(extensions.join("github.copilot-chat-0.22.4-darwin-arm64"))
        );
        assert_eq!(
            newest(&["github.copilot-chat-next", "github.copilot-chat-beta"]),
            Some(extensions.join("github.copilot-chat-next"))
        );
    }

    #[test]
    fn copilot_finds_vscode_in_its_install_directories() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let linux =
            Search::for_platform(Platform::Linux, &home, &|_| None, Vec::new(), root.path());
        executable(&root.path().join("snap/bin/code"));
        assert_eq!(
            linux.detect().vscode,
            Some(root.path().join("snap/bin/code"))
        );

        let mac = Search::for_platform(Platform::Mac, &home, &|_| None, Vec::new(), root.path());
        assert_eq!(mac.detect().vscode, None);
        let app = home.join("Applications/Visual Studio Code.app");
        fs::create_dir_all(&app).unwrap();
        assert_eq!(mac.detect().vscode, Some(app));
        assert!(mac
            .detail(&mac.detect())
            .starts_with("VS Code: detected at "));
    }

    #[test]
    fn copilot_windows_checks_per_user_and_system_installs_and_code_cmd() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("Local");
        let program_files = root.path().join("Program Files");
        let profile = root.path().join("person");
        let bin = root.path().join("VS Code bin");
        let env = |key: &str| {
            match key {
                "LOCALAPPDATA" => Some(&local),
                "ProgramFiles" => Some(&program_files),
                "USERPROFILE" => Some(&profile),
                _ => None,
            }
            .map(|path| path.display().to_string())
        };
        let search = Search::for_platform(
            Platform::Windows,
            &root.path().join("home"),
            &env,
            vec![bin.clone()],
            Path::new("/"),
        );
        let per_user = local
            .join("Programs")
            .join("Microsoft VS Code")
            .join("Code.exe");
        let system = program_files.join("Microsoft VS Code").join("Code.exe");
        let detail = search.detail(&search.detect());
        assert!(
            detail.contains(&format!(
                "Checked {}, {}, code.cmd on PATH.",
                per_user.display(),
                system.display()
            )),
            "{detail}"
        );
        let extensions = profile.join(".vscode").join("extensions");
        assert!(
            detail.contains(
                &extensions
                    .join("github.copilot-chat-*")
                    .display()
                    .to_string()
            ),
            "{detail}"
        );

        executable(&bin.join("code.cmd"));
        assert_eq!(search.detect().vscode, Some(bin.join("code.cmd")));
        executable(&system);
        assert_eq!(search.detect().vscode, Some(system));
        executable(&per_user);
        fs::create_dir_all(extensions.join("github.copilot-chat-0.22.4")).unwrap();
        let found = search.detect();
        assert_eq!(found.vscode, Some(per_user));
        assert!(found.ready());

        // Without the environment, Windows falls back to the profile's AppData
        // and the standard Program Files directory.
        let home = Path::new("/profiles/person");
        let fallback = Search::for_platform(
            Platform::Windows,
            home,
            &|_| None,
            Vec::new(),
            Path::new("/"),
        );
        let detail = fallback.detail(&fallback.detect());
        assert!(
            detail.contains(
                &home
                    .join("AppData")
                    .join("Local")
                    .join("Programs")
                    .join("Microsoft VS Code")
                    .join("Code.exe")
                    .display()
                    .to_string()
            ),
            "{detail}"
        );
        assert!(detail.contains(r"C:\Program Files"), "{detail}");
    }
    #[test]
    fn bundled_chat_extension_is_detected_on_every_platform() {
        for platform in [Platform::Mac, Platform::Linux, Platform::Windows] {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home");
            let local = home.join("AppData").join("Local");
            let search = Search::for_platform(
                platform,
                &home,
                &|key| (key == "LOCALAPPDATA").then(|| local.display().to_string()),
                Vec::new(),
                root.path(),
            );
            let install = &search.installs[0];
            let bundled = if platform == Platform::Mac {
                fs::create_dir_all(install).unwrap();
                install.join("Contents/Resources/app/extensions/copilot")
            } else {
                executable(install);
                install
                    .parent()
                    .unwrap()
                    .join("resources/app/extensions/copilot")
            };
            fs::create_dir_all(&bundled).unwrap();
            let package = bundled.join("package.json");
            fs::write(&package, r#"{"name":"copilot-chat","publisher":"Other"}"#).unwrap();
            assert!(
                !search.detect().ready(),
                "{platform:?}: unrelated bundled directory"
            );
            fs::write(
                &package,
                r#"{"name":"copilot-chat","publisher":"GitHub","version":"0.72.0"}"#,
            )
            .unwrap();
            let canonical = fs::canonicalize(&bundled).unwrap();
            assert_eq!(
                fs::canonicalize(search.detect().extension.unwrap()).unwrap(),
                canonical,
                "{platform:?}"
            );
            assert!(search.detect().ready());
            let found = search.detect();
            assert!(search
                .detail(&found)
                .contains(&found.extension.unwrap().display().to_string()));
        }
    }
    #[test]
    fn flatpak_exports_are_not_advertised_with_host_profile_paths() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let export = home.join(".local/share/flatpak/exports/bin");
        executable(&export.join("code"));
        executable(
            &root
                .path()
                .join("var/lib/flatpak/exports/bin/com.visualstudio.code"),
        );
        fs::create_dir_all(home.join(".vscode/extensions/github.copilot-chat-0.72.0")).unwrap();
        let search =
            Search::for_platform(Platform::Linux, &home, &|_| None, vec![export], root.path());
        assert!(!search.detect().ready());
        assert!(search.detect().vscode.is_none());
    }
    #[test]
    fn macos_path_cli_finds_its_app_ancestor_and_reports_every_missing_bundle() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let app = root.path().join("custom/Code.app");
        let cli = app.join("Contents/Resources/app/bin/code");
        executable(&cli);
        let search = Search::for_platform(
            Platform::Mac,
            &home,
            &|_| None,
            vec![cli.parent().unwrap().to_owned()],
            root.path(),
        );
        let missing = search.detect();
        assert!(!missing.ready());
        for candidate in &missing.bundled_checked {
            assert!(search
                .detail(&missing)
                .contains(&candidate.display().to_string()));
        }
        let bundle = app.join("Contents/Resources/app/extensions/copilot");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("package.json"),
            r#"{"publisher":"GitHub","name":"copilot-chat"}"#,
        )
        .unwrap();
        assert!(search.detect().ready());
        assert_eq!(
            fs::canonicalize(search.detect().extension.unwrap()).unwrap(),
            fs::canonicalize(bundle).unwrap()
        );
    }
    #[cfg(unix)]
    #[test]
    fn snap_launcher_uses_the_installed_snap_bundle_despite_its_canonical_binary() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let snap = root.path().join("usr/bin/snap");
        executable(&snap);
        let cli = root.path().join("snap/bin/code");
        fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&snap, &cli).unwrap();
        let bundle = root
            .path()
            .join("snap/code/current/usr/share/code/resources/app/extensions/copilot");
        let search =
            Search::for_platform(Platform::Linux, &home, &|_| None, Vec::new(), root.path());
        let missing = search.detect();
        assert!(search
            .detail(&missing)
            .contains(&bundle.display().to_string()));
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("package.json"),
            r#"{"publisher":"GitHub","name":"copilot-chat"}"#,
        )
        .unwrap();
        let found = search.detect();
        assert!(found.ready());
        assert_eq!(found.extension.as_ref(), Some(&bundle));
    }
}
