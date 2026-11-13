//! The shipped binary serves with the private layer's plugins.
#![cfg(unix)]

use serde_json::Value;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PROBE: &str = env!("CARGO_BIN_EXE_agent-relay-desktop-probe");

#[test]
fn the_shipped_binary_serves_the_account_methods() {
    let home = tempfile::tempdir().unwrap();
    let socket = home.path().join("core.sock");
    // Nothing from this computer reaches the core: not its profile or site.
    let isolated = |command: &mut Command| {
        command
            .env_clear()
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("APPDATA", home.path().join("AppData/Roaming"))
            .env("LOCALAPPDATA", home.path().join("AppData/Local"))
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("XDG_DATA_HOME", home.path().join(".local/share"))
            .env("TMPDIR", home.path());
    };
    let mut command = Command::new(PROBE);
    isolated(&mut command);
    let mut serve = command
        .args(["serve", "--socket"])
        .arg(&socket)
        .arg("--app-data-dir")
        .arg(home.path().join("app-data"))
        .arg("--exit-on-stdin-eof")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let answer = loop {
        let mut command = Command::new(PROBE);
        isolated(&mut command);
        let call = command
            .args(["call", "uploader.sessions.preview", "--socket"])
            .arg(&socket)
            .arg("--json")
            .output()
            .unwrap();
        if call.status.success() {
            break serde_json::from_slice::<Value>(&call.stdout).unwrap();
        }
        assert!(Instant::now() < deadline, "{call:?}");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(answer["result"]["sessions"], serde_json::json!([]));
    drop(serve.stdin.take());
    serve.wait().unwrap();
}

#[test]
fn help_names_one_surface_and_every_listed_command_answers() {
    let help = Command::new(PROBE).arg("--help").output().unwrap();
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("agent-relay-probe"), "{text}");
    for name in agent_relay_desktop_core::command_line::listed() {
        assert!(text.contains(&name), "{name} in {text}");
        // Parsing a command's own help proves it exists; `setup`, the header
        // helpers and the attestation hook parse their arguments by hand.
        if ["setup", "mcp-headers", "feed-headers", "claude-attestation-hook"]
            .contains(&name.as_str())
        {
            continue;
        }
        let answer = Command::new(PROBE).args([&name, "--help"]).output().unwrap();
        assert!(answer.status.success(), "{name}: {answer:?}");
    }
}

#[test]
fn the_teammate_context_hook_prints_session_start_guidance_and_never_fails() {
    use std::io::Write;
    let run = |input: &[u8]| {
        let mut child = Command::new(PROBE)
            .args(["teammate-context-hook", "--socket", "/nonexistent.sock"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };
    let started = run(br#"{"hook_event_name":"SessionStart","source":"startup"}"#);
    assert!(started.status.success(), "{started:?}");
    let output: serde_json::Value = serde_json::from_slice(&started.stdout).unwrap();
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains("list_session_summaries"));
    let other = run(br#"{"hook_event_name":"PreToolUse"}"#);
    assert!(other.status.success() && other.stdout.is_empty(), "{other:?}");
    let garbage = run(b"not json");
    assert!(garbage.status.success(), "{garbage:?}");
}

#[test]
fn the_version_is_this_binary_s_and_the_earlier_relay_word_still_runs() {
    let version = Command::new(PROBE).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("agent-relay-probe "));
    // Hooks and header helpers earlier versions wrote keep running until
    // setup repair rewrites them: the helper's own commands and the core's.
    let home = tempfile::tempdir().unwrap();
    let hook = Command::new(PROBE)
        .args(["relay", "claude-attestation-hook", "--socket"])
        .arg(home.path().join("missing.sock"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(hook.status.success(), "{hook:?}");
    let headers = Command::new(PROBE)
        .args(["relay", "mcp-headers", "--site", "https://agentrelay.com"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&headers.stderr).contains("--workspace"),
        "{headers:?}"
    );
    for core in ["call", "codex-stop-hook"] {
        let answer = Command::new(PROBE)
            .args(["relay", core, "--help"])
            .output()
            .unwrap();
        assert!(answer.status.success(), "{core}: {answer:?}");
    }
    // A `relay` word before nothing the helper runs still fails loudly.
    let answer = Command::new(PROBE)
        .args(["relay", "unknown"])
        .output()
        .unwrap();
    assert!(!answer.status.success());
    assert!(
        String::from_utf8_lossy(&answer.stderr).contains("relay"),
        "{answer:?}"
    );
}

/// `feed verify` as the install scripts run it: a manifest signed at run
/// time with a throwaway key, read from a file and from stdin.
#[test]
fn feed_verify_answers_one_json_line_and_its_exit_status() {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Write;

    let manifest: Value = serde_json::from_str(include_str!(
        "fixtures/desktop-feed/manifest/valid-stable.json"
    ))
    .unwrap();
    let key = SigningKey::from_bytes(&[7; 32]);
    let canonical = agent_relay_desktop_core::feed::jcs::canonicalize(&manifest).unwrap();
    let signature = URL_SAFE_NO_PAD.encode(key.sign(&canonical).to_bytes());
    let public_key = STANDARD.encode(key.verifying_key().to_bytes());
    let mut tampered = manifest.clone();
    tampered["version"] = Value::from("2026.11.10");
    let home = tempfile::tempdir().unwrap();
    let write = |name: &str, manifest: &Value| {
        let path = home.path().join(name);
        let envelope = serde_json::json!({"manifest": manifest, "signature": signature, "key_id": "fk_x7k2q_k1"});
        std::fs::write(&path, envelope.to_string()).unwrap();
        path
    };
    let signed = write("signed.json", &manifest);
    let changed = write("tampered.json", &tampered);
    let verify = |extra: &[&str], file: Option<&std::path::Path>, stdin: Option<&[u8]>| {
        let mut command = Command::new(PROBE);
        command
            .args([
                "feed",
                "verify",
                "--profile",
                "x7k2q",
                "--public-key",
                &public_key,
            ])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(file) = file {
            command.arg(file);
        }
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(stdin.unwrap_or_default()).unwrap();
        drop(input);
        let output = child.wait_with_output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        (
            output.status.code(),
            serde_json::from_str::<Value>(&text).unwrap(),
        )
    };

    let (status, answer) = verify(&[], Some(&signed), None);
    assert_eq!(status, Some(0), "{answer}");
    assert_eq!(answer["ok"], Value::Bool(true));
    assert_eq!(answer["version"], "2026.11.9");
    // The same envelope on stdin, with - and with no file at all.
    let bytes = std::fs::read(&signed).unwrap();
    for dash in [&["-"][..], &[][..]] {
        let (status, answer) = verify(dash, None, Some(&bytes));
        assert_eq!(
            (status, &answer["ok"]),
            (Some(0), &Value::Bool(true)),
            "{answer}"
        );
    }
    // A changed manifest, another profile's or channel's, or an older
    // release than the installed one is refused with its code.
    for (extra, file, code) in [
        (&[][..], &changed, "signature_invalid"),
        (&["--channel", "beta"][..], &signed, "channel_mismatch"),
        (
            &["--installed-version", "2026.11.9"][..],
            &signed,
            "not_newer",
        ),
    ] {
        let (status, answer) = verify(extra, Some(file), None);
        assert_eq!(status, Some(1), "{code}: {answer}");
        assert_eq!(answer["ok"], Value::Bool(false));
        assert_eq!(answer["code"], code, "{answer}");
    }
    let other = Command::new(PROBE)
        .args([
            "feed",
            "verify",
            "--profile",
            "other",
            "--public-key",
            &public_key,
        ])
        .arg(&signed)
        .output()
        .unwrap();
    assert_eq!(other.status.code(), Some(1), "{other:?}");
    // A usage error is 2, and the help answers.
    let usage = Command::new(PROBE)
        .args(["feed", "verify", "--profile", "x7k2q"])
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2), "{usage:?}");
    let help = Command::new(PROBE)
        .args(["feed", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success(), "{help:?}");
}
