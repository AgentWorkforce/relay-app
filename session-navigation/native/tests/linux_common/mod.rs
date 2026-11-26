//! A fake Linux machine for the Linux adapter tests: `/proc` files in the
//! kernel's own formats (fixtures under `tests/fixtures/linux/`), helper
//! programs on the program path, desktop files, and scripted runs.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod scenes;

use session_navigation::model::{BootId, ProcessIdentity};
use session_navigation_native::linux::procfs::parse_boot_id;
use session_navigation_native::linux::LinuxConfig;
use session_navigation_native::system::{FakeSystem, Output, Run};

pub const HOME: &str = "/home/example";
pub const UID: u32 = 1000;
pub const BTIME: u64 = 1_762_900_000;
pub const PROJECT: &str = "/home/example/code/project";
pub const GROK_ID: &str = "0b5a3c1e-1111-4222-8333-444455556666";
pub const GROK_BIN: &str = "/home/example/.grok/bin/grok";
pub const GROK_REAL: &str = "/home/example/.grok/bin/grok-0.1.23-linux-x86_64";
pub const TMUX: &str = "/usr/bin/tmux";
pub const KITTY: &str = "/usr/bin/kitty";
pub const WEZTERM: &str = "/usr/bin/wezterm";
pub const XDOTOOL: &str = "/usr/bin/xdotool";
pub const WMCTRL: &str = "/usr/bin/wmctrl";
pub const ENV: &str = "/usr/bin/env";
pub const KILL: &str = "/usr/bin/kill";
pub const KITTY_ADDRESS: &str = "unix:/run/user/1000/kitty-2470";
pub const WEZ_SOCKET: &str = "/run/user/1000/wezterm/gui-sock-4100";
pub const TMUX_SOCKET: &str = "/tmp/tmux-1000/default";

pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/linux/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

pub fn fixture_text(name: &str) -> String {
    String::from_utf8(fixture(name)).unwrap()
}

pub fn boot() -> BootId {
    parse_boot_id(&fixture("boot_id.txt")).unwrap()
}

/// Seconds after boot → ticks at 100 Hz.
pub fn ticks(secs: u64) -> u64 {
    secs * 100
}

pub fn identity(pid: u32, start_after_boot: u64) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        start: BTIME + start_after_boot,
        boot: boot(),
    }
}

/// `tty_nr` of `/dev/pts/<n>` (major 136).
pub fn pts(n: u64) -> u64 {
    (136 << 8) | n
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
    /// `None`: the `exe` link is unreadable (another user's process).
    pub exe: Option<String>,
    pub ruid: u32,
    pub euid: u32,
    pub tty_nr: u64,
    /// Seconds after boot.
    pub start: u64,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Proc {
    pub fn new(pid: u32, ppid: u32, exe: &str, argv: &[&str]) -> Self {
        let comm: String = exe.rsplit('/').next().unwrap().chars().take(15).collect();
        Self {
            pid,
            ppid,
            comm,
            exe: Some(exe.to_owned()),
            ruid: UID,
            euid: UID,
            tty_nr: 0,
            start: u64::from(pid),
            argv: argv.iter().map(|a| (*a).to_owned()).collect(),
            cwd: None,
            env: Vec::new(),
        }
    }

    pub fn tty(mut self, n: u64) -> Self {
        self.tty_nr = pts(n);
        self
    }

    pub fn started(mut self, secs_after_boot: u64) -> Self {
        self.start = secs_after_boot;
        self
    }

    pub fn cwd(mut self, cwd: &str) -> Self {
        self.cwd = Some(cwd.to_owned());
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_owned(), value.to_owned()));
        self
    }

    pub fn root(mut self) -> Self {
        self.ruid = 0;
        self.euid = 0;
        self.exe = None;
        self
    }

    pub fn identity(&self) -> ProcessIdentity {
        identity(self.pid, self.start)
    }

    /// The 52-field `/proc/<pid>/stat` line of Linux 5.x/6.x.
    pub fn stat(&self) -> String {
        format!(
            "{} ({}) S {} {} {} {} -1 4194560 4386 21539 0 3 9 4 25 13 20 0 1 0 {} 10133504 1310 \
             18446744073709551615 1 1 0 0 0 0 65536 3686404 1266761467 1 0 0 17 3 0 0 0 0 0 \
             1 1 1 1 1 1 1 0\n",
            self.pid,
            self.comm,
            self.ppid,
            self.pid,
            self.pid,
            self.tty_nr,
            ticks(self.start)
        )
    }

    pub fn status(&self) -> String {
        format!(
            "Name:\t{}\nUmask:\t0022\nState:\tS (sleeping)\nTgid:\t{pid}\nNgid:\t0\nPid:\t{pid}\n\
             PPid:\t{}\nTracerPid:\t0\nUid:\t{r}\t{e}\t{e}\t{e}\nGid:\t1000\t1000\t1000\t1000\n\
             FDSize:\t256\nThreads:\t1\n",
            self.comm,
            self.ppid,
            pid = self.pid,
            r = self.ruid,
            e = self.euid,
        )
    }
}

pub fn proc_dir(pid: u32) -> String {
    format!("/proc/{pid}")
}

/// The base machine: init, the session's systemd, and a display.
pub struct Machine {
    pub procs: Vec<Proc>,
    /// `x11`, `wayland` or `None` (no desktop session).
    pub display: Option<&'static str>,
    /// Programs on the program path (`/usr/bin/<name>`).
    pub programs: Vec<&'static str>,
    pub desktop_files: Vec<&'static str>,
}

impl Machine {
    pub fn new(display: Option<&'static str>) -> Self {
        Self {
            procs: vec![
                Proc::new(1, 0, "/usr/lib/systemd/systemd", &["/sbin/init"])
                    .started(0)
                    .root(),
                Proc::new(2, 0, "/kthreadd", &[]).started(0).root(),
                Proc::new(
                    1600,
                    1,
                    "/usr/lib/systemd/systemd",
                    &["/lib/systemd/systemd", "--user"],
                )
                .started(5),
            ],
            display,
            programs: vec![
                "env", "kill", "xdotool", "wmctrl", "kitty", "wezterm", "konsole", "xterm",
            ],
            desktop_files: vec![
                "kitty",
                "org.wezfurlong.wezterm",
                "org.kde.konsole",
                "xterm",
            ],
        }
    }

    pub fn push(&mut self, proc: Proc) -> &mut Self {
        self.procs.retain(|p| p.pid != proc.pid);
        self.procs.push(proc);
        self
    }

    pub fn write_procs(&self, fake: &FakeSystem) {
        for p in &self.procs {
            let dir = proc_dir(p.pid);
            fake.add_file(&format!("{dir}/stat"), p.stat(), 0);
            fake.add_file(&format!("{dir}/status"), p.status(), 0);
            let mut cmdline = Vec::new();
            for a in &p.argv {
                cmdline.extend_from_slice(a.as_bytes());
                cmdline.push(0);
            }
            fake.add_file(&format!("{dir}/cmdline"), cmdline, 0);
            let mut environ = b"LANG=C.UTF-8\0SECRET_TOKEN=do-not-read\0".to_vec();
            for (k, v) in &p.env {
                environ.extend_from_slice(format!("{k}={v}\0").as_bytes());
            }
            fake.add_file(&format!("{dir}/environ"), environ, 0);
            match &p.exe {
                Some(exe) => fake.add_symlink(&format!("{dir}/exe"), exe),
                None => {
                    fake.make_unreadable(&format!("{dir}/exe"));
                    fake.make_unreadable(&format!("{dir}/environ"));
                    fake.make_unreadable(&format!("{dir}/cwd"));
                }
            }
            if let Some(cwd) = &p.cwd {
                fake.add_symlink(&format!("{dir}/cwd"), cwd);
            }
        }
    }

    pub fn build(&self) -> FakeSystem {
        let fake = FakeSystem::new();
        fake.set_now(BTIME + 10_000);
        fake.add_file("/proc/sys/kernel/random/boot_id", fixture("boot_id.txt"), 0);
        fake.add_file("/proc/stat", fixture("proc-stat.txt"), 0);
        fake.add_file("/proc/sys/kernel/hostname", "devbox\n", 0);
        self.write_procs(&fake);
        for name in &self.programs {
            fake.add_executable(&format!("/usr/bin/{name}"), 1);
        }
        for id in &self.desktop_files {
            fake.add_file(
                &format!("/usr/share/applications/{id}.desktop"),
                "[Desktop Entry]\n",
                1,
            );
        }
        fake.add_dir(PROJECT);
        fake
    }
}

pub fn cfg(display: Option<&str>) -> LinuxConfig {
    let mut env = vec![
        ("HOME", HOME.to_owned()),
        ("PATH", "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("XDG_RUNTIME_DIR", "/run/user/1000".to_owned()),
    ];
    match display {
        Some("x11") => {
            env.push(("XDG_SESSION_TYPE", "x11".into()));
            env.push(("DISPLAY", ":0".into()));
            env.push(("XAUTHORITY", "/run/user/1000/gdm/Xauthority".into()));
        }
        Some("wayland") => {
            env.push(("XDG_SESSION_TYPE", "wayland".into()));
            env.push(("WAYLAND_DISPLAY", "wayland-0".into()));
            env.push(("DISPLAY", ":0".into()));
        }
        _ => {}
    }
    LinuxConfig::for_user(HOME, UID).with_session_env(env)
}

/// An X11 helper run as the adapter builds it.
pub fn x11_run(program: &str, args: &[&str], display: Option<&str>) -> Run {
    session_navigation_native::linux::x11::run(
        program,
        args.iter().map(|a| (*a).to_owned()).collect(),
        &cfg(display),
    )
}

pub fn ok(text: impl Into<Vec<u8>>) -> Result<Output, session_navigation_native::system::RunError> {
    Ok(Output::ok(text))
}

pub fn fail(stderr: &str) -> Result<Output, session_navigation_native::system::RunError> {
    Ok(Output::exit(1, "", stderr))
}
