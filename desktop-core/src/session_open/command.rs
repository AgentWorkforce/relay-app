//! `agent-relay-probe session-open plan|open|release|strings`: the macOS
//! app's way in. `plan`, `open` and `release` read one request object on
//! stdin; `strings` reads nothing and prints the fixed words
//! ([`super::wording::strings`], checked in as
//! `shared/session-open-strings.json`). One answer object on stdout. Exit 0 whenever an
//! answer was written (a refusal is an answer); 2 with a line on stderr for
//! a malformed command or request; 1 when the answer could not be written.
//! It never starts, stops or talks to the running core, and prints nothing
//! but the answer (no snapshot, no provider secret).
use std::io::{Read, Write};

use super::contract::{OpenResult, PlanView, Request};

/// Largest request read from stdin.
const MAX_REQUEST: u64 = 64 * 1024;

pub const USAGE: &str = "usage: agent-relay-probe session-open plan|open|release < request.json\n       agent-relay-probe session-open strings\n";

/// What one verb answers with, given the request.
pub struct Verbs<P, O, R> {
    pub plan: P,
    pub open: O,
    pub release: R,
}

/// The fixed words as `session-open strings` prints them (and as
/// `shared/session-open-strings.json` holds them): pretty JSON, one line
/// per field, ending in a newline.
pub fn strings_json() -> Result<String, (i32, String)> {
    serde_json::to_string_pretty(&super::wording::strings())
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|_| {
            (
                1,
                "session-open: the answer could not be encoded.\n".to_owned(),
            )
        })
}

/// The answer to `args` and `input`, or the exit code and the line for
/// stderr. The verbs are injected so tests stay off the machine.
pub fn answer<P, O, R>(
    args: &[String],
    input: &[u8],
    verbs: Verbs<P, O, R>,
) -> Result<String, (i32, String)>
where
    P: FnOnce(&Request) -> PlanView,
    O: FnOnce(&Request) -> OpenResult,
    R: FnOnce(&Request) -> PlanView,
{
    let verb = match args {
        [verb] if verb == "strings" => return strings_json(),
        [verb] if ["plan", "open", "release"].contains(&verb.as_str()) => verb.as_str(),
        _ => return Err((2, USAGE.to_owned())),
    };
    let request: Request = serde_json::from_slice(input).map_err(|error| {
        (
            2,
            format!(
                "session-open: the request is not a valid request object (line {}, column {}).\n",
                error.line(),
                error.column()
            ),
        )
    })?;
    let encoded = match verb {
        "plan" => serde_json::to_string(&(verbs.plan)(&request)),
        "release" => serde_json::to_string(&(verbs.release)(&request)),
        _ => serde_json::to_string(&(verbs.open)(&request)),
    };
    encoded
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|_| {
            (
                1,
                "session-open: the answer could not be encoded.\n".to_owned(),
            )
        })
}

/// Run the command on this process's stdin and stdout; the exit code.
pub fn run(args: &[String]) -> i32 {
    if let [flag] = args {
        if flag == "--help" || flag == "-h" {
            print!("{USAGE}");
            return 0;
        }
    }
    let mut input = Vec::new();
    let reads = !matches!(args, [verb] if verb == "strings");
    if reads
        && std::io::stdin()
            .take(MAX_REQUEST + 1)
            .read_to_end(&mut input)
            .is_err()
        || input.len() as u64 > MAX_REQUEST
    {
        eprintln!("session-open: the request could not be read.");
        return 2;
    }
    let verbs = Verbs {
        plan: super::plan,
        open: super::open,
        release: super::release_launch,
    };
    match answer(args, &input, verbs) {
        Ok(text) => {
            let mut out = std::io::stdout().lock();
            if out
                .write_all(text.as_bytes())
                .and_then(|()| out.flush())
                .is_err()
            {
                return 1;
            }
            0
        }
        Err((code, line)) => {
            eprint!("{line}");
            code
        }
    }
}
