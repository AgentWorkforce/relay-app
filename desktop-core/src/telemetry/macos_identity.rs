//! Disk protocol shared with CoreOutageTelemetry.swift.
use super::{parse_uuid, IDENTITY_FILE, PREFERENCES_FILE};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

const LOCK_FILE: &str = "installation-id.lock";

fn unsafe_state() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "unsafe analytics state")
}

fn directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let info = file.metadata()?;
    if info.uid() != nix::unistd::geteuid().as_raw() || info.mode() & 0o022 != 0 {
        return Err(unsafe_state());
    }
    Ok(file)
}

pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
    let file = directory(path)?;
    // File::sync_all uses F_FULLFSYNC on macOS. Use fsync for directory entries.
    nix::unistd::fsync(file.as_raw_fd()).map_err(io::Error::from)
}

fn check_file(file: &File) -> io::Result<()> {
    let info = file.metadata()?;
    if !info.is_file()
        || info.uid() != nix::unistd::geteuid().as_raw()
        || info.mode() & 0o022 != 0
        || info.nlink() != 1
    {
        return Err(unsafe_state());
    }
    Ok(())
}

pub(super) fn lock(path: &Path) -> io::Result<File> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700).recursive(true).create(path)?;
    directory(path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path.join(LOCK_FILE))?;
    check_file(&file)?;
    if file.metadata()?.len() != 0 {
        return Err(unsafe_state());
    }
    fs2::FileExt::lock_exclusive(&file)?;
    Ok(file)
}

fn read(path: &Path, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let parent = path.parent().ok_or_else(unsafe_state)?;
    let result = directory(parent).and_then(|_| {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
    });
    let file = match result {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    check_file(&file)?;
    if file.metadata()?.len() > limit as u64 {
        return Err(unsafe_state());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(unsafe_state());
    }
    Ok(Some(bytes))
}

#[derive(serde::Deserialize)]
struct Preferences {
    #[serde(default = "default_enabled")]
    analytics_enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[cfg(target_os = "macos")]
pub(super) fn preferences_for_update(
    path: &Path,
) -> io::Result<(serde_json::Map<String, serde_json::Value>, bool)> {
    let Some(bytes) = read(path, 16 * 1024)? else {
        return Ok((serde_json::Map::new(), true));
    };
    let consent = parse_consent(&bytes).unwrap_or(false);
    let preferences = serde_json::from_slice(&bytes).unwrap_or_default();
    Ok((preferences, consent))
}

pub(super) fn consent(path: &Path) -> io::Result<bool> {
    let Some(bytes) = read(path, 16 * 1024)? else {
        return Ok(true);
    };
    parse_consent(&bytes)
}

fn parse_consent(bytes: &[u8]) -> io::Result<bool> {
    // Require a JSON object. Serde structs can otherwise accept arrays.
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(unsafe_state());
    }
    Ok(serde_json::from_slice::<Preferences>(bytes)?.analytics_enabled)
}

fn existing(path: &Path) -> io::Result<Option<String>> {
    read(path, 64)?
        .map(|bytes| {
            let text = std::str::from_utf8(&bytes).map_err(|_| unsafe_state())?;
            parse_uuid(text.trim_matches(|c: char| c.is_ascii_whitespace()))
                .ok_or_else(unsafe_state)
        })
        .transpose()
}

pub(super) fn identity(directory_path: &Path) -> io::Result<Option<String>> {
    let _lock = lock(directory_path)?;
    if !consent(&directory_path.join(PREFERENCES_FILE))? {
        return Ok(None);
    }
    let path = directory_path.join(IDENTITY_FILE);
    if let Some(id) = existing(&path)? {
        return Ok(Some(id));
    }
    let mut bytes = [0; 16];
    getrandom::getrandom(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    let id = super::format_uuid(bytes);
    let mut staged = tempfile::NamedTempFile::new_in(directory_path)?;
    writeln!(staged, "{id}")?;
    staged.as_file().sync_all()?;
    match staged.persist_noclobber(&path) {
        Ok(_) => sync_directory(directory_path)?,
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => drop(error),
        Err(error) => return Err(error.error),
    }
    existing(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_first_writers_reuse_one_durable_id() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("first-launch");
        let ready = Arc::new(Barrier::new(12));
        let ids = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..12)
                .map(|_| {
                    let directory = &directory;
                    let ready = ready.clone();
                    scope.spawn(move || {
                        ready.wait();
                        identity(directory).unwrap().unwrap()
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        let saved = existing(&directory.join(IDENTITY_FILE)).unwrap().unwrap();
        assert!(ids.iter().all(|id| id == &saved));
        assert_eq!(
            fs::read_to_string(directory.join(IDENTITY_FILE)).unwrap(),
            format!("{saved}\n")
        );
    }

    #[test]
    fn invalid_preferences_and_unsafe_ids_cannot_create_an_identity() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let preferences = directory.join(PREFERENCES_FILE);
        let id = directory.join(IDENTITY_FILE);
        for bytes in [
            r#"{"analytics_enabled":false}"#,
            r#"{"analytics_enabled":"true"}"#,
            r#"{"analytics_enabled":null}"#,
            r#"{"analytics_enabled":true,"analytics_enabled":false}"#,
            r#"[]"#,
            "{",
        ] {
            fs::write(&preferences, bytes).unwrap();
            assert!(identity(directory).ok().flatten().is_none());
            assert!(!id.exists());
        }
        fs::remove_file(&preferences).unwrap();
        symlink("missing", &preferences).unwrap();
        assert!(identity(directory).is_err());
        assert!(!id.exists());
        fs::remove_file(&preferences).unwrap();

        fs::write(&id, "malformed").unwrap();
        assert!(identity(directory).is_err());
        assert_eq!(fs::read_to_string(&id).unwrap(), "malformed");
        fs::remove_file(&id).unwrap();
        symlink("missing", &id).unwrap();
        assert!(identity(directory).is_err());
        assert!(id.symlink_metadata().unwrap().is_symlink());
        fs::remove_file(&id).unwrap();
        fs::create_dir(&id).unwrap();
        assert!(identity(directory).is_err());
        fs::remove_dir(&id).unwrap();
        fs::write(&id, b"ABCDEF01-2345-4678-9ABC-DEF012345678\n").unwrap();
        fs::set_permissions(&id, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(identity(directory).is_err());
        fs::set_permissions(&id, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            identity(directory).unwrap().as_deref(),
            Some("abcdef01-2345-4678-9abc-def012345678")
        );
    }

    #[test]
    fn opt_out_and_rotation_prevent_reusing_the_previous_id() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let first = identity(directory).unwrap().unwrap();
        let preferences = directory.join(PREFERENCES_FILE);
        let id = directory.join(IDENTITY_FILE);
        {
            let _lock = lock(directory).unwrap();
            fs::write(&preferences, r#"{"analytics_enabled":false}"#).unwrap();
            fs::remove_file(&id).unwrap();
        }
        assert_eq!(identity(directory).unwrap(), None);
        assert!(!id.exists());
        fs::write(&preferences, r#"{"analytics_enabled":true}"#).unwrap();
        assert_ne!(identity(directory).unwrap().unwrap(), first);
    }
}
