//! tmux, OS-independent: socket discovery paths, the `list-panes` /
//! `list-clients` / server-pid formats and parsers, the focus and attach
//! argv, and validation of every id that reaches a tmux argv.
//!
//! UNVERIFIED (tmux was not installed where this was written): the formats
//! are tmux's documented `FORMATS` variables (`pane_id`, `pane_tty`,
//! `session_id`, `window_id`, `client_tty`, `client_pid`, `pid`) and the
//! error texts are tmux's own messages, not captured output.
//!
//! No call here starts a server: `list-*` and `switch-client` /
//! `select-*` fail with "no server running" when nothing listens.
use session_navigation::hosts::{TmuxClientInfo, TmuxPaneInfo};

/// `list-panes -a -F` (the contract's format).
pub const PANES_FORMAT: &str = "#{pane_id}\t#{pane_tty}\t#{session_id}\t#{window_id}";
/// `list-clients -F`: the contract's three columns plus the active pane of
/// the client's current window (`#{pane_id}` in a client's context).
pub const CLIENTS_FORMAT: &str = "#{client_tty}\t#{client_pid}\t#{session_id}\t#{pane_id}";
/// `list-sessions -F`: the server's pid on every line.
pub const PID_FORMAT: &str = "#{pid}";

const MAX_OUTPUT: usize = 4 * 1024 * 1024;

/// `<tmpdir>/tmux-<uid>`: where tmux puts its sockets (`TMUX_TMPDIR`,
/// default `/tmp`; on macOS `/tmp` is `/private/tmp`).
pub fn socket_dir(tmpdir: &str, uid: u32) -> String {
    format!("{}/tmux-{uid}", tmpdir.trim_end_matches('/'))
}

fn is_id(text: &str, sigil: char) -> bool {
    text.strip_prefix(sigil)
        .is_some_and(|n| !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `%N`.
pub fn is_pane_id(text: &str) -> bool {
    is_id(text, '%')
}

/// `$N`.
pub fn is_session_id(text: &str) -> bool {
    is_id(text, '$')
}

/// `@N`.
pub fn is_window_id(text: &str) -> bool {
    is_id(text, '@')
}

/// A `/dev/…` tty with no whitespace or control characters.
pub fn is_tty_path(text: &str) -> bool {
    text.starts_with("/dev/")
        && text.len() <= 64
        && text.len() > 5
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
}

/// An absolute socket path with no control characters.
pub fn is_socket_path(text: &str) -> bool {
    text.starts_with('/') && text.len() <= 1024 && !text.chars().any(char::is_control)
}

fn lines(bytes: &[u8]) -> Option<Vec<&str>> {
    if bytes.len() > MAX_OUTPUT {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.lines().filter(|l| !l.trim().is_empty()).collect())
}

/// `list-panes -a -F PANES_FORMAT`. Any malformed row fails the parse.
pub fn parse_panes(bytes: &[u8]) -> Option<Vec<TmuxPaneInfo>> {
    let mut out = Vec::new();
    for line in lines(bytes)? {
        let cols: Vec<&str> = line.split('\t').collect();
        let [pane, tty, session, window] = cols.as_slice() else {
            return None;
        };
        let ok =
            is_pane_id(pane) && is_tty_path(tty) && is_session_id(session) && is_window_id(window);
        if !ok {
            return None;
        }
        out.push(TmuxPaneInfo {
            pane: (*pane).to_owned(),
            tty: (*tty).to_owned(),
            session: (*session).to_owned(),
            window: (*window).to_owned(),
        });
    }
    Some(out)
}

fn pid(text: &str) -> Option<u32> {
    let ok = !text.is_empty() && text.len() <= 10 && text.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| text.parse().ok()).flatten().filter(|p| *p > 0)
}

/// `list-clients -F CLIENTS_FORMAT`. Any malformed row fails the parse.
pub fn parse_clients(bytes: &[u8]) -> Option<Vec<TmuxClientInfo>> {
    let mut out = Vec::new();
    for line in lines(bytes)? {
        let cols: Vec<&str> = line.split('\t').collect();
        let [tty, client_pid, session, active] = cols.as_slice() else {
            return None;
        };
        let ok = is_tty_path(tty) && is_session_id(session) && is_pane_id(active);
        if !ok {
            return None;
        }
        out.push(TmuxClientInfo {
            tty: (*tty).to_owned(),
            pid: pid(client_pid)?,
            session: (*session).to_owned(),
            active_pane: (*active).to_owned(),
        });
    }
    Some(out)
}

/// `list-sessions -F '#{pid}'`: one server pid, the same on every line.
pub fn parse_server_pid(bytes: &[u8]) -> Option<u32> {
    let rows = lines(bytes)?;
    let first = pid(rows.first()?.trim())?;
    rows.iter()
        .all(|r| pid(r.trim()) == Some(first))
        .then_some(first)
}

fn on(socket: &str, rest: &[&str]) -> Vec<String> {
    let mut out = vec!["-S".to_owned(), socket.to_owned()];
    out.extend(rest.iter().map(|s| (*s).to_owned()));
    out
}

/// `-S <socket> list-panes -a -F PANES_FORMAT`.
pub fn list_panes_args(socket: &str) -> Vec<String> {
    on(socket, &["list-panes", "-a", "-F", PANES_FORMAT])
}

/// `-S <socket> list-clients -F CLIENTS_FORMAT`.
pub fn list_clients_args(socket: &str) -> Vec<String> {
    on(socket, &["list-clients", "-F", CLIENTS_FORMAT])
}

/// `-S <socket> list-sessions -F '#{pid}'`.
pub fn server_pid_args(socket: &str) -> Vec<String> {
    on(socket, &["list-sessions", "-F", PID_FORMAT])
}

/// Every id in a focus or attach argv must have its exact shape.
fn ids_ok(socket: &str, session: &str, window: &str, pane: &str) -> bool {
    is_socket_path(socket) && is_session_id(session) && is_window_id(window) && is_pane_id(pane)
}

/// One tmux run, each `;` its own argv word:
/// `-S <socket> switch-client -c <client_tty> -t <$S> ; select-window -t <@W> ; select-pane -t <%P>`.
/// `None` when any id is malformed.
pub fn focus_args(
    socket: &str,
    client_tty: &str,
    session: &str,
    window: &str,
    pane: &str,
) -> Option<Vec<String>> {
    if !ids_ok(socket, session, window, pane) || !is_tty_path(client_tty) {
        return None;
    }
    Some(on(
        socket,
        &[
            "switch-client",
            "-c",
            client_tty,
            "-t",
            session,
            ";",
            "select-window",
            "-t",
            window,
            ";",
            "select-pane",
            "-t",
            pane,
        ],
    ))
}

/// The contract's attach argv (after the tmux program):
/// `-S <socket> select-window -t @W ; select-pane -t %P ; attach-session -t $S`.
pub fn attach_args(socket: &str, session: &str, window: &str, pane: &str) -> Option<Vec<String>> {
    ids_ok(socket, session, window, pane).then(|| {
        on(
            socket,
            &[
                "select-window",
                "-t",
                window,
                ";",
                "select-pane",
                "-t",
                pane,
                ";",
                "attach-session",
                "-t",
                session,
            ],
        )
    })
}

/// Whether `args` is exactly an attach argv (what an executor accepts as a
/// tmux `Launch`; anything else could type or run something).
pub fn is_attach_args(args: &[String]) -> bool {
    let [s, socket, sw, t1, window, semi1, sp, t2, pane, semi2, at, t3, session] = args else {
        return false;
    };
    s == "-S"
        && sw == "select-window"
        && sp == "select-pane"
        && at == "attach-session"
        && [t1, t2, t3].iter().all(|t| *t == "-t")
        && semi1 == ";"
        && semi2 == ";"
        && ids_ok(socket, session, window, pane)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TmuxError {
    /// No server on the socket (it exited, or the socket is stale).
    Gone,
    /// The client, session, window or pane named is not there.
    NotFound,
    Other,
}

/// Classify a failed tmux run. UNVERIFIED texts (tmux's own messages:
/// "no server running on …", "error connecting to …", "can't find pane").
pub fn classify_error(stderr: &[u8]) -> TmuxError {
    let text = String::from_utf8_lossy(stderr.get(..stderr.len().min(64 * 1024)).unwrap_or(&[]))
        .to_ascii_lowercase();
    if text.contains("no server running")
        || text.contains("error connecting")
        || text.contains("no such file")
        || text.contains("connection refused")
    {
        TmuxError::Gone
    } else if text.contains("can't find") || text.contains("no current client") {
        TmuxError::NotFound
    } else {
        TmuxError::Other
    }
}
