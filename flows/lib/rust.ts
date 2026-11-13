// The Rust half of flows/ci.flow.ts, apart so its tests can hold each
// `vouched` step to the command desktop-ci.yml runs.
import type { FleetStep } from "./fleet.ts";

// --no-fail-fast: without it cargo stops at the first red test binary, and
// the rest never run, so no single report could be complete.
/** The desktop core's tests; relay's CI tests the public core it pins. */
export const CARGO = "bash scripts/test-platform-cargo.sh --manifest-path desktop-core/Cargo.toml --locked --no-fail-fast";
/** The app crate's tests that need no GTK; `linux_package` needs GNU install, so a Mac skips it. */
export const APP_CARGO = "bash scripts/test-platform-cargo.sh --manifest-path gtk/Cargo.toml --locked --no-fail-fast";
const RATCHET =
  '{ [ "$(rust-oleum --version 2>/dev/null)" = "rust-oleum 0.4.0" ] || cargo install rust-oleum --version 0.4.0 --locked --force --quiet; } && (cd gtk && rust-oleum) && (cd desktop-core && rust-oleum)';
const CONTRACT = "macos/Sources/RelayDesktopCore/CoreContract.swift";

/** The Desktop CI job that runs each vouched step's command on the same operating system. */
export const VOUCHING_JOB = { linux: "probe", macos: "macos-core-tests" } as const;

/**
 * The Rust steps Desktop CI runs on this machine's operating system, and only
 * those: GitHub runs the desktop core's whole `cargo test` on Linux and its
 * library tests on macOS. Each local step is GitHub's own command for this
 * OS, so a pass on GitHub can vouch for a step that failed here only under
 * load (lib/changes.ts underLocalLoad). The public core's own suite, live Pi
 * harness included, runs in relay's CI.
 */
export function rustSteps(lanes: Record<string, boolean>, linux: boolean, _dir: string): FleetStep[] {
  if (!lanes.probe && !lanes.macos_core && !lanes.linux) return [];
  // Compile once first, so the test steps measure tests rather than the build.
  const steps: FleetStep[] = [{ name: "build tests", command: `${CARGO} --no-run` }];
  if (lanes.probe && linux) steps.push({ name: "desktop core tests", command: CARGO, cargoTests: true, vouched: true });
  if (lanes.macos_core && !linux) {
    steps.push({ name: "desktop core tests", command: `${CARGO} --lib`, cargoTests: true, vouched: true });
  }
  if (lanes.probe || lanes.linux) {
    steps.push(
      linux
        ? { name: "app tests", command: APP_CARGO, cargoTests: true, vouched: Boolean(lanes.probe) }
        : { name: "app tests", command: `${APP_CARGO} --lib --test linux_app_hosting`, cargoTests: true },
      { name: "lock parity", command: "python3 scripts/check-lock-parity.py" },
      { name: "code-quality ratchet", command: RATCHET },
    );
  }
  if (lanes.probe) {
    steps.push({
      name: "generated contract",
      // What scripts/generate-contract.sh writes, compared without writing the tracked file.
      command:
        `generated="$(mktemp)" && ` +
        `cargo run --quiet --locked --manifest-path desktop-core/Cargo.toml --bin agent-relay-desktop-contract -- "$generated" && ` +
        `if cmp -s "$generated" ${CONTRACT}; then echo contract up to date; ` +
        `else diff -u ${CONTRACT} "$generated" | head -60; echo 'CoreContract.swift is stale: run scripts/generate-contract.sh'; exit 1; fi`,
    });
  }
  return steps;
}
