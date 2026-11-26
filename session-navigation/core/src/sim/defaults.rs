//! The operating system's default terminal in the fake machine, and the
//! oracle's own rule for which terminal a launch may open. Truth only;
//! nothing here calls `decide` or reuses its helpers.
use serde::{Deserialize, Serialize};

use super::oracle::{fail, Invariant, Violation};
use super::world::World;
use crate::engine::OpenReport;
use crate::evidence::Fact;
use crate::model::{Host, Reason, TerminalKind};
use crate::plan::{OpenRequest, Plan, Step};

/// What the machine says its default terminal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemDefault {
    /// A terminal this crate models (installed or not).
    Set(TerminalKind),
    /// None is set, or one this crate does not model.
    NoneSet,
    /// The setting cannot be read.
    Unreadable,
}

impl World {
    /// The machine's default terminal, drawn per run (a stream of its own,
    /// so no other draw moves) from the terminals of this platform: the
    /// person's usual terminal most often, sometimes another (possibly not
    /// installed), sometimes none, sometimes unreadable.
    pub fn system_default(&self) -> SystemDefault {
        let draw = self.hosts.default_draw;
        let [first, second] = self.default_terminals();
        let choices = self.terminal_choices();
        let len = u64::try_from(choices.len()).unwrap_or(1).max(1);
        let any = usize::try_from((draw / 100) % len)
            .ok()
            .and_then(|i| choices.get(i).copied())
            .unwrap_or(first);
        match draw % 100 {
            0..=39 => SystemDefault::Set(first),
            40..=54 => SystemDefault::Set(second),
            55..=69 => SystemDefault::Set(any),
            70..=84 => SystemDefault::NoneSet,
            _ => SystemDefault::Unreadable,
        }
    }

    /// What an adapter reports for [`World::system_default`].
    pub fn observed_default(&self) -> Fact<Option<TerminalKind>> {
        match self.system_default() {
            SystemDefault::Set(terminal) => Fact::Known(Some(terminal)),
            SystemDefault::NoneSet => Fact::Known(None),
            SystemDefault::Unreadable => Fact::Unknown,
        }
    }
}

fn host_terminal(host: Option<&Host>) -> Option<TerminalKind> {
    match host {
        Some(Host::Terminal(t) | Host::Tmux(Some(t))) => Some(*t),
        _ => None,
    }
}

/// A launch opens the default terminal (the person's preference, else the
/// machine's) or a terminal the session was seen in, and only the default
/// when the person asked for it. "No default terminal" is said only when
/// there truly is none (not when it is unreadable, nor when one is set).
pub fn check_terminal(
    world: &World,
    request: &OpenRequest,
    report: &OpenReport,
) -> Result<(), Violation> {
    let provider = Some(report.session.provider);
    let system = world.system_default();
    let default = request.prefs.default_terminal.or(match system {
        SystemDefault::Set(t) => Some(t),
        _ => None,
    });
    let none_exists = request.prefs.default_terminal.is_none() && system == SystemDefault::NoneSet;
    if report.decision.plan == Plan::Unavailable(Reason::NoDefaultTerminal) && !none_exists {
        return fail(
            Invariant::RightTerminal,
            "no default claimed",
            provider,
            format!("NoDefaultTerminal with preference {request:?} and system {system:?}"),
        );
    }
    let session = &report.session;
    let mut seen = vec![
        host_terminal(world.history.get(session)),
        host_terminal(world.first_host.get(session)),
        host_terminal(report.before.last_seen_host.known()),
        host_terminal(report.before.original_host.known()),
    ];
    seen.retain(Option::is_some);
    // What the decision knew was the last terminal is kept: neither the
    // default nor a terminal preference overrides it unasked.
    let last = host_terminal(report.before.last_seen_host.known());
    for (step, _) in &report.executed {
        let Step::Launch { terminal, .. } = step else {
            continue;
        };
        let is_default = default == Some(*terminal);
        let ok = if request.use_default_terminal {
            is_default
        } else {
            is_default || seen.contains(&Some(*terminal))
        };
        if !request.use_default_terminal && last.is_some_and(|last| last != *terminal) {
            return fail(
                Invariant::RightTerminal,
                "overrode the last-seen terminal",
                provider,
                format!("launched in {terminal:?}; last seen in {last:?}"),
            );
        }
        if !ok {
            return fail(
                Invariant::RightTerminal,
                if request.use_default_terminal {
                    "not the default terminal"
                } else {
                    "unchosen terminal"
                },
                provider,
                format!(
                    "launched in {terminal:?}; default {default:?} (system {system:?}), seen in {seen:?}"
                ),
            );
        }
    }
    Ok(())
}
