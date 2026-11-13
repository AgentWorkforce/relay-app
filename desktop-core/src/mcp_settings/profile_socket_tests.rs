//! A site profile build's core serves its own sockets, so the commands
//! written for the person's agents must reach it, not Agent Relay's, under
//! its own entry beside Agent Relay's.
use super::*;
use agent_relay_node::relay::{daemon::Config, paths};
use std::collections::BTreeMap;

const PROFILE: &str = "com.agentrelay.desktop.devaws";
/// A profile build's entry and hook server, from `core.devaws.sock`.
const SERVER: &str = "agent-relay-devaws";

/// A short directory for the sockets, which are only named, never bound: a
/// temporary directory on macOS is too long for a Unix socket path.
fn short(test: &str) -> PathBuf {
    let base = if cfg!(windows) {
        std::env::temp_dir()
    } else {
        PathBuf::from("/tmp")
    };
    base.join(format!("ar-p0-{}-{test}", std::process::id()))
}

fn config(home: &Path, app_data_dir: PathBuf, socket: PathBuf) -> Config {
    Config {
        home: home.into(),
        app_data_dir,
        paths: paths::CorePaths::for_socket(socket).unwrap(),
        env: BTreeMap::from([("XDG_RUNTIME_DIR".into(), "/run/user/4242".into())]),
        version: "test".into(),
        host_name: "test".into(),
        rediscover_every: std::time::Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    }
}

/// A profile build's config and the session socket its core serves.
fn profile(home: &Path, test: &str) -> (Config, PathBuf) {
    let short = short(test);
    let config = config(home, short.join(PROFILE), short.join("core.devaws.sock"));
    let session = if cfg!(target_os = "linux") {
        PathBuf::from("/run/user/4242/agent-relay-devaws/relay.sock")
    } else {
        short.join(PROFILE).join("run").join("relay.sock")
    };
    (config, session)
}

fn installer(home: &Path) -> Installer {
    Installer::new(
        home.into(),
        CodexConfig::from_env(&|_| None, home),
        ClaudeConfig::resolve(&|_| None, home, &[]),
        OpenCodeConfig::from_env(&|_| None, home),
    )
}

fn probe(home: &Path) -> PathBuf {
    let path = crate::probe_copy::destination(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "synthetic executable").unwrap();
    private_fs::set_mode(&path, 0o700).unwrap();
    path
}

fn binding() -> Binding {
    Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "workspace".into(),
        account_id: "account".into(),
    }
}

fn word(path: &Path) -> String {
    quote(path.to_str().unwrap()).unwrap()
}

#[test]
fn a_profile_builds_claude_and_codex_commands_name_its_own_core() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, session) = profile(home, "own");
    let socket = config.paths.socket.clone();
    let app = installer(home).with_core_sockets(&config);
    let executable = probe(home);
    let b = binding();
    fs::create_dir_all(&app.claude.home).unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    app.select(vec!["claude".into(), "codex".into()], Some(&b), None)
        .unwrap();
    assert!(app.refresh(Some(&b)).is_empty());

    let helper = app.helper.command(&b).unwrap();
    assert_eq!(helper, b.helper_at(&executable, Some(&socket)).unwrap());
    assert!(
        helper.ends_with(&format!(
            " {} {}",
            word(Path::new("--socket")),
            word(&socket)
        )),
        "{helper}"
    );
    let claude: Value =
        serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(
        claude["mcpServers"][SERVER]["headersHelper"],
        helper.as_str()
    );
    assert!(claude["mcpServers"].get(NAME).is_none(), "{claude}");
    let codex: toml_edit::DocumentMut = fs::read_to_string(app.codex.home.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        codex["mcp_servers"][SERVER]["http_headers_helper"].as_str(),
        Some(helper.as_str())
    );
    assert!(codex["mcp_servers"].get(NAME).is_none());

    let hook = app.claude_hook_command().unwrap();
    assert_eq!(
        hook,
        format!(
            "{} claude-attestation-hook --socket {}",
            word(&executable),
            word(&session)
        )
    );
    let settings: Value =
        serde_json::from_slice(&fs::read(app.claude.home.join("settings.json")).unwrap()).unwrap();
    let build = app.helper.hook_build().unwrap();
    assert!(
        claude_hook::current_for(&settings, &hook, &build),
        "{settings}"
    );
    assert!(app.claude_hook_configured());
}

#[test]
fn a_profile_build_and_agent_relay_keep_their_own_entries_in_one_file() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, _) = profile(home, "both");
    let production = installer(home);
    let app = installer(home).with_core_sockets(&config);
    probe(home);
    let b = binding();
    fs::create_dir_all(&app.claude.home).unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    for build in [&production, &app] {
        build
            .select(vec!["claude".into(), "codex".into()], Some(&b), None)
            .unwrap();
    }
    // Each refresh after the other's leaves the other's entry and hook alone.
    for _ in 0..2 {
        assert!(production.refresh(Some(&b)).is_empty());
        assert!(app.refresh(Some(&b)).is_empty());
    }
    let ours = production.helper.command(&b).unwrap();
    let theirs = app.helper.command(&b).unwrap();
    assert_ne!(ours, theirs);
    let claude: Value =
        serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(claude["mcpServers"][NAME]["headersHelper"], ours.as_str());
    assert_eq!(
        claude["mcpServers"][SERVER]["headersHelper"],
        theirs.as_str()
    );
    let codex: toml_edit::DocumentMut = fs::read_to_string(app.codex.home.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        codex["mcp_servers"][NAME]["http_headers_helper"].as_str(),
        Some(ours.as_str())
    );
    assert_eq!(
        codex["mcp_servers"][SERVER]["http_headers_helper"].as_str(),
        Some(theirs.as_str())
    );
    let settings: Value =
        serde_json::from_slice(&fs::read(app.claude.home.join("settings.json")).unwrap()).unwrap();
    let hook = production.claude_hook_command().unwrap();
    assert!(claude_hook::current(&settings, &hook), "{settings}");
    let build = app.helper.hook_build().unwrap();
    let profile_hook = app.claude_hook_command().unwrap();
    assert!(
        claude_hook::current_for(&settings, &profile_hook, &build),
        "{settings}"
    );
    assert!(production.claude_hook_configured());
    assert!(app.claude_hook_configured());
}

#[test]
fn the_default_socket_leaves_every_command_as_it_was() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let executable = probe(home);
    let b = binding();
    let app = installer(home).with_core_sockets(&config(
        home,
        home.join(PROFILE),
        paths::default_socket(home),
    ));
    let before = installer(home);
    assert_eq!(app.helper.command(&b), before.helper.command(&b));
    assert_eq!(app.helper.command(&b), b.helper(&executable));
    assert_eq!(app.claude_hook_command(), before.claude_hook_command());
    assert_eq!(
        app.claude_hook_command().unwrap(),
        format!("{} claude-attestation-hook", word(&executable))
    );
}

#[test]
fn a_development_build_keeps_its_own_sockets() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let short = short("dev");
    let socket = short.join("dev.sock");
    let config = config(home, home.join("desktop.dev"), socket.clone());
    let source = home.join("source-probe");
    fs::write(&source, "synthetic dev executable").unwrap();
    let development = || installer(home).with_development(&config.app_data_dir, &socket, &source);
    let before = development();
    let app = development().with_core_sockets(&config);
    let b = binding();
    assert!(app.helper.command(&b).is_some());
    assert_eq!(app.helper.command(&b), before.helper.command(&b));
    assert_eq!(app.claude_hook_command(), before.claude_hook_command());
    assert!(app.claude_hook_command().unwrap().ends_with(" --dev"));
    assert_eq!(app.helper.server(), NAME);
}
