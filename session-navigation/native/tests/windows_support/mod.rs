//! A fake Windows machine (sanitized: profile `C:\Users\dev`). Tests
//! describe processes, windows and files, then `build()` scripts a fresh
//! `FakeSystem` that answers the adapter's PowerShell and `wsl.exe` runs
//! with JSON shaped like Windows PowerShell 5.1's `ConvertTo-Json`
//! (`"\/Date(ms)\/"` dates, PascalCase `Win32_Process` properties).
#![allow(dead_code)]

use serde_json::{json, Value};
use session_navigation::model::{BootId, ProcessIdentity, Provider, SessionRef};
use session_navigation::parse::lstart;
use session_navigation_native::system::{FakeSystem, Output, Run};
use session_navigation_native::windows::execute::{focus_run, identity_run, launch_run};
use session_navigation_native::windows::launch::LaunchSpec;
use session_navigation_native::windows::observe::observe_run;
use session_navigation_native::windows::{wsl, WindowsConfig};

pub const SYSROOT: &str = r"C:\Windows";
pub const PROFILE: &str = r"C:\Users\dev";
pub const APPDATA: &str = r"C:\Users\dev\AppData\Roaming";
pub const LOCAL: &str = r"C:\Users\dev\AppData\Local";
pub const PROJECT: &str = r"C:\Users\dev\code\project";
pub const CLAUDE_EXE: &str = r"C:\Users\dev\.local\bin\claude.exe";
pub const BOOT_ID: u64 = 4242;
pub const BOOT_TIME: u64 = 1_794_291_866;
pub const NOW: u64 = 1_794_500_000;
pub const CLAUDE_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
pub const PROFILE_GUID: &str = "{11111111-2222-3333-4444-555555555555}";
pub const DISTRO: &str = "Ubuntu";
pub const WSL_HOME: &str = "/home/dev";

pub fn cfg() -> WindowsConfig {
    WindowsConfig::native(SYSROOT, PROFILE, APPDATA, LOCAL).expect("valid config")
}

pub fn wsl_cfg() -> WindowsConfig {
    WindowsConfig::wsl(&cfg(), DISTRO, WSL_HOME).expect("valid WSL config")
}

pub fn claude() -> SessionRef {
    SessionRef::new(Provider::Claude, CLAUDE_ID)
}

pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/windows/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap_or_default()
}

pub fn identity(pid: u32, start: u64) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        start,
        boot: BootId(BOOT_ID),
    }
}

#[derive(Clone, Debug)]
pub struct WinProc {
    pub pid: u32,
    pub ppid: u32,
    pub start: u64,
    pub name: String,
    pub path: Option<String>,
    pub cmd: Option<String>,
}

impl WinProc {
    pub fn new(pid: u32, ppid: u32, path: &str, cmd: &str) -> Self {
        let name = path.rsplit('\\').next().unwrap_or(path).to_owned();
        Self {
            pid,
            ppid,
            start: BOOT_TIME + 100 + u64::from(pid),
            name,
            path: Some(path.to_owned()),
            cmd: Some(cmd.to_owned()),
        }
    }

    pub fn id(&self) -> ProcessIdentity {
        identity(self.pid, self.start)
    }

    pub fn json(&self) -> Value {
        json!({
            "ProcessId": self.pid,
            "ParentProcessId": self.ppid,
            "CreationDate": format!("/Date({}123)/", self.start),
            "Name": self.name,
            "ExecutablePath": self.path,
            "CommandLine": self.cmd,
            "SessionId": 1,
        })
    }
}

/// A window as the C# helper reports it.
#[derive(Clone, Copy, Debug)]
pub struct Win {
    pub hwnd: i64,
    pub pid: u32,
    pub created: i64,
}

impl Win {
    pub fn of(hwnd: i64, owner: &WinProc) -> Self {
        Self {
            hwnd,
            pid: owner.pid,
            created: i64::try_from(owner.start).unwrap_or(-1),
        }
    }

    pub fn json(&self) -> Value {
        json!({"Handle": self.hwnd, "ProcessId": self.pid, "Created": self.created})
    }
}

pub fn base_procs() -> Vec<WinProc> {
    let mut system = WinProc::new(4, 0, "System", "");
    system.path = None;
    system.cmd = None;
    vec![
        system,
        WinProc::new(
            2100,
            2000,
            r"C:\Windows\explorer.exe",
            r"C:\Windows\Explorer.EXE",
        ),
    ]
}

/// explorer → cmd.exe (+ its conhost.exe) → claude.exe; the console window
/// is reported as owned by `cmd.exe` (its first client).
pub fn console_chain() -> Vec<WinProc> {
    vec![
        WinProc::new(
            3000,
            2100,
            r"C:\Windows\System32\cmd.exe",
            r#""C:\Windows\System32\cmd.exe" "#,
        ),
        WinProc::new(
            3001,
            3000,
            r"C:\Windows\System32\conhost.exe",
            r"\??\C:\Windows\system32\conhost.exe 0x4",
        ),
        WinProc::new(
            3002,
            3000,
            CLAUDE_EXE,
            &format!(r#""{CLAUDE_EXE}" --resume {CLAUDE_ID}"#),
        ),
    ]
}

pub const WT_EXE: &str = r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21.2911.0_x64__8wekyb3d8bbwe\WindowsTerminal.exe";

/// explorer → WindowsTerminal.exe → (OpenConsole --headless, pwsh.exe →
/// claude.exe at `claude_pid`).
pub fn wt_chain(claude_pid: u32) -> Vec<WinProc> {
    vec![
        WinProc::new(4000, 2100, WT_EXE, &format!("\"{WT_EXE}\" ")),
        WinProc::new(
            4001,
            4000,
            r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21.2911.0_x64__8wekyb3d8bbwe\OpenConsole.exe",
            r#""C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21.2911.0_x64__8wekyb3d8bbwe\OpenConsole.exe" --headless --width 120 --height 30 --signal 0x9c4 --server 0x9bc"#,
        ),
        WinProc::new(
            4002,
            4000,
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            r#""C:\Program Files\PowerShell\7\pwsh.exe" "#,
        ),
        WinProc::new(
            claude_pid,
            4002,
            CLAUDE_EXE,
            &format!(r#""{CLAUDE_EXE}" --resume {CLAUDE_ID}"#),
        ),
    ]
}

/// One observation's worth of machine state.
#[derive(Clone, Debug)]
pub struct State {
    pub procs: Vec<WinProc>,
    pub windows: Vec<Win>,
    pub foreground: Option<Win>,
    pub wt_installed: bool,
}

impl State {
    pub fn new(procs: Vec<WinProc>) -> Self {
        Self {
            procs,
            windows: Vec::new(),
            foreground: Some(Win {
                hwnd: 0,
                pid: 0,
                created: -1,
            }),
            wt_installed: true,
        }
    }

    pub fn proc(&self, pid: u32) -> &WinProc {
        self.procs
            .iter()
            .find(|p| p.pid == pid)
            .expect("process in state")
    }

    pub fn report(&self) -> String {
        json!({
            "bootId": BOOT_ID,
            "bootTime": format!("/Date({BOOT_TIME}500)/"),
            "processes": self.procs.iter().map(WinProc::json).collect::<Vec<_>>(),
            "windows": self.windows.iter().map(Win::json).collect::<Vec<_>>(),
            "foreground": self.foreground.map(|w| w.json()),
            "terminal": {"Installed": self.wt_installed, "Version": self.wt_installed.then_some("1.21.2911.0")},
        })
        .to_string()
    }
}

pub struct Machine {
    pub cfg: WindowsConfig,
    /// What each observation sees, in order (the last repeats).
    pub states: Vec<State>,
    pub files: Vec<(String, Vec<u8>, u64)>,
    pub dirs: Vec<String>,
    pub runs: Vec<(Run, Vec<Output>)>,
    pub focus: Option<&'static str>,
    pub launch: Option<&'static str>,
}

impl Machine {
    pub fn new(state: State) -> Self {
        let mut m = Self {
            cfg: cfg(),
            states: vec![state],
            files: Vec::new(),
            dirs: vec![PROJECT.to_owned()],
            runs: Vec::new(),
            focus: None,
            launch: None,
        };
        m.file(CLAUDE_EXE, b"MZ".to_vec());
        m.version(CLAUDE_EXE, "2.1.321 (Claude Code)\n");
        m.file(&m.cfg.conhost(), b"MZ".to_vec());
        m
    }

    pub fn file(&mut self, path: &str, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.files.push((path.to_owned(), bytes.into(), NOW - 10));
        self
    }

    pub fn version(&mut self, path: &str, out: &str) -> &mut Self {
        self.runs.push((
            Run::new(path, &["--version"]),
            vec![Output::ok(out.as_bytes().to_vec())],
        ));
        self
    }

    /// The session's transcript (cwd `cwd`).
    pub fn transcript(&mut self, home: &str, cwd: &str) -> &mut Self {
        let line = json!({
            "type": "user", "sessionId": CLAUDE_ID, "cwd": cwd,
            "entrypoint": "cli", "version": "2.1.321",
        });
        let path = format!("{home}/projects/C--Users-dev-code-project/{CLAUDE_ID}.jsonl");
        self.file(&path, format!("{line}\n").into_bytes())
    }

    /// A Claude registry file for `pid`, started at `start`.
    pub fn registry(&mut self, home: &str, pid: u32, start: u64) -> &mut Self {
        let record = json!({
            "pid": pid, "sessionId": CLAUDE_ID, "cwd": PROJECT,
            "procStart": lstart::format(start), "kind": "interactive", "entrypoint": "cli",
        });
        let path = format!("{home}/sessions/{pid}.json");
        self.file(&path, record.to_string().into_bytes())
    }

    pub fn claude_home(&self) -> String {
        self.cfg.common.paths.claude_home.clone()
    }

    pub fn build(&self) -> FakeSystem {
        let fake = FakeSystem::new();
        fake.set_now(NOW);
        for (path, bytes, mtime) in &self.files {
            fake.add_file(path, bytes.clone(), *mtime);
        }
        for dir in &self.dirs {
            fake.add_dir(dir);
        }
        let reports: Vec<_> = self
            .states
            .iter()
            .map(|s| Ok(Output::ok(s.report().into_bytes())))
            .collect();
        fake.on(&observe_run(&self.cfg), reports.clone());
        fake.on(&identity_run(&self.cfg), reports);
        if let Some(status) = self.focus {
            let body = json!({ "status": status }).to_string();
            // The focus run's argv does not depend on its stdin.
            let any = focus_run(
                &self.cfg,
                session_navigation::hosts::WindowId(0),
                &identity(0, 0),
                &identity(0, 0),
            );
            fake.on(&any, vec![Ok(Output::ok(body.into_bytes()))]);
        }
        if let Some(status) = self.launch {
            let body = json!({ "status": status }).to_string();
            let any = LaunchSpec {
                file: String::new(),
                line: String::new(),
                cwd: String::new(),
                env: Default::default(),
            };
            fake.on(
                &launch_run(&self.cfg, &any),
                vec![Ok(Output::ok(body.into_bytes()))],
            );
        }
        for (run, outputs) in &self.runs {
            fake.on(run, outputs.iter().cloned().map(Ok).collect());
        }
        fake
    }
}

/// The stdin JSON of every recorded run of `script`'s encoded argv.
pub fn inputs(fake: &FakeSystem, run: &Run) -> Vec<Value> {
    fake.calls()
        .into_iter()
        .filter_map(|c| match c {
            session_navigation_native::system::Call::Run {
                program,
                args,
                stdin,
                ..
            } if program == run.program && args == run.args => {
                stdin.and_then(|s| serde_json::from_slice(s.expose_secret()).ok())
            }
            _ => None,
        })
        .collect()
}

/// `count` runs of `run` (by program and argv).
pub fn count(fake: &FakeSystem, run: &Run) -> usize {
    fake.runs()
        .into_iter()
        .filter(|(p, a)| *p == run.program && *a == run.args)
        .count()
}

/// A WSL `/proc` probe line for one process.
pub fn wsl_proc(pid: u32, ppid: u32, exe: &str, argv: &[&str], tty_nr: u64, ticks: u64) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let name = exe.rsplit('/').next().unwrap_or(exe);
    let stat = format!(
        "{pid} ({name}) S {ppid} {pid} {pid} {tty_nr} {pid} 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {ticks} 0 0"
    );
    let cmd: Vec<u8> = argv.iter().flat_map(|a| a.bytes().chain([0])).collect();
    let field = |b: &[u8]| if b.is_empty() { "-".to_owned() } else { hex(b) };
    format!(
        "proc {pid} 1000 1000 {} {} {} {}",
        hex(stat.as_bytes()),
        field(exe.as_bytes()),
        field(WSL_HOME.as_bytes()),
        field(&cmd)
    )
}

/// The WSL table probe's whole output.
pub fn wsl_table(lines: &[String]) -> String {
    let mut out = String::from(
        "boot 3f1b9c2e-8d4a-4b6f-9e2a-1c0d5e7f8a9b\nbtime 1794400000\nhz 100\nuid 1000\n",
    );
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Script `wsl.exe` for the WSL config: `-l -q`, the table and the tools.
pub fn script_wsl(fake: &FakeSystem, table: &str, tools: &str) {
    let cfg = wsl_cfg();
    let list: Vec<u8> = "Ubuntu\r\ndocker-desktop\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    fake.on(&wsl::list_run(&cfg), vec![Ok(Output::ok(list))]);
    fake.on(
        &wsl::table_run(&cfg, DISTRO),
        vec![Ok(Output::ok(table.as_bytes().to_vec()))],
    );
    if let Some(run) = wsl::tools_run(&cfg, DISTRO) {
        fake.on(&run, vec![Ok(Output::ok(tools.as_bytes().to_vec()))]);
    }
}
