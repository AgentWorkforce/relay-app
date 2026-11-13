//! Steady-state CPU of the broad capture thread under production pacing.
//! Runs only with BENCH_OUT set (the JSON-lines output file), in release:
//! `BENCH_OUT=/tmp/bench.jsonl cargo test --release --lib live::cpu_bench`
//! Knobs: BENCH_FILES, BENCH_LINES, BENCH_IDLE_SECS, BENCH_BUSY_SECS,
//! BENCH_WRITERS, BENCH_WRITE_MS. Results are one JSON line per phase.
use super::tests::*;
use super::*;
use std::fs;

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn cpu() -> Duration {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: the pointer is to writable, aligned storage for one `rusage`
    // that outlives the call; getrusage writes only into it.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    // SAFETY: all-zero bytes are a valid `rusage` (integers and timevals),
    // so the value is initialised whether or not getrusage wrote to it.
    let usage = unsafe { usage.assume_init() };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

/// Append `line` to the BENCH_OUT file.
fn emit(line: serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    let path = std::env::var("BENCH_OUT").unwrap_or_else(|_| String::new());
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(out, "{line}")
}

fn snapshot(shared: &Shared) -> serde_json::Value {
    shared.report().unwrap_or_default()
}

fn delta(after: &serde_json::Value, before: &serde_json::Value, key: &str) -> i64 {
    after[key].as_i64().unwrap_or(0) - before[key].as_i64().unwrap_or(0)
}

#[test]
fn cpu_bench() {
    // A measurement, not a check: runs only when asked for.
    if std::env::var_os("BENCH_OUT").is_none() {
        return;
    }
    let Some(home) = isolated_home("history::probe::live::cpu_bench::cpu_bench") else {
        return;
    };
    let files = knob("BENCH_FILES", 3000);
    let lines = knob("BENCH_LINES", 100);
    let idle = Duration::from_secs(knob("BENCH_IDLE_SECS", 300));
    let busy = Duration::from_secs(knob("BENCH_BUSY_SECS", 120));
    let writers = knob("BENCH_WRITERS", 6);
    let write_every = Duration::from_millis(knob("BENCH_WRITE_MS", 500));

    let directory = home.join("probe");
    let db = directory.join("history.db");
    let job = delivery::create_job(&delivery::open_db(&db).unwrap(), &job_config(), 0).unwrap();
    let pad = "x".repeat(300);
    for file in 0..files {
        let project = home.join(format!(".claude/projects/p{}", file % 100));
        fs::create_dir_all(&project).unwrap();
        let session = format!("s{file}");
        let mut body = String::new();
        for line in 0..lines {
            body.push_str(&transcript_line(
                &session,
                &format!("{session}-{line}-{pad}"),
            ));
        }
        fs::write(project.join(format!("{session}.jsonl")), body).unwrap();
    }

    // Wait for the first completed round, reporting progress each minute so a
    // slow round still leaves evidence.
    let first_round = |label: &str, shared: &Shared, limit: Duration| {
        let started = Instant::now();
        let cpu_before = cpu();
        let mut reported = Instant::now();
        while snapshot(shared)["sweeps"].as_u64().unwrap_or(0) < 1 && started.elapsed() < limit {
            std::thread::sleep(Duration::from_millis(200));
            if reported.elapsed() >= Duration::from_secs(60) {
                reported = Instant::now();
                let now = snapshot(shared);
                emit(
                    json!({"phase":format!("{label}_progress"),"wall_s":started.elapsed().as_secs(),
                    "cpu_ms":(cpu()-cpu_before).as_millis(),"cold_slices":now["cold_slices"],
                    "cold_entries":now["cold_entries"],"cold_paths":now["cold_paths"]}),
                )
                .expect("append a result line to BENCH_OUT");
            }
        }
        let now = snapshot(shared);
        emit(
            json!({"phase":label,"files":files,"wall_s":started.elapsed().as_secs_f64(),
            "cpu_ms":(cpu()-cpu_before).as_millis(),"sweeps":now["sweeps"],
            "cold_slices":now["cold_slices"],"cold_entries":now["cold_entries"],
            "cold_paths":now["cold_paths"],"cold_slice_ms_max":now["cold_slice_ms_max"]}),
        )
        .expect("append a result line to BENCH_OUT");
    };

    // Seed: capture the whole history unpaced, as an established install has.
    {
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::default());
        let capture = start(
            &directory,
            &config(&job, false),
            cancelled.clone(),
            shared.clone(),
            LiveOptions {
                duty_factor: 0,
                cold_duty_factor: 0,
                ..LiveOptions::default()
            },
        );
        first_round("seed_unpaced", &shared, Duration::from_secs(1800));
        cancelled.store(true, Ordering::Relaxed);
        drop(capture);
    }

    // Steady state: a restarted collector on production pacing.
    let cancelled = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::default());
    let capture = start(
        &directory,
        &config(&job, false),
        cancelled.clone(),
        shared.clone(),
        LiveOptions::default(),
    );
    first_round(
        "restart_round",
        &shared,
        Duration::from_secs(knob("BENCH_RESTART_SECS", 900)),
    );
    std::thread::sleep(Duration::from_secs(5));

    let phase = |name: &str, length: Duration, appended: &dyn Fn() -> u64| {
        let before = snapshot(&shared);
        let cpu_before = cpu();
        let wall = Instant::now();
        std::thread::sleep(length);
        let used = cpu() - cpu_before;
        let after = snapshot(&shared);
        let bytes = appended();
        let minutes = wall.elapsed().as_secs_f64() / 60.0;
        let mut out = json!({
            "phase": name,
            "wall_s": wall.elapsed().as_secs_f64(),
            "cpu_ms": used.as_millis(),
            "cpu_ms_per_min": used.as_millis() as f64 / minutes,
            "appended_kib": bytes as f64 / 1024.0,
        });
        if bytes > 0 {
            out["cpu_ms_per_kib"] = json!(used.as_millis() as f64 / (bytes as f64 / 1024.0));
        }
        for key in [
            "sweeps",
            "cold_slices",
            "cold_entries",
            "cold_paths",
            "ticks",
            "ticks_swept",
            "ticks_unchanged",
            "ticks_contended",
            "ticks_paced",
            "sessions_changed",
        ] {
            out[format!("d_{key}")] = json!(delta(&after, &before, key));
        }
        out["by_trigger"] = after["ticks_by_trigger"].clone();
        out["cold_slice_ms_max"] = after["cold_slice_ms_max"].clone();
        out["capture_lag_ms_max"] = after["capture_lag_ms_max"].clone();
        emit(out).expect("append a result line to BENCH_OUT");
    };

    phase("idle", idle, &|| 0);

    let written = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let stop_writers = Arc::new(AtomicBool::new(false));
    let handles: Vec<_> = (0..writers)
        .map(|writer| {
            let path = home.join(format!(".claude/projects/p{writer}/s{writer}.jsonl"));
            let (written, stop) = (written.clone(), stop_writers.clone());
            let pad = pad.clone();
            std::thread::spawn(move || {
                use std::io::Write;
                let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    // A held-open streaming writer, as Claude Code is.
                    let line =
                        transcript_line(&format!("s{writer}"), &format!("w{writer}-{n}-{pad}"));
                    file.write_all(line.as_bytes()).unwrap();
                    written.fetch_add(line.len() as u64, Ordering::Relaxed);
                    n += 1;
                    std::thread::sleep(write_every);
                }
                n
            })
        })
        .collect();
    let counted = written.clone();
    phase("busy", busy, &|| counted.load(Ordering::Relaxed));
    stop_writers.store(true, Ordering::Relaxed);
    let appended: Vec<u64> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    phase("idle_after", idle.min(Duration::from_secs(90)), &|| 0);
    cancelled.store(true, Ordering::Relaxed);
    drop(capture);
    // No evidence lost: every appended record reached the store.
    let (expected, captured): (Vec<_>, Vec<_>) = appended
        .iter()
        .enumerate()
        .map(|(writer, n)| {
            (
                lines as usize + *n as usize,
                super::tests::events(&db, &format!("s{writer}")),
            )
        })
        .unzip();
    emit(json!({"phase":"lossless","expected":expected,"captured":captured}))
        .expect("append a result line to BENCH_OUT");
    assert_eq!(captured, expected, "every appended record is captured");
}
