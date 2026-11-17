//! Where the desktop keeps the relay-node token it mints on a gate-only
//! install (`cloud_api::self_tokens`): the OS keychain, and nowhere else.
//!
//! One entry per site and workspace, in the service
//! [`SERVICE`] ("Agent Relay relay-node token"), account
//! `<site>/workspaces/<id>`:
//! - macOS: a generic password in the login keychain;
//! - Windows: a generic credential in Credential Manager, target
//!   `Agent Relay relay-node token:<site>/workspaces/<id>`;
//! - Linux: a Secret Service item (`secret-tool`, the secret on stdin) with
//!   the attributes `service` and `account`. Without a Secret Service the
//!   keychain is unavailable and no token is minted.
//!
//! The entry holds `{"id", "nodeName", "expiresAt", "token"}`: the token, and
//! the id its sign-out `DELETE` takes. Errors never carry the value.

use agent_relay_node::relay::workspace::Secret;
use serde_json::{json, Value};

/// The keychain service every entry is kept under.
pub const SERVICE: &str = "Agent Relay relay-node token";

/// The relay-node token kept for one workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub id: String,
    pub node_name: String,
    pub expires_at: Option<String>,
    pub token: Secret,
}

impl Kept {
    fn encode(&self) -> Secret {
        Secret::new(
            json!({"id": self.id, "nodeName": self.node_name, "expiresAt": self.expires_at,
                   "token": self.token.expose()})
            .to_string(),
        )
    }

    fn decode(value: &Secret) -> Option<Kept> {
        let value: Value = serde_json::from_str(value.expose()).ok()?;
        let text = |key: &str| {
            value[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Some(Kept {
            id: text("id")?,
            node_name: text("nodeName").unwrap_or_default(),
            expires_at: text("expiresAt"),
            token: Secret::new(text("token")?),
        })
    }

    /// Whether Control's expiry has passed at `now_ms`; one it did not name,
    /// or one that cannot be read, has not.
    pub fn expired(&self, now_ms: i64) -> bool {
        self.expires_at
            .as_deref()
            .and_then(agent_relay_node::parse_iso_ms)
            .is_some_and(|at| at <= now_ms)
    }
}

/// The entry's account for `workspace_id` on `site`.
pub fn entry(site: &str, workspace_id: &str) -> String {
    format!(
        "{}/workspaces/{}",
        site.trim_end_matches('/'),
        workspace_id.to_ascii_lowercase()
    )
}

/// An OS keychain. Blocking; callers run it off the async runtime. Each
/// error is a sentence for the person, without the value.
pub trait Keychain: Send + Sync {
    fn read(&self, account: &str) -> Result<Option<Secret>, String>;
    fn write(&self, account: &str, value: &Secret) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// The token kept for `account`. An entry that cannot be read as one is
/// treated as absent.
pub fn load(keychain: &dyn Keychain, account: &str) -> Result<Option<Kept>, String> {
    Ok(keychain.read(account)?.as_ref().and_then(Kept::decode))
}

pub fn store(keychain: &dyn Keychain, account: &str, kept: &Kept) -> Result<(), String> {
    keychain.write(account, &kept.encode())
}

/// This platform's keychain: Keychain on macOS, Credential Manager on
/// Windows, the Secret Service elsewhere.
pub struct Os;

#[cfg(target_os = "macos")]
impl Keychain for Os {
    fn read(&self, account: &str) -> Result<Option<Secret>, String> {
        use security_framework::passwords::get_generic_password;
        // errSecItemNotFound
        match get_generic_password(SERVICE, account) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(|value| Some(Secret::new(value)))
                .map_err(|_| "The keychain entry is not text.".to_string()),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(error) => Err(format!(
                "The keychain could not be read (OSStatus {}).",
                error.code()
            )),
        }
    }

    fn write(&self, account: &str, value: &Secret) -> Result<(), String> {
        security_framework::passwords::set_generic_password(
            SERVICE,
            account,
            value.expose().as_bytes(),
        )
        .map_err(|error| {
            format!(
                "The keychain could not be written (OSStatus {}).",
                error.code()
            )
        })
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(SERVICE, account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(error) => Err(format!(
                "The keychain entry could not be removed (OSStatus {}).",
                error.code()
            )),
        }
    }
}

#[cfg(windows)]
mod windows_credentials {
    use super::{Secret, SERVICE};
    use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
    use windows_sys::Win32::Security::Credentials::{
        CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    fn target(account: &str) -> Vec<u16> {
        format!("{SERVICE}:{account}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn read(account: &str) -> Result<Option<Secret>, String> {
        match ffi::read(&target(account)) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(|value| Some(Secret::new(value)))
                .map_err(|_| "The Credential Manager entry is not text.".to_string()),
            Err(ERROR_NOT_FOUND) => Ok(None),
            Err(error) => Err(format!(
                "Credential Manager could not be read (error {error})."
            )),
        }
    }

    pub fn write(account: &str, value: &Secret) -> Result<(), String> {
        let mut target = target(account);
        let mut blob = value.expose().as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        ffi::write(&credential)
            .map_err(|error| format!("Credential Manager could not be written (error {error})."))
    }

    pub fn delete(account: &str) -> Result<(), String> {
        match ffi::delete(&target(account)) {
            Ok(()) | Err(ERROR_NOT_FOUND) => Ok(()),
            Err(error) => Err(format!(
                "The Credential Manager entry could not be removed (error {error})."
            )),
        }
    }

    /// The Credential Manager calls, one `unsafe` block each, behind safe
    /// signatures. Errors are the thread's last error code.
    mod ffi {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Security::Credentials::{
            CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_TYPE_GENERIC,
        };

        /// The generic credential's blob for the NUL-terminated `target`.
        pub fn read(target: &[u16]) -> Result<Vec<u8>, u32> {
            debug_assert_eq!(target.last(), Some(&0), "target is NUL-terminated");
            let mut found: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: `target` is NUL-terminated and outlives the call; a
            // found credential's blob is copied out once, then the
            // credential is freed with `CredFree` and not touched again.
            unsafe {
                if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) == 0 {
                    return Err(GetLastError());
                }
                let credential = &*found;
                let bytes = if credential.CredentialBlob.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(
                        credential.CredentialBlob,
                        credential.CredentialBlobSize as usize,
                    )
                    .to_vec()
                };
                CredFree(found as *const _);
                Ok(bytes)
            }
        }

        /// Writes `credential`, whose pointers the caller keeps alive.
        pub fn write(credential: &CREDENTIALW) -> Result<(), u32> {
            // SAFETY: every pointer in `credential` points into a buffer
            // that outlives the call; `CredWriteW` copies them.
            // `GetLastError` reads this thread's last error.
            unsafe {
                if CredWriteW(credential, 0) == 0 {
                    return Err(GetLastError());
                }
            }
            Ok(())
        }

        /// Deletes the generic credential for the NUL-terminated `target`.
        pub fn delete(target: &[u16]) -> Result<(), u32> {
            debug_assert_eq!(target.last(), Some(&0), "target is NUL-terminated");
            // SAFETY: `target` is NUL-terminated and outlives the call.
            unsafe {
                if CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
                    return Err(GetLastError());
                }
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
impl Keychain for Os {
    fn read(&self, account: &str) -> Result<Option<Secret>, String> {
        windows_credentials::read(account)
    }
    fn write(&self, account: &str, value: &Secret) -> Result<(), String> {
        windows_credentials::write(account, value)
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        windows_credentials::delete(account)
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
impl Keychain for Os {
    fn read(&self, account: &str) -> Result<Option<Secret>, String> {
        let output = secret_tool(&["lookup", "service", SERVICE, "account", account], None)?;
        if output.status.success() {
            let value = String::from_utf8(output.stdout)
                .map_err(|_| "The Secret Service entry is not text.".to_string())?;
            let value = value.trim_end_matches('\n');
            return Ok((!value.is_empty()).then(|| Secret::new(value)));
        }
        // `lookup` exits 1 without a word when there is no such item.
        if output.stderr.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        Err(unavailable(&output.stderr))
    }

    fn write(&self, account: &str, value: &Secret) -> Result<(), String> {
        let output = secret_tool(
            &[
                "store", "--label", SERVICE, "service", SERVICE, "account", account,
            ],
            Some(value),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(unavailable(&output.stderr))
        }
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let output = secret_tool(&["clear", "service", SERVICE, "account", account], None)?;
        if output.status.success() || output.stderr.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err(unavailable(&output.stderr))
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn unavailable(stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.lines().next().unwrap_or_default().trim();
    format!("This computer's Secret Service keychain is unavailable: {detail}")
}

/// How long `secret-tool` has to answer. A Secret Service that hangs
/// (a locked collection whose unlock prompt never shows, a wedged
/// daemon) would otherwise hold the mint, and sign-out, for good.
#[cfg(not(any(target_os = "macos", windows)))]
const SECRET_TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Runs `secret-tool`, giving it `secret` on stdin, never on its command
/// line. One that has not exited within [`SECRET_TOOL_TIMEOUT`] is killed,
/// and is an error: the keychain did not answer, which is not an entry
/// that is absent.
#[cfg(not(any(target_os = "macos", windows)))]
fn secret_tool(args: &[&str], secret: Option<&Secret>) -> Result<std::process::Output, String> {
    use std::process::{Command, Stdio};
    let child = Command::new("secret-tool")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| {
            "This computer has no Secret Service keychain (secret-tool is not installed)."
                .to_string()
        })?;
    let input = secret.map(|s| s.expose().as_bytes().to_vec());
    wait_within(child, input, SECRET_TOOL_TIMEOUT).map_err(|timed_out| {
        if timed_out {
            format!(
                "The Secret Service keychain did not answer within {} seconds.",
                SECRET_TOOL_TIMEOUT.as_secs()
            )
        } else {
            "The Secret Service keychain did not answer.".to_string()
        }
    })
}

/// Gives `child` `input` on stdin and collects its output, killing it once
/// `timeout` has passed: `Err(true)` then, `Err(false)` when it could not
/// be waited for. Its pipes are read on threads of their own, so a child
/// that fills one never blocks the wait.
#[cfg(not(any(target_os = "macos", windows)))]
fn wait_within(
    mut child: std::process::Child,
    input: Option<Vec<u8>>,
    timeout: std::time::Duration,
) -> Result<std::process::Output, bool> {
    use std::io::{Read, Write};
    fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    }
    let stdin = child.stdin.take();
    let feed = std::thread::spawn(move || {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            let _ = stdin.write_all(&input);
        }
    });
    let (stdout, stderr) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(true);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(false);
            }
        }
    };
    let _ = feed.join();
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// A keychain in memory, for tests: nothing reaches the person's keychain.
#[cfg(any(test, feature = "test-site"))]
#[derive(Default)]
pub struct MemoryKeychain {
    pub entries: std::sync::Mutex<std::collections::BTreeMap<String, Secret>>,
    /// Every read and write fails with this while set.
    pub unavailable: std::sync::Mutex<Option<String>>,
    /// Every write fails with this while set; reads still answer.
    pub write_fails: std::sync::Mutex<Option<String>>,
    /// The next write waits for a message on this before it lands.
    pub write_paused: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// Each call so far, in order: `read`, `write` or `delete`.
    pub calls: std::sync::Mutex<Vec<&'static str>>,
}

#[cfg(any(test, feature = "test-site"))]
impl MemoryKeychain {
    fn held(&self) -> std::sync::MutexGuard<'_, std::collections::BTreeMap<String, Secret>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The token kept for `account`, if any.
    pub fn kept(&self, account: &str) -> Option<Kept> {
        self.held().get(account).and_then(Kept::decode)
    }

    pub fn keep(&self, account: &str, kept: &Kept) {
        self.held().insert(account.to_string(), kept.encode());
    }

    /// The calls so far, in order.
    pub fn calls(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn call(&self, call: &'static str) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    fn check(&self, switch: &std::sync::Mutex<Option<String>>) -> Result<(), String> {
        let failure = switch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(any(test, feature = "test-site"))]
impl Keychain for MemoryKeychain {
    fn read(&self, account: &str) -> Result<Option<Secret>, String> {
        self.call("read");
        self.check(&self.unavailable)?;
        Ok(self.held().get(account).cloned())
    }
    fn write(&self, account: &str, value: &Secret) -> Result<(), String> {
        self.call("write");
        let paused = self
            .write_paused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(paused) = paused {
            let _ = paused.recv();
        }
        self.check(&self.unavailable)?;
        self.check(&self.write_fails)?;
        self.held().insert(account.to_string(), value.clone());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        self.call("delete");
        self.held().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_round_trips_and_names_its_workspace() {
        let keychain = MemoryKeychain::default();
        let account = entry(
            "https://relay.example.test/",
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
        );
        assert_eq!(
            account,
            "https://relay.example.test/workspaces/0f8fad5b-d9cb-469f-a165-70867728950e"
        );
        let kept = Kept {
            id: "tok_1".into(),
            node_name: "desk-desktop".into(),
            expires_at: Some("2027-02-14T00:00:00Z".into()),
            token: Secret::new("sh_wk_fixture1234"),
        };
        store(&keychain, &account, &kept).unwrap();
        assert_eq!(load(&keychain, &account).unwrap(), Some(kept.clone()));
        assert!(!kept.expired(0));
        assert!(kept.expired(i64::MAX));
        keychain.delete(&account).unwrap();
        assert_eq!(load(&keychain, &account).unwrap(), None);
        // An entry that is not one of ours is absent.
        keychain.write(&account, &Secret::new("not json")).unwrap();
        assert_eq!(load(&keychain, &account).unwrap(), None);
    }

    /// A child that never answers is killed at the deadline and reported
    /// as not answering, not as an entry that is absent.
    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn a_keychain_child_that_hangs_is_killed_at_the_deadline() {
        use std::process::{Command, Stdio};
        let spawn = |script: &str| {
            Command::new("sh")
                .args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        };
        let started = std::time::Instant::now();
        let hung = wait_within(spawn("sleep 30"), None, std::time::Duration::from_millis(200));
        assert!(hung.unwrap_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        // One that answers in time gives its output, stdin included.
        let answered = wait_within(
            spawn("cat; echo oops >&2"),
            Some(b"value".to_vec()),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert!(answered.status.success());
        assert_eq!(answered.stdout, b"value");
        assert_eq!(answered.stderr, b"oops\n");
    }

    #[test]
    fn a_keys_debug_form_never_shows_the_token() {
        let kept = Kept {
            id: "tok_1".into(),
            node_name: "n".into(),
            expires_at: None,
            token: Secret::new("sh_wk_fixture1234"),
        };
        assert!(!format!("{kept:?}").contains("fixture1234"));
        assert!(!format!("{:?}", kept.encode()).contains("fixture1234"));
    }
}
