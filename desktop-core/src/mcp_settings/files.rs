use super::*;
use std::io::{self, Write};

pub(super) fn update(path: &Path, render: impl FnOnce(&str) -> Option<String>) -> io::Result<()> {
    let original = match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.is_file()
                || !private_fs::owned_by_current_user(&meta)
                || !private_fs::private_to_owner(&meta, 0o022)
            {
                return Ok(());
            }
            Some(fs::read_to_string(path)?)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let Some(replacement) = render(original.as_deref().unwrap_or("")) else {
        return Ok(());
    };
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("No config directory"))?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    private_fs::set_mode(temporary.path(), 0o600)?;
    temporary.write_all(replacement.as_bytes())?;
    temporary.as_file().sync_all()?;
    // Agent CLIs can update these files while the desktop runs. A changed
    // snapshot is retried rather than overwriting the newer settings.
    if fs::read_to_string(path).ok() != original {
        return Ok(());
    }
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
