//! A short, owner-scoped Dev profile keeps Codex's native control socket usable.
use super::*;

#[derive(Clone)]
pub(super) struct CodexScope {
    home: PathBuf,
    pub profile: PathBuf,
}
impl CodexScope {
    pub fn new(home: &Path, app_data: &Path) -> Self {
        let scope = format!(
            "{:x}",
            Sha256::digest(app_data.to_string_lossy().as_bytes())
        );
        Self {
            home: home.into(),
            profile: home
                .join(".agent-relay-dev")
                .join(&scope[..12])
                .join("codex"),
        }
    }

    pub fn from_profile(profile: &Path, app_data: &Path) -> Option<Self> {
        let home = profile.ancestors().nth(3)?;
        let expected = Self::new(home, app_data);
        (expected.profile == profile).then_some(expected)
    }

    pub fn valid(&self) -> bool {
        safe_directory(&self.home, &self.profile)
            && ["auth.json", "config.toml"].iter().all(|name| {
                match fs::symlink_metadata(self.profile.join(name)) {
                    Ok(meta) => {
                        meta.is_file()
                            && !meta.file_type().is_symlink()
                            && private_fs::owned_by_current_user(&meta)
                    }
                    Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                }
            })
    }

    pub fn ensure(&self) -> bool {
        if !self.valid() {
            return false;
        }
        let Some(scope) = self.profile.parent() else {
            return false;
        };
        let Some(namespace) = scope.parent() else {
            return false;
        };
        for directory in [namespace, scope, &self.profile] {
            if fs::create_dir(directory)
                .is_err_and(|error| error.kind() != std::io::ErrorKind::AlreadyExists)
                || !safe_directory(&self.home, directory)
                || private_fs::set_mode(directory, 0o700).is_err()
            {
                return false;
            }
        }
        self.valid()
    }
}
