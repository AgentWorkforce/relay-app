use super::*;

const OURS: &str = "'/Apps/Agent Relay.app/probe' claude-attestation-hook";

fn others() -> Value {
    json!({
        "model": "opus",
        "hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "audit-bash"}]},
            ],
            "Stop": [{"hooks": [{"type": "command", "command": "echo done"}]}],
        },
    })
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn the_hook_matches_both_server_forms_and_keeps_other_hooks() {
    let installed = parse(&render(&others().to_string(), Some(OURS)).unwrap());
    let mut expected = others();
    expected["hooks"]["PreToolUse"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "matcher": MATCHER,
            "hooks": [{"type": "command", "command": OURS, "timeout": 5}],
        }));
    assert_eq!(installed, expected);
    assert!(current(&installed, OURS));
    // Installing again changes nothing.
    assert!(render(&installed.to_string(), Some(OURS)).is_none());

    let matcher = regex_lite(MATCHER);
    for name in [
        "mcp__agent-relay__send_message",
        "mcp__plugin_relay_agent-relay__join_relay",
    ] {
        assert!(matcher(name), "{name}");
    }
    for name in [
        "Bash",
        "mcp__github__create_issue",
        "mcp__agent-relay-sessions__send_relay_message",
        "xmcp__agent-relay__send_message",
    ] {
        assert!(!matcher(name), "{name}");
    }
}

/// The matcher's two anchored forms, checked without a regex engine.
fn regex_lite(matcher: &str) -> impl Fn(&str) -> bool {
    assert_eq!(
        matcher,
        "^(mcp__agent-relay__.+|mcp__plugin_.+_agent-relay__.+)$"
    );
    |name: &str| {
        name.strip_prefix("mcp__agent-relay__")
            .is_some_and(|tool| !tool.is_empty())
            || name
                .strip_prefix("mcp__plugin_")
                .and_then(|rest| rest.split_once("_agent-relay__"))
                .is_some_and(|(plugin, tool)| !plugin.is_empty() && !tool.is_empty())
    }
}

#[test]
fn an_older_copy_is_replaced_and_one_sharing_a_group_leaves_its_neighbor() {
    let old = json!({"hooks": {"PreToolUse": [
        {"matcher": "mcp__agent-relay__.*", "hooks": [
            {"type": "command", "command": "/old/probe relay claude-attestation-hook --dev"},
            {"type": "command", "command": "keep-me"},
        ]},
    ]}});
    let repaired = parse(&render(&old.to_string(), Some(OURS)).unwrap());
    assert_eq!(
        repaired["hooks"]["PreToolUse"],
        json!([
            {"matcher": "mcp__agent-relay__.*", "hooks": [{"type": "command", "command": "keep-me"}]},
            {"matcher": MATCHER, "hooks": [{"type": "command", "command": OURS, "timeout": 5}]},
        ])
    );
    assert!(current(&repaired, OURS));
    assert!(!current(&old, OURS));
    assert!(present(&old));
}

#[test]
fn removal_takes_out_only_agent_relay_hooks() {
    let installed = render(&others().to_string(), Some(OURS)).unwrap();
    let removed = parse(&render(&installed, None).unwrap());
    assert_eq!(removed, others());
    assert!(!present(&removed));
    // Nothing of ours: nothing to write.
    assert!(render(&others().to_string(), None).is_none());
    assert!(render("", None).is_none());

    let only = render("{}", Some(OURS)).unwrap();
    assert_eq!(parse(&render(&only, None).unwrap()), json!({}));
}

#[test]
fn unknown_shapes_are_left_unchanged() {
    for text in [
        "[1]",
        "not json",
        r#"{"hooks": []}"#,
        r#"{"hooks": {"PreToolUse": {"matcher": "x"}}}"#,
    ] {
        assert!(render(text, Some(OURS)).is_none(), "{text}");
        assert!(render(text, None).is_none(), "{text}");
    }
    assert!(render("", Some(OURS)).is_some(), "a new file starts empty");
}

#[test]
fn the_command_names_the_build_and_quotes_the_executable() {
    let production = command(Path::new("/Apps/Agent Relay.app/probe"), false, None).unwrap();
    let development = command(Path::new("/dev/probe"), true, None).unwrap();
    #[cfg(unix)]
    {
        assert_eq!(production, OURS);
        assert_eq!(development, "/dev/probe claude-attestation-hook --dev");
    }
    for command in [production, development] {
        assert!(ours(&json!({ "command": command })));
    }
    // The earlier form setup repair replaces.
    assert!(ours(
        &json!({"command": "/old/probe relay claude-attestation-hook --dev"})
    ));
    assert!(!ours(&json!({"command": "probe claude-session-hook"})));
}

#[test]
fn a_profile_builds_hook_sits_beside_agent_relays_and_each_keeps_its_own() {
    let socket = Path::new(
        "/Users/me/Library/Application Support/com.agentrelay.desktop.devaws/run/relay.sock",
    );
    let profile = command(
        Path::new("/Apps/Agent Relay.app/probe"),
        false,
        Some(socket),
    )
    .unwrap();
    #[cfg(unix)]
    assert_eq!(
        profile,
        format!("{OURS} --socket '/Users/me/Library/Application Support/com.agentrelay.desktop.devaws/run/relay.sock'")
    );
    let build = Build::new(Some(socket), "agent-relay-devaws").unwrap();
    // Agent Relay's own never claims a hook naming a session socket.
    assert!(!ours(&json!({ "command": profile })));
    assert!(!ours(
        &json!({"command": "/p claude-attestation-hook --socket /s --dev"})
    ));
    assert!(ours_for(&json!({ "command": profile }), &build, Kind::Attestation));
    assert!(!ours_for(&json!({ "command": OURS }), &build, Kind::Attestation));
    assert!(!ours_for(
        &json!({"command": "/p claude-attestation-hook --socket /other"}),
        &build,
        Kind::Attestation
    ));
    // Each build writes its own group and leaves the other's in place.
    let default = parse(&render(&others().to_string(), Some(OURS)).unwrap());
    let both = parse(&render_for(&default.to_string(), Some(&profile), &build).unwrap());
    assert!(current(&both, OURS), "{both}");
    assert!(current_for(&both, &profile, &build), "{both}");
    assert!(groups(&both, Kind::Attestation).any(|group| group["matcher"] == "^mcp__agent-relay-devaws__.+$"));
    assert_eq!(render(&both.to_string(), Some(OURS)), None);
    // Removing the profile build's hook restores Agent Relay's file exactly.
    let back = parse(&render_for(&both.to_string(), None, &build).unwrap());
    assert_eq!(back, default);
}

#[test]
fn the_teammate_context_hook_and_the_identity_hook_never_touch_each_other() {
    const TEAM: &str = "'/Apps/Agent Relay.app/probe' teammate-context-hook";
    const PROFILE: &str = "'/Apps/Agent Relay.app/probe' teammate-context-hook --socket /tmp/p.sock";
    let build = Build::production();
    let mut start = others();
    start["hooks"]["SessionStart"] = json!([
        {"hooks": [{"type": "command", "command": "probe claude-session-hook"}]},
        {"hooks": [{"type": "command", "command": PROFILE, "timeout": 5}]},
    ]);
    let identity = parse(&render(&start.to_string(), Some(OURS)).unwrap());
    let both = parse(
        &render_kind(&identity.to_string(), Some(TEAM), &build, Kind::TeammateContext).unwrap(),
    );
    assert!(current(&both, OURS));
    assert!(current_kind(&both, TEAM, &build, Kind::TeammateContext));
    let starts = both["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(starts.len(), 3, "{both}");
    assert_eq!(starts[..2], start["hooks"]["SessionStart"].as_array().unwrap()[..]);
    assert_eq!(
        starts[2],
        json!({"hooks": [{"type": "command", "command": TEAM, "timeout": 5}]})
    );
    // Again: nothing to write.
    assert!(render_kind(&both.to_string(), Some(TEAM), &build, Kind::TeammateContext).is_none());

    // Removing the teammate hook leaves the identity hook, relay-node's
    // SessionStart hook and the profile build's teammate hook.
    let without = parse(
        &render_kind(&both.to_string(), None, &build, Kind::TeammateContext).unwrap(),
    );
    assert_eq!(without, identity);
    // Removing the identity hook leaves the teammate hook.
    let only_team = parse(&render(&both.to_string(), None).unwrap());
    assert!(current_kind(&only_team, TEAM, &build, Kind::TeammateContext));
    assert!(!present(&only_team));
}
