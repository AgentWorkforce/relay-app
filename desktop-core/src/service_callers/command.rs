//! `service-callers`: the probe's management commands
//! (`docs/service-callers.md` section 4). A provision or rotate from here
//! stays pending until the person confirms it in Settings, and names no
//! place for the secret: Desktop picks it on approval. Disable and delete
//! are app-only changes, so the core refuses them from here with
//! `app_attestation_unavailable`. Nothing printed here is a secret, a hash,
//! a token or a path.

use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Duration;

const USAGE: &str = "\
Usage: agent-relay-probe service-callers <command> [--socket <path>]

Commands:
  list                                   List service callers and their state
  provision --name <name> --harness <harness>... --root <directory>...
            [--expires-in-days <days>]
                                         Ask for a new service caller (pending
                                         until you confirm it in Settings)
  rotate --caller <id> [--overlap-seconds <seconds>]
                                         Ask for a new secret (pending until
                                         you confirm it in Settings)
  disable --caller <id>                  Refuse the caller's calls (Settings
                                         only: refused from the command line)
  delete --caller <id>                   Delete the caller and its handoff file
                                         (Settings only: refused from the
                                         command line)
";

/// The parsed command: the bridge method and its params.
fn request(arguments: &[String]) -> Result<(&'static str, Value), String> {
    let [command, rest @ ..] = arguments else {
        return Err(USAGE.into());
    };
    let mut options: Vec<(&str, &str)> = Vec::new();
    let mut at = 0;
    while at < rest.len() {
        let name = rest[at].as_str();
        let value = rest
            .get(at + 1)
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("{name} needs a value.\n\n{USAGE}"))?;
        options.push((name, value.as_str()));
        at += 2;
    }
    let allowed: &[&str] = match command.as_str() {
        "list" => &[],
        "provision" => &["--name", "--harness", "--root", "--expires-in-days"],
        "rotate" => &["--caller", "--overlap-seconds"],
        "disable" | "delete" => &["--caller"],
        _ => return Err(format!("Unknown command {command}.\n\n{USAGE}")),
    };
    if let Some((name, _)) = options
        .iter()
        .find(|(name, _)| *name != "--socket" && !allowed.contains(name))
    {
        return Err(format!("{command} does not take {name}.\n\n{USAGE}"));
    }
    let all = |name: &str| -> Vec<&str> {
        options
            .iter()
            .filter(|(given, _)| *given == name)
            .map(|(_, value)| *value)
            .collect()
    };
    let one = |name: &str| -> Result<Option<&str>, String> {
        match all(name).as_slice() {
            [] => Ok(None),
            [value] => Ok(Some(value)),
            _ => Err(format!("{name} is given more than once.")),
        }
    };
    let needed = |name: &str| -> Result<&str, String> {
        one(name)?.ok_or_else(|| format!("{command} needs {name}.\n\n{USAGE}"))
    };
    let number = |name: &str| -> Result<Option<u64>, String> {
        one(name)?
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| format!("{name} must be a whole number."))
            })
            .transpose()
    };
    let mut params = Map::new();
    let mut put = |key: &str, value: Value| {
        params.insert(key.into(), value);
    };
    let method = match command.as_str() {
        "list" => "spawn_callers.list",
        "provision" => {
            put("name", json!(needed("--name")?));
            put("harnesses", json!(all("--harness")));
            put("directory_roots", json!(all("--root")));
            if let Some(days) = number("--expires-in-days")? {
                put("expires_in_days", json!(days));
            }
            "spawn_callers.provision"
        }
        "rotate" => {
            put("caller_id", json!(needed("--caller")?));
            if let Some(seconds) = number("--overlap-seconds")? {
                put("overlap_seconds", json!(seconds));
            }
            "spawn_callers.rotate"
        }
        "disable" => {
            put("caller_id", json!(needed("--caller")?));
            put("enabled", json!(false));
            "spawn_callers.set_enabled"
        }
        _ => {
            put("caller_id", json!(needed("--caller")?));
            "spawn_callers.revoke"
        }
    };
    Ok((method, Value::Object(params)))
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string()
}

/// One line for a caller summary.
fn line(caller: &Value) -> String {
    format!(
        "{}  {}  {}  expires {}",
        text(caller, "caller_id"),
        text(caller, "name"),
        text(caller, "state"),
        text(caller, "expires_at"),
    )
}

/// What the command prints for the bridge's answer.
fn render(method: &str, answer: &Value) -> String {
    let caller = answer.get("caller").unwrap_or(&Value::Null);
    let pending = |state: &str| {
        if text(caller, "state") == state {
            "Confirm it in Agent Relay Settings; until then nothing is written.\n"
        } else {
            ""
        }
    };
    match method {
        "spawn_callers.list" => {
            let callers = answer
                .get("callers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if callers.is_empty() {
                return "No service callers.\n".into();
            }
            callers.iter().map(|caller| line(caller) + "\n").collect()
        }
        "spawn_callers.provision" => format!("{}\n{}", line(caller), pending("pending_provision")),
        "spawn_callers.rotate" => format!("{}\n{}", line(caller), pending("pending_rotation")),
        "spawn_callers.set_enabled" => format!("{}\n", line(caller)),
        _ => "Deleted.\n".into(),
    }
}

/// `service-callers <command> ...`; the exit status.
pub fn run(arguments: &[String]) -> i32 {
    if arguments.is_empty()
        || arguments
            .iter()
            .any(|argument| ["-h", "--help", "help"].contains(&argument.as_str()))
    {
        print!("{USAGE}");
        return if arguments.is_empty() { 2 } else { 0 };
    }
    let (method, params) = match request(arguments) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    let socket = arguments
        .iter()
        .position(|argument| argument == "--socket")
        .and_then(|at| arguments.get(at + 1))
        .map(Path::new);
    match agent_relay_node::relay::cli::call(socket, method, params, Duration::from_secs(60)) {
        Ok(answer) => {
            print!("{}", render(method, &answer));
            0
        }
        Err(error) => {
            eprintln!("Agent Relay refused {method}: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn each_command_names_its_method_and_params() {
        let (method, params) = request(&words(
            "provision --name ci --harness claude --harness codex --root /src --expires-in-days 30",
        ))
        .unwrap();
        assert_eq!(method, "spawn_callers.provision");
        assert_eq!(
            params,
            json!({"name": "ci", "harnesses": ["claude", "codex"], "directory_roots": ["/src"],
                   "expires_in_days": 30})
        );
        let (method, params) = request(&words("disable --caller sc_1")).unwrap();
        assert_eq!(method, "spawn_callers.set_enabled");
        assert_eq!(params, json!({"caller_id": "sc_1", "enabled": false}));
        let (method, _) = request(&words("delete --caller sc_1")).unwrap();
        assert_eq!(method, "spawn_callers.revoke");
        let (method, params) =
            request(&words("rotate --caller sc_1 --overlap-seconds 60")).unwrap();
        assert_eq!(method, "spawn_callers.rotate");
        assert_eq!(params, json!({"caller_id": "sc_1", "overlap_seconds": 60}));
        assert!(request(&words("decide --caller sc_1")).is_err());
        assert!(request(&words("disable --caller sc_1 --name x")).is_err());
        assert!(request(&words("rotate")).is_err());
        // The command line never names a place for a secret.
        assert!(request(&words(
            "provision --name ci --harness claude --root /src --handoff /x"
        ))
        .is_err());
        assert!(request(&words("rotate --caller sc_1 --handoff /x")).is_err());
    }

    #[test]
    fn a_pending_provision_prints_its_state_and_no_path() {
        let answer = json!({
            "caller": {"caller_id": "sc_1", "name": "ci", "state": "pending_provision",
                       "expires_at": "2027-02-06T00:00:00Z"},
        });
        let printed = render("spawn_callers.provision", &answer);
        assert!(printed.contains("sc_1  ci  pending_provision"), "{printed}");
        assert!(!printed.contains("handoff"), "{printed}");
        assert!(printed.contains("Settings"));
    }
}
