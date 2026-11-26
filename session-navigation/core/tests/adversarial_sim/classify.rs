use super::*;

impl Probe {
    // ---- classification ----------------------------------------------------

    pub(super) fn classify(
        &mut self,
        session: &SessionRef,
        report: &OpenReport,
        pending_before: bool,
        second_after_start: bool,
        truth: &TruthBefore,
    ) {
        let host = self.host_of(session);
        let mut tags: Vec<String> = Vec::new();
        let action = report.decision.action();
        match &report.decision.plan {
            Plan::Act(a) => tags.push(format!("plan:{:?}", a.kind)),
            Plan::Choose(_) => tags.push("plan:Choose".into()),
            Plan::Confirm(_) => tags.push("plan:Confirm".into()),
            Plan::Unavailable(r) => tags.push(format!("refused:{r:?}")),
        }
        let outcome = match &report.outcome {
            Outcome::Verified => "Verified".to_owned(),
            Outcome::DispatchedUnverified => "DispatchedUnverified".to_owned(),
            Outcome::Failed(r) => format!("Failed({r:?})"),
            Outcome::Blocked(r) => format!("Blocked({r:?})"),
            Outcome::NeedsChoice => "NeedsChoice".to_owned(),
        };
        tags.push(format!("outcome:{outcome}"));
        if matches!(
            report.outcome,
            Outcome::Verified | Outcome::DispatchedUnverified
        ) && !report.executed.is_empty()
        {
            tags.push("useful_outcome".into());
        }
        if report.decision.state == SessionState::LiveLocatable
            && action
                .is_some_and(|a| matches!(a.kind, ActionKind::FocusPane | ActionKind::OpenExisting))
        {
            tags.push("live_locatable".into());
        }
        let launched = report
            .executed
            .iter()
            .any(|(s, _)| matches!(s, Step::Launch { .. }));
        if launched {
            tags.push("stopped_resume_dispatched".into());
            if report.outcome == Outcome::Verified {
                tags.push("stopped_resume_verified".into());
            }
        }
        if action.is_some_and(|a| a.kind == ActionKind::Reopen) && !report.executed.is_empty() {
            tags.push("archived_revive_dispatched".into());
        }
        if action.is_some_and(|a| a.kind == ActionKind::OpenExisting)
            && report.decision.state == SessionState::Stopped
            && !report.executed.is_empty()
        {
            tags.push("stopped_open_in_app".into());
        }
        if action.is_some_and(|a| a.kind == ActionKind::BringForward) && !report.executed.is_empty()
        {
            tags.push("bring_forward_dispatched".into());
        }
        if report
            .executed
            .iter()
            .any(|(s, _)| matches!(s, Step::Stop { .. }))
        {
            tags.push("take_over_stop_dispatched".into());
        }
        if report.outcome == Outcome::Blocked(Reason::PermissionDenied) {
            tags.push("permission_denied".into());
        }
        let unverified = report.decision.plan == Plan::Unavailable(Reason::UnverifiedVersion)
            || action.is_some_and(|a| a.note == Some(Reason::UnverifiedVersion));
        if unverified {
            tags.push("unverified_version".into());
        }
        if pending_before {
            tags.push("open_while_own_launch_pending".into());
        }
        if second_after_start {
            tags.push("double_click_after_start".into());
        }
        if report.decision.plan == Plan::Unavailable(Reason::ToolNotInstalled) {
            tags.push("tool_not_installed".into());
        }
        // Claude registry situations, read from the decision snapshot.
        if let (ProviderEvidence::Claude(ev), Fact::Known(table)) =
            (&report.before.provider, &report.before.processes)
        {
            if let Fact::Known(claims) = &ev.registry {
                for claim in claims.iter().filter(|c| c.session_id == session.id) {
                    if claim.spare {
                        tags.push("spare_registry_names_session".into());
                        continue;
                    }
                    let proc = table.iter().find(|p| p.identity.pid == claim.pid);
                    match proc {
                        Some(p)
                            if p.identity.start == claim.proc_start
                                && p.exe == Exe::ClaudeCli
                                && claim.written_this_boot == Fact::Known(false) =>
                        {
                            tags.push("pid_reuse_across_reboot".into())
                        }
                        Some(p) if p.identity.start == claim.proc_start => {}
                        _ => tags.push("stale_registry_dead_pid".into()),
                    }
                }
            }
        }
        if truth.hidden_claude_host {
            tags.push("hidden_claude_host".into());
        }
        if report.decision.plan == Plan::Unavailable(Reason::PossiblyOpenElsewhere) {
            let needless = truth.writers == 0;
            let rw = u8::from(self.vars.claude_registry_rewritten_on_clear);
            tags.push(format!(
                "possibly_open_elsewhere|rewritten={rw}|{}",
                if needless { "needless" } else { "protective" }
            ));
        }
        // World-variable triggers: situations in which the variable's value
        // can change what happens.
        let executed_url = |prefix: &str| {
            report
                .executed
                .iter()
                .any(|(s, _)| matches!(s, Step::OpenUrl { url, .. } if url.starts_with(prefix)))
        };
        let v = self.vars;
        let mut triggers: Vec<(&str, bool)> = Vec::new();
        if executed_url("claude://resume") && truth.claude_archived {
            triggers.push((
                "claude_resume_link_unarchives",
                v.claude_resume_link_unarchives,
            ));
        }
        if executed_url("claude://resume") && truth.claude_prior_only {
            triggers.push((
                "claude_resume_prior_imports_duplicate",
                v.claude_resume_prior_imports_duplicate,
            ));
        }
        if session.provider == Provider::Claude && truth.spare_and_last_stopped {
            triggers.push((
                "claude_spare_registry_before_claim",
                v.claude_spare_registry_before_claim,
            ));
        }
        if truth.cleared_process_involved {
            triggers.push((
                "claude_registry_rewritten_on_clear",
                v.claude_registry_rewritten_on_clear,
            ));
        }
        if executed_url("claude://code/continue") && truth.writers == 0 {
            triggers.push((
                "claude_continue_starts_process",
                v.claude_continue_starts_process,
            ));
        }
        if truth.codex_interactive_host {
            triggers.push(("codex_tui_holds_lock", v.codex_tui_holds_lock));
            // Added with the variable: a live Codex TUI is where a thread
            // switch after launch would matter.
            triggers.push(("codex_tui_switches_thread", v.codex_tui_switches_thread));
        }
        if executed_url("codex://") && truth.codex_archived {
            triggers.push((
                "codex_link_archived_shows_thread",
                v.codex_link_archived_shows_thread,
            ));
        }
        if session.provider == Provider::Codex && truth.codex_desktop_running {
            triggers.push((
                "codex_ipc_owner_discovery_works",
                v.codex_ipc_owner_discovery_works,
            ));
        }
        if action.is_some_and(|a| matches!(a.post, PostCondition::ConversationShown { .. }))
            && !report.executed.is_empty()
        {
            triggers.push(("desktop_view_readable", v.desktop_view_readable));
        }
        for t in tags {
            self.bump(format!("{host}|{t}"));
        }
        for (name, value) in triggers {
            self.var_trigger(name, value);
        }
    }
}

/// Truth about the target captured right before an open.
pub struct TruthBefore {
    pub writers: usize,
    pub hidden_claude_host: bool,
    pub claude_archived: bool,
    pub claude_prior_only: bool,
    pub spare_and_last_stopped: bool,
    pub cleared_process_involved: bool,
    pub codex_interactive_host: bool,
    pub codex_archived: bool,
    pub codex_desktop_running: bool,
}

impl TruthBefore {
    pub fn capture(w: &World, s: &SessionRef) -> Self {
        let claude = s.provider == Provider::Claude;
        let hidden_claude_host = claude
            && w.procs.values().any(|p| {
                p.is_writer()
                    && p.hosts.as_ref() == Some(s)
                    && !String::from_utf8_lossy(
                        &w.claude
                            .registry
                            .get(&p.pid)
                            .map(|f| f.bytes.clone())
                            .unwrap_or_default(),
                    )
                    .contains(&s.id)
            });
        let claude_archived = claude
            && w.claude
                .desktop
                .values()
                .any(|d| d.archived && d.holds_live(&s.id));
        let claude_prior_only = claude
            && !w.claude.desktop.values().any(|d| d.holds_live(&s.id))
            && w.claude.desktop.values().any(|d| d.prior.contains(&s.id));
        let spare_and_last_stopped = w.procs.values().any(|p| p.role == Role::Spare)
            && w.claude.last_stopped.as_deref() == Some(s.id.as_str());
        let cleared_process_involved = claude
            && w.procs.values().any(|p| {
                w.past_hosts
                    .get(&(p.pid, p.start))
                    .is_some_and(|past| past.contains(s) || p.hosts.as_ref() == Some(s))
            });
        let codex_interactive_host = s.provider == Provider::Codex
            && w.procs
                .values()
                .any(|p| p.role == Role::Interactive && p.hosts.as_ref() == Some(s));
        let codex_archived =
            s.provider == Provider::Codex && w.codex.threads.get(&s.id).is_some_and(|t| t.archived);
        Self {
            writers: w.writers(s),
            hidden_claude_host,
            claude_archived,
            claude_prior_only,
            spare_and_last_stopped,
            cleared_process_involved,
            codex_interactive_host,
            codex_archived,
            codex_desktop_running: w.running(App::CodexDesktop),
        }
    }
}
