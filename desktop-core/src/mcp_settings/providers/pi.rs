//! Pi registers the canonical hosted server from native session context, so a
//! global mcp.json entry must not shadow the extension's session-bound headers.
use super::{
    files, json_document, private_fs, recognized, HostedConfig, Inspection, Provider, Result,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SOURCE: &str = include_str!("extensions/pi-agent-relay.mjs");
const OWNER: &str = "agent-relay-provider-pi";

fn extension(profile: &Path) -> PathBuf {
    profile.join("extensions").join("agent-relay-mcp.js")
}

fn rendered(credential_path: &Path) -> Result<String> {
    let path = credential_path
        .to_str()
        .ok_or("Pi credential path is not UTF-8")?;
    if !credential_path.is_absolute() {
        return Err("Pi credential path must be absolute".into());
    }
    Ok(SOURCE.replace(
        "__RELAY_CONFIG_PATH__",
        &serde_json::to_string(path).map_err(|_| "Invalid Pi credential path")?,
    ))
}

fn read(path: &Path, private: bool) -> Result<Option<String>> {
    safe_parent(path)?;
    match fs::symlink_metadata(path) {
        Ok(meta)
            if meta.is_file()
                && private_fs::owned_by_current_user(&meta)
                && private_fs::private_to_owner(&meta, if private { 0o077 } else { 0o022 }) =>
        {
            if meta.len() > 1024 * 1024 {
                return Err("Pi configuration is too large".into());
            }
            fs::read_to_string(path)
                .map(Some)
                .map_err(|_| "Cannot read Pi configuration".into())
        }
        Ok(_) => Err("Pi configuration must be a private owned regular file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Cannot inspect Pi configuration".into()),
    }
}

fn safe_parent(path: &Path) -> Result<()> {
    // Reject links anywhere in the install tree. A symlinked profile must not
    // redirect a credential write or an uninstall to another application's files.
    let mut current = path.parent();
    while let Some(parent) = current {
        // macOS exposes its root-owned /private directories through these
        // system aliases; they are outside the user-controlled install tree.
        #[cfg(target_os = "macos")]
        if matches!(parent.to_str(), Some("/var" | "/tmp" | "/etc")) {
            break;
        }
        match fs::symlink_metadata(parent) {
            Ok(meta) if !meta.is_dir() => {
                return Err("Pi configuration directory must not be a symlink".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Cannot inspect Pi configuration directory".into()),
        }
        current = parent.parent();
    }
    Ok(())
}

fn credential(value: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(value).map_err(|_| "Malformed Pi provider credential")?;
    if value["managed_by"] != OWNER || !value["url"].is_string() || !value["headers"].is_object() {
        return Err("A different file occupies the Pi provider credential path".into());
    }
    Ok(value)
}

fn write(path: &Path, previous: Option<&str>, next: &str) -> Result<()> {
    safe_parent(path)?;
    if previous == Some(next) && read(path, true)?.as_deref() == Some(next) {
        return Ok(());
    }
    files::update(path, |current| {
        (current == previous.unwrap_or("")).then(|| next.to_string())
    })
    .map_err(|_| "Cannot write Pi provider configuration")?;
    if read(path, true)?.as_deref() != Some(next) {
        return Err("Pi configuration changed while installing; retry installation".into());
    }
    Ok(())
}

fn check_extension(path: &Path, expected: &str) -> Result<Option<String>> {
    let previous = read(path, false)?;
    // The marker and exact credential-path binding identify our installation
    // across source upgrades. Never claim a similarly named user extension or
    // an extension belonging to another app profile.
    let owned = |text: &str| {
        text.lines().next() == Some("// AGENT_RELAY_HOSTED_PI_MCP_V1")
            && expected
                .lines()
                .find(|line| line.starts_with("const configPath = "))
                .is_some_and(|binding| {
                    text.lines()
                        .filter(|line| line.starts_with("const configPath = "))
                        .eq(std::iter::once(binding))
                })
            && text
                .lines()
                .any(|line| line == "const serverName = 'agent-relay';")
    };
    if previous
        .as_ref()
        .is_some_and(|text| text != expected && !owned(text))
    {
        return Err("A different extension already uses the agent-relay-mcp name".into());
    }
    Ok(previous)
}

fn clean_native(profile: &Path, endpoint: Option<&str>, installing: bool) -> Result<()> {
    let path = profile.join("mcp.json");
    let previous = read(&path, false)?;
    let Some(previous) = previous else {
        return Ok(());
    };
    let servers = json_document::servers(Provider::Devin, &previous)?;
    if installing
        && servers
            .get("agent-relay")
            .is_some_and(|entry| !recognized::json(entry, endpoint, None))
    {
        return Err("A different Pi MCP server already uses the agent-relay name".into());
    }
    let next = json_document::render(Provider::Devin, &previous, None, endpoint, None)?;
    if next != previous {
        write(&path, Some(&previous), &next)?;
    }
    Ok(())
}

pub(crate) fn install(profile: &Path, credential_path: &Path, hosted: &HostedConfig) -> Result<()> {
    if !hosted.valid() {
        return Err("Invalid hosted MCP authentication configuration".into());
    }
    let source = rendered(credential_path)?;
    let path = extension(profile);
    let old_source = check_extension(&path, &source)?;
    let old_credential = read(credential_path, true)?;
    if let Some(old) = &old_credential {
        credential(old)?;
    }
    let new_credential = serde_json::to_string_pretty(&json!({
        "managed_by":OWNER, "url":hosted.endpoint, "headers":hosted.headers,
    }))
    .map_err(|_| "Cannot encode Pi provider credential")?;
    clean_native(profile, Some(&hosted.endpoint), true)?;
    write(credential_path, old_credential.as_deref(), &new_credential)?;
    write(&path, old_source.as_deref(), &source)?;
    if !inspect(profile, credential_path, Some(hosted))?.configured {
        return Err("Pi configuration changed while installing; retry installation".into());
    }
    Ok(())
}

pub(crate) fn inspect(
    profile: &Path,
    credential_path: &Path,
    expected: Option<&HostedConfig>,
) -> Result<Inspection> {
    let source = rendered(credential_path)?;
    let installed = check_extension(&extension(profile), &source)?;
    let present = installed.is_some();
    let value = read(credential_path, true)?
        .map(|text| credential(&text))
        .transpose()?;
    let native = read(&profile.join("mcp.json"), false)?.unwrap_or_default();
    let servers = json_document::servers(Provider::Devin, &native)?;
    let endpoint = expected.map(|h| h.endpoint.as_str());
    let shadowed = servers.contains_key("agent-relay")
        || servers
            .values()
            .any(|entry| recognized::json(entry, endpoint, None));
    let configured = installed.as_deref() == Some(source.as_str())
        && !shadowed
        && expected.is_some_and(|expected| {
            value.as_ref().is_some_and(|value| {
                value["url"] == expected.endpoint && value["headers"] == json!(expected.headers)
            })
        });
    Ok(Inspection {
        present,
        configured,
    })
}

pub(crate) fn remove(profile: &Path, credential_path: &Path, endpoint: Option<&str>) -> Result<()> {
    let path = extension(profile);
    let source = rendered(credential_path)?;
    let old_source = check_extension(&path, &source)?;
    let old_credential = read(credential_path, true)?;
    if let Some(old) = &old_credential {
        credential(old)?;
    }
    clean_native(profile, endpoint, false)?;
    // Verify snapshots again rather than deleting a concurrently replaced file.
    for (path, previous) in [
        (path.as_path(), old_source.as_deref()),
        (credential_path, old_credential.as_deref()),
    ] {
        if let Some(previous) = previous {
            if read(path, true)?.as_deref() != Some(previous) {
                return Err("Pi configuration changed while removing; retry removal".into());
            }
            fs::remove_file(path).map_err(|_| "Cannot remove Pi provider configuration")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn hosted() -> HostedConfig {
        HostedConfig {
            bearer_token_file: None,
            copilot_stdio: None,
            endpoint: "https://agentrelay.com/cloud/api/v1/mcp/shared-sessions".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer synthetic-grant".into())]),
        }
    }
    #[test]
    fn lifecycle_preserves_other_servers_and_never_duplicates_native_registration() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("pi");
        let credentials = temp.path().join("private/pi.json");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("mcp.json"), format!(r#"{{"mcpServers":{{"old-relay":{{"url":"{}"}},"other":{{"url":"https://example.test/mcp"}}}}}}"#,hosted().endpoint)).unwrap();
        install(&profile, &credentials, &hosted()).unwrap();
        assert!(
            inspect(&profile, &credentials, Some(&hosted()))
                .unwrap()
                .configured
        );
        let native = fs::read_to_string(profile.join("mcp.json")).unwrap();
        assert!(!native.contains("old-relay"));
        assert!(native.contains("other"));
        assert!(!fs::read_to_string(extension(&profile))
            .unwrap()
            .contains("synthetic-grant"));
        let mut rotated = hosted();
        rotated
            .headers
            .insert("Authorization".into(), "Bearer second-grant".into());
        install(&profile, &credentials, &rotated).unwrap();
        assert!(
            inspect(&profile, &credentials, Some(&rotated))
                .unwrap()
                .configured
        );
        remove(&profile, &credentials, Some(&hosted().endpoint)).unwrap();
        assert!(!credentials.exists());
        assert!(!extension(&profile).exists());
        assert!(fs::read_to_string(profile.join("mcp.json"))
            .unwrap()
            .contains("other"));
    }
    #[test]
    fn custom_extension_and_native_name_conflicts_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("pi");
        let credentials = temp.path().join("credentials.json");
        fs::create_dir_all(profile.join("extensions")).unwrap();
        fs::write(extension(&profile), "// user's extension").unwrap();
        assert!(install(&profile, &credentials, &hosted()).is_err());
        assert_eq!(
            fs::read_to_string(extension(&profile)).unwrap(),
            "// user's extension"
        );
        assert!(!credentials.exists());
        fs::remove_file(extension(&profile)).unwrap();
        fs::write(
            profile.join("mcp.json"),
            r#"{"mcpServers":{"agent-relay":{"url":"https://custom.test"}}}"#,
        )
        .unwrap();
        assert!(install(&profile, &credentials, &hosted()).is_err());
        assert!(!credentials.exists());
    }

    #[test]
    fn owned_extension_upgrades_once_without_rewriting_unchanged_files() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("pi");
        let credentials = temp.path().join("credentials.json");
        install(&profile, &credentials, &hosted()).unwrap();
        let current = rendered(&credentials).unwrap();
        let old = current.replace(
            "// This is a Pi extension",
            "// Previous version: this is a Pi extension",
        );
        fs::write(extension(&profile), old).unwrap();
        assert!(
            !inspect(&profile, &credentials, Some(&hosted()))
                .unwrap()
                .configured
        );
        install(&profile, &credentials, &hosted()).unwrap();
        assert_eq!(fs::read_to_string(extension(&profile)).unwrap(), current);
        let source_time = fs::metadata(extension(&profile))
            .unwrap()
            .modified()
            .unwrap();
        let credential_time = fs::metadata(&credentials).unwrap().modified().unwrap();
        install(&profile, &credentials, &hosted()).unwrap();
        assert_eq!(
            fs::metadata(extension(&profile))
                .unwrap()
                .modified()
                .unwrap(),
            source_time
        );
        assert_eq!(
            fs::metadata(&credentials).unwrap().modified().unwrap(),
            credential_time
        );

        let other_binding = current.replace("credentials.json", "another-app.json");
        fs::write(extension(&profile), &other_binding).unwrap();
        assert!(install(&profile, &credentials, &hosted()).is_err());
        assert_eq!(
            fs::read_to_string(extension(&profile)).unwrap(),
            other_binding
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_install_directories_do_not_receive_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("other-app");
        fs::create_dir(&actual).unwrap();
        let profile = temp.path().join("pi");
        std::os::unix::fs::symlink(&actual, &profile).unwrap();
        let credentials = temp.path().join("credential.json");
        assert!(install(&profile, &credentials, &hosted()).is_err());
        assert!(!credentials.exists());
        assert_eq!(fs::read_dir(actual).unwrap().count(), 0);
    }
}
