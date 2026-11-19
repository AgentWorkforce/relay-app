//! Sign-in with an enrollment key: a single-use key an administrator minted
//! for one person, so a headless install signs in without anyone approving a
//! device code. Cloud exchanges the key once for the session a device
//! approval gives, and the sign-in then goes on as a device sign-in does.
//!
//! The key reaches the core as a file path and is read here. It is held as a
//! [`Secret`], sent only in the exchange's body, and never logged, shown or
//! put in an error. The file must be private to this user and is not read
//! through a link. Once Cloud accepts the key, its file is deleted if it is
//! still the file that was read, even when the sign-in was cancelled while
//! the exchange was in flight.
//!
//! A key minted with the organization's consent makes the session
//! **managed**. The exchange's answer says whether it is (`managed`) and may
//! carry the organization's workspace policy (`workspace_policy`, the
//! document `access/me` serves, with its `policy_revision`). The answer names
//! no workspace, so the document is cached for the workspace the sign-in
//! connects to, just before that workspace's first policy read: a new
//! computer has the policy even when that read fails. A document that is
//! missing or malformed is ignored and the first read decides.

use super::{Account, Start};
use crate::contract::account::{Flow, SignInRequest};
use crate::workspace_policy::{self, WorkspacePolicy};
use agent_relay_node::platform::fs::{owned_by_current_user, private_to_owner};
use agent_relay_node::relay::cloud_http::sendable;
use agent_relay_node::relay::cloud_login::LoginError;
use agent_relay_node::relay::cloud_session::{api_base, iso};
use agent_relay_node::relay::daemon::Core;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use agent_relay_node::relay::request_log::Capture;
use agent_relay_node::relay::state::{now_ms, Level};
use agent_relay_node::relay::store::CloudSession;
use agent_relay_node::relay::websocket_tls::{client_config, platform_root_environment_is_safe};
use agent_relay_node::relay::workspace::Secret;
use serde_json::{json, Value};
use std::fs::{File, Metadata, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::Duration;

/// The phase while Cloud exchanges the key.
pub(super) const EXCHANGING: &str = "exchanging_enrollment_key";
/// How an exchange that did not sign in ended. Each is also the failed
/// attempt's phase, so a caller can act on it without reading the message.
const REJECTED: &str = "enrollment_key_rejected";
const UNSUPPORTED: &str = "enrollment_key_unsupported";
const BUSY: &str = "enrollment_key_busy";

/// The client Cloud records for the exchange.
const CLIENT_NAME: &str = "agent-relay-desktop";
/// The largest key file read; a key is far shorter.
const MAX_KEY_BYTES: u64 = 4096;
/// The largest answer read.
const MAX_ANSWER_BYTES: u64 = 1024 * 1024;
/// How many times `slow_down` is retried before the attempt fails.
const SLOW_DOWN_RETRIES: u32 = 3;
/// The wait when Cloud asks to slow down without saying how long, and the
/// longest wait honoured when it does.
const DEFAULT_RETRY_AFTER: u64 = 5;
const MAX_RETRY_AFTER: u64 = 60;
const TIMEOUT: Duration = Duration::from_secs(30);
/// The longest host name Cloud keeps, in UTF-16 units.
const MAX_HOST_UNITS: usize = 80;

/// A key read for an attempt, waiting for its exchange.
pub(super) struct EnrollmentKeyLogin {
    key: Secret,
    file: PathBuf,
    /// The file the key was read from, so only that file is deleted.
    read: Identity,
    host_name: String,
}

/// What tells the key file apart from one put at its path later: device and
/// inode on Unix, volume serial number and file index on Windows.
type Identity = (u64, u64);

/// What one exchange answered.
struct Reply {
    status: u16,
    body: Value,
    retry_after: Option<u64>,
}

/// Why an exchange got no answer.
#[derive(Debug, PartialEq)]
enum Unanswered {
    /// The request never reached Cloud, so the key is unspent.
    NotSent,
    /// The request may have reached Cloud, so the key may be spent.
    Lost,
}

impl Unanswered {
    fn error(&self) -> LoginError {
        let message = match self {
            Unanswered::NotSent => "Could not reach Agent Relay Cloud.",
            Unanswered::Lost => {
                "Agent Relay Cloud did not answer, so the enrollment key may have been used. Try \
                 again; if the key is not accepted, ask your administrator for a new key."
            }
        };
        LoginError {
            state: "error",
            message: message.into(),
        }
    }
}

/// Whether a request that failed with `kind` may have reached Cloud: only a
/// failure before the connection was made means it did not.
fn unanswered(kind: ureq::ErrorKind) -> Unanswered {
    use ureq::ErrorKind::*;
    match kind {
        InvalidUrl
        | UnknownScheme
        | Dns
        | InsecureRequestHttpsOnly
        | ConnectionFailed
        | InvalidProxyUrl
        | ProxyConnect
        | ProxyUnauthorized => Unanswered::NotSent,
        _ => Unanswered::Lost,
    }
}

/// What a session the exchange gave comes with.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Enrolled {
    /// The organization's workspace policy the answer carried, with the
    /// answer's `managed` as its own.
    pub(super) seed: Option<Box<WorkspacePolicy>>,
}

enum Outcome {
    Session(Box<CloudSession>, Enrolled),
    Wait(u64),
    Failed(LoginError),
}

/// Refuses a request whose flow and key file disagree: the file is required
/// with the `enrollment_key` flow and refused with any other.
pub(super) fn check_request(request: &SignInRequest) -> Result<(), RpcError> {
    let keyed = request.flow == Some(Flow::EnrollmentKey);
    match (keyed, request.enrollment_key_file.is_some()) {
        (true, false) => Err(RpcError::invalid_params(
            "An enrollment key sign-in needs enrollment_key_file, the path of the file holding the key.",
        )),
        (false, true) => Err(RpcError::invalid_params(
            "enrollment_key_file is only read with the enrollment_key flow.",
        )),
        _ => Ok(()),
    }
}

/// An enrollment key signs in a computer that is not signed in yet; one
/// whose kept session Cloud refused approves again on a device.
pub(super) fn renewal_refused() -> RpcError {
    RpcError::new(
        ErrorKind::Refused,
        "This computer is already signed in. Sign in again with device approval.",
    )
}

/// The phase a failed attempt keeps: the enrollment key's ending, if it was
/// one, or a refused certificate-file override.
pub(super) fn ended_phase(state: &str) -> Option<String> {
    [
        REJECTED,
        UNSUPPORTED,
        BUSY,
        super::login_start::TLS_ROOTS_REFUSED,
    ]
    .contains(&state)
    .then(|| state.to_string())
}

fn ended(state: &'static str) -> LoginError {
    let message = match state {
        REJECTED => "This enrollment key was not accepted. Ask your administrator for a new key.",
        UNSUPPORTED => {
            "This Agent Relay site does not accept enrollment keys. Sign in with device approval instead."
        }
        BUSY => "Agent Relay Cloud is busy. Try the enrollment key again in a few minutes.",
        _ => "Enrollment key sign-in did not finish. Try again.",
    };
    LoginError {
        state,
        message: message.into(),
    }
}

/// The identity of the open `file`.
#[cfg(unix)]
fn identity(file: &File) -> std::io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

/// The identity of the open `file`, read from its handle: std's accessors
/// for the volume serial number and file index are not stable.
#[cfg(windows)]
fn identity(file: &File) -> std::io::Result<Identity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: an open handle `file` keeps alive, and one valid out pointer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((u64::from(info.dwVolumeSerialNumber), index))
}

/// Opens `file` for reading without following a link at its last component:
/// `O_NOFOLLOW` on Unix (with `O_NONBLOCK`, so a FIFO cannot stall the
/// open); on Windows the link itself is opened, and refused by its type.
fn open_no_follow(file: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(file)
}

/// The key in `file`: an absolute path to a regular file, not a link, owned
/// by this user and readable by nobody else, holding one line of printable
/// text. Nothing about the content reaches the error.
fn read_key(file: &Path) -> Result<(Secret, Identity), RpcError> {
    if !file.is_absolute() {
        return Err(RpcError::invalid_params(
            "The enrollment key file must be an absolute path.",
        ));
    }
    let unreadable =
        || RpcError::invalid_params("Could not read an enrollment key from that file.");
    let opened = open_no_follow(file).map_err(|_| unreadable())?;
    let metadata = opened.metadata().map_err(|_| unreadable())?;
    if !regular(&metadata) || metadata.len() > MAX_KEY_BYTES {
        return Err(unreadable());
    }
    // Unix only. Windows has no mode to check: the key file is expected in
    // the user's own profile and setup's copy of a key from stdin in the
    // user's own temp directory, so only the link refusal applies there.
    if !owned_by_current_user(&metadata) || !private_to_owner(&metadata, 0o077) {
        return Err(RpcError::invalid_params(
            "The enrollment key file must be owned by you and readable by nobody else (mode 0600).",
        ));
    }
    let read = identity(&opened).map_err(|_| unreadable())?;
    let mut text = String::new();
    opened
        .take(MAX_KEY_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| unreadable())?;
    let key = text.trim();
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(unreadable());
    }
    Ok((Secret::new(key), read))
}

/// A regular file, not a link opened as itself (Windows).
fn regular(metadata: &Metadata) -> bool {
    !metadata.file_type().is_symlink() && metadata.is_file()
}

/// Deletes a spent key's file if `file`, opened afresh without following a
/// link, is still the file that was read; one already gone is fine.
/// Otherwise answers the warning to show.
fn delete_spent(file: &Path, read: &Identity) -> Result<(), String> {
    let left = |why: &str| {
        Err(format!(
            "The enrollment key was used, but its file {why}. Delete {}.",
            file.display()
        ))
    };
    let changed = "changed since it was read and was left alone";
    match std::fs::symlink_metadata(file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return left("could not be deleted"),
        Ok(current) if !regular(&current) => return left(changed),
        Ok(_) => {}
    }
    let current = open_no_follow(file).and_then(|opened| {
        let metadata = opened.metadata()?;
        Ok(regular(&metadata) && identity(&opened)? == *read)
    });
    match current {
        Ok(true) => {}
        Ok(false) => return left(changed),
        Err(_) => return left("could not be deleted"),
    }
    match std::fs::remove_file(file) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => left("could not be deleted"),
        _ => Ok(()),
    }
}

/// This computer's name as the exchange sends it: trimmed, without control
/// characters, cut to what Cloud keeps.
fn host_name(name: &str) -> Option<String> {
    let mut units = 0;
    let name: String = name
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take_while(|ch| {
            units += ch.len_utf16();
            units <= MAX_HOST_UNITS
        })
        .collect();
    let name = name.trim_end();
    (!name.is_empty()).then(|| name.to_string())
}

fn inner(value: &Value) -> &Value {
    match value.get("data") {
        Some(data) if data.is_object() => data,
        _ => value,
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The session in a 200, built as a completed device sign-in builds it.
fn session(value: &Value, site: &str) -> Result<CloudSession, LoginError> {
    let access = text(value, "access_token").ok_or_else(|| LoginError {
        state: "error",
        message: "Agent Relay Cloud did not return a sign-in session.".into(),
    })?;
    let api = format!("{site}/cloud");
    if value["api_url"]
        .as_str()
        .is_some_and(|url| url.trim_end_matches('/') != api)
    {
        return Err(LoginError {
            state: "error",
            message: "Agent Relay Cloud changed origin during sign-in.".into(),
        });
    }
    let expires_at = text(value, "access_token_expires_at").or_else(|| {
        value["expires_in"]
            .as_f64()
            .map(|seconds| iso(now_ms() + (seconds * 1000.0) as i64))
    });
    Ok(CloudSession {
        access_token: Secret::new(access),
        access_token_expires_at: expires_at,
        refresh_token: text(value, "refresh_token").map(Secret::new),
        refresh_token_expires_at: text(value, "refresh_token_expires_at"),
        site_url: site.to_string(),
    })
}

/// What a 200 says of the session beyond its tokens: whether it is managed,
/// and the workspace policy to seed the cache with. A document that does not
/// parse seeds nothing. The answer's `managed` is the session's, so it is
/// the document's too.
fn enrolled(value: &Value) -> Enrolled {
    let managed = value["managed"].as_bool().unwrap_or(false);
    let revision = value["policy_revision"].as_i64();
    let seed = value
        .get("workspace_policy")
        .filter(|document| !document.is_null())
        .and_then(
            |document| match workspace_policy::parse_document(document) {
                // The answer names the document's revision; one that
                // disagrees is not trusted.
                Ok(policy) if revision.is_some_and(|revision| revision != policy.revision) => {
                    eprintln!(
                        "enrollment key: the workspace policy was ignored: its revision is not the answer's policy_revision"
                    );
                    None
                }
                Ok(mut policy) => {
                    policy.managed = managed;
                    Some(Box::new(policy))
                }
                Err(error) => {
                    eprintln!("enrollment key: the workspace policy was ignored: {error}");
                    None
                }
            },
        );
    Enrolled { seed }
}

/// What an answer means. The rejections stay generic: a wrong, expired,
/// revoked or already used key all read the same.
fn outcome(reply: &Reply, site: &str) -> Outcome {
    let value = inner(&reply.body);
    let code = value["error"].as_str();
    match (reply.status, code) {
        (200, _) => match session(value, site) {
            Ok(session) => Outcome::Session(Box::new(session), enrolled(value)),
            Err(error) => Outcome::Failed(error),
        },
        (429, _) | (_, Some("slow_down")) => Outcome::Wait(
            reply
                .retry_after
                .unwrap_or(DEFAULT_RETRY_AFTER)
                .min(MAX_RETRY_AFTER),
        ),
        (404, _) => Outcome::Failed(ended(UNSUPPORTED)),
        (_, Some("invalid_grant" | "invalid_request")) => Outcome::Failed(ended(REJECTED)),
        _ => Outcome::Failed(ended("error")),
    }
}

/// Posts the exchange, naming the site as its origin, and reads the answer
/// with its `Retry-After`. Redirects are not followed. An answer whose body
/// cannot be read keeps its status, so a 200 still counts as spending the
/// key. Blocking.
fn post(url: &str, site: &str, body: &Value) -> Result<Reply, Unanswered> {
    if !sendable(url) || !platform_root_environment_is_safe() {
        return Err(Unanswered::NotSent);
    }
    let agent = ureq::AgentBuilder::new()
        .tls_config(client_config())
        .redirects(0)
        .timeout(TIMEOUT)
        .build();
    let response = match agent
        .post(url)
        .set("Accept", "application/json")
        .set("Origin", site)
        .set("User-Agent", &crate::client_telemetry::user_agent())
        .send_json(body)
    {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error)) => return Err(unanswered(error.kind())),
    };
    let status = response.status();
    let retry_after = response
        .header("Retry-After")
        .and_then(|seconds| seconds.trim().parse().ok());
    let mut bytes = Vec::new();
    let read = response
        .into_reader()
        .take(MAX_ANSWER_BYTES)
        .read_to_end(&mut bytes);
    let body = match read {
        Ok(_) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    Ok(Reply {
        status,
        body,
        retry_after,
    })
}

/// [`post`], recorded in the request log with the key replaced.
fn logged_post(url: &str, site: &str, body: &Value, key: &Secret) -> Result<Reply, Unanswered> {
    let mut shown = body.clone();
    shown["enrollment_key"] = json!("[redacted]");
    let mut capture = Capture::start("Cloud", "POST", url, &shown, key.expose());
    let reply = post(url, site, body);
    match &reply {
        Ok(reply) => capture.finish(
            Some(reply.status),
            !(200..300).contains(&reply.status),
            &reply.body,
        ),
        Err(_) => capture.finish(None, true, &json!({"error": "unreachable"})),
    }
    reply
}

/// [`logged_post`], then, once Cloud accepted the key (any 200 spends it,
/// even one without a usable session), deletes its file and shows a warning
/// on `core` if the file is left. This runs on the blocking thread, which
/// cancelling or signing out does not stop, so a key spent by a request
/// already sent has its file deleted even then.
fn exchange_once(
    url: &str,
    site: &str,
    body: &Value,
    login: &EnrollmentKeyLogin,
    core: Option<Weak<Core>>,
) -> Result<Reply, Unanswered> {
    let reply = logged_post(url, site, body, &login.key)?;
    if reply.status == 200 {
        if let Err(warning) = delete_spent(&login.file, &login.read) {
            if let Some(core) = core.as_ref().and_then(Weak::upgrade) {
                core.hub.activity(Level::Warning, warning);
            }
        }
    }
    Ok(reply)
}

impl EnrollmentKeyLogin {
    /// Exchanges the key once, waiting out at most a few `slow_down`s.
    async fn exchange(
        self,
        site: &str,
        core: Option<Weak<Core>>,
    ) -> Result<(CloudSession, Enrolled), LoginError> {
        let url = format!("{}/auth/enrollment/token", api_base(site));
        let body = json!({ "enrollment_key": self.key.expose(), "client_name": CLIENT_NAME,
            "desktop_host_name": self.host_name });
        let login = std::sync::Arc::new(self);
        let mut retries = 0;
        loop {
            let request = (
                url.clone(),
                site.to_string(),
                body.clone(),
                login.clone(),
                core.clone(),
            );
            let reply = tokio::task::spawn_blocking(move || {
                let (url, origin, body, login, core) = request;
                exchange_once(&url, &origin, &body, &login, core)
            })
            .await
            .unwrap_or(Err(Unanswered::Lost));
            let reply = reply.map_err(|unanswered| unanswered.error())?;
            match outcome(&reply, site) {
                Outcome::Session(session, enrolled) => return Ok((*session, enrolled)),
                Outcome::Wait(seconds) if retries < SLOW_DOWN_RETRIES => {
                    retries += 1;
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                }
                Outcome::Wait(_) => return Err(ended(BUSY)),
                Outcome::Failed(error) => return Err(error),
            }
        }
    }
}

impl Account {
    /// Reads the key for `attempt` before the request is answered, so the
    /// caller may remove a file it made once the call returns. A key that
    /// cannot be read ends the attempt.
    pub(super) fn enrollment_key_start(
        &self,
        attempt: u64,
        file: Option<&Path>,
    ) -> Result<Start, RpcError> {
        let login = file
            .ok_or_else(|| RpcError::invalid_params("No enrollment key file was given."))
            .and_then(|file| {
                let (key, read) = read_key(file)?;
                let host_name = host_name(&self.desktop_host()?).ok_or_else(|| {
                    RpcError::invalid_params("Could not identify this computer for sign-in.")
                })?;
                Ok(EnrollmentKeyLogin {
                    key,
                    file: file.to_path_buf(),
                    read,
                    host_name,
                })
            });
        login.map(Start::EnrollmentKey).inspect_err(|error| {
            self.fail(
                attempt,
                &LoginError {
                    state: "error",
                    message: error.message.clone(),
                },
            )
        })
    }

    /// Exchanges the key; once Cloud accepts it the key is spent, so its
    /// file is deleted, unless it changed since it was read. That holds even
    /// when the answer carried no usable session and the attempt fails, and
    /// when the attempt is cancelled while the exchange is in flight. The
    /// workspace policy the answer carried is kept for this attempt, to be
    /// cached for the workspace it connects to
    /// ([`Account::take_enrollment_seed`]).
    pub(super) async fn enrollment_key_session(
        &self,
        attempt: u64,
        login: EnrollmentKeyLogin,
        site: &str,
    ) -> Result<CloudSession, LoginError> {
        let (session, enrolled) = login.exchange(site, self.core.get().cloned()).await?;
        // The attempt that ran this exchange: one that started since takes
        // nothing from it.
        self.operator_policy
            .workspace_policies()
            .hold_seed(attempt, enrolled.seed.map(|policy| *policy));
        Ok(session)
    }

    /// The sign-in attempt under way now.
    fn current_attempt(&self) -> u64 {
        self.attempt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .id
    }

    /// Caches the workspace policy the current attempt's enrollment key
    /// answered with for `workspace_id`, the workspace the sign-in connects
    /// to, as a read would cache it: a lower revision than the cached one is
    /// refused, and the read that follows replaces it. A seed of an earlier
    /// attempt is dropped.
    pub(super) fn take_enrollment_seed(&self, workspace_id: &str) {
        self.operator_policy
            .workspace_policies()
            .take_seed(self.current_attempt(), workspace_id);
    }
}

#[cfg(test)]
mod tests;
