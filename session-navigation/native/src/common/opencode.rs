//! OpenCode evidence: the shared server (through curl, read-only GETs), the
//! `session_v2` database when the server is down, and tab hints (OpenCode
//! facts A, B, D, F).
//!
//! The server password goes from `service.json` into a [`Secret`] and from
//! there only into curl's stdin config. `opencode api` is never run (it can
//! start the service), and nothing here can start, stop or restart it.
use session_navigation::evidence::{
    Fact, OpenCodeEvidence, OpenCodeSession, ProviderEvidence, ServerState,
};
use session_navigation::parse::{ids, opencode};

use super::{probe, Found};
use crate::common::config::CommonConfig;
use crate::common::parse;
use crate::common::reservations;
use crate::system::{FsError, Run, Secret, System};

/// curl's "Failed to connect": nothing listens, the server is down.
const CURL_COULDNT_CONNECT: i32 = 7;
const MAX_BODY: usize = 1024 * 1024;

/// Accept only `http://` to a loopback host with a numeric port and a path.
pub fn is_loopback_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let port = ["127.0.0.1:", "localhost:", "[::1]:"]
        .iter()
        .find_map(|host| authority.strip_prefix(host));
    let port_ok = port
        .is_some_and(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    port_ok
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\' | b'`' | b'\''))
}

/// The curl config on stdin: Basic auth as `opencode:<password>`.
pub fn curl_config(password: &Secret) -> Option<Secret> {
    let raw = std::str::from_utf8(password.expose_secret()).ok()?;
    if raw.chars().any(char::is_control) {
        return None;
    }
    let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
    Some(Secret::new(
        format!("user = \"opencode:{escaped}\"\n").into_bytes(),
    ))
}

/// The exact curl argv for a GET of `url`; the secret goes on stdin.
pub fn curl_args(cfg: &CommonConfig, url: &str) -> Vec<String> {
    let max_time = cfg.timeouts.http.as_secs().max(1).to_string();
    [
        "-q",
        "--silent",
        "--show-error",
        "--noproxy",
        "*",
        "--proto",
        "=http",
        "--max-time",
        &max_time,
        "--max-filesize",
        "1048576",
        "--config",
        "-",
        "--write-out",
        "\n%{http_code}",
        "--url",
        url,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// A connection to the server named by `service.json`.
pub struct Server {
    url: String,
    password: Secret,
}

enum Get {
    Response(u16, Vec<u8>),
    Refused,
    Failed,
}

fn get<S: System>(sys: &S, cfg: &CommonConfig, server: &Server, path: &str) -> Get {
    let url = format!("{}{path}", server.url);
    let Some(config) = curl_config(&server.password) else {
        return Get::Failed;
    };
    if !is_loopback_url(&url) {
        return Get::Failed;
    }
    let run = Run::with_args(&cfg.programs.curl, curl_args(cfg, &url))
        .stdin(config)
        .timeout(cfg.timeouts.http + std::time::Duration::from_secs(1))
        .max_output(MAX_BODY + 64);
    match sys.run(&run) {
        Ok(out) if out.status == Some(CURL_COULDNT_CONNECT) => Get::Refused,
        Ok(out) if out.success() => match parse::curl_response(&out.stdout) {
            Some((status, body)) => Get::Response(status, body.to_vec()),
            None => Get::Failed,
        },
        _ => Get::Failed,
    }
}

/// `Down` for a missing `service.json` or a refused connection; `Up` only
/// when `/api/info` agrees with the file on pid and version.
pub fn server<S: System>(sys: &S, cfg: &CommonConfig) -> (Fact<ServerState>, Option<Server>) {
    let bytes = match sys.read_file(&cfg.paths.opencode_service_file(), 64 * 1024) {
        Err(FsError::NotFound) => return (Fact::Known(ServerState::Down), None),
        Err(_) => return (Fact::Unknown, None),
        Ok(bytes) => bytes,
    };
    let (Ok(state), Some(password)) = (
        opencode::parse_service_state(&bytes),
        parse::service_password(&bytes),
    ) else {
        return (Fact::Unknown, None);
    };
    let server = Server {
        url: state.url.clone(),
        password,
    };
    match get(sys, cfg, &server, "/api/info") {
        Get::Refused => (Fact::Known(ServerState::Down), None),
        Get::Response(200, body) => match opencode::parse_api_info(&body) {
            Ok(info) if info.pid == state.pid && info.version == state.version => (
                Fact::Known(ServerState::Up {
                    pid: info.pid,
                    version: info.version,
                }),
                Some(server),
            ),
            _ => (Fact::Unknown, None),
        },
        _ => (Fact::Unknown, None),
    }
}

pub fn db_query(id: &str) -> String {
    format!("SELECT id, parent_id, directory, time_archived FROM session_v2 WHERE id = '{id}';")
}

pub fn sqlite_run(cfg: &CommonConfig, db: String, query: String) -> Run {
    Run::with_args(
        &cfg.programs.sqlite3,
        vec![
            "-readonly".into(),
            "-json".into(),
            "-init".into(),
            "/dev/null".into(),
            db,
            query,
        ],
    )
    .timeout(cfg.timeouts.probe)
}

fn from_db<S: System>(sys: &S, cfg: &CommonConfig, id: &str) -> Fact<Option<OpenCodeSession>> {
    let run = sqlite_run(cfg, cfg.paths.opencode_db(), db_query(id));
    let Some(rows) = probe(sys, &run).and_then(|o| parse::sqlite_rows(&o.stdout)) else {
        return Fact::Unknown;
    };
    match rows.as_slice() {
        [] => Fact::Known(None),
        [row] => {
            let bytes = serde_json::Value::Object(row.clone()).to_string();
            opencode::parse_db_row(bytes.as_bytes(), id)
                .map_or(Fact::Unknown, |s| Fact::Known(Some(s)))
        }
        _ => Fact::Unknown,
    }
}

fn session<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    id: &str,
    up: Option<&Server>,
) -> Fact<Option<OpenCodeSession>> {
    let Some(server) = up else {
        return from_db(sys, cfg, id);
    };
    match get(sys, cfg, server, &format!("/api/session/{id}")) {
        Get::Response(status, body) => {
            opencode::parse_session_lookup(status, &body, id).map_or(Fact::Unknown, Fact::Known)
        }
        _ => Fact::Unknown,
    }
}

fn busy<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    id: &str,
    state: &Fact<ServerState>,
    up: Option<&Server>,
) -> Fact<bool> {
    match (state, up) {
        (Fact::Known(ServerState::Up { .. }), Some(server)) => {
            match get(sys, cfg, server, "/api/session/active") {
                Get::Response(200, body) => opencode::parse_active(&body)
                    .map_or(Fact::Unknown, |active| {
                        Fact::Known(active.iter().any(|a| a == id))
                    }),
                _ => Fact::Unknown,
            }
        }
        (Fact::Known(ServerState::Down), _) => Fact::Known(false),
        _ => Fact::Unknown,
    }
}

pub const TABS_QUERY: &str =
    "SELECT value FROM state WHERE key = 'tabs' AND name LIKE 'opencode.window.%';";

/// Every session in the desktop's saved tabs. No `drafts.sqlite` is no tabs.
pub fn desktop_tabs<S: System>(sys: &S, cfg: &CommonConfig) -> Fact<Vec<String>> {
    let db = cfg.paths.opencode_drafts_db();
    match sys.metadata(&db) {
        Err(FsError::NotFound) => return Fact::Known(Vec::new()),
        Err(_) => return Fact::Unknown,
        Ok(_) => {}
    }
    let run = sqlite_run(cfg, db, TABS_QUERY.to_owned());
    let Some(rows) = probe(sys, &run).and_then(|o| parse::sqlite_rows(&o.stdout)) else {
        return Fact::Unknown;
    };
    let mut tabs: Vec<String> = Vec::new();
    for row in rows {
        let Some(value) = row.get("value").and_then(serde_json::Value::as_str) else {
            return Fact::Unknown;
        };
        let Some(ids) = parse::desktop_tabs(value) else {
            return Fact::Unknown;
        };
        for id in ids {
            if !tabs.contains(&id) {
                tabs.push(id);
            }
        }
    }
    Fact::Known(tabs)
}

fn tui_tab_hint<S: System>(sys: &S, cfg: &CommonConfig, id: &str) -> bool {
    sys.read_file(&cfg.paths.opencode_tui_tabs(), MAX_BODY)
        .ok()
        .and_then(|b| opencode::parse_tui_tabs(&b).ok())
        .is_some_and(|dirs| dirs.iter().any(|(_, tabs)| tabs.iter().any(|t| t == id)))
}

/// Evidence for an id that cannot be looked up.
pub fn unknown() -> ProviderEvidence {
    ProviderEvidence::OpenCode(OpenCodeEvidence {
        server: Fact::Unknown,
        session: Fact::Unknown,
        busy: Fact::Unknown,
        tui_tab_hint: false,
        desktop_tab_hint: false,
        desktop_tabs: Fact::Unknown,
        reserved: Fact::Unknown,
    })
}

pub fn observe<S: System>(sys: &S, cfg: &CommonConfig, id: &str) -> Found {
    if !ids::is_opencode_session_id(id) {
        return Found::unknown(unknown());
    }
    let (state, up) = server(sys, cfg);
    let session = session(sys, cfg, id, up.as_ref());
    let busy = busy(sys, cfg, id, &state, up.as_ref());
    let desktop_tabs = desktop_tabs(sys, cfg);
    let cwd = session.known().cloned().flatten().map(|s| s.directory);
    let found = cwd.is_some();
    Found {
        evidence: ProviderEvidence::OpenCode(OpenCodeEvidence {
            server: state,
            session,
            busy,
            tui_tab_hint: tui_tab_hint(sys, cfg, id),
            desktop_tab_hint: desktop_tabs
                .known()
                .is_some_and(|t| t.iter().any(|t| t == id)),
            desktop_tabs,
            reserved: reservations::other_opencode(sys, &cfg.paths.reservation_dir, id),
        }),
        cwd,
        found,
    }
}
