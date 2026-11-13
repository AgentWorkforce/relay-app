//! Synthetic tool-selection and management flows, shared by Linux and Windows.
use super::*;

fn report(complete: bool, configured: bool) -> Value {
    json!({"onboarding_complete":complete,"account_health":"ready","tools":[
        {"id":"codex","name":"Codex","detected":true,"supported":true,"selected":configured,
         "state":if configured {"configured"} else {"not_installed"},
         "status_label":if configured {"Configured"} else {"Not installed"},
         "detail":"Configuration does not verify that a running chat has loaded them.",
         "can_install":!configured,"can_repair":configured,"can_uninstall":configured},
        {"id":"claude","name":"Claude Code","detected":true,"supported":false,"selected":false,
         "state":"unavailable","status_label":"Unavailable","detail":"Agent Relay hasn’t added automatic setup for this provider yet.",
         "can_install":false,"can_repair":false,"can_uninstall":false},
        {"id":"opencode","name":"OpenCode","detected":false,"supported":false,"selected":false,
         "state":"unavailable","status_label":"Not detected","detail":"Not installed on this computer.",
         "can_install":false,"can_repair":false,"can_uninstall":false}
    ]})
}

pub(super) fn answers(core: &FakeCore, complete: bool) {
    core.answer("tools.status", report(complete, complete));
    core.answer("tools.check", report(true, true));
    core.answer("tools.select", report(true, true));
    core.answer("tools.manage", report(true, false));
}

fn show_chooser(case: &Case) {
    case.set_state(workspace_snapshot());
    case.app.show_window();
    case.pump(|case| {
        case.window_labels()
            .contains(&"Choose your providers".to_string())
    });
}

pub(super) fn absent_amp_and_capy_stay_out_of_onboarding_until_present(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::with(runtime, |core| {
        answers(core, false);
        let mut catalog = report(false, false);
        let mut amp = catalog["tools"][2].clone();
        amp["id"] = json!("amp");
        amp["name"] = json!("Amp");
        amp["detected"] = json!(false);
        amp["state"] = json!("external_setup");
        amp["status_label"] = json!("Connects through Agent Relay Cloud");
        catalog["tools"].as_array_mut().unwrap().push(amp.clone());
        amp["id"] = json!("capy");
        amp["name"] = json!("Capy");
        catalog["tools"].as_array_mut().unwrap().push(amp);
        core.answer("tools.status", catalog);
    });
    // Signed in to a development site, Amp opens that site's connections.
    let mut snapshot = workspace_snapshot();
    snapshot["account"]["site_url"] = json!("http://localhost:3100");
    case.set_state(snapshot);
    case.app.show_window();
    case.pump(|case| case.window_labels().contains(&"Choose your providers".into()));
    assert!(!case.window_labels().contains(&"Amp".into()));
    assert!(!case.window_labels().contains(&"Capy".into()));
    assert!(buttons(case.app.window(), "Open Cloud connections").is_empty());
    assert!(buttons(case.app.window(), "Set up in Capy").is_empty());
    assert!(case.called("tools.select").is_empty());
    assert!(case.called("tools.manage").is_empty());

    // Saved/configured rows and data-only detection are not local installs.
    let mut available = report(false, false);
    available["tools"][0]["name"] = json!("Installed Codex");
    for (id, name) in [("amp", "Amp"), ("capy", "Capy")] {
        let mut candidate = available["tools"][2].clone();
        candidate["id"] = json!(id); candidate["name"] = json!(name);
        candidate["detected"] = json!(true); candidate["selected"] = json!(true);
        candidate["state"] = json!("configured");
        candidate["onboarding_available"] = json!(false);
        available["tools"].as_array_mut().unwrap().push(candidate);
    }
    case.answer("tools.status", available.clone());
    case.app.refresh_tools();
    case.pump(|case| case.window_labels().contains(&"Installed Codex".into()));
    assert!(!case.window_labels().contains(&"Amp".into()));
    assert!(!case.window_labels().contains(&"Capy".into()));
    assert!(buttons(case.app.window(), "Open Cloud connections").is_empty());
    assert!(buttons(case.app.window(), "Set up in Capy").is_empty());

    // The core's app-installation witness admits Capy, still without a checkbox.
    available["tools"][4]["onboarding_available"] = json!(true);
    case.answer("tools.status", available);
    case.app.refresh_tools();
    case.pump(|case| case.window_labels().contains(&"Capy".into()));
    assert!(!case.window_labels().contains(&"Amp".into()));
    assert_eq!(buttons(case.app.window(), "Set up in Capy").len(), 1);
    let checks: Vec<_> = case.checks().iter().map(|check| check.widget_name().to_string()).collect();
    assert!(!checks.contains(&"Capy".to_string()), "{checks:?}");
    assert!(checks.contains(&"Installed Codex".to_string()), "{checks:?}");
    assert!(case.called("tools.select").is_empty());
    assert!(case.called("tools.manage").is_empty());

    // Explicit Settings still supports the existing configured Cloud connection.
    case.app.continue_without_tools();
    case.app.open_provider("amp");
    case.pump(|case| !buttons(case.app.window(), "Open Cloud connections").is_empty());
    let open = buttons(case.app.window(), "Open Cloud connections");
    assert_eq!(open.len(), 1);
    assert!(open[0].has_css_class("primary"));
    assert!(open[0].is_sensitive());
    case.desktop.opened.borrow_mut().clear();
    open[0].emit_clicked();
    assert_eq!(*case.desktop.opened.borrow(),
        vec!["http://localhost:3100/cloud/dashboard/settings#connections".to_string()]);
    assert!(case.called("tools.select").is_empty());
    assert!(case.called("tools.manage").is_empty());
}

pub(super) fn onboarding_does_not_wait_for_history_capture(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, false);
        let mut catalog = report(false, false);
        catalog["tools"][0]["state"] = json!("signed_out");
        catalog["tools"][0]["detail"] = json!("Sign in to Agent Relay to connect your providers.");
        core.answer("tools.status", catalog);
    });
    let mut snapshot = workspace_snapshot();
    snapshot["account"]["state"] = json!("preparing");
    snapshot["account"]["phase"] = json!("preparing_local_sessions");
    case.set_state(snapshot);
    case.app.show_window();
    case.pump(|case| case.window_labels().contains(&"Choose your providers".into()));
    assert!(case.called("tools.select").is_empty());
    assert!(case.window_labels().contains(&"Choose this provider now. Setup will finish after your local sessions are prepared.".into()));
    assert!(!case.window_labels().contains(&"Sign in to Agent Relay to connect your providers.".into()));
}

pub(super) fn cursor_is_selectable_in_provider_onboarding(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, false);
        let mut catalog = report(false, false);
        let mut cursor = catalog["tools"][0].clone();
        cursor["id"] = json!("cursor");
        cursor["name"] = json!("Cursor");
        catalog["tools"].as_array_mut().unwrap().push(cursor);
        core.answer("tools.status", catalog);
    });
    show_chooser(&case);
    let cursor = case.checks().into_iter().find(|check| check.widget_name() == "Cursor").unwrap();
    assert!(cursor.is_sensitive());
    cursor.set_active(true);
    buttons(case.app.window(), "Continue")[0].emit_clicked();
    case.pump(|case| !case.called("tools.select").is_empty());
    assert_eq!(case.called("tools.select")[0], json!({"tool_ids":["cursor"]}));
}

pub(super) fn reopened_chooser_keeps_saved_choices_until_continue(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| answers(core, true));
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| case.app.with_state(|state| state.tools.report.is_some()));
    case.app.open_provider("codex");
    case.settle();
    assert!(buttons(case.app.window(), "Choose providers…").is_empty(), "Individual providers do not reopen global onboarding");
    case.app.open_providers();
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    case.answer("tools.status", Answer::with(move |_| {
        let gate = held.clone();
        async move { gate.notified().await; Ok(report(true, true)) }
    }));
    let before = case.called("tools.status").len();
    case.app.refresh_tools();
    case.pump(|case| case.called("tools.status").len() > before);
    buttons(case.app.window(), "Choose providers…")[0].emit_clicked();
    case.pump(|case| case.window_labels().contains(&"Choose your providers".into()));
    assert!(!buttons(case.app.window(), "Continue")[0].is_sensitive());
    case.answer("tools.status", report(true, true));
    gate.notify_one();
    case.pump(|case| buttons(case.app.window(), "Continue").iter().any(|button| button.is_sensitive()));
    assert!(case.checks().iter().find(|check| check.widget_name() == "Codex").unwrap().is_active());
    buttons(case.app.window(), "Not now")[0].emit_clicked();
    case.pump(|case| !case.window_labels().contains(&"Choose your providers".into()));
    assert!(case.called("tools.select").is_empty(), "Opening and cancelling never clears existing connections");

    let mut catalog = report(true, true);
    for (id, supported) in [("paseo", false), ("replicas", false), ("pi", true)] {
        let mut hidden = catalog["tools"][0].clone();
        hidden["id"] = json!(id);
        hidden["name"] = json!(id);
        hidden["detected"] = json!(false);
        hidden["supported"] = json!(supported);
        catalog["tools"].as_array_mut().unwrap().push(hidden);
    }
    catalog["tools"][2]["detected"] = json!(true);
    catalog["tools"][2]["supported"] = json!(true);
    case.answer("tools.status", catalog);
    case.app.refresh_tools();
    case.pump(|case| case.app.with_state(|state| state.tools.report.as_ref().is_some_and(|report| report.tools.len() == 6)));
    case.app.open_providers();
    buttons(case.app.window(), "Choose providers…")[0].emit_clicked();
    case.pump(|case| case.window_labels().contains(&"Choose your providers".into()));
    case.checks().iter().find(|check| check.widget_name() == "Codex").unwrap().set_active(false);
    case.checks().iter().find(|check| check.widget_name() == "OpenCode").unwrap().set_active(true);
    buttons(case.app.window(), "Continue")[0].emit_clicked();
    case.pump(|case| !case.called("tools.select").is_empty());
    assert_eq!(case.called("tools.select")[0], json!({"tool_ids":["opencode", "pi", "replicas"]}));
}

pub(super) fn available_cloud_setup_prefers_installed_apps(runtime: &tokio::runtime::Runtime) {
    use agent_relay_app::models::CloudProviderSetup;
    let case = Case::with(runtime, |core| {
        answers(core, false);
        let mut catalog = report(false, false);
        for (id, name) in [("capy", "Capy"), ("grok-bot", "Grok Bot"), ("dot", "Dot"), ("muse-assistant", "Muse")] {
            let mut tool = catalog["tools"][1].clone();
            tool["id"] = json!(id);
            tool["name"] = json!(name);
            tool["detected"] = json!(id == "capy");
            // Installed-only onboarding admits Capy only with the core's local-install witness.
            tool["onboarding_available"] = json!(id == "capy");
            tool["state"] = json!("external_setup");
            catalog["tools"].as_array_mut().unwrap().push(tool);
        }
        core.answer("tools.status", catalog);
    });
    show_chooser(&case);
    assert!(case.checks().iter().all(|check| !matches!(check.widget_name().as_str(), "Capy" | "Grok Bot" | "Dot" | "Muse")));
    for (installed, fails) in [(false, false), (true, false), (true, true)] {
        case.desktop.cloud_apps_installed.set(installed);
        case.desktop.cloud_app_open_fails.set(fails);
        for setup in [CloudProviderSetup::Capy, CloudProviderSetup::GrokBot, CloudProviderSetup::Dot, CloudProviderSetup::MuseAssistant] {
            case.desktop.opened.borrow_mut().clear();
            let action = buttons(case.app.window(), setup.action());
            assert_eq!(action.len(), 1);
            assert!(action[0].has_css_class("primary"));
            action[0].clipboard().set_text("clipboard sentinel");
            action[0].emit_clicked();
            let expected = if setup.copy_only() { Vec::new() } else { match setup.app_url().filter(|_| installed) {
                Some(app) if fails => vec![app.to_string(), setup.web_url().into()],
                Some(app) => vec![app.to_string()],
                None => vec![setup.web_url().into()],
            }};
            assert_eq!(*case.desktop.opened.borrow(), expected);
            let copied = Rc::new(RefCell::new(None));
            let received = copied.clone();
            action[0].clipboard().read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
                *received.borrow_mut() = Some(result.unwrap().unwrap().to_string());
            });
            case.pump(|_| copied.borrow().is_some());
            assert_eq!(copied.borrow().as_deref(), Some(setup.prompt().unwrap_or("clipboard sentinel")));
            if let Some(copied) = setup.copied() { assert!(case.window_labels().contains(&copied.into())); }
            if setup.copy_only() {
                assert!(case.window_labels().contains(&setup.prompt().unwrap().into()));
                let open = buttons(case.app.window(), "Open Muse");
                assert_eq!(open.len(), 1);
                open[0].emit_clicked();
                assert_eq!(*case.desktop.opened.borrow(), vec![setup.web_url().to_string()]);
            }
        }
    }
    assert!(case.called("tools.select").is_empty());
    assert!(case.called("tools.manage").is_empty());
}

pub(super) fn pane_shows_all_tools_and_management_actions(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| answers(core, true));
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| {
        case.settings_labels()
            .contains(&"Setup not available yet".to_string())
            && !case.app.with_state(|state| state.tools.loading)
    });
    let shown = labels(&case.app.shell().unwrap().sidebar_rows);
    for label in ["Providers", "1 on", "Codex", "Claude Code", "Add provider"] {
        assert!(shown.contains(&label.into()), "{label}");
    }
    assert!(!shown.contains(&"OpenCode".into()));
    assert!(
        shown.iter().position(|name| name == "Claude Code")
            < shown.iter().position(|name| name == "Codex")
    );
    let sidebar = case.app.shell().unwrap().sidebar_rows.clone();
    buttons(&sidebar, "Claude Code")[0].emit_clicked();
    case.pump(|case| {
        case.settings_labels()
            .contains(&"Setup not available yet".into())
            && !case.app.with_state(|state| state.tools.loading)
    });
    assert!(buttons(&case.app.shell().unwrap().settings.column, "Install").is_empty());
    buttons(&sidebar, "Codex")[0].emit_clicked();
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    assert!(buttons(case.app.window(), "Connect Codex").is_empty());
    assert!(buttons(case.app.window(), "Fix Codex connection").is_empty());
    let details = provider_details(&case);
    assert!(!details.is_expanded());
    details.set_expanded(true);
    assert!(case.settings_labels().contains(&"Configured".into()));
    maintenance(&case, "Reinstall").emit_clicked();
    case.pump(|case| !case.called("tools.manage").is_empty());
    assert_eq!(
        case.called("tools.manage")[0],
        json!({"tool_id":"codex","action":"reinstall"})
    );
    case.pump(|case| !case.app.with_state(|state| state.tools.busy));
    provider_details(&case).set_expanded(true);
    buttons(case.app.window(), "Check again")[0].emit_clicked();
    case.pump(|case| !case.called("tools.check").is_empty());
}

pub(super) fn onboarding_persists_continue_and_not_now(runtime: &tokio::runtime::Runtime) {
    for skip in [false, true] {
        let case = Case::with(runtime, |core| answers(core, false));
        show_chooser(&case);
        let choices = case.checks();
        let codex = choices
            .iter()
            .find(|check| check.widget_name() == "Codex")
            .unwrap();
        assert!(choices.iter().all(|check| check.widget_name() != "Claude Code"));
        assert!(case.window_labels().contains(&"Claude Code".into()));
        assert!(choices
            .iter()
            .all(|check| check.widget_name() != "OpenCode"));
        codex.set_active(true);
        buttons(case.app.window(), if skip { "Not now" } else { "Continue" })[0].emit_clicked();
        case.pump(|case| !case.called("tools.select").is_empty());
        assert_eq!(
            case.called("tools.select")[0],
            json!({"tool_ids":if skip {Vec::<String>::new()} else {vec!["codex".into()]}})
        );
        case.pump(|case| {
            !case
                .window_labels()
                .contains(&"Choose your providers".to_string())
        });
    }
}

pub(super) fn installed_hosts_are_selectable_in_onboarding(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, false);
        let mut catalog = report(false, false);
        for (id, name) in [("conductor", "Conductor"), ("paseo", "Paseo")] {
            let mut host = catalog["tools"][1].clone();
            host["id"] = json!(id);
            host["name"] = json!(name);
            host["state"] = json!("external_setup");
            catalog["tools"].as_array_mut().unwrap().push(host);
        }
        core.answer("tools.status", catalog);
    });
    show_chooser(&case);
    for name in ["Conductor", "Paseo"] {
        let check = case
            .checks()
            .into_iter()
            .find(|check| check.widget_name() == name)
            .unwrap();
        assert!(check.is_sensitive());
        // Wait for layout, then measure the whole checkbox, including the
        // theme's padding and border rather than only its content height.
        case.pump(|_| check.is_mapped() && check.height() > 0);
        assert!(
            check.allocated_height() >= 28,
            "{name} checkbox allocated height: {}",
            check.allocated_height()
        );
        let row = check.parent().unwrap();
        let mark = check.next_sibling().unwrap();
        let title = mark.next_sibling().unwrap().first_child().unwrap();
        let center = |widget: &gtk::Widget| {
            let bounds = widget.compute_bounds(&row).unwrap();
            bounds.y() + bounds.height() / 2.0
        };
        assert!((center(check.upcast_ref()) - center(&mark)).abs() <= 1.0);
        assert!((center(&title) - center(&mark)).abs() <= 1.0);
        check.set_active(true);
    }
    assert!(case
        .checks()
        .iter()
        .find(|check| check.widget_name() == "Codex")
        .unwrap()
        .is_active());
    buttons(case.app.window(), "Continue")[0].emit_clicked();
    case.pump(|case| !case.called("tools.select").is_empty());
    assert_eq!(
        case.called("tools.select")[0],
        json!({"tool_ids":["codex", "conductor", "paseo"]})
    );
}

pub(super) fn codex_runtime_failure_rechecks_instead_of_reinstalling(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        let mut catalog = report(true, true);
        catalog["tools"][0]["state"] = json!("action_required");
        catalog["tools"][0]["status_label"] = json!("MCP not ready");
        core.answer("tools.status", catalog);
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.report.is_some() && !state.tools.loading)
    });
    case.app.open_provider("codex");
    case.pump(|case| {
        case.settings_labels()
            .contains(&"Reload Codex chats".into())
    });
    assert!(buttons(case.app.window(), "Fix Codex connection").is_empty());
    buttons(case.app.window(), "Check connection")[0].emit_clicked();
    case.pump(|case| !case.called("tools.check").is_empty());
    assert!(case.called("tools.manage").is_empty());
}

pub(super) fn full_access_is_on_the_corresponding_provider_page(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        let mut values = settings();
        values["accept_spawns"] = json!(true);
        core.answer("settings.get", check::<method::SettingsGet>(values));
    });
    case.sign_in();
    let mut snapshot = workspace_snapshot();
    snapshot["spawn"] = json!({"state":"live", "node_name":"test-desktop", "error":null,
    "harnesses":["codex","claude"], "harness_descriptors":[
        {"id":"codex","label":"Codex","billing":true,"full_access":{"detail":"Codex permissions","default_on":true}},
        {"id":"claude","label":"Claude","billing":true,"full_access":{"detail":"Claude permissions","default_on":true}}
    ]});
    case.set_state(snapshot);
    case.app.go(Page::Settings(Pane::Relay));
    case.settle();
    assert!(!case
        .settings_labels()
        .iter()
        .any(|label| label.ends_with(": full access")));
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.report.is_some() && !state.tools.loading)
    });
    for (id, own, other) in [
        ("codex", "Codex: full access", "Claude: full access"),
        ("claude", "Claude: full access", "Codex: full access"),
    ] {
        case.app.open_provider(id);
        case.pump(|case| case.settings_labels().contains(&own.into()));
        assert!(!case.settings_labels().contains(&other.into()));
    }
}

pub(super) fn failed_selection_stays_visible_with_explicit_escape_and_retry(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::with(runtime, |core| {
        answers(core, false);
        core.answer(
            "tools.select",
            Answer::fail(ErrorKind::Refused, "Disk is read-only"),
        );
    });
    show_chooser(&case);
    case.pump(|case| {
        case.window_labels()
            .contains(&"Choose your providers".to_string())
    });
    buttons(case.app.window(), "Continue")[0].emit_clicked();
    case.pump(|case| {
        case.window_labels().contains(
            &"Your provider choices could not be saved. Try again in Settings › Providers."
                .to_string(),
        )
    });
    assert!(case
        .window_labels()
        .contains(&"Choose your providers".to_string()));
    assert!(!case
        .app
        .with_state(|state| state.tools.report.as_ref().unwrap().onboarding_complete));
    buttons(case.app.window(), "Continue to app")[0].emit_clicked();
    case.pump(|case| {
        !case
            .window_labels()
            .contains(&"Choose your providers".to_string())
    });
    case.app.go(Page::Settings(Pane::Tools));
    let settings = case.app.shell().unwrap().settings.column.clone();
    case.pump(|case| {
        !case.app.with_state(|state| state.tools.loading)
            && buttons(&settings, "Try again")
                .first()
                .is_some_and(|button| button.is_sensitive())
    });
    assert!(case.settings_labels().contains(
        &"Your provider choices could not be saved. Try again in Settings › Providers.".to_string()
    ));
    let retries = buttons(&settings, "Try again");
    assert_eq!(
        retries.len(),
        1,
        "one Providers-specific retry must be present"
    );
    assert!(retries[0].is_sensitive());
    retries[0].emit_clicked();
    case.pump(|case| {
        case.window_labels()
            .contains(&"Choose your providers".to_string())
    });
    case.answer("tools.select", report(true, false));
    case.pump(|case| {
        buttons(case.app.window(), "Not now")
            .first()
            .is_some_and(|button| button.is_sensitive())
    });
    buttons(case.app.window(), "Not now")[0].emit_clicked();
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.report.as_ref().unwrap().onboarding_complete)
    });
}

pub(super) fn failed_management_keeps_confirmed_configuration(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        core.answer(
            "tools.manage",
            Answer::fail(ErrorKind::Refused, "Could not update settings"),
        );
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    case.app.open_provider("codex");
    case.pump(|case| {
        case.settings_labels().contains(&"Connected".to_string())
            && !case.app.with_state(|state| state.tools.loading)
    });
    maintenance(&case, "Disconnect").emit_clicked();
    case.pump(|case| {
        case.settings_labels().contains(
            &"Couldn’t update this connection. Try again or open Connection details.".to_string(),
        )
    });
    assert!(case.settings_labels().contains(&"Connected".to_string()));
    assert!(case
        .app
        .with_state(|state| state.tools.report.as_ref().unwrap().tools[0].selected));
}

pub(super) fn unsupported_core_never_blocks_sign_in(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    case.sign_in();
    assert!(case.called("tools.status").is_empty());
    assert!(!case
        .window_labels()
        .contains(&"Choose your providers".to_string()));
    case.app.go(Page::Settings(Pane::General));
    let sidebar = case.app.shell().unwrap().sidebar_rows.clone();
    buttons(&sidebar, "Check provider setup")[0].emit_clicked();
    assert!(case.settings_labels().contains(
        &"Provider setup is unavailable. Update or restart Agent Relay and try again.".to_string()
    ));
    assert!(buttons(case.app.window(), "Check again").is_empty());
}

pub(super) fn stale_tools_action_cannot_replace_reconnected_state(
    runtime: &tokio::runtime::Runtime,
) {
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    let case = Case::with(runtime, move |core| {
        answers(core, true);
        core.answer(
            "tools.manage",
            Answer::with(move |_| {
                let gate = held.clone();
                async move {
                    gate.notified().await;
                    Ok(report(true, false))
                }
            }),
        );
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    case.app.manage_tool("codex", "uninstall");
    case.pump(|case| !case.called("tools.manage").is_empty());
    case.app.on_connection(false, Some("Restarting".into()));
    case.app.on_connection(true, None);
    case.set_state(workspace_snapshot());
    // The report from before the reconnect already shows the tool selected,
    // so wait for the new connection to clear it and read it again; a slow
    // runner otherwise reached the assert while that read was still out.
    case.pump(|case| case.app.with_state(|state| state.tools.report.is_none()));
    case.pump(|case| {
        case.app.with_state(|state| {
            state
                .tools
                .report
                .as_ref()
                .is_some_and(|report| report.tools[0].selected)
        })
    });
    gate.notify_waiters();
    case.settle();
    assert!(case
        .app
        .with_state(|state| state.tools.report.as_ref().unwrap().tools[0].selected));
}

pub(super) fn long_chooser_keeps_footer_outside_the_scroll(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, false);
        let mut catalog = report(false, false);
        catalog["tools"] = json!((0..20)
            .map(|index| {
                let mut tool = report(false, false)["tools"][0].clone();
                tool["id"] = json!(format!("tool-{index}"));
                tool["name"] = json!(format!("Tool {index}"));
                tool
            })
            .collect::<Vec<_>>());
        core.answer("tools.status", catalog);
    });
    show_chooser(&case);
    case.app.window().set_default_size(640, 520);
    case.pump(|case| !buttons(case.app.window(), "Continue").is_empty());
    case.settle();
    let control = &buttons(case.app.window(), "Continue")[0];
    let mut ancestor = control.parent();
    while let Some(widget) = ancestor {
        assert!(
            !widget.is::<gtk::ScrolledWindow>(),
            "footer must remain outside the list scroll"
        );
        ancestor = widget.parent();
    }
    let bounds = control.compute_bounds(case.app.window()).unwrap();
    assert!(bounds.y() >= 0.0);
    assert!(bounds.y() + bounds.height() <= case.app.window().height() as f32 + 1.0);
}

pub(super) fn unsupported_legacy_configuration_can_be_uninstalled(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        let mut status = report(true, true);
        status["tools"][2]["selected"] = json!(true);
        status["tools"][2]["can_uninstall"] = json!(true);
        status["tools"][2]["status_label"] = json!("Legacy configuration");
        core.answer("tools.status", status);
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    let sidebar = case.app.shell().unwrap().sidebar_rows.clone();
    buttons(&sidebar, "OpenCode")[0].emit_clicked();
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    provider_details(&case).set_expanded(true);
    assert!(case
        .settings_labels()
        .contains(&"Legacy configuration".into()));
    let removals = vec![maintenance(&case, "Disconnect")];
    assert_eq!(removals.len(), 1);
    assert!(buttons(case.app.window(), "Install").is_empty());
    removals[0].emit_clicked();
    case.pump(|case| !case.called("tools.manage").is_empty());
    assert_eq!(
        case.called("tools.manage")[0],
        json!({"tool_id":"opencode","action":"uninstall"})
    );
}

pub(super) fn visible_tools_poll_preserves_action_errors(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        core.answer(
            "tools.manage",
            Answer::fail(ErrorKind::Refused, "Could not update settings"),
        );
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    case.app.open_provider("codex");
    case.app.show_window();
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    maintenance(&case, "Disconnect").emit_clicked();
    let error = "Couldn’t update this connection. Try again or open Connection details.";
    case.pump(|case| case.settings_labels().contains(&error.to_string()));
    let before = case.called("tools.status").len();
    case.answer("tools.status", report(true, false));
    case.pump(|case| {
        case.called("tools.status").len() > before
            && case
                .settings_labels()
                .contains(&"Not connected".to_string())
    });
    assert!(case.settings_labels().contains(&error.to_string()));
    assert!(case.called("tools.check").is_empty());
}

/// A failed background read is not a failed action: the next successful
/// read clears it, as on the Mac.
pub(super) fn recovered_status_reads_clear_their_error(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| answers(core, true));
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    case.app.open_provider("codex");
    case.app.show_window();
    let read = "Couldn’t load provider status. Try again.";
    case.answer(
        "tools.status",
        Answer::fail(ErrorKind::Refused, "Provider status is unavailable"),
    );
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.error.as_deref() == Some(read))
    });
    case.answer("tools.status", report(true, true));
    case.pump(|case| case.app.with_state(|state| state.tools.error.is_none()));
}

pub(super) fn providers_sidebar_tracks_verified_state_and_keeps_navigation(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::with(runtime, |core| answers(core, true));
    case.sign_in();
    case.app.go(Page::Settings(Pane::General));
    case.app.show_window();
    let sidebar = case.app.shell().unwrap().sidebar_rows.clone();
    case.pump(|_| labels(&sidebar).contains(&"1 on".into()));
    let shown = labels(&sidebar);
    let position = |name: &str| shown.iter().position(|label| label == name).unwrap();
    assert!(position("Preferences") < position("Providers"));
    assert!(position("Providers") < position("Sessions"));
    assert!(!shown.contains(&"Tools".into()));
    assert!(!shown.contains(&"Azure".into()));
    assert!(!shown.contains(&"OpenAI".into()));
    let has_widget = |name: &str| {
        let mut found = false;
        walk(sidebar.upcast_ref(), &mut |widget| {
            found |= widget.widget_name() == name
        });
        found
    };
    assert!(has_widget("provider-status-codex-on"));
    assert!(has_widget("agent-mark-codex"));
    // Opening General independently refreshes settings and service state.
    // Let those navigation reads finish before testing the provider poll.
    case.pump(|case| case.app.with_state(|state| state.service_state.is_some()));
    case.settle();
    let settings = case.app.shell().unwrap().settings.column.clone();
    let existing = settings.first_child().unwrap();
    let mut changed = report(true, true);
    changed["account_health"] = json!("offline");
    case.answer("tools.status", changed);
    case.pump(|_| labels(&sidebar).contains(&"0 on".into()));
    assert!(has_widget("provider-status-codex-attention"));
    assert!(
        existing.parent().is_some(),
        "provider polling must not rebuild General settings"
    );
    buttons(&sidebar, "Codex")[0].emit_clicked();
    case.pump(|case| {
        case.settings_labels()
            .contains(&"Can’t verify connection".into())
            && !case.app.with_state(|state| state.tools.loading)
    });
    assert!(buttons(&sidebar, "OpenCode").is_empty());
    let mut enabled = report(true, true);
    enabled["tools"][2]["selected"] = json!(true);
    case.answer("tools.manage", enabled.clone());
    buttons(&sidebar, "Add provider")[0].emit_clicked();
    case.pump(|case| !buttons(&case.app.shell().unwrap().settings.column, "Add").is_empty());
    let enable = buttons(&case.app.shell().unwrap().settings.column, "Add")[0].clone();
    assert!(
        !enable.has_css_class("primary"),
        "a list of providers must not read as a column of primary actions"
    );
    enable.emit_clicked();
    // A background poll must not announce the provider as enabled before the
    // simulated user clicks Enable, or the available-provider row disappears.
    case.answer("tools.status", enabled);
    case.pump(|case| {
        case.app.with_state(|state| {
            state.tools.provider_id.as_deref() == Some("opencode") && !state.tools.busy
        })
    });
    assert_eq!(
        case.called("tools.manage").last().unwrap(),
        &json!({"tool_id":"opencode","action":"enable"})
    );
    assert!(has_widget("provider-status-opencode-unverified"));
    buttons(&sidebar, "OpenCode")[0].emit_clicked();
    case.pump(|case| {
        case.settings_labels()
            .contains(&"Setup not available yet".into())
            && !case.app.with_state(|state| state.tools.loading)
    });
    assert!(case
        .settings_labels()
        .contains(&"Setup not available yet".into()));
    assert_eq!(
        case.app.with_state(|state| state.tools.provider_id.clone()),
        Some("opencode".into())
    );
    case.settle();
    let selected_row = buttons(&sidebar, "OpenCode")[0].clone();
    let before = case.called("tools.status").len();
    case.pump(|case| {
        case.called("tools.status").len() > before
            && !case.app.with_state(|state| state.tools.loading)
    });
    assert_eq!(
        case.app.with_state(|state| state.tools.provider_id.clone()),
        Some("opencode".into())
    );
    assert_eq!(
        buttons(&sidebar, "OpenCode")[0],
        selected_row,
        "unchanged status polling must preserve provider navigation widgets"
    );
    buttons(&sidebar, "Providers")[0].emit_clicked();
    case.pump(|case| {
        case.settings_labels().contains(
            &"Installed providers appear in the sidebar automatically. Add others here, including GitHub Copilot and Amp.".into(),
        ) && !case.app.with_state(|state| state.tools.loading)
    });
    assert!(!case
        .settings_labels()
        .iter()
        .any(|text| text.contains("Use your Agent Relay login with this provider.")));
    assert!(case
        .app
        .with_state(|state| state.tools.provider_id.is_none()));
    let before = case.called("tools.status").len();
    case.pump(|case| {
        case.called("tools.status").len() > before
            && !case.app.with_state(|state| state.tools.loading)
    });
    assert!(case
        .app
        .with_state(|state| state.tools.provider_id.is_none()));
    case.answer("tools.manage", report(true, true));
    case.app.open_provider("opencode");
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    maintenance(&case, "Remove from sidebar").emit_clicked();
    case.answer("tools.status", report(true, true));
    case.pump(|case| {
        buttons(&sidebar, "OpenCode").is_empty() && !case.app.with_state(|state| state.tools.busy)
    });
    assert_eq!(
        case.called("tools.manage").last().unwrap(),
        &json!({"tool_id":"opencode","action":"disable"})
    );
}

/// A provider catalogue for session-control tests without changing the smaller
/// setup/onboarding fixtures used elsewhere.
pub(super) fn open_session_provider(case: &Case, id: &str) {
    let mut catalog = report(true, true);
    catalog["tools"][2]["detected"] = json!(true);
    for (provider, name) in [
        ("grok", "Grok"), ("devin", "Devin"), ("cursor", "Cursor"),
        ("openclaw", "OpenClaw"), ("hermes", "Hermes"),
    ] {
        let mut tool = catalog["tools"][0].clone();
        tool["id"] = json!(provider);
        tool["name"] = json!(name);
        if provider == "cursor" {
            tool["state"] = json!("external_setup");
            tool["supported"] = json!(false);
            tool["can_install"] = json!(false);
            tool["can_repair"] = json!(false);
            tool["can_uninstall"] = json!(false);
        }
        catalog["tools"].as_array_mut().unwrap().push(tool);
    }
    case.answer("tools.status", catalog);
    case.app.go(Page::Settings(Pane::Tools));
    case.app.refresh_tools();
    case.pump(|case| {
        !case.app.with_state(|state| state.tools.loading)
            && case.app.with_state(|state| state.tools.report.as_ref().is_some_and(
                |report| report.tools.iter().any(|tool| tool.id == "cursor")))
    });
    case.app.open_provider(id);
    case.pump(|case| case.app.with_state(|state| state.tools.provider_id.as_deref() == Some(id)));
    case.settle();
}

pub(super) fn open_claude(case: &Case) {
    case.answer("tools.status", report(true, true));
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.report.is_some() && !state.tools.loading)
    });
    case.app.open_provider("claude");
    case.pump(|case| {
        !case.app.with_state(|state| state.tools.loading)
            && case
                .settings_labels()
                .contains(&"Claude Code delivery".into())
    });
}

pub(super) fn provider_preferences_have_one_home(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        let mut catalog = report(true, true);
        let mut grok = catalog["tools"][0].clone();
        grok["id"] = json!("grok");
        grok["name"] = json!("Grok");
        catalog["tools"].as_array_mut().unwrap().push(grok);
        core.answer("tools.status", catalog);
    });
    case.sign_in();
    case.app.open_provider("codex");
    case.pump(|case| case.settings_labels().contains(&"Local sessions".into()));
    assert!(case.settings_labels().contains(&"Codex".into()));
    assert!(case.settings_labels().contains(&"Open with".into()));
    assert!(
        !case.settings_labels().contains(&"Providers".into()),
        "the provider page names itself without a generic heading"
    );
    assert!(!case
        .settings_labels()
        .contains(&"Claude Code folders".into()));
    assert!(!case.settings_labels().contains(&"Upload policy".into()));
    let sidebar = labels(&case.app.shell().unwrap().sidebar_rows);
    assert!(!sidebar.contains(&"Opening sessions".into()));
    assert!(!sidebar.contains(&"Session sharing".into()));
    let opening_requirements = || {
        let mut found = None;
        walk(
            case.app.shell().unwrap().settings.column.upcast_ref(),
            &mut |widget| {
                if let Some(expander) = widget
                    .downcast_ref::<gtk::Expander>()
                    .filter(|item| item.label().as_deref() == Some("Opening requirements"))
                {
                    found = Some(expander.clone());
                }
            },
        );
        found.expect("opening requirements disclosure")
    };
    assert!(!opening_requirements().is_expanded());
    opening_requirements().set_expanded(true);
    case.app.render_settings();
    assert!(opening_requirements().is_expanded());
    case.app.open_provider("grok");
    assert!(
        !opening_requirements().is_expanded(),
        "each provider keeps its own disclosure state"
    );
    case.app.open_provider("codex");
    assert!(opening_requirements().is_expanded());
    case.app.open_provider("grok");
    case.settle();
    assert!(buttons(
        &case.app.shell().unwrap().settings.column,
        "Check installed apps"
    )
    .is_empty());
    let grok = case.settings_labels();
    let opens = grok.iter().position(|label| label == "Open with");
    assert!(opens.is_some(), "{grok:?}");
    assert_eq!(
        grok.get(opens.unwrap() + 1).map(String::as_str),
        Some("Terminal")
    );
    open_claude(&case);
    assert!(case.settings_labels().contains(&"Local sessions".into()));
    assert!(case.settings_labels().contains(&"Sharing".into()));
    assert!(case
        .settings_labels()
        .contains(&"Claude Code folders".into()));
    let before = case.called("claude_delivery.status").len();
    case.app.activate();
    case.app.update_state(|state| state.poll_ticks = 5);
    case.pump(|case| case.called("claude_delivery.status").len() > before);
    let provider_labels = case.settings_labels();
    assert!(
        provider_labels
            .iter()
            .position(|label| label == "Claude Code folders")
            < provider_labels.iter().position(|label| label == "Messages")
    );
    // The session prompt belongs only to the manual upload policy. Polling
    // another policy hides its control without changing its saved setting.
    for (mode, shown) in [("new", false), ("all", false), ("selected", true)] {
        let mut status = uploader();
        status["sharing_mode"] = json!(mode);
        case.answer("uploader.status", check::<agent_relay_desktop_core::contract::method::UploaderStatus>(status));
        case.app.refresh_uploader(false);
        case.pump(|case| {
            let policy_matches = case.app.with_state(|state| {
                state.status.as_ref().is_some_and(|status| status.sharing_mode.as_str() == mode)
            });
            policy_matches && case.settings_labels().contains(
                &agent_relay_app::models::CLAUDE_SESSION_HOOK_TITLE.into()) == shown
        });
        assert_eq!(case.app.with_state(|state| state.settings.as_ref().and_then(
            |settings| settings.claude_session_hook)), Some(false));
    }
    assert!(case.called("settings.set").is_empty(), "visibility never changes the saved hook");
    // As on the Mac, delivery shows only while a Claude terminal session exists.
    case.app.update_state(|state| state.local_sessions.clear());
    case.app.render_settings();
    assert!(!case
        .settings_labels()
        .contains(&"Claude Code delivery".into()));
    case.app.go(Page::Settings(Pane::Relay));
    case.settle();
    assert!(!case
        .settings_labels()
        .contains(&"Claude Code delivery".into()));
    assert!(!case
        .settings_labels()
        .contains(&"Claude Code folders".into()));
    case.app.go(Page::Settings(Pane::General));
    case.pump(|case| case.settings_labels().contains(&"Upload policy".into()));
    assert!(case.settings_labels().contains(&"Applies to all providers. Shared sessions are visible to your workspace. Sessions you put on the relay are always shared.".into()));
    // Every session integration has one home, including Cursor's local hooks.
    let mut controls = settings();
    controls["codex_stop_hook"] = json!(false);
    controls["openclaw_gateway"] = json!(false);
    controls["hermes_agent"] = json!(false);
    case.answer("settings.get", check::<method::SettingsGet>(controls));
    case.app.load_settings();
    case.pump(|case| case.app.with_state(|state| state.settings.as_ref().is_some_and(
        |settings| settings.codex_stop_hook.is_some())));
    let rows = [
        ("grok", "Grok sessions"),
        ("opencode", "OpenCode sessions"),
        ("devin", "Devin sessions"),
        ("cursor", agent_relay_app::models::CURSOR_HOOKS_TITLE),
        ("codex", agent_relay_app::models::CODEX_HOOK_TITLE),
        ("openclaw", agent_relay_app::models::OPENCLAW_TITLE),
    ];
    for (id, title) in rows {
        open_session_provider(&case, id);
        let shown = case.settings_labels();
        assert!(shown.contains(&title.into()), "{title} on {id}: {shown:?}");
        for (other_id, other_title) in rows {
            if other_id != id {
                assert!(!shown.contains(&other_title.into()), "{other_title} must stay on {other_id}");
            }
        }
    }
    case.app.go(Page::Settings(Pane::Relay));
    case.settle();
    for (_, title) in rows {
        assert!(!case.settings_labels().contains(&title.into()), "{title} must not be duplicated on Relay");
    }
}

pub(super) fn provider_layout_fits_narrow_and_wide_windows(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        let mut catalog = report(true, true);
        let claude = &mut catalog["tools"][1];
        claude["supported"] = json!(true);
        claude["can_install"] = json!(true);
        claude["state"] = json!("not_installed");
        claude["status_label"] = json!("Not installed");
        core.answer("tools.status", catalog);
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.report.is_some() && !state.tools.loading)
    });
    case.app.open_provider("claude");
    case.pump(|case| {
        !buttons(
            &case.app.shell().unwrap().settings.column,
            "Connect Claude Code",
        )
        .is_empty()
    });
    for (name, appearance) in [("dark", Appearance::Dark), ("light", Appearance::Light)] {
        case.app.set_appearance(appearance);
        // GTK's application window has a 980px minimum, including its sidebar.
        for width in [980, 1200] {
            let shell = case.app.shell().unwrap();
            let minimum = shell.root.measure(gtk::Orientation::Horizontal, -1).0;
            assert!(
                minimum <= width,
                "provider settings minimum width {minimum} exceeds requested {width}"
            );
            // Set geometry before mapping: the initial X11 configure at the
            // app's 1280px default can otherwise race a resize immediately
            // after present(). Win32 also applies default size on remapping.
            case.app.window().set_visible(false);
            // A hidden X11 window retains its last native surface size.
            // Recreate that surface so this test measures the requested
            // geometry rather than the app's original default geometry.
            gtk::prelude::WidgetExt::unrealize(case.app.window());
            case.app.window().set_default_size(width, 780);
            // A second mapped frame follows a complete layout/paint pass.
            // Native window decorations may consume part of the requested
            // width, so require the content to fit rather than equal it.
            let frames = Rc::new(Cell::new(0));
            let observed = frames.clone();
            shell.root.add_tick_callback(move |root, _| {
                if root.is_mapped() {
                    observed.set(observed.get() + 1);
                }
                if observed.get() >= 2 {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
            case.app.show_window();
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                dispatch_one();
                if frames.get() >= 2 && shell.root.width() > 0 && shell.root.width() <= width {
                    break;
                }
            }
            assert!(
                frames.get() >= 2 && shell.root.width() > 0 && shell.root.width() <= width,
                "provider layout did not fit after mapping: content={}, window={}, minimum={minimum}, requested={width}, frames={}, mapped={}",
                shell.root.width(), case.app.window().width(), frames.get(), case.app.window().is_mapped()
            );
            assert_eq!(
                buttons(
                    &case.app.shell().unwrap().settings.column,
                    "Connect Claude Code"
                )
                .len(),
                1
            );
            assert!(
                case.app.shell().unwrap().root.width() <= width,
                "provider settings must not widen the window: actual={}, requested={width}",
                case.app.shell().unwrap().root.width()
            );
            if let Ok(dir) = std::env::var("PROVIDERS_RENDER_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                let paintable = gtk::WidgetPaintable::new(Some(case.app.window()));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(
                    &snapshot,
                    case.app.window().width() as f64,
                    case.app.window().height() as f64,
                );
                let node = snapshot
                    .to_node()
                    .expect("mapped provider page has a render node");
                let texture = case
                    .app
                    .window()
                    .renderer()
                    .unwrap()
                    .render_texture(&node, None);
                texture
                    .save_to_png(
                        std::path::Path::new(&dir).join(format!("claude-gtk-{name}-{width}.png")),
                    )
                    .unwrap();
            }
        }
    }
    assert!(case.called("tools.manage").is_empty());
    assert!(case.called("settings.set").is_empty());
}

pub(super) fn external_providers_offer_existing_setup_routes(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        let mut catalog = report(true, true);
        for (id, name) in [
            ("conductor", "Conductor"),
            ("paseo", "Paseo"),
            ("capy", "Capy"),
            ("replicas", "Replicas"),
            ("claude-cloud", "Claude cloud"),
        ] {
            let mut tool = catalog["tools"][0].clone();
            tool["id"] = json!(id);
            tool["name"] = json!(name);
            tool["selected"] = json!(false);
            tool["state"] = json!("external_setup");
            tool["can_install"] = json!(false);
            tool["can_repair"] = json!(false);
            tool["can_uninstall"] = json!(false);
            catalog["tools"].as_array_mut().unwrap().push(tool);
        }
        core.answer("tools.status", catalog);
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.report.is_some() && !state.tools.loading)
    });
    case.app.open_provider("conductor");
    case.settle();
    assert!(case
        .settings_labels()
        .contains(&"Manage the connection through the agent running inside Conductor.".into()));
    let content = case.app.shell().unwrap().settings.column.clone();
    buttons(&content, "Codex settings")[0].emit_clicked();
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.provider_id.as_deref() == Some("codex"))
    });
    case.app.open_provider("paseo");
    case.settle();
    assert_eq!(buttons(&content, "Grok settings").len(), 1);
    assert_eq!(buttons(&content, "Muse Code settings").len(), 1);
    buttons(&content, "Grok settings")[0].emit_clicked();
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.provider_overview && state.tools.provider_id.is_none())
    });
    assert!(case.settings_labels().contains(&"Add provider".into()));
    assert!(case.called("tools.manage").is_empty());
    case.app.open_provider("claude-cloud");
    case.settle();
    buttons(&content, "Claude Code settings")[0].emit_clicked();
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.provider_id.as_deref() == Some("claude"))
    });
    case.app.open_provider("capy");
    case.settle();
    assert!(buttons(&content, "Check again").is_empty());
    assert!(!case.settings_labels().contains(&"Capy setup".into()));
    let setup = buttons(&content, "Set up in Capy");
    assert!(setup[0].has_css_class("primary"));
    setup[0].emit_clicked();
    assert_eq!(
        case.desktop.opened.borrow().last().map(String::as_str),
        Some(agent_relay_app::models::CAPY_URL)
    );
    assert!(case
        .settings_labels()
        .contains(&agent_relay_app::models::CAPY_SETUP_COPIED.into()));
    case.app.open_provider("replicas");
    case.settle();
    assert!(buttons(&content, "Install").is_empty());
    assert!(buttons(&content, "Set up in Capy").is_empty());
}

pub(super) fn amp_and_copilot_pages_show_the_core_wording(runtime: &tokio::runtime::Runtime) {
    const AMP: &str = "Amp connects through Agent Relay Cloud, in your workspace's Settings under Connections. Open Cloud connections to add or change it there. Agent Relay Desktop never asks for or stores the Amp token, and Amp's connection is not verified here.";
    const COPILOT: &str = "GitHub Copilot needs VS Code with the GitHub Copilot Chat extension, signed in to GitHub inside VS Code. VS Code: detected at /usr/bin/code. GitHub Copilot Chat extension: detected at /home/person/.vscode/extensions/github.copilot-chat-0.72.0.";
    let case = Case::with(runtime, |core| {
        answers(core, true);
        let mut catalog = report(true, true);
        // Amp was added from Available providers; Copilot found VS Code and
        // was added there too. Added and not yet set up, the core reports
        // Copilot as Setup needed, installable and repairable.
        for (id, name, detected, state, label, detail) in [
            ("amp", "Amp", false, "external_setup", "Connects through Agent Relay Cloud", AMP),
            ("copilot", "GitHub Copilot (VS Code)", true, "action_required", "Setup needed", COPILOT),
        ] {
            let selected = true;
            let mut tool = catalog["tools"][0].clone();
            tool["id"] = json!(id);
            tool["name"] = json!(name);
            tool["detected"] = json!(detected);
            tool["supported"] = json!(id == "copilot");
            tool["selected"] = json!(selected);
            tool["state"] = json!(state);
            tool["status_label"] = json!(label);
            tool["detail"] = json!(detail);
            tool["can_install"] = json!(id == "copilot");
            tool["can_repair"] = json!(id == "copilot");
            tool["can_uninstall"] = json!(false);
            catalog["tools"].as_array_mut().unwrap().push(tool);
        }
        core.answer("tools.status", catalog);
    });
    case.sign_in();
    // Signed in to a development site, Amp opens that site's connections.
    let mut development = workspace_snapshot();
    development["account"]["site_url"] = json!("http://localhost:3100");
    case.set_state(development);
    case.pump(|case| {
        case.app.with_state(|state| {
            state.account().site_url.as_deref() == Some("http://localhost:3100")
        })
    });
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| {
        case.app
            .with_state(|state| state.tools.report.is_some() && !state.tools.loading)
    });
    let content = case.app.shell().unwrap().settings.column.clone();
    case.app.open_provider("amp");
    case.settle();
    let labels = case.settings_labels();
    assert!(labels.contains(&"Connects through Agent Relay Cloud".into()), "{labels:?}");
    assert!(labels.contains(&AMP.into()));
    // Amp's on state needs a ready account, so its details show that line.
    provider_details(&case).set_expanded(true);
    let labels = case.settings_labels();
    assert!(
        labels.contains(&"Agent Relay account: Verified".into()),
        "{labels:?}"
    );
    assert!(buttons(&content, "Check again").is_empty());
    let open = buttons(&content, "Open Cloud connections");
    assert_eq!(open.len(), 1);
    assert!(open[0].has_css_class("primary"));
    open[0].emit_clicked();
    assert_eq!(
        case.desktop.opened.borrow().last().map(String::as_str),
        Some("http://localhost:3100/cloud/dashboard/settings#connections")
    );
    case.app.open_provider("copilot");
    case.settle();
    let labels = case.settings_labels();
    assert!(!labels.contains(&"Setup not available yet".into()));
    // A detected provider's setup action is concise; the core's requirements
    // and exact detection locations are in its connection evidence disclosure.
    let details = provider_details(&case);
    assert!(!details.is_expanded());
    details.set_expanded(true);
    case.settle();
    let labels = case.settings_labels();
    assert!(labels.contains(&COPILOT.into()), "{labels:?}");
    assert!(buttons(&content, "Open Cloud connections").is_empty());
    // An added Copilot sets up through its one primary action, as on the Mac:
    // a selected provider's setup is a repair, named for its connection.
    let connect=buttons(&content,"Fix GitHub Copilot (VS Code) connection");
    assert_eq!(connect.len(),1);assert!(connect[0].has_css_class("primary"));connect[0].emit_clicked();
    case.pump(|case|!case.called("tools.manage").is_empty());
    assert_eq!(case.called("tools.manage")[0],json!({"tool_id":"copilot","action":"repair"}));
}

fn provider_details(case: &Case) -> gtk::Expander {
    let mut result = None;
    walk(
        case.app.shell().unwrap().settings.column.upcast_ref(),
        &mut |widget| {
            if let Some(details) = widget
                .downcast_ref::<gtk::Expander>()
                .filter(|details| details.label().as_deref() == Some("Connection details"))
            {
                result = Some(details.clone());
            }
        },
    );
    result.expect("connection evidence disclosure")
}
fn maintenance(case: &Case, label: &str) -> gtk::Button {
    let mut result = None;
    walk(
        case.app.shell().unwrap().settings.column.upcast_ref(),
        &mut |widget| {
            if let Some(menu) = widget
                .downcast_ref::<gtk::MenuButton>()
                .filter(|menu| menu.widget_name() == "provider-more-options")
            {
                menu.popup();
                result = buttons(&menu.popover().unwrap(), label).first().cloned();
            }
        },
    );
    result.expect("maintenance menu action")
}

pub(super) fn compact_provider_actions_match_the_next_step(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| {
        answers(core, true);
        core.answer("tools.status", report(true, false));
        core.answer("tools.manage", report(true, true));
    });
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    case.app.open_provider("codex");
    let content = case.app.shell().unwrap().settings.column.clone();
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    let connect = buttons(&content, "Connect Codex");
    assert_eq!(connect.len(), 1);
    assert!(connect[0].has_css_class("primary"));
    assert!(
        labels(&connect[0].parent().unwrap()).contains(&"Codex".into()),
        "the next step sits beside the provider's name"
    );
    assert!(
        !case
            .settings_labels()
            .iter()
            .any(|label| label.contains("Connect once")),
        "the action names what it connects without a filler sentence"
    );
    connect[0].emit_clicked();
    case.pump(|case| case.settings_labels().contains(&"Connected".into()));
    assert_eq!(
        case.called("tools.manage")[0],
        json!({"tool_id":"codex","action":"install"})
    );
    assert!(!provider_details(&case).is_expanded());
    assert!(buttons(&content, "Fix Codex connection").is_empty());
    assert!(buttons(&content, "Repair").is_empty());
    let mut failed = report(true, true);
    failed["tools"][0]["state"] = json!("action_required");
    case.answer("tools.status", failed.clone());
    case.app.refresh_tools();
    case.pump(|case| {
        !case.app.with_state(|state| state.tools.loading)
            && !buttons(&content, "Fix Codex connection").is_empty()
    });
    buttons(&content, "Fix Codex connection")[0].emit_clicked();
    case.pump(|case| case.called("tools.manage").len() == 2);
    assert_eq!(
        case.called("tools.manage")[1],
        json!({"tool_id":"codex","action":"repair"})
    );
    case.pump(|case| !case.app.with_state(|state| state.tools.busy));
    failed["account_health"] = json!("offline");
    case.answer("tools.status", failed.clone());
    case.app.refresh_tools();
    case.pump(|case| {
        !case.app.with_state(|state| state.tools.loading)
            && !buttons(&content, "Check connection").is_empty()
    });
    buttons(&content, "Check connection")[0].emit_clicked();
    case.settle();
    case.pump(|case| {
        !case.called("tools.check").is_empty() && !case.app.with_state(|state| state.tools.busy)
    });
    assert!(case
        .app
        .with_state(|state| state.tools.last_checked.is_some()));
    case.app
        .update_state(|state| state.tools.last_checked = Some("Previous manual check".into()));
    failed["account_health"] = json!("sign_in_required");
    case.answer("tools.status", failed);
    case.app.refresh_tools();
    case.pump(|case| {
        !case.app.with_state(|state| state.tools.loading)
            && !buttons(&content, "Sign in to Agent Relay").is_empty()
    });
    buttons(&content, "Sign in to Agent Relay")[0].emit_clicked();
    assert_eq!(
        case.app
            .with_state(|state| state.tools.last_checked.clone())
            .as_deref(),
        Some("Previous manual check")
    );
    assert_eq!(case.app.page(), Page::Settings(Pane::Account));
    // Provider-only reconsent must start renewal even while global auth is ready.
    let mut consent = report(true, true);
    consent["tools"][0]["state"] = json!("signed_out");
    case.answer("tools.status", consent);
    case.answer(
        "account.sign_in",
        json!({"state":"pending_approval","phase":"waiting_for_browser"}),
    );
    case.app.go(Page::Settings(Pane::Tools));
    case.app.open_provider("codex");
    case.app.refresh_tools();
    case.pump(|case| {
        !case.app.with_state(|state| state.tools.loading)
            && !buttons(case.app.window(), "Sign in to Agent Relay").is_empty()
    });
    let calls = case.called("account.sign_in").len();
    buttons(case.app.window(), "Sign in to Agent Relay")[0].emit_clicked();
    case.pump(|case| case.called("account.sign_in").len() == calls + 1);
    assert_eq!(
        case.called("account.sign_in").last().unwrap()["flow"],
        "browser"
    );
    assert!(case.called("account.sign_out").is_empty());
    assert_eq!(case.app.page(), Page::Settings(Pane::Account));

    let mut cleanup_failed = report(true, false);
    cleanup_failed["tools"][0]["state"] = json!("action_required");
    cleanup_failed["tools"][0]["can_uninstall"] = json!(true);
    case.answer("tools.status", cleanup_failed);
    case.app.open_provider("codex");
    case.pump(|case| case.settings_labels().contains(&"Needs attention".into()));
    assert!(!case.settings_labels().contains(&"Not connected".into()));
    assert!(buttons(&content, "Connect Codex").is_empty());
    assert!(buttons(&content, "Fix Codex connection").is_empty());
    assert!(maintenance(&case, "Disconnect").is_sensitive());
    provider_details(&case).set_expanded(true);
    assert!(case
        .settings_labels()
        .contains(&"Configuration does not verify that a running chat has loaded them.".into()));
    let mut checking = report(true, true);
    checking["tools"][0]["state"] = json!("checking");
    case.answer("tools.status", checking);
    case.app.refresh_tools();
    case.pump(|case| case.settings_labels().contains(&"Checking".into()));
    assert!(case
        .settings_labels()
        .contains(&"Agent Relay is checking setup. This will finish automatically.".into()));
    for label in [
        "Connect Codex",
        "Fix Codex connection",
        "Check connection",
        "Try again",
    ] {
        assert!(buttons(&content, label).is_empty());
    }
}

pub(super) fn passive_provider_reads_preserve_menus_and_yield_to_actions(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::with(runtime, |core| answers(core, true));
    case.sign_in();
    case.app.go(Page::Settings(Pane::Tools));
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    case.app.open_provider("codex");
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    let content = case.app.shell().unwrap().settings.column.clone();
    let row = content.first_child().unwrap();
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    case.answer(
        "tools.status",
        Answer::with(move |_| {
            let gate = held.clone();
            async move {
                gate.notified().await;
                Ok(report(true, true))
            }
        }),
    );
    let disconnect = maintenance(&case, "Disconnect");
    let before = case.called("tools.status").len();
    case.app.refresh_tools();
    case.pump(|case| case.called("tools.status").len() > before);
    assert!(disconnect.is_sensitive());
    assert!(
        row.parent().is_some(),
        "passive start must retain the open menu's page"
    );
    gate.notify_one();
    case.pump(|case| !case.app.with_state(|state| state.tools.loading));
    assert!(
        row.parent().is_some(),
        "unchanged passive result must retain the open menu's page"
    );
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    case.answer(
        "tools.status",
        Answer::with(move |_| {
            let gate = held.clone();
            async move {
                gate.notified().await;
                Ok(report(true, true))
            }
        }),
    );
    let before = case.called("tools.status").len();
    case.app.refresh_tools();
    case.pump(|case| case.called("tools.status").len() > before);
    disconnect.emit_clicked();
    case.pump(|case| {
        !case.called("tools.manage").is_empty() && !case.app.with_state(|state| state.tools.busy)
    });
    assert_eq!(
        case.called("tools.manage")[0],
        json!({"tool_id":"codex","action":"uninstall"})
    );
    gate.notify_one();
    case.settle();
    assert!(
        case.settings_labels().contains(&"Not connected".into()),
        "late passive result must not restore a disconnected provider"
    );
}

/// A status update that changes nothing the chooser shows, such as a
/// background account check, keeps it mounted; a toggled choice does too.
pub(super) fn chooser_stays_mounted_through_unchanged_updates(runtime: &tokio::runtime::Runtime) {
    let case = Case::with(runtime, |core| answers(core, false));
    show_chooser(&case);
    case.settle();
    let mounted = case.app.window().child().unwrap();
    let codex = case
        .checks()
        .into_iter()
        .find(|check| check.widget_name() == "Codex")
        .unwrap();
    codex.set_active(true);
    let mut checked = workspace_snapshot();
    checked["account"]["auth_checked_at"] = json!(1_800_000_000_000_i64);
    case.set_state(checked);
    case.settle();
    assert_eq!(case.app.window().child().as_ref(), Some(&mounted));
    assert!(codex.is_active());
}

/// All rows share the finished texture and later rows use it synchronously.
pub(super) fn avatar_rows_share_the_pending_download(runtime: &tokio::runtime::Runtime) {
    use agent_relay_app::ui::widgets::avatar;
    let case = Case::new(runtime);
    let url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
    let first = avatar("Ada", Some(url), 24);
    let second = avatar("Ada", Some(url), 32);
    let picture = |holder: &gtk::Box| {
        let mut found = None;
        walk(holder.upcast_ref(), &mut |widget| {
            if let Ok(picture) = widget.clone().downcast::<gtk::Picture>() {
                found = Some(picture);
            }
        });
        found
    };
    assert!(picture(&first).is_none());
    assert!(picture(&second).is_none());
    case.pump(|_| picture(&first).is_some() && picture(&second).is_some());
    let shared = picture(&first).unwrap().paintable();
    assert_eq!(picture(&second).unwrap().paintable(), shared);
    let cached = avatar("Ada", Some(url), 40);
    assert_eq!(picture(&cached).unwrap().paintable(), shared);
}
