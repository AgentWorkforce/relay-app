//! Opt-in, read-only: the kernel (FFI) process table agrees with the `ps`
//! table on this machine. Skipped unless `SESSION_NAVIGATION_NATIVE=1`:
//!
//! ```text
//! SESSION_NAVIGATION_NATIVE=1 cargo test -p session-navigation-native --test kernel_agreement -- --nocapture
//! ```
//!
//! Nothing is started, focused or stopped; only counts are printed, never
//! another process's arguments.
#![cfg(all(feature = "macos", target_os = "macos"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use session_navigation::evidence::{Fact, ProcessInfo};
use session_navigation::model::BootId;
use session_navigation_native::macos::{kernel, machine, parse, registry};
use session_navigation_native::system::{Kernel, Run};
use session_navigation_native::{MacConfig, RealSystem, System};

fn native() -> bool {
    let on = std::env::var("SESSION_NAVIGATION_NATIVE").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set SESSION_NAVIGATION_NATIVE=1 to run against this machine");
    }
    on
}

fn real_kernel() -> &'static dyn Kernel {
    RealSystem
        .kernel()
        .expect("RealSystem has kernel reads on macOS")
}

fn by_key(table: &[ProcessInfo]) -> BTreeMap<(u32, u64), &ProcessInfo> {
    table
        .iter()
        .map(|p| ((p.identity.pid, p.identity.start), p))
        .collect()
}

fn printable(text: &str) -> bool {
    text.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// `ps` joins argv with spaces; compare what both say word by word.
fn words(argv: &[String]) -> Vec<String> {
    argv.join(" ")
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[test]
fn kernel_and_ps_tables_agree() {
    if !native() {
        return;
    }
    let cfg = MacConfig::from_env().expect("HOME");
    let run = Run::new(machine::SYSCTL, &["-n", "kern.boottime"]);
    let sysctl = parse::boot_time(&RealSystem.run(&run).unwrap().stdout).unwrap();
    assert_eq!(real_kernel().boot_time(), Some(sysctl), "kern.boottime");
    let boot = BootId(sysctl);

    let raw = real_kernel().processes().expect("kernel table");
    let closed: std::collections::BTreeSet<u32> = raw
        .iter()
        .filter(|r| r.argv.is_none())
        .map(|r| r.pid)
        .collect();
    let before = kernel::KernelTable {
        processes: raw.iter().map(|r| kernel::process(r, boot)).collect(),
        executables: Default::default(),
        from_kernel: true,
    };
    let ps = machine::ps_table(&RealSystem, &cfg, boot).expect("ps table");
    let after = kernel::table(real_kernel(), boot).expect("kernel table");
    let (k, k2, p) = (
        by_key(&before.processes),
        by_key(&after.processes),
        by_key(&ps),
    );

    let mut compared = 0;
    let mut argv_unreadable = 0;
    let mut escaped = 0;
    for (key, kp) in &k {
        let Some(pp) = p.get(key) else {
            // Exited between the reads (then the second kernel read lacks
            // it too) — anything else is a row `ps` cannot see.
            assert!(!k2.contains_key(key), "pid {} only in the kernel", key.0);
            continue;
        };
        compared += 1;
        let pid = key.0;
        assert_eq!(kp.ppid, pp.ppid, "ppid of {pid}");
        assert_eq!((kp.euid, kp.ruid), (pp.euid, pp.ruid), "uids of {pid}");
        assert_eq!(kp.tty, pp.tty, "tty of {pid}");
        assert_eq!(kp.exe, pp.exe, "exe class of {pid}");
        if closed.contains(&pid) {
            // Another user's argument area: only `ps` (setuid root) reads
            // it. The kernel row keeps the executable path as argv[0]
            // (`ps` shows the path or the name it was started by); the
            // class above still agrees.
            argv_unreadable += 1;
        } else if kp.argv.iter().all(|a| printable(a)) {
            // Never print the vectors: they are other programs' arguments.
            assert!(words(&kp.argv) == words(&pp.argv), "argv words of {pid}");
            assert!(kp.argv.first() == pp.argv.first(), "argv[0] of {pid}");
        } else {
            // `ps` escapes control and non-ASCII bytes (`\012`); the
            // kernel's vector is the raw one.
            escaped += 1;
        }
    }
    for key in p.keys() {
        if !k.contains_key(key) {
            // Started between the reads: the later kernel read has it, or
            // it was gone again by then.
            let started_late = key.1 + 2 >= RealSystem.now().saturating_sub(10);
            assert!(
                k2.contains_key(key) || started_late,
                "pid {} only in ps",
                key.0
            );
        }
    }
    eprintln!(
        "kernel {} rows, ps {} rows, {compared} compared, {argv_unreadable} with unreadable argv, {escaped} with escaped argv",
        k.len(),
        p.len()
    );
    assert!(compared * 10 >= p.len() * 9, "most rows compared");
    // Every closed argument area belongs to another uid (or a setuid
    // program such as `login`); the person's own processes are all read.
    let me = cfg.uid;
    assert!(raw
        .iter()
        .filter(|r| r.argv.is_none())
        .all(|r| r.ruid != me || r.euid != me || r.path.is_none()));

    // Agent CLI cwds: kernel vs lsof.
    let mut by_kernel = before.processes.clone();
    kernel::fill_cwds(real_kernel(), &mut by_kernel);
    let mut by_lsof = before.processes.clone();
    machine::lsof_cwds_into(&RealSystem, &cfg, &mut by_lsof);
    for (a, b) in by_kernel.iter().zip(&by_lsof) {
        if let (Fact::Known(_), Fact::Known(_)) = (&a.cwd, &b.cwd) {
            assert!(a.cwd == b.cwd, "cwd of {}", a.identity.pid);
        }
    }
}

#[test]
fn the_kernel_row_of_this_test_is_right() {
    if !native() {
        return;
    }
    let boot = BootId(real_kernel().boot_time().unwrap());
    let raw = real_kernel().processes().unwrap();
    let me = std::process::id();
    let row = raw.iter().find(|r| r.pid == me).expect("own row");
    assert_eq!(row.ppid, std::os::unix::process::parent_id());
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert_eq!(row.path.as_deref(), exe.to_str());
    let argv: Vec<String> = std::env::args().collect();
    assert_eq!(row.argv.as_ref(), Some(&argv));
    let now = RealSystem.now();
    assert!(
        row.start <= now && row.start + 600 >= now,
        "start {}",
        row.start
    );
    assert!(row.start >= boot.0);
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(real_kernel().cwd(me).as_deref(), cwd.to_str());
    // The parent (cargo) reads too, and a pid that does not exist is None.
    assert!(raw.iter().any(|r| r.pid == row.ppid));
    assert_eq!(real_kernel().cwd(u32::MAX - 1), None);
}

#[test]
fn running_and_front_apps_agree_with_the_registry_query() {
    if !native() {
        return;
    }
    let cfg = MacConfig::from_env().expect("HOME");
    let boot = BootId(real_kernel().boot_time().unwrap());
    // The person may switch apps meanwhile: a few tries.
    let agreed = (0..3).any(|_| {
        let report = machine::apps(&RealSystem, &cfg).expect("registry query");
        let table = kernel::table(real_kernel(), boot).expect("kernel table");
        let look = registry::look(&RealSystem, &cfg, &mut registry::RegistryCache::new());
        let looked = look.report.expect("registry look");
        let mut told = registry::running(&look.executables, &looked, &table.executables, cfg.uid)
            .expect("running apps told from the table");
        let mut queried = report.running.clone();
        told.sort();
        queried.sort();
        told == queried && looked.front == report.front
    });
    assert!(agreed, "running apps and front app as the JXA query says");
}
