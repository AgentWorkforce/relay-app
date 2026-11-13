import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { AGENT_CI_PREFIXES, latestRuns, mobilePrefixes, refuseToPublish, snapshot, staleSnapshots, touches } from "./remote-ci.mjs";

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
