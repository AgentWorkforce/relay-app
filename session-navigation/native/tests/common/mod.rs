//! A fake Mac shaped like the surveyed one (sanitized: home `/Users/example`,
//! uid 501). Tests change the description, then `build()` scripts a fresh
//! `FakeSystem` from it.
#![allow(dead_code)]

use session_navigation::model::{App, Provider, SessionRef};
use session_navigation::parse::lstart;
use session_navigation_native::macos::machine;
use session_navigation_native::macos::parse::ps;
use session_navigation_native::system::{FakeSystem, Output, Run, RunError};
use session_navigation_native::MacConfig;

pub const HOME: &str = "/Users/example";
pub const UID: u32 = 501;
pub const BOOT: u64 = 1_794_291_866;
pub const NOW: u64 = 1_794_500_000;

pub const PS_IDENTITY: &str = include_str!("../fixtures/ps-identity.txt");
pub const PS_COMM: &str = include_str!("../fixtures/ps-comm.txt");
pub const PS_ARGS: &str = include_str!("../fixtures/ps-args.txt");
pub const SYSCTL: &str = include_str!("../fixtures/sysctl-boottime.txt");
pub const APPS_JXA: &str = include_str!("../fixtures/apps-jxa.json");
pub const LSOF_CWD: &str = include_str!("../fixtures/lsof-cwd.txt");

pub const CLAUDE_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
pub const CODEX_ID: &str = "01900000-0000-7000-8000-000000000002";
pub const OPENCODE_ID: &str = "ses_0123456789abAAAAAAAAAAAAAA";
pub const SECRET: &str = "SYNTHETIC-not-a-real-secret-43-chars-xxxxxx";

pub fn core_fixture(rel: &str) -> Vec<u8> {
    let path = format!(
        "{}/../core/tests/fixtures/{rel}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap_or_default()
}

pub fn cfg() -> MacConfig {
    MacConfig::for_user(HOME, UID)
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub ruid: u32,
    pub tty: &'static str,
    pub start: u64,
    pub ucomm: String,
    pub comm: String,
    pub args: String,
}

impl Proc {
    pub fn new(
        pid: u32,
        ppid: u32,
        tty: &'static str,
        ucomm: &str,
        comm: &str,
        args: &str,
    ) -> Self {
        Self {
            pid,
            ppid,
            uid: UID,
            ruid: UID,
            tty,
            start: BOOT + 1000 + u64::from(pid),
            ucomm: ucomm.to_owned(),
            comm: comm.to_owned(),
            args: args.to_owned(),
        }
    }
}

pub fn ghostty_chain() -> Vec<Proc> {
    let mut login = Proc::new(
        995,
        811,
        "ttys000",
        "login",
        "/usr/bin/login",
        "/usr/bin/login -flp example /bin/bash --noprofile --norc -c exec -l /bin/zsh",
    );
    login.uid = 0;
    vec![
        Proc::new(1, 0, "??", "launchd", "/sbin/launchd", "/sbin/launchd"),
        Proc::new(
            811,
            1,
            "??",
            "ghostty",
            "/Applications/Ghostty.app/Contents/MacOS/ghostty",
            "/Applications/Ghostty.app/Contents/MacOS/ghostty",
        ),
        login,
        Proc::new(998, 995, "ttys000", "zsh", "-/bin/zsh", "-/bin/zsh"),
    ]
}

pub struct Machine {
    pub procs: Vec<Proc>,
    pub apps_json: String,
    pub front_tty: Option<Result<Output, RunError>>,
    pub tools: Vec<(String, String)>,
    pub files: Vec<(String, Vec<u8>, u64)>,
    pub dirs: Vec<String>,
    pub runs: Vec<(Run, Vec<Result<Output, RunError>>)>,
    pub ps_broken: bool,
}

pub fn apps_json(front: Option<App>, running: &[App]) -> String {
    let mut value: serde_json::Value = serde_json::from_str(APPS_JXA).unwrap_or_default();
    value["front"] = front.map_or(serde_json::Value::Null, |a| a.bundle_id().into());
    value["running"] = running
        .iter()
        .map(|a| a.bundle_id())
        .collect::<Vec<_>>()
        .into();
    value.to_string()
}

impl Machine {
    pub fn new() -> Self {
        Self {
            procs: ghostty_chain(),
            apps_json: apps_json(None, &[App::Ghostty]),
            front_tty: None,
            tools: vec![
                (
                    "/opt/homebrew/bin/codex".into(),
                    "codex-cli 0.167.0\n".into(),
                ),
                (format!("{HOME}/.opencode/bin/opencode"), "2.2.6\n".into()),
            ],
            files: Vec::new(),
            dirs: Vec::new(),
            runs: Vec::new(),
            ps_broken: false,
        }
    }

    pub fn file(&mut self, path: &str, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.files.push((path.to_owned(), bytes.into(), NOW - 10));
        self
    }

    pub fn dir(&mut self, path: &str) -> &mut Self {
        self.dirs.push(path.to_owned());
        self
    }

    pub fn run(&mut self, run: Run, response: Result<Output, RunError>) -> &mut Self {
        self.runs.push((run, vec![response]));
        self
    }

    pub fn ps_outputs(&self) -> (String, String, String) {
        let mut a = String::new();
        let mut b = String::new();
        let mut c = String::new();
        for p in &self.procs {
            let start = lstart::format(p.start);
            a.push_str(&format!(
                "{:>5} {:>5} {:>5} {:>5} {:<8} {start}     {}\n",
                p.pid, p.ppid, p.uid, p.ruid, p.tty, p.ucomm
            ));
            b.push_str(&format!("{:>5} {start}     {}\n", p.pid, p.comm));
            c.push_str(&format!("{:>5} {start}     {}\n", p.pid, p.args));
        }
        (a, b, c)
    }

    pub fn agent_pids(&self) -> Vec<u32> {
        self.procs
            .iter()
            .filter(|p| ps::is_agent_cli(ps::classify(&p.ucomm, &p.comm)))
            .map(|p| p.pid)
            .collect()
    }

    pub fn lsof_cwd_run(&self) -> Option<Run> {
        let pids = self.agent_pids();
        (!pids.is_empty()).then(|| {
            Run::with_args(
                machine::LSOF,
                vec![
                    "-a".into(),
                    "-d".into(),
                    "cwd".into(),
                    "-Fpn".into(),
                    "-p".into(),
                    pids.iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                ],
            )
        })
    }

    pub fn build(&self) -> FakeSystem {
        let fake = FakeSystem::new();
        let cfg = cfg();
        fake.set_now(NOW);
        fake.on(
            &Run::new(machine::SYSCTL, &["-n", "kern.boottime"]),
            vec![Ok(Output::ok(SYSCTL))],
        );
        let (a, b, c) = self.ps_outputs();
        let [ra, rb, rc] = machine::ps_runs(&cfg);
        let failure = || Ok(Output::exit(1, "", "ps: boom"));
        fake.on(
            &ra,
            vec![if self.ps_broken {
                failure()
            } else {
                Ok(Output::ok(a))
            }],
        );
        fake.on(&rb, vec![Ok(Output::ok(b))]);
        fake.on(&rc, vec![Ok(Output::ok(c))]);
        if let Some(run) = self.lsof_cwd_run() {
            // Agent CLIs live in the project directory unless a test says so.
            let out: String = self
                .agent_pids()
                .iter()
                .map(|p| format!("p{p}\nfcwd\nn{HOME}/code/project\n"))
                .collect();
            fake.on(&run, vec![Ok(Output::ok(out))]);
        }
        fake.on(
            &machine::apps_run(&cfg),
            vec![Ok(Output::ok(self.apps_json.clone()))],
        );
        for (path, version) in &self.tools {
            fake.add_executable(path, NOW - 100);
            fake.on(
                &Run::new(path, &["--version"]),
                vec![Ok(Output::ok(version.clone()))],
            );
        }
        let front = self
            .front_tty
            .clone()
            .unwrap_or_else(|| Ok(Output::ok("/dev/ttys000\n")));
        for script in [
            session_navigation_native::macos::scripts::GHOSTTY_FRONT_TTY,
            session_navigation_native::macos::scripts::TERMINAL_FRONT_TTY,
        ] {
            fake.on(
                &machine::script_run(script, &[], cfg.common.timeouts.script_read),
                vec![front.clone()],
            );
        }
        for (path, bytes, mtime) in &self.files {
            fake.add_file(path, bytes.clone(), *mtime);
        }
        for dir in &self.dirs {
            fake.add_dir(dir);
        }
        for (run, responses) in &self.runs {
            fake.on(run, responses.clone());
        }
        fake
    }
}

pub fn claude() -> SessionRef {
    SessionRef::new(Provider::Claude, CLAUDE_ID)
}

pub fn codex() -> SessionRef {
    SessionRef::new(Provider::Codex, CODEX_ID)
}

pub fn opencode() -> SessionRef {
    SessionRef::new(Provider::OpenCode, OPENCODE_ID)
}

/// Every argv the fake recorded, flattened, for leak checks.
pub fn all_args(fake: &FakeSystem) -> String {
    fake.runs()
        .into_iter()
        .map(|(p, a)| format!("{p} {}", a.join(" ")))
        .collect::<Vec<_>>()
        .join("\n")
}
pub mod worlds;
#[allow(unused_imports)]
pub use worlds::*;
