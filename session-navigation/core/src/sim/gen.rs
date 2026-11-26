//! Seeded event-sequence generator.
use super::agents::StartHost;
use super::events::{Channel, Event, OpenEvent};
use super::hosts::Hazard;
use super::labels::STABLE;
use super::rng::Rng;
use super::starts::Mux;
use super::world::LEGACY_APPS;
use crate::model::{App, Provider, TerminalKind, Tool};

const CHANNELS: [Channel; 9] = [
    Channel::ProcTable,
    Channel::ClaudeRegistry,
    Channel::ClaudeDesktop,
    Channel::CodexDb,
    Channel::OpenCodeDb,
    Channel::OpenCodeHung,
    Channel::Fs,
    Channel::Reservations,
    Channel::Environment,
];
const PROVIDERS: [Provider; 4] = [
    Provider::Claude,
    Provider::Codex,
    Provider::OpenCode,
    Provider::Grok,
];
const HAZARDS: [Hazard; 6] = [
    Hazard::RevokeApi,
    Hazard::RevokeAutomation,
    Hazard::RestartTerminal,
    Hazard::RestartTmux,
    Hazard::ReuseWindow,
    Hazard::DetachTmux,
];

fn one<T: Copy>(rng: &mut Rng, items: &[T], fallback: T) -> T {
    let index = usize::try_from(rng.below(items.len() as u64)).unwrap_or(0);
    items.get(index).copied().unwrap_or(fallback)
}

/// A stable pick: it follows the target's identity across shrinking.
fn pick(rng: &mut Rng) -> u32 {
    STABLE | u32::try_from(rng.below(1 << 16)).unwrap_or(0)
}

fn start(rng: &mut Rng) -> Event {
    let host = match rng.below(13) {
        8..=12 => StartHost::Terminal {
            provider: one(rng, &PROVIDERS, Provider::Grok),
            terminal: u8::try_from(rng.below(8)).unwrap_or(0),
            mux: match rng.below(10) {
                0..=5 => Mux::None,
                6 | 7 => Mux::Attached,
                _ => Mux::Detached,
            },
            wsl: rng.chance(30),
            with_id: rng.chance(50),
        },
        0 => StartHost::ClaudeDesktop,
        1 => StartHost::CodexDesktop,
        2 => StartHost::ClaudeGhostty,
        3 => StartHost::CodexGhostty,
        4 => StartHost::OpenCodeDesktop,
        5 | 6 => StartHost::OpenCodeTerminal {
            with_id: rng.chance(50),
        },
        _ => StartHost::Headless {
            provider: one(rng, &PROVIDERS, Provider::Claude),
            supervised: rng.chance(50),
        },
    };
    Event::Start {
        host,
        dir: u8::try_from(rng.below(4)).unwrap_or(0),
        tag: u32::try_from(rng.below(1 << 30)).unwrap_or(0),
    }
}

fn open(rng: &mut Rng) -> Event {
    // Drawn from a stream of its own (seeded by a peek at the next draw), so
    // every other draw stays where it was.
    let no_preference = Rng::new(rng.clone().next_u64() ^ 0x0DEF_A017).chance(40);
    // Likewise its own stream: adding the draw moved no other.
    let prefer_terminal = Rng::new(rng.clone().next_u64() ^ 0x7E2A_1AA1).chance(35);
    Event::Open(OpenEvent {
        pick: pick(rng),
        twice: rng.chance(20),
        take_over: rng.chance(15),
        default_terminal: rng.chance(10),
        confirm_gone: rng.chance(10),
        allow_import: rng.chance(15),
        choose_first: rng.chance(50),
        second_terminal: rng.chance(30),
        prefer_terminal,
        no_preference,
        answer_confirm: rng.chance(60),
    })
}

fn upgrade(rng: &mut Rng) -> Event {
    if rng.chance(50) {
        Event::Upgrade {
            // Only the first round's apps are version-gated.
            app: Some(one(rng, &LEGACY_APPS, App::Ghostty)),
            tool: None,
        }
    } else {
        Event::Upgrade {
            app: None,
            // Only the first round's CLIs are version-gated.
            tool: Some(one(
                rng,
                &[Tool::ClaudeCli, Tool::CodexCli, Tool::OpenCodeCli],
                Tool::ClaudeCli,
            )),
        }
    }
}

fn event(rng: &mut Rng) -> Event {
    let dir = u8::try_from(rng.below(4)).unwrap_or(0);
    let terminal = one(
        rng,
        &[TerminalKind::Ghostty, TerminalKind::AppleTerminal],
        TerminalKind::Ghostty,
    );
    match rng.below(108) {
        0..=17 => start(rng),
        18..=37 => open(rng),
        38..=42 => Event::ClosePane { pick: pick(rng) },
        43..=45 => Event::QuitApp {
            app: one(rng, &App::ALL, App::Ghostty),
        },
        46..=48 => Event::Crash { pick: pick(rng) },
        49..=51 => Event::Archive { pick: pick(rng) },
        52..=53 => Event::Unarchive { pick: pick(rng) },
        54..=55 => Event::Delete { pick: pick(rng) },
        56..=58 => Event::Clear { pick: pick(rng) },
        59..=60 => Event::SpawnChild { pick: pick(rng) },
        61 => Event::Reboot,
        62..=63 => Event::PidReuse,
        64 => Event::DeleteDir { dir },
        65 => Event::RestoreDir { dir },
        66..=73 => Event::Tick {
            seconds: u8::try_from(1 + rng.below(3)).unwrap_or(1),
        },
        74..=75 => Event::Permission {
            terminal,
            granted: rng.chance(50),
        },
        76 => upgrade(rng),
        77 if rng.chance(50) => Event::Uninstall {
            app: one(rng, &App::ALL, App::Ghostty),
        },
        77 => Event::UninstallTool {
            tool: one(rng, &Tool::ALL, Tool::ClaudeCli),
        },
        78 => Event::HijackScheme {
            codex: rng.chance(50),
        },
        79..=80 => Event::ServerStop {
            crash: rng.chance(50),
        },
        81 => Event::ServerStart,
        82..=84 => Event::Degrade {
            channel: one(rng, &CHANNELS, Channel::Fs),
            ok: rng.chance(50),
        },
        85 => Event::TornRegistry { pick: pick(rng) },
        86..=90 => Event::Discover,
        91 => Event::DismissUpdate { pick: pick(rng) },
        92..=95 => Event::PersonResume { pick: pick(rng) },
        96 | 97 => Event::TmuxDetach { pick: pick(rng) },
        98 | 99 => Event::TmuxAttach {
            pick: pick(rng),
            terminal: u8::try_from(rng.below(8)).unwrap_or(0),
        },
        100..=104 => Event::Race {
            hazard: one(rng, &HAZARDS, Hazard::RevokeApi),
        },
        105 => Event::SlowStart { on: rng.chance(60) },
        107 => upgrade(rng),
        106 => Event::HostApi {
            kitty: rng.chance(50),
            on: rng.chance(50),
        },
        _ => Event::PersonResume { pick: pick(rng) },
    }
}

/// `len` events derived only from `seed`.
pub fn generate(seed: u64, len: usize) -> Vec<Event> {
    let mut rng = Rng::new(seed ^ 0x5E55_1011_7E7A_u64);
    (0..len).map(|_| event(&mut rng)).collect()
}
