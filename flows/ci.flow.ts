// ci: run the CI this change needs, and only that, as fast as it can go.
//
//   flows/node_modules/.bin/flows run --local-agent flows/ci.flow.ts --input '{}'                every round
//   flows/node_modules/.bin/flows run --local-agent flows/ci.flow.ts --input '{"github":true}'   the final gate
//
// 1. Select lanes exactly as Desktop CI does: scripts/ci-changes.py over the
//    paths changed since the merge base with main (tracked edits included).
//    A docs-only change selects nothing and runs only the always-on checks.
// 2. Start every selected lane this machine can run at once
//    (flows/lib/fleet.mjs, one process per job). All cargo work shares one
//    target dir and one build lock, so it is one ordered job beside the
//    others instead of several racing for the lock. This takes minutes and is
//    what each review-and-fix round waits on.
// 3. With `github: true`, once, before the squash push: also run GitHub
//    Actions, the only place the Swift tests, GTK tests, Windows and Linux
//    packaging and the second architecture run. flows/lib/remote-ci.mjs
//    pushes a snapshot of the working tree to a throwaway ci/<sha> branch,
//    dispatches Desktop CI with the merge base (the lanes a pull request
//    would pick; this repo opens none) on every architecture, as main's own
//    run does, and Mobile CI when its paths changed, waits (about half an
//    hour for a change that selects every lane), and deletes the branch.
//
// Result: .relayflow/ci/report.md (under $RELAYFLOW_STATE_DIR/ci/ when set),
// a pass/fail line per step with the output tail and failed GitHub job logs
// for each red one (report.json holds every step's tail, and each step's full
// log paths). The run ends step_failed while any lane is red. Without
// `github`, a green run says the GitHub gate is still to come.
//
// With `github`, a local cargo step whose only failures are named tests is
// reported as failing under local load rather than red when GitHub ran the
// same command on this operating system and passed: the machine is shared
// with the review fleets, and a few timing-sensitive tests miss their
// deadlines there.
import { flow } from "@relayflows/surface";
import { GITHUB_STEP, lanesCommand, parseLanes, underLocalLoad, UNTRACKED, type Lanes } from "./lib/changes.ts";
import { runFleet, writeFile, type FleetJob } from "./lib/fleet.ts";
import { rustSteps } from "./lib/rust.ts";
import { resolveBase } from "./lib/review.ts";
import { shellWord } from "./lib/shell.ts";

// 2-minute polls (lib/fleet.ts says why they are short), 130 minutes in all: they cover GitHub's 125-minute step, inside the 150m budget.
export const POLL_MINUTES = 2;
export const POLLS = Math.ceil(130 / POLL_MINUTES);

interface CiInput {
  /** The branch the change lands on. Default `origin/main`. */
  base?: string;
  /** Also run GitHub Actions on a pushed snapshot: the final gate before the squash push. Default false. */
  github?: boolean;
}

function jobs(dir: string, lanes: Lanes, linux: boolean, xcode: boolean, flows: boolean, remote: string | undefined, desktop: boolean, mobile: boolean, agent: boolean): FleetJob[] {
  const all: FleetJob[] = [
    {
      name: "always",
      steps: [
        { name: "open source", command: "bash scripts/check-open-source.sh" },
        { name: "release notes tooling", command: "python3 scripts/test_release_notes.py && python3 scripts/test_release_version.py" },
      ],
    },
    { name: "rust", steps: rustSteps(lanes, linux, dir) },
  ];
  if (remote) {
    all.push({
      name: "github",
      steps: [{ name: GITHUB_STEP, command: `node flows/lib/remote-ci.mjs watch ${shellWord(remote)} ${desktop ? 1 : 0} ${mobile ? 1 : 0} ${agent ? 1 : 0}`, timeoutMinutes: 125 }],
    });
  }
  if (flows) {
    // These flows gate every change, so a change to them is checked too.
    all.push({
      name: "flows",
      steps: [{ name: "typecheck, flows check and helper tests", command: "cd flows && npm run -s check" }],
    });
  }
  if (lanes.shell) {
    all.push({
      name: "shell",
      steps: [
        {
          name: "shellcheck",
          // Desktop CI installs it; a machine without it leaves the lint to GitHub.
          command:
            "if command -v shellcheck >/dev/null; then shellcheck scripts/*.sh linux/agent-relay.postinst agent/*.sh && shellcheck -x scripts/dev-reload.sh; " +
            "else echo 'shellcheck not installed: left to GitHub'; fi",
        },
        { name: "probe bridge validation", command: "bash scripts/test-probe-bridge.sh" },
        { name: "CI change detection", command: "python3 scripts/test_ci_changes.py" },
      ],
    });
  }
  if (lanes.probe) {
    all.push({
      name: "probe scripts",
      steps: [
        { name: "cargo target selection", command: "python3 scripts/test_cargo_target_selection.py" },
        { name: "cache pruning", command: "node --test .github/actions/windows-build-cache-prune/main.test.cjs" },
        { name: "mobile bridge", command: "node --test scripts/mobile-build-inventory.test.mjs" },
        { name: "site origin", command: "bash scripts/test-site-origin.sh" },
        { name: "build scripts", command: "bash scripts/test-build-scripts.sh" },
        { name: "crate overrides", command: "bash scripts/test-crate-overrides.sh" },
        { name: "build-app options", command: "bash scripts/test-build-app-options.sh" },
        { name: "development reload", command: "python3 scripts/test_dev_reload.py" },
      ],
    });
  }
  if (lanes.macos && xcode) {
    // The Swift tests are GitHub's (XCTest may be missing here); a build
    // still catches a compile error minutes before the runner does. Command
    // Line Tools alone cannot load the SwiftUI macros, so without Xcode the
    // build is left to GitHub too rather than reported red.
    all.push({
      name: "macos",
      steps: [
        { name: "swift build", command: "swift build --package-path macos" },
        { name: "session overview build", command: "swift build --package-path shared/RelaySessionOverview" },
      ],
    });
  }
  return all.filter((job) => job.steps.length > 0);
}

export default flow<CiInput>(
  "relay-desktop.ci",
  { budget: { wallclock: "150m" } },
  async (f, input) => {
    const base = input?.base ?? "origin/main";
    const dir = (await f.run("node flows/lib/state-dir.mjs ci")).trim();
    await f.run(`rm -rf ${shellWord(dir)} && mkdir -p ${shellWord(dir)}`);
    // The same fetch, lease handling and explanation as the review flows.
    const problem = await resolveBase(f, base);
    if (problem) throw new Error(problem);
    const mergeBase = (await f.run(`git merge-base ${shellWord(base)} HEAD`)).trim();
    const lanes = parseLanes(await f.run(lanesCommand(mergeBase), { timeout: "2m" }));
    const names = Object.keys(lanes);
    const any = names.some((lane) => lanes[lane]);
    const changed = (await f.run(`git diff --no-renames --name-only ${shellWord(mergeBase)}`)).split("\n").filter(Boolean);
    // New files reach the selector and GitHub only once staged. A change with
    // unstaged new files is never reported CI-green.
    const untracked = (await f.run(UNTRACKED)).split("\n").filter(Boolean);
    const mobile = (await f.run(`node flows/lib/remote-ci.mjs mobile ${shellWord(mergeBase)}`)).trim() === "1";
    const agentDispatch = (await f.run(`node flows/lib/remote-ci.mjs agent ${shellWord(mergeBase)}`)).trim();
    const agent = agentDispatch === "1";
    const flowsChanged = [...changed, ...untracked].some((path) => path.startsWith("flows/"));
    const linux = (await f.run("uname -s")).trim() === "Linux";
    const xcode = !!lanes.macos && (await f.run("xcode-select -p", { onNonZero: "record" })).stdout.includes(".app/");
    // GitHub runs when a desktop lane, Mobile CI's or Agent CI's paths
    // changed; remote-ci dispatches only the workflows whose paths changed.
    // A tree with unstaged new files is never CI-green, so GitHub is not asked.
    const remote = input?.github === true && (any || mobile || agent) && !untracked.length ? mergeBase : undefined;

    const results = underLocalLoad(await runFleet(f, `${dir}/fleet`, jobs(dir, lanes, linux, xcode, flowsChanged, remote, any, mobile, agent), POLLS, POLL_MINUTES));
    const red = results.filter((r) => !r.ok);

    const selection = [...names.filter((lane) => lanes[lane]), ...(mobile ? ["Mobile CI"] : []), ...(agent ? ["Agent CI"] : [])].join(", ") || "none (always-on checks only)";
    const flaky = results.filter((r) => r.ok && r.step.includes("only under local load"));
    // A lane only GitHub can run is not green until GitHub ran it.
    const localOnly = input?.github !== true && (any || mobile || agent);
    const report = [
      `# CI: ${red.length ? `${red.length} red` : untracked.length ? "NOT CI-green: untracked files were not tested" : localOnly ? "local checks green; GitHub gate still to come" : flaky.length ? `green, with ${flaky.length} step(s) failing only under local load` : "green"}`,
      "",
      `Lanes selected: ${selection}`,
      localOnly
        ? `GitHub Actions: not run; before the squash push, rerun with --input '{"github":true}'`
        : remote ? "" : untracked.length ? "GitHub Actions: skipped until the new files are staged" : "GitHub Actions: not needed",
      lanes.macos && !xcode ? "Swift build: left to GitHub (no Xcode on this machine)" : "",
      lanes.probe && !linux ? "Full cargo test and live Pi sessions: left to GitHub (Desktop CI runs them on Linux)" : "",
      lanes.probe && linux ? "Live Pi sessions: left to GitHub" : "",
      agentDispatch === "main" ? "Agent CI (the agent folder's stack tests): left to main's push after landing; agent-ci.yml is not on main yet, so GitHub cannot dispatch it" : "",
      untracked.length ? `Not covered: ${untracked.length} untracked file(s); stage the ones that belong to the change and list machine-local ones in .git/info/exclude (first: ${untracked.slice(0, 5).join(", ")})` : "",
      "",
      ...results.map((r) => `- ${r.ok ? "PASS" : "FAIL"} ${r.job}: ${r.step}`),
      ...red.flatMap((r) => ["", `## ${r.job}: ${r.step}`, "", ...(r.log ? [`Full log: ${r.log} (stderr: ${r.errLog})`, ""] : []), "```", r.output, "```"]),
    ].join("\n");
    await f.run(`${writeFile(`${dir}/report.md`, report)} && ${writeFile(`${dir}/report.json`, JSON.stringify({ lanes, results }, null, 2))}`);

    if (red.length) {
      return f.done("step_failed", { detail: `${red.map((r) => `${r.job}: ${r.step}`).join("; ")}; see ${dir}/report.md` });
    }
    if (untracked.length) {
      return f.done("needs_human", { detail: `${untracked.length} untracked file(s) were not tested; stage the ones that belong to the change and rerun` });
    }
    if (localOnly) {
      // Green for a fix round; the squash push still waits on the GitHub gate.
      return f.done("success", { detail: `local checks green (lanes: ${selection}); GitHub gate still to come: rerun with {"github":true} before pushing` });
    }
    f.done("success", { detail: `${results.length} check(s) green; lanes: ${selection}` });
  },
);
