//! A site profile build's core serves its own sockets, so the commands
//! written for the person's agents must reach it, not Agent Relay's, under
//! its own entry beside Agent Relay's. A site profile not built to coexist
//! (it takes over Agent Relay) runs the shared `~/.local/bin` copy; one built
//! to coexist runs its own copy in its app data and leaves the shared one.
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

/// The binary a build runs from, which a site profile built to coexist
/// copies into its own app data.
fn bundled(home: &Path) -> PathBuf {
    let path = home.join("bundled-probe");
    fs::write(&path, "synthetic bundled executable").unwrap();
    private_fs::set_mode(&path, 0o700).unwrap();
    path
}

/// A site profile build's installer: built to coexist (`coexists`), it is
/// given the binary it copies into its app data, as `account::plugin` gives
/// it when `executable_identity::built_to_coexist`; otherwise none.
fn profile_installer(home: &Path, config: &Config, coexists: bool) -> Installer {
    let bundled = bundled(home);
    installer(home).with_core_sockets(config, Some(bundled.as_path()).filter(|_| coexists))
}

/// The executable a site profile's entries run: its own copy when built to
/// coexist, the shared one otherwise.
fn profile_executable(home: &Path, config: &Config, coexists: bool) -> PathBuf {
    if coexists {
        own_copy(config)
    } else {
        assert!(!crate::probe_copy::own_destination(&config.app_data_dir).exists());
        crate::probe_copy::destination(home)
    }
}

/// The shared copy the stock app keeps is left as it was.
fn assert_shared_untouched(home: &Path) {
    assert_eq!(
        fs::read(crate::probe_copy::destination(home)).unwrap(),
        b"synthetic executable"
    );
}

/// The profile's own copy is the bundled binary, in its app data.
fn own_copy(config: &Config) -> PathBuf {
    let own = crate::probe_copy::own_destination(&config.app_data_dir);
    assert_eq!(fs::read(&own).unwrap(), b"synthetic bundled executable");
    own
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

/// Both site profile cases name their own core: one not built to coexist
/// through the shared copy, as before; one built to coexist through its own
/// copy, leaving the shared one.
#[test]
fn a_profile_builds_claude_and_codex_commands_name_its_own_core() {
    for coexists in [false, true] {
        profile_commands_name_its_own_core(coexists);
    }
}

fn profile_commands_name_its_own_core(coexists: bool) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, session) = profile(home, &format!("own{}", u8::from(coexists)));
    let socket = config.paths.socket.clone();
    let app = profile_installer(home, &config, coexists);
    probe(home);
    let b = binding();
    fs::create_dir_all(&app.claude.home).unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    app.select(vec!["claude".into(), "codex".into()], Some(&b), None)
        .unwrap();
    assert!(app.refresh(Some(&b)).is_empty());
    let executable = profile_executable(home, &config, coexists);
    assert_shared_untouched(home);

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

/// Both site profile cases keep their own entries beside Agent Relay's.
#[test]
fn a_profile_build_and_agent_relay_keep_their_own_entries_in_one_file() {
    for coexists in [false, true] {
        profile_and_agent_relay_keep_their_own_entries(coexists);
    }
}

fn profile_and_agent_relay_keep_their_own_entries(coexists: bool) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, _) = profile(home, &format!("both{}", u8::from(coexists)));
    let production = installer(home);
    let app = profile_installer(home, &config, coexists);
    let shared = probe(home);
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
    assert_eq!(ours, b.helper(&shared).unwrap());
    let executable = profile_executable(home, &config, coexists);
    assert_eq!(
        theirs,
        b.helper_at(&executable, Some(&config.paths.socket))
            .unwrap()
    );
    assert_shared_untouched(home);
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
    let stock = config(home, home.join(PROFILE), paths::default_socket(home));
    // The stock app is never built to coexist; were it given its binary to
    // copy, the default socket still leaves every command as it was.
    for coexists in [false, true] {
        let app = profile_installer(home, &stock, coexists);
        let before = installer(home);
        assert_eq!(app.helper.command(&b), before.helper.command(&b));
        assert_eq!(app.helper.command(&b), b.helper(&executable));
        assert_eq!(app.claude_hook_command(), before.claude_hook_command());
        assert_eq!(
            app.claude_hook_command().unwrap(),
            format!("{} claude-attestation-hook", word(&executable))
        );
        // The stock app keeps no copy of its own.
        assert!(!crate::probe_copy::own_destination(&home.join(PROFILE)).exists());
    }
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
    let app = development().with_core_sockets(&config, Some(&source));
    let b = binding();
    assert!(app.helper.command(&b).is_some());
    assert_eq!(app.helper.command(&b), before.helper.command(&b));
    assert_eq!(app.claude_hook_command(), before.claude_hook_command());
    assert!(app.claude_hook_command().unwrap().ends_with(" --dev"));
    assert_eq!(app.helper.server(), NAME);
}

/// Cursor's guide hook names the profile build's session socket, beside
/// Agent Relay's own entry, and each build takes out only its own: through
/// the shared copy for a profile not built to coexist, through its own for
/// one built to coexist.
#[cfg(not(windows))]
#[test]
fn a_profile_builds_cursor_guide_hook_names_its_session_socket() {
    for coexists in [false, true] {
        profile_cursor_guide_hook_names_its_session_socket(coexists);
    }
}

#[cfg(not(windows))]
fn profile_cursor_guide_hook_names_its_session_socket(coexists: bool) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, session) = profile(home, &format!("cursor{}", u8::from(coexists)));
    let executable = probe(home);
    let app = profile_installer(home, &config, coexists);
    let default = installer(home);
    app.set_cursor_hooks(true).unwrap();
    default.set_cursor_hooks(true).unwrap();
    let theirs = if coexists {
        crate::probe_copy::own_destination(&config.app_data_dir)
    } else {
        executable.clone()
    };
    let ours = format!(
        "{} cursor-guide-hook --socket {}",
        word(&theirs),
        word(&session)
    );
    let commands = |event: &str| -> Vec<String> {
        let hooks: Value =
            serde_json::from_slice(&fs::read(home.join(".cursor/hooks.json")).unwrap()).unwrap();
        hooks["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|hook| hook["command"].as_str().map(str::to_string))
            .filter(|command| command.contains(" cursor-guide-hook"))
            .collect()
    };
    for event in crate::cursor_guide::EVENTS {
        let found = commands(event);
        assert!(found.contains(&ours), "{event}: {found:?}");
        assert!(
            found.contains(&format!("{} cursor-guide-hook", word(&executable))),
            "{event}: {found:?}"
        );
    }
    app.set_cursor_hooks(false).unwrap();
    for event in crate::cursor_guide::EVENTS {
        assert_eq!(
            commands(event),
            [format!("{} cursor-guide-hook", word(&executable))],
            "{event}"
        );
    }
}

/// A site profile built to coexist, with no stock app installed, runs its
/// own copy and its settings name it; it creates no shared copy.
#[test]
fn a_profile_built_to_coexist_alone_runs_its_own_copy() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, _) = profile(home, "alone");
    let app = profile_installer(home, &config, true);
    let b = binding();
    fs::create_dir_all(&app.claude.home).unwrap();
    app.select(vec!["claude".into()], Some(&b), None).unwrap();
    assert!(app.refresh(Some(&b)).is_empty());
    let own = own_copy(&config);
    assert!(!crate::probe_copy::destination(home).exists());
    let claude: Value =
        serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(
        claude["mcpServers"][SERVER]["headersHelper"],
        b.helper_at(&own, Some(&config.paths.socket))
            .unwrap()
            .as_str()
    );
    let hook = app.claude_hook_command().unwrap();
    assert!(hook.starts_with(&format!("{} ", word(&own))), "{hook}");
    let settings: Value =
        serde_json::from_slice(&fs::read(app.claude.home.join("settings.json")).unwrap()).unwrap();
    assert!(
        claude_hook::current_for(&settings, &hook, &app.helper.hook_build().unwrap()),
        "{settings}"
    );
}

/// A site profile built to coexist: its entries and hooks written when it
/// ran the shared copy move to its own copy; the stock app's stay as they
/// were.
#[test]
fn a_coexisting_profiles_entry_on_the_shared_copy_moves_to_its_own() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, session) = profile(home, "moves");
    let shared = probe(home);
    let b = binding();
    let stock = b.helper(&shared).unwrap();
    let before = b.helper_at(&shared, Some(&config.paths.socket)).unwrap();
    let app = profile_installer(home, &config, true);
    fs::create_dir_all(&app.claude.home).unwrap();
    // Claude: the stock app's hook and this build's on the shared copy.
    let hook_group = |command: String, matcher: String| {
        serde_json::json!({"matcher": matcher, "hooks": [
            {"type": "command", "command": command, "timeout": 5}
        ]})
    };
    let stock_hook = hook_group(
        format!("{} claude-attestation-hook", word(&shared)),
        crate::claude_attestation::MATCHER.into(),
    );
    let old_hook = hook_group(
        format!(
            "{} claude-attestation-hook --socket {}",
            word(&shared),
            word(&session)
        ),
        crate::claude_attestation::matcher_for(SERVER),
    );
    fs::write(
        app.claude.home.join("settings.json"),
        serde_json::to_vec(&serde_json::json!({"hooks": {
            "PreToolUse": [stock_hook.clone(), old_hook],
        }}))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        home.join(".claude.json"),
        serde_json::to_vec(&serde_json::json!({"mcpServers": {
            NAME: {"type": "http", "url": b.endpoint().unwrap(), "headersHelper": stock},
            SERVER: {"type": "http", "url": b.endpoint().unwrap(), "headersHelper": before},
        }}))
        .unwrap(),
    )
    .unwrap();
    // Codex: moving its own helper needs no logout, which would sign it out.
    fs::create_dir_all(&app.codex.home).unwrap();
    let endpoint = b.endpoint().unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        format!(
            "[mcp_servers.{NAME}]\nurl = {endpoint:?}\nhttp_headers_helper = {stock:?}\n\n\
             [mcp_servers.{SERVER}]\nurl = {endpoint:?}\nhttp_headers_helper = {before:?}\n"
        ),
    )
    .unwrap();
    app.select(vec!["claude".into(), "codex".into()], Some(&b), None)
        .unwrap();
    assert!(app.refresh(Some(&b)).is_empty());
    let own = own_copy(&config);
    let theirs = b.helper_at(&own, Some(&config.paths.socket)).unwrap();
    let claude: Value =
        serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(claude["mcpServers"][NAME]["headersHelper"], stock.as_str());
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
        Some(stock.as_str())
    );
    assert_eq!(
        codex["mcp_servers"][SERVER]["http_headers_helper"].as_str(),
        Some(theirs.as_str())
    );
    let settings: Value =
        serde_json::from_slice(&fs::read(app.claude.home.join("settings.json")).unwrap()).unwrap();
    let groups = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups[0], stock_hook, "{settings}");
    let hook = app.claude_hook_command().unwrap();
    assert!(hook.starts_with(&format!("{} ", word(&own))), "{hook}");
    let profile_hooks: Vec<&str> = groups
        .iter()
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .filter_map(|hook| hook["command"].as_str())
        .filter(|command| command.contains(" --socket "))
        .collect();
    assert_eq!(profile_hooks, [hook.as_str()], "{settings}");
    assert!(
        claude_hook::current_for(&settings, &hook, &app.helper.hook_build().unwrap()),
        "{settings}"
    );
    #[cfg(not(windows))]
    cursor_guide_hook_moves_to_the_own_copy(home, &app, &shared, &session, &own);
    assert_shared_untouched(home);
}

/// Cursor: the guide hook this build wrote for the shared copy moves to its
/// own copy, and the stock app's stays as it was.
#[cfg(not(windows))]
fn cursor_guide_hook_moves_to_the_own_copy(
    home: &Path,
    app: &Installer,
    shared: &Path,
    session: &Path,
    own: &Path,
) {
    let stock = format!("{} cursor-guide-hook", word(shared));
    let old = format!(
        "{} cursor-guide-hook --socket {}",
        word(shared),
        word(session)
    );
    let entries = serde_json::json!([
        {"command": stock, "timeout": 20},
        {"command": old, "timeout": 20},
    ]);
    let mut hooks = serde_json::Map::new();
    for event in crate::cursor_guide::EVENTS {
        hooks.insert(event.into(), entries.clone());
    }
    let file = home.join(".cursor/hooks.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        serde_json::to_vec(&serde_json::json!({"version": 1, "hooks": hooks})).unwrap(),
    )
    .unwrap();
    app.set_cursor_hooks(true).unwrap();
    let ours = format!("{} cursor-guide-hook --socket {}", word(own), word(session));
    let hooks: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    for event in crate::cursor_guide::EVENTS {
        let found: Vec<&Value> = hooks["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| {
                entry["command"]
                    .as_str()
                    .is_some_and(|command| command.contains(" cursor-guide-hook"))
            })
            .collect();
        assert_eq!(
            found,
            [
                &serde_json::json!({"command": stock, "timeout": 20}),
                &serde_json::json!({"command": ours, "timeout": 20}),
            ],
            "{event}: {hooks}"
        );
    }
}

/// A site profile not built to coexist (it takes over Agent Relay): its
/// entry on the shared copy stays exactly as it was, and it keeps no copy
/// of its own.
#[test]
fn a_profile_not_built_to_coexist_keeps_its_entry_on_the_shared_copy() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, _) = profile(home, "stays");
    let shared = probe(home);
    let b = binding();
    let stock = b.helper(&shared).unwrap();
    let before = b.helper_at(&shared, Some(&config.paths.socket)).unwrap();
    let app = profile_installer(home, &config, false);
    fs::create_dir_all(&app.claude.home).unwrap();
    fs::write(
        home.join(".claude.json"),
        serde_json::to_vec(&serde_json::json!({"mcpServers": {
            NAME: {"type": "http", "url": b.endpoint().unwrap(), "headersHelper": stock},
            SERVER: {"type": "http", "url": b.endpoint().unwrap(), "headersHelper": before},
        }}))
        .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    let endpoint = b.endpoint().unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        format!(
            "[mcp_servers.{NAME}]\nurl = {endpoint:?}\nhttp_headers_helper = {stock:?}\n\n\
             [mcp_servers.{SERVER}]\nurl = {endpoint:?}\nhttp_headers_helper = {before:?}\n"
        ),
    )
    .unwrap();
    app.select(vec!["claude".into(), "codex".into()], Some(&b), None)
        .unwrap();
    assert!(app.refresh(Some(&b)).is_empty());
    let claude: Value =
        serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(claude["mcpServers"][NAME]["headersHelper"], stock.as_str());
    assert_eq!(
        claude["mcpServers"][SERVER]["headersHelper"],
        before.as_str()
    );
    let codex: toml_edit::DocumentMut = fs::read_to_string(app.codex.home.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        codex["mcp_servers"][NAME]["http_headers_helper"].as_str(),
        Some(stock.as_str())
    );
    assert_eq!(
        codex["mcp_servers"][SERVER]["http_headers_helper"].as_str(),
        Some(before.as_str())
    );
    assert!(!crate::probe_copy::own_destination(&config.app_data_dir).exists());
    assert_shared_untouched(home);
}

/// A site profile built to coexist whose own copy cannot be written still
/// sets up Copilot's hooks: they run the core executable, never that copy.
#[test]
fn an_own_copy_that_cannot_be_written_does_not_stop_copilot_hook_setup() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let (config, _) = profile(home, "copilot");
    let _ = fs::remove_dir_all(short("copilot"));
    // A file where the own copy's directory goes, so it is never written.
    fs::create_dir_all(&config.app_data_dir).unwrap();
    let blocked = crate::probe_copy::own_destination(&config.app_data_dir);
    fs::write(blocked.parent().unwrap(), "not a directory").unwrap();
    // Windows runs a file by its extension, not its mode.
    let core = home.join(if cfg!(windows) {
        "Agent Relay Helper.exe"
    } else {
        "Agent Relay Helper"
    });
    fs::write(&core, "synthetic core executable").unwrap();
    private_fs::set_mode(&core, 0o700).unwrap();
    let app = profile_installer(home, &config, true).with_copilot_executable(Some(core.clone()));
    assert!(!app.helper.ensure());
    assert!(app.helper.copilot_ready());
    #[cfg(target_os = "macos")]
    {
        // The whole Copilot write: it sets up the hooks and then the
        // observer, which `copilot_hooks_ready` also needs.
        let hosted = providers::HostedConfig {
            endpoint: binding().endpoint().unwrap(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer copilot-grant".into())]),
            bearer_token_file: None,
            copilot_stdio: None,
            cursor_stdio: None,
        };
        app.write_copilot(&hosted).unwrap();
        assert!(app.copilot_hooks_ready());
    }
    assert_eq!(app.copilot_helper(), core);
    let _ = fs::remove_dir_all(short("copilot"));
}
