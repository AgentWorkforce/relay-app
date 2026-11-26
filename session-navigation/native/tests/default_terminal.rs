//! The operating system's default terminal as each adapter reads it into
//! `Snapshot::default_terminal`: a modelled terminal, `Known(None)` (none
//! set, or one this crate does not model) or `Unknown` (unreadable).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(feature = "macos")]
mod common;

#[cfg(feature = "macos")]
mod macos {
    use super::common::*;
    use session_navigation::evidence::Fact;
    use session_navigation::model::TerminalKind;
    use session_navigation::ports::Observe;
    use session_navigation_native::macos::parse::apps_report;
    use session_navigation_native::macos::scripts::APPS_JXA;
    use session_navigation_native::MacObserver;

    fn observed(terminal: serde_json::Value) -> Fact<Option<TerminalKind>> {
        let mut machine = Machine::new();
        let mut value: serde_json::Value = serde_json::from_str(&machine.apps_json).unwrap();
        value["terminal"] = terminal;
        machine.apps_json = value.to_string();
        MacObserver::new(&machine.build(), cfg())
            .observe(&codex())
            .default_terminal
    }

    #[test]
    fn the_unix_executable_handler_maps_through_the_bundle_table() {
        let bundle = |b: &str| serde_json::json!({ "bundle": b });
        assert_eq!(
            observed(bundle("com.apple.Terminal")),
            Fact::Known(Some(TerminalKind::AppleTerminal))
        );
        assert_eq!(
            observed(bundle("com.googlecode.iterm2")),
            Fact::Known(Some(TerminalKind::ITerm2))
        );
        assert_eq!(
            observed(bundle("com.mitchellh.ghostty")),
            Fact::Known(Some(TerminalKind::Ghostty))
        );
        // A handler this crate does not model, or none at all.
        assert_eq!(observed(bundle("com.example.other")), Fact::Known(None));
        assert_eq!(
            observed(serde_json::json!({ "bundle": null })),
            Fact::Known(None)
        );
        assert_eq!(observed(serde_json::json!({})), Fact::Known(None));
        // Not readable (or an older script without the field).
        assert_eq!(observed(serde_json::Value::Null), Fact::Unknown);
    }

    #[test]
    fn the_recorded_report_without_the_field_is_unknown() {
        let snap = MacObserver::new(&Machine::new().build(), cfg()).observe(&codex());
        assert_eq!(snap.default_terminal, Fact::Unknown);
        let report = apps_report(APPS_JSON_WITH_TERMINAL.as_bytes(), &["com.apple.Terminal"]);
        assert_eq!(
            report.and_then(|r| r.terminal).and_then(|t| t.bundle),
            Some("com.apple.Terminal".to_owned())
        );
    }

    /// What the script printed on the Mac it was checked on (trimmed).
    const APPS_JSON_WITH_TERMINAL: &str = r#"{"schemes":{},"apps":{"com.apple.Terminal":{"path":"/System/Applications/Utilities/Terminal.app","version":"2.15"}},"front":null,"running":[],"terminal":{"bundle":"com.apple.Terminal"}}"#;

    #[test]
    fn the_script_reads_the_handler_in_the_same_call_without_automation() {
        assert!(APPS_JXA.contains("URLForApplicationToOpenContentType"));
        assert!(APPS_JXA.contains("public.unix-executable"));
        assert!(!APPS_JXA.contains("Application("), "scripts no app");
    }
}

#[cfg(feature = "linux")]
mod linux {
    use session_navigation::evidence::Fact;
    use session_navigation::model::TerminalKind;
    use session_navigation_native::linux::default_terminal::{
        gsettings_exec, list_entries, list_paths, read, ALTERNATIVE, GSETTINGS_ARGS,
    };
    use session_navigation_native::linux::LinuxConfig;
    use session_navigation_native::system::{FakeSystem, Output};

    const HOME: &str = "/home/example";

    fn cfg(desktop: Option<&str>) -> LinuxConfig {
        LinuxConfig::for_user(HOME, 1000)
            .with_session_env(desktop.map(|d| ("XDG_CURRENT_DESKTOP", d)))
    }

    fn desktop(fake: &FakeSystem, id: &str) {
        fake.add_file(&format!("/usr/share/applications/{id}.desktop"), "", 1);
    }

    #[test]
    fn nothing_anywhere_is_none_set() {
        assert_eq!(read(&FakeSystem::new(), &cfg(None)), Fact::Known(None));
        assert_eq!(
            read(&FakeSystem::new(), &cfg(Some("GNOME"))),
            Fact::Known(None),
            "no gsettings program"
        );
    }

    #[test]
    fn the_list_names_the_first_installed_terminal() {
        let fake = FakeSystem::new();
        fake.add_file(
            &format!("{HOME}/.config/xdg-terminals.list"),
            "# mine\n-org.gnome.Terminal.desktop\nfoot.desktop\n+kitty.desktop:new-window\nAlacritty.desktop\n",
            1,
        );
        desktop(&fake, "org.gnome.Terminal");
        desktop(&fake, "kitty");
        desktop(&fake, "Alacritty");
        // foot is not installed, GNOME Terminal is excluded.
        assert_eq!(
            read(&fake, &cfg(None)),
            Fact::Known(Some(TerminalKind::Kitty))
        );
        // A desktop-specific list comes first, and an unmodelled choice is
        // kept as none rather than swapped for the next one.
        fake.add_file(
            &format!("{HOME}/.config/kde-xdg-terminals.list"),
            "org.kde.yakuake.desktop\n",
            1,
        );
        desktop(&fake, "org.kde.yakuake");
        assert_eq!(read(&fake, &cfg(Some("KDE"))), Fact::Known(None));
        assert_eq!(
            read(&fake, &cfg(None)),
            Fact::Known(Some(TerminalKind::Kitty))
        );
    }

    #[test]
    fn list_order_and_syntax() {
        let paths = list_paths(&cfg(Some("ubuntu:GNOME")));
        assert_eq!(
            &paths[..3],
            [
                format!("{HOME}/.config/ubuntu-xdg-terminals.list"),
                format!("{HOME}/.config/gnome-xdg-terminals.list"),
                format!("{HOME}/.config/xdg-terminals.list"),
            ]
        );
        assert!(paths.contains(&"/etc/xdg/xdg-terminals.list".to_owned()));
        assert!(paths.contains(&"/usr/share/xdg-terminal-exec/xdg-terminals.list".to_owned()));
        assert_eq!(
            list_entries(
                "a.desktop\n # c\n\n-b.desktop\n+c.desktop:x\nnot-a-desktop-id\n../d.desktop\n"
            ),
            vec!["a".to_owned(), "c".to_owned()]
        );
    }

    #[test]
    fn the_alternative_link_follows_an_empty_list() {
        let fake = FakeSystem::new();
        fake.add_symlink(ALTERNATIVE, "/usr/bin/gnome-terminal.wrapper");
        assert_eq!(
            read(&fake, &cfg(None)),
            Fact::Known(Some(TerminalKind::GnomeTerminal))
        );
        let fake = FakeSystem::new();
        fake.add_symlink(ALTERNATIVE, "/usr/bin/konsole");
        assert_eq!(
            read(&fake, &cfg(None)),
            Fact::Known(Some(TerminalKind::Konsole))
        );
        let fake = FakeSystem::new();
        fake.add_symlink(ALTERNATIVE, "/usr/bin/terminator");
        assert_eq!(read(&fake, &cfg(None)), Fact::Known(None));
    }

    #[test]
    fn gnome_setting_is_read_last_and_only_under_gnome() {
        let fake = FakeSystem::new();
        fake.add_executable("/usr/bin/gsettings", 1);
        fake.on_run(
            "/usr/bin/gsettings",
            &GSETTINGS_ARGS,
            Ok(Output::ok("'kitty'\n")),
        );
        assert_eq!(read(&fake, &cfg(Some("XFCE"))), Fact::Known(None));
        assert_eq!(
            read(&fake, &cfg(Some("ubuntu:GNOME"))),
            Fact::Known(Some(TerminalKind::Kitty))
        );
        assert_eq!(
            gsettings_exec(b"'gnome-terminal --wait'\n").as_deref(),
            Some("gnome-terminal")
        );
        assert_eq!(gsettings_exec(b"''\n"), None);
        let broken = FakeSystem::new();
        broken.add_executable("/usr/bin/gsettings", 1);
        broken.on_run(
            "/usr/bin/gsettings",
            &GSETTINGS_ARGS,
            Ok(Output::exit(1, "", "Cannot autolaunch D-Bus")),
        );
        assert_eq!(read(&broken, &cfg(Some("GNOME"))), Fact::Unknown);
        let no_schema = FakeSystem::new();
        no_schema.add_executable("/usr/bin/gsettings", 1);
        no_schema.on_run(
            "/usr/bin/gsettings",
            &GSETTINGS_ARGS,
            Ok(Output::exit(
                1,
                "",
                "No such schema “org.gnome.desktop.default-applications.terminal”",
            )),
        );
        assert_eq!(read(&no_schema, &cfg(Some("GNOME"))), Fact::Known(None));
    }

    #[test]
    fn a_source_present_but_unreadable_is_unknown() {
        let fake = FakeSystem::new();
        let list = format!("{HOME}/.config/xdg-terminals.list");
        fake.add_file(&list, "kitty.desktop\n", 1);
        fake.make_unreadable(&list);
        fake.add_symlink(ALTERNATIVE, "/usr/bin/xterm");
        assert_eq!(read(&fake, &cfg(None)), Fact::Unknown);
        let fake = FakeSystem::new();
        fake.add_symlink(ALTERNATIVE, "/usr/bin/xterm");
        fake.make_unreadable(ALTERNATIVE);
        assert_eq!(read(&fake, &cfg(None)), Fact::Unknown);
        // A list entry whose desktop file cannot be checked.
        let fake = FakeSystem::new();
        fake.add_file(&list, "kitty.desktop\n", 1);
        fake.make_unreadable(&format!("{HOME}/.local/share/applications/kitty.desktop"));
        assert_eq!(read(&fake, &cfg(None)), Fact::Unknown);
    }

    #[test]
    fn xdg_config_variables_move_the_lists() {
        let cfg = LinuxConfig::for_user(HOME, 1000)
            .with_xdg_config(Some("/cfg"), Some("/etc/a:relative:/etc/b"));
        assert_eq!(cfg.config_dirs, vec!["/cfg", "/etc/a", "/etc/b"]);
    }
}

#[cfg(feature = "windows")]
mod windows {
    use session_navigation::evidence::Fact;
    use session_navigation::model::TerminalKind;
    use session_navigation_native::windows::report::{default_terminal, Report};
    use session_navigation_native::windows::scripts::OBSERVE;

    const WT_CONSOLE: &str = "{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}";
    const WT_TERMINAL: &str = "{E12CFF52-A866-4C77-9A90-F570A7AA2C6B}";
    const CONHOST: &str = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}";
    const ZERO: &str = "{00000000-0000-0000-0000-000000000000}";

    fn read(json: &str) -> Fact<Option<TerminalKind>> {
        let report: Report = serde_json::from_str(json).unwrap();
        default_terminal(report.delegation.as_ref())
    }

    fn pair(console: &str, terminal: &str) -> Fact<Option<TerminalKind>> {
        read(&format!(
            r#"{{"bootId":1,"delegation":{{"Console":"{console}","Terminal":"{terminal}"}}}}"#
        ))
    }

    #[test]
    fn the_console_delegation_maps_to_a_terminal() {
        let wt = Fact::Known(Some(TerminalKind::WindowsTerminal));
        let conhost = Fact::Known(Some(TerminalKind::ConsoleHost));
        assert_eq!(pair(WT_CONSOLE, WT_TERMINAL), wt);
        assert_eq!(pair(&WT_CONSOLE.to_lowercase(), ""), wt);
        assert_eq!(pair("", WT_TERMINAL), wt);
        assert_eq!(pair(CONHOST, CONHOST), conhost);
        // "Let Windows decide" is not assumed to be Windows Terminal.
        assert_eq!(pair(ZERO, ZERO), Fact::Known(None));
        assert_eq!(pair("", ""), Fact::Known(None));
        // Preview's (or a third party's) classes and mixes are not modelled.
        assert_eq!(
            pair(
                "{06EC847C-C0A5-46B8-92CB-7C92F6E35CD5}",
                "{86633F1F-6454-40EC-89CE-DA4EBA977EE2}"
            ),
            Fact::Known(None)
        );
        assert_eq!(pair(WT_CONSOLE, CONHOST), Fact::Known(None));
    }

    #[test]
    fn an_unreadable_or_missing_section_is_unknown() {
        assert_eq!(read(r#"{"bootId":1,"delegation":null}"#), Fact::Unknown);
        assert_eq!(read(r#"{"bootId":1}"#), Fact::Unknown);
    }

    #[test]
    fn the_observe_script_reads_the_startup_key() {
        assert!(OBSERVE.contains(r"'HKCU:\Console\%%Startup'"));
        assert!(OBSERVE.contains("DelegationConsole") && OBSERVE.contains("DelegationTerminal"));
        assert!(OBSERVE.contains("delegation = $delegation"));
    }
}
