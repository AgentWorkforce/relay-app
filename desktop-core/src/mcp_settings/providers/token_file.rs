//! Grok rereads bearer_token_file before every HTTP request and skips OAuth.
//! The file is inside app-owned state, separate from the global native config.
use super::*;

fn read(path: &Path, private: bool) -> Result<Option<String>> {
    safe_parent(path)?;
    match fs::symlink_metadata(path) {
        Ok(meta)
            if meta.is_file()
                && private_fs::owned_by_current_user(&meta)
                && private_fs::private_to_owner(&meta, if private { 0o077 } else { 0o022 })
                && meta.len() <= 16_384 =>
        {
            fs::read_to_string(path)
                .map(Some)
                .map_err(|_| "Cannot read provider token file".into())
        }
        Ok(_) => Err("Provider token file must be a private owned regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Cannot inspect provider token file".into()),
    }
}

fn token(config: &HostedConfig) -> Result<&str> {
    let token = config
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| value.strip_prefix("Bearer "))
        .ok_or("Provider authorization is unavailable")?;
    if token.is_empty() || token.len() > 16_384 || token.chars().any(char::is_whitespace) {
        return Err("Invalid provider authorization".into());
    }
    Ok(token)
}

pub(crate) fn install(config: &HostedConfig) -> Result<()> {
    let path = config
        .bearer_token_file
        .as_ref()
        .filter(|path| path.is_absolute())
        .ok_or("Provider token file must be absolute")?;
    let token = token(config)?;
    let previous = read(path, false)?;
    if previous.as_deref() == Some(token) && ready(config).unwrap_or(false) {
        return Ok(());
    }
    files::update(path, |current| {
        (current == previous.as_deref().unwrap_or("")).then(|| token.to_string())
    })
    .map_err(|_| "Cannot write provider token file")?;
    if !ready(config)? {
        return Err("Provider token file changed during installation; retry".into());
    }
    Ok(())
}

pub(crate) fn ready(config: &HostedConfig) -> Result<bool> {
    let path = config
        .bearer_token_file
        .as_ref()
        .ok_or("Provider token file is unavailable")?;
    Ok(read(path, true)?.as_deref() == Some(token(config)?))
}

pub(crate) fn remove(path: &Path) -> Result<()> {
    if let Some(previous) = read(path, false)? {
        if read(path, false)?.as_ref() != Some(&previous) {
            return Err("Provider token file changed during removal; retry".into());
        }
        fs::remove_file(path).map_err(|_| "Cannot remove provider token file")?;
    }
    Ok(())
}
