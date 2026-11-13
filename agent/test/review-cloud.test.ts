/**
 * review.sh's review round against a fake `flows` CLI: the round is planned
 * locally, reviewed only as a Cloud flow run, and reported in the sized
 * review's place, with local CI beside it.
 *
 * Structure: review.sh keeps the Cloud round in functions defined before its
 * sourced-return line (parse_args, cloud_credentials, cloud_round and the
 * cloud_* steps), so a test sources the script inside a throwaway git repo
 * and calls them on a prepared checkout, without the clone, fetch and npm
 * install the main body does. The repo carries an executable fake at
 * flows/node_modules/.bin/flows, the path review.sh calls, which records
 * every invocation (argv, and whether the Cloud token was in its environment,
 * or the inherited one)
 * and answers from a scenario directory: plan.json for plan-review, a report
 * for ci.flow.ts (red when the ci-red marker is there, or when the tree has
 * cloud-review/ or anything `git status` shows), run.<n>, status.<n>,
 * dry.json and sync.json for the Cloud calls (the last numbered answer
 * repeats; the dry run's answer defaults to sync.json's file list with a
 * patch naming those files). A sync without --dry-run writes every file it
 * lists into --dir, as the run would bring it back. Secrets come from a fake
 * secrets file through REVIEW_SECRETS_FILE; CLOUD_POLL_SECONDS is 0, and
 * TMPDIR is the fixture's own, to see the scratch directory go. flows/lib is
 * a git-ignored copy of the real one (with a link to its yaml package), so the
 * context-review state is saved by the real review-state.mjs. The no-token case runs the whole script, to show
 * it refuses before it clones. Needs bash, git and jq. Nothing is written
 * inside the repository.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../review.sh", import.meta.url));
const TOKEN = "fake-cloud-token-value-0123456789";

const FAKE_FLOWS = `#!/bin/bash
# The fake flows CLI: records each call and answers from $FAKE.
printf '%s\\n' "$*" >> "$FAKE/calls"
case "\${FLOWS_CLOUD_TOKEN:-}" in "") token= ;; inherited) token=inherited ;; *) token=set ;; esac
printf '%s|token=%s url=%s anthropic=%s typesafe=%s\\n' "$1 $2 $3" "$token" "\${FLOWS_CLOUD_URL:-}" "\${ANTHROPIC_API_KEY:+set}" "\${TYPESAFE_API_KEY:+set}" >> "$FAKE/env"
next() {
  local n
  n=$(( $(cat "$FAKE/$1.n" 2>/dev/null || echo 0) + 1 ))
  echo "$n" > "$FAKE/$1.n"
  if [ -f "$FAKE/$1.$n" ]; then cat "$FAKE/$1.$n"; else cat "$FAKE/$1.last"; fi
}
case "$1 $2" in
  "run --local-agent")
    case "$3" in
      flows/plan-review.flow.ts) mkdir -p "$RELAYFLOW_STATE_DIR/plan" && cp "$FAKE/plan.json" "$RELAYFLOW_STATE_DIR/plan/plan.json" ;;
      flows/ci.flow.ts)
        mkdir -p "$RELAYFLOW_STATE_DIR/ci"
        if [ -f "$FAKE/ci-red" ] || [ -e cloud-review ] || [ -n "$(git status --porcelain)" ]; then
          echo "# CI: red" > "$RELAYFLOW_STATE_DIR/ci/report.md"
          exit 1
        fi
        echo "# CI: green" > "$RELAYFLOW_STATE_DIR/ci/report.md" ;;
    esac ;;
  "run --cloud")
    ls cloud-review/input > "$FAKE/uploaded"
    [ ! -f cloud-review/input/intent.md ] || cp cloud-review/input/intent.md "$FAKE/intent.md"
    [ ! -f cloud-review/input/manifest ] || cp cloud-review/input/manifest "$FAKE/manifest"
    next run ;;
  "status --cloud") next status ;;
  "sync --json")
    if [ "$3" = --dry-run ]; then
      if [ -f "$FAKE/dry.json" ]; then cat "$FAKE/dry.json"
      else jq -c 'if .hasChanges == true then . + {applied: false, dryRun: true, patch: ((.files // []) | map("diff --git a/\\(.) b/\\(.)\\n--- a/\\(.)\\n+++ b/\\(.)") | join("\\n"))} else . end' "$FAKE/sync.json"; fi
      exit 0
    fi
    [ "$3" = --dir ] || { echo '{"ok":false,"code":"usage","message":"no --dir"}'; exit 2; }
    dir=$4
    if jq -e '.hasChanges == true' "$FAKE/sync.json" >/dev/null; then
      [ -f "$dir/cloud-review/input/change.patch" ] || { echo '{"ok":false,"code":"patch_conflict","message":"baseline missing"}'; exit 2; }
      rm -f "$dir/cloud-review/input/intent.md"
      if [ -f "$FAKE/summary.json" ]; then
        for f in $(jq -r '.flows | keys[]' "$FAKE/summary.json"); do
          mkdir -p "$dir/cloud-review/$f" && echo "# $f report" > "$dir/cloud-review/$f/report.md"
          if [ -f "$FAKE/report-$f.json" ]; then cp "$FAKE/report-$f.json" "$dir/cloud-review/$f/report.json"; else echo '{"findings":[]}' > "$dir/cloud-review/$f/report.json"; fi
        done
        cp "$FAKE/summary.json" "$dir/cloud-review/summary.json"
      fi
      for f in $(jq -r '(.files // [])[]' "$FAKE/sync.json"); do
        [ -e "$dir/$f" ] || { mkdir -p "$(dirname "$dir/$f")" && echo "planted by the run" > "$dir/$f"; }
      done
    fi
    cat "$FAKE/sync.json" ;;
esac
`;

const STANDARD_PLAN = {
  depth: "standard", flows: ["adversarial-review", "context-review"], model: "claude-opus-5-5", full: false,
  lanes: { agent: true, linux: true, windows: false }, windows: false, origin: "Jev, confidence 0.90", source: "jev",
  change: { fileCount: 1, lines: 1, commitCount: 1, mergeBase: "MB", snapshot: "SNAPSHOT", redacted: 0, redactedSubjects: 0 },
};
const DEEP_PLAN = { ...STANDARD_PLAN, depth: "deep", full: true };
const SMALL_PLAN = { ...STANDARD_PLAN, depth: "small", flows: ["adversarial-review"], model: "claude-sonnet-5-5", origin: "given in the input; the floors were not applied", source: "override" };
const SUBMITTED = { ok: true, runId: "run-1", status: "queued", specHash: "h", apiUrl: "https://cloud.example/runs/run-1" };
const RUNNING = { v: 1, ok: true, run: { run_id: "run-1", status: "running", completion_reason: null } };
const COMPLETED = { v: 1, ok: true, run: { run_id: "run-1", status: "completed", completion_reason: "success" } };
const summaryOf = (flows: string[], extra: Record<string, unknown> = {}) => ({
  depth: "standard", lead: "claude", lenses: [],
  flows: Object.fromEntries(flows.map((f) => [f, { blocking: 0, total: 2, report: `cloud-review/${f}/report.md` }])),
  lensesUnavailable: [], ...extra,
});
const SYNCED = { ok: true, hasChanges: true, applied: true, files: ["cloud-review/summary.json", "cloud-review/adversarial-review/report.md"], excluded: [] };
const FAILED = (reason: string) => ({ v: 1, ok: true, run: { run_id: "run-1", status: "failed", completion_reason: reason } });

interface Scenario {
  plan?: unknown;
  run?: unknown[];
  status?: unknown[];
  dry?: unknown;
  sync?: unknown;
  /** null: the run brings back no summary.json. */
  summary?: unknown;
  /** summary.json's text as the run brings it back, written as it is. */
  summaryText?: string;
  /** report.json per flow, as the run brings it back. */
  reports?: Record<string, unknown>;
  ciRed?: boolean;
  secrets?: string;
}

const realLib = fileURLToPath(new URL("../../flows/lib", import.meta.url));

const tmpBase = (() => {
  const repoRoot = realpathSync(fileURLToPath(new URL("../..", import.meta.url)));
  const t = realpathSync(tmpdir());
  return t === repoRoot || t.startsWith(`${repoRoot}/`) ? "/tmp" : t;
})();

/** A throwaway repo with origin/main and a one-commit branch, the fake CLI, and the scenario. */
function fixture(s: Scenario) {
  const root = mkdtempSync(join(tmpBase, "review-cloud-"));
  const repo = join(root, "repo");
  const fake = join(root, "fake");
  mkdirSync(fake);
  const env = childEnv(root, fake);
  assert.ok(existsSync(join(realLib, "../node_modules/yaml/package.json")), "flows/node_modules is not installed; run npm --prefix flows ci");
  const sh = (cmd: string) => {
    const r = spawnSync("bash", ["-c", cmd], { cwd: root, env: { ...env, LIB: realLib }, encoding: "utf8" });
    assert.equal(r.status, 0, r.stderr);
    return r.stdout;
  };
  sh(`set -e
g() { git -c user.email=t@t -c user.name=t "$@"; }
git init -q -b main repo && cd repo
printf '.relayflow/\\nflows/node_modules/\\nflows/lib\\n' > .gitignore && echo a > app.txt && git add -A && g commit -qm base
git update-ref refs/remotes/origin/main HEAD
git checkout -q -b review/x && echo b >> app.txt && g commit -qam "Make the app say b" -m "The body of the commit."
mkdir -p flows/node_modules/.bin ../tmp && cp -R "$LIB" flows/lib && ln -s "$LIB/../node_modules/yaml" flows/node_modules/yaml`);
  writeFileSync(join(repo, "flows/node_modules/.bin/flows"), FAKE_FLOWS);
  chmodSync(join(repo, "flows/node_modules/.bin/flows"), 0o755);
  const json = (name: string, v: unknown) => writeFileSync(join(fake, name), `${JSON.stringify(v)}\n`);
  const head = sh("git -C repo rev-parse HEAD").trim();
  writeFileSync(join(fake, "plan.json"), `${JSON.stringify(s.plan ?? STANDARD_PLAN).replace('"SNAPSHOT"', JSON.stringify(head))}\n`);
  const numbered = (name: string, list: unknown[]) => {
    list.forEach((v, i) => json(`${name}.${i + 1}`, v));
    json(`${name}.last`, list[list.length - 1]);
  };
  numbered("run", s.run ?? [SUBMITTED]);
  numbered("status", s.status ?? [RUNNING, COMPLETED]);
  json("sync.json", s.sync ?? SYNCED);
  if (s.dry !== undefined) json("dry.json", s.dry);
  if (s.summaryText !== undefined) writeFileSync(join(fake, "summary.json"), s.summaryText);
  else if (s.summary !== null) json("summary.json", s.summary ?? summaryOf(["adversarial-review", "context-review"]));
  for (const [flow, report] of Object.entries(s.reports ?? {})) json(`report-${flow}.json`, report);
  if (s.ciRed) writeFileSync(join(fake, "ci-red"), "");
  writeFileSync(join(root, "secrets.env"), s.secrets ?? `TYPESAFE_API_KEY=ts\nFLOWS_CLOUD_TOKEN=${TOKEN}\nFLOWS_CLOUD_URL=https://cloud.example\n`);
  const read = (p: string) => readFileSync(p, "utf8");
  return {
    root, repo, fake, env, sh, read, head,
    state: join(repo, ".relayflow"),
    calls: () => (existsSync(join(fake, "calls")) ? read(join(fake, "calls")).trim().split("\n") : []),
    cloudCalls: () => (existsSync(join(fake, "calls")) ? read(join(fake, "calls")).trim().split("\n").filter((c) => !c.startsWith("run --local-agent")) : []),
  };
}

function childEnv(root: string, fake: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith("GIT_") && !/API_KEY|FLOWS_CLOUD|REVIEW_/.test(k)) env[k] = v;
  return { ...env, HOME: root, FAKE: fake, REVIEW_SECRETS_FILE: join(root, "secrets.env"), CLOUD_POLL_SECONDS: "0", TMPDIR: join(root, "tmp") };
}

/** Source review.sh in the fixture repo and run one Cloud round with these arguments. */
function round(fx: ReturnType<typeof fixture>, args: string[], extraEnv: NodeJS.ProcessEnv = {}) {
  const r = spawnSync("bash", ["-c", `cd "$REPO" || exit 9
. "$1"; shift
export RELAYFLOW_STATE_DIR=$PWD/.relayflow
parse_args "$@"
cloud_credentials
base=$(git merge-base origin/main HEAD)
cloud_round`, "bash", script, ...args], { cwd: fx.root, env: { ...fx.env, ...extraEnv, REPO: fx.repo }, encoding: "utf8" });
  return { status: r.status, out: `${r.stdout}${r.stderr}` };
}

const scratchLeft = (fx: ReturnType<typeof fixture>) => readdirSync(join(fx.root, "tmp"));
const inputOf = (call: string) => JSON.parse(call.slice(call.indexOf("--input ") + "--input ".length));

test("(a) no FLOWS_CLOUD_TOKEN: the whole script refuses with cloud_auth_missing, exit 3, before it clones", () => {
  const fx = fixture({ secrets: "TYPESAFE_API_KEY=ts\nFLOWS_CLOUD_TOKEN=\n" });
  const r = spawnSync("bash", [script, "review/x", "--intent", "x"], { cwd: fx.root, env: fx.env, encoding: "utf8" });
  assert.equal(r.status, 3, r.stderr);
  assert.equal(r.stderr.trim(), "round refused: cloud_auth_missing (no FLOWS_CLOUD_TOKEN on this host; reviews run only as Agent Relay Cloud flow runs, never locally)");
  assert.ok(!existsSync(join(fx.root, "reviews")), "no clone was made");
  assert.deepEqual(fx.calls(), [], "no flows call");
  // --ci-only needs no token: it gets past the check (and fails later, at the clone, here).
  const ci = spawnSync("bash", [script, "review/x", "--ci-only", "--lead", "codex"], { cwd: fx.root, env: fx.env, encoding: "utf8" });
  assert.equal(ci.status, 2);
  assert.match(ci.stderr, /--lead and --lenses do not apply with --ci-only/);
});

test("(b) a configuration refusal ends the round at once: exit 3, no retry, no CI, recorded in run.json", () => {
  const refusal = { ok: false, code: "configuration", message: "workspace has no flows entitlement" };
  const fx = fixture({ run: [refusal] });
  const r = round(fx, ["review/x", "--intent", "make it say b"]);
  assert.equal(r.status, 3, r.out);
  assert.match(r.out, /^round refused: configuration workspace has no flows entitlement$/m);
  assert.equal(fx.cloudCalls().length, 1, "submitted once");
  assert.ok(!fx.calls().some((c) => c.includes("flows/ci.flow.ts")), "CI does not start without a run");
  assert.deepEqual(JSON.parse(fx.read(join(fx.state, "cloud/run.json"))), refusal);
  assert.ok(!existsSync(join(fx.repo, "cloud-review")), "the input dir is removed");
});

test("(c) a transient submit failure is never resubmitted: cloud_transient, run again, exit 3", () => {
  for (const [refusal, message] of [
    [{ ok: false, code: "http_error", message: "Cloud request failed with HTTP 503." }, "Cloud request failed with HTTP 503."],
    [{ ok: false, code: "transient_error", message: "Cloud transport temporarily unavailable." }, "Cloud transport temporarily unavailable."],
    [{ ok: false, code: "admission_unknown", message: "Admission is unknown; Cloud may have started the run. Do not resubmit blindly." }, "Admission is unknown; Cloud may have started the run. Do not resubmit blindly."],
  ] as const) {
    // A second answer that would succeed is there to be ignored.
    const fx = fixture({ run: [refusal, SUBMITTED] });
    const r = round(fx, ["review/x", "--intent", "make it say b"]);
    assert.equal(r.status, 3, r.out);
    assert.ok(r.out.split("\n").includes(`round refused: cloud_transient (${message}); run again`), r.out);
    assert.equal(fx.cloudCalls().filter((c) => c.startsWith("run --cloud")).length, 1, "submitted once");
    assert.ok(!fx.calls().some((c) => c.includes("flows/ci.flow.ts")), "CI does not start without a run");
    assert.deepEqual(JSON.parse(fx.read(join(fx.state, "cloud/run.json"))), refusal);
    assert.ok(!existsSync(join(fx.repo, "cloud-review")));
  }
  // An HTTP 4xx other than 408/429 keeps its own code.
  const forbidden = fixture({ run: [{ ok: false, code: "http_error", message: "Cloud request failed with HTTP 403." }] });
  const f = round(forbidden, ["review/x", "--intent", "x"]);
  assert.equal(f.status, 3);
  assert.match(f.out, /^round refused: http_error Cloud request failed with HTTP 403\.$/m);
});

test("(d) a completed run is synced through a scratch dir, its reports moved, and report.md written in the sized review's shape", () => {
  const fx = fixture({});
  const r = round(fx, ["review/x", "--intent", "make it say b"]);
  assert.equal(r.status, 0, r.out);
  const mb = fx.sh("git -C repo merge-base origin/main HEAD").trim();
  // The calls, in order, and the token only in the Cloud calls' environment, never in argv.
  const calls = fx.calls();
  assert.equal(calls[0], `run --local-agent flows/plan-review.flow.ts --input {"base":"origin/main","intent":"make it say b"}`);
  const submit = calls.find((c) => c.startsWith("run --cloud"));
  assert.ok(submit?.startsWith("run --cloud --json --sync-code --no-connect --budget-minutes 60 flows/cloud-review.flow.ts --input "), submit);
  assert.deepEqual(inputOf(submit ?? ""), { depth: "standard", flows: ["adversarial-review", "context-review"], lead: "claude", dir: "cloud-review", model: "claude-opus-5-5" });
  assert.ok(calls.includes(`run --local-agent flows/ci.flow.ts --input {"base":"origin/main","github":false}`));
  const cloud = fx.cloudCalls().slice(1);
  assert.deepEqual(cloud.slice(0, 3), ["status --cloud --json run-1", "status --cloud --json run-1", "sync --json --dry-run run-1"]);
  assert.match(cloud[3] ?? "", new RegExp(`^sync --json --dir ${join(fx.root, "tmp")}/cloud-sync\\.\\w+ run-1$`), "applied in a scratch dir, not the clone");
  assert.equal(cloud.length, 4);
  assert.deepEqual(scratchLeft(fx), [], "the scratch dir is removed");
  assert.ok(!fx.read(join(fx.fake, "calls")).includes(TOKEN));
  const env = fx.read(join(fx.fake, "env")).trim().split("\n").sort();
  assert.deepEqual(env, [
    "run --cloud --json|token=set url=https://cloud.example anthropic= typesafe=",
    "run --local-agent flows/ci.flow.ts|token= url= anthropic= typesafe=",
    "run --local-agent flows/plan-review.flow.ts|token= url= anthropic= typesafe=set",
    "status --cloud --json|token=set url=https://cloud.example anthropic= typesafe=",
    "status --cloud --json|token=set url=https://cloud.example anthropic= typesafe=",
    "sync --json --dir|token=set url=https://cloud.example anthropic= typesafe=",
    "sync --json --dry-run|token=set url=https://cloud.example anthropic= typesafe=",
  ], "the Cloud token reaches only the Cloud calls, TYPESAFE only the plan");
  for (const log of ["cloud.log", "plan-review.log", "ci.log"]) assert.ok(!fx.read(join(fx.state, "logs", log)).includes(TOKEN), `${log} holds no token`);
  // The uploaded input and the intent.
  assert.equal(fx.read(join(fx.fake, "uploaded")), "change.patch\nfiles.txt\nintent.md\nmanifest\nmeta.json\n");
  const indexListing = spawnSync("git", ["ls-files", "-s", "-z"], { cwd: fx.repo, encoding: "utf8" }).stdout;
  assert.ok(indexListing.includes("\tapp.txt\0"), indexListing);
  assert.equal(fx.read(join(fx.fake, "manifest")), indexListing, "the manifest lists the uploaded tracked files as git ls-files -s -z does");
  assert.equal(fx.read(join(fx.fake, "intent.md")), "# Intent\n\nmake it say b\n\n## Branch\n\nreview/x\n\n## Commits\n\n### Make the app say b\n\nThe body of the commit.\n\n");
  assert.deepEqual(JSON.parse(fx.read(join(fx.state, "cloud/input/meta.json"))), { ref: "review/x", head: fx.head, mergeBase: mb, depth: "standard", model: "claude-opus-5-5" });
  // Results moved; nothing left in the tree (the fake CI would have failed on either).
  assert.equal(fx.read(join(fx.state, "adversarial-review/report.md")), "# adversarial-review report\n");
  assert.ok(existsSync(join(fx.state, "context-review/report.json")));
  assert.ok(existsSync(join(fx.state, "cloud/summary.json")));
  assert.ok(!existsSync(join(fx.repo, "cloud-review")));
  assert.equal(fx.sh("git -C repo status --porcelain"), "");
  assert.equal(fx.read(join(fx.state, "sized-review/report.md")), [
    "# Review: standard, green",
    "",
    "Depth: standard (Jev, confidence 0.90); the whole change is reviewed in every Cloud round",
    "Reviewers on: claude on claude-opus-5-5; extra lenses: none",
    "Change: 1 file(s), 1 line(s), 1 commit(s); merge base MB",
    "Lanes selected: agent, linux; Windows runners: no",
    "GitHub gate: not run; before landing, run again with --ci-only --final",
    "Cloud run: run-1 https://cloud.example/runs/run-1",
    "",
    `- PASS adversarial-review: ${fx.state}/adversarial-review/report.md (0 blocker/major of 2 finding(s))`,
    `- PASS context-review: ${fx.state}/context-review/report.md (0 blocker/major of 2 finding(s))`,
    `- PASS ci: ${fx.state}/ci/report.md`,
    "",
  ].join("\n"));
  // Blocking findings make a flow red, and so does a failed one.
  const red = fixture({ summary: summaryOf(["adversarial-review", "context-review"], { flows: { "adversarial-review": { blocking: 1, total: 3, report: "cloud-review/adversarial-review/report.md" }, "context-review": { blocking: 0, total: 0, report: "cloud-review/context-review/report.md", failed: "reviewed without intent (gate)" } } }) });
  assert.equal(round(red, ["review/x", "--intent", "x"]).status, 1);
  const report = red.read(join(red.state, "sized-review/report.md"));
  assert.match(report, /^# Review: standard, 2 flow\(s\) red$/m);
  assert.match(report, /^- FAIL adversarial-review: .*\(1 blocker\/major of 3 finding\(s\)\)$/m);
  assert.match(report, /^- FAIL context-review: .*\(0 blocker\/major of 0 finding\(s\); reviewed without intent \(gate\)\)$/m);
  assert.ok(!existsSync(join(red.state, "context-review.state.json")), "a failed context-review saves no state");
  // A sync with no changes refuses.
  const empty = fixture({ sync: { ok: true, hasChanges: false } });
  const e = round(empty, ["review/x", "--intent", "x"]);
  assert.equal(e.status, 3);
  assert.match(e.out, /^round refused: cloud_sync_failed no changes came back from run run-1$/m);
});

test("a clean reported round writes an exact-tree release receipt; a red round writes none", () => {
  const green = fixture({});
  const result = round(green, ["review/x", "--intent", "x"]);
  assert.equal(result.status, 0, result.out);
  const receipt = JSON.parse(green.read(join(green.state, "cloud/release-readiness.json")));
  assert.equal(receipt.reviewRunId, "run-1");
  assert.equal(receipt.reviewReported, true);
  assert.equal(receipt.openBlockers, 0);
  assert.equal(receipt.localCiPassed, true);
  assert.match(receipt.sourceTree, /^[a-f0-9]{40}$/);
  const red = fixture({ ciRed: true });
  const failed = round(red, ["review/x", "--intent", "x"]);
  assert.equal(failed.status, 1, failed.out);
  assert.ok(!existsSync(join(red.state, "cloud/release-readiness.json")));
});

test("context-review's state is saved with the real review-state.mjs, so the next plan's open reads it", () => {
  const major = { findings: [{ severity: "major", file: "app.txt", line: 1, title: "says b" }, { severity: "minor", file: "app.txt", line: 1, title: "nit" }] };
  const fx = fixture({ reports: { "context-review": major } });
  const r = round(fx, ["review/x", "--intent", "x"]);
  assert.equal(r.status, 0, r.out);
  assert.ok(existsSync(join(fx.state, "context-review.state.json")), r.out);
  const saved = JSON.parse(fx.read(join(fx.state, "context-review.state.json")));
  assert.deepEqual(saved, { branch: "review/x", mergeBase: "MB", snapshot: fx.head, open: [major.findings[0]] });
  assert.equal(fx.sh("git -C repo rev-parse refs/relayflow/context-review/reviewed").trim(), fx.head);
  // What flows/plan-review.flow.ts runs to count the open findings.
  const open = spawnSync("node", ["flows/lib/review-state.mjs", "open", "context-review", "MB"], { cwd: fx.repo, env: { ...fx.env, RELAYFLOW_STATE_DIR: fx.state }, encoding: "utf8" });
  assert.equal(open.stdout, "1\n", open.stderr);
  assert.ok(!existsSync(join(fx.state, "adversarial-review.state.json")), "only context-review's state is read by the plan");
});

test("(e) a failed run that wrote its summary is reported red from it, not refused; one with none is refused after CI ends", () => {
  const fx = fixture({
    status: [RUNNING, FAILED("step_failed")],
    summary: summaryOf(["adversarial-review", "context-review"], { flows: { "adversarial-review": { blocking: 0, total: 1, report: "cloud-review/adversarial-review/report.md" }, "context-review": { blocking: 0, total: 0, report: "cloud-review/context-review/report.md", failed: "no intent.md in the input" } } }),
  });
  const r = round(fx, ["review/x", "--intent", "x"]);
  assert.equal(r.status, 1, r.out);
  assert.ok(!/round refused/.test(r.out), r.out);
  assert.match(r.out, /^note: run run-1 ended failed \(step_failed\) after writing its summary; the round is reported from it$/m);
  const report = fx.read(join(fx.state, "sized-review/report.md"));
  assert.match(report, /^# Review: standard, 1 flow\(s\) red$/m);
  assert.match(report, /^- PASS adversarial-review: /m);
  assert.match(report, /^- FAIL context-review: .*; no intent\.md in the input\)$/m);
  assert.match(report, /^Cloud run: run-1 https:\/\/cloud\.example\/runs\/run-1 \(ended failed: step_failed; reported from its summary\)$/m);
  // No summary came back: refused, after CI ran to its end.
  const none = fixture({ status: [RUNNING, FAILED("step_failed")], sync: { ok: true, hasChanges: false } });
  const n = round(none, ["review/x", "--intent", "x"]);
  assert.equal(n.status, 3, n.out);
  assert.match(n.out, /^round refused: cloud_failed step_failed run run-1$/m);
  assert.match(n.out, /^note: no summary came back from run run-1: no changes came back from run run-1$/m);
  assert.ok(existsSync(join(none.state, "ci/report.md")), "CI ran to its end");
  assert.ok(!existsSync(join(none.state, "sized-review/report.md")));
  // An auth refusal while polling refuses; a transient read failure is polled through.
  const flaky = fixture({ status: [{ ok: false, code: "cloud_unreachable", message: "ECONNRESET" }, { ok: false, code: "cloud_auth_expired", message: "token expired" }] });
  const f = round(flaky, ["review/x", "--intent", "x"]);
  assert.equal(f.status, 3);
  assert.match(f.out, /reading run run-1 failed transiently \(cloud_unreachable\)/);
  assert.match(f.out, /^round refused: cloud_auth_expired token expired run run-1$/m);
  assert.ok(!flaky.cloudCalls().some((c) => c.startsWith("sync")), "nothing is synced");
});

test("budget_exceeded with no summary refuses as cloud_budget_exceeded", () => {
  const fx = fixture({ status: [FAILED("budget_exceeded")], sync: { ok: true, hasChanges: false } });
  const r = round(fx, ["review/x", "--intent", "x"]);
  assert.equal(r.status, 3, r.out);
  assert.match(r.out, /^round refused: cloud_budget_exceeded run run-1$/m);
});

test("a run that never ends refuses at the poll deadline", () => {
  const fx = fixture({ status: [RUNNING] });
  const r = round(fx, ["review/x", "--intent", "x"], { CLOUD_POLL_DEADLINE_SECONDS: "0" });
  assert.equal(r.status, 3, r.out);
  assert.match(r.out, /^round refused: cloud_poll_timeout no end within 0 minute\(s\) run run-1$/m);
  assert.ok(!fx.cloudCalls().some((c) => c.startsWith("sync")));
  assert.ok(!existsSync(join(fx.state, "sized-review/report.md")));
});

test("a sync with changes but no summary.json refuses", () => {
  const fx = fixture({ summary: null, sync: { ...SYNCED, files: ["cloud-review/adversarial-review/report.md"] } });
  const r = round(fx, ["review/x", "--intent", "x"]);
  assert.equal(r.status, 3, r.out);
  assert.match(r.out, /^round refused: cloud_sync_failed no cloud-review\/summary\.json came back run run-1$/m);
  assert.ok(!existsSync(join(fx.repo, "cloud-review")));
  assert.deepEqual(scratchLeft(fx), []);
});

test("a summary.json that is not in the flow's shape refuses as cloud_sync_failed, with no report", () => {
  for (const [summaryText, why] of [
    ['{"flows":', "jq: parse error: Unfinished JSON term at EOF at line 1, column 9"],
    [`${JSON.stringify(summaryOf(["adversarial-review"], { flows: { "adversarial-review": { total: 2, report: "cloud-review/adversarial-review/report.md" } } }))}\n`, "wrong shape"],
  ]) {
    const fx = fixture({ summaryText });
    const r = round(fx, ["review/x", "--intent", "x"]);
    assert.equal(r.status, 3, r.out);
    assert.ok(r.out.split("\n").includes(`round refused: cloud_sync_failed unreadable cloud-review/summary.json (${why}) run run-1`), r.out);
    assert.ok(!existsSync(join(fx.state, "sized-review/report.md")));
    assert.ok(!existsSync(join(fx.repo, "cloud-review")));
    assert.deepEqual(scratchLeft(fx), []);
  }
});

test("a Cloud token and URL inherited from the host's environment reach no local flow; the Cloud calls get the ones from secrets", () => {
  const fx = fixture({});
  const r = round(fx, ["review/x", "--intent", "x"], { FLOWS_CLOUD_TOKEN: "inherited", FLOWS_CLOUD_URL: "https://inherited.example" });
  assert.equal(r.status, 0, r.out);
  const env = fx.read(join(fx.fake, "env")).trim().split("\n");
  const local = env.filter((l) => l.startsWith("run --local-agent"));
  const cloud = env.filter((l) => !l.startsWith("run --local-agent"));
  assert.equal(local.length, 2);
  for (const l of local) assert.match(l, /\|token= url= /, l);
  assert.equal(cloud.length, 5);
  for (const l of cloud) assert.match(l, /\|token=set url=https:\/\/cloud\.example /, l);
});

test("a run that changed files outside cloud-review/ is refused and nothing in the clone changes", () => {
  const planted = ["app.txt", ".gitignore", "flows/node_modules/.bin/flows", ".relayflow/context-review.state.json", ".relayflow/keep.txt"];
  const files = ["cloud-review/summary.json", "flows/ci.flow.ts", ...planted];
  const check = (dry: unknown, expect: RegExp, applied: boolean) => {
    const fx = fixture({ sync: { ...SYNCED, files }, dry });
    mkdirSync(fx.state, { recursive: true });
    writeFileSync(join(fx.state, "context-review.state.json"), "the last round's state\n");
    writeFileSync(join(fx.state, "keep.txt"), "kept\n");
    const before = planted.map((p) => fx.read(join(fx.repo, p)));
    const r = round(fx, ["review/x", "--intent", "x"]);
    assert.equal(r.status, 3, r.out);
    assert.match(r.out, expect);
    assert.deepEqual(planted.map((p) => fx.read(join(fx.repo, p))), before, "tracked files, .relayflow and flows/node_modules are untouched");
    assert.ok(!existsSync(join(fx.repo, "flows/ci.flow.ts")) && !existsSync(join(fx.repo, "cloud-review")));
    assert.equal(fx.sh("git -C repo status --porcelain"), "");
    assert.equal(fx.cloudCalls().some((c) => c.startsWith("sync --json --dir")), applied);
    assert.deepEqual(scratchLeft(fx), [], "the scratch dir is removed");
  };
  // The dry run lists them: refused before anything is applied.
  check(undefined, /^round refused: cloud_sync_failed the run changed files outside cloud-review\/ \(flows\/ci\.flow\.ts app\.txt \.gitignore \) run run-1$/m, false);
  // A dry-run file list that is clean but a patch header that is not.
  check({ ok: true, hasChanges: true, applied: false, dryRun: true, files: ["cloud-review/summary.json"], excluded: [], patch: "diff --git a/cloud-review/summary.json b/cloud-review/summary.json\ndiff --git a/cloud-review/x b/flows/ci.flow.ts\n" },
    /^round refused: cloud_sync_failed the run changed files outside cloud-review\/ \(diff --git a\/cloud-review\/x b\/flows\/ci\.flow\.ts \) run run-1$/m, false);
  // A dry run that looked clean but an apply that was not: caught in the scratch dir.
  check({ ok: true, hasChanges: true, applied: false, dryRun: true, files: ["cloud-review/summary.json"], excluded: [], patch: "diff --git a/cloud-review/summary.json b/cloud-review/summary.json\n" },
    /^round refused: cloud_sync_failed the run changed files outside cloud-review\/ \(flows\/ci\.flow\.ts app\.txt \.gitignore \) run run-1$/m, true);
});

test("red local CI makes the round red", () => {
  const fx = fixture({ ciRed: true });
  const r = round(fx, ["review/x", "--intent", "x"]);
  assert.equal(r.status, 1, r.out);
  const report = fx.read(join(fx.state, "sized-review/report.md"));
  assert.match(report, /^- FAIL ci: /m);
  assert.match(report, /^# Review: standard, 1 flow\(s\) red$/m);
});

test("a planned flow missing from summary.json is red: no result came back", () => {
  const fx = fixture({ summary: summaryOf(["adversarial-review"]) });
  const r = round(fx, ["review/x", "--intent", "x"]);
  assert.equal(r.status, 1, r.out);
  assert.match(fx.read(join(fx.state, "sized-review/report.md")), /^- FAIL context-review: .*\(0 blocker\/major of 0 finding\(s\); no result came back from the run\)$/m);
});

test("a plan model of claude-fable-5-1 is refused before anything is submitted", () => {
  for (const args of [[], ["--lead", "codex", "--lenses", "grok"]]) {
    const fx = fixture({ plan: { ...STANDARD_PLAN, model: "claude-fable-5-1" } });
    const r = round(fx, ["review/x", "--intent", "x", ...args]);
    assert.equal(r.status, 1, r.out);
    assert.match(r.out, /^refusing the plan's model claude-fable-5-1: Claude reviewers run on Sonnet or Opus only$/m);
    assert.ok(!fx.calls().some((c) => c.startsWith("run --cloud")));
  }
});

test("--lead: claude by default on the plan's model; REVIEW_LEAD sets the default; --lead codex runs every lens on codex", () => {
  const fx = fixture({ plan: SMALL_PLAN, summary: summaryOf(["adversarial-review"]) });
  const r = round(fx, ["review/x", "--depth", "small"]);
  assert.equal(r.status, 0, r.out);
  const submit = fx.calls().find((c) => c.startsWith("run --cloud")) ?? "";
  assert.match(submit, /--budget-minutes 30 /);
  assert.deepEqual(inputOf(submit), { depth: "small", flows: ["adversarial-review"], lead: "claude", dir: "cloud-review", model: "claude-sonnet-5-5" });
  assert.equal(fx.calls()[0], `run --local-agent flows/plan-review.flow.ts --input {"base":"origin/main","depth":"small"}`);
  assert.equal(fx.read(join(fx.fake, "uploaded")), "change.patch\nfiles.txt\nmanifest\nmeta.json\n", "no intent.md without context-review");
  assert.match(fx.read(join(fx.state, "sized-review/report.md")), /^Reviewers on: claude on claude-sonnet-5-5; extra lenses: none$/m);
  const codex = fixture({ plan: SMALL_PLAN, summary: summaryOf(["adversarial-review"], { lead: "codex" }) });
  assert.equal(round(codex, ["review/x", "--depth", "small", "--lead", "codex"]).status, 0);
  assert.deepEqual(inputOf(codex.calls().find((c) => c.startsWith("run --cloud")) ?? ""), { depth: "small", flows: ["adversarial-review"], lead: "codex", dir: "cloud-review" });
  assert.match(codex.read(join(codex.state, "sized-review/report.md")), /^Reviewers on: codex; extra lenses: none$/m);
  const lead = (secrets: string, ...args: string[]) =>
    spawnSync("bash", ["-c", '. "$1"; shift; parse_args "$@"; echo "$lead"', "bash", script, "review/x", ...args], {
      cwd: fx.root, env: { ...fx.env, REVIEW_SECRETS_FILE: join(fx.root, secrets) }, encoding: "utf8",
    });
  writeFileSync(join(fx.root, "lead.env"), "REVIEW_LEAD=codex\n");
  assert.equal(lead("lead.env").stdout, "codex\n");
  assert.equal(lead("lead.env", "--lead", "claude").stdout, "claude\n");
  assert.equal(lead("missing.env").stdout, "claude\n");
  const bad = lead("missing.env", "--lead", "grok");
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /bad lead grok/);
});

test("--lenses: extra reviewers in the input, one on the lead's CLI ignored, only claude given the model, all in the Reviewers line", () => {
  const fx = fixture({ summary: summaryOf(["adversarial-review", "context-review"], { lensesUnavailable: ["grok-second-opinion: the grok CLI is not installed in this sandbox"] }) });
  const r = round(fx, ["review/x", "--intent", "x", "--lenses", "grok,codex,claude"]);
  assert.equal(r.status, 0, r.out);
  const submit = fx.calls().find((c) => c.startsWith("run --cloud")) ?? "";
  assert.deepEqual(inputOf(submit), { depth: "standard", flows: ["adversarial-review", "context-review"], lead: "claude", dir: "cloud-review", lenses: { codex: true, grok: true }, model: "claude-opus-5-5" });
  assert.match(fx.read(join(fx.state, "sized-review/report.md")), /^Reviewers on: claude on claude-opus-5-5; extra lenses: codex, grok; unavailable: grok-second-opinion: the grok CLI is not installed in this sandbox$/m);
  const codexLead = fixture({});
  round(codexLead, ["review/x", "--intent", "x", "--lead", "codex", "--lenses", "codex,grok"]);
  assert.deepEqual(inputOf(codexLead.calls().find((c) => c.startsWith("run --cloud")) ?? ""), { depth: "standard", flows: ["adversarial-review", "context-review"], lead: "codex", dir: "cloud-review", lenses: { grok: true } });
  const withClaude = fixture({});
  round(withClaude, ["review/x", "--intent", "x", "--lead", "codex", "--lenses", "claude"]);
  assert.deepEqual(inputOf(withClaude.calls().find((c) => c.startsWith("run --cloud")) ?? ""), { depth: "standard", flows: ["adversarial-review", "context-review"], lead: "codex", dir: "cloud-review", lenses: { claude: true }, model: "claude-opus-5-5" });
  const bad = spawnSync("bash", ["-c", '. "$1"; shift; parse_args "$@"', "bash", script, "review/x", "--lenses", "codex,gemini"], { cwd: fx.root, env: fx.env, encoding: "utf8" });
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /bad lens gemini/);
});

test("the Cloud budget is the depth's cap: 30 minutes small, 60 standard, 90 deep", () => {
  for (const [plan, flows, budget] of [[SMALL_PLAN, ["adversarial-review"], 30], [STANDARD_PLAN, ["adversarial-review", "context-review"], 60], [DEEP_PLAN, ["adversarial-review", "context-review"], 90]] as const) {
    const fx = fixture({ plan, summary: summaryOf([...flows], { depth: plan.depth }) });
    const r = round(fx, ["review/x", "--intent", "x"]);
    assert.equal(r.status, 0, r.out);
    const submit = fx.calls().find((c) => c.startsWith("run --cloud")) ?? "";
    assert.match(submit, new RegExp(`--budget-minutes ${budget} flows/cloud-review\\.flow\\.ts `), `${plan.depth}: ${submit}`);
    assert.equal(inputOf(submit).depth, plan.depth);
    assert.match(r.out, new RegExp(`^Cloud run run-1 submitted \\(${plan.depth}, ${budget} budget minutes\\)`, "m"));
  }
});
