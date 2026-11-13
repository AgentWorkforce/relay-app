//! Reuse the public native validator; a shared runtime is not a chat identity.
use super::{control, intent::Peer};
use crate::mcp_settings::{Binding, Installer};
use agent_relay_node::relay::{
    copilot::proof,
    process::{ProcessInfo, ProcessTable, SystemProcesses},
};
use std::path::PathBuf;

pub(crate) trait Verifier: Send + Sync {
    fn runtime(
        &self,
        installer: &Installer,
        binding: &Binding,
        pid: i32,
        uid: u32,
    ) -> Result<ProcessInfo, &'static str>;
    fn held(
        &self,
        installer: &Installer,
        runtime: ProcessInfo,
        sid: &str,
    ) -> Result<Peer, &'static str>;
    fn hook(
        &self,
        installer: &Installer,
        pid: i32,
        uid: u32,
        sid: &str,
    ) -> Result<Peer, &'static str>;
}

pub(super) struct Kernel;
impl Verifier for Kernel {
    fn runtime(
        &self,
        i: &Installer,
        b: &Binding,
        p: i32,
        u: u32,
    ) -> Result<ProcessInfo, &'static str> {
        runtime(i, b, p, u)
    }
    fn held(&self, i: &Installer, r: ProcessInfo, s: &str) -> Result<Peer, &'static str> {
        held(i, r, s)
    }
    fn hook(&self, i: &Installer, p: i32, u: u32, s: &str) -> Result<Peer, &'static str> {
        hook(i, p, u, s)
    }
}

pub(crate) fn arguments(binding: &Binding, development: bool) -> Vec<String> {
    let mut args = vec![
        "copilot-mcp".into(),
        "--site".into(),
        binding.site_url.clone(),
        "--workspace".into(),
        binding.workspace_id.clone(),
        "--account".into(),
        binding.account_id.clone(),
    ];
    if development {
        args.push("--dev".into());
    }
    args
}

pub(super) fn runtime(
    installer: &Installer,
    binding: &Binding,
    pid: i32,
    uid: u32,
) -> Result<ProcessInfo, &'static str> {
    if !proof::SUPPORTED {
        return Err("native_platform_unavailable");
    }
    let table = SystemProcesses::default();
    let caller = table
        .process(pid)
        .filter(|p| p.uid == uid)
        .ok_or("native_peer_unavailable")?;
    let executable = table.executable(pid).ok_or("native_peer_unavailable")?;
    let expected =
        std::fs::canonicalize(installer.copilot_helper()).map_err(|_| "native_peer_unavailable")?;
    let actual_args = table.arguments(pid);
    if executable != expected
        || !table.is_trusted_relay_helper(pid, &executable)
        || actual_args.get(1..)
            != Some(arguments(binding, installer.copilot_development()).as_slice())
    {
        return Err("native_peer_unavailable");
    }
    let runtime = proof::runtime(&table, caller.parent, uid).ok_or("native_peer_unavailable")?;
    if table.process(pid) != Some(caller) {
        return Err("native_peer_changed");
    }
    Ok(runtime)
}

pub(super) fn held(
    installer: &Installer,
    runtime: ProcessInfo,
    sid: &str,
) -> Result<Peer, &'static str> {
    let table = SystemProcesses::default();
    let config = installer.copilot_native_config();
    if !proof::held(&config, &table, runtime.pid, runtime.uid)
        .iter()
        .any(|id| id == sid)
        || table.process(runtime.pid) != Some(runtime)
    {
        return Err("native_chat_unavailable");
    }
    let hold = config
        .sessions()
        .join(sid)
        .join(format!("inuse.{}.hold", runtime.pid));
    let stamp = std::fs::metadata(hold)
        .ok()
        .and_then(|m| control::identity(&m))
        .ok_or("native_chat_unavailable")?;
    if table.process(runtime.pid) != Some(runtime) {
        return Err("native_peer_changed");
    }
    Ok(Peer {
        runtime,
        hold: stamp,
    })
}

pub(super) fn hook(
    installer: &Installer,
    pid: i32,
    uid: u32,
    sid: &str,
) -> Result<Peer, &'static str> {
    // The caller already passed SessionDirectory::copilot_hook, which verifies
    // the managed command, signature and direct ancestry. Snapshot that same
    // incarnation for later redemption; do not infer SID from this parent.
    let table = SystemProcesses::default();
    let hook = table
        .process(pid)
        .filter(|p| p.uid == uid)
        .ok_or("native_peer_unavailable")?;
    let runtime = proof::runtime(&table, hook.parent, uid).ok_or("native_peer_unavailable")?;
    let peer = held(installer, runtime, sid)?;
    if table.process(pid) != Some(hook) {
        return Err("native_peer_changed");
    }
    Ok(peer)
}

pub(super) fn journal(installer: &Installer, sid: &str) -> PathBuf {
    installer
        .copilot_native_config()
        .sessions()
        .join(sid)
        .join("events.jsonl")
}
