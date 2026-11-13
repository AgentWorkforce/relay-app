// context-review: a fleet of reviewers that know what the change was meant to
// do and judge it against that: did it do all of it, only it, the way this
// repository does things? The adversarial fleet covers the code cold; this
// one covers fit.
//
//   flows/node_modules/.bin/flows run --local-agent flows/context-review.flow.ts --input '{"intent":"<what was asked>"}'
//   flows/node_modules/.bin/flows run --local-agent flows/context-review.flow.ts --input '{"intentFile":".agentworkforce/brief.md"}'
//
// The intent is the given text and/or file, plus the branch's commit messages.
// The first run reviews the whole change; each rerun only what changed since
// the last one, with its open findings re-checked (`"full": true` for a whole
// review again).
//
// Result: .relayflow/context-review/report.md (and report.json), or under
// $RELAYFLOW_STATE_DIR/context-review/ when set. The intent is never written
// there: it goes to a temporary dir outside the checkout. The run ends
// step_failed while any blocker or major finding survives verification; minor
// ones are listed, not blocking.
// `"model": "<id as the CLI names it>"` runs every reviewer and the verifier
// on that model; absent, each CLI's own default.
import { flow, type Ctx } from "@relayflows/surface";
import { prepare, runReview, shellWord, type Lens, type ReviewInput, type Verdict } from "./lib/review.ts";

interface ContextReviewInput extends ReviewInput {
  /** What the change was asked to do, in the requester's words. */
  intent?: string;
  /** A repo-relative file holding the brief, spec or ticket. */
  intentFile?: string;
}

const LENSES: readonly Lens[] = [
  {
    name: "intent",
    brief:
      "List every requirement the intent states or clearly implies, then check each against the code: done, " +
      "partly done, done differently, or missing. Report each gap and each misreading of the ask.",
  },
  {
    name: "scope",
    brief:
      "Find what the change does that was not asked for: unrelated edits, drive-by refactors, behaviour removed " +
      "or altered as a side effect, leftover debug output, files that look accidentally included.",
  },
  {
    name: "conventions",
    brief:
      "Check the change follows this repository's own rules: read AGENTS.md, CLAUDE.md, the READMEs and " +
      "docs/workspace-layout.md. In particular: a user-facing change on macOS (macos/) has its matching GTK " +
      "change for Linux and Windows (gtk/src) or says why not; a user-visible change adds a bullet " +
      "under ## Unreleased in CHANGELOG.md; contract changes regenerate CoreContract.swift; docs follow layout " +
      "changes. Match new code to the idiom of the code around it.",
  },
  {
    name: "verification",
    brief:
      "Check the tests prove the behaviour the intent asked for, not merely that the code runs. Name each " +
      "asked-for behaviour with no test, and each test that would still pass if that behaviour were broken. " +
      "Run the narrowest relevant tests you can and report real failures.",
  },
  {
    name: "user-facing",
    brief:
      "Judge what a person using Agent Relay sees: wording, errors and their remedies, defaults, settings, " +
      "and whether docs and the changelog describe what the code now does. Wording should match across macOS, " +
      "Linux and Windows unless the platform differs.",
  },
];

const preamble = (intentPath: string, patch: string, files: string, stateRoot: string): string =>
  [
    "You are reviewing a change against what it was meant to do.",
    `What it was meant to do is in ${intentPath}. Read it first.`,
    `The change is the patch in ${patch}; the files it touches are listed in ${files}. Read the patch, and`,
    "read the code around it only as far as you need to understand the changed lines.",
    `Do not read ${stateRoot}/adversarial-review/: that fleet works independently of yours.`,
  ].join("\n");

export default flow<ContextReviewInput>(
  "relay-app.context-review",
  { budget: { wallclock: "45m" } },
  async (f, input) => {
    const base = input?.base ?? "origin/main";
    const said = (await f.run(
      `git log --oneline "$(git merge-base ${shellWord(base)} HEAD)..HEAD" | wc -l`,
    )).trim();
    if (!input?.intent?.trim() && !input?.intentFile && Number(said) === 0) {
      await f.run("echo 'Stopped: no intent given and no commits on the branch to read one from. Pass intent or intentFile.' >&2");
      return f.done("needs_human", { detail: "no intent: pass intent or intentFile, or commit the work so its messages can be read" });
    }

    // Outside the checkout: the adversarial fleet runs beside this one in the
    // same tree and must not come across what the change was for. Removed
    // however the run ends.
    const intentDir = (await f.run('mktemp -d "${TMPDIR:-/tmp}/relay-context-intent.XXXXXX"')).trim();
    let verdict: Verdict;
    try {
      verdict = await review(f, input, base, `${intentDir}/intent.md`);
    } finally {
      await f.run(`rm -rf ${shellWord(intentDir)}`);
    }
    f.done(verdict.reason, { detail: verdict.detail });
  },
);

/** Everything after the intent is written; returns the verdict for done(). */
async function review(f: Ctx, input: ContextReviewInput | undefined, base: string, intentPath: string): Promise<Verdict> {
  const prepared = await prepare(f, "context-review", input);
  const intent = Buffer.from(input?.intent ?? "", "utf8").toString("base64");
  await f.run(
    [
      `out=${shellWord(intentPath)}`,
      `printf '# Intent\\n\\n' > "$out"`,
      `printf %s ${shellWord(intent)} | base64 -d >> "$out"`,
      input?.intentFile
        ? `{ printf '\\n\\n## Brief: %s\\n\\n' ${shellWord(input.intentFile)}; cat -- ${shellWord(input.intentFile)}; } >> "$out"`
        : "true",
      `{ printf '\\n\\n## Branch\\n\\n%s\\n\\n## Commits\\n\\n' "$(git rev-parse --abbrev-ref HEAD)"; ` +
        `git log --no-merges --format='### %s%n%n%b' "$(git merge-base ${shellWord(base)} HEAD)..HEAD"; } >> "$out"`,
    ].join(" && "),
  );
  const clis = input?.clis?.length ? input.clis : ["claude"];
  return runReview(f, {
    flow: "context-review", prepared, lenses: LENSES, clis, model: input?.model, preamble: preamble(intentPath, prepared.patch, prepared.files, prepared.stateRoot),
  });
}
