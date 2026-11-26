//! Read-only: how long one look at this Mac takes. Wraps [`RealSystem`] so
//! every program run, file read and kernel read of one `observe` is timed,
//! then times N whole observes (one observer, so caches behave as inside one
//! `engine::open`).
//!
//!     cargo run --release -p session-navigation-native --example timing -- codex <thread-id> [N]
//!
//! `TIMING_NO_KERNEL=1` hides the kernel reads (the tool path) and
//! `TIMING_SEQUENTIAL=1` turns concurrency off, to compare.
//!
//! Nothing is executed, focused, launched or reserved. Program arguments are
//! shortened and stdin (the OpenCode password) is never printed.
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use session_navigation::model::{Provider, SessionRef};
use session_navigation::ports::Observe;
use session_navigation_native::system::{
    FsError, Kernel, Metadata, Output, RawProcess, Run, RunError,
};
use session_navigation_native::{MacConfig, MacObserver, RealSystem, System};

#[derive(Default)]
struct Timed {
    real: RealSystem,
    log: Mutex<Vec<(String, Duration)>>,
}

impl Timed {
    fn note<T>(&self, label: impl FnOnce() -> String, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        let took = start.elapsed();
        if let Ok(mut log) = self.log.lock() {
            log.push((label(), took));
        }
        out
    }

    fn take(&self) -> Vec<(String, Duration)> {
        self.log
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default()
    }
}

fn short(run: &Run) -> String {
    let name = run.program.rsplit('/').next().unwrap_or(&run.program);
    let args: Vec<String> = run
        .args
        .iter()
        .take(4)
        .map(|a| {
            let line = a.lines().next().unwrap_or("");
            let cut: String = line.chars().take(28).collect();
            if a.len() > cut.len() {
                format!("{cut}…")
            } else {
                cut
            }
        })
        .collect();
    format!("run {name} {}", args.join(" "))
}

fn kind(path: &str) -> String {
    let tail: Vec<&str> = path.rsplit('/').take(2).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join("/")
}

impl Kernel for Timed {
    fn boot_time(&self) -> Option<u64> {
        let k = self.real.kernel()?;
        self.note(|| "kernel boot_time".into(), || k.boot_time())
    }
    fn processes(&self) -> Option<Vec<RawProcess>> {
        let k = self.real.kernel()?;
        self.note(|| "kernel processes".into(), || k.processes())
    }
    fn cwd(&self, pid: u32) -> Option<String> {
        let k = self.real.kernel()?;
        self.note(|| "kernel cwd".into(), || k.cwd(pid))
    }
}

impl System for Timed {
    fn run(&self, run: &Run) -> Result<Output, RunError> {
        self.note(|| short(run), || self.real.run(run))
    }
    fn read_file(&self, path: &str, cap: usize) -> Result<Vec<u8>, FsError> {
        self.note(
            || format!("read {}", kind(path)),
            || self.real.read_file(path, cap),
        )
    }
    fn read_prefix(&self, path: &str, len: usize) -> Result<Vec<u8>, FsError> {
        self.note(
            || format!("read-prefix {}", kind(path)),
            || self.real.read_prefix(path, len),
        )
    }
    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        self.note(
            || format!("list {}", kind(path)),
            || self.real.list_dir(path),
        )
    }
    fn metadata(&self, path: &str) -> Result<Metadata, FsError> {
        self.note(
            || format!("stat {}", kind(path)),
            || self.real.metadata(path),
        )
    }
    fn canonicalize(&self, path: &str) -> Result<String, FsError> {
        self.note(
            || format!("canonicalize {}", kind(path)),
            || self.real.canonicalize(path),
        )
    }
    fn create_private_dir(&self, path: &str) -> Result<(), FsError> {
        self.real.create_private_dir(path)
    }
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        self.real.write_atomic(path, bytes)
    }
    fn remove_file(&self, path: &str) -> Result<(), FsError> {
        self.real.remove_file(path)
    }
    fn now(&self) -> u64 {
        self.real.now()
    }
    fn read_link(&self, path: &str) -> Result<String, FsError> {
        self.note(
            || format!("readlink {}", kind(path)),
            || self.real.read_link(path),
        )
    }
    fn concurrent(&self) -> bool {
        std::env::var_os("TIMING_SEQUENTIAL").is_none()
    }
    fn kernel(&self) -> Option<&dyn Kernel> {
        if std::env::var_os("TIMING_NO_KERNEL").is_some() {
            return None;
        }
        self.real.kernel().map(|_| self as &dyn Kernel)
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let provider = match args.first().map(String::as_str) {
        Some("claude") => Provider::Claude,
        Some("codex") => Provider::Codex,
        Some("opencode") => Provider::OpenCode,
        Some("grok") => Provider::Grok,
        _ => {
            eprintln!("usage: timing <claude|codex|opencode|grok> <session-id> [N]");
            return ExitCode::from(2);
        }
    };
    let Some(id) = args.get(1) else {
        eprintln!("usage: timing <claude|codex|opencode|grok> <session-id> [N]");
        return ExitCode::from(2);
    };
    let n: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(5);
    let Some(cfg) = MacConfig::from_env() else {
        eprintln!("HOME is not set to an absolute path");
        return ExitCode::from(2);
    };
    let session = SessionRef::new(provider, id.clone());
    let timed: &'static Timed = Box::leak(Box::default());

    // One cold observe, every call timed (calls overlap when they run
    // concurrently, so their sum can exceed the wall time).
    let mut observer = MacObserver::new(timed, cfg.clone());
    let start = Instant::now();
    let _ = observer.observe(&session);
    let first = start.elapsed();
    let log = timed.take();
    println!("first observe, per call (ms):");
    let mut sum = Duration::ZERO;
    for (label, took) in &log {
        sum += *took;
        if took.as_micros() >= 300 {
            println!("  {:>8.2}  {label}", ms(*took));
        }
    }
    let small = log.iter().filter(|(_, t)| t.as_micros() < 300).count();
    println!("  ({small} calls under 0.3 ms not listed)");
    println!(
        "  calls: {}  sum of calls: {:.1} ms  wall: {:.1} ms",
        log.len(),
        ms(sum),
        ms(first)
    );

    // N more observes on the same observer (as inside one engine::open).
    let mut walls = Vec::new();
    for _ in 0..n {
        let start = Instant::now();
        let _ = observer.observe(&session);
        walls.push(start.elapsed());
        let _ = timed.take();
    }
    let total: Duration = walls.iter().sum();
    let list: Vec<String> = walls.iter().map(|w| format!("{:.1}", ms(*w))).collect();
    println!(
        "{n} more observes on one observer: total {:.1} ms, mean {:.1} ms [{}]",
        ms(total),
        ms(total) / n.max(1) as f64,
        list.join(", ")
    );

    // One look after the reuse window ran out (registry and tools read
    // again, version probes still cached by the resolver).
    std::thread::sleep(Duration::from_millis(2100));
    let start = Instant::now();
    let _ = observer.observe(&session);
    println!(
        "a look after the 2 s reuse window: {:.1} ms",
        ms(start.elapsed())
    );
    let _ = timed.take();

    // N cold observers (no reuse between looks).
    let mut walls = Vec::new();
    for _ in 0..n {
        let mut fresh = MacObserver::new(timed, cfg.clone());
        let start = Instant::now();
        let _ = fresh.observe(&session);
        walls.push(start.elapsed());
        let _ = timed.take();
    }
    let total: Duration = walls.iter().sum();
    println!(
        "{n} cold observers: total {:.1} ms, mean {:.1} ms",
        ms(total),
        ms(total) / n.max(1) as f64
    );
    ExitCode::SUCCESS
}
