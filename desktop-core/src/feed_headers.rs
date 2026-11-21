//! `feed-headers`: prints the headers for the site's signed release feed, for
//! a headless install or a person checking the feed by hand. The feed routes
//! are Cloud's proposed contract (`/cloud/api/v1/desktop/feed/...`), not a
//! stable one. No credentials on disk and no independent login.
//!
//! A failure says which check failed in one line: the core could not be
//! reached (naming the socket or pipe tried), the core refused (its own fixed
//! text), or the core's answer was not understood. Nothing from a response
//! body or a header value is ever printed on stderr.

use agent_relay_node::relay::daemon::SafeError;
use agent_relay_node::relay::paths::{self, CorePaths};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FAILURE: &str =
    "Agent Relay could not supply release feed authentication. Open the app and check Account health.";

const NOT_UNDERSTOOD: &str =
    "Agent Relay's core gave an answer feed-headers does not understand. It may be an older or different core.";

/// The fixed messages of the errors `account.feed_headers` returns
/// (`InvalidParams` and `NotSignedIn`, `account/mcp_auth/feed_auth.rs`).
/// `cli::call` hands back only an error's message, not its kind, so these
/// exact texts are the allowlist: any other error keeps [`FAILURE`].
const CORE_REASONS: [&str; 4] = [
    "The site profile id must be 1 to 32 lowercase letters or digits.",
    "Sign in to the configured Agent Relay account and workspace first.",
    "Agent Relay needs you to sign in again.",
    "Agent Relay sign-in changed. Check for updates again.",
];

const USAGE: &str = "Usage: agent-relay-probe feed-headers --site <url> --workspace <id> \
--account <id> --profile <id> [--socket <path>] [--format json|lines]";

/// How the three headers are printed on stdout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// One JSON object and a newline; the installers parse this.
    Json,
    /// `Name: value` lines, for `curl -H @file`.
    Lines,
}

/// The core's socket or named pipe `cli::call` connects to, resolved the
/// same way, so a connection failure can name it.
fn core_socket(socket: Option<&Path>) -> Option<PathBuf> {
    let home = paths::home().ok()?;
    CorePaths::resolve(socket, |key| std::env::var(key).ok(), &home)
        .ok()
        .map(|paths| paths.socket)
}

/// One printable line for a failed call. `cli::call` reports a connection
/// failure and a core error both as a [`SafeError`]; the connection failure
/// is its own text naming `socket`, and a core error passes through only
/// when it is one of [`CORE_REASONS`].
fn reason(error: &anyhow::Error, socket: Option<&Path>) -> String {
    let Some(SafeError(text)) = error.downcast_ref::<SafeError>() else {
        return FAILURE.into();
    };
    if let Some(socket) = socket {
        let shown = socket.display();
        if *text == format!("Agent Relay core is not running at {shown}.")
            || *text == format!("{shown} is not served by your own Agent Relay core.")
        {
            return format!("Agent Relay's core is not reachable at {shown}.");
        }
    }
    match CORE_REASONS.iter().find(|known| **known == text.as_str()) {
        Some(known) => (*known).into(),
        None => FAILURE.into(),
    }
}

fn has_line_break(value: &str) -> bool {
    value.contains(['\r', '\n'])
}

fn headers(
    site: &str,
    workspace: &str,
    account: &str,
    profile: &str,
    socket: Option<&Path>,
    call: impl FnOnce(&str, Value) -> Result<Value>,
) -> Result<Map<String, Value>> {
    let response = call(
        "account.feed_headers",
        json!({
            "site_url": site, "workspace_id": workspace, "account_id": account,
            "profile_id": profile,
        }),
    )
    .map_err(|error| SafeError(reason(&error, socket)))?;
    let headers = response
        .get("headers")
        .and_then(Value::as_object)
        .filter(|headers| headers.len() == 3)
        .ok_or_else(|| SafeError(NOT_UNDERSTOOD.into()))?;
    if headers
        .values()
        .any(|value| value.as_str().is_some_and(has_line_break))
    {
        return Err(SafeError(FAILURE.into()).into());
    }
    let field = |name: &str| headers.get(name).and_then(Value::as_str);
    let bearer = field("Authorization").is_some_and(|value| {
        value
            .strip_prefix("Bearer ")
            .is_some_and(|token| !token.is_empty() && !token.chars().any(char::is_whitespace))
    });
    if !bearer
        || field("X-Agent-Relay-Profile") != Some(profile)
        || field("X-Agent-Relay-Desktop") != Some("1")
    {
        return Err(SafeError(NOT_UNDERSTOOD.into()).into());
    }
    Ok(Map::clone(headers))
}

/// The stdout bytes for `headers`. A value with a CR or LF is refused in
/// every format, so a line can never be split into another header.
fn render(headers: &Map<String, Value>, format: Format) -> Result<Vec<u8>> {
    let mut lines = String::new();
    for (name, value) in headers {
        let value = value
            .as_str()
            .filter(|value| !has_line_break(value) && !has_line_break(name))
            .ok_or_else(|| SafeError(FAILURE.into()))?;
        lines.push_str(&format!("{name}: {value}\n"));
    }
    match format {
        Format::Lines => Ok(lines.into_bytes()),
        Format::Json => {
            let mut bytes = serde_json::to_vec(&Value::Object(headers.clone()))
                .map_err(|_| SafeError(FAILURE.into()))?;
            bytes.push(b'\n');
            Ok(bytes)
        }
    }
}

/// Prints only the HTTP headers, never a bridge result wrapper, from the
/// same user-owned core socket or named pipe as the app.
fn print_at(
    socket: Option<&Path>,
    site: &str,
    workspace: &str,
    account: &str,
    profile: &str,
    format: Format,
) -> Result<()> {
    let tried = core_socket(socket);
    let headers = headers(
        site,
        workspace,
        account,
        profile,
        tried.as_deref(),
        |method, params| {
            agent_relay_node::relay::cli::call(socket, method, params, Duration::from_secs(30))
        },
    )?;
    let bytes = render(&headers, format)?;
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| SafeError(FAILURE.into()))?;
    Ok(())
}

/// A parsed command line.
#[derive(Debug, PartialEq, Eq)]
struct Request<'a> {
    site: &'a str,
    workspace: &'a str,
    account: &'a str,
    profile: &'a str,
    socket: Option<&'a Path>,
    format: Format,
}

/// The request, or the usage lines for stderr.
fn parse(arguments: &[String]) -> Result<Request<'_>, String> {
    let value = |name: &str| {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|at| arguments.get(at + 1))
    };
    let (Some(site), Some(workspace), Some(account), Some(profile)) = (
        value("--site"),
        value("--workspace"),
        value("--account"),
        value("--profile"),
    ) else {
        return Err(format!(
            "feed-headers needs --site, --workspace, --account and --profile.\n{USAGE}"
        ));
    };
    let format = if arguments.iter().any(|argument| argument == "--format") {
        match value("--format").map(String::as_str) {
            Some("json") => Format::Json,
            Some("lines") => Format::Lines,
            _ => return Err(format!("feed-headers --format is json or lines.\n{USAGE}")),
        }
    } else {
        Format::Json
    };
    Ok(Request {
        site,
        workspace,
        account,
        profile,
        socket: value("--socket").map(Path::new),
        format,
    })
}

/// `feed-headers --site <url> --workspace <id> --account <id> --profile <id>
/// [--socket <path>] [--format json|lines]`. Exits 0 on success, 1 when the
/// headers could not be supplied, 2 on a usage error.
pub fn run(arguments: &[String]) -> i32 {
    let request = match parse(arguments) {
        Ok(request) => request,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };
    match print_at(
        request.socket,
        request.site,
        request.workspace,
        request.account,
        request.profile,
        request.format,
    ) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOCKET: &str = "/fixture/core.sock";

    fn answer() -> Value {
        json!({"headers":{
            "Authorization":"Bearer fixture-only",
            "X-Agent-Relay-Profile":"x7k2q",
            "X-Agent-Relay-Desktop":"1",
        }, "ignored":"private"})
    }

    fn failing(error: anyhow::Error) -> String {
        headers(
            "site",
            "w",
            "a",
            "x7k2q",
            Some(Path::new(SOCKET)),
            |_, _| Err(error),
        )
        .unwrap_err()
        .to_string()
    }

    fn arguments(extra: &[&str]) -> Vec<String> {
        [
            "--site",
            "https://site.invalid",
            "--workspace",
            "w-1",
            "--account",
            "a-1",
            "--profile",
            "x7k2q",
        ]
        .iter()
        .chain(extra)
        .map(|argument| argument.to_string())
        .collect()
    }

    #[test]
    fn pins_and_profile_are_sent_and_only_headers_are_returned() {
        let result = headers(
            "https://agentrelay.com",
            "w-1",
            "a-1",
            "x7k2q",
            None,
            |method, params| {
                assert_eq!(method, "account.feed_headers");
                assert_eq!(
                    params,
                    json!({"site_url":"https://agentrelay.com", "workspace_id":"w-1",
                    "account_id":"a-1", "profile_id":"x7k2q"})
                );
                Ok(answer())
            },
        )
        .unwrap();
        assert_eq!(Value::Object(result), answer()["headers"]);
    }

    #[test]
    fn failures_and_malformed_headers_never_echo_the_response() {
        assert_eq!(
            failing(anyhow::anyhow!("Authorization: Bearer private-fixture")),
            FAILURE
        );
        assert_eq!(
            failing(SafeError("Authorization: Bearer private-fixture".into()).into()),
            FAILURE
        );
        let mut wrong_profile = answer();
        wrong_profile["headers"]["X-Agent-Relay-Profile"] = json!("other");
        let mut extra = answer();
        extra["headers"]["X-Extra"] = json!("private-fixture");
        let mut not_text = answer();
        not_text["headers"]["X-Agent-Relay-Desktop"] = json!(1);
        for response in [
            json!(null),
            json!({"headers":{}}),
            json!({"headers":"Bearer private-fixture"}),
            wrong_profile,
            extra,
            not_text,
        ] {
            let error = headers("site", "w", "a", "x7k2q", None, |_, _| Ok(response))
                .unwrap_err()
                .to_string();
            assert_eq!(error, NOT_UNDERSTOOD);
            assert!(!error.contains("private"), "{error}");
        }
    }

    #[test]
    fn an_unreachable_core_names_the_socket_that_was_tried() {
        for relay_text in [
            format!("Agent Relay core is not running at {SOCKET}."),
            format!("{SOCKET} is not served by your own Agent Relay core."),
        ] {
            assert_eq!(
                failing(SafeError(relay_text).into()),
                format!("Agent Relay's core is not reachable at {SOCKET}.")
            );
        }
        // Another socket's connection failure is not this one's.
        assert_eq!(
            failing(SafeError("Agent Relay core is not running at /other.sock.".into()).into()),
            FAILURE
        );
    }

    /// Pins the relay crate's connection-failure text this command relies
    /// on, against a socket in an empty directory (no core is contacted).
    #[cfg(unix)]
    #[test]
    fn the_relay_connection_failure_is_recognised() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("core.sock");
        let tried = core_socket(Some(&socket)).unwrap();
        assert_eq!(tried, socket);
        let error = headers("site", "w", "a", "x7k2q", Some(&tried), |method, params| {
            agent_relay_node::relay::cli::call(
                Some(&socket),
                method,
                params,
                Duration::from_secs(5),
            )
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "Agent Relay's core is not reachable at {}.",
                socket.display()
            )
        );
    }

    #[test]
    fn the_core_s_own_sign_in_and_profile_errors_pass_through() {
        for known in CORE_REASONS {
            assert_eq!(failing(SafeError(known.into()).into()), known);
        }
    }

    #[test]
    fn core_reasons_match_the_rpc_s_own_text() {
        let rpc = include_str!("account/mcp_auth/feed_auth.rs");
        for known in CORE_REASONS {
            assert!(rpc.contains(&format!("\"{known}\"")), "{known}");
        }
    }

    #[test]
    fn another_error_from_the_core_keeps_the_generic_line() {
        for other in [
            "Agent Relay core closed the connection.",
            "unknown method account.feed_headers",
            "Sign in to the configured Agent Relay account and workspace first. Bearer x",
        ] {
            assert_eq!(failing(SafeError(other.into()).into()), FAILURE);
        }
    }

    #[test]
    fn default_output_is_the_unchanged_json_object() {
        let headers = answer()["headers"].as_object().unwrap().clone();
        let bytes = render(&headers, Format::Json).unwrap();
        let mut before = serde_json::to_vec(&Value::Object(headers)).unwrap();
        before.push(b'\n');
        assert_eq!(bytes, before);
        assert_eq!(
            bytes,
            b"{\"Authorization\":\"Bearer fixture-only\",\"X-Agent-Relay-Desktop\":\"1\",\
\"X-Agent-Relay-Profile\":\"x7k2q\"}\n"
        );
    }

    #[test]
    fn lines_output_is_one_header_per_line() {
        let headers = answer()["headers"].as_object().unwrap().clone();
        assert_eq!(
            render(&headers, Format::Lines).unwrap(),
            b"Authorization: Bearer fixture-only\nX-Agent-Relay-Desktop: 1\n\
X-Agent-Relay-Profile: x7k2q\n"
        );
    }

    #[test]
    fn a_line_break_in_a_value_is_refused_in_every_format() {
        for value in [
            "Bearer private\r\nInjected: value",
            "Bearer a\rb",
            "Bearer a\nb",
        ] {
            let mut injected = answer();
            injected["headers"]["Authorization"] = json!(value);
            let error = headers("site", "w", "a", "x7k2q", None, |_, _| Ok(injected.clone()))
                .unwrap_err()
                .to_string();
            assert_eq!(error, FAILURE);
            let headers = injected["headers"].as_object().unwrap().clone();
            for format in [Format::Json, Format::Lines] {
                let error = render(&headers, format).unwrap_err().to_string();
                assert_eq!(error, FAILURE);
            }
        }
    }

    #[test]
    fn format_is_parsed_and_an_unknown_one_is_a_usage_error() {
        assert_eq!(parse(&arguments(&[])).unwrap().format, Format::Json);
        assert_eq!(
            parse(&arguments(&["--format", "json"])).unwrap().format,
            Format::Json
        );
        let lines = arguments(&["--format", "lines", "--socket", SOCKET]);
        let request = parse(&lines).unwrap();
        assert_eq!(request.format, Format::Lines);
        assert_eq!(request.socket, Some(Path::new(SOCKET)));
        for bad in [&["--format", "yaml"][..], &["--format"][..]] {
            let usage = parse(&arguments(bad)).unwrap_err();
            assert!(usage.ends_with(USAGE), "{usage}");
            assert_eq!(run(&arguments(bad)), 2);
        }
        assert!(parse(&arguments(&[])[2..]).unwrap_err().ends_with(USAGE));
    }
}
