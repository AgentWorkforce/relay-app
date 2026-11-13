//! Claude Code's claude.ai login in the macOS login Keychain: a generic
//! password, service `Claude Code-credentials`, account the login user name.
//! Claude Code created the item, so macOS asks the person before another app
//! reads it. That prompt belongs to a moment the person chose: turning
//! Claude Code cloud sessions on, or opening one. Every other read is quiet:
//! it fails at once instead of asking. Background capture never raises
//! Keychain UI.
//!
//! Reads are one at a time per process, because a quiet read turns the
//! Keychain's user interaction off for the whole process while it runs.
//!
//! Claude Code rewrites the item whenever it refreshes its login, through
//! Apple's `security` tool. That rewrite leaves the item's access list
//! trusting this helper (the person's Always Allow) but resets its
//! partition list to `apple-tool:` alone, and macOS requires both: from
//! then on every Security framework read fails with OSStatus -25293
//! (`errSecAuthFailed`) until the person grants access again, and the next
//! refresh withdraws it again. So when a read fails with -25293, and only
//! then, the secret may be read the way Claude Code itself reads and writes
//! it: `/usr/bin/security find-generic-password -w`, against the login
//! keychain by path. That path is taken only while the person has Claude
//! Code cloud sessions turned on, the consent every read of this item
//! already rests on.
//!
//! Whether the tool can read the item without a prompt is decided by the
//! item's own ACL, which Claude Code's own write through the tool sets:
//! its access list trusts `/usr/bin/security` for decrypt and its
//! partition list is `apple-tool:`. Nothing about how the tool is started
//! can stop `securityd` from showing a dialog if the ACL does not permit
//! the read, and the tool asks for the keychain password when the keychain
//! is locked. Every check below looks at one keychain, the one the tool
//! reads from: the login keychain, opened by path (`SecKeychainOpen`). If
//! it is missing or cannot be opened, the fallback is skipped. A -25293 can
//! also come from a locked login keychain, so the fallback first requires
//! that keychain's status (`SecKeychainGetStatus`) to say it is unlocked;
//! when it is locked the read fails marked [`ReadError::locked`], so the
//! person is told to unlock it rather than to grant access again. Then
//! "background capture never raises Keychain UI" is kept by an ACL precheck
//! that cannot prompt. With user interaction turned off, as for the quiet
//! read, the item is found by reference in that keychain only, which reads
//! neither its secret nor decrypts it, so an item with the same service and
//! account in another keychain on the search list never stands in for it.
//! Its access list is read through the Security API:
//! `SecKeychainItemCopyAccess`, `SecAccessCopyACLList`, then
//! `SecACLCopyAuthorizations` and `SecACLCopyContents` for each entry
//! ([`item_acls`]). The fallback runs only when [`fallback_permitted`]
//! finds an entry authorizing decrypt that trusts `/usr/bin/security` (or
//! every application), and a partition entry whose description (a plist,
//! hex-encoded or not) lists `apple-tool:`. An entry's prompt selector does
//! not enter into it: the password prompt it may ask for applies only to
//! applications the entry does not trust. If either fact cannot be
//! established, or any of those calls fails, the read reports the original
//! -25293.
//!
//! The precheck does not ask the tool (`security dump-keychain -a`): on the
//! login keychain the tool is refused the authorization to read access
//! lists (`SecKeychainItemCopyAccess: Unable to obtain authorization for
//! this operation.` for every item), prints no access list at all and still
//! exits 0, so a check built on its dump could never pass.
//!
//! Defence in depth, not the guarantee: the tool runs without stdin and in
//! a session of its own, which only keeps it off a terminal; it is killed
//! after [`FALLBACK_TIMEOUT`], which also closes any dialog `securityd`
//! showed for it; and a secret read that times out (the shape a dialog
//! takes, since a permitted read answers in milliseconds) latches the
//! fallback off for the rest of the process. The latch clears when the
//! person turns Claude Code cloud sessions on ([`ask_access`]), so even if
//! the precheck were wrong, a dialog could appear at most once per such
//! episode, and it closes when the tool is killed. A secret read that fails
//! fast does not latch. The tool's output is never logged. Product code
//! never changes the item's access list or partition list.

#[cfg(target_os = "macos")]
use {security_framework::os::macos::keychain::SecKeychain, std::path::Path};

/// The Keychain service Claude Code keeps its claude.ai login under.
pub const SERVICE: &str = "Claude Code-credentials";

/// A Keychain read failed. The OSStatus, and whether the login keychain was
/// locked, are safe to report and are the only details retained; the item,
/// account and secret never cross this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub code: Option<i32>,
    /// The read failed with -25293 and the `security` tool fallback was
    /// skipped because the login keychain is locked: unlocking it, not a
    /// new grant, is what the person has to do.
    pub locked: bool,
}

impl ReadError {
    pub const fn unknown() -> Self {
        Self {
            code: None,
            locked: false,
        }
    }
}

/// OSStatus `errSecAuthFailed`: what a Security framework read answers once
/// Claude Code's rewrite dropped this helper's team from the item's
/// partition list.
pub const PARTITION_DENIED: i32 = -25293;

/// How long the `security` tool may take before it is killed. It answers
/// in milliseconds when the item permits it; anything longer is a prompt
/// nobody asked for, or a stuck `securityd`.
pub const FALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// The bytes `text` spells in hexadecimal, when it is non-empty, of even
/// length and all ASCII hex digits; `None` otherwise.
fn decode_hex(text: &[u8]) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() % 2 != 0 || !text.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    text.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// The secret from `security find-generic-password -w`'s standard output:
/// the password followed by one newline. The tool prints a password that
/// holds non-printable bytes (a newline in pretty-printed JSON, say) as
/// hexadecimal instead, and that is decoded. Claude Code's credentials are
/// JSON, which always holds `{` and `"`, so a real secret is never all hex
/// digits and is never mistaken for hex. Empty output is no secret.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn secret_from_tool_output(mut stdout: Vec<u8>) -> Option<Vec<u8>> {
    if stdout.last() == Some(&b'\n') {
        stdout.pop();
        if stdout.last() == Some(&b'\r') {
            stdout.pop();
        }
    }
    if stdout.is_empty() {
        return None;
    }
    match decode_hex(&stdout) {
        Some(decoded) => {
            stdout.fill(0);
            Some(decoded)
        }
        None => Some(stdout),
    }
}

/// The tool Claude Code reads and writes the item with, and the one the
/// item's access list trusts.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SECURITY_TOOL: &str = "/usr/bin/security";

/// How a run of the `security` tool failed.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolFailure {
    /// It did not start, exited unsuccessfully or its output was unreadable:
    /// an answer, given at once.
    Failed,
    /// Still running at its timeout, and killed.
    TimedOut,
}

/// Whether a failed secret read through the tool latches the fallback off
/// for the process: only a timeout. A dialog keeps the tool waiting; a
/// refusal ends it fast.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn latch_for(failure: ToolFailure) -> bool {
    failure == ToolFailure::TimedOut
}

/// Whether a failed Security framework read may try the `security` tool:
/// only on -25293, only while the fallback is not latched off, and only
/// while cloud sessions are on (asked last, only when the rest hold).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn fallback_allowed(
    code: Option<i32>,
    latched: bool,
    cloud_sessions_on: impl FnOnce() -> bool,
) -> bool {
    code == Some(PARTITION_DENIED) && !latched && cloud_sessions_on()
}

/// One entry of the item's access list, as the Security API reports it
/// ([`item_acls`]).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AclSummary {
    /// `SecACLCopyAuthorizations`: `ACLAuthorizationDecrypt` and the like.
    authorizations: Vec<String>,
    /// The trusted applications' paths; `None` when the entry trusts every
    /// application.
    applications: Option<Vec<String>>,
    /// The entry's description; the partition entry's is the partition
    /// list as a plist.
    description: Option<String>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl AclSummary {
    fn authorizes(&self, any_of: &[&str]) -> bool {
        self.authorizations
            .iter()
            .any(|authorization| any_of.contains(&authorization.as_str()))
    }

    /// Whether this entry lets [`SECURITY_TOOL`] decrypt the item without a
    /// prompt: it authorizes decrypt and trusts the tool, or every
    /// application. The entry's prompt selector is not consulted: the
    /// password prompt it may ask for is shown only to applications the
    /// entry does not trust, and a trusted one is let in without any.
    fn lets_the_tool_decrypt(&self) -> bool {
        self.authorizes(&["ACLAuthorizationDecrypt", "ACLAuthorizationAny"])
            && self
                .applications
                .as_ref()
                .map_or(true, |apps| apps.iter().any(|app| app == SECURITY_TOOL))
    }

    /// Whether this is the partition entry and its list holds `apple-tool:`.
    /// A description that is all hex is decoded first; any other is read as
    /// it is.
    fn partitions_include_apple_tool(&self) -> bool {
        let Some(description) = &self.description else {
            return false;
        };
        if !self.authorizes(&["ACLAuthorizationPartitionID"]) {
            return false;
        }
        let text = description.trim();
        match decode_hex(text.as_bytes()) {
            Some(bytes) => String::from_utf8_lossy(&bytes).contains("apple-tool:"),
            None => text.contains("apple-tool:"),
        }
    }
}

/// `kSecUnlockStateStatus`: the keychain's status flags say it is unlocked.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const UNLOCK_STATE_STATUS: u32 = 0x1;

/// Whether `SecKeychainGetStatus` flags say the keychain is unlocked. A
/// readable but locked keychain (0x2) is not.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn keychain_unlocked(status_flags: u32) -> bool {
    status_flags & UNLOCK_STATE_STATUS != 0
}

/// Whether the item's access list lets the `security` tool read it without
/// a prompt: some entry lets the tool decrypt
/// ([`AclSummary::lets_the_tool_decrypt`]) and the partition entry lists
/// `apple-tool:`. Anything less, an empty list included, is no.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn fallback_permitted(acls: &[AclSummary]) -> bool {
    acls.iter().any(AclSummary::lets_the_tool_decrypt)
        && acls.iter().any(AclSummary::partitions_include_apple_tool)
}

#[cfg(target_os = "macos")]
static READING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Set when a secret read through the tool timed out: the fallback is not
/// tried again in this process until the person turns cloud sessions on
/// ([`ask_access`]).
#[cfg(target_os = "macos")]
static FALLBACK_SUPPRESSED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Claude Code files its item under the login user name.
#[cfg(target_os = "macos")]
fn account() -> Option<String> {
    if let Some(user) = std::env::var("USER").ok().filter(|u| !u.is_empty()) {
        return Some(user);
    }
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.name)
}

/// Whether the item exists, by its attributes only: never reads the secret,
/// so it never prompts.
#[cfg(target_os = "macos")]
pub fn present() -> bool {
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    let Some(account) = account() else {
        return false;
    };
    ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(SERVICE)
        .account(&account)
        .load_attributes(true)
        .limit(Limit::Max(1))
        .search()
        .is_ok_and(|found| !found.is_empty())
}

/// The item's secret: Claude Code's credentials JSON. A quiet read
/// (`interactive` false) never shows macOS's prompt; it returns an error unless
/// the person already chose Always Allow. An interactive read waits until the
/// person answers the prompt.
#[cfg(target_os = "macos")]
pub fn read(interactive: bool) -> Result<Vec<u8>, ReadError> {
    let account = account().ok_or_else(ReadError::unknown)?;
    // Held across the fallback too: one read of the item at a time.
    let _one = READING.lock().unwrap_or_else(|e| e.into_inner());
    let read = {
        // `kSecUseAuthenticationUI` does not cover another app's item's access
        // list; turning user interaction off does.
        let _quiet = if interactive {
            None
        } else {
            Some(
                SecKeychain::disable_user_interaction().map_err(|error| ReadError {
                    code: Some(error.code()),
                    locked: false,
                })?,
            )
        };
        security_framework::passwords::generic_password(
            security_framework::passwords::PasswordOptions::new_generic_password(SERVICE, &account),
        )
        .map_err(|error| ReadError {
            code: Some(error.code()),
            locked: false,
        })
    };
    match read {
        Err(error)
            if fallback_allowed(
                error.code,
                FALLBACK_SUPPRESSED.load(std::sync::atomic::Ordering::Acquire),
                agent_relay_node::relay::claude_dirs::cloud_sessions_on_here,
            ) =>
        {
            read_with_fallback(&account, error)
        }
        other => other,
    }
}

/// The `security` tool fallback for a read refused with -25293. It resolves
/// one keychain, the login keychain ([`login_keychain`]) opened by path, and
/// every later step uses that one: its lock state, the item whose access
/// list is checked and the path the tool reads from. It runs only while
/// that keychain is unlocked ([`keychain_unlocked`]). The tool asks for the
/// keychain password when it is locked, so a locked keychain never reaches
/// it: the read fails with `error` marked `locked`. A login keychain that
/// is missing, cannot be opened or whose status cannot be read is not tried
/// either, and `error` is returned as it is.
#[cfg(target_os = "macos")]
fn read_with_fallback(account: &str, error: ReadError) -> Result<Vec<u8>, ReadError> {
    let Some(path) = login_keychain() else {
        return Err(error);
    };
    let Ok(keychain) = SecKeychain::open(&path) else {
        return Err(error);
    };
    match acl::keychain_status(&keychain) {
        Some(flags) if keychain_unlocked(flags) => {
            read_with_security_tool(account, &keychain, &path).ok_or(error)
        }
        Some(_) => Err(ReadError {
            locked: true,
            ..error
        }),
        None => Err(error),
    }
}

/// The item's secret through `/usr/bin/security`, the `apple-tool:` path
/// the item's own ACL permits, read from `keychain`, the login keychain the
/// caller opened, at `path`, the same keychain by path. Runs the tool only
/// after [`fallback_permitted`] found, in the access list [`item_acls`]
/// read from that keychain without any possibility of a prompt, that the
/// item lets the tool read it without a dialog. Running without stdin and
/// in a session of its own only keeps the tool off a terminal; the kill at
/// [`FALLBACK_TIMEOUT`] is the backstop that also closes any dialog
/// `securityd` showed for it, and a timeout latches the fallback off until
/// [`ask_access`] runs. `None` when the precheck fails or the tool fails,
/// prints nothing or times out. Its output is never logged and its error
/// output is discarded.
#[cfg(target_os = "macos")]
fn read_with_security_tool(account: &str, keychain: &SecKeychain, path: &Path) -> Option<Vec<u8>> {
    let path = path.to_str()?;
    let acls = item_acls(account, keychain)?;
    if !fallback_permitted(&acls) {
        return None;
    }
    let out = run_security_tool(
        &[
            "find-generic-password",
            "-s",
            SERVICE,
            "-a",
            account,
            "-w",
            path,
        ],
        FALLBACK_TIMEOUT,
    )
    .ok()?;
    secret_from_tool_output(out)
}

/// The access list of the item `SERVICE`/`account` in `keychain` only,
/// read through the Security API with user interaction turned off, as the
/// quiet read does: the item is found by reference, never decrypted, and
/// its secret is never read. `None` when the item is not found exactly once
/// in that keychain or any call fails, so the fallback is skipped (fails
/// closed).
#[cfg(target_os = "macos")]
fn item_acls(account: &str, keychain: &SecKeychain) -> Option<Vec<AclSummary>> {
    let _quiet = SecKeychain::disable_user_interaction().ok()?;
    let item = acl::find_item(account, keychain)?;
    let access = acl::copy_access(&item)?;
    acl::copy_acl_list(&access)?
        .iter()
        .map(|entry| acl::summary(&entry))
        .collect()
}

/// The Security framework's access list calls, which `security-framework`
/// does not bind. Each foreign call sits in a function of its own. Every
/// `Copy` result is owned under the create rule and released on drop.
#[cfg(target_os = "macos")]
mod acl {
    use super::{AclSummary, SERVICE};
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, CFTypeRef, OSStatus, TCFType};
    use core_foundation::data::{CFData, CFDataRef};
    use core_foundation::string::{CFString, CFStringRef};
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::os::macos::keychain_item::SecKeychainItem;
    use security_framework_sys::base::{SecKeychainItemRef, SecKeychainRef};

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecKeychainItemCopyAccess(item: SecKeychainItemRef, access: *mut CFTypeRef) -> OSStatus;
        fn SecAccessCopyACLList(access: CFTypeRef, acls: *mut CFArrayRef) -> OSStatus;
        fn SecACLCopyAuthorizations(acl: CFTypeRef) -> CFArrayRef;
        fn SecACLCopyContents(
            acl: CFTypeRef,
            applications: *mut CFArrayRef,
            description: *mut CFStringRef,
            prompt_selector: *mut u16,
        ) -> OSStatus;
        fn SecTrustedApplicationCopyData(app: CFTypeRef, data: *mut CFDataRef) -> OSStatus;
        fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> OSStatus;
    }

    /// `keychain`'s status flags (`SecKeychainGetStatus`); `None` when its
    /// status cannot be had. The call does not prompt.
    pub(super) fn keychain_status(keychain: &SecKeychain) -> Option<u32> {
        let mut flags: u32 = 0;
        // SAFETY: `keychain` is a live SecKeychainRef for the call, and
        // `flags` is a valid out pointer. Nothing is retained.
        let status = unsafe { SecKeychainGetStatus(keychain.as_concrete_TypeRef(), &mut flags) };
        (status == 0).then_some(flags)
    }

    /// The item `SERVICE`/`account` by reference, found exactly once in
    /// `keychain`; no other keychain on the search list is looked at. A
    /// reference carries no secret, so the search decrypts nothing.
    pub(super) fn find_item(account: &str, keychain: &SecKeychain) -> Option<SecKeychainItem> {
        use security_framework::item::{
            ItemClass, ItemSearchOptions, Limit, Reference, SearchResult,
        };
        let mut found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(SERVICE)
            .account(account)
            .keychains(std::slice::from_ref(keychain))
            .load_refs(true)
            .limit(Limit::Max(1))
            .search()
            .ok()?;
        if found.len() != 1 {
            return None;
        }
        match found.pop()? {
            SearchResult::Ref(Reference::KeychainItem(item)) => Some(item),
            _ => None,
        }
    }

    /// The item's `SecAccess`.
    pub(super) fn copy_access(item: &SecKeychainItem) -> Option<CFType> {
        let mut access: CFTypeRef = std::ptr::null();
        // SAFETY: `item` is a live SecKeychainItemRef for the call, and
        // `access` is a valid out pointer. A non-NULL result is a new
        // reference (create rule), owned by the returned CFType.
        let (status, access) = unsafe {
            let status = SecKeychainItemCopyAccess(item.as_concrete_TypeRef(), &mut access);
            (
                status,
                (!access.is_null()).then(|| CFType::wrap_under_create_rule(access)),
            )
        };
        if status != 0 {
            return None;
        }
        access
    }

    /// The access list of a `SecAccess`: its `SecACL` entries.
    pub(super) fn copy_acl_list(access: &CFType) -> Option<CFArray<CFType>> {
        let mut list: CFArrayRef = std::ptr::null();
        // SAFETY: `access` is a live SecAccessRef for the call, and `list`
        // is a valid out pointer. A non-NULL result is a new array (create
        // rule), owned by the returned CFArray.
        let (status, list) = unsafe {
            let status = SecAccessCopyACLList(access.as_CFTypeRef(), &mut list);
            (
                status,
                (!list.is_null()).then(|| CFArray::<CFType>::wrap_under_create_rule(list)),
            )
        };
        if status != 0 {
            return None;
        }
        list
    }

    /// An entry's authorizations, `ACLAuthorizationDecrypt` and the like.
    fn copy_authorizations(entry: &CFType) -> Option<CFArray<CFType>> {
        // SAFETY: `entry` is a live SecACLRef for the call. A non-NULL
        // result is a new array (create rule), owned by the returned
        // CFArray.
        unsafe {
            let authorizations = SecACLCopyAuthorizations(entry.as_CFTypeRef());
            (!authorizations.is_null())
                .then(|| CFArray::<CFType>::wrap_under_create_rule(authorizations))
        }
    }

    /// An entry's contents: its trusted applications (`None` when it trusts
    /// every application) and its description. The API requires an out
    /// pointer for the prompt selector too; its value is not used, since the
    /// prompt applies only to applications the entry does not trust.
    fn copy_contents(entry: &CFType) -> Option<(Option<CFArray<CFType>>, Option<CFString>)> {
        let mut applications: CFArrayRef = std::ptr::null();
        let mut description: CFStringRef = std::ptr::null();
        let mut selector: u16 = 0;
        // SAFETY: `entry` is a live SecACLRef for the call, and the three
        // out pointers are valid. Non-NULL arrays and strings are new
        // references (create rule), owned by the returned wrappers.
        let (status, applications, description) = unsafe {
            let status = SecACLCopyContents(
                entry.as_CFTypeRef(),
                &mut applications,
                &mut description,
                &mut selector,
            );
            (
                status,
                (!applications.is_null())
                    .then(|| CFArray::<CFType>::wrap_under_create_rule(applications)),
                (!description.is_null()).then(|| CFString::wrap_under_create_rule(description)),
            )
        };
        if status != 0 {
            return None;
        }
        Some((applications, description))
    }

    /// A trusted application's path, without the NUL the data may end in.
    fn application_path(app: &CFType) -> Option<String> {
        let mut data: CFDataRef = std::ptr::null();
        // SAFETY: `app` is a live SecTrustedApplicationRef for the call, and
        // `data` is a valid out pointer. A non-NULL result is a new
        // reference (create rule), owned by the returned CFData.
        let (status, data) = unsafe {
            let status = SecTrustedApplicationCopyData(app.as_CFTypeRef(), &mut data);
            (
                status,
                (!data.is_null()).then(|| CFData::wrap_under_create_rule(data)),
            )
        };
        if status != 0 {
            return None;
        }
        let data = data?;
        let bytes = data.bytes();
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// The strings in an array of `CFString`s; `None` if any is not one.
    fn strings(array: &CFArray<CFType>) -> Option<Vec<String>> {
        array
            .iter()
            .map(|item| item.downcast::<CFString>().map(|text| text.to_string()))
            .collect()
    }

    /// One access list entry as an [`AclSummary`]; `None` if any part of it
    /// cannot be read.
    pub(super) fn summary(entry: &CFType) -> Option<AclSummary> {
        let authorizations = strings(&copy_authorizations(entry)?)?;
        let (applications, description) = copy_contents(entry)?;
        let applications = match applications {
            Some(apps) => Some(
                apps.iter()
                    .map(|app| application_path(&app))
                    .collect::<Option<Vec<_>>>()?,
            ),
            None => None,
        };
        Some(AclSummary {
            authorizations,
            applications,
            description: description.map(|text| text.to_string()),
        })
    }
}

/// The login keychain, where Claude Code keeps the item.
#[cfg(target_os = "macos")]
fn login_keychain() -> Option<std::path::PathBuf> {
    let home = nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.dir)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(std::path::PathBuf::from)
        })?;
    let path = home.join("Library/Keychains/login.keychain-db");
    path.is_file().then_some(path)
}

/// A failed tool run, latching the fallback off when [`latch_for`] says so.
#[cfg(target_os = "macos")]
fn tool_failed(timed_out: bool) -> ToolFailure {
    let failure = if timed_out {
        ToolFailure::TimedOut
    } else {
        ToolFailure::Failed
    };
    if latch_for(failure) {
        FALLBACK_SUPPRESSED.store(true, std::sync::atomic::Ordering::Release);
    }
    failure
}

/// Starts the command in a session of its own. A session of its own has no
/// controlling terminal, so the tool cannot ask for a password on one
/// either. That is all it does: it does not stop `securityd` from showing a
/// dialog.
#[cfg(target_os = "macos")]
fn own_session(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and touches no parent state.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(drop)
                .map_err(|errno| std::io::Error::from_raw_os_error(errno as i32))
        });
    }
}

/// Runs [`SECURITY_TOOL`] with `args` and returns its standard output, or
/// how it failed. Killed after `timeout`; a timeout latches the fallback
/// off ([`latch_for`]).
#[cfg(target_os = "macos")]
fn run_security_tool(args: &[&str], timeout: std::time::Duration) -> Result<Vec<u8>, ToolFailure> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut command = Command::new(SECURITY_TOOL);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    own_session(&mut command);
    let Ok(mut child) = command.spawn() else {
        return Err(tool_failed(false));
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(tool_failed(false));
    };
    // Drained on a thread of its own so a full pipe never holds the tool.
    let reader = std::thread::Builder::new()
        .name("claude-keychain-tool".into())
        .spawn(move || {
            let mut out = Vec::new();
            let read = stdout.read_to_end(&mut out);
            (read, out)
        });
    let Ok(reader) = reader else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(tool_failed(false));
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let timed_out = status.is_none();
    // The tool has exited or been killed, so its end of the pipe is closed
    // and the reader finishes.
    let Ok((read, mut out)) = reader.join() else {
        return Err(tool_failed(timed_out));
    };
    if read.is_ok() && status.is_some_and(|status| status.success()) {
        return Ok(out);
    }
    out.fill(0);
    Err(tool_failed(timed_out))
}

/// Asks macOS for access to the item now, on a thread of its own, when it
/// exists and a quiet read is refused: the person just turned Claude Code
/// cloud sessions on and is at the screen to answer. At most one ask is
/// pending; `wanted` is checked again just before the prompt, so a person
/// who turned the setting back off meanwhile is not asked. The secret is
/// dropped unused; only the person's answer matters. It runs when the
/// person turns cloud sessions on, so it first clears a fallback latched
/// off by a timeout.
#[cfg(target_os = "macos")]
pub fn ask_access(wanted: fn() -> bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    FALLBACK_SUPPRESSED.store(false, Ordering::Release);
    static ASKING: AtomicBool = AtomicBool::new(false);
    /// Lets the next ask start when dropped, also when the ask panicked or
    /// its thread never started.
    struct Asking;
    impl Drop for Asking {
        fn drop(&mut self) {
            ASKING.store(false, Ordering::Release);
        }
    }
    if ASKING.swap(true, Ordering::AcqRel) {
        return;
    }
    let asking = Asking;
    let _ = std::thread::Builder::new()
        .name("claude-keychain-ask".into())
        .spawn(move || {
            let _asking = asking;
            if present() && read(false).map(|mut secret| secret.fill(0)).is_err() && wanted() {
                if let Ok(mut secret) = read(true) {
                    secret.fill(0);
                }
            }
        });
}

/// No Keychain off macOS: Claude Code keeps its login in its credentials
/// file there.
#[cfg(not(target_os = "macos"))]
pub fn ask_access(_wanted: fn() -> bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_output_loses_its_trailing_newline() {
        assert_eq!(
            secret_from_tool_output(b"{\"claudeAiOauth\":{}}\n".to_vec()),
            Some(b"{\"claudeAiOauth\":{}}".to_vec())
        );
        assert_eq!(
            secret_from_tool_output(b"secret\r\n".to_vec()),
            Some(b"secret".to_vec())
        );
        assert_eq!(
            secret_from_tool_output(b"secret".to_vec()),
            Some(b"secret".to_vec())
        );
        // Only the one newline the tool adds is removed.
        assert_eq!(
            secret_from_tool_output(b"a\n\n".to_vec()),
            Some(b"a\n".to_vec())
        );
    }

    #[test]
    fn empty_tool_output_is_no_secret() {
        assert_eq!(secret_from_tool_output(Vec::new()), None);
        assert_eq!(secret_from_tool_output(b"\n".to_vec()), None);
    }

    #[test]
    fn only_a_timeout_of_the_secret_read_latches_the_fallback() {
        assert!(latch_for(ToolFailure::TimedOut));
        assert!(!latch_for(ToolFailure::Failed));
    }

    #[test]
    fn raw_single_line_json_is_returned_as_is() {
        let json = br#"{"claudeAiOauth":{"accessToken":"a","expiresAt":1}}"#;
        let mut out = json.to_vec();
        out.push(b'\n');
        assert_eq!(secret_from_tool_output(out), Some(json.to_vec()));
    }

    #[test]
    fn hex_output_of_a_multi_line_secret_is_decoded() {
        let json = b"{\n  \"claudeAiOauth\": {}\n}\n";
        let mut out: Vec<u8> = json
            .iter()
            .flat_map(|b| format!("{b:02x}").into_bytes())
            .collect();
        out.push(b'\n');
        assert_eq!(secret_from_tool_output(out), Some(json.to_vec()));
        let upper: Vec<u8> = json
            .iter()
            .flat_map(|b| format!("{b:02X}").into_bytes())
            .collect();
        assert_eq!(secret_from_tool_output(upper), Some(json.to_vec()));
    }

    #[test]
    fn odd_length_or_non_hex_output_is_returned_raw() {
        assert_eq!(
            secret_from_tool_output(b"abc\n".to_vec()),
            Some(b"abc".to_vec())
        );
        assert_eq!(
            secret_from_tool_output(b"7b0\n".to_vec()),
            Some(b"7b0".to_vec())
        );
        assert_eq!(
            secret_from_tool_output(b"7b0g\n".to_vec()),
            Some(b"7b0g".to_vec())
        );
    }

    #[test]
    fn the_fallback_needs_the_partition_error_no_latch_and_cloud_sessions() {
        assert!(fallback_allowed(Some(PARTITION_DENIED), false, || true));
        assert!(!fallback_allowed(Some(PARTITION_DENIED), true, || true));
        assert!(!fallback_allowed(Some(PARTITION_DENIED), false, || false));
        assert!(!fallback_allowed(Some(-25308), false, || true));
        assert!(!fallback_allowed(None, false, || true));
        // While latched, the cloud setting is not even consulted.
        assert!(!fallback_allowed(Some(PARTITION_DENIED), true, || panic!(
            "asked while latched"
        )));
    }

    fn hex(text: &str) -> String {
        text.bytes().map(|b| format!("{b:02X}")).collect()
    }

    fn partition_plist(ids: &[&str]) -> String {
        let strings: String = ids
            .iter()
            .map(|id| format!("\t\t<string>{id}</string>\n"))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\
             \t<key>Partitions</key>\n\t<array>\n{strings}\t</array>\n</dict>\n</plist>\n"
        )
    }

    const HELPER: &str = "/Applications/Agent Relay.app/Contents/MacOS/agent-relay-probe";

    fn acl(authorizations: &[&str]) -> AclSummary {
        AclSummary {
            authorizations: authorizations.iter().map(|a| a.to_string()).collect(),
            applications: None,
            description: Some(SERVICE.to_string()),
        }
    }

    fn decrypt(applications: Option<&[&str]>) -> AclSummary {
        AclSummary {
            applications: applications.map(|apps| apps.iter().map(|a| a.to_string()).collect()),
            ..acl(&[
                "ACLAuthorizationDecrypt",
                "ACLAuthorizationDerive",
                "ACLAuthorizationExportClear",
                "ACLAuthorizationExportWrapped",
                "ACLAuthorizationMAC",
                "ACLAuthorizationSign",
            ])
        }
    }

    fn partition(description: String) -> AclSummary {
        AclSummary {
            description: Some(description),
            ..acl(&["ACLAuthorizationPartitionID"])
        }
    }

    /// The item as Claude Code leaves it after a refresh, as the Security
    /// API reported it on a Mac: five entries; decrypt trusts the helper (the
    /// person's Always Allow) and the tool; the partition entry's
    /// description is a hex-encoded plist holding only `apple-tool:`.
    fn real_item(
        decrypt_entry: AclSummary,
        partition_entry: Option<AclSummary>,
    ) -> Vec<AclSummary> {
        let mut acls = vec![acl(&["ACLAuthorizationEncrypt"]), decrypt_entry];
        acls.push(acl(&["ACLAuthorizationIntegrity"]));
        acls.extend(partition_entry);
        acls.push(AclSummary {
            applications: Some(Vec::new()),
            ..acl(&["ACLAuthorizationChangeACL"])
        });
        acls
    }

    fn real_partition() -> Option<AclSummary> {
        Some(partition(hex(&partition_plist(&["apple-tool:"]))))
    }

    #[test]
    fn the_item_claude_code_wrote_permits_the_tool() {
        let acls = real_item(decrypt(Some(&[HELPER, SECURITY_TOOL])), real_partition());
        assert!(fallback_permitted(&acls));
        // The order of the entries does not matter.
        let reversed: Vec<AclSummary> = acls.into_iter().rev().collect();
        assert!(fallback_permitted(&reversed));
        // A plain-text plist with more partitions permits it too.
        let acls = real_item(
            decrypt(Some(&[HELPER, SECURITY_TOOL])),
            Some(partition(partition_plist(&["apple-tool:", "apple:"]))),
        );
        assert!(fallback_permitted(&acls));
    }

    #[test]
    fn only_an_unlocked_keychain_reaches_the_tool() {
        assert!(!keychain_unlocked(0x2));
        assert!(!keychain_unlocked(0));
        assert!(keychain_unlocked(0x3));
        assert!(keychain_unlocked(0x7));
    }

    #[test]
    fn a_decrypt_entry_trusting_every_application_permits_the_tool() {
        let acls = real_item(
            decrypt(None),
            Some(partition(partition_plist(&["apple-tool:"]))),
        );
        assert!(fallback_permitted(&acls));
        let any = AclSummary {
            applications: None,
            ..acl(&["ACLAuthorizationAny"])
        };
        assert!(fallback_permitted(&[
            any,
            partition(partition_plist(&["apple-tool:"]))
        ]));
    }

    #[test]
    fn a_hex_encoded_partition_list_is_decoded() {
        let acls = real_item(
            decrypt(Some(&[SECURITY_TOOL])),
            Some(partition(hex(&partition_plist(&["apple-tool:", "apple:"])))),
        );
        assert!(fallback_permitted(&acls));
        let without = real_item(
            decrypt(Some(&[SECURITY_TOOL])),
            Some(partition(hex(&partition_plist(&["teamid:QUJ7SA6X8X"])))),
        );
        assert!(!fallback_permitted(&without));
    }

    #[test]
    fn anything_short_of_both_facts_skips_the_fallback() {
        let tool = real_partition;
        // No partition entry at all.
        assert!(!fallback_permitted(&real_item(
            decrypt(Some(&[HELPER, SECURITY_TOOL])),
            None
        )));
        // A partition list without apple-tool:.
        assert!(!fallback_permitted(&real_item(
            decrypt(Some(&[HELPER, SECURITY_TOOL])),
            Some(partition(partition_plist(&["apple:", "teamid:QUJ7SA6X8X"])))
        )));
        // apple-tool: in a description that is not the partition entry's.
        let mut misplaced = real_item(decrypt(Some(&[SECURITY_TOOL])), None);
        misplaced[0].description = Some(partition_plist(&["apple-tool:"]));
        assert!(!fallback_permitted(&misplaced));
        // A partition entry without a description.
        let mut blank = partition(String::new());
        blank.description = None;
        assert!(!fallback_permitted(&real_item(
            decrypt(Some(&[SECURITY_TOOL])),
            Some(blank)
        )));
        // The tool not trusted for decrypt.
        assert!(!fallback_permitted(&real_item(
            decrypt(Some(&[HELPER])),
            tool()
        )));
        assert!(!fallback_permitted(&real_item(decrypt(Some(&[])), tool())));
        // An entry's prompt selector is not part of the summary at all: a
        // decrypt entry that trusts the tool permits it whether or not the
        // entry asks other applications for the password
        // (the_item_claude_code_wrote_permits_the_tool), and one that does
        // not trust it is refused either way (the two cases just above).
        // The tool trusted for something other than decrypt.
        let encrypt_only = AclSummary {
            applications: Some(vec![SECURITY_TOOL.to_string()]),
            ..acl(&["ACLAuthorizationEncrypt"])
        };
        assert!(!fallback_permitted(&[encrypt_only, tool().unwrap()]));
        // Nothing read.
        assert!(!fallback_permitted(&[]));
    }
}
