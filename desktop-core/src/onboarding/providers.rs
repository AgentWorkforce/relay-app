//! Provider choices and upload settings preserve existing values when omitted.
use super::*;

pub(super) fn provider_ids(report: &Value) -> BTreeSet<String> {
    report["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| tool["selected"] == true)
        .filter_map(|tool| tool["id"].as_str().map(str::to_string))
        .collect()
}
fn eligible(tool: &Value) -> bool {
    tool["supported"] == true || matches!(tool["id"].as_str(), Some("conductor" | "paseo"))
}
fn safe_state(value: &Value) -> &str {
    const STATES: &[&str] = &[
        "ready",
        "configured",
        "installed",
        "external_setup",
        "checking",
        "action_required",
        "signed_out",
        "sign_in_required",
        "offline",
        "not_installed",
        "unavailable",
        "onboarding_needed",
        "uploading",
        "paused",
        "watching",
        "idle",
        "needs_attention",
        "conflict",
        "preparing",
        "working",
        "blocked",
    ];
    let state = value.as_str().unwrap_or("");
    if STATES.contains(&state) {
        state
    } else {
        "unknown"
    }
}
pub(super) fn configure(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
    account: &Value,
) -> Result<i32, Failure> {
    let mut tools = call(core, "tools.status", json!({}))?;
    let uploader = call(core, "uploader.status", json!({}))?;
    let mut actions = Vec::new();
    if let Some(requested) = provider_choice(ui, options, &tools)? {
        select_providers(core, ui, &mut tools, &requested, &mut actions)?;
    }
    if tools["onboarding_complete"] == false {
        tools = call(
            core,
            "tools.select",
            json!({"tool_ids":provider_ids(&tools)}),
        )?;
    }
    configure_sharing(core, ui, options, &uploader)?;
    if uploader["needs_start"] == true && uploader["paused"] != true {
        call(core, "uploader.start", json!({}))?;
    }
    let result = health(core, &tools, account, &uploader, &mut actions)?;
    let exit = if actions.is_empty() { 0 } else { 2 };
    ui.event("result", result);
    Ok(exit)
}
fn provider_choice(
    ui: &mut impl Interface,
    options: &Options,
    tools: &Value,
) -> Result<Option<String>, Failure> {
    if options.providers.is_some() || !ui.interactive() {
        return Ok(options.providers.clone());
    }
    let detected: Vec<_> = tools["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| tool["detected"] == true && eligible(tool))
        .filter_map(|tool| tool["id"].as_str())
        .collect();
    ui.event("providers", json!({"message":format!("Detected providers: {}. Existing choices are kept unless changed.", detected.join(", ")), "detected":detected}));
    let answer = ui
        .prompt("Providers (comma-separated IDs, detected, none; Enter keeps current):")
        .map_err(|_| Failure::choice())?;
    Ok((!answer.is_empty()).then_some(answer))
}
fn requested_ids(requested: &str, all: &[Value]) -> Result<BTreeSet<String>, Failure> {
    let ids = if requested == "detected" {
        all.iter()
            .filter(|tool| tool["detected"] == true)
            .filter_map(|tool| tool["id"].as_str().map(str::to_string))
            .collect()
    } else if requested == "none" {
        BTreeSet::new()
    } else {
        requested
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect()
    };
    if ids
        .iter()
        .any(|id| !all.iter().any(|tool| tool["id"] == *id))
    {
        return Err(Failure { code:"unknown_provider", message:"A requested provider is not in this core's provider catalog. Use a listed provider ID or --providers detected." });
    }
    Ok(ids)
}
fn select_providers(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    tools: &mut Value,
    requested: &str,
    actions: &mut Vec<Value>,
) -> Result<(), Failure> {
    let all = tools["tools"].as_array().cloned().unwrap_or_default();
    let requested = requested.trim().to_ascii_lowercase();
    let ids = requested_ids(&requested, &all)?;
    let local: BTreeSet<_> = all
        .iter()
        .filter(|tool| eligible(tool) && tool["id"].as_str().is_some_and(|id| ids.contains(id)))
        .filter_map(|tool| tool["id"].as_str().map(str::to_string))
        .collect();
    for tool in all
        .iter()
        .filter(|tool| !eligible(tool) && tool["id"].as_str().is_some_and(|id| ids.contains(id)))
    {
        configure_external(core, ui, tool, actions)?;
    }
    // External-only requests must not disconnect existing local providers.
    let replace =
        !local.is_empty() || requested == "none" || requested == "detected" && ids.is_empty();
    if replace && (local != provider_ids(tools) || tools["onboarding_complete"] == false) {
        *tools = call(core, "tools.select", json!({"tool_ids":local}))?;
    }
    Ok(())
}
fn configure_external(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    tool: &Value,
    actions: &mut Vec<Value>,
) -> Result<(), Failure> {
    let id = tool["id"].as_str().unwrap_or("");
    if id == "cursor" && enable_cursor(core)? {
        ui.event(
            "provider_configured",
            json!({"provider":"cursor", "message":"Cursor session messaging is enabled."}),
        );
        return Ok(());
    }
    if tool["state"] == "configured" {
        return Ok(());
    }
    let mut action = json!({"provider":id, "action":"external_setup", "message":"This provider requires its external connection or supported local integration; automatic MCP installation is not available."});
    if id == "capy" {
        let mut url = url::Url::parse("https://capy.ai/new").map_err(|_| Failure::step())?;
        url.query_pairs_mut().append_pair("prompt", "Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/setting-up-capy-on-agent-relay/SKILL.md");
        action["url"] = json!(url.to_string());
        action["message"] =
            json!("Open Capy on another computer and send the prefilled Agent Relay setup prompt.");
    }
    ui.event("action_guidance", action.clone());
    actions.push(action);
    Ok(())
}
fn enable_cursor(core: &mut impl Backend) -> Result<bool, Failure> {
    let settings = call(core, "settings.get", json!({}))?;
    let Some(enabled) = settings["cursor_direct_delivery"].as_bool() else {
        return Ok(false);
    };
    if !enabled {
        call(core, "settings.set", json!({"cursor_direct_delivery":true}))?;
    }
    Ok(true)
}
fn configure_sharing(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
    uploader: &Value,
) -> Result<(), Failure> {
    let current = uploader["sharing_mode"]
        .as_str()
        .and_then(Sharing::parse)
        .unwrap_or(Sharing::Selected);
    // The organization sets the policy: nothing to ask or change.
    let enforced = &uploader["enforced_settings"][crate::history::operator_policy::SHARING_MODE];
    if let Some(mode) = enforced.as_str().and_then(Sharing::parse) {
        ui.event(
            "sharing",
            json!({"message":format!("Upload policy is {}. {}", mode.name(), crate::history::operator_policy::MANAGED)}),
        );
        return Ok(());
    }
    let mut chosen = options.sharing;
    if chosen.is_none() && ui.interactive() {
        ui.event("sharing", json!({"message":format!("Upload policy is {}. selected shares only chosen chats; new uploads future chats; all uploads existing and future chats.", current.name())}));
        let answer = ui
            .prompt("Upload policy (selected, new, all; Enter keeps current):")
            .map_err(|_| Failure::choice())?;
        if !answer.is_empty() {
            chosen = Some(Sharing::parse(&answer).ok_or_else(Failure::choice)?);
        }
    }
    if let Some(sharing) = chosen.filter(|sharing| *sharing != current) {
        call(core, "uploader.sharing.set", json!({"mode":sharing.name()}))?;
    }
    Ok(())
}
fn health(
    core: &mut impl Backend,
    prior_tools: &Value,
    account: &Value,
    prior_uploader: &Value,
    actions: &mut Vec<Value>,
) -> Result<Value, Failure> {
    let workspace = check(
        core,
        "workspace.verify",
        Value::Null,
        "verify_workspace",
        actions,
    );
    let tools = check(
        core,
        "tools.check",
        prior_tools.clone(),
        "check_providers",
        actions,
    );
    let uploader = call(core, "uploader.status", json!({}))?;
    health_actions(&workspace, &tools, actions);
    uploader_actions(&uploader, actions);
    let providers: Vec<_> = tools["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| tool["selected"] == true)
        .map(|tool| json!({"id":tool["id"],"state":safe_state(&tool["state"])}))
        .collect();
    let sharing = uploader["sharing_mode"]
        .as_str()
        .or(prior_uploader["sharing_mode"].as_str())
        .and_then(Sharing::parse)
        .unwrap_or(Sharing::Selected);
    Ok(
        json!({"status":if actions.is_empty() {"complete"} else {"action_required"},
        "message":if actions.is_empty() {"Agent Relay setup is complete."} else {"Local setup finished; the listed actions still need attention."},
        "workspace_id":account["workspace_id"], "sharing":sharing.name(), "providers":providers,
        "health":{"account":safe_state(&tools["account_health"]), "workspace":workspace["verification"].as_str().filter(|s| matches!(*s,"verified"|"pending"|"unreachable"|"rejected")).unwrap_or("unknown"),
            "uploader_running":uploader["running"] == true,"uploader_paused":uploader["paused"] == true,"uploader_stage":safe_state(&uploader["stage"]),
            "claude_cloud":uploader["claude_cloud"].as_object().map(|_| safe_state(&uploader["claude_cloud"]["state"]))},
        "actions":actions}),
    )
}
fn uploader_actions(uploader: &Value, actions: &mut Vec<Value>) {
    if matches!(
        uploader["stage"].as_str(),
        Some("offline" | "conflict" | "needs_attention")
    ) || uploader["stalled"] == true
        || uploader["journal_full"] == true
    {
        actions.push(json!({"action":"check_uploader", "message":"Session uploading needs attention. Check the core's upload status and network before retrying setup."}));
    }
    if matches!(
        uploader["claude_cloud"]["state"].as_str(),
        Some("blocked" | "needs_attention")
    ) {
        actions.push(json!({"action":"check_claude_cloud", "message":"Claude cloud sessions need attention. Check the cloud-session connection in Agent Relay Settings."}));
    }
}
fn health_actions(workspace: &Value, tools: &Value, actions: &mut Vec<Value>) {
    if matches!(
        workspace["verification"].as_str(),
        Some("unreachable" | "rejected")
    ) {
        actions.push(json!({"action":"verify_workspace", "message":"Workspace verification needs attention. Check the account and network, then retry setup."}));
    }
    if matches!(
        tools["account_health"].as_str(),
        Some("sign_in_required" | "signed_out" | "offline" | "unavailable")
    ) {
        actions.push(json!({"action":"check_account", "message":"Account connectivity needs attention. Check the account and network, then retry setup."}));
    }
    for tool in tools["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| tool["selected"] == true)
    {
        if matches!(
            tool["state"].as_str(),
            Some("action_required" | "conflict" | "not_installed" | "unavailable")
        ) {
            actions.push(json!({"action":"check_provider", "provider":tool["id"], "message":"This selected provider needs attention. Repair its connection before using it."}));
        }
    }
}
fn check(
    core: &mut impl Backend,
    method: &'static str,
    fallback: Value,
    action: &str,
    actions: &mut Vec<Value>,
) -> Value {
    match core.call(method, json!({})) {
        Ok(report) => report,
        Err(()) => {
            actions.push(json!({"action":action,"message":"This health check could not finish. Retry setup when the service and network are available."}));
            fallback
        }
    }
}
