//! Oracle checks for terminal hosts: what the person sees at the front
//! (through tmux clients and window tabs), launches in the session's own
//! runtime, and tmux command lines that only select and attach. Truth only;
//! nothing here calls `decide`.
use super::oracle::{fail, Invariant, Violation};
use super::tmux::client_of;
use super::truth::command_target;
use super::world::{Proc, World};
use crate::engine::OpenReport;
use crate::model::Runtime;
use crate::plan::Step;

/// The agent the person sees: the front pane's foreground agent, or, when a
/// tmux client runs there, the agent of that session's active pane.
pub fn front_agent(world: &World) -> Option<&Proc> {
    let pane = world
        .front_pane
        .and_then(|id| world.panes.get(&id))
        .filter(|pane| pane.tmux.is_none() && Some(pane.terminal.app()) == world.front_app)?;
    if let Some(agent) = world.pane_agent(pane) {
        return Some(agent);
    }
    let client = world
        .procs
        .values()
        .find(|p| p.tty.as_deref() == Some(pane.tty.as_str()) && client_of(p).is_some())?;
    let (socket, session) = client_of(client)?;
    let active = world.tmux.servers.get(socket)?.sessions.get(&session)?;
    world.panes.get(active).and_then(|p| world.pane_agent(p))
}

/// A launch runs in the runtime the session truly lives in.
pub fn check_runtime(world: &World, report: &OpenReport) -> Result<(), Violation> {
    for (step, _) in &report.executed {
        let Step::Launch {
            command, context, ..
        } = step
        else {
            continue;
        };
        let Some(session) = command_target(command) else {
            continue;
        };
        let wsl = world.wsl_sessions.contains(&session);
        let ok = match &context.runtime {
            Runtime::Native => !wsl,
            Runtime::Wsl { .. } => wsl,
            Runtime::Foreign { .. } => false,
        };
        if !ok {
            return fail(
                Invariant::SameRuntime,
                "launch in another runtime",
                Some(session.provider),
                format!(
                    "{command:?} runs in {:?}; the session lives in WSL: {wsl}",
                    context.runtime
                ),
            );
        }
    }
    Ok(())
}

/// A tmux command line that only selects and attaches: `-S <socket>`, then
/// `select-window`, `select-pane` and `attach-session` with `-t <target>`,
/// separated by `;`. Nothing that types into a pane (`send-keys`).
pub fn tmux_attach_only(args: &[String]) -> bool {
    let mut words = args.iter().map(String::as_str);
    if words.next() != Some("-S") || words.next().is_none() {
        return false;
    }
    let mut attached = false;
    while let Some(word) = words.next() {
        match word {
            ";" => {}
            "select-window" | "select-pane" | "attach-session" => {
                attached |= word == "attach-session";
                if words.next() != Some("-t") || words.next().is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    attached
}
