use super::*;
fn fixture(development: bool) -> (tempfile::TempDir, Settings) {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::new(
        dir.path().join("hooks/agent-relay-desktop-mcp.json"),
        dir.path().join("helper with ' quote"),
        development,
    );
    (dir, settings)
}
fn write(settings: &Settings, bytes: &[u8]) {
    private_fs::create_private_dir_all(settings.path.parent().unwrap()).unwrap();
    fs::write(&settings.path, bytes).unwrap();
    private_fs::set_mode(&settings.path, 0o600).unwrap();
}
#[test]
fn exact_schema_enable_disable_and_absent_rollback() {
    let (_dir, settings) = fixture(false);
    let backup = settings.snapshot().unwrap();
    assert!(!settings.enabled());
    settings.set_enabled(true).unwrap();
    assert!(settings.enabled());
    let bytes = read(&settings.path).unwrap().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        value(&settings.helper, false)
    );
    let installed: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(installed["version"], 1);
    assert_eq!(installed["agentRelayManagedBy"], "agent-relay-desktop");
    assert_eq!(installed["hooks"].as_object().unwrap().len(), 1);
    assert_eq!(
        installed["hooks"]["preMcpToolCall"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(installed["hooks"]["preMcpToolCall"][0]["type"], "command");
    assert_eq!(installed["hooks"]["preMcpToolCall"][0]["timeoutSec"], 5);
    assert!(owned(&bytes, &settings.helper));
    backup.restore().unwrap();
    assert!(!settings.path.exists());
    settings.set_enabled(true).unwrap();
    settings.set_enabled(false).unwrap();
    assert!(!settings.path.exists());
}
#[test]
fn rollback_restores_exact_owned_prior_bytes_including_formatting() {
    let (_dir, settings) = fixture(true);
    let previous = serde_json::to_vec(&value(&settings.helper, false)).unwrap();
    write(&settings, &previous);
    let backup = settings.snapshot().unwrap();
    settings.set_enabled(true).unwrap();
    assert!(settings.enabled());
    backup.restore().unwrap();
    assert_eq!(fs::read(&settings.path).unwrap(), previous);
}
#[test]
fn rollback_handles_our_delete_but_not_foreign_delete() {
    let (_dir, settings) = fixture(false);
    settings.set_enabled(true).unwrap();
    let backup = settings.snapshot().unwrap();
    settings.set_enabled(false).unwrap();
    backup.restore().unwrap();
    assert!(settings.enabled());
    let backup = settings.snapshot().unwrap();
    fs::remove_file(&settings.path).unwrap();
    assert!(backup.restore().is_err());
    assert!(!settings.path.exists());
}
#[test]
fn foreign_file_or_additions_are_never_owned_or_overwritten() {
    let (_dir, settings) = fixture(false);
    let mut foreign = value(&settings.helper, false);
    foreign["hooks"]["other"] = json!([]);
    let bytes = serde_json::to_vec(&foreign).unwrap();
    write(&settings, &bytes);
    assert!(!settings.enabled());
    assert!(settings.snapshot().is_err());
    assert!(settings.set_enabled(true).is_err());
    assert!(settings.set_enabled(false).is_err());
    assert_eq!(fs::read(&settings.path).unwrap(), bytes);
}
#[test]
fn concurrent_foreign_edit_survives_rollback() {
    let (_dir, settings) = fixture(false);
    let backup = settings.snapshot().unwrap();
    settings.set_enabled(true).unwrap();
    write(&settings, b"{\"foreign\":true}");
    assert!(backup.restore().is_err());
    assert_eq!(fs::read(&settings.path).unwrap(), b"{\"foreign\":true}");
}
#[test]
fn recognized_legacy_helper_repairs_but_arbitrary_absolute_command_does_not() {
    let (_dir, settings) = fixture(false);
    let old = settings
        .helper
        .parent()
        .unwrap()
        .join(".agentworkforce/desktop/bin/agent-relay-probe");
    let previous = serde_json::to_vec(&value(&old, false)).unwrap();
    write(&settings, &previous);
    assert!(!settings.enabled());
    let backup = settings.snapshot().unwrap();
    settings.set_enabled(true).unwrap();
    backup.restore().unwrap();
    assert_eq!(fs::read(&settings.path).unwrap(), previous);
    write(
        &settings,
        &serde_json::to_vec(&value(Path::new("/tmp/foreign-helper"), false)).unwrap(),
    );
    assert!(settings.snapshot().is_err());
    assert!(settings.set_enabled(false).is_err());
}
#[test]
fn shell_metacharacters_cannot_extend_the_hook_command() {
    let (_dir, settings) = fixture(false);
    let mut document = value(&settings.helper, false);
    let command = document["hooks"]["preMcpToolCall"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    document["hooks"]["preMcpToolCall"][0]["command"] = json!(format!("{command}; echo foreign"));
    assert!(!owned(
        &serde_json::to_vec(&document).unwrap(),
        &settings.helper
    ));
    assert!(!valid_path(Path::new(
        "/tmp/../.local/bin/agent-relay-probe"
    )));
}
#[cfg(unix)]
#[test]
fn symlinks_and_shared_writable_files_fail_closed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (dir, settings) = fixture(false);
    let target = dir.path().join("foreign.json");
    fs::write(&target, b"{}").unwrap();
    fs::create_dir(settings.path.parent().unwrap()).unwrap();
    symlink(&target, &settings.path).unwrap();
    assert!(settings.snapshot().is_err());
    assert!(settings.set_enabled(true).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"{}");
    fs::remove_file(&settings.path).unwrap();
    write(
        &settings,
        &serde_json::to_vec(&value(&settings.helper, false)).unwrap(),
    );
    fs::set_permissions(&settings.path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(settings.snapshot().is_err());
}
#[test]
fn rollback_after_no_change_does_not_claim_a_prior_installation() {
    let (_dir, settings) = fixture(false);
    settings.set_enabled(true).unwrap();
    let backup = settings.snapshot().unwrap();
    settings.set_enabled(true).unwrap();
    fs::remove_file(&settings.path).unwrap();
    assert!(backup.restore().is_err());
}
