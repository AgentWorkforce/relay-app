//! Provider layouts on the fake Mac, matching the real paths and shapes.
use serde_json::json;
use session_navigation::model::App;
use session_navigation::parse::lstart;
use session_navigation_native::common::{codex as ocodex, opencode as oopencode};
use session_navigation_native::system::{Output, Run};

use super::*;

pub const PROJECT: &str = "/Users/example/code/project";
pub const LOCAL_ID: &str = "local_11111111-2222-4333-8444-555555555555";
pub const BUNDLED_CLAUDE: &str =
    "/Users/example/Library/Application Support/Claude/claude-code/2.1.321/abcdef012345/claude.app/Contents/MacOS/claude";

pub fn desktop_dir() -> String {
    format!("{HOME}/Library/Application Support/Claude/claude-code-sessions/acct-0001/org-0001")
}

pub fn desktop_record(archived: bool) -> Vec<u8> {
    json!({
        "sessionId": LOCAL_ID, "cliSessionId": CLAUDE_ID, "isArchived": archived,
        "cwd": PROJECT, "lastFocusedAt": 1_790_000_400_000u64, "title": "Example",
    })
    .to_string()
    .into_bytes()
}

pub fn transcript_path() -> String {
    format!("{HOME}/.claude/projects/-Users-example-code-project/{CLAUDE_ID}.jsonl")
}

pub fn claude_desktop_procs() -> Vec<Proc> {
    vec![
        Proc::new(
            25208,
            1,
            "??",
            "Claude",
            "/Applications/Claude.app/Contents/MacOS/Claude",
            "/Applications/Claude.app/Contents/MacOS/Claude",
        ),
        Proc::new(
            25353,
            25208,
            "??",
            "disclaimer",
            "/Applications/Claude.app/Contents/Helpers/disclaimer",
            "/Applications/Claude.app/Contents/Helpers/disclaimer --pgroup --",
        ),
        Proc::new(
            25354,
            25353,
            "??",
            "claude",
            BUNDLED_CLAUDE,
            &format!("{BUNDLED_CLAUDE} --output-format stream-json --verbose"),
        ),
    ]
}

pub fn registry_record(pid: u32, start: u64) -> Vec<u8> {
    json!({
        "pid": pid, "sessionId": CLAUDE_ID, "cwd": PROJECT, "procStart": lstart::format(start),
        "kind": "interactive", "entrypoint": "claude-desktop", "hostSessionId": LOCAL_ID,
    })
    .to_string()
    .into_bytes()
}

/// Claude.app with the session's desktop record and transcript; `live`
/// adds its running CLI process and registry file.
pub fn claude_machine(archived: bool, live: bool) -> Machine {
    let mut m = Machine::new();
    m.apps_json = apps_json(
        Some(App::ClaudeDesktop),
        &[App::ClaudeDesktop, App::Ghostty],
    );
    m.dir(PROJECT);
    m.file(
        &format!("{}/{LOCAL_ID}.json", desktop_dir()),
        desktop_record(archived),
    );
    m.file(
        &transcript_path(),
        core_fixture("claude/transcript-first-message-line.json"),
    );
    m.dir(&format!("{HOME}/.claude/sessions"));
    if live {
        let procs = claude_desktop_procs();
        let start = procs[2].start;
        m.procs.extend(procs);
        m.file(
            &format!("{HOME}/.claude/sessions/25354.json"),
            registry_record(25354, start),
        );
    }
    m
}

pub fn codex_row(archived: bool, originator: &str) -> String {
    json!([{
        "id": CODEX_ID, "cwd": PROJECT, "archived": i64::from(archived), "originator": originator,
        "thread_source": "user", "source": "vscode",
    }])
    .to_string()
}

/// A Codex TUI thread in `state_5.sqlite`, nothing holding its lock.
pub fn codex_machine(row: Option<String>) -> Machine {
    let mut m = Machine::new();
    let cfg = cfg();
    m.dir(PROJECT);
    m.file(
        &cfg.common.paths.codex_state_db(),
        b"SQLite format 3\0".to_vec(),
    );
    m.dir(&cfg.common.paths.codex_locks_dir());
    m.run(
        ocodex::thread_run(&cfg.common, CODEX_ID),
        Ok(Output::ok(row.unwrap_or_default())),
    );
    m
}

pub fn curl(path: &str) -> Run {
    let cfg = cfg();
    Run::with_args(
        session_navigation_native::macos::CURL,
        oopencode::curl_args(&cfg.common, &format!("http://127.0.0.1:49374{path}")),
    )
}

pub fn curl_ok(status: u16, body: &str) -> Output {
    Output::ok(format!("{body}\n{status}"))
}

pub fn opencode_service_procs() -> Vec<Proc> {
    vec![
        Proc::new(815, 1, "??", "OpenCode", "/Applications/OpenCode.app/Contents/MacOS/OpenCode",
            "/Applications/OpenCode.app/Contents/MacOS/OpenCode"),
        Proc::new(1111, 815, "??", "opencode-cli",
            "/Users/example/Library/Application Support/ai.opencode.desktop/cli/2.2.6-arm64/opencode-cli",
            "/Users/example/Library/Application Support/ai.opencode.desktop/cli/2.2.6-arm64/opencode-cli serve --service"),
    ]
}

pub fn opencode_session_body() -> String {
    let info: serde_json::Value =
        serde_json::from_slice(&core_fixture("opencode/api-session-info.json")).unwrap_or_default();
    let mut info = info;
    info["location"]["directory"] = PROJECT.into();
    json!({ "data": info }).to_string()
}

/// The shared OpenCode server up (desktop not running, so a TUI resume is
/// the plan), answering info, the session and `active`.
pub fn opencode_machine() -> Machine {
    let mut m = Machine::new();
    let cfg = cfg();
    m.procs
        .extend(opencode_service_procs().into_iter().skip(1).map(|mut p| {
            p.ppid = 1;
            p
        }));
    m.dir(PROJECT);
    let mut state: serde_json::Value =
        serde_json::from_slice(&core_fixture("opencode/service-state.json")).unwrap_or_default();
    state["pid"] = 1111.into();
    m.file(
        &cfg.common.paths.opencode_service_file(),
        state.to_string().into_bytes(),
    );
    let info = json!({"version": "2.2.6", "pid": 1111, "hostname": "127.0.0.1"}).to_string();
    m.run(curl("/api/info"), Ok(curl_ok(200, &info)));
    m.run(
        curl(&format!("/api/session/{OPENCODE_ID}")),
        Ok(curl_ok(200, &opencode_session_body())),
    );
    m.run(
        curl("/api/session/active"),
        Ok(curl_ok(200, "{\"data\":{}}")),
    );
    m
}
