//! Device approval stays in the daemon, including when this client exits.
use super::*;

pub(super) enum Connection {
    Ready(Value),
    Pending(i32),
}
enum Progress {
    Waiting,
    Restarted,
    Finished(i32),
}
struct Wait {
    enabled: bool,
    announced: bool,
    acknowledged: bool,
}

pub(super) fn connect(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
) -> Result<Connection, Failure> {
    let Some(mut account) = initial_account(core, ui, options)? else {
        return Ok(Connection::Pending(2));
    };
    if !auth_ready(&account)
        && !matches!(
            account["state"].as_str(),
            Some("pending_approval" | "preparing")
        )
    {
        account = sign_in(core, options, options.acknowledge_uninspected_schedules)?;
    }
    let mut wait = Wait {
        enabled: options.wait || ui.interactive(),
        announced: false,
        acknowledged: options.acknowledge_uninspected_schedules,
    };
    while !auth_ready(&account) {
        match advance(core, ui, options, &mut account, &mut wait)? {
            Progress::Finished(exit) => return Ok(Connection::Pending(exit)),
            Progress::Restarted => continue,
            Progress::Waiting => {}
        }
        let remaining = Duration::from_secs(options.timeout).saturating_sub(core.elapsed());
        let interval = Duration::from_secs(account["interval"].as_u64().unwrap_or(1).clamp(1, 60));
        core.pause(interval.min(remaining));
        account = call(core, "account.status", json!({}))?;
    }
    if options
        .workspace
        .as_deref()
        .is_some_and(|wanted| account["workspace_id"].as_str() != Some(wanted))
    {
        return Ok(Connection::Pending(pending(
            ui,
            "different_workspace",
            json!({"message":"This installation is signed in to another workspace. Sign out explicitly before rerunning setup --workspace UUID; existing uploads were left unchanged."}),
        )));
    }
    ui.event(
        "authenticated",
        json!({"message":"Agent Relay account connected.", "workspace_id":account["workspace_id"]}),
    );
    Ok(Connection::Ready(account))
}
fn initial_account(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
) -> Result<Option<Value>, Failure> {
    if let Ok(account) = core.call("account.status", json!({})) {
        return Ok(Some(account));
    }
    if !options.start_service || !core.start_service() {
        pending(
            ui,
            "core_not_running",
            json!({"message":"The Agent Relay core is not running. Install the headless core and run systemctl --user start agent-relay.service, or rerun setup --start-service for an existing Linux service."}),
        );
        return Ok(None);
    }
    for _ in 0..10 {
        if let Ok(account) = core.call("account.status", json!({})) {
            return Ok(Some(account));
        }
        core.pause(Duration::from_secs(1));
    }
    Err(Failure {
        code: "service_unavailable",
        message:
            "The user service did not become ready. Check journalctl --user -u agent-relay.service.",
    })
}
fn advance(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
    account: &mut Value,
    wait: &mut Wait,
) -> Result<Progress, Failure> {
    if account["action"] == "acknowledge_uninspected_schedules" {
        return acknowledge(core, ui, options, account, wait);
    }
    if account["action"] == "create_workspace" {
        return Ok(Progress::Finished(pending(
            ui,
            "create_workspace",
            json!({"message":"Create a workspace in your Agent Relay account, then rerun setup."}),
        )));
    }
    if let Some(failure) = terminal_auth_failure(account) {
        return Err(failure);
    }
    if account["phase"] == "selecting_workspace" {
        return select_workspace(core, ui, options, account);
    }
    if account["phase"] == "waiting_for_browser" {
        return Ok(Progress::Finished(pending(
            ui,
            "browser_sign_in_pending",
            json!({"message":"A browser sign-in is already pending. Cancel that attempt explicitly before starting device setup; this wizard will not replace an active login."}),
        )));
    }
    if account["state"] == "pending_approval" && !wait.announced {
        let instructions = approval(account);
        ui.event("approval_required", instructions.clone());
        wait.announced = true;
        if !wait.enabled {
            return Ok(Progress::Finished(pending(
                ui,
                "device_approval_required",
                instructions,
            )));
        }
    }
    if !wait.enabled {
        return Ok(Progress::Finished(pending(
            ui,
            "setup_in_progress",
            json!({"message":"Account setup is running in the core. Rerun setup --wait to wait for it."}),
        )));
    }
    if core.elapsed() >= Duration::from_secs(options.timeout) {
        return Ok(Progress::Finished(pending(
            ui,
            "setup_timeout",
            json!({"message":"The setup wait ended. The core keeps its current attempt; rerun setup --wait to resume."}),
        )));
    }
    Ok(Progress::Waiting)
}
fn acknowledge(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
    account: &mut Value,
    wait: &mut Wait,
) -> Result<Progress, Failure> {
    if wait.acknowledged {
        return Err(Failure { code:"schedule_acknowledgement_failed", message:"Setup still cannot confirm older upload schedules after acknowledgement. Check the existing installation." });
    }
    let consent = if ui.interactive() {
        let answer = ui.prompt("Older upload schedules could not be inspected. Continuing may leave those schedules active. Continue anyway? [y/N]").map_err(|_| Failure::choice())?;
        matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
    } else {
        false
    };
    if !consent {
        return Ok(Progress::Finished(pending(
            ui,
            "schedule_acknowledgement_required",
            json!({"message":"Review the older upload schedules, then use --acknowledge-uninspected-schedules if you choose to continue."}),
        )));
    }
    wait.acknowledged = true;
    *account = sign_in(core, options, true)?;
    Ok(Progress::Restarted)
}
fn select_workspace(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
    account: &Value,
) -> Result<Progress, Failure> {
    let choices: Vec<Value> = account["workspace_choices"].as_array().into_iter().flatten().filter_map(|choice| {
        Some(json!({"id":choice["id"].as_str()?, "name":choice["name"].as_str().unwrap_or("Workspace")}))
    }).collect();
    let chosen = if let Some(id) = &options.workspace {
        Some(id.clone())
    } else if ui.interactive() {
        prompt_workspace(ui, &choices)?
    } else {
        None
    };
    let Some(workspace) = chosen else {
        return Ok(Progress::Finished(pending(
            ui,
            "choose_workspace",
            json!({"message":"Choose a workspace with --workspace UUID and rerun setup.", "workspaces":choices}),
        )));
    };
    call(
        core,
        "account.select_workspace",
        json!({"workspace_id":workspace}),
    )?;
    // Read fresh status before acting on another picker response.
    Ok(Progress::Waiting)
}
fn prompt_workspace(ui: &mut impl Interface, choices: &[Value]) -> Result<Option<String>, Failure> {
    for (index, choice) in choices.iter().enumerate() {
        let name: String = choice["name"]
            .as_str()
            .unwrap_or("Workspace")
            .chars()
            .filter(|ch| !ch.is_control())
            .take(80)
            .collect();
        ui.event("workspace", json!({"message":format!("{}. {} ({})", index+1, name, choice["id"].as_str().unwrap_or(""))}));
    }
    let answer = ui
        .prompt("Workspace number:")
        .map_err(|_| Failure::choice())?;
    Ok(answer
        .parse::<usize>()
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| choices.get(i))
        .and_then(|choice| choice["id"].as_str())
        .map(str::to_string))
}
