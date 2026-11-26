use super::*;

// ====================================== 4. generator / 2. vacuity measures ===

const MIN: usize = 10;
const SEEDS: u64 = 6000;
const LEN: usize = 150;

fn totals() -> &'static (BTreeMap<String, usize>, Vec<String>) {
    use std::sync::OnceLock;
    static T: OnceLock<(BTreeMap<String, usize>, Vec<String>)> = OnceLock::new();
    T.get_or_init(|| aggregate(0..SEEDS, LEN))
}

fn get(map: &BTreeMap<String, usize>, key: &str) -> usize {
    map.get(key).copied().unwrap_or(0)
}

/// Guard against drift: the probe must reproduce the official runner's
/// statistics for the same seeds.
#[test]
fn replica_matches_the_official_runner() {
    for seed in 0..60u64 {
        let bits = long_run_bits(seed);
        let vars = WorldVars::from_bits(bits);
        let events = generate(seed, LEN);
        let official = run(seed, vars, &events).expect("official run passes");
        let p = probe_run(seed, vars, &events);
        assert!(p.violation.is_none(), "{:?}", p.violation);
        let t = &p.totals;
        assert_eq!(
            (
                official.opens,
                official.verified,
                official.blocked,
                official.failed,
                official.unverified
            ),
            (t.opens, t.verified, t.blocked, t.failed, t.unverified),
            "seed {seed}"
        );
    }
}

/// Measures, per host, how often each interesting situation actually occurs
/// in the seeded random runs (same seed → vars mapping as long_random_runs).
/// An invariant guarding a situation that never arises is vacuous.
#[test]
fn every_host_meets_every_interesting_situation() {
    let (t, violations) = totals();
    assert!(
        violations.is_empty(),
        "probe found violations: {violations:?}"
    );
    let required: &[(&str, &[&str])] = &[
        (
            "claude-desktop",
            &[
                "live_locatable",
                "stopped_open_in_app",
                "archived_revive_dispatched",
                "stale_registry_dead_pid",
                "unverified_version",
                "useful_outcome",
            ],
        ),
        (
            "claude-ghostty",
            &[
                "live_locatable",
                "stopped_resume_dispatched",
                "stopped_resume_verified",
                "stale_registry_dead_pid",
                "pid_reuse_across_reboot",
                "double_click_after_start",
                "open_while_own_launch_pending",
                "permission_denied",
                "unverified_version",
            ],
        ),
        ("claude-headless", &["take_over_stop_dispatched"]),
        (
            "codex-desktop",
            &[
                "live_locatable",
                "stopped_open_in_app",
                "unverified_version",
                "useful_outcome",
            ],
        ),
        (
            "codex-ghostty",
            &[
                "live_locatable",
                "stopped_resume_dispatched",
                "stopped_resume_verified",
                "double_click_after_start",
                "open_while_own_launch_pending",
                "permission_denied",
                "unverified_version",
            ],
        ),
        ("codex-headless", &["take_over_stop_dispatched"]),
        (
            "opencode-desktop",
            &["bring_forward_dispatched", "unverified_version"],
        ),
        (
            "opencode-terminal",
            &[
                "live_locatable",
                "stopped_resume_dispatched",
                "stopped_resume_verified",
                "double_click_after_start",
                "open_while_own_launch_pending",
                "permission_denied",
                "unverified_version",
            ],
        ),
        ("opencode-headless", &["take_over_stop_dispatched"]),
    ];
    let mut table = String::new();
    let mut thin = Vec::new();
    for (host, situations) in required {
        for s in *situations {
            let n = get(t, &format!("{host}|{s}"));
            table.push_str(&format!("  {host:18} {s:32} {n}\n"));
            if n < MIN {
                thin.push(format!("{host}|{s}={n}"));
            }
        }
    }
    eprintln!("situation counts over {SEEDS} runs x {LEN} events:\n{table}");
    assert!(
        thin.is_empty(),
        "situations (almost) never generated: {thin:?}"
    );
}

/// Each UNVERIFIED world variable must be exercised under BOTH values by an
/// event that its value can affect, or "the invariants hold under both
/// answers" is vacuous for it. Note: `codex_resume_archived_works`,
/// `opencode_tui_opens_archived_or_child` and `codex_second_writer_refused`
/// are only reachable through person events (the planner refuses first).
#[test]
fn every_world_variable_is_exercised_under_both_values() {
    let (t, _) = totals();
    let mut thin = Vec::new();
    let mut table = String::new();
    for name in WorldVars::NAMES {
        let off = get(t, &format!("var:{name}=0"));
        let on = get(t, &format!("var:{name}=1"));
        table.push_str(&format!("  {name:40} off={off:6} on={on:6}\n"));
        if off < MIN || on < MIN {
            thin.push(format!("{name}: off={off} on={on}"));
        }
    }
    eprintln!("world-variable triggers:\n{table}");
    assert!(
        thin.is_empty(),
        "variables not exercised under both values: {thin:?}"
    );
}

/// FINDING (over-caution, measured). In worlds where the Claude registry IS
/// rewritten (what the CLI code indicates, facts C/F), every Claude
/// CLI hosting a session has a registry file naming it, so a
/// PossiblyOpenElsewhere refusal with no live host in truth protects nothing.
/// Claim: such needless refusals should not occur. Also prints how many
/// refusals were protective when the registry is NOT rewritten.
#[test]
fn claude_possibly_open_elsewhere_refusals_are_not_needless() {
    let (t, _) = totals();
    let mut needless_rw = 0;
    let mut lines = String::new();
    for (k, n) in t
        .iter()
        .filter(|(k, _)| k.contains("possibly_open_elsewhere"))
    {
        lines.push_str(&format!("  {k}: {n}\n"));
        if k.starts_with("claude") && k.contains("rewritten=1|needless") {
            needless_rw += n;
        }
    }
    let hidden: usize = t
        .iter()
        .filter(|(k, _)| k.ends_with("|hidden_claude_host"))
        .map(|(_, n)| *n)
        .sum();
    eprintln!("PossiblyOpenElsewhere refusals:\n{lines}  opens whose target had a hidden (unregistered) Claude host: {hidden}");
    assert_eq!(
        needless_rw, 0,
        "needless Claude refusals with a rewritten registry"
    );
}

/// FINDING (fidelity / generator). Claude facts header and correction 4
/// (VERIFIED): "No `claude` is on PATH" on the surveyed Mac; the CLI exists
/// only inside Claude.app's support directory, outside relay's lookup list.
/// The sim installs all three CLIs at their verified versions in every world
/// and no event removes one, so ToolNotInstalled — the observed reality for
/// the Claude CLI — is never generated, and the plans' bare `claude` program
/// name is never challenged. Fix: a tool-install event/variable, and plans
/// that carry the resolved executable path.
#[test]
fn random_runs_ever_see_a_missing_cli() {
    let (t, _) = totals();
    let n: usize = t
        .iter()
        .filter(|(k, _)| k.ends_with("|tool_not_installed"))
        .map(|(_, n)| *n)
        .sum();
    assert!(n >= MIN, "ToolNotInstalled reached {n} times");
}

/// Strength check on the generator: every event kind is produced.
#[test]
fn generator_produces_every_event_kind() {
    let mut seen = BTreeSet::new();
    for seed in 0..200 {
        for e in generate(seed, LEN) {
            let name = format!("{e:?}");
            seen.insert(name.split([' ', '(', '{']).next().unwrap_or("").to_owned());
        }
    }
    for kind in [
        "Start",
        "PersonResume",
        "ClosePane",
        "QuitApp",
        "Crash",
        "Archive",
        "Unarchive",
        "Delete",
        "Clear",
        "SpawnChild",
        "Reboot",
        "PidReuse",
        "DeleteDir",
        "RestoreDir",
        "Tick",
        "Permission",
        "Upgrade",
        "Uninstall",
        "HijackScheme",
        "ServerStop",
        "ServerStart",
        "Degrade",
        "TornRegistry",
        "Discover",
        "Open",
    ] {
        assert!(seen.contains(kind), "never generated: {kind}");
    }
}

// ===================================================== 5. determinism ===

/// The same seed and variables must produce byte-identical traces and the
/// same final world, run after run.
#[test]
fn same_seed_gives_identical_trace_and_world() {
    for seed in [3u64, 77, 1234, 999_999] {
        let bits = long_run_bits(seed);
        let vars = WorldVars::from_bits(bits);
        let events = generate(seed, LEN);
        let (r1, t1) = run_traced(seed, vars, &events);
        let (r2, t2) = run_traced(seed, vars, &events);
        assert_eq!(r1, r2, "seed {seed}");
        assert_eq!(t1, t2, "seed {seed}");
        let w1 = format!("{:?}", probe_run(seed, vars, &events).world);
        let w2 = format!("{:?}", probe_run(seed, vars, &events).world);
        assert_eq!(w1, w2, "seed {seed}: final worlds differ");
    }
}
