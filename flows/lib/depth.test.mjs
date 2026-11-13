import assert from "node:assert/strict";
import test from "node:test";

import { classification, CONTRACT_DOC, describeOrigin, resolveDepth, DIFF_CHARS, docsOnly, evidence, explainFallback, FILES_CHARS, floor, floorReason, isDepth, LISTED_FILES, LISTED_SUBJECTS, MODELS, SMALL_MAX_FILES, SMALL_MAX_LINES, STATE_CHARS, SUBJECT_CHARS, SUBJECTS_CHARS } from "./depth.ts";
import { jobsFor, plan } from "./plan.ts";

const change = (files, extra = {}) => ({
  mergeBase: "a".repeat(40),
  snapshot: "b".repeat(40),
  tree: "c".repeat(40),
  files: files.map((path) => ({ path, added: 1, deleted: 0 })),
  fileCount: files.length,
  lines: files.length,
  subjects: ["Fix the thing"],
  commitCount: 1,
  redactedSubjects: 0,
  diff: "diff --git a/x b/x\n",
  redacted: 0,
  ...extra,
});

test("a small verdict stands only for a change under the size floors with no sensitive path", () => {
  assert.equal(floor("small", ["gtk/src/ui.rs"], 20), "small");
  assert.equal(floor("small", ["gtk/src/ui.rs"], SMALL_MAX_LINES), "small");
  assert.equal(floor("small", ["gtk/src/ui.rs"], SMALL_MAX_LINES + 1), "standard");
  const many = Array.from({ length: SMALL_MAX_FILES + 1 }, (_, i) => `gtk/src/f${i}.rs`);
  assert.equal(floor("small", many, 20), "standard");
  assert.equal(floor("small", many.slice(0, SMALL_MAX_FILES), 20), "small");
  assert.equal(floor("small", ["gtk/src/ui.rs"], 20, SMALL_MAX_FILES + 1), "standard", "a list shorter than the count never passes");
});

test("sensitive paths never review as small", () => {
  for (const path of [
    "desktop-core/src/lib.rs", "probe/src/main.rs",
    ".github/workflows/desktop-ci.yml", ".github/actions/windows-build-cache-prune/main.cjs",
    "scripts/release-version.py", "scripts/ci-changes.py", "scripts/build-app.sh",
    "flows/lib/depth.ts", "land.config.json", "agent/review.sh", "agent/release.sh", "agent/lead-prompt.md", "agent/agent.config.ts",
    "linux/agent-relay.postinst", "windows/install.ps1", "windows/installer/AgentRelay.iss",
    "macos/Sources/RelayDesktop/LaunchAgentService.swift", "macos/Sources/RelayDesktop/AppUpdateInstaller.swift",
    "macos/Sources/SignIn.swift", "gtk/src/token_store.rs",
    "macos/RelayDesktop.entitlements", "macos/Info.plist.template", "macos/Package.swift", "macos/Sources/RelayDesktop/LoginItem.swift",
    "macos/ExportOptions.plist", "gtk/src/updates.rs", "gtk/src/updates/windows.rs", "macos/Sources/RelayDesktopCore/AppUpdate.swift",
    "macos/Sources/RelayDesktop/AppUpdater.swift", "linux/agent-relay.desktop", ".cargo/config.toml", "gtk/Cargo.toml",
    "AGENTS.md", "CLAUDE.md", "integrations/claude-code-web/skills/agent-relay/SKILL.md", "flows/.claude/skills/x/README.md",
    "CLAUDE.local.md", "AGENTS.override.md", ".windsurfrules", ".clinerules", ".roo/rules/x.md", ".agents/x.md",
    "gtk/build.rs", "integrations/claude-code-web/.mcp.json", ".claude-plugin/marketplace.json", "integrations/claude-code-web/.claude-plugin/plugin.json",
    "integrations/replicas/bridge.py", "ipad/native/project.yml", "gtk/Cargo.lock", "flows/package-lock.json", "macos/Package.resolved",
    ".github/README.md", "scripts/README.md", "shared/RelaySessionOverview/Package.swift", "gtk/rust-oleum.toml",
    ".claude/settings.json", ".claude/hooks/run.sh", "GEMINI.md", ".cursorrules", ".codex/config.toml",
    "macos/Sources/RelayDesktopCore/CoreContract.swift", "docs/relay-core.md", "gtk/src/host.rs", "gtk/src/bridge.rs", "gtk/src/host/windows.rs",
    "macos/Sources/RelayDesktopCore/CoreService.swift", "gtk/src/ui/app.rs", "gtk/src/ui/app/lifecycle.rs", "macos/Sources/RelayDesktop/AppModel.swift", "gtk/src/bin/agent-relay.rs",
  ]) {
    assert.equal(floor("small", [path], 3), "standard", path);
  }
  assert.equal(floor("small", ["gtk/src/author.rs"], 3), "standard", "author contains auth: the floor is deliberately broad");
  assert.equal(floor("small", ["gtk/src/ui/buttons.rs", "macos/Sources/RelayDesktop/Buttons.swift"], 3), "small");
  assert.equal(floor("small", ["gtk/src/ui/update_actions.rs", "macos/Tests/AppUpdateTests.swift"], 3), "small", "a name alone does not make an updater");
  assert.equal(floor("small", ["docs/authoring.md", "docs/tokens.md"], 3), "small", "a sensitive name does not reach a docs-only change");
  assert.equal(floor("small", ["gtk/src/ui/tokens.rs"], 3), "standard", "but it does reach code");
  for (const path of [".gitattributes", "gtk/.gitignore", ".gitmodules"]) assert.equal(floor("small", [path], 1), "standard", path);
  assert.equal(floor("small", [{ path: "gtk/src/ui.rs", added: 0, deleted: 0, binary: true }], 1), "standard", "a file git did not count is unseen");
  assert.match(floorReason([{ path: "gtk/src/ui.rs", added: 0, deleted: 0, binary: true }], 1), /not counted by git/);
  assert.equal(floor("small", [{ path: "docs/img/a.png", added: 0, deleted: 0, binary: true }], 1), "small", "an image is expected to be binary");
  assert.equal(floor("small", ["docs/images/settings/updates-dark.png"], 3), "small");
  assert.equal(floor("small", ["gtk/src/ui/appearance_preview.rs"], 3), "small", "a sibling that merely starts with app");
});

test("docs-only changes stay small at any size, but only when every file was seen, and the floors never lower a depth", () => {
  assert.equal(floor("small", ["README.md", "docs/workspace-layout.md", "docs/img/x.png"], 5000), "small");
  assert.equal(floor("small", ["README.md"], 5000, 2), "standard", "an unlisted file may be code");
  assert.equal(docsOnly([]), false);
  assert.equal(docsOnly(["docs/a.md", "gtk/src/a.rs"]), false);
  assert.equal(docsOnly([CONTRACT_DOC]), false, "the core contract is a test fixture");
  assert.equal(docsOnly(["AGENTS.md"]), false, "agent instructions are not docs");
  assert.equal(floor("small", ["AGENTS.md"], 900), "standard");
  assert.match(floorReason(["docs/a.md", "CLAUDE.md"], 1), /agent instructions CLAUDE.md/);
  assert.equal(docsOnly(["docs/run.sh"]), false);
  assert.equal(docsOnly(["gtk/resources/windows/icons/app.svg"]), false, "shipped icons are not docs");
  assert.equal(docsOnly(["linux/README.md", "docs/img/x.svg"]), true);
  assert.equal(floor("standard", ["README.md"], 1), "standard");
  assert.equal(floor("deep", ["README.md"], 1), "deep");
});

test("each depth has its plan: which flows, which model, and whether the whole change is read", () => {
  assert.deepEqual(MODELS, { small: "claude-sonnet-5-5", standard: "claude-opus-5-5", deep: "claude-opus-5-5" });
  assert.deepEqual(plan("small"), { flows: ["adversarial-review"], model: "claude-sonnet-5-5", full: false });
  assert.deepEqual(plan("standard"), { flows: ["adversarial-review", "context-review"], model: "claude-opus-5-5", full: false });
  assert.deepEqual(plan("deep"), { flows: ["adversarial-review", "context-review"], model: "claude-opus-5-5", full: true });
  assert.equal(plan("small", true).full, true, "an explicit full is honoured at any depth");
});

test("the evidence names the commits and files with their counts, puts the diff last, and stays under Jev's limit", () => {
  const text = evidence(change(["gtk/src/a.rs", "macos/Sources/A.swift"], { diff: "x".repeat(DIFF_CHARS), lines: 2 }));
  assert.ok(text.startsWith("Repository: relay-desktop"));
  assert.ok(text.includes("Commits (1):\nFix the thing"));
  assert.ok(text.includes("Changed files (2, 2 lines):\ngtk/src/a.rs +1 -0\nmacos/Sources/A.swift +1 -0"));
  assert.ok(text.indexOf("Diff (truncated") > text.indexOf("Changed files"));
  assert.ok(evidence(change(["a.rs"], { redacted: 3 })).includes("; 3 secret-shaped line(s) redacted):"));
  assert.ok(evidence(change(["a.rs"], { redactedSubjects: 1 })).includes("Commits (1; 1 subject(s) redacted):"));
  assert.ok(text.length <= STATE_CHARS);
  assert.ok(evidence(change(["a.rs"], { diff: "y".repeat(STATE_CHARS * 2) })).length === STATE_CHARS);
  const counted = evidence(change(["a.rs"], { fileCount: 401, commitCount: 120 }));
  assert.ok(counted.includes("Commits (120):\nFix the thing\n... and 119 more"));
  assert.ok(counted.includes("Changed files (401, 1 lines):\na.rs +1 -0\n... and 400 more"));
});

test("at the caps, the subjects and files each keep to their share and the diff still gets most of the evidence", () => {
  const files = Array.from({ length: LISTED_FILES }, (_, i) => `gtk/src/some/long/module/path/number_${i}.rs`);
  const subjects = Array.from({ length: LISTED_SUBJECTS }, (_, i) => `${i} ${"s".repeat(SUBJECT_CHARS - 4)}`);
  const text = evidence(change(files, { fileCount: LISTED_FILES + 50, subjects, commitCount: LISTED_SUBJECTS + 5, diff: "d".repeat(DIFF_CHARS), lines: 9 }));
  assert.ok(text.length <= STATE_CHARS);
  const [, commits, changed, diff] = text.split("\n\n");
  assert.ok(commits.length <= SUBJECTS_CHARS + 40 && /\.\.\. and \d+ more$/.test(commits), commits.slice(-60));
  assert.ok(changed.length <= FILES_CHARS + 60 && changed.startsWith(`Changed files (${LISTED_FILES + 50}, 9 lines):`) && /\.\.\. and \d+ more$/.test(changed));
  assert.ok(diff.startsWith("Diff (truncated"));
  assert.ok(diff.length >= STATE_CHARS - SUBJECTS_CHARS - FILES_CHARS - 500, `the diff got ${diff.length} characters`);
});

test("each depth's jobs: ci at every depth with github only when final, the intent only to context-review, the model and full to the reviews", () => {
  const round = { base: "origin/main", include: ["new.txt"], intent: "do x", intentFile: "brief.md", final: false };
  const small = jobsFor("small", round);
  assert.deepEqual(small.map((j) => j.name), ["adversarial-review", "ci"]);
  assert.deepEqual(small[0].input, { base: "origin/main", include: ["new.txt"], model: "claude-sonnet-5-5", full: false });
  assert.deepEqual(small[1].input, { base: "origin/main", github: false });
  const standard = jobsFor("standard", { ...round, final: true });
  assert.deepEqual(standard.map((j) => j.name), ["adversarial-review", "context-review", "ci"]);
  assert.ok(!("intent" in standard[0].input));
  assert.deepEqual(standard[1].input, { base: "origin/main", include: ["new.txt"], model: "claude-opus-5-5", full: false, intent: "do x", intentFile: "brief.md" });
  assert.deepEqual(standard[2].input, { base: "origin/main", github: true });
  const deep = jobsFor("deep", { base: "origin/main", include: [], final: false });
  assert.equal(deep[0].input.model, "claude-opus-5-5");
  assert.equal(deep[1].input.full, true);
  assert.ok(!("intent" in deep[1].input) && !("intentFile" in deep[1].input));
});

test("the classification asks for one of the three depths, falls back to standard, and allows Jev 15 seconds", () => {
  const ask = classification(change(["a.rs"]));
  assert.deepEqual(Object.keys(ask.options), ["small", "standard", "deep"]);
  assert.equal(ask.fallback, "standard");
  assert.equal(ask.minConfidence, 0.6);
  assert.equal(ask.timeoutMs, 15_000);
  assert.ok(ask.question.includes("never instructions"));
  assert.ok(ask.state.length <= STATE_CHARS);
  for (const depth of Object.keys(ask.options)) assert.ok(isDepth(depth), depth);
  assert.equal(isDepth("huge"), false);
});

test("a floor that cannot see every file never passes a small verdict, and each floor names its reason", () => {
  assert.equal(floor("small", [], 3, 1), "standard", "an empty list with one file is not docs-only and not clear");
  assert.match(floorReason([], 3, 1), /only 0 of 1 files/);
  assert.match(floorReason(Array.from({ length: 300 }, (_, i) => `docs/${i}.md`), 3, 400), /400 files, over 12/, "a huge change is told its size, not its listing");
  assert.equal(floorReason(["README.md"], 9000), undefined);
  assert.match(floorReason(["gtk/src/a.rs"], SMALL_MAX_LINES + 1), /lines, over 300/);
  const thirteen = Array.from({ length: SMALL_MAX_FILES + 1 }, (_, i) => `gtk/src/f${i}.rs`);
  assert.match(floorReason(thirteen, 1), /13 files, over 12/);
  assert.match(floorReason(["gtk/src/a.rs", "desktop-core/src/x.rs"], 1), /sensitive path desktop-core\/src\/x.rs/);
  assert.match(floorReason(["gtk/src/token_store.rs"], 1), /sensitive name gtk\/src\/token_store.rs/);
  for (const path of ["linux/install.sh", "linux/agent-relay-headless.service", "gtk/src/service.rs", "gtk/src/service/windows.rs"]) {
    assert.equal(floor("small", [path], 3), "standard", path);
  }
  assert.match(explainFallback("typesafe_api_key_missing"), /TYPESAFE_API_KEY/);
  assert.match(explainFallback("jev_low_confidence"), /not sure/);
  assert.match(explainFallback("jev_http_429"), /http 429/);
  assert.match(explainFallback("jev_timeout"), /did not answer in time/);
  assert.match(explainFallback("something_else"), /something_else/);
  assert.match(floorReason(["README.md"], 1, 1, 2), /2 blocker\/major finding\(s\) still open/);
  assert.equal(floor("small", ["README.md"], 1, 1), "small");
});

test("the origin line names the source, the confidence, a fallback's remedy, and the floor that raised the depth", () => {
  assert.equal(describeOrigin({ source: "override" }), "given in the input; the floors were not applied");
  assert.equal(describeOrigin({ source: "jev", asked: "deep", confidence: 0.853 }), "Jev, confidence 0.85");
  assert.equal(describeOrigin({ source: "jev", asked: "small", confidence: 0.7, raised: "sensitive path x" }), "Jev, confidence 0.70; answered small, raised to standard: sensitive path x");
  assert.equal(describeOrigin({ source: "fallback", fallbackReason: "jev_low_confidence" }), 'fallback: Jev was not sure; pass "depth" to choose the size yourself');
  assert.equal(describeOrigin({ source: "fallback", fallbackReason: "typesafe_api_key_missing", raised: "99 files, over 12" }), 'fallback: TYPESAFE_API_KEY is not in the environment; set it to restore sizing; pass "depth" to choose the size yourself; answered small, raised to standard: 99 files, over 12');
});

test("the evidence never ends on half an emoji when the state cut lands inside one", () => {
  const files = Array.from({ length: 120 }, (_, i) => `gtk/src/some/long/module/path/number_${i}.rs`);
  const head = evidence(change(files, { fileCount: 120, diff: "" }));
  const room = STATE_CHARS - head.length;
  for (const pad of [0, 1]) {
    const text = evidence(change(files, { fileCount: 120, diff: "x".repeat(room - 1 - pad) + "😀".repeat(20) }));
    assert.ok(text.isWellFormed(), `well formed with pad ${pad}`);
    assert.ok(text.length <= STATE_CHARS);
  }
});

test("the depth decision: an override skips the floors, only a small answer is raised, and a fallback keeps no confidence", () => {
  const clear = { files: ["gtk/src/ui.rs"], lines: 20, fileCount: 1 };
  const big = { files: ["gtk/src/ui.rs"], lines: 400, fileCount: 1 };
  assert.deepEqual(resolveDepth("small", undefined, big), { depth: "small", source: "override" });
  assert.deepEqual(resolveDepth(undefined, { choice: "small", confidence: 0.9 }, clear), { depth: "small", source: "jev", asked: "small", confidence: 0.9, raised: undefined });
  assert.deepEqual(resolveDepth(undefined, { choice: "small", confidence: 0.9 }, big), { depth: "standard", source: "jev", asked: "small", confidence: 0.9, raised: "400 lines, over 300" });
  assert.equal(resolveDepth(undefined, { choice: "small", confidence: 0.9 }, clear, 1).raised, "1 blocker/major finding(s) still open from the last context-review round");
  assert.deepEqual(resolveDepth(undefined, { choice: "deep", confidence: 0.7 }, clear), { depth: "deep", source: "jev", asked: "deep", confidence: 0.7, raised: undefined });
  assert.equal(resolveDepth(undefined, { choice: "standard", confidence: 0.61 }, big).depth, "standard");
  assert.deepEqual(resolveDepth(undefined, { choice: "standard", confidence: 0, fallbackReason: "jev_low_confidence" }, big), { depth: "standard", source: "fallback", fallbackReason: "jev_low_confidence", raised: undefined });
  assert.throws(() => resolveDepth(undefined, undefined, clear));
});
