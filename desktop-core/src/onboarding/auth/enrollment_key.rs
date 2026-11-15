//! Signing in with an enrollment key an administrator issued, for unattended
//! installs. The key never passes through this process's arguments or output:
//! setup hands the core a file path and the core reads the file itself. A site
//! that does not take enrollment keys falls back to device approval.
use super::*;
use std::io::Read;
use std::path::Path;

mod stdin_copy;
pub(in crate::onboarding) use stdin_copy::StdinCopy;

/// The core's phases for an enrollment-key sign-in (`account::enrollment_key`).
const EXCHANGING: &str = "exchanging_enrollment_key";
const REJECTED: &str = "enrollment_key_rejected";
const UNSUPPORTED: &str = "enrollment_key_unsupported";
const BUSY: &str = "enrollment_key_busy";
/// The longest key read from stdin; the core refuses anything longer.
const MAX_KEY_BYTES: u64 = 4096;

/// Points `options` at a file holding the key. A path is made absolute for
/// the core; `-` reads the key from stdin into a private temporary file that
/// is deleted once the core has read it, when the returned handle drops, or
/// when setup is interrupted. Reading stdin also turns prompts off, since
/// stdin is spent.
pub(in crate::onboarding) fn prepare(options: &mut Options) -> Result<Option<StdinCopy>, Failure> {
    prepare_from(options, || io::stdin().lock())
}

/// [`prepare`], reading `-` from what `stdin` opens.
pub(in crate::onboarding) fn prepare_from<R: Read>(
    options: &mut Options,
    stdin: impl FnOnce() -> R,
) -> Result<Option<StdinCopy>, Failure> {
    let Some(path) = options.enrollment_key_file.take() else {
        return Ok(None);
    };
    if path.as_os_str() != "-" {
        let path = std::path::absolute(&path).map_err(|_| unreadable())?;
        options.enrollment_key_file = Some(path);
        return Ok(None);
    }
    options.non_interactive = true;
    // Interruptions are watched before the copy exists, so none can leave it.
    stdin_copy::watch();
    let file = stdin_file(stdin())?;
    options.enrollment_key_file = Some(file.path().to_path_buf());
    Ok(Some(file))
}

/// Copies the key from `input` into a new temporary file only this user can
/// read (mode 0600 on Unix; on Windows, the user's own temp directory). The
/// file is kept as a copy, where an interruption removes it, before the key
/// is written into it.
fn stdin_file(input: impl Read) -> Result<StdinCopy, Failure> {
    let mut key = Vec::new();
    input
        .take(MAX_KEY_BYTES + 1)
        .read_to_end(&mut key)
        .map_err(|_| unreadable())?;
    if key.iter().all(u8::is_ascii_whitespace) || key.len() as u64 > MAX_KEY_BYTES {
        return Err(unreadable());
    }
    let file = tempfile::Builder::new()
        .prefix("agent-relay-enrollment-")
        .tempfile()
        .map_err(|_| unreadable())?;
    let mut writer = file.as_file().try_clone().map_err(|_| unreadable())?;
    // Dropping the copy on a failed write removes the file.
    let copy = StdinCopy::keep(file);
    writer.write_all(&key).map_err(|_| unreadable())?;
    writer.flush().map_err(|_| unreadable())?;
    Ok(copy)
}

fn unreadable() -> Failure {
    Failure {
        code: "enrollment_key_unreadable",
        message: "Setup could not read the enrollment key. Pass --enrollment-key-file with a file \
                  holding only the key, or - to read it from stdin.",
    }
}

/// The first sign-in of this run: with the enrollment key when one was
/// given and this computer is not signed in, otherwise device approval.
/// Answers the account and whether the sign-in uses the key.
pub(super) fn sign_in_first(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
    account: &Value,
) -> Result<(Value, bool), Failure> {
    let acknowledge = options.acknowledge_uninspected_schedules;
    let Some(file) = options.enrollment_key_file.as_deref() else {
        return Ok((sign_in(core, options, acknowledge)?, false));
    };
    // A signed-in computer that needs re-approval renews by device approval.
    if account["state"] == "signed_in" {
        unused(ui, options);
        return Ok((sign_in(core, options, acknowledge)?, false));
    }
    ui.event(
        "enrollment_key",
        json!({"message":"Signing in with the enrollment key."}),
    );
    Ok((with_key(core, options, file, acknowledge)?, true))
}

/// Starts the sign-in with the key in `file`. The core reads the key before
/// it answers and never reads the file again, so a copy of stdin is deleted
/// as soon as the call returns, whatever it answered.
fn with_key(
    core: &mut impl Backend,
    options: &Options,
    file: &Path,
    acknowledge: bool,
) -> Result<Value, Failure> {
    let started = call(
        core,
        "account.sign_in",
        json!({"flow":"enrollment_key", "enrollment_key_file":file, "workspace":options.workspace,
               "sharing":"selected", "acknowledge_uninspected_schedules":acknowledge}),
    );
    stdin_copy::remove(file);
    started
}

/// Says so when setup leaves a given key alone because this computer is
/// already signed in or a sign-in is already under way.
pub(super) fn unused(ui: &mut impl Interface, options: &Options) {
    if options.enrollment_key_file.is_some() {
        ui.event(
            "enrollment_key_unused",
            json!({"message":"This computer is already signed in or signing in, so the enrollment key was not used."}),
        );
    }
}

/// How an enrollment-key sign-in moves setup on, or `None` when the account
/// is not in one. The sign-in is waited for even without `--wait`, within
/// `--timeout`: the exchange, then the workspace connection that follows it,
/// until the account is signed in or the attempt fails. A site without
/// enrollment keys gets a device sign-in instead, which this run does not
/// wait for even with `--wait`: setup prints the device instructions and
/// exits 2, so an installer is not held up for a person's approval.
pub(super) fn advance(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
    account: &mut Value,
    wait: &mut Wait,
) -> Result<Option<Progress>, Failure> {
    let phase = account["phase"].as_str().unwrap_or_default();
    // One already exchanging when this run started is waited for too.
    wait.keyed |= phase == EXCHANGING;
    if wait.keyed && account["state"] == "preparing" && phase != "selecting_workspace" {
        let waiting = core.elapsed() < Duration::from_secs(options.timeout);
        return Ok(waiting.then_some(Progress::Waiting));
    }
    if account["state"] != "error" {
        return Ok(None);
    }
    match phase {
        UNSUPPORTED => {
            ui.event(
                "enrollment_key_unsupported",
                json!({"message":"This Agent Relay site does not accept enrollment keys. Approve this server with a device code instead."}),
            );
            *account = sign_in(core, options, wait.acknowledged)?;
            wait.enabled = false;
            wait.keyed = false;
            Ok(Some(Progress::Restarted))
        }
        REJECTED => Err(Failure {
            code: REJECTED,
            message: "The enrollment key was not accepted. Ask your administrator for a new key, \
                      then rerun setup.",
        }),
        BUSY => Err(Failure {
            code: BUSY,
            message:
                "Agent Relay Cloud is busy. Rerun setup with the same enrollment key in a few \
                      minutes.",
        }),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_from_stdin_lands_in_a_private_file() {
        let file = stdin_file(&b"sh_ek_synthetic-key\n"[..]).unwrap();
        let read = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(read, "sh_ek_synthetic-key\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let path = file.path().to_path_buf();
        drop(file);
        assert!(!path.exists());
    }

    #[test]
    fn empty_or_oversized_stdin_is_refused_without_a_file() {
        assert_eq!(
            stdin_file(&b" \n"[..]).map(|_| ()).unwrap_err().code,
            "enrollment_key_unreadable"
        );
        let long = vec![b'k'; MAX_KEY_BYTES as usize + 1];
        assert!(stdin_file(&long[..]).is_err());
    }

    #[test]
    fn a_relative_key_path_is_made_absolute_for_the_core() {
        let mut options = options(&["--enrollment-key-file", "key.txt"]);
        assert!(prepare(&mut options).unwrap().is_none());
        let path = options.enrollment_key_file.unwrap();
        assert!(path.is_absolute());
        assert!(path.ends_with("key.txt"));
    }

    #[test]
    fn a_key_from_stdin_turns_prompts_off_and_points_at_a_private_copy() {
        let mut opts = options(&["--enrollment-key-file", "-"]);
        assert!(!opts.non_interactive);
        let file = prepare_from(&mut opts, || &b"sh_ek_synthetic-key\n"[..])
            .unwrap()
            .expect("a temporary file");
        assert!(opts.non_interactive, "stdin is spent");
        assert_eq!(
            opts.enrollment_key_file.as_deref(),
            Some(file.path()),
            "the core reads the copy"
        );
        assert!(file.path().is_absolute());
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            "sh_ek_synthetic-key\n"
        );
        let path = file.path().to_path_buf();
        drop(file);
        assert!(!path.exists());

        // Nothing on stdin: refused, no file, and stdin is still spent.
        let mut opts = options(&["--enrollment-key-file", "-"]);
        let failure = prepare_from(&mut opts, || &b""[..]).unwrap_err();
        assert_eq!(failure.code, "enrollment_key_unreadable");
        assert!(opts.non_interactive);

        // A path does not touch stdin.
        let mut opts = options(&["--enrollment-key-file", "/k"]);
        let untouched = prepare_from(&mut opts, || -> &[u8] { panic!("stdin read") });
        assert!(untouched.unwrap().is_none());
        assert!(!opts.non_interactive);
    }

    fn options(args: &[&str]) -> Options {
        Options::try_parse_from(std::iter::once("setup").chain(args.iter().copied())).unwrap()
    }
}
