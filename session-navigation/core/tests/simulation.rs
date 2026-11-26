//! Seeded random runs across all six hosts and every world-variable
//! combination, with the invariant oracle checked after every step.
use session_navigation::sim::gen::generate;
use session_navigation::sim::runner::{explain, run, shrink, RunStats};
use session_navigation::sim::WorldVars;

/// The first round's variables, enumerated exhaustively.
const COMBINATIONS: u32 = 1 << WorldVars::LEGACY;
/// Every variable, for seed-derived draws.
const ALL_VARS: u64 = 1 << WorldVars::COUNT;

fn check(seed: u64, bits: u32, len: usize) -> RunStats {
    let vars = WorldVars::from_bits(bits);
    let events = generate(seed, len);
    match run(seed, vars, &events) {
        Ok(stats) => stats,
        Err(failure) => {
            let minimal = shrink(seed, vars, &events, &failure.violation);
            panic!("{}", explain(&failure, &minimal));
        }
    }
}

fn merge(total: &mut RunStats, stats: RunStats) {
    total.opens += stats.opens;
    total.verified += stats.verified;
    total.verified_resumes += stats.verified_resumes;
    total.blocked += stats.blocked;
    total.failed += stats.failed;
    total.unverified += stats.unverified;
    total.verified_kinds.extend(stats.verified_kinds);
    for (reason, n) in stats.blocked_reasons {
        *total.blocked_reasons.entry(reason).or_default() += n;
    }
    total.confirmations += stats.confirmations;
    for (target, source) in [
        (&mut total.by_provider, stats.by_provider),
        (&mut total.by_action, stats.by_action),
        (&mut total.refusals, stats.refusals),
        (&mut total.by_host, stats.by_host),
        (&mut total.by_platform, stats.by_platform),
    ] {
        for (key, classes) in source {
            let entry = target.entry(key).or_default();
            for (class, n) in classes {
                *entry.entry(class).or_default() += n;
            }
        }
    }
}

/// Every one of the 2^14 world-variable combinations, one seed each.
#[test]
fn every_world_variable_combination() {
    let mut total = RunStats::default();
    for bits in 0..COMBINATIONS {
        let seed = u64::from(bits).wrapping_mul(0x9E37_79B9) ^ 0xC0FFEE;
        merge(&mut total, check(seed, bits, 60));
    }
    assert!(total.verified > 1000, "too few verified opens: {total:?}");
}

/// Every combination of the Grok and terminal-host variables (bits 14..26),
/// each with seed-derived first-round variables, on every platform in turn.
#[test]
fn every_new_variable_combination() {
    let mut total = RunStats::default();
    let new_bits = WorldVars::COUNT - WorldVars::LEGACY;
    for combo in 0..1u32 << new_bits {
        let seed = u64::from(combo).wrapping_mul(0x2545_F491) ^ 0xBEEF;
        let legacy =
            u32::try_from(seed.wrapping_mul(2_654_435_761) % u64::from(COMBINATIONS)).unwrap();
        let bits = legacy | combo << WorldVars::LEGACY;
        merge(&mut total, check(seed, bits, 60));
    }
    assert!(total.verified > 1000, "too few verified opens: {total:?}");
}

/// Longer sequences with seed-derived variables.
#[test]
fn long_random_runs() {
    let mut total = RunStats::default();
    for seed in 0..6000u64 {
        let bits = u32::try_from(seed.wrapping_mul(2_654_435_761) % ALL_VARS).unwrap();
        merge(&mut total, check(seed, bits, 150));
    }
    // The runs must actually exercise success paths, not only refusals.
    assert!(total.verified_resumes > 100, "{total:?}");
    for kind in ["FocusPane", "OpenExisting", "Resume", "BringForward"] {
        assert!(
            total.verified_kinds.iter().any(|(_, k)| k == kind),
            "no verified {kind}: {:?}",
            total.verified_kinds
        );
    }
    eprintln!("long_random_runs: {total:?}");
    eprintln!("{}", usefulness(&total));
}

/// Share of opens per provider (and per planned Claude action) by outcome
/// class, plus the PossiblyOpenElsewhere refusals per provider.
fn usefulness(total: &RunStats) -> String {
    let mut out = String::from("usefulness (share of opens):\n");
    let rows = total
        .by_provider
        .iter()
        .chain(total.by_action.iter().filter(|(k, _)| {
            matches!(
                k.as_str(),
                "Claude/OpenExisting" | "Claude/Reopen" | "Codex/Resume" | "OpenCode/Resume"
            )
        }));
    for (key, classes) in rows {
        let n: usize = classes.values().sum();
        let share = |c: &str| 100.0 * *classes.get(c).unwrap_or(&0) as f64 / n.max(1) as f64;
        out.push_str(&format!(
            "  {key:<20} n={n:<6} verified={:5.1}% unverified={:5.1}% confirm={:5.1}% choice={:5.1}% failed={:5.1}% refused={:5.1}%\n",
            share("Verified"),
            share("DispatchedUnverified"),
            share("NeedsConfirmation"),
            share("NeedsChoice"),
            share("Failed"),
            share("Refused"),
        ));
    }
    out.push_str("by provider/origin host:\n");
    for (key, classes) in &total.by_host {
        let n: usize = classes.values().sum();
        let share = |c: &str| 100.0 * *classes.get(c).unwrap_or(&0) as f64 / n.max(1) as f64;
        out.push_str(&format!(
            "  {key:<28} n={n:<6} verified={:5.1}% unverified={:5.1}% refused={:5.1}%\n",
            share("Verified"),
            share("DispatchedUnverified"),
            share("Refused"),
        ));
    }
    for (platform, row) in &total.by_platform {
        out.push_str(&format!("  {platform}: {row:?}\n"));
    }
    for (provider, reasons) in &total.refusals {
        out.push_str(&format!(
            "  {provider} PossiblyOpenElsewhere refusals: {}\n",
            reasons.get("PossiblyOpenElsewhere").unwrap_or(&0)
        ));
    }
    out
}

/// `SESSION_NAVIGATION_REPLAY=<seed>:<vars> cargo test --test simulation replay -- --nocapture`
#[test]
fn replay() {
    let Ok(spec) = std::env::var("SESSION_NAVIGATION_REPLAY") else {
        return;
    };
    let (seed, bits) = spec.split_once(':').expect("seed:vars");
    let seed: u64 = seed.parse().expect("seed");
    let bits: u32 = bits
        .trim_start_matches("0x")
        .parse()
        .or_else(|_| u32::from_str_radix(bits.trim_start_matches("0x"), 16))
        .expect("vars");
    for len in [40, 120] {
        let stats = check(seed, bits, len);
        eprintln!("replay seed={seed} vars={bits:#x} len={len}: {stats:?}");
    }
}

/// `SESSION_NAVIGATION_TRACE=<seed>:<vars>:<len>` prints the minimal failing trace.
#[test]
fn trace() {
    let Ok(spec) = std::env::var("SESSION_NAVIGATION_TRACE") else {
        return;
    };
    let parts: Vec<&str> = spec.split(':').collect();
    let seed: u64 = parts[0].parse().unwrap();
    let bits: u32 = parts[1].parse().unwrap();
    let len: usize = parts[2].parse().unwrap();
    let vars = WorldVars::from_bits(bits);
    let events = generate(seed, len);
    let events = match run(seed, vars, &events) {
        Err(f) => shrink(seed, vars, &events, &f.violation),
        Ok(_) => events,
    };
    let (result, lines) = session_navigation::sim::runner::run_traced(seed, vars, &events);
    for line in lines {
        eprintln!("{line}");
    }
    eprintln!("{result:?}");
}

/// Seeds that once failed during development, kept as permanent tests:
/// (seed, world-variable bits, sequence length, what it found).
const REGRESSIONS: &[(u64, u32, usize, &str)] = &[
    (
        140_689_329_315,
        0x35,
        40,
        "headless Claude after /clear without registry rewrite",
    ),
    (
        4_294_872_902_828,
        0x652,
        40,
        "OpenCode desktop launched next to a pending TUI launch",
    ),
    (
        570,
        0x141a,
        120,
        "pid reuse helper overwrote a pane's shell",
    ),
    (
        13_335_881_068_878,
        0x13a0,
        40,
        "reservation released by someone else's ownership",
    ),
    (
        123,
        0x180b,
        40,
        "pending launch counted after its window closed",
    ),
    (552_135_170_494, 0xd0, 40, "same, Codex"),
    (839, 0x1317, 120, "OpenCode desktop tab restore"),
    (
        4885,
        0x1e85,
        150,
        "Codex desktop link next to a pending TUI launch",
    ),
    (
        16,
        0x377_9b10,
        150,
        "a launch counted as resolved when its window reported a tty before any process",
    ),
    (
        4,
        0xdd_e6c4,
        150,
        "a terminal restarted between decision and focus (tty reused)",
    ),
    (
        82,
        0x1c4_fab2,
        150,
        "Windows Terminal window raised with another tab in front",
    ),
    (
        91,
        0x1b8_41eb,
        150,
        "Codex CLI removed while our launch waited at the update menu",
    ),
    (
        138,
        0x1e7_996a,
        150,
        "Grok resume that never started, confirmed gone, resumed again",
    ),
    (
        1775,
        0x2a4_c23f,
        150,
        "OpenCode TUI hidden by a slow window, confirmed gone by mistake",
    ),
    (
        3437,
        0x2cc_cd5d,
        150,
        "slow-start tty reused by a later pane",
    ),
    (
        29_047_494_857_961,
        0x2abf,
        60,
        "/clear in a window whose processes had not appeared",
    ),
    (
        20_462,
        0x220_c18e,
        150,
        "Grok /new must start the session in the TUI's own directory",
    ),
    (
        34_683,
        0x1d3_cf0b,
        150,
        "Grok resume from another directory duplicating the id",
    ),
    (
        1_832_153,
        0x29e_31cd,
        150,
        "OpenCode desktop started (new version) while our TUI launch was pending",
    ),
    (
        5_126_032,
        0xf4_4ed0,
        400,
        "same, the desktop upgraded after the launch was dispatched",
    ),
];

#[test]
fn regressions() {
    for (seed, bits, len, _) in REGRESSIONS {
        check(*seed, *bits, *len);
    }
}
