import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { parse } from "yaml";

import {
  AGENT_CI_PREFIXES, DEFERRED_LANES, gateVerdicts, latestRuns, MACOS_COMPLETION, mobilePrefixes, OTHER_COMPLETION, refuseToPublish, settled,
  snapshot, staleSnapshots, touches, verdict,
} from "./remote-ci.mjs";

test("Mobile CI's path filter is read from its workflow, so an iOS-only change dispatches it", () => {
  const prefixes = mobilePrefixes(readFileSync(new URL("../../.github/workflows/mobile-ci.yml", import.meta.url), "utf8"));
  assert.ok(prefixes.includes("ipad/native/"), `got ${prefixes}`);
  assert.ok(touches(["ipad/native/Sources/App.swift"], prefixes));
  assert.ok(!touches(["probe/src/main.rs", "flows/ci.flow.ts"], prefixes));
});

test("a change under agent/ or its workflows asks the gate for Agent CI", () => {
  assert.ok(touches(["agent/tick.sh"], AGENT_CI_PREFIXES));
  assert.ok(touches([".github/workflows/agent-ci.yml"], AGENT_CI_PREFIXES));
  assert.ok(touches([".github/workflows/agent-deploy.yml"], AGENT_CI_PREFIXES));
  assert.ok(!touches(["flows/ci.flow.ts", "scripts/ci-changes.py"], AGENT_CI_PREFIXES));
});

test("only the push and pull_request paths count, however they are quoted", () => {
  const workflow = [
    "on:",
    "  pull_request:",
    "    branches: ['main']",
    "    paths:",
    "      - 'ipad/native/**'",
    '      - "shared/RelayMobileCore/**"',
    "    paths-ignore:",
    "      - 'docs/**'",
    "  push:",
    "    branches:",
    "      - 'main'",
    "    paths:",
    "      - .github/workflows/mobile-ci.yml",
    "jobs:",
    "  a:",
    "    strategy:",
    "      matrix:",
    "        os:",
    "          - 'macos-26'",
  ].join("\n");
  assert.deepEqual(mobilePrefixes(workflow).sort(), [".github/workflows/mobile-ci.yml", "ipad/native/", "shared/RelayMobileCore/"]);
});

test("a path glob that is not a plain prefix is refused, not misread", () => {
  assert.throws(() => mobilePrefixes("on:\n  push:\n    paths:\n      - 'ipad/*/Sources/**'\n"), /not a plain prefix/);
});

test("only this dispatch's newest run per workflow counts", () => {
  const runs = [
    { databaseId: 1, workflowName: "Desktop CI", createdAt: "2026-10-29T00:00:00Z" },
    { databaseId: 3, workflowName: "Desktop CI", createdAt: "2026-10-29T01:00:00Z" },
    { databaseId: 2, workflowName: "Desktop CI", createdAt: "2026-10-29T01:00:00Z" },
    { databaseId: 4, workflowName: "Mobile CI", createdAt: "2026-10-29T01:00:00Z" },
    { databaseId: 5, workflowName: "Desktop Release", createdAt: "2026-10-29T01:00:00Z" },
  ];
  assert.deepEqual(latestRuns(runs, "2026-10-29T00:30:00Z", ["Desktop CI"]).map((r) => r.databaseId), [3]);
  assert.deepEqual(latestRuns(runs, "2026-10-29T00:30:00Z", ["Desktop CI", "Mobile CI"]).map((r) => r.databaseId), [3, 4]);
});

test("the snapshot holds tracked and staged edits, works on main, and touches nothing", () => {
  const repo = mkdtempSync(join(tmpdir(), "snapshot-test-"));
  const git = (...args) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main");
  git("config", "user.email", "t@example.com");
  git("config", "user.name", "t");
  writeFileSync(join(repo, "a.txt"), "one\n");
  git("add", "a.txt");
  git("commit", "-qm", "base");
  writeFileSync(join(repo, "a.txt"), "two\n"); // tracked edit, unstaged
  writeFileSync(join(repo, "new.txt"), "staged\n");
  git("add", "new.txt"); // new file, staged
  writeFileSync(join(repo, "junk.txt"), "untracked\n"); // never sent
  const head = git("rev-parse", "HEAD");
  const index = git("write-tree");

  const cwd = process.cwd();
  process.chdir(repo);
  let sha;
  try {
    sha = snapshot();
  } finally {
    process.chdir(cwd);
  }
  assert.equal(git("show", `${sha}:a.txt`), "two");
  assert.equal(git("show", `${sha}:new.txt`), "staged");
  assert.throws(() => git("show", `${sha}:junk.txt`));
  assert.equal(git("rev-parse", `${sha}^`), head);
  assert.equal(git("rev-parse", "HEAD"), head, "HEAD moved");
  assert.equal(git("write-tree"), index, "the index changed");
  assert.equal(git("rev-parse", "--abbrev-ref", "HEAD"), "main");
});

test("only old snapshot branches made in this checkout are swept", () => {
  const repo = mkdtempSync(join(tmpdir(), "sweep-test-"));
  const git = (args, env = {}) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8", env: { ...process.env, ...env } }).trim();
  git(["init", "-q", "-b", "main"]);
  git(["config", "user.email", "t@example.com"]);
  git(["config", "user.name", "t"]);
  writeFileSync(join(repo, "a.txt"), "one\n");
  git(["add", "a.txt"]);
  const old = "2026-01-01T00:00:00Z";
  git(["commit", "-qm", "old"], { GIT_COMMITTER_DATE: old, GIT_AUTHOR_DATE: old });
  const oldSha = git(["rev-parse", "HEAD"]);
  writeFileSync(join(repo, "a.txt"), "two\n");
  git(["commit", "-qam", "new"]);
  const newSha = git(["rev-parse", "HEAD"]);
  const elsewhere = "f".repeat(40); // a snapshot another machine made: not in this repo
  const listing = [
    `${oldSha}\trefs/heads/ci/${oldSha.slice(0, 12)}`,
    `${newSha}\trefs/heads/ci/${newSha.slice(0, 12)}`,
    `${elsewhere}\trefs/heads/ci/${elsewhere.slice(0, 12)}`,
    `${oldSha}\trefs/heads/ci/drop-trunk-gating`,
  ].join("\n");
  const cwd = process.cwd();
  process.chdir(repo);
  try {
    assert.deepEqual(staleSnapshots(listing), [`ci/${oldSha.slice(0, 12)}`]);
  } finally {
    process.chdir(cwd);
  }
});

test("a snapshot holding machine-local state is never pushed", () => {
  const repo = mkdtempSync(join(tmpdir(), "publish-test-"));
  const git = (...args) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main");
  git("config", "user.email", "t@example.com");
  git("config", "user.name", "t");
  execFileSync("mkdir", ["-p", join(repo, ".claude"), join(repo, "sub/.agentworkforce")]);
  writeFileSync(join(repo, ".claude/settings.json"), "{}\n");
  writeFileSync(join(repo, "sub/.agentworkforce/key"), "secret\n");
  writeFileSync(join(repo, "a.txt"), "one\n");
  git("add", "-A");
  git("commit", "-qm", "base");
  const cwd = process.cwd();
  process.chdir(repo);
  try {
    const why = refuseToPublish(git("rev-parse", "HEAD"), repo);
    assert.match(why, /machine-local state/);
    assert.match(why, /\.claude\/settings\.json/);
    assert.match(why, /sub\/\.agentworkforce\/key/);
  } finally {
    process.chdir(cwd);
  }
});

const desktop = (conclusion) => ({ workflowName: "Desktop CI", conclusion });
const green = ["changes", "Probe", "macOS Swift tests (arm64)", "macOS package (arm64)", "Linux tests (arm64)", "Windows x64"]
  .map((name) => ({ name, conclusion: "success" }));

test("a Desktop CI run whose only red jobs are the macOS x64 lanes passes the gate and says why", () => {
  const jobs = [
    ...green,
    { name: "macOS Swift tests (x64)", conclusion: "failure" },
    { name: "macOS package (x64)", conclusion: "failure" },
    // Both completion gates read the lanes' result over the whole matrix.
    { name: "macOS arm64", conclusion: "failure" },
    { name: "macOS x64", conclusion: "failure" },
    { name: "Record successful snapshot gate", conclusion: "skipped" },
  ];
  assert.deepEqual(verdict(desktop("failure"), jobs), { pass: true, note: " (macOS x64 lanes deferred)" });
});

test("an x64 failure beside any other red job, or no x64 failure at all, still fails the gate", () => {
  const x64 = { name: "macOS Swift tests (x64)", conclusion: "failure" };
  for (const other of [
    { name: "macOS Swift tests (arm64)", conclusion: "failure" },
    { name: "Linux tests (arm64)", conclusion: "failure" },
    { name: "Windows tests (x64)", conclusion: "cancelled" },
    { name: "macOS core tests (arm64)", conclusion: "timed_out" },
  ]) {
    assert.equal(verdict(desktop("failure"), [...green, x64, other]).pass, false, other.name);
  }
  // A completion gate red on its own is not an x64 lane's doing.
  assert.equal(verdict(desktop("failure"), [...green, { name: "macOS arm64", conclusion: "failure" }]).pass, false);
  // No jobs listed (the listing failed) proves nothing.
  assert.equal(verdict(desktop("failure")).pass, false);
  // Only Desktop CI failures are ever excused, and never a cancelled run.
  assert.equal(verdict({ workflowName: "Mobile CI", conclusion: "failure" }, [...green, x64]).pass, false);
  assert.equal(verdict(desktop("cancelled"), [...green, x64]).pass, false);
});

test("a run with no failed jobs passes as before", () => {
  assert.deepEqual(verdict(desktop("success"), green), { pass: true, note: "" });
  assert.deepEqual(verdict({ workflowName: "Mobile CI", conclusion: "success" }), { pass: true, note: "" });
});

test("the deferred lanes are the x64 rows of Desktop CI's macOS lane jobs", () => {
  const { jobs } = parse(readFileSync(new URL("../../.github/workflows/desktop-ci.yml", import.meta.url), "utf8"));
  const x64 = ["macos-package", "macos-swift-tests", "macos-core-tests"]
    .map((id) => jobs[id].name.replace("${{ matrix.arch }}", "x64"));
  assert.deepEqual([...x64].sort(), [...DEFERRED_LANES].sort());
  assert.equal(jobs.macos.name, "macOS ${{ matrix.arch }}");
  assert.deepEqual(jobs.macos.needs, ["changes", "macos-package", "macos-swift-tests", "macos-core-tests"]);
  assert.deepEqual(MACOS_COMPLETION, jobs.macos.strategy.matrix.arch.map((arch) => `macOS ${arch}`));
  // The gate stops waiting once these are listed and finished, so between
  // them they must need every lane that counts.
  const others = ["windows", "linux"].flatMap((id) => jobs[id].strategy.matrix.arch.map((arch) => jobs[id].name.replace("${{ matrix.arch }}", arch)));
  assert.deepEqual(OTHER_COMPLETION, others);
  assert.equal(jobs["snapshot-evidence"].name, "Record successful snapshot gate");
});

test("a macOS completion gate is excused only when the x64 rows alone can have turned it red", () => {
  const lanes = (arm64, x64) => [
    { name: "macOS package (arm64)", conclusion: arm64 }, { name: "macOS Swift tests (arm64)", conclusion: arm64 },
    { name: "macOS core tests (arm64)", conclusion: arm64 },
    { name: "macOS package (x64)", conclusion: x64 }, { name: "macOS Swift tests (x64)", conclusion: x64 },
    { name: "macOS core tests (x64)", conclusion: x64 },
  ];
  const gates = MACOS_COMPLETION.map((name) => ({ name, conclusion: "failure" }));
  const base = [{ name: "changes", conclusion: "success" }, { name: "Windows x64", conclusion: "success" }];
  assert.equal(verdict(desktop("failure"), [...base, ...lanes("success", "failure"), ...gates]).pass, true);
  // Red while every arm64 row and every x64 row is green: the gate failed its
  // own selection check.
  assert.equal(verdict(desktop("failure"), [...base, ...lanes("success", "success"), gates[0]]).pass, false);
  // An arm64 row skipped while its x64 row ran is a selection the gates refuse.
  const skipped = lanes("success", "failure").map((job) => job.name === "macOS core tests (arm64)" ? { ...job, conclusion: "skipped" } : job);
  assert.equal(verdict(desktop("failure"), [...base, ...skipped, ...gates]).pass, false);
  // Packaging without the Swift tests is one too, whatever the x64 rows did.
  const noSwift = lanes("success", "failure").map((job) => job.name.startsWith("macOS Swift") ? { ...job, conclusion: "skipped" } : job);
  assert.equal(verdict(desktop("failure"), [...base, ...noSwift, ...gates]).pass, false);
  // A lane not selected at all skips both rows.
  const noCore = lanes("success", "failure").map((job) => job.name.startsWith("macOS core") ? { ...job, conclusion: "skipped" } : job);
  assert.equal(verdict(desktop("failure"), [...base, ...noCore, ...gates]).pass, true);
});

// A Desktop CI run whose x64 lanes are still queued or running.
const running = { workflowName: "Desktop CI", status: "in_progress", conclusion: "", databaseId: 7, url: "u7" };
const done = (name, conclusion = "success") => ({ name, status: "completed", conclusion });
const counted = [
  done("changes"), done("Probe"), done("macOS package (arm64)"), done("macOS Swift tests (arm64)"), done("macOS core tests (arm64)"),
  ...OTHER_COMPLETION.map((name) => done(name)),
];
const waiting = [
  { name: "macOS package (x64)", status: "queued", conclusion: null },
  { name: "macOS Swift tests (x64)", status: "in_progress", conclusion: null },
  done("macOS core tests (x64)", "failure"),
  ...MACOS_COMPLETION.map((name) => ({ name, status: "pending", conclusion: null })),
  { name: "Record successful snapshot gate", status: "pending", conclusion: null },
];

test("the gate stops waiting once only the x64 lanes and the jobs after them are still running", () => {
  assert.equal(settled([...counted, ...waiting]), true);
  // A lane that counts is still running.
  assert.equal(settled([...counted.slice(1), { name: "changes", status: "in_progress", conclusion: null }, ...waiting]), false);
  // The Windows and Linux lanes may not even be listed yet.
  assert.equal(settled([...counted.filter((job) => job.name !== "Linux x64"), ...waiting]), false);
  // Every x64 lane finished: the run ends soon, so wait for its own verdict.
  assert.equal(settled([...counted, ...DEFERRED_LANES.map((name) => done(name)), ...waiting.slice(3)]), false);
});

test("the gate's verdicts list jobs only for a failed or running Desktop CI run and fall back when the listing fails", () => {
  const listed = [];
  const lister = (jobs) => (id) => {
    listed.push(id);
    return jobs;
  };
  const mobile = { workflowName: "Mobile CI", status: "completed", conclusion: "success", databaseId: 1, url: "u1" };
  const failed = { workflowName: "Desktop CI", status: "completed", conclusion: "failure", databaseId: 2, url: "u2" };
  const passed = { ...failed, conclusion: "success" };
  const x64Red = [
    ...counted, done("macOS Swift tests (x64)", "failure"),
    ...MACOS_COMPLETION.map((name) => done(name, "failure")), done("Record successful snapshot gate", "skipped"),
  ];

  // All green: nothing is listed.
  assert.deepEqual(gateVerdicts([mobile, passed], lister([])), [{ pass: true, note: "" }, { pass: true, note: "" }]);
  assert.deepEqual(listed, []);
  // Only x64 lanes red: passes with the note.
  assert.deepEqual(gateVerdicts([mobile, failed], lister(x64Red)), [{ pass: true, note: "" }, { pass: true, note: " (macOS x64 lanes deferred)" }]);
  assert.deepEqual(listed, [2]);
  // A lane that counts red fails, and so does a Mobile CI failure.
  const armRed = x64Red.map((job) => job.name === "macOS core tests (arm64)" ? done(job.name, "failure") : job);
  assert.deepEqual(gateVerdicts([failed], lister(armRed)).map((v) => v.pass), [false]);
  assert.deepEqual(gateVerdicts([{ ...mobile, conclusion: "failure" }], lister(x64Red)).map((v) => v.pass), [false]);
  // The run the gate stopped waiting for passes while its x64 lanes run, and
  // fails if a lane that counts went red.
  assert.deepEqual(gateVerdicts([running], lister([...counted, ...waiting])), [{ pass: true, note: " (macOS x64 lanes deferred)" }]);
  const probeRed = [...counted.map((job) => job.name === "Probe" ? done("Probe", "failure") : job), ...waiting];
  assert.deepEqual(gateVerdicts([running], lister(probeRed)).map((v) => v.pass), [false]);
  // The listing failing falls back to the run alone, which fails it, and says so.
  const logged = [];
  const broken = () => {
    throw new Error("HTTP 502");
  };
  assert.deepEqual(gateVerdicts([mobile, failed, running], broken, (line) => logged.push(line)), [
    { pass: true, note: "" }, { pass: false, note: "" }, { pass: false, note: "" },
  ]);
  assert.deepEqual(logged, ["could not list the jobs of u2: HTTP 502", "could not list the jobs of u7: HTTP 502"]);
});
