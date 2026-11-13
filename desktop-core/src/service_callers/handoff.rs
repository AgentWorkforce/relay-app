//! `caller.json` (`docs/service-callers.md` section 5,
//! `docs/service-callers-handoff-v2.md`): the one place a caller's secret
//! leaves Desktop. Never holds the Relaycast token.

use super::registry::{Binding, CallerRecord};
use super::Class;
use agent_relay_node::platform::fs as private_fs;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The schema this build writes and the only one it reads.
pub(crate) const SCHEMA: &str = "agent-relay-service-caller/2";
/// The phase-1 schema. Its secret was sent as `caller_secret`; a reader
/// refuses it and never upgrades it in place.
pub(crate) const RETIRED_SCHEMA: &str = "agent-relay-service-caller/1";
/// The authentication `secret` keys
/// (`docs/service-callers-auth.md` section 3).
pub(crate) const AUTH: &str = "challenge-hmac-sha256/v1";
/// The two calls that bind a bridge connection to the caller.
pub(crate) const CHALLENGE_METHOD: &str = "spawn_callers.challenge";
pub(crate) const AUTHENTICATE_METHOD: &str = "spawn_callers.authenticate";
/// How results reach the caller: the result-inbox bridge on `socket`
/// (`docs/service-callers.md` section 8).
pub(crate) const INBOX_TRANSPORT: &str = "core-bridge";
pub(crate) const INBOX_OPEN: &str = "spawn_callers.inbox.open";
pub(crate) const INBOX_NEXT: &str = "spawn_callers.inbox.next";
pub(crate) const INBOX_ACK: &str = "spawn_callers.inbox.ack";
pub(crate) const INBOX_FAIL: &str = "spawn_callers.inbox.fail";
pub(crate) const INBOX_CLOSE: &str = "spawn_callers.inbox.close";
pub(crate) const DISCONNECT: &str = "spawn_callers.disconnect";

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Publisher {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct WorkspaceRef {
    pub(crate) site_url: String,
    pub(crate) base_url: String,
    pub(crate) workspace_id: String,
    pub(crate) relaycast_workspace_id: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct AccountRef {
    pub(crate) user_id: String,
}

/// The two calls of the challenge and authentication.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct AuthRoute {
    pub(crate) scheme: String,
    pub(crate) challenge: String,
    pub(crate) authenticate: String,
}

/// The result-inbox route: the bridge methods on `socket`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct InboxRoute {
    pub(crate) transport: String,
    pub(crate) open: String,
    pub(crate) next: String,
    pub(crate) ack: String,
    pub(crate) fail: String,
    pub(crate) close: String,
    pub(crate) disconnect: String,
}

/// `caller.json`, schema `agent-relay-service-caller/2`. Its `Debug`
/// never shows the secret.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallerFile {
    pub(crate) schema: String,
    pub(crate) auth: AuthRoute,
    pub(crate) caller_id: String,
    pub(crate) caller_name: String,
    pub(crate) publisher: Publisher,
    pub(crate) workspace: WorkspaceRef,
    pub(crate) account: AccountRef,
    pub(crate) secret: String,
    pub(crate) created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rotated_at: Option<String>,
    pub(crate) expires_at: String,
    pub(crate) harnesses: Vec<String>,
    pub(crate) directory_scope: Vec<String>,
    pub(crate) owns_only: bool,
    pub(crate) socket: String,
    pub(crate) inbox: InboxRoute,
    pub(crate) desktop_version: String,
}

impl std::fmt::Debug for CallerFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallerFile")
            .field("schema", &self.schema)
            .field("auth", &self.auth)
            .field("caller_id", &self.caller_id)
            .field("caller_name", &self.caller_name)
            .field("publisher", &self.publisher)
            .field("workspace", &self.workspace)
            .field("account", &self.account)
            .field("secret", &"<redacted>")
            .field("created_at", &self.created_at)
            .field("rotated_at", &self.rotated_at)
            .field("expires_at", &self.expires_at)
            .field("harnesses", &self.harnesses)
            .field("directory_scope", &self.directory_scope)
            .field("owns_only", &self.owns_only)
            .field("socket", &self.socket)
            .field("inbox", &self.inbox)
            .field("desktop_version", &self.desktop_version)
            .finish()
    }
}

/// Why a `caller.json` was refused. Every message is fixed text: none
/// carries a value read from the file, so the secret never reaches a log
/// or an error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CallerFileError {
    /// No file, or it could not be read as a private regular file.
    Unreadable,
    /// Not JSON, a field missing or of the wrong type, or a field this
    /// schema does not have.
    Malformed,
    /// Schema `agent-relay-service-caller/1`. Never upgraded in place.
    RetiredSchema,
    /// A schema this build does not know.
    UnknownSchema,
    /// An `auth` this build does not know.
    UnknownAuth,
}

impl std::fmt::Display for CallerFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CallerFileError::Unreadable => "caller file is missing or unreadable",
            CallerFileError::Malformed => {
                "caller file is not a valid agent-relay-service-caller/2 file"
            }
            CallerFileError::RetiredSchema => {
                "caller file schema agent-relay-service-caller/1 is no longer accepted; \
                 rotate the caller in Agent Relay to write an agent-relay-service-caller/2 file"
            }
            CallerFileError::UnknownSchema => "caller file schema is not supported by this version",
            CallerFileError::UnknownAuth => {
                "caller file auth scheme is not supported by this version"
            }
        })
    }
}

impl std::error::Error for CallerFileError {}

/// Reads `caller.json` as schema `/2`, failing closed on anything else.
pub(crate) fn read(path: &Path) -> Result<CallerFile, CallerFileError> {
    let bytes = super::registry::read_private(path)
        .ok()
        .flatten()
        .ok_or(CallerFileError::Unreadable)?;
    parse(&bytes)
}

/// Parses `caller.json` bytes as schema `/2`. The schema is checked before
/// the shape, so a `/1` file reads as [`CallerFileError::RetiredSchema`].
pub(crate) fn parse(bytes: &[u8]) -> Result<CallerFile, CallerFileError> {
    #[derive(Deserialize)]
    struct Head {
        schema: Option<serde_json::Value>,
    }
    let head: Head = serde_json::from_slice(bytes).map_err(|_| CallerFileError::Malformed)?;
    match head.schema.as_ref().and_then(serde_json::Value::as_str) {
        Some(SCHEMA) => {}
        Some(RETIRED_SCHEMA) => return Err(CallerFileError::RetiredSchema),
        Some(_) => return Err(CallerFileError::UnknownSchema),
        None => return Err(CallerFileError::Malformed),
    }
    let file: CallerFile = serde_json::from_slice(bytes).map_err(|_| CallerFileError::Malformed)?;
    if file.auth.scheme != AUTH
        || file.auth.challenge != CHALLENGE_METHOD
        || file.auth.authenticate != AUTHENTICATE_METHOD
    {
        return Err(CallerFileError::UnknownAuth);
    }
    if file.inbox != inbox_route() {
        return Err(CallerFileError::Malformed);
    }
    Ok(file)
}

fn inbox_route() -> InboxRoute {
    InboxRoute {
        transport: INBOX_TRANSPORT.into(),
        open: INBOX_OPEN.into(),
        next: INBOX_NEXT.into(),
        ack: INBOX_ACK.into(),
        fail: INBOX_FAIL.into(),
        close: INBOX_CLOSE.into(),
        disconnect: DISCONNECT.into(),
    }
}

/// The `/2` body Desktop writes for `caller`.
pub(crate) fn body(
    binding: &Binding,
    caller: &CallerRecord,
    secret: &str,
    socket: &str,
    version: &str,
) -> CallerFile {
    CallerFile {
        schema: SCHEMA.into(),
        auth: AuthRoute {
            scheme: AUTH.into(),
            challenge: CHALLENGE_METHOD.into(),
            authenticate: AUTHENTICATE_METHOD.into(),
        },
        caller_id: caller.caller_id.clone(),
        caller_name: caller.name.clone(),
        publisher: Publisher {
            id: caller.agent_id.clone(),
            name: caller.name.clone(),
        },
        workspace: WorkspaceRef {
            site_url: binding.site_url.clone(),
            base_url: binding.workspace.base_url.clone(),
            workspace_id: binding.workspace_id.clone(),
            relaycast_workspace_id: binding.relaycast_workspace_id.clone(),
        },
        account: AccountRef {
            user_id: binding.user_id.clone(),
        },
        secret: secret.into(),
        created_at: super::rfc3339(caller.created_at_ms),
        rotated_at: caller.rotated_at_ms.map(super::rfc3339),
        expires_at: super::rfc3339(caller.expires_at_ms),
        harnesses: caller.harnesses.clone(),
        directory_scope: caller
            .directory_roots
            .iter()
            .map(|root| root.resolved.display().to_string())
            .collect(),
        owns_only: caller.owns_only,
        socket: socket.into(),
        inbox: inbox_route(),
        desktop_version: version.into(),
    }
}

/// Where a provisioning call may put the file: the default, or an
/// absolute path in an existing folder this user owns and no one else can
/// write to.
pub(crate) fn path(
    binding: &Binding,
    caller_id: &str,
    given: Option<&str>,
) -> Result<PathBuf, Class> {
    let Some(given) = given else {
        return Ok(binding.default_handoff(caller_id));
    };
    let path = Path::new(given);
    if !path.is_absolute()
        || given.contains('\0')
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(Class::InvalidInput);
    }
    let folder = path.parent().ok_or(Class::InvalidInput)?;
    let meta = std::fs::metadata(folder).map_err(|_| Class::InvalidInput)?;
    if !meta.is_dir()
        || !private_fs::owned_by_current_user(&meta)
        || !private_fs::private_to_owner(&meta, 0o022)
    {
        return Err(Class::InvalidInput);
    }
    Ok(path.to_path_buf())
}

/// Writes the file 0600 next to its place and renames it over, refusing a
/// symbolic link at the place.
pub(crate) fn write(
    binding: &Binding,
    caller: &CallerRecord,
    secret: &str,
    socket: &str,
    version: &str,
) -> Result<(), Class> {
    #[cfg(test)]
    if FAIL_WRITE.with(|fail| fail.get()) {
        if FAIL_REGISTRY_AFTER.with(|after| after.get()) {
            super::registry::FAIL_SAVE.with(|fail| fail.set(true));
        }
        return Err(Class::StoreFailed);
    }
    let path = caller.handoff_path.as_ref().ok_or(Class::StoreFailed)?;
    let body = body(binding, caller, secret, socket, version);
    let bytes = serde_json::to_vec_pretty(&body).map_err(|_| Class::StoreFailed)?;
    let folder = path.parent().ok_or(Class::StoreFailed)?;
    if *folder == binding.folder.join("handoff") {
        private_fs::create_private_dir_all(folder).map_err(|_| Class::StoreFailed)?;
    }
    if std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_file()) {
        return Err(Class::StoreFailed);
    }
    let name = path.file_name().ok_or(Class::StoreFailed)?;
    let suffix = super::random_hex(8).ok_or(Class::StoreFailed)?;
    let temporary = folder.join(format!(".{}.{suffix}.tmp", name.to_string_lossy()));
    let result = (|| {
        let mut file = private_fs::no_follow(private_fs::private(
            std::fs::OpenOptions::new().write(true).create_new(true),
        ))
        .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err(Class::StoreFailed);
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// A failpoint: every handoff write on this thread fails.
    pub(crate) static FAIL_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// With [`FAIL_WRITE`]: the failed write also sets
    /// `registry::FAIL_SAVE`, so every registry write after it fails.
    pub(crate) static FAIL_REGISTRY_AFTER: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Whether `caller`'s `caller.json` is a `/2` file that holds its newest
/// secret: the digest of the file's secret against the newest kept
/// digest. A `/1` or otherwise refused file holds nothing. Nothing read is
/// kept or shown.
pub(crate) fn holds_newest(caller: &CallerRecord) -> bool {
    let Some(path) = caller.handoff_path.as_ref() else {
        return false;
    };
    let Some(newest) = caller.secrets.iter().max_by_key(|kept| kept.created_at_ms) else {
        return false;
    };
    let Ok(file) = read(path) else {
        return false;
    };
    super::registry::same(&newest.sha256, &super::registry::digest(&file.secret))
}

/// Deletes a handoff file; a symbolic link there is removed, never
/// followed.
pub(crate) fn remove(path: &Path) {
    if std::fs::symlink_metadata(path).is_ok() {
        let _ = std::fs::remove_file(path);
    }
}

/// Deletes a caller's handoff file, when it has one.
pub(crate) fn remove_kept(caller: &CallerRecord) {
    if let Some(path) = &caller.handoff_path {
        remove(path);
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::super::registry::{Binding, CallerRecord, Root, SecretDigest};
    use agent_relay_node::relay::workspace::{Secret, Workspace};
    use std::path::PathBuf;

    /// `fixtures/caller.v2.json`: what [`super::write`] puts in the file for
    /// [`fixed_input`], byte for byte.
    const FIXTURE: &[u8] = include_bytes!("fixtures/caller.v2.json");

    /// A binding and a caller with every value fixed: no clock, no random
    /// id and no secret is generated.
    fn fixed_input() -> (Binding, CallerRecord) {
        let binding = Binding {
            site_url: "https://agentrelay.example".into(),
            workspace_id: "ws_fixture0001".into(),
            user_id: "user_fixture0001".into(),
            workspace: Workspace {
                key: Secret::new("rk_live_fixture_not_written"),
                base_url: "https://api.relaycast.example".into(),
                workspace_id: Some("rw_fixture0001".into()),
                source: Default::default(),
                control_url: None,
            },
            relaycast_workspace_id: "rw_fixture0001".into(),
            folder: PathBuf::from("/fixture/ServiceCallers"),
            key: "fixture".into(),
        };
        let caller = CallerRecord {
            caller_id: "sc_0123456789abcdef".into(),
            name: "fixture-caller".into(),
            agent_id: "agent_fixture0001".into(),
            token: Some(Secret::new("at_live_fixture_not_written")),
            secrets: vec![SecretDigest {
                sha256: "0".repeat(64),
                created_at_ms: 1_793_491_200_000,
                retires_at_ms: None,
            }],
            harnesses: vec!["claude".into(), "codex".into()],
            directory_roots: vec![Root {
                given: "~/projects/fixture".into(),
                resolved: PathBuf::from("/home/fixture/projects/fixture"),
            }],
            owns_only: true,
            enabled: true,
            created_at_ms: 1_791_331_200_000,
            rotated_at_ms: Some(1_793_491_200_000),
            expires_at_ms: 1_801_267_200_000,
            // Not in the body; [`written`] points it at a temporary folder.
            handoff_path: None,
            requested_by: "app".into(),
            requested_at_ms: 1_793_491_200_000,
            pending: None,
            invocations: Vec::new(),
            authority_generation: 2,
            expiry_advanced: false,
            rollback: None,
        };
        (binding, caller)
    }

    /// The bytes [`super::write`] puts in the file at the record's
    /// handoff path, read back from disk.
    fn written(binding: &Binding, caller: &mut CallerRecord) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("caller.json");
        caller.handoff_path = Some(path.clone());
        super::write(
            binding,
            caller,
            "rsc_AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
            "/fixture/agent-relay/core.sock",
            "2026.11.4",
        )
        .unwrap();
        std::fs::read(&path).unwrap()
    }

    #[test]
    fn the_v2_writer_matches_the_fixture_byte_for_byte() {
        let (binding, mut caller) = fixed_input();
        let bytes = written(&binding, &mut caller);
        assert_eq!(
            String::from_utf8_lossy(&bytes),
            String::from_utf8_lossy(FIXTURE),
            "caller.json v2 output changed; the fixture is the contract"
        );
        assert_eq!(bytes, FIXTURE);
        let parsed = super::parse(FIXTURE).expect("the fixture reads as /2");
        assert_eq!(parsed.schema, super::SCHEMA);
    }
}
