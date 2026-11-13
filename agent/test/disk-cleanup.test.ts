/**
 * disk-cleanup.sh against a fake HOME: idle, unowned checkouts go; ones an
 * open ledger entry, a held round lock, a process or another clone still
 * owns stay; a whole checkout goes only when nothing in it is unpushed or
 * uncommitted (unless idle a week); ~/build, ~/src and ~/hotfix are never
 * touched; over the cap only cargo targets are pruned, oldest first, only
 * until usage is back under the cap. A target is a `target/` directory with
 * a CACHEDIR.TAG at most three levels below the checkout (`target`,
 * `gtk/target`, `x/y/target`): a `target/` without the tag, or a tagged one
 * four or more levels down, is never touched.
 * Usage comes from the test-only simulated volume DISK_CLEANUP_FAKE_FS
 * (bytes under the fake HOME against a chosen capacity). Needs bash, git,
 * jq, flock, du and setsid.
 *
 * Nothing here writes inside the repository: every fixture root (the fake
 * HOME with its build/, src/, origins and worktrees) is a fresh mkdtemp
 * directory outside the checkout, each builder asserts that, and every
 * process (fixture shells, the script, git, holders) runs with that root as
 * its cwd and HOME, with no GIT_* variables inherited.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../disk-cleanup.sh", import.meta.url));
const repoRoot = realpathSync(fileURLToPath(new URL("../..", import.meta.url)));
const inside = (p: string, dir: string) => p === dir || p.startsWith(`${dir}/`);
// The temp dir every fixture lives in: never the repository, even if TMPDIR
// points into it.
const tmpBase = (() => {
  const t = realpathSync(tmpdir());
  return inside(t, repoRoot) ? "/tmp" : t;
})();

// The environment of every child: HOME is the fixture root, no GIT_* leaks in
// (a GIT_DIR or GIT_WORK_TREE from a hook would point git at the repository).
function envFor(root: string, extra: Record<string, string> = {}): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith("GIT_")) env[k] = v;
  return { ...env, HOME: root, DISK_CLEANUP_TEST_TMP: tmpBase, ...extra };
}

// A fresh fixture root under the temp dir, asserted to be there.
function tempRoot(prefix: string): string {
  const root = realpathSync(mkdtempSync(join(tmpBase, prefix)));
  assertTemp(root);
  return root;
}
function assertTemp(root: string) {
  assert.ok(isAbsolute(root) && inside(root, tmpBase) && root !== tmpBase && !inside(root, repoRoot), `fixture root ${root} is not a temp dir`);
}

// No background gc or maintenance from any fixture git command: such a process
// deletes files (.git/objects/maintenance.lock) while age() is touching them.
const noGitMaintenance = {
  GIT_CONFIG_COUNT: "2",
  GIT_CONFIG_KEY_0: "gc.auto", GIT_CONFIG_VALUE_0: "0",
  GIT_CONFIG_KEY_1: "maintenance.auto", GIT_CONFIG_VALUE_1: "false",
};

// Runs a fixture builder (bash, $1 = its root) inside its root; the shell
// side refuses a root outside the temp dir too.
const guard = `case "$1" in "$DISK_CLEANUP_TEST_TMP"/?*) ;; *) echo "fixture root '$1' is outside $DISK_CLEANUP_TEST_TMP" >&2; exit 1 ;; esac\n`;
function buildFixture(prefix: string, body: string, ...args: string[]): string {
  const root = tempRoot(prefix);
  execFileSync("bash", ["-c", guard + body, "fixture", root, ...args], { cwd: root, env: envFor(root, noGitMaintenance), stdio: ["ignore", "ignore", "pipe"] });
  return root;
}

// The fake HOME: each checkout a real git repo with a cargo target and a
// pushed bare origin (so it is safe to lose), "old" ones three days idle.
const build = `
set -euo pipefail
H=$1; mkdir -p "$H"/{reviews,work,hotfix,build,src,state,origins}
repo() {
  # One bare origin per checkout, keyed by its path under HOME (never the
  # basename: build/relay-app and src/relay-app must not share one).
  local rel origin
  rel=$(realpath --relative-to="$H" "$1"); origin="$H/origins/$(printf %s "$rel" | sed 's|/|__|g')"
  [ ! -e "$origin" ] || { echo "origin collision: $origin" >&2; exit 1; }
  git init -q -b "$2" "$1"
  git -C "$1" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
  git init -q --bare "$origin"
  git -C "$1" remote add origin "$origin"
  git -C "$1" push -q origin "$2"
  # No target/ in info/exclude: the real repo ignores only /target/, so the
  # nested gtk/target is untracked, as it is on the host.
  printf '.relayflow/\n' > "$1/.git/info/exclude"
  mkdir -p "$1/target/debug" "$1/gtk/target"
  echo 'Signature: 8a477f597d28d172789f06886806bc55' | tee "$1/target/CACHEDIR.TAG" > "$1/gtk/target/CACHEDIR.TAG"
  head -c 300000 /dev/zero > "$1/target/debug/blob"
}
old() { find "$1" -exec touch -h -d '3 days ago' {} +; }
repo "$H/reviews/review-old-free" main; mkdir -p "$H/reviews/review-old-free/.relayflow"; touch "$H/reviews/review-old-free/.relayflow/.lock"; old "$H/reviews/review-old-free"
repo "$H/reviews/review-old-ledger" main; old "$H/reviews/review-old-ledger"
repo "$H/reviews/review-old-done" main; old "$H/reviews/review-old-done"
repo "$H/reviews/review-old-locked" main; mkdir -p "$H/reviews/review-old-locked/.relayflow"; touch "$H/reviews/review-old-locked/.relayflow/.lock"; old "$H/reviews/review-old-locked"
repo "$H/reviews/review-new" main
repo "$H/reviews/review-old-flow-log" main; old "$H/reviews/review-old-flow-log"; mkdir -p "$H/reviews/review-old-flow-log/.relayflow/logs"; touch "$H/reviews/review-old-flow-log/.relayflow/logs/x.log"
mkdir -p "$H/reviews/not-a-checkout"; old "$H/reviews/not-a-checkout"
repo "$H/work/old-cwd" main; old "$H/work/old-cwd"
repo "$H/work/old-branch" feature/x; old "$H/work/old-branch"
repo "$H/work/relay" main; old "$H/work/relay"
repo "$H/work/relay-cwd" main; echo "$H/work/relay/.git/objects" > "$H/work/relay-cwd/.git/objects/info/alternates"
for d in hotfix/link-failure build/relay-app src/relay-app; do repo "$H/$d" main; old "$H/$d"; done
ln -s "$H/build/relay-app" "$H/reviews/a-symlink"
cat > "$H/state/ledger.json" <<'JSON'
[{"agent":"a","status":"working","task":"t | pipe","branch":"review/old-ledger","updated":"x"},
 {"agent":"a","status":"working","task":"t","branch":"feature/x","updated":"x"},
 {"agent":"a","status":"working","task":"no branch","branch":null,"updated":"x"},
 {"agent":"a","status":"done","task":"t","branch":"review/old-done","updated":"x"}]
JSON
`;

function fakeHome(): { home: string; holders: ChildProcess[] } {
  const home = buildFixture("disk-cleanup-", build);
  const holders = [
    spawn("flock", [join(home, "reviews/review-old-locked/.relayflow/.lock"), "sleep", "60"], { cwd: home, stdio: "ignore" }),
    spawn("sleep", ["60"], { cwd: join(home, "work/old-cwd"), stdio: "ignore" }),
  ];
  return { home, holders };
}

const MiB = 1024 * 1024;
const bytesUnder = (dir: string) => Number(execFileSync("du", ["-sxB1", dir], { encoding: "utf8" }).split("\t")[0]);

// Runs against a simulated volume of `capacity` bytes holding everything
// under HOME; by default twice what is there now, so usage starts near 50%.
function run(home: string, args: string[], env: Record<string, string> = {}, capacity = 2 * bytesUnder(home)): string {
  assertTemp(home);
  execFileSync("bash", [script, ...args], {
    cwd: home,
    env: envFor(home, { DISK_CLEANUP_FAKE_FS: `${home}:${capacity}`, ...env }),
    stdio: ["ignore", "pipe", "pipe"],
  });
  const lines = readFileSync(join(home, "state/disk-cleanup.log"), "utf8").trim().split("\n");
  return lines[lines.length - 1] ?? "";
}

function usage(line: string): { before: number; after: number } {
  const m = line.match(/ usage=(\d+)%->(\d+)% /);
  assert.ok(m, line);
  return { before: Number(m[1]), after: Number(m[2]) };
}

const settle = () => new Promise((r) => setTimeout(r, 300));
const done = (h: { home: string; holders: ChildProcess[] }) => {
  for (const p of h.holders) p.kill();
  rmSync(h.home, { recursive: true, force: true });
};
const present = (home: string, rel: string) => existsSync(join(home, rel));
const owned = ["reviews/review-old-ledger", "reviews/review-old-locked", "work/old-cwd", "work/old-branch", "work/relay"];
const untouched = ["hotfix/link-failure", "build/relay-app", "src/relay-app", "reviews/not-a-checkout", "reviews/a-symlink"];

test("a dry run decides and logs one line, and deletes nothing", async () => {
  const h = fakeHome();
  try {
    await settle();
    const line = run(h.home, ["--dry-run"]);
    const u = usage(line);
    assert.ok(u.before < 70 && u.after < u.before, line);
    assert.match(line, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ dry-run freed=\d+\(/);
    assert.match(line, /deleted=\[reviews\/review-old-done\([^)]*idle 72h\),reviews\/review-old-free\([^)]*idle 72h\)\]/);
    assert.match(line, /reviews\/review-old-ledger:open ledger entry for slug review-old-ledger/);
    assert.match(line, /reviews\/review-old-locked:round lock held/);
    assert.match(line, /work\/old-branch:open ledger entry feature\/x/);
    assert.match(line, /work\/old-cwd:pid \d+ \(sleep\) works in it/);
    assert.match(line, /work\/relay:used by /);
    assert.match(line, /reviews\/review-old-flow-log:fresh \(idle 0h < 24h\)/);
    assert.match(line, /reviews\/review-new:fresh \(idle 0h < 24h\)/);
    assert.match(line, /reviews\/not-a-checkout:not a checkout/);
    assert.match(line, /reviews\/a-symlink:symlink/);
    assert.doesNotMatch(line, /hotfix|build\/|src\//);
    for (const rel of ["reviews/review-old-free", "reviews/review-old-done", ...owned, ...untouched]) assert.ok(present(h.home, rel), rel);
  } finally {
    done(h);
  }
});

test("a run deletes idle safe unowned checkouts, and over the cap prunes only targets", async () => {
  const h = fakeHome();
  try {
    await settle();
    const first = run(h.home, []);
    const u = usage(first);
    assert.ok(u.before < 70 && u.after < u.before, first);
    assert.match(first, /targets=\[\]/);
    for (const rel of ["reviews/review-old-free", "reviews/review-old-done"]) assert.ok(!present(h.home, rel), rel);
    for (const rel of [...owned, ...untouched, "reviews/review-new", "work/relay-cwd"]) assert.ok(present(h.home, rel), rel);
    // Nothing left to delete: usage is the same before and after.
    const idle = run(h.home, []);
    assert.match(idle, /freed=0\(.* deleted=\[\] targets=\[\]/);
    const v = usage(idle);
    assert.equal(v.after, v.before, idle);
    // A cap of 0 is always exceeded: only targets go, fresh checkouts stay.
    const line = run(h.home, [], { DISK_CLEANUP_CAP: "0" });
    assert.match(line, /targets=\[[^\]]*reviews\/review-new\/target/, line);
    assert.match(line, /deleted=\[\]/);
    assert.match(line, /still over cap: \d+ kept \(fresh\)/);
    for (const rel of ["reviews/review-new", "reviews/review-old-flow-log", "work/relay-cwd", ...owned, ...untouched]) assert.ok(present(h.home, rel), rel);
    assert.ok(!present(h.home, "reviews/review-new/target"));
  } finally {
    done(h);
  }
});

test("an unreadable ledger deletes nothing", () => {
  const h = fakeHome();
  try {
    writeFileSync(join(h.home, "state/ledger.json"), "{\n");
    assert.throws(() => run(h.home, []));
    const line = readFileSync(join(h.home, "state/disk-cleanup.log"), "utf8");
    assert.match(line, /aborted: no readable ledger/);
    assert.ok(present(h.home, "reviews/review-old-free"));
  } finally {
    done(h);
  }
});

// Three unowned checkouts idle 3h (a), 2h (b) and 1h (c): under the 24h
// age limit, so only the cap can remove anything. Each is 1 MiB of its own
// plus 2 MiB of cargo targets (and ~0.2 MiB of git), so pruning a checkout's
// targets frees 2 MiB. Targets are laid out like the real repo's: a and b
// have gtk/target and desktop-core/target (1 MiB each), c a top-level target
// (2 MiB). a also holds two decoys that must survive: tools/target (no
// CACHEDIR.TAG) and p/q/r/target (tagged, but four levels deep), 100 KB each.
const capFixture = `
set -euo pipefail
H=$1; mkdir -p "$H"/{reviews,work,state}; echo '[]' > "$H/state/ledger.json"
tag() { mkdir -p "$1/debug"; echo 'Signature: 8a477f597d28d172789f06886806bc55' > "$1/CACHEDIR.TAG"; head -c "$2" /dev/zero > "$1/debug/blob"; }
for c in a:3 b:2 c:1; do
  d=$H/reviews/\${c%:*}
  git init -q -b main "$d"
  git -C "$d" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
  head -c ${MiB} /dev/zero > "$d/own"
  case "\${c%:*}" in
    c) tag "$d/target" ${2 * MiB} ;;
    *) tag "$d/gtk/target" ${MiB}; tag "$d/desktop-core/target" ${MiB} ;;
  esac
  if [ "\${c%:*}" = a ]; then
    mkdir -p "$d/tools/target/debug"; head -c 100000 /dev/zero > "$d/tools/target/debug/blob"
    tag "$d/p/q/r/target" 100000
  fi
  find "$d" -exec touch -h -d "\${c#*:} hours ago" {} +
done
`;

function capHome(): string {
  return buildFixture("disk-cleanup-cap-", capFixture);
}

const size = /\(\d+(?:\.\d+)?MiB\)/.source;

// The pruned targets the line names, in order, without their sizes.
function targetsOf(line: string): string[] {
  const m = line.match(/ targets=\[([^\]]*)\] /);
  assert.ok(m, line);
  return m[1]!.split(",").filter(Boolean).map((t) => t.replace(new RegExp(`${size}$`), ""));
}

const aTargets = ["reviews/a/desktop-core/target", "reviews/a/gtk/target"];
const bTargets = ["reviews/b/desktop-core/target", "reviews/b/gtk/target"];
const decoys = ["reviews/a/tools/target/debug/blob", "reviews/a/p/q/r/target/debug/blob"];

test("over the cap, nested targets are pruned oldest checkout first and pruning stops once usage is under it", () => {
  const home = capHome();
  try {
    // ~9.8 MiB on a 12 MiB volume: 82%; less a's targets 65% (still over 50);
    // less b's targets 49%: under, so c's target stays.
    const line = run(home, [], { DISK_CLEANUP_CAP: "50" }, 12 * MiB);
    assert.match(line, / deleted=\[\] targets=\[/, line);
    const t = targetsOf(line);
    assert.deepEqual(t.slice(0, 2).sort(), aTargets, line);
    assert.deepEqual(t.slice(2).sort(), bTargets, line);
    const u = usage(line);
    assert.ok(u.before > 50 && u.after <= 50, line);
    assert.doesNotMatch(line, /still over cap/);
    for (const rel of [...aTargets, ...bTargets]) assert.ok(!present(home, rel), rel);
    for (const rel of ["reviews/a", "reviews/b", "reviews/c", "reviews/c/target/debug/blob", "reviews/a/own", "reviews/b/own"]) assert.ok(present(home, rel), rel);
    // A target without CACHEDIR.TAG, and one deeper than three levels, stay.
    for (const rel of decoys) assert.ok(present(home, rel), rel);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("over the cap with every target gone, fresh checkouts are kept and the line says so", () => {
  const home = capHome();
  try {
    const line = run(home, [], { DISK_CLEANUP_CAP: "50" }, 6 * MiB);
    const t = targetsOf(line);
    assert.deepEqual(t.slice(0, 2).sort(), aTargets, line);
    assert.deepEqual(t.slice(2, 4).sort(), bTargets, line);
    assert.deepEqual(t.slice(4), ["reviews/c/target"], line);
    assert.match(line, /still over cap: 3 kept \(fresh\)/);
    for (const c of ["a", "b", "c"]) assert.ok(present(home, `reviews/${c}/own`), c);
    for (const rel of decoys) assert.ok(present(home, rel), rel);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("a dry run over the cap deletes nothing and logs what it would prune", () => {
  const home = capHome();
  try {
    const line = run(home, ["--dry-run"], { DISK_CLEANUP_CAP: "50" }, 12 * MiB);
    assert.match(line, / dry-run freed=\d+\(/);
    assert.match(line, / deleted=\[\] targets=\[/, line);
    const t = targetsOf(line);
    assert.deepEqual(t.slice(0, 2).sort(), aTargets, line);
    assert.deepEqual(t.slice(2).sort(), bTargets, line);
    const u = usage(line);
    assert.ok(u.before > 50 && u.after <= 50, line);
    for (const rel of ["a/gtk", "a/desktop-core", "b/gtk", "b/desktop-core"]) assert.ok(present(home, `reviews/${rel}/target/debug/blob`), rel);
    assert.ok(present(home, "reviews/c/target/debug/blob"));
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

// Safety fixture: all pushed to a bare origin, then
//   fresh      idle 1h, clean, with a 2 MiB target
//   dirty      idle 30h, an uncommitted file
//   unpushed   idle 30h, a local commit never pushed
//   stashed    idle 30h, clean and pushed, but holds one stash
//   clean      idle 30h, clean and pushed
// Every checkout also has untracked nested cargo targets (gtk/target,
// desktop-core/target) and nothing in .git/info/exclude, as on the host.
//   abandoned  idle 8 days, a local commit never pushed (only with $2 = abandon)
//   abandonedstash  idle 8 days, clean and pushed, one stash (only with $2 = abandon)
const safeFixture = `
set -euo pipefail
H=$1; mkdir -p "$H"/{reviews,work,state,origins}; echo '[]' > "$H/state/ledger.json"
mk() {
  local d=$H/reviews/$1
  git init -q -b main "$d"
  git -C "$d" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
  git init -q --bare "$H/origins/$1"
  git -C "$d" remote add origin "$H/origins/$1"
  git -C "$d" push -q origin main
  head -c ${MiB} /dev/zero > "$d/own.bin"; git -C "$d" add own.bin; git -C "$d" -c user.name=t -c user.email=t@t commit -q -m own; git -C "$d" push -q origin main
  mkdir -p "$d/target/debug"
  echo 'Signature: 8a477f597d28d172789f06886806bc55' > "$d/target/CACHEDIR.TAG"
  head -c ${2 * MiB} /dev/zero > "$d/target/debug/blob"
  for n in gtk desktop-core; do
    mkdir -p "$d/$n/target/debug"
    echo 'Signature: 8a477f597d28d172789f06886806bc55' > "$d/$n/target/CACHEDIR.TAG"
    head -c 1000 /dev/zero > "$d/$n/target/debug/blob"
  done
}
age() { find "$H/reviews/$1" -exec sh -c 'for f; do touch -h -d "$0" "$f" 2>/dev/null || [ ! -e "$f" ] || exit 1; done' "$2 hours ago" {} +; }
for n in fresh dirty unpushed stashed clean abandoned abandonedstash; do case "$n" in abandoned*) [ "\${2:-}" = abandon ] || continue ;; esac; mk "$n"; done
stash() { echo wip >> "$1/own.bin"; git -C "$1" -c user.name=t -c user.email=t@t stash -q; }
echo scratch > "$H/reviews/dirty/notes.txt"
git -C "$H/reviews/unpushed" -c user.name=t -c user.email=t@t commit -q --allow-empty -m local
if [ -d "$H/reviews/abandoned" ]; then git -C "$H/reviews/abandoned" -c user.name=t -c user.email=t@t commit -q --allow-empty -m local; age abandoned 192; fi
stash "$H/reviews/stashed"
if [ -d "$H/reviews/abandonedstash" ]; then stash "$H/reviews/abandonedstash"; age abandonedstash 192; fi
age fresh 1; age dirty 30; age unpushed 30; age stashed 30; age clean 30
`;

function safeHome(abandon = false): string {
  return buildFixture("disk-cleanup-safe-", safeFixture, abandon ? "abandon" : "");
}

function assertOnlyCleanWent(home: string, line: string) {
  assert.match(line, /deleted=\[reviews\/clean\([^)]*idle 30h\)\]/, line);
  assert.match(line, /reviews\/dirty:uncommitted changes/, line);
  assert.match(line, /reviews\/unpushed:unpushed commits/, line);
  assert.match(line, /reviews\/stashed:stashed work/, line);
  assert.match(line, /reviews\/fresh:fresh \(idle 1h < 24h\)/, line);
  assert.ok(!present(home, "reviews/clean"));
  for (const c of ["fresh", "dirty", "unpushed", "stashed"]) assert.ok(present(home, `reviews/${c}/own.bin`), c);
}

test("a day-idle checkout goes only when nothing is uncommitted, stashed or unpushed", () => {
  const home = safeHome();
  try {
    // clean has untracked nested targets and no exclude entry: still clean.
    assert.ok(present(home, "reviews/clean/gtk/target/CACHEDIR.TAG"));
    assert.equal(readFileSync(join(home, "reviews/clean/.git/info/exclude"), "utf8").split("\n").filter((l) => l && !l.startsWith("#")).length, 0);
    const line = run(home, []);
    assertOnlyCleanWent(home, line);
    assert.match(line, /targets=\[\]/);
    assert.doesNotMatch(line, /still over cap/);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("over the cap, targets go from every checkout but only the clean pushed one is deleted", () => {
  const home = safeHome();
  try {
    const line = run(home, [], { DISK_CLEANUP_CAP: "0" });
    assertOnlyCleanWent(home, line);
    const kept = line.match(/still over cap: 4 kept \(([^)]*)\)/);
    assert.ok(kept, line);
    assert.deepEqual(kept[1]!.split(";").sort(), ["fresh", "stashed work", "uncommitted changes", "unpushed commits"]);
    for (const c of ["fresh", "dirty", "unpushed", "stashed"]) assert.ok(!present(home, `reviews/${c}/target`), c);
    assert.ok(present(home, "reviews/dirty/notes.txt"));
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("a checkout idle a week is deleted even with unpushed commits, logged as abandoned", () => {
  const home = safeHome(true);
  try {
    const line = run(home, []);
    assert.match(line, /reviews\/abandoned\([^)]*abandoned \(idle 192h, unpushed commits\)\)/, line);
    assert.match(line, /reviews\/abandonedstash\([^)]*abandoned \(idle 192h, stashed work\)\)/, line);
    assert.ok(!present(home, "reviews/abandoned"));
    assert.ok(!present(home, "reviews/abandonedstash"));
    assert.ok(!present(home, "reviews/clean"));
    for (const c of ["fresh", "dirty", "unpushed", "stashed"]) assert.ok(present(home, `reviews/${c}`), c);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("same-named checkouts never share a bare origin", () => {
  const { home, holders } = fakeHome();
  try {
    const url = (d: string) => execFileSync("git", ["-C", join(home, d), "remote", "get-url", "origin"], { cwd: home, env: envFor(home), encoding: "utf8" }).trim();
    assert.notEqual(url("build/relay-app"), url("src/relay-app"));
  } finally {
    for (const h of holders) h.kill();
    rmSync(home, { recursive: true, force: true });
  }
});

test("a checkout that becomes owned after the scan is skipped, not deleted", () => {
  const { home, holders } = fakeHome();
  try {
    const entry = '[{"agent":"a","status":"working","task":"t","branch":"review/old-free","updated":"x"}]';
    // The hook runs after the scan and the lock, right before the re-check.
    const line = run(home, [], {
      DISK_CLEANUP_BEFORE_REMOVE: `[ "$(basename "$DISK_CLEANUP_CHECKOUT")" != review-old-free ] || printf '%s' '${entry}' > '${join(home, "state/ledger.json")}'`,
    });
    assert.match(line, /reviews\/review-old-free:became owned \(open ledger entry for slug review-old-free\)/, line);
    assert.doesNotMatch(line, /deleted=\[[^\]]*review-old-free/, line);
    assert.ok(present(home, "reviews/review-old-free"));
    assert.ok(!present(home, "reviews/review-old-done"));
  } finally {
    for (const h of holders) h.kill();
    rmSync(home, { recursive: true, force: true });
  }
});

test("a process that enters a checkout after the scan keeps it", () => {
  const { home, holders } = fakeHome();
  try {
    const line = run(home, [], {
      DISK_CLEANUP_BEFORE_REMOVE: `[ "$(basename "$DISK_CLEANUP_CHECKOUT")" != review-old-free ] || { cd "$DISK_CLEANUP_CHECKOUT" && sleep 8 </dev/null >/dev/null 2>&1 & }`,
    });
    assert.match(line, /reviews\/review-old-free:now in use \(pid \d+ \(\w+\) works in it\)/, line);
    assert.ok(present(home, "reviews/review-old-free"));
    assert.ok(!present(home, "reviews/review-old-done"));
  } finally {
    for (const h of holders) h.kill();
    rmSync(home, { recursive: true, force: true });
  }
});

test("over the cap, the round lock is free while a target is removed", () => {
  const home = capHome();
  try {
    const lock = join(home, "reviews/a/.relayflow/.lock");
    mkdirSync(join(home, "reviews/a/.relayflow"), { recursive: true });
    writeFileSync(lock, "");
    // Keep a the oldest checkout, so it is pruned first.
    execFileSync("touch", ["-h", "-d", "3 hours ago", join(home, "reviews/a/.relayflow"), lock, join(home, "reviews/a")], { cwd: home });
    const probe = join(home, "probe");
    // After the lock is released the target must already be renamed (trash on
    // disk) and the lock free for a round that starts now.
    const hook = `[ "$(basename "$DISK_CLEANUP_CHECKOUT")" = a ] || exit 0; ` +
      `if flock -n '${lock}' true; then l=free; else l=held; fi; ` +
      `t=$(ls -d "$DISK_CLEANUP_CHECKOUT"/{gtk,desktop-core}/target.disk-cleanup-trash-* 2>/dev/null | wc -l); ` +
      `[ -e "$DISK_CLEANUP_CHECKOUT/gtk/target" ] || [ -e "$DISK_CLEANUP_CHECKOUT/desktop-core/target" ] && o=target-still-there || o=target-renamed; ` +
      `echo "$l trash=$t $o" > '${probe}'`;
    const line = run(home, [], { DISK_CLEANUP_CAP: "50", DISK_CLEANUP_AFTER_RENAME: hook }, 12 * MiB);
    assert.equal(readFileSync(probe, "utf8").trim(), "free trash=2 target-renamed", line);
    assert.deepEqual(targetsOf(line).sort(), [...aTargets, ...bTargets], line);
    for (const rel of aTargets) assert.ok(!present(home, rel), rel);
    for (const sub of ["", "gtk", "desktop-core"]) assert.deepEqual(readdirSync(join(home, "reviews/a", sub)).filter((n) => n.includes("trash")), [], sub);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("target trash left by an interrupted run is swept at the next run", () => {
  const home = capHome();
  try {
    const lefts = ["reviews/b/target.disk-cleanup-trash-4242", "reviews/b/gtk/target.disk-cleanup-trash-4242"].map((rel) => join(home, rel));
    for (const left of lefts) {
      mkdirSync(join(left, "debug"), { recursive: true });
      writeFileSync(join(left, "debug/blob"), Buffer.alloc(MiB));
    }
    const line = run(home, [], {});
    for (const left of lefts) assert.ok(!existsSync(left), left);
    assert.match(line, /reviews\/b\/target\.disk-cleanup-trash-4242\(\d+(?:\.\d+)?MiB leftover\)/, line);
    assert.match(line, /reviews\/b\/gtk\/target\.disk-cleanup-trash-4242\(\d+(?:\.\d+)?MiB leftover\)/, line);
    // The live targets next to the trash are not swept.
    assert.ok(present(home, "reviews/b/gtk/target/debug/blob"));
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("a process that enters between the scan and the rename moves the checkout back", () => {
  const { home, holders } = fakeHome();
  try {
    // Runs after the rename, before the trash path's cwd scan: a process that
    // cd'd into the checkout just before the rename now works in the trash.
    const line = run(home, [], {
      DISK_CLEANUP_AFTER_RENAME_SCAN: `[ "$(basename "$DISK_CLEANUP_CHECKOUT")" = review-old-free ] || exit 0; cd "$DISK_CLEANUP_TRASH" && { sleep 8 </dev/null >/dev/null 2>&1 & }`,
    });
    assert.match(line, /reviews\/review-old-free:now in use \(pid \d+ \(sleep\) works in it\)/, line);
    assert.doesNotMatch(line, /deleted=\[[^\]]*review-old-free/, line);
    assert.ok(present(home, "reviews/review-old-free/.git"));
    assert.deepEqual(readdirSync(join(home, "reviews")).filter((n) => n.includes("trash")), []);
    assert.ok(!present(home, "reviews/review-old-done"));
  } finally {
    for (const h of holders) h.kill();
    rmSync(home, { recursive: true, force: true });
  }
});

// A stand-in for a detached flows daemon: a bash script at bin/<COMM> (a
// script's comm is its basename, so COMM, named to the script by the test-only
// DISK_CLEANUP_DAEMON_COMM) that sleeps <seconds> with the read builtin, so it
// never execs another binary (which would change its comm) and has no child
// working in its cwd. /bin/sleep cannot simply be copied: on Ubuntu images
// with Rust uutils it is the multicall coreutils binary, which dispatches on
// argv[0] and exits at once when named COMM. It is started with its cwd in
// `cwd` under setsid by a shell that exits at once, so it is reparented to
// pid 1. Returns its pid once its parent is pid 1.
const COMM = "fakeflowd";
const daemonEnv = { DISK_CLEANUP_DAEMON_COMM: COMM };
const statOf = (pid: number): string[] | undefined => {
  try {
    const s = readFileSync(`/proc/${pid}/stat`, "utf8");
    return s.slice(s.lastIndexOf(") ") + 2).split(" ");
  } catch {
    return undefined;
  }
};
// Gone, or a zombie waiting for pid 1 to reap it.
const dead = (pid: number) => {
  const f = statOf(pid);
  return f === undefined || f[0] === "Z";
};
async function detachedDaemon(home: string, cwd: string): Promise<number> {
  assertTemp(home);
  assert.ok(inside(cwd, home), cwd);
  const bin = join(home, "bin", COMM);
  mkdirSync(join(home, "bin"), { recursive: true });
  // <(:) exits at once and `wait $!` reaps it (no child, not even a zombie);
  // opening it read-write keeps the pipe alive, so the read blocks for the
  // full timeout. TERM kills bash in the read.
  if (!existsSync(bin)) { writeFileSync(bin, '#!/bin/bash\nexec 3<> <(:)\nwait $!\nread -r -t "$1" -u 3\n'); chmodSync(bin, 0o755); }
  const pid = Number(execFileSync("setsid", ["bash", "-c", 'cd "$1" || exit 1; "$0" 300 </dev/null >/dev/null 2>&1 & echo $!', bin, cwd], {
    cwd: home, env: envFor(home), encoding: "utf8", stdio: ["ignore", "pipe", "ignore"],
  }).trim());
  // The forked child execs the script after its shell may already have exited.
  const comm = () => { try { return readFileSync(`/proc/${pid}/comm`, "utf8").trim(); } catch { return ""; } };
  for (let i = 0; i < 50 && (statOf(pid)?.[1] !== "1" || comm() !== COMM); i++) await new Promise((r) => setTimeout(r, 100));
  assert.equal(statOf(pid)?.[1], "1", `daemon ${pid} was not reparented to pid 1`);
  assert.equal(comm(), COMM);
  return pid;
}
async function gone(pid: number): Promise<boolean> {
  for (let i = 0; i < 30 && !dead(pid); i++) await new Promise((r) => setTimeout(r, 100));
  return dead(pid);
}
const reap = (pids: number[]) => {
  for (const pid of pids) try { process.kill(pid, "SIGKILL"); } catch { /* already gone */ }
};

test("a directory with only .relayflow/ and/or .relayflowd/ and no .git is removed as a leftover stub", async () => {
  const { home, holders } = fakeHome();
  const pids: number[] = [];
  try {
    mkdirSync(join(home, "reviews/stub/.relayflow/logs"), { recursive: true });
    writeFileSync(join(home, "reviews/stub/.relayflow/.lock"), "");
    // The stub a deleted checkout's detached daemon recreates: .relayflowd/
    // only, the daemon still working in it.
    mkdirSync(join(home, "reviews/dstub/.relayflowd"), { recursive: true });
    writeFileSync(join(home, "reviews/dstub/.relayflowd/relayflowd.lock"), "");
    const daemon = await detachedDaemon(home, join(home, "reviews/dstub/.relayflowd"));
    pids.push(daemon);
    // Anything else in it makes it not a stub.
    mkdirSync(join(home, "work/notes/.relayflow"), { recursive: true });
    writeFileSync(join(home, "work/notes/todo.txt"), "keep");
    const dry = run(home, ["--dry-run"], daemonEnv);
    assert.match(dry, /reviews\/stub\([^)]*leftover stub\)/, dry);
    assert.match(dry, /reviews\/dstub\([^)]*leftover stub\)/, dry);
    assert.ok(present(home, "reviews/stub/.relayflow"));
    const line = run(home, [], daemonEnv);
    assert.match(line, /reviews\/stub\([^)]*leftover stub\)/, line);
    assert.match(line, new RegExp(`reviews/dstub\\([^)]*leftover stub; stopped 1 orphaned ${COMM}\\)`), line);
    assert.ok(!present(home, "reviews/dstub"));
    assert.ok(await gone(daemon), `daemon ${daemon} still runs`);
    assert.match(line, /work\/notes:not a checkout/, line);
    assert.match(line, /reviews\/not-a-checkout:not a checkout/, line);
    assert.ok(!present(home, "reviews/stub"));
    assert.ok(present(home, "work/notes/todo.txt"));
  } finally {
    reap(pids);
    for (const h of holders) h.kill();
    rmSync(home, { recursive: true, force: true });
  }
});

test("a detached flows daemon does not keep a finished checkout: it is stopped and the checkout deleted", async () => {
  const h = fakeHome();
  const pids: number[] = [];
  try {
    const daemon = await detachedDaemon(h.home, join(h.home, "reviews/review-old-free"));
    pids.push(daemon);
    // The same comm as a child of a live process (not detached) is work.
    const bin = join(h.home, "bin", COMM);
    const attached = spawn(bin, ["300"], { cwd: join(h.home, "reviews/review-old-done"), stdio: "ignore" });
    h.holders.push(attached);
    await settle();
    const dry = run(h.home, ["--dry-run"], daemonEnv);
    assert.match(dry, /deleted=\[reviews\/review-old-free\([^)]*idle 72h\)\]/, dry);
    assert.ok(!dead(daemon), "a dry run signals nothing");
    const line = run(h.home, [], daemonEnv);
    assert.match(line, new RegExp(`reviews/review-old-free\\([^)]*idle 72h; stopped 1 orphaned ${COMM}\\)`), line);
    assert.match(line, new RegExp(`reviews/review-old-done:pid ${attached.pid} \\(${COMM}\\) works in it`), line);
    assert.ok(!present(h.home, "reviews/review-old-free"));
    assert.deepEqual(readdirSync(join(h.home, "reviews")).filter((n) => n.includes("trash")), []);
    assert.ok(await gone(daemon), `daemon ${daemon} still runs`);
    assert.ok(!dead(attached.pid!), "the attached process is never signalled");
  } finally {
    reap(pids);
    done(h);
  }
});

test("a detached flows daemon whose checkout is already deleted is stopped by the orphan pass", async () => {
  const h = fakeHome();
  const pids: number[] = [];
  try {
    mkdirSync(join(h.home, "reviews/gone/.relayflowd"), { recursive: true });
    mkdirSync(join(h.home, "elsewhere"), { recursive: true });
    const orphan = await detachedDaemon(h.home, join(h.home, "reviews/gone/.relayflowd"));
    // Outside ~/reviews and ~/work: not this script's to stop.
    const outside = await detachedDaemon(h.home, join(h.home, "elsewhere"));
    pids.push(orphan, outside);
    rmSync(join(h.home, "reviews/gone"), { recursive: true });
    rmSync(join(h.home, "elsewhere"), { recursive: true });
    const dry = run(h.home, ["--dry-run"], daemonEnv);
    assert.match(dry, /orphan daemons: 1 to stop/, dry);
    assert.ok(!dead(orphan), "a dry run signals nothing");
    const line = run(h.home, [], daemonEnv);
    assert.match(line, /freed=\d+\(.* orphan daemons: 1 stopped$/, line);
    assert.ok(await gone(orphan), `daemon ${orphan} still runs`);
    assert.ok(!dead(outside), "a daemon outside ~/reviews and ~/work is never signalled");
  } finally {
    reap(pids);
    done(h);
  }
});

test("no ledger file yet reads as an empty ledger, with a warning on the line", () => {
  const { home, holders } = fakeHome();
  try {
    rmSync(join(home, "state/ledger.json"));
    const line = run(home, []);
    assert.match(line, /warning: no ledger in \S+ yet, read as empty$/, line);
    assert.doesNotMatch(line, /aborted/);
    // Nothing is owned through the ledger any more.
    for (const rel of ["reviews/review-old-done", "reviews/review-old-free", "reviews/review-old-ledger"]) assert.ok(!present(home, rel), rel);
    for (const rel of ["reviews/review-old-locked", "work/old-cwd", "work/relay"]) assert.ok(present(home, rel), rel);
  } finally {
    for (const h of holders) h.kill();
    rmSync(home, { recursive: true, force: true });
  }
});

test("a linked worktree committed to today is fresh, measured from its real git dir", () => {
  const home = buildFixture("disk-cleanup-wt-", `
set -euo pipefail
H=$1; mkdir -p "$H"/{reviews,work,state}; echo '[]' > "$H/state/ledger.json"
g() { git -c user.name=t -c user.email=t@t "$@"; }
git init -q -b main "$H/work/wt-main"; g -C "$H/work/wt-main" commit -q --allow-empty -m init
git init -q --bare "$H/origin"; git -C "$H/work/wt-main" remote add origin "$H/origin"; git -C "$H/work/wt-main" push -q origin main
git -C "$H/work/wt-main" worktree add -q -b wt "$H/work/wt"
echo x > "$H/work/wt/f"; g -C "$H/work/wt" add f; g -C "$H/work/wt" commit -q -m today; git -C "$H/work/wt" push -q origin wt
# The worktree's own files (its .git gitfile included) look three days old;
# its HEAD, index and logs live in wt-main/.git/worktrees/wt and are new.
find "$H/work/wt" -exec touch -h -d '3 days ago' {} +
`);
  try {
    const line = run(home, []);
    assert.match(line, /work\/wt:fresh \(idle 0h < 24h\)/, line);
    assert.match(line, /work\/wt-main:used by /, line);
    assert.ok(present(home, "work/wt/f"));
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("a daemon-written .relayflowd file is not activity: the finished checkout is deleted as idle 72h", () => {
  const home = buildFixture("disk-cleanup-rfd-", `
set -euo pipefail
H=$1; mkdir -p "$H"/{reviews,state}; echo '[]' > "$H/state/ledger.json"
g() { git -c user.name=t -c user.email=t@t "$@"; }
d="$H/reviews/review-done"
git init -q -b main "$d"; g -C "$d" commit -q --allow-empty -m init
git init -q --bare "$H/origin"; git -C "$d" remote add origin "$H/origin"; git -C "$d" push -q origin main
find "$d" -exec touch -h -d '3 days ago' {} +
mkdir -p "$d/.relayflowd/fleet"; echo x > "$d/.relayflowd/x"; echo y > "$d/.relayflowd/fleet/y"
touch -d '3 days ago' "$d"
`);
  try {
    const line = run(home, []);
    assert.match(line, /deleted=\[reviews\/review-done\([^)]*idle 72h\)\]/, line);
    assert.ok(!present(home, "reviews/review-done"));
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("a checkout whose HEAD is the FETCH_HEAD of a sha ref is judged safe, not unpushed", () => {
  const home = buildFixture("disk-cleanup-fh-", `
set -euo pipefail
H=$1; mkdir -p "$H"/{reviews,state}; echo '[]' > "$H/state/ledger.json"
g() { git -c user.name=t -c user.email=t@t "$@"; }
git init -q -b main "$H/seed"; g -C "$H/seed" commit -q --allow-empty -m init
git init -q --bare "$H/origin"; git -C "$H/seed" remote add origin "$H/origin"; git -C "$H/seed" push -q origin main
sha=$(git -C "$H/seed" rev-parse HEAD)
d="$H/reviews/review-sha"
git init -q "$d"; git -C "$d" remote add origin "$H/origin"
git -C "$d" fetch -q origin "$sha"; git -C "$d" checkout -q --detach FETCH_HEAD
[ -z "$(git -C "$d" for-each-ref refs/remotes)" ]
find "$d" -exec touch -h -d '3 days ago' {} +
`);
  try {
    const line = run(home, []);
    assert.doesNotMatch(line, /unpushed commits/, line);
    assert.match(line, /deleted=\[reviews\/review-sha\([^)]*idle 72h\)\]/, line);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});
