//! The macOS fast path (a system with kernel reads) through the same
//! adapter code: it must say exactly what the tool path says, run no `ps`,
//! `sysctl` or `lsof`, reuse only the slow-changing registry within its
//! window, and fall back to the tools when the kernel cannot answer.
#![cfg(feature = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use session_navigation::evidence::{Fact, Snapshot};
use session_navigation::model::App;
use session_navigation::ports::Observe;
use session_navigation_native::macos::{machine, registry};
use session_navigation_native::system::{
    FakeSystem, KernelCall, Output, RawProcess, RunError, ScriptedKernel,
};
use session_navigation_native::MacObserver;

const GHOSTTY_EXE: &str = "/Applications/Ghostty.app/Contents/MacOS/ghostty";
const ASN: &str = "ASN:0x0-0x4af4af:";

fn machine_with_codex() -> Machine {
    let mut m = Machine::new();
    m.procs.push(Proc::new(
        1447,
        998,
        "ttys000",
        "codex",
        "codex",
        &format!("codex resume {CODEX_ID}"),
    ));
    m.apps_json = apps_json(Some(App::Ghostty), &[App::Ghostty]);
    m
}

/// The same processes as the kernel would list them.
fn raw(m: &Machine) -> Vec<RawProcess> {
    m.procs
        .iter()
        .map(|p| RawProcess {
            pid: p.pid,
            ppid: p.ppid,
            euid: p.uid,
            ruid: p.ruid,
            start: p.start,
            tty: (p.tty != "??").then(|| format!("/dev/{}", p.tty)),
            comm: p.ucomm.clone(),
            path: Some(p.comm.trim_start_matches('-').to_owned()).filter(|c| c.starts_with('/')),
            argv: Some(p.args.split_whitespace().map(str::to_owned).collect()),
        })
        .collect()
}

/// Every installed app's bundle executable, as the JXA query reports it.
fn with_executables(json: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
    for (_, entry) in value["apps"].as_object_mut().unwrap() {
        if let Some(app) = entry.as_object_mut() {
            let path = app["path"].as_str().unwrap().to_owned();
            let name = path.rsplit('/').next().unwrap().trim_end_matches(".app");
            let exe = if name == "Ghostty" {
                GHOSTTY_EXE.to_owned()
            } else {
                format!("{path}/Contents/MacOS/{name}")
            };
            app.insert("executable".into(), exe.into());
        }
    }
    value.to_string()
}

fn front_answers(fake: &FakeSystem, bundle: &str) {
    let cfg = cfg();
    fake.on(
        &registry::front_run(&cfg),
        vec![Ok(Output::ok(format!("{ASN}\n")))],
    );
    fake.set_run(
        registry::front_info_run(&cfg, ASN).program.as_str(),
        registry::front_info_run(&cfg, ASN).args,
        Ok(Output::ok(format!(
            "[ NULL ]  {ASN} (in front) \n    bundleID=\"{bundle}\"\n"
        ))),
    );
}

/// A fake with kernel reads for `m`, running reads at once.
fn fast(m: &Machine, kernel: ScriptedKernel) -> (FakeSystem, Arc<ScriptedKernel>) {
    let mut json_machine = Machine::new();
    json_machine.procs = m.procs.clone();
    json_machine.apps_json = with_executables(&m.apps_json);
    json_machine.files = m.files.clone();
    let mut fake = json_machine.build();
    let value: serde_json::Value = serde_json::from_str(&json_machine.apps_json).unwrap();
    for entry in value["apps"].as_object().unwrap().values() {
        if let Some(exe) = entry["executable"].as_str() {
            fake.add_executable(exe, NOW - 1000);
        }
    }
    front_answers(&fake, App::Ghostty.bundle_id());
    let kernel = Arc::new(kernel);
    fake.set_kernel(kernel.clone());
    fake.set_concurrent(true);
    (fake, kernel)
}

fn scripted(m: &Machine) -> ScriptedKernel {
    let mut kernel = ScriptedKernel::new(Some(BOOT), Some(raw(m)));
    for pid in m.agent_pids() {
        kernel = kernel.with_cwd(pid, &format!("{HOME}/code/project"));
    }
    kernel
}

fn observe(fake: &FakeSystem) -> Snapshot {
    MacObserver::new(fake, cfg()).observe(&codex())
}

fn ran(fake: &FakeSystem, program: &str) -> usize {
    fake.runs().iter().filter(|(p, _)| p == program).count()
}

#[test]
fn the_fast_path_says_what_the_tool_path_says() {
    let m = machine_with_codex();
    let slow = observe(&m.build());
    let (fake, kernel) = fast(&m, scripted(&m));
    let quick = observe(&fake);
    assert_eq!(quick, slow);
    assert!(matches!(quick.processes, Fact::Known(ref t) if t.len() == m.procs.len()));
    // No ps, sysctl or lsof; the kernel was asked instead.
    for tool in [machine::PS, machine::SYSCTL, machine::LSOF] {
        assert_eq!(ran(&fake, tool), 0, "{tool}");
    }
    let calls = kernel.calls();
    assert!(calls.contains(&KernelCall::BootTime) && calls.contains(&KernelCall::Processes));
    assert!(calls.contains(&KernelCall::Cwd(1447)));
}

#[test]
fn a_kernel_that_cannot_read_falls_back_to_the_tools() {
    let m = machine_with_codex();
    let slow = observe(&m.build());
    for kernel in [
        ScriptedKernel::new(None, None),
        ScriptedKernel::new(Some(BOOT), None),
    ] {
        let (fake, _) = fast(&m, kernel);
        assert_eq!(observe(&fake), slow);
        assert_eq!(ran(&fake, machine::PS), 3);
    }
}

#[test]
fn the_registry_is_reused_inside_its_window_and_the_front_app_is_not() {
    let m = machine_with_codex();
    let (fake, _) = fast(&m, scripted(&m));
    let mut observer = MacObserver::new(&fake, cfg());
    let first = observer.observe(&codex());
    assert_eq!(first.front.app, Fact::Known(Some(App::Ghostty)));
    front_answers(&fake, App::AppleTerminal.bundle_id());
    let second = observer.observe(&codex());
    assert_eq!(second.front.app, Fact::Known(Some(App::AppleTerminal)));
    assert_eq!(
        (first.apps.clone(), first.schemes.clone()),
        (second.apps, second.schemes)
    );
    let jxa = machine::apps_run(&cfg());
    let count = |fake: &FakeSystem| {
        fake.runs()
            .iter()
            .filter(|r| **r == (jxa.program.clone(), jxa.args.clone()))
            .count()
    };
    assert_eq!(count(&fake), 1, "one registry query for two looks");
    assert_eq!(ran(&fake, registry::LSAPPINFO), 4, "front read every look");

    // Without reuse every look asks again.
    let (fake, _) = fast(&m, scripted(&m));
    let mut observer = MacObserver::new(&fake, cfg());
    observer.set_reuse(Duration::ZERO);
    let _ = observer.observe(&codex());
    let _ = observer.observe(&codex());
    assert_eq!(count(&fake), 2);
}

#[test]
fn running_apps_come_from_the_kernel_table() {
    let mut m = machine_with_codex();
    // A stale registry answer: nothing runs.
    m.apps_json = apps_json(Some(App::Ghostty), &[]);
    let (fake, _) = fast(&m, scripted(&m));
    let snap = observe(&fake);
    let running = |snap: &Snapshot, app: App| {
        snap.apps
            .known()
            .and_then(|a| a.iter().find(|i| i.app == app))
            .map(|i| i.running)
    };
    assert_eq!(running(&snap, App::Ghostty), Some(true));
    assert_eq!(running(&snap, App::AppleTerminal), Some(false));
    // Another person's Ghostty is not this person's running app.
    let mut other = machine_with_codex();
    if let Some(g) = other.procs.iter_mut().find(|p| p.pid == 811) {
        g.ruid = 502;
        g.uid = 502;
    }
    let (fake, _) = fast(&other, scripted(&other));
    assert_eq!(running(&observe(&fake), App::Ghostty), Some(false));
}

#[test]
fn a_failed_front_read_uses_a_fresh_registry_query() {
    let m = machine_with_codex();
    let (fake, _) = fast(&m, scripted(&m));
    fake.on(&registry::front_run(&cfg()), vec![Err(RunError::TimedOut)]);
    let mut observer = MacObserver::new(&fake, cfg());
    let a = observer.observe(&codex());
    let b = observer.observe(&codex());
    assert_eq!(a.front.app, Fact::Known(Some(App::Ghostty)));
    assert_eq!(a, b);
    let jxa = machine::apps_run(&cfg());
    let queries = fake
        .runs()
        .iter()
        .filter(|r| **r == (jxa.program.clone(), jxa.args.clone()))
        .count();
    assert_eq!(queries, 2, "no reuse when the front app cannot be read");
}

#[test]
fn the_executor_reads_the_kernel_table_too() {
    let m = machine_with_codex();
    let (fake, _) = fast(&m, scripted(&m));
    let boot = machine::boot(&fake, &cfg()).unwrap();
    let table = machine::table(&fake, &cfg(), session_navigation::model::BootId(boot)).unwrap();
    let slow = m.build();
    let slow_table =
        machine::table(&slow, &cfg(), session_navigation::model::BootId(boot)).unwrap();
    assert_eq!(table, slow_table);
    assert!(fake
        .runs()
        .iter()
        .all(|(p, _)| p != machine::PS && p != machine::SYSCTL));
}
