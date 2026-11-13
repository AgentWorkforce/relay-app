import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { GITHUB_STEP, lanesCommand, parseLanes, selectsWindows, underLocalLoad, UNTRACKED } from "./changes.ts";
import { TREE_FINGERPRINT } from "./review.ts";

/** A repo on `work`, one commit past `main`, with a tracked edit not yet committed. */
function repo() {
  const dir = mkdtempSync(join(tmpdir(), "changes-test-"));
  const git = (...args) => execFileSync("git", ["-C", dir, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main");
  git("config", "user.email", "t@example.com");
  git("config", "user.name", "t");
  mkdirSync(join(dir, "src"));
  writeFileSync(join(dir, "src/a.txt"), "one\n");
  git("add", ".");
  git("commit", "-qm", "base");
  git("checkout", "-qb", "work");
  writeFileSync(join(dir, "src/b.txt"), "committed\n");
  writeFileSync(join(dir, "Cargo.lock"), "lock\n".repeat(50));
  git("add", ".");
  git("commit", "-qm", "work");
  writeFileSync(join(dir, "src/a.txt"), "two\n");
  return { dir, git };
}
const sh = (dir, command) => execFileSync("/bin/sh", ["-c", command], { cwd: dir, encoding: "utf8" });

test("unstaged new files count anywhere, but the relay's and the CLIs' state does not", () => {
  const { dir } = repo();
  mkdirSync(join(dir, "tools"));
  writeFileSync(join(dir, "tools/build.sh"), "x\n");
  writeFileSync(join(dir, "MIGRATION.md"), "x\n");
  writeFileSync(join(dir, "src/new.txt"), "x\n");
  for (const local of [".agentworkforce", ".claude"]) {
    mkdirSync(join(dir, local));
    writeFileSync(join(dir, local, "state.json"), "{}\n");
  }
  assert.deepEqual(sh(dir, UNTRACKED).trim().split("\n").sort(), ["MIGRATION.md", "src/new.txt", "tools/build.sh"]);
});

test("a cargo step red only on named tests is green when GitHub passed the same command on this OS", () => {
  const red = { job: "rust", step: "cargo test", ok: false, output: "", failedTests: ["a::b"], vouched: true };
  const build = { job: "rust", step: "build tests", ok: false, output: "" };
  const github = (ok) => ({ job: "github", step: GITHUB_STEP, ok, output: "" });
  const [passed, stillRed] = underLocalLoad([red, build, github(true)]);
  assert.equal(passed.ok, true);
  assert.match(passed.step, /failed only under local load, passed on GitHub: a::b/);
  assert.equal(stillRed.ok, false, "a failure without named tests stays red");
  assert.equal(underLocalLoad([red, github(false)])[0].ok, false);
  assert.equal(underLocalLoad([red])[0].ok, false, "without GitHub nothing vouches for it");
  const unvouched = { ...red, vouched: undefined };
  assert.equal(underLocalLoad([unvouched, github(true)])[0].ok, false, "GitHub never ran this step here, so it stays red");
});

test("the tree fingerprint moves with any edit a reviewer could make", () => {
  const { dir } = repo();
  mkdirSync(join(dir, "flows/node_modules/.bin"), { recursive: true });
  writeFileSync(join(dir, ".gitignore"), "flows/node_modules/\n/.claude/\n");
  mkdirSync(join(dir, ".claude"));
  writeFileSync(join(dir, ".claude/settings.json"), "{}\n");
  writeFileSync(join(dir, "untracked.txt"), "one\n");
  writeFileSync(join(dir, "flows/node_modules/.bin/flows"), "one\n");
  const print = () => sh(dir, TREE_FINGERPRINT).trim();
  const seen = new Set([print()]);
  assert.equal(print(), [...seen][0], "stable when nothing changed");
  mkdirSync(join(dir, ".claude/hooks"), { recursive: true });
  for (const [path, text] of [
    ["src/a.txt", "three\n"],
    ["untracked.txt", "two\n"],
    ["flows/node_modules/.bin/flows", "two\n"],
    [".claude/settings.json", '{"hooks":{}}\n'],
    [".claude/hooks/run.sh", "echo planted\n"],
    [".git/hooks/pre-commit", "echo planted\n"],
  ]) {
    writeFileSync(join(dir, path), text);
    const now = print();
    assert.match(now, /^[0-9a-f]{40}$/);
    assert.ok(!seen.has(now), `editing ${path} did not move it`);
    seen.add(now);
  }
  mkdirSync(join(dir, ".agentworkforce"));
  writeFileSync(join(dir, ".agentworkforce/inbox.json"), "{}\n");
  assert.ok(seen.has(print()), "the relay's own state must not trip it");
});

test("the selector's output parses to a lane map, and only a windows lane selects the Windows runners", () => {
  const lanes = parseLanes("shell=true\nprobe=false\nwindows=false\nwindows_package=false\n");
  assert.deepEqual(lanes, { shell: true, probe: false, windows: false, windows_package: false });
  assert.equal(selectsWindows(lanes), false);
  assert.equal(selectsWindows({ ...lanes, windows_package: true }), true);
  assert.deepEqual(parseLanes("\n"), {});
});

test("the lane command feeds the selector the paths changed to the working tree, and asks it for nothing on an empty diff", () => {
  const { dir, git } = repo();
  mkdirSync(join(dir, "scripts"));
  // A stand-in for scripts/ci-changes.py: echo what it was asked about.
  writeFileSync(
    join(dir, "scripts/ci-changes.py"),
    'import sys\nif "--base" in sys.argv: print("asked=" + sys.argv[sys.argv.index("--base") + 1][:7] + ".." + sys.argv[sys.argv.index("--head") + 1][:7])\n' +
      'else: print("files=" + ",".join(sorted(p.decode() for p in sys.stdin.buffer.read().split(b"\\0") if p)))\n',
  );
  const mb = git("rev-parse", "main");
  assert.equal(sh(dir, lanesCommand(mb)).trim(), "files=Cargo.lock,src/a.txt,src/b.txt", "the working tree includes the uncommitted edit");
  git("stash", "-q");
  assert.equal(sh(dir, lanesCommand(git("rev-parse", "HEAD"))).trim(), `asked=${git("rev-parse", "HEAD").slice(0, 7)}..${git("rev-parse", "HEAD").slice(0, 7)}`, "an empty diff asks for no lane");
});
