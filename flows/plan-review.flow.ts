// plan-review: size a round and write the plan down; never run a reviewer.
// agent/review.sh runs this on the host (Jev needs TYPESAFE_API_KEY, which
// stays here) and submits the plan as an Agent Relay Cloud flow run of
// cloud-review.flow.ts. The sizing is sized-review's, through the same
// lib/plan.ts: Jev's answer through the floors (size, sensitive paths, open
// context-review findings), or the author's `depth` override without them,
// and the same refusals before it: a bad depth or base, untracked files, no
// change, and a standard or deep round with no intent and no commits.
//
//   flows/node_modules/.bin/flows run --local-agent flows/plan-review.flow.ts --input '{"intent":"<what was asked>"}'
//   flows/node_modules/.bin/flows run --local-agent flows/plan-review.flow.ts --input '{"depth":"deep"}'
//   Also honoured: "base" (default origin/main), "intentFile" and "full".
//
// Result: $RELAYFLOW_STATE_DIR/plan/plan.json (or .relayflow/plan/plan.json):
//   { depth, flows, model, full, lanes, windows, origin, source, asked?,
//     confidence?, fallbackReason?, raised?, change: { fileCount, lines,
//     commitCount, mergeBase, snapshot, tree, redacted, redactedSubjects } }
// ending success; or { reason, title, detail } ending needs_human or
// step_failed (as sized-review would have) when the round cannot start.
import { flow } from "@relayflows/surface";
import { lanesCommand, parseLanes, selectsWindows, UNTRACKED } from "./lib/changes.ts";
import { DEPTHS, describeOrigin, isDepth, type ChangeSummary, type Depth } from "./lib/depth.ts";
import { writeFile } from "./lib/fleet.ts";
import { sizeRound } from "./lib/plan.ts";
import { resolveBase, shellWord } from "./lib/review.ts";

interface PlanReviewInput {
  /** The branch the change lands on. Default `origin/main`. */
  base?: string;
  /** What the change was asked to do; only its presence matters here (the intent check). */
  intent?: string;
  /** A repo-relative file holding the brief; only its presence matters here. */
  intentFile?: string;
  /** Plan this depth instead of asking Jev. */
  depth?: Depth;
  /** Review the whole change at any depth. */
  full?: boolean;
}

export default flow<PlanReviewInput>(
  "relay-desktop.plan-review",
  { budget: { wallclock: "15m" } },
  async (f, input) => {
    const base = input?.base ?? "origin/main";
    const dir = (await f.run("node flows/lib/state-dir.mjs plan")).trim();
    await f.run(`rm -rf ${shellWord(dir)} && mkdir -p ${shellWord(dir)}`);
    const bail = async (reason: "step_failed" | "needs_human", title: string, detail: string) => {
      await f.run(writeFile(`${dir}/plan.json`, JSON.stringify({ reason, title, detail }, null, 2)));
      return f.done(reason, { detail });
    };
    if (input?.depth !== undefined && !isDepth(input.depth)) {
      return bail("needs_human", "bad depth", `depth must be one of ${DEPTHS.join(", ")}, not ${JSON.stringify(input.depth)}`);
    }
    const badBase = await resolveBase(f, base);
    if (badBase) return bail("needs_human", "bad base", badBase);
    const change = JSON.parse(await f.run(`node flows/lib/change-summary.mjs ${shellWord(base)}`, { timeout: "2m" })) as ChangeSummary;
    const untracked = (await f.run(UNTRACKED)).split("\n").filter(Boolean);
    if (untracked.length) {
      return bail("needs_human", "untracked files", `CI tests only staged files; stage ${untracked.slice(0, 5).join(", ")}${untracked.length > 5 ? ", ..." : ""} (or list machine-local ones in .git/info/exclude) and run again`);
    }
    if (change.fileCount === 0) return bail("step_failed", "nothing to review", "nothing to review: no changes against the base");
    const lanes = parseLanes(await f.run(lanesCommand(change.mergeBase), { timeout: "2m" }));
    const open = Number((await f.run(`node flows/lib/review-state.mjs open context-review ${shellWord(change.mergeBase)}`)).trim()) || 0;
    const { decision, plan } = await sizeRound(f, input?.depth, change, open, input?.full === true);
    const { depth, source, asked, confidence, fallbackReason, raised } = decision;
    if (plan.flows.includes("context-review") && !input?.intent?.trim() && !input?.intentFile && change.commitCount === 0) {
      return bail("needs_human", "no intent", `${depth} review needs an intent: pass intent or intentFile, commit the work so its messages can be read, or pass depth:"small" for a review without one`);
    }
    const origin = describeOrigin({ source, asked, confidence, fallbackReason, raised });
    const planned = {
      depth, flows: plan.flows, model: plan.model, full: plan.full, lanes, windows: selectsWindows(lanes),
      origin, source, asked, confidence, fallbackReason, raised,
      change: {
        fileCount: change.fileCount, lines: change.lines, commitCount: change.commitCount, mergeBase: change.mergeBase,
        snapshot: change.snapshot, tree: change.tree, redacted: change.redacted, redactedSubjects: change.redactedSubjects,
      },
    };
    await f.run(writeFile(`${dir}/plan.json`, JSON.stringify(planned, null, 2)));
    f.done("success", { detail: `${depth} (${origin}) on ${plan.model}: ${plan.flows.join(", ")}` });
  },
);
