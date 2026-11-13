use super::*;
use std::cell::{Cell, RefCell};
fn location() -> SessionLocation {
    serde_json::from_value(serde_json::json!({"application":"Kitty","bundle_id":"kitty","pid":71,"process_start":170,"tty":"/dev/pts/4","observed_at_ms":1,"host_pid":10,"host_process_start":20,"host_executable":"/usr/bin/kitty","boot_id":"boot-a","pane_id":"8","window_id":"99","focus_capability":"kitty","environment_identity":"unix:/run/user/1000/kitty.sock","runtime":"linux"})).unwrap()
}
fn plan(action: SessionOpenAction) -> SessionOpenPlan {
    serde_json::from_value(serde_json::json!({"action":action,"location":location(),"choices":[]}))
        .unwrap()
}
struct Fake {
    valid: Cell<bool>,
    commands: RefCell<Vec<Invocation>>,
    listing: String,
    fail: bool,
}
impl Fake {
    fn new(listing: &str) -> Self {
        Self {
            valid: Cell::new(true),
            commands: RefCell::default(),
            listing: listing.into(),
            fail: false,
        }
    }
}
impl Host for Fake {
    fn process_matches(&self, _: &SessionLocation) -> bool {
        self.valid.get()
    }
    fn run(&self, command: &Invocation) -> Result<String, String> {
        self.commands.borrow_mut().push(command.clone());
        if self.fail {
            Err("permission denied".into())
        } else if command
            .args
            .last()
            .is_some_and(|s| s == "ls" || s == "json")
        {
            Ok(self.listing.clone())
        } else {
            Ok(String::new())
        }
    }
    fn launch(&self, command: &Invocation) -> Result<(), String> {
        self.commands.borrow_mut().push(command.clone());
        Ok(())
    }
    fn window(&self, _: &SessionLocation) -> Result<Outcome, String> {
        Ok(Outcome::Verified)
    }
}
#[test]
fn exact_pane_uses_id_and_process_evidence_never_same_title_or_cwd() {
    let fake = Fake::new(
        r#"[{"tabs":[{"windows":[{"id":8,"pid":71,"title":"same","cwd":"/repo"},{"id":9,"pid":72,"title":"same","cwd":"/repo"}]}]}]"#,
    );
    assert_eq!(
        execute_with(&plan(SessionOpenAction::Focus), &fake),
        Ok(Outcome::Dispatched)
    );
    assert!(fake.commands.borrow()[1]
        .args
        .ends_with(&["--match".into(), "id:8".into()]));
    let wrong = Fake::new(r#"[{"id":8,"pid":72,"title":"same","cwd":"/repo"}]"#);
    assert!(execute_with(&plan(SessionOpenAction::Focus), &wrong).is_err());
    assert_eq!(wrong.commands.borrow().len(), 1);
}
#[test]
fn stale_process_missing_pane_and_denied_remote_control_never_resume() {
    let fake = Fake::new("[]");
    fake.valid.set(false);
    assert!(execute_with(&plan(SessionOpenAction::Focus), &fake).is_err());
    assert!(fake.commands.borrow().is_empty());
    fake.valid.set(true);
    assert!(execute_with(&plan(SessionOpenAction::Focus), &fake).is_err());
    assert_eq!(fake.commands.borrow().len(), 1);
    let mut denied = Fake::new("");
    denied.fail = true;
    assert!(execute_with(&plan(SessionOpenAction::Focus), &denied).is_err());
    assert_eq!(denied.commands.borrow().len(), 1);
}
#[test]
fn tty_must_be_exact_and_unique_even_with_same_title() {
    let target = location();
    assert!(pane_matches(&target, r#"[{"id":8,"tty":"/dev/pts/4"}]"#));
    assert!(!pane_matches(&target, r#"[{"id":8,"tty":"/dev/pts/40"}]"#));
    assert!(!pane_matches(
        &target,
        r#"[{"id":8,"tty":"/dev/pts/4"},{"id":8,"tty":"/dev/pts/4"}]"#
    ));
}
#[test]
fn structured_resume_preserves_literal_argv_and_original_host() {
    let command = SessionResumeCommand {
        environment: None,
        executable: "codex".into(),
        args: vec!["resume".into(), "id';$(touch BAD)".into()],
        cwd: "/repo's $HOME".into(),
    };
    let actual = resume_invocation(&command, Some(&location()), false).unwrap();
    assert_eq!(actual.executable, "/usr/bin/kitty");
    assert_eq!(
        actual.args,
        vec!["-e", "codex", "resume", "id';$(touch BAD)"]
    );
    assert_eq!(actual.cwd, Some(command.cwd.clone()));
    let mut old = location();
    old.host_executable = Some("/missing/original-app".into());
    assert!(resume_invocation(&command, Some(&old), false).is_err());
    old.runtime = Some("wsl:Ubuntu".into());
    assert!(resume_invocation(&command, Some(&old), false).is_err());
}
#[test]
fn windows_resume_restores_profile_without_shell_or_existing_live_tab() {
    let command = SessionResumeCommand {
        environment: None,
        executable: "codex.exe".into(),
        args: vec!["resume".into(), "id & calc".into()],
        cwd: r"C:\work\my repo".into(),
    };
    let mut old = location();
    old.runtime = Some("windows".into());
    old.host_executable = Some(r"C:\Apps\WindowsTerminal.exe".into());
    old.profile_id = Some("{profile}".into());
    let actual = resume_invocation(&command, Some(&old), true).unwrap();
    assert!(actual.args.starts_with(&[
        "new-tab".into(),
        "--startingDirectory".into(),
        command.cwd.clone()
    ]));
    assert!(actual
        .args
        .ends_with(&["codex.exe".into(), "resume".into(), "id & calc".into()]));
    assert!(!actual
        .args
        .iter()
        .any(|arg| arg == "cmd.exe" || arg == "/K"));
    let unix = SessionResumeCommand {
        cwd: "/home/me/work".into(),
        ..command
    };
    assert!(resume_invocation(&unix, Some(&old), true).is_err());
}
#[test]
fn observation_timestamp_does_not_change_target_identity() {
    let mut now = location();
    now.observed_at_ms += 30000;
    assert!(pane_matches(&now, r#"[{"id":8,"pid":71}]"#));
}

#[test]
fn native_window_selection_refuses_closed_reused_and_ambiguous_handles() {
    let target = location();
    assert_eq!(
        choose_window(&target, &[NativeWindow { id: 99, pid: 10 }]),
        Ok(99)
    );
    assert!(choose_window(&target, &[NativeWindow { id: 99, pid: 11 }]).is_err());
    assert!(choose_window(&target, &[NativeWindow { id: 100, pid: 10 }]).is_err());
    assert!(choose_window(&target, &[]).is_err());
    let mut without_handle = target;
    without_handle.window_id = None;
    assert_eq!(
        choose_window(&without_handle, &[NativeWindow { id: 99, pid: 10 }]),
        Ok(99)
    );
    assert!(choose_window(
        &without_handle,
        &[
            NativeWindow { id: 99, pid: 10 },
            NativeWindow { id: 100, pid: 10 }
        ]
    )
    .is_err());
}
#[test]
fn shell_syntax_never_enters_terminal_command_parsing() {
    let command = SessionResumeCommand {
        environment: None,
        executable: "claude".into(),
        args: vec!["--resume".into(), "literal; $HOME `cmd` & other".into()],
        cwd: "/repo".into(),
    };
    for terminal in ["kitty", "gnome-terminal", "konsole", "wezterm", "alacritty"] {
        let mut original = location();
        original.host_executable = Some(format!("/usr/bin/{terminal}"));
        let invocation = resume_invocation(&command, Some(&original), false).unwrap();
        assert_eq!(invocation.args.last(), command.args.last());
        assert!(!invocation
            .args
            .iter()
            .any(|arg| arg == "/bin/sh" || arg == "-lc"));
    }
}
#[test]
fn unknown_runtime_never_falls_back_to_native_terminal() {
    let command = SessionResumeCommand {
        environment: None,
        executable: "codex".into(),
        args: vec!["resume".into(), "id".into()],
        cwd: "/repo".into(),
    };
    for runtime in ["wsl", "wsl:Ubuntu", "ssh", "container:my-app", "unknown"] {
        let mut original = location();
        original.runtime = Some(runtime.into());
        assert!(
            resume_invocation(&command, Some(&original), false).is_err(),
            "{runtime}"
        );
    }
}
#[test]
fn missing_remote_control_endpoint_or_invalid_pane_is_not_a_default_target() {
    for (pane, endpoint) in [
        ("-1", Some("unix:/socket")),
        ("8", None),
        ("8", Some("tcp:host:5000")),
    ] {
        let mut action = plan(SessionOpenAction::Focus);
        let location = action.location.as_mut().unwrap();
        location.pane_id = Some(pane.into());
        location.environment_identity = endpoint.map(str::to_owned);
        let fake = Fake::new(r#"[{"id":8,"pid":71}]"#);
        assert!(execute_with(&action, &fake).is_err());
        assert!(fake.commands.borrow().is_empty());
    }
}
#[test]
fn remote_focus_requires_foreground_owner_or_exact_tty() {
    let target = location();
    assert!(pane_matches(
        &target,
        r#"[{"tabs":[{"windows":[{"id":8,"pid":40,"foreground_processes":[{"pid":71}]}]}]}]"#
    ));
    assert!(!pane_matches(
        &target,
        r#"[{"tabs":[{"windows":[{"id":8,"pid":40,"foreground_processes":[{"pid":72}]}]}]}]"#
    ));
}
#[test]
fn gnome_server_origin_resolves_the_public_client_for_real_distro_layouts() {
    let command = SessionResumeCommand {
        environment: None,
        executable: "claude".into(),
        args: vec!["--resume".into(), "id".into()],
        cwd: "/repo".into(),
    };
    for server in [
        "/usr/libexec/gnome-terminal-server",
        "/usr/lib/gnome-terminal/gnome-terminal-server",
    ] {
        let mut original = location();
        original.host_executable = Some(server.into());
        let actual = resume_invocation(&command, Some(&original), false).unwrap();
        assert_eq!(actual.executable, "gnome-terminal");
        assert_eq!(
            actual.args,
            vec![
                "--working-directory",
                "/repo",
                "--",
                "claude",
                "--resume",
                "id"
            ]
        );
        assert_eq!(actual.cwd, Some("/repo".into()));
    }
}
#[test]
fn link_dispatch_refuses_a_live_plan_whose_process_changed() {
    let fake = Fake::new("");
    fake.valid.set(false);
    let mut action = plan(SessionOpenAction::OpenLink);
    action.url = Some("codex://threads/id".into());
    assert!(execute_with(&action, &fake).is_err());
    assert!(fake.commands.borrow().is_empty());
}
#[test]
fn windows_terminal_delimiters_cannot_create_an_extra_tab_or_command() {
    let mut original = location();
    original.runtime = Some("windows".into());
    original.host_executable = Some("wt.exe".into());
    let command = SessionResumeCommand {
        environment: None,
        executable: "codex.exe".into(),
        args: vec!["resume".into(), "id; new-tab calc.exe".into()],
        cwd: r"C:\repo".into(),
    };
    assert!(resume_invocation(&command, Some(&original), true).is_err());
}
#[test]
fn process_evidence_rejects_reuse_other_users_exec_changes_zombies_and_reboot() {
    let mut target = location();
    target.executable = Some("/usr/bin/codex".into());
    let owner = ProcessEvidence {
        pid: 71,
        uid: 1000,
        start: 170,
        executable: Some("/usr/bin/codex".into()),
        zombie: false,
    };
    let host = ProcessEvidence {
        pid: 10,
        uid: 1000,
        start: 20,
        executable: Some("/usr/bin/kitty".into()),
        zombie: false,
    };
    let valid = |owner: &ProcessEvidence, host: &ProcessEvidence, boot: &str| {
        process_evidence_matches(&target, Some(owner), Some(host), 1000, Some(boot))
    };
    assert!(valid(&owner, &host, "boot-a"));
    assert!(!valid(
        &ProcessEvidence {
            start: 171,
            ..owner.clone()
        },
        &host,
        "boot-a"
    ));
    assert!(!valid(
        &owner,
        &ProcessEvidence {
            start: 21,
            ..host.clone()
        },
        "boot-a"
    ));
    assert!(!valid(
        &ProcessEvidence {
            uid: 1001,
            ..owner.clone()
        },
        &host,
        "boot-a"
    ));
    assert!(!valid(
        &owner,
        &ProcessEvidence {
            uid: 1001,
            ..host.clone()
        },
        "boot-a"
    ));
    assert!(!valid(
        &ProcessEvidence {
            executable: Some("/usr/bin/other".into()),
            ..owner.clone()
        },
        &host,
        "boot-a"
    ));
    assert!(!valid(
        &owner,
        &ProcessEvidence {
            executable: Some("/usr/bin/other".into()),
            ..host.clone()
        },
        "boot-a"
    ));
    assert!(!valid(
        &ProcessEvidence {
            zombie: true,
            ..owner.clone()
        },
        &host,
        "boot-a"
    ));
    assert!(!valid(&owner, &host, "boot-b"));
    assert!(!process_evidence_matches(
        &target,
        None,
        Some(&host),
        1000,
        Some("boot-a")
    ));
}
#[test]
fn stopped_claude_gui_handoff_runs_the_provider_cli_directly_on_windows() {
    let mut original = location();
    original.application = "Claude".into();
    original.runtime = Some("windows".into());
    original.host_executable = Some(r"C:\Apps\Claude.exe".into());
    let command = SessionResumeCommand {
        environment: None,
        executable: "claude".into(),
        args: vec!["--desktop".into(), "--resume".into(), "id".into()],
        cwd: r"C:\repo".into(),
    };
    let actual = resume_invocation(&command, Some(&original), true).unwrap();
    assert_eq!(actual.executable, "claude");
    assert_eq!(actual.args, command.args);
    assert!(resume_invocation(
        &SessionResumeCommand {
            cwd: "/repo".into(),
            ..command
        },
        Some(&location()),
        false
    )
    .is_err());
}
struct Tmux {
    commands: RefCell<Vec<Invocation>>,
    listings: RefCell<std::collections::VecDeque<String>>,
}
impl Tmux {
    fn new(clients: &[&str]) -> Self {
        Self {
            commands: RefCell::default(),
            listings: RefCell::new(clients.iter().map(|s| s.to_string()).collect()),
        }
    }
}
impl Host for Tmux {
    fn process_matches(&self, _: &SessionLocation) -> bool {
        true
    }
    fn run(&self, command: &Invocation) -> Result<String, String> {
        self.commands.borrow_mut().push(command.clone());
        if command.args.iter().any(|arg| arg == "display-message") {
            Ok("%8\t70\t/dev/pts/4\t$1\t@2".into())
        } else if command.args.iter().any(|arg| arg == "list-clients") {
            Ok(self.listings.borrow_mut().pop_front().unwrap_or_default())
        } else {
            Ok(String::new())
        }
    }
    fn launch(&self, _: &Invocation) -> Result<(), String> {
        panic!("live focus cannot start a session")
    }
    fn window(&self, _: &SessionLocation) -> Result<Outcome, String> {
        panic!("tmux does not own clients' GUI windows")
    }
}
fn tmux_plan() -> SessionOpenPlan {
    let mut action = plan(SessionOpenAction::Focus);
    let original = action.location.as_mut().unwrap();
    original.pane_id = Some("%8".into());
    original.focus_capability = Some("tmux".into());
    original.environment_identity = Some("/run/user/1000/tmux/default".into());
    original.host_executable = Some("/usr/bin/tmux".into());
    action
}
#[test]
fn tmux_switches_the_verified_attached_client_before_selecting_window_and_pane() {
    let clients = "/dev/pts/3\t80\t/dev/pts/3\t$1\n/dev/pts/9\t90\t/dev/pts/9\t$2";
    let host = Tmux::new(&[clients, clients]);
    assert_eq!(execute_with(&tmux_plan(), &host), Ok(Outcome::Dispatched));
    let commands = host.commands.borrow();
    assert_eq!(commands.len(), 6);
    assert!(commands[3].args.ends_with(&[
        "switch-client".into(),
        "-c".into(),
        "/dev/pts/3".into(),
        "-t".into(),
        "$1".into()
    ]));
    assert!(commands[4]
        .args
        .ends_with(&["select-window".into(), "-t".into(), "@2".into()]));
    assert!(commands[5]
        .args
        .ends_with(&["select-pane".into(), "-t".into(), "%8".into()]));
    assert!(commands.iter().all(|command| command
        .args
        .starts_with(&["-S".into(), "/run/user/1000/tmux/default".into()])));
}
#[test]
fn detached_tmux_session_is_unavailable_even_when_another_session_has_a_client() {
    let host = Tmux::new(&["/dev/pts/9\t90\t/dev/pts/9\t$2"]);
    let error = execute_with(&tmux_plan(), &host).unwrap_err();
    assert!(error.contains("attached terminal client"));
    assert_eq!(host.commands.borrow().len(), 2);
}
#[test]
fn tmux_client_disappearance_replacement_or_ambiguity_cancels_selection() {
    let first = "/dev/pts/3\t80\t/dev/pts/3\t$1";
    for second in [
        "",
        "/dev/pts/3\t81\t/dev/pts/3\t$1",
        "/dev/pts/3\t80\t/dev/pts/3\t$1\n/dev/pts/4\t82\t/dev/pts/4\t$1",
    ] {
        let host = Tmux::new(&[first, second]);
        assert!(execute_with(&tmux_plan(), &host).is_err());
        assert_eq!(host.commands.borrow().len(), 3);
    }
}
#[test]
fn process_exit_between_remote_listing_and_focus_cancels_dispatch() {
    struct Exiting {
        alive: Cell<bool>,
        commands: RefCell<Vec<Invocation>>,
    }
    impl Host for Exiting {
        fn process_matches(&self, _: &SessionLocation) -> bool {
            self.alive.get()
        }
        fn run(&self, command: &Invocation) -> Result<String, String> {
            self.commands.borrow_mut().push(command.clone());
            self.alive.set(false);
            Ok(r#"[{"id":8,"pid":71}]"#.into())
        }
        fn launch(&self, _: &Invocation) -> Result<(), String> {
            panic!("focus cannot resume")
        }
        fn window(&self, _: &SessionLocation) -> Result<Outcome, String> {
            panic!("exited target cannot raise a window")
        }
    }
    let host = Exiting {
        alive: Cell::new(true),
        commands: RefCell::default(),
    };
    assert!(execute_with(&plan(SessionOpenAction::Focus), &host).is_err());
    assert_eq!(host.commands.borrow().len(), 1);
}
#[test]
fn unix_provider_environment_is_restored_inside_the_terminal_child_as_literal_argv() {
    use std::collections::BTreeMap;
    let command = SessionResumeCommand {
        executable: "codex".into(),
        args: vec!["resume".into(), "id';$(touch BAD)".into()],
        cwd: "/repo".into(),
        environment: Some(BTreeMap::from([(
            "CODEX_HOME".into(),
            "/store's $(literal)".into(),
        )])),
    };
    let actual = resume_invocation(&command, Some(&location()), false).unwrap();
    assert!(actual.clear_provider_environment);
    assert_eq!(&actual.args[..2], &["-e", "/usr/bin/env"]);
    for key in PROVIDER_ENVIRONMENT_KEYS {
        assert!(actual.args.windows(2).any(|pair| pair == ["-u", key]));
    }
    assert!(actual.args.ends_with(&[
        "CODEX_HOME=/store's $(literal)".into(),
        "codex".into(),
        "resume".into(),
        "id';$(touch BAD)".into()
    ]));
    let empty = SessionResumeCommand {
        environment: Some(BTreeMap::new()),
        ..command
    };
    let actual = resume_invocation(&empty, Some(&location()), false).unwrap();
    assert!(actual.args.contains(&"/usr/bin/env".into()));
    assert!(actual
        .args
        .iter()
        .all(|arg| !arg.starts_with("CODEX_HOME=")));
}

#[test]
fn windows_store_wrapper_encodes_utf16_script_and_keeps_argv_out_of_shell_syntax() {
    use {base64::Engine, std::collections::BTreeMap};
    let mut original = location();
    original.runtime = Some("windows".into());
    original.host_executable = Some("wt.exe".into());
    let command = SessionResumeCommand {
        executable: r"C:\tools’;$HOME\codex.exe".into(),
        args: vec!["resume".into(), "id';& $HOME\"\\".into()],
        cwd: r"C:\repo’;Start-Process calc;#".into(),
        environment: Some(BTreeMap::from([(
            "CODEX_HOME".into(),
            r"C:\state’;Start-Process calc;# $HOME".into(),
        )])),
    };
    let actual = resume_invocation(&command, Some(&original), true).unwrap();
    assert!(actual
        .args
        .starts_with(&["new-tab".into(), "--".into(), "powershell.exe".into()]));
    assert!(actual.args.iter().any(|arg| arg == "-NoProfile"));
    assert!(
        !actual.args.iter().any(|arg| arg.contains(';')),
        "Semicolons exist only inside Base64, never Windows Terminal command delimiters"
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(actual.args.last().unwrap())
        .unwrap();
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let script = String::from_utf16(&units).unwrap();
    assert!(script.contains("$resume.UseShellExecute=$false"));
    let payload = script
        .strip_prefix("$payload='")
        .unwrap()
        .split_once("';")
        .unwrap()
        .0;
    assert!(payload
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c)));
    let data: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap(),
    )
    .unwrap();
    let decode = |value: &serde_json::Value| {
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(value.as_str().unwrap())
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(decode(&data["executable"]), command.executable);
    assert_eq!(decode(&data["cwd"]), command.cwd);
    for (actual, expected) in data["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&command.args)
    {
        assert_eq!(decode(actual), windows_argument(expected));
    }
    assert_eq!(
        data["arguments"].as_array().unwrap().len(),
        command.args.len()
    );
    assert_eq!(decode(&data["environment"][0]["key"]), "CODEX_HOME");
    assert_eq!(
        decode(&data["environment"][0]["value"]),
        command.environment.as_ref().unwrap()["CODEX_HOME"]
    );
    for value in [
        &command.executable,
        &command.cwd,
        &command.environment.as_ref().unwrap()["CODEX_HOME"],
    ] {
        assert!(
            !script.contains(value),
            "Data cannot occur in PowerShell source: {value}"
        );
    }
    assert!(script.contains("$resume.FileName=Decode $data.executable"));
    assert!(script.contains("[System.Diagnostics.Process]::Start($resume)"));
    assert_eq!(
        data["clear"]
            .as_array()
            .unwrap()
            .iter()
            .map(decode)
            .collect::<Vec<_>>(),
        PROVIDER_ENVIRONMENT_KEYS
    );
    assert_eq!(windows_argument("has\"quote\\"), "\"has\\\"quote\\\\\"");
    assert_eq!(windows_argument(""), "\"\"");
}

#[test]
fn unrelated_environment_variables_cannot_enter_the_resume_wrapper() {
    use std::collections::BTreeMap;
    let command = SessionResumeCommand {
        executable: "codex".into(),
        args: vec!["resume".into(), "id".into()],
        cwd: "/repo".into(),
        environment: Some(BTreeMap::from([(
            "LD_PRELOAD".into(),
            "/injected.so".into(),
        )])),
    };
    assert!(resume_invocation(&command, Some(&location()), false).is_err());
}
#[cfg(unix)]
#[test]
fn unix_environment_wrapper_actually_clears_inherited_store_and_preserves_captured_path() {
    use std::collections::BTreeMap;
    let directory = tempfile::tempdir().unwrap();
    let command = SessionResumeCommand {
        executable: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "printf '%s\\n' \"$CODEX_HOME\" \"${CLAUDE_CONFIG_DIR+x}\"".into(),
        ],
        cwd: directory.path().to_string_lossy().into_owned(),
        environment: Some(BTreeMap::from([(
            "CODEX_HOME".into(),
            "/original's $(literal)".into(),
        )])),
    };
    let invocation = resume_invocation(&command, Some(&location()), false).unwrap();
    let output = Command::new(&invocation.args[1])
        .args(&invocation.args[2..])
        .current_dir(command.cwd)
        .env("CODEX_HOME", "/wrong/ambient")
        .env("CLAUDE_CONFIG_DIR", "/wrong/ambient")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "/original's $(literal)\n\n"
    );
}

#[cfg(windows)]
#[test]
fn windows_environment_probe() {
    if std::env::var_os("GTK_SESSION_ENV_PROBE").is_none() {
        return;
    }
    let store = std::env::var("CODEX_HOME").unwrap();
    assert!(std::env::var_os("CLAUDE_CONFIG_DIR").is_none());
    assert_eq!(
        std::env::args().last().unwrap(),
        std::env::var("GTK_SESSION_EXPECTED_ARG").unwrap()
    );
    assert_eq!(
        std::env::current_dir().unwrap(),
        std::path::PathBuf::from(&store)
    );
    std::fs::write(
        std::path::Path::new(&store).join("probe.json"),
        serde_json::to_vec(&serde_json::json!({"store":store,"argument":std::env::args().last()}))
            .unwrap(),
    )
    .unwrap();
}

#[cfg(windows)]
#[test]
fn windows_encoded_environment_program_runs_an_isolated_process_probe() {
    use std::collections::BTreeMap;
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("state’;Start-Process calc;# $HOME");
    std::fs::create_dir(&store).unwrap();
    let argument = "literal id’;& $HOME\"\\";
    let command = SessionResumeCommand {
        executable: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        args: vec![
            "--exact".into(),
            "session_navigation::tests::windows_environment_probe".into(),
            "--nocapture".into(),
            "--skip".into(),
            argument.into(),
        ],
        cwd: store.to_string_lossy().into_owned(),
        environment: Some(BTreeMap::from([(
            "CODEX_HOME".into(),
            store.to_string_lossy().into_owned(),
        )])),
    };
    let program = windows_environment_program(&command);
    let output = Command::new(&program[0])
        .args(&program[1..])
        .env("GTK_SESSION_ENV_PROBE", "1")
        .env("GTK_SESSION_EXPECTED_ARG", argument)
        .env("CODEX_HOME", "C:\\wrong\\ambient")
        .env("CLAUDE_CONFIG_DIR", "C:\\wrong\\ambient")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.join("probe.json")).unwrap()).unwrap();
    assert_eq!(actual["store"], store.to_string_lossy().as_ref());
    assert_eq!(actual["argument"], argument);
}
#[cfg(not(windows))]
#[test]
fn explicit_default_terminal_plan_keeps_the_captured_provider_store() {
    use std::collections::BTreeMap;
    let mut action = plan(SessionOpenAction::Resume);
    action.location = None;
    action.command = Some(SessionResumeCommand {
        executable: "codex".into(),
        args: vec!["resume".into(), "id".into()],
        cwd: "/repo".into(),
        environment: Some(BTreeMap::from([(
            "CODEX_HOME".into(),
            "/original/store".into(),
        )])),
    });
    let host = Fake::new("");
    assert_eq!(execute_with(&action, &host), Ok(Outcome::Dispatched));
    let commands = host.commands.borrow();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].executable, "xdg-terminal-exec");
    assert!(commands[0]
        .args
        .iter()
        .any(|arg| arg == "CODEX_HOME=/original/store"));
}
