/**
 * Delegated approvals: release.sh beta|stable --approval-message <id> promotes
 * only on a permit from /opt/agent-host/delegation-decide.py for exactly that
 * channel, run and profile. The real release.sh runs with the validator,
 * cloud-release.sh and tick.sh paths replaced by fakes, and gh, git and sleep
 * faked by PATH shims that log every call, so nothing is dispatched and
 * "no dispatch" is read from the call log.
 */
import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const source = (name: string) => readFileSync(new URL(`../${name}`, import.meta.url), "utf8");
function executable(path: string, script: string) { writeFileSync(path, `#!/bin/bash\n${script}`, { mode: 0o755 }); }

const RUN = 40000000001;
const MSG = "250000000000000001";
const SHA = "a".repeat(40);

interface Outcome { status: number | null; stdout: string; stderr: string; calls: string[]; decideArgv: string[] | null; before: Array<Record<string, unknown>>; rows: Array<Record<string, unknown>>; bridge: string | null }

interface Options {
  args: string[];
  decideOut?: string;
  decideExit?: number;
  /** false: no validator at the path release.sh looks at. */
  validator?: boolean;
  orchestrator?: string;
  rows?: Array<Record<string, unknown>>;
}

function release(opts: Options): Outcome {
  const root = mkdtempSync(join(tmpdir(), "delegation-"));
  try {
    const bin = join(root, "bin"); mkdirSync(bin);
    mkdirSync(join(root, "build/relay-app"), { recursive: true });
    mkdirSync(join(root, "state"));
    const calls = join(root, "calls.log");
    const rows = opts.rows ?? [{ channel: "stable", sha: null, run_id: RUN, dispatched_at: "2026-11-19T00:00:00Z", status: "ready", version: null, auto: false, profile: null }];
    const state = join(root, "state/candidates.json");
    writeFileSync(state, JSON.stringify(rows));
    executable(join(bin, "gh"), `printf 'gh %s\\n' "$*" >> "$CALLS"
case "$1 $2" in
  "api repos/AgentWorkforce/relay-app/actions/runs/${RUN}") echo '{"conclusion":"success","event":"workflow_dispatch","head_sha":"${SHA}"}' ;;
  "api repos/AgentWorkforce/relay-app/actions/runs/${RUN}/jobs") echo 555 ;;
  "api --allow-escape-sequences") printf 'Releasing v2026.11.99\\nSource ${SHA}\\n' ;;
  "run list") echo 777 ;;
  "run view") echo "completed success" ;;
esac
exit 0
`);
    executable(join(bin, "git"), `printf 'git %s\\n' "$*" >> "$CALLS"
case "$1" in
  rev-parse) exit 1 ;;
  rev-list) echo 3 ;;
esac
exit 0
`);
    executable(join(bin, "sleep"), "exit 0\n");
    const decide = join(root, "delegation-decide.py");
    if (opts.validator !== false) {
      executable(decide, `printf 'decide\\n' >> "$CALLS"
printf '%s\\0' "$@" > "$DECIDE_ARGV"
printf '%s' "$DECIDE_OUT"
exit "$DECIDE_EXIT"
`);
    }
    const bridge = join(root, "cloud-release.sh");
    executable(bridge, `printf '%s\\0' "$@" > "$BRIDGE_ARGV"\n`);
    const tick = join(root, "tick.sh");
    executable(tick, `printf 'tick %s\\n' "$*" >> "$CALLS"\n`);
    const script = source("release.sh")
      .replaceAll("/opt/agent-host/delegation-decide.py", decide)
      .replaceAll("/opt/agent-host/cloud-release.sh", bridge)
      .replaceAll("/opt/agent-host/tick.sh", tick);
    assert.ok(!script.includes("/opt/agent-host/"), "every /opt/agent-host path is faked");
    const file = join(root, "release.sh");
    writeFileSync(file, script, { mode: 0o755 });
    const env: NodeJS.ProcessEnv = {
      ...process.env, HOME: root, PATH: `${bin}:${process.env.PATH}`, CALLS: calls,
      DECIDE_ARGV: join(root, "decide.argv"), BRIDGE_ARGV: join(root, "bridge.argv"),
      DECIDE_OUT: opts.decideOut ?? "", DECIDE_EXIT: String(opts.decideExit ?? 0),
    };
    delete env.DESKTOP_RELEASE_ORCHESTRATOR;
    if (opts.orchestrator !== undefined) env.DESKTOP_RELEASE_ORCHESTRATOR = opts.orchestrator;
    const result = spawnSync("bash", [file, ...opts.args], { env, encoding: "utf8" });
    const argv = (path: string) => existsSync(path) ? readFileSync(path, "utf8").split("\0").slice(0, -1) : null;
    return {
      status: result.status, stdout: result.stdout, stderr: result.stderr,
      calls: existsSync(calls) ? readFileSync(calls, "utf8").split("\n").filter(Boolean) : [],
      decideArgv: argv(join(root, "decide.argv")),
      before: rows,
      rows: JSON.parse(readFileSync(state, "utf8")) as Array<Record<string, unknown>>,
      bridge: existsSync(join(root, "bridge.argv")) ? argv(join(root, "bridge.argv"))!.join("|") : null,
    };
  } finally { rmSync(root, { recursive: true, force: true }); }
}

const permit = (over: Record<string, unknown> = {}) => JSON.stringify({
  decision: "permit", action: "release-stable", target: `stable run ${RUN} profile -`, agent: "fixture-agent",
  agentId: "240000000000000001", messageId: MSG, sentAt: "2026-11-19T07:44:48Z", expiresAt: "2026-12-19T00:00:00Z",
  rule: "allowlist", subject: null, auditSeq: 17, ...over,
});
const delegated = ["stable", "--run", String(RUN), "--approval-message", MSG];
const dispatched = (o: Outcome) => o.calls.some((c) => c.startsWith("gh workflow run"));
const tagged = (o: Outcome) => o.calls.some((c) => /^git (tag|push)/.test(c));

/** A refusal: exit 4, nothing dispatched, tagged or written. */
function refused(o: Outcome, label: string) {
  assert.equal(o.status, 4, `${label}: ${o.stderr}`);
  assert.ok(!dispatched(o), `${label}: no dispatch (${o.calls.join("; ")})`);
  assert.ok(!tagged(o), `${label}: no tag`);
  assert.deepEqual(o.rows, o.before, `${label}: the rows are untouched`);
  assert.match(o.stderr, /refusing/, label);
}

test("a permit for the candidate's own profile passes the gate and the promote is dispatched", () => {
  const rows = [{ channel: "stable", sha: null, run_id: RUN, dispatched_at: "2026-11-19T00:00:00Z", status: "ready", version: null, auto: false, profile: "p1x2" }];
  const target = `stable run ${RUN} profile p1x2`;
  const o = release({ args: delegated, rows, decideOut: permit({ target }) + "\n" });
  assert.equal(o.status, 0, o.stderr);
  assert.deepEqual(o.decideArgv, ["--action", "release-stable", "--target", target, "--message-id", MSG, "--repo", "AgentWorkforce/relay-app"]);
  assert.match(o.stdout, new RegExp(`delegated approval: permit for fixture-agent, message ${MSG}, audit seq 17`));
  const decideAt = o.calls.indexOf("decide");
  const promoteAt = o.calls.findIndex((c) => c.startsWith("gh workflow run desktop-promote.yml"));
  assert.ok(decideAt >= 0 && promoteAt > decideAt, o.calls.join("; "));
  assert.match(o.calls[promoteAt] ?? "", /-f candidate-run-id=40000000001 -f version=2026\.11\.99 -f channel=stable -f profile=p1x2$/);
  assert.equal(o.rows[0]?.status, "published");
});

for (const code of [10, 11, 2]) {
  test(`the validator exiting ${code} refuses with exit 4 and its reason`, () => {
    const o = release({ args: delegated, decideExit: code, decideOut: JSON.stringify({ decision: "deny", reason: "sender_not_allowlisted", detail: "no grant", messageId: MSG }) });
    refused(o, `exit ${code}`);
    assert.match(o.stderr, new RegExp(`exited ${code} .*sender_not_allowlisted: no grant`));
  });
}

test("exit 0 with output that is not one JSON object refuses", () => {
  for (const out of ["", "not json", "{\"decision\":\"permit\"", permit() + permit(), "[]"]) refused(release({ args: delegated, decideOut: out }), JSON.stringify(out));
});

test("exit 0 with decision deny refuses", () => {
  refused(release({ args: delegated, decideOut: JSON.stringify({ decision: "deny", reason: "stale", detail: "too old", messageId: MSG }) }), "deny");
});

test("a permit for another message, action or target refuses", () => {
  refused(release({ args: delegated, decideOut: permit({ messageId: "250000000000000002" }) }), "messageId");
  refused(release({ args: delegated, decideOut: permit({ action: "release-beta" }) }), "action");
  refused(release({ args: delegated, decideOut: permit({ target: `stable run ${RUN} profile other` }) }), "target");
});

test("a missing validator refuses with exit 4", () => {
  const o = release({ args: delegated, validator: false });
  refused(o, "missing");
  assert.match(o.stderr, /not installed/);
});

test("--approval-message without --run is a usage error before any call", () => {
  const o = release({ args: ["stable", "--approval-message", MSG], decideOut: permit() });
  assert.equal(o.status, 2, o.stderr);
  assert.match(o.stderr, /needs --run/);
  assert.equal(o.decideArgv, null);
  assert.ok(!o.calls.some((c) => c.startsWith("gh") || c.startsWith("tick")), o.calls.join("; "));
});

test("--approval-message with prepare is a usage error and nothing is dispatched", () => {
  const o = release({ args: ["prepare", "stable", "main", "--approval-message", MSG], decideOut: permit() });
  assert.equal(o.status, 2, o.stderr);
  assert.equal(o.decideArgv, null);
  assert.ok(!dispatched(o), o.calls.join("; "));
});

test("a malformed message id is a usage error", () => {
  for (const id of ["123", "0250000000000000001", "25000000000000000a", "250000000000000000000"]) {
    const o = release({ args: ["stable", "--run", String(RUN), "--approval-message", id], decideOut: permit() });
    assert.equal(o.status, 2, `${id}: ${o.stderr}`);
    assert.equal(o.decideArgv, null, id);
    assert.ok(!dispatched(o), id);
  }
});

test("cloud mode refuses --approval-message with exit 4 and never execs cloud-release.sh", () => {
  for (const args of [delegated, ["stable", "--run", String(RUN), `--approval-message=${MSG}`]]) {
    const o = release({ args, orchestrator: "cloud", decideOut: permit() });
    assert.equal(o.status, 4, o.stderr);
    assert.match(o.stderr, /legacy-orchestrator only/);
    assert.equal(o.bridge, null);
    assert.equal(o.decideArgv, null);
    assert.deepEqual(o.calls, []);
  }
});

test("cloud mode without the flag execs cloud-release.sh with the arguments unchanged", () => {
  const o = release({ args: ["stable", "--run", String(RUN), "--profile", "p1x2"], orchestrator: "cloud" });
  assert.equal(o.status, 0, o.stderr);
  assert.equal(o.bridge, `stable|--run|${RUN}|--profile|p1x2`);
  assert.deepEqual(o.calls, []);
});

test("without the flag the validator is never called and the promote goes ahead", () => {
  const o = release({ args: ["stable", "--run", String(RUN)], decideOut: "{\"decision\":\"deny\"}", decideExit: 10 });
  assert.equal(o.status, 0, o.stderr);
  assert.equal(o.decideArgv, null);
  assert.ok(!o.calls.includes("decide"));
  assert.ok(o.calls.some((c) => c.startsWith("gh workflow run desktop-promote.yml")), o.calls.join("; "));
  assert.doesNotMatch(o.stdout, /delegated approval/);
});

test("delegations.json is a lane file", async () => {
  assert.match(source("agent.config.ts"), /^  laneFiles: \[[^\]\n]*"delegations\.json"[^\]\n]*\],$/m);
  // Imported here, so the release.sh cases above run without the kit package.
  const { default: agent } = await import("../agent.config.ts");
  // The kit turns laneFiles into agent.files entries, as test/agent.test.ts reads them.
  assert.ok(agent.files.some((f) => f.name === "delegations.json" && f.source !== "kit"));
});

// The grants are not in the repository: Agent Deploy writes delegations.json
// from the agent-deploy environment's secret (test/delegations-from-env.test.ts
// checks the generator). What stays pinned here is what the secret cannot
// change: the version and who the owner is.
test("the generated delegation list is version 2 with the owner, and empty without the secret", async () => {
  const { delegationsFromEnv, render } = await import("../delegations-from-env.ts");
  assert.equal(render(delegationsFromEnv(undefined)), `${JSON.stringify({ version: 2, owners: ["khaliq-gant"], delegations: [] }, null, 2)}\n`);
});

// The secret can change between the preview and the deploy: the deploy job
// deploys only the list the preview wrote, compared by sha256 in step
// conditions so that no digest is printed.
test("Agent Deploy's deploy job stops when its delegation list differs from the preview's", () => {
  const workflow = readFileSync(new URL("../../.github/workflows/agent-deploy.yml", import.meta.url), "utf8");
  const job = (name: string) => {
    const start = workflow.indexOf(`\n  ${name}:\n`);
    assert.ok(start >= 0, name);
    const next = workflow.slice(start + 1).search(/\n  [a-z][a-z-]*:\n/);
    return next < 0 ? workflow.slice(start) : workflow.slice(start, start + 1 + next);
  };
  const hash = [
    "      - name: Hash the delegation list",
    "        id: delegations",
    "        run: |",
    "          set -euo pipefail",
    "          sum=$(sha256sum agent/delegations.json)",
    "          echo \"sha256=${sum%% *}\" >> \"$GITHUB_OUTPUT\"",
  ].join("\n");
  const write = "      - name: Write the delegation list\n";
  const preview = job("preview");
  const deploy = job("deploy");
  for (const [name, text] of [["preview", preview], ["deploy", deploy]] as const) {
    assert.equal(text.split(hash).length, 2, `${name} hashes the list once`);
    assert.ok(text.indexOf(write) >= 0 && text.indexOf(write) < text.indexOf(hash), `${name}: after the list is written`);
  }
  assert.match(preview, /^    outputs:\n      fresh: \$\{\{ steps\.preview\.outputs\.fresh \}\}\n(?: +#.*\n)*      delegations-sha256: \$\{\{ steps\.delegations\.outputs\.sha256 \}\}\n/m);
  // The deploy runs only after a successful preview.
  assert.match(deploy, /^    needs: \[changed, preview\]$/m);
  assert.match(deploy, /^      && needs\.preview\.result == 'success'$/m);
  const none = [
    "      - name: Stop if the preview recorded no delegation list digest",
    "        if: needs.preview.outputs.delegations-sha256 == ''",
    "        run: |",
    "          echo \"::error::The preview recorded no digest of the delegation list: run the deploy again.\"",
    "          exit 1",
  ].join("\n");
  const changed = [
    "      - name: Stop if the delegation list changed after the preview",
    "        if: needs.preview.outputs.delegations-sha256 != steps.delegations.outputs.sha256",
    "        run: |",
    "          echo \"::error::The delegation list changed after the preview: run the deploy again.\"",
    "          exit 1",
  ].join("\n");
  for (const step of [none, changed]) {
    assert.equal(deploy.split(step).length, 2, step.split("\n")[0]);
    assert.ok(deploy.indexOf(hash) < deploy.indexOf(step), "after the deploy's own hash");
    assert.ok(deploy.indexOf(step) < deploy.indexOf("      - name: Deploy\n"), "before the kit deploys");
  }
  // Neither digest reaches a step's env or script, where the log would print it.
  assert.equal(workflow.split("outputs.delegations-sha256").length, 3);
  assert.equal(workflow.split("steps.delegations.outputs.sha256").length, 3);
});

const row = (over: Record<string, unknown> = {}) => ({ channel: "stable", sha: SHA, run_id: RUN, dispatched_at: "2026-11-19T00:00:00Z", status: "ready", version: "2026.11.99", auto: false, profile: null, ...over });
const deny = JSON.stringify({ decision: "deny", reason: "already_spent", detail: "used", messageId: MSG });

test("a refusal for a run not in candidates.json writes no row", () => {
  const o = release({ args: delegated, rows: [], decideExit: 10, decideOut: deny });
  refused(o, "no row");
  assert.deepEqual(o.rows, []);
});

test("a permitted promote records the message id on the row", () => {
  const o = release({ args: delegated, rows: [row()], decideOut: permit() });
  assert.equal(o.status, 0, o.stderr);
  assert.equal(o.rows[0]?.approval_message, MSG);
  assert.equal(o.rows[0]?.status, "published");
});

for (const status of ["promoting", "tagging"]) {
  test(`a ${status} row with the same message id resumes without the validator`, () => {
    const o = release({ args: delegated, rows: [row({ status, promote_since: "2026-11-19T01:00:00Z", approval_message: MSG })], decideExit: 10, decideOut: deny });
    assert.equal(o.status, 0, o.stderr);
    assert.equal(o.decideArgv, null);
    assert.ok(!o.calls.includes("decide"), o.calls.join("; "));
    assert.match(o.stdout, new RegExp(`resuming run ${RUN} under the permit already spent for message ${MSG}`));
    assert.match(o.stdout, /resuming promote run 777/);
    assert.ok(!dispatched(o), o.calls.join("; "));
    assert.ok(o.calls.some((c) => c.startsWith("git tag -a released-v2026.11.99")), o.calls.join("; "));
    assert.equal(o.rows[0]?.status, "published");
    assert.equal(o.rows[0]?.approval_message, MSG);
  });
}

test("a promoting row with another message id asks the validator and is refused on a deny", () => {
  const o = release({ args: delegated, rows: [row({ status: "promoting", promote_since: "2026-11-19T01:00:00Z", approval_message: "250000000000000002" })], decideExit: 10, decideOut: deny });
  refused(o, "other id");
  assert.ok(o.calls.includes("decide"));
  assert.equal(o.decideArgv?.[5], MSG);
});

test("a promoting row of the other channel with the same message id asks the validator", () => {
  const o = release({ args: delegated, rows: [row({ channel: "beta", status: "promoting", promote_since: "2026-11-19T01:00:00Z", approval_message: MSG })], decideExit: 10, decideOut: deny });
  refused(o, "other channel");
  assert.ok(o.calls.includes("decide"));
});

test("a promoting row with no recorded message id asks the validator", () => {
  const o = release({ args: delegated, rows: [row({ status: "promoting", promote_since: "2026-11-19T01:00:00Z" })], decideExit: 10, decideOut: deny });
  refused(o, "no stored id");
  assert.ok(o.calls.includes("decide"));
});

test("a rerun without the flag resumes a delegated promote as before", () => {
  const o = release({ args: ["stable", "--run", String(RUN)], rows: [row({ status: "promoting", promote_since: "2026-11-19T01:00:00Z", approval_message: MSG })], decideExit: 10, decideOut: deny });
  assert.equal(o.status, 0, o.stderr);
  assert.equal(o.decideArgv, null);
  assert.match(o.stdout, /resuming promote run 777/);
  assert.ok(!dispatched(o), o.calls.join("; "));
});

test("--profile - with the flag is a usage error before the validator and nothing is dispatched", () => {
  const o = release({ args: [...delegated, "--profile", "-"], rows: [row()], decideOut: permit() });
  assert.equal(o.status, 2, o.stderr);
  assert.match(o.stderr, /lowercase letters and digits/);
  assert.equal(o.decideArgv, null);
  assert.ok(!dispatched(o), o.calls.join("; "));
  assert.deepEqual(o.rows, o.before);
});

test("a row profile that is not lowercase letters and digits refuses before the validator", () => {
  const o = release({ args: delegated, rows: [row({ profile: "-" })], decideOut: permit({ target: `stable run ${RUN} profile -` }) });
  refused(o, "row profile");
  assert.equal(o.decideArgv, null);
});

test("a delegated beta promote asks for release-beta and refuses a release-stable permit", () => {
  const beta = ["beta", "--run", String(RUN), "--approval-message", MSG];
  const target = `beta run ${RUN} profile -`;
  const o = release({ args: beta, rows: [row({ channel: "beta" })], decideOut: permit({ action: "release-beta", target }) });
  assert.equal(o.status, 0, o.stderr);
  assert.deepEqual(o.decideArgv, ["--action", "release-beta", "--target", target, "--message-id", MSG, "--repo", "AgentWorkforce/relay-app"]);
  assert.match(o.calls.find((c) => c.startsWith("gh workflow run desktop-promote.yml")) ?? "", /-f channel=beta$/);
  refused(release({ args: beta, rows: [row({ channel: "beta" })], decideOut: permit({ action: "release-stable", target }) }), "stable permit for beta");
});

test("the lead prompt keeps the permit-only sentences", () => {
  const prompt = source("lead-prompt.md");
  for (const phrase of ["only through a delegation-decide.py permit", "it counts only with a permit", "the script refuses without a permit", "Delegated stable is legacy-orchestrator only"]) {
    assert.ok(prompt.includes(phrase), phrase);
  }
});
