// sized-review: one review round, no bigger than the change needs. Jev reads
// the change and picks a depth; the depth picks which of the existing flows
// run, on which model; they all run at once and their verdicts are reported
// together. The other flows' logic lives in them: this one only sizes and
// starts them.
//
//   flows/node_modules/.bin/flows run --local-agent flows/sized-review.flow.ts --input '{"intent":"<what was asked>"}'
//   flows/node_modules/.bin/flows run --local-agent flows/sized-review.flow.ts --input '{"intent":"...","final":true}'   the GitHub gate
//   flows/node_modules/.bin/flows run --local-agent flows/sized-review.flow.ts --input '{"depth":"deep"}'              an override
//   Also honoured: "base" (the branch the change lands on, default origin/main)
//   and "full": true (review the whole change at any depth).
//
// Depth -> flows (flows/lib/plan.ts plan):
//   small     adversarial-review on claude-sonnet-5-5
//   standard  adversarial-review and context-review on claude-opus-5-5
//   deep      both, on claude-opus-5-5, over the whole change every round
//   ci.flow.ts at every depth; with `final`, GitHub Actions too.
//
// The depth comes from Jev (flows/lib/depth.ts: the question, the criteria,
// and the floors a small verdict must clear) over a summary of the change
// from the merge base to the same snapshot of the working tree the reviews
// read (flows/lib/change-summary.mjs); a small answer is raised to standard
// by the size and sensitive-path floors, and by blocker or major findings a
// context-review left open last round, since only that flow re-checks them.
// That summary is sent to TypeSafe
// (api.typesafe.ai); its diff leaves out the private core (desktop-core/),
// lockfiles and credential-shaped files, which still count in the file list
// and the floors, and any line that holds a secret is redacted whatever file
// it is in, with the counts in the report. Without TYPESAFE_API_KEY, or when
// Jev is not sure, the round runs at standard. A `depth` in the input skips
// Jev and the floors: it is the author's call.
//
// Result: .relayflow/sized-review/report.md (and report.json), or under
// $RELAYFLOW_STATE_DIR/sized-review/ when set: the depth and where it came
// from, the CI lanes the change selects, and each flow's verdict with its own
// report's path. The run ends step_failed while any flow does, if the tree
// changed while they ran, or if a newer round took the checkout over; it ends
// needs_human, with the remedy, for a bad depth, a base that cannot be
// fetched, untracked files, or a standard or deep round with no intent and no
// commits to read one from; and step_failed with "nothing to review" for a
// branch with no change against the base. Every outcome leaves a report,
// except a round whose checkout a newer round took over, whose report would
// be stale: it ends step_failed and the newer round's report stands.
import { flow } from "@relayflows/surface";
import { lanesCommand, parseLanes, selectsWindows, UNTRACKED } from "./lib/changes.ts";
import { DEPTHS, describeOrigin, isDepth, type ChangeSummary, type Depth } from "./lib/depth.ts";
import { preempted, runFleet, writeFile, type FleetResult } from "./lib/fleet.ts";
import { jobsFor, sizeRound } from "./lib/plan.ts";
import { resolveBase, shellWord, type ReviewInput } from "./lib/review.ts";

// No `include`: CI tests only what is staged, so a round with an untracked
// file stops and says so; stage new files instead.
interface SizedReviewInput extends Omit<ReviewInput, "clis" | "model" | "include"> {
  /** What the change was asked to do, for context-review. Without it, the branch's commit messages are the intent. */
  intent?: string;
  /** A repo-relative file holding the brief, spec or ticket, for context-review. */
  intentFile?: string;
  /** Review at this depth instead of asking Jev. */
  depth?: Depth;
  /** The final gate: the CI flow also runs GitHub Actions. Default false. */
  final?: boolean;
}

// A review flow's budget is 45 minutes and the CI flow's 150, of which it
// polls its own fleet for up to 130. The outer fleet polls in 2-minute
// steps for up to 150 minutes: the polls outlast every step's own limit, so a
// step ends on its limit rather than on the poll budget. The polls are short
// because this root's own lease is lost if its renewals stall for 30 seconds,
// and the root only sees that when the running poll returns (lib/fleet.ts):
// a 13-minute poll let deep rounds spend every retry the kernel gives and end
// terminal at the poll's end, the reviews and CI still running with no report.
export const REVIEW_MINUTES = 50;
export const CI_MINUTES = 140;
export const POLL_MINUTES = 2;
export const POLLS = Math.ceil((CI_MINUTES + 10) / POLL_MINUTES);

export default flow<SizedReviewInput>(
  "relay-app.sized-review",
  { budget: { wallclock: "160m" } },
  async (f, input) => {
    const base = input?.base ?? "origin/main";
    const final = input?.final === true;
    const stateRoot = (await f.run("node flows/lib/state-dir.mjs")).trim();
    const dir = (await f.run("node flows/lib/state-dir.mjs sized-review")).trim();
    // An earlier round still running here (one this round preempts, or one
    // whose body is gone) is stopped first, its flows' own fleets with it:
    // whatever this round goes on to run or not, nothing of the old one may
    // keep reviewing or testing beside it. A no-op when nothing runs. (A flow
    // started by hand in this same checkout is stopped too: a checkout hosts
    // one round at a time.)
    const INNER = ["adversarial-review", "context-review", "ci"] as const;
    const stopFleet = (fleet: string) => f.run(`node flows/lib/fleet.mjs stop ${shellWord(fleet)}`, { onNonZero: "record" });
    for (const fleet of [`${dir}/fleet`, ...INNER.map((name) => `${stateRoot}/${name}/fleet`)]) await stopFleet(fleet);
    await f.run(`rm -rf ${shellWord(dir)} && mkdir -p ${shellWord(dir)}`);
    // A round that cannot start still leaves both report files, saying why.
    const bail = async (reason: "step_failed" | "needs_human", title: string, detail: string) => {
      await f.run(`${writeFile(`${dir}/report.md`, `# Review: ${title}\n\n${detail}\n`)} && ${writeFile(`${dir}/report.json`, JSON.stringify({ reason, detail }, null, 2))}`);
      return f.done(reason, { detail });
    };
    if (input?.depth !== undefined && !isDepth(input.depth)) {
      return bail("needs_human", "bad depth", `depth must be one of ${DEPTHS.join(", ")}, not ${JSON.stringify(input.depth)}`);
    }
    const badBase = await resolveBase(f, base);
    if (badBase) return bail("needs_human", "bad base", badBase);
    const summarize = `node flows/lib/change-summary.mjs ${shellWord(base)}`;
    const change = JSON.parse(await f.run(summarize, { timeout: "2m" })) as ChangeSummary;
    const untracked = (await f.run(UNTRACKED)).split("\n").filter(Boolean);
    if (untracked.length) {
      return bail("needs_human", "untracked files", `CI tests only staged files; stage ${untracked.slice(0, 5).join(", ")}${untracked.length > 5 ? ", ..." : ""} (or list machine-local ones in .git/info/exclude) and run again`);
    }
    if (change.fileCount === 0) return bail("step_failed", "nothing to review", "nothing to review: no changes against the base");
    // The lanes as the CI flow selects them: from the working tree.
    const lanes = parseLanes(await f.run(lanesCommand(change.mergeBase), { timeout: "2m" }));

    // Open context-review findings from the last round are re-checked only when that flow runs again.
    const open = Number((await f.run(`node flows/lib/review-state.mjs open context-review ${shellWord(change.mergeBase)}`)).trim()) || 0;
    const { decision, plan: { flows: reviews, model, full } } = await sizeRound(f, input?.depth, change, open, input?.full === true);
    const { depth, source, asked, confidence, fallbackReason, raised } = decision;
    // context-review parks without an intent or commits; say so before the fleet runs.
    const intent = input?.intent?.trim();
    if (reviews.includes("context-review") && !intent && !input?.intentFile && change.commitCount === 0) {
      return bail("needs_human", "no intent", `${depth} review needs an intent: pass intent or intentFile, commit the work so its messages can be read, or pass depth:"small" for a review without one`);
    }

    const flows = "flows/node_modules/.bin/flows run --local-agent --no-observer-link --data-dir .relayflowd/sized-review-fleet";
    const jobs = jobsFor(depth, { base, include: [], full: input?.full === true, intent, intentFile: input?.intentFile, final }).map(({ name, input: flowInput }) => ({
      name,
      steps: [{ name: "run", command: `${flows} flows/${name}.flow.ts --input ${shellWord(JSON.stringify(flowInput))}`, timeoutMinutes: name === "ci" ? CI_MINUTES : REVIEW_MINUTES }],
    }));
    // The intent travels to context-review inside its fleet job under the
    // state root, which the adversarial fleet's preamble puts off limits.
    // The inner fleets are stopped once more right before this round's own
    // start: an earlier round's flows may have started theirs since the first stop.
    for (const name of INNER) await stopFleet(`${stateRoot}/${name}/fleet`);
    const results = await runFleet(f, `${dir}/fleet`, jobs, POLLS, POLL_MINUTES);
    // A newer round took this directory over: its report, not a stale one, belongs here.
    if (preempted(results)) return f.done("step_failed", { detail: "preempted by a newer round in this checkout" });
    const red = results.filter((r) => !r.ok);
    // A red flow may have been stopped mid-run (out of polls, its timeout, or
    // its own budget) with its fleet of reviewer agents, cargo or the GitHub
    // watch still running: stop that too. One that ended on its own left no
    // pids, so this is a no-op for it; and no newer round owns these fleets,
    // or this round would be the preempted one.
    for (const r of red) await stopFleet(`${stateRoot}/${r.job}/fleet`);
    const after = JSON.parse(await f.run(summarize, { timeout: "2m" })) as ChangeSummary;
    const moved = after.tree !== change.tree;

    // A review flow writes its report only when its verifier ran; a round with nothing new to review ends before that.
    const reportOf = async (r: FleetResult): Promise<string> => {
      const path = `${stateRoot}/${r.job}/report.md`;
      const exists = (await f.run(`test -f ${shellWord(path)}`, { onNonZero: "record" })).exitCode === 0;
      const said = r.output.split("\n").filter(Boolean).pop() ?? "";
      return exists ? `${path}${said ? ` (${said})` : ""}` : said || "(no report)";
    };
    const flowLines: string[] = [];
    for (const r of results) flowLines.push(`- ${r.ok ? "PASS" : "FAIL"} ${r.job}: ${await reportOf(r)}`);
    const selection = Object.keys(lanes).filter((lane) => lanes[lane]).join(", ") || "none (always-on checks only)";
    const origin = describeOrigin({ source, asked, confidence, fallbackReason, raised });
    const report = [
      `# Review: ${depth}, ${moved ? "tree changed during the round" : red.length ? `${red.length} flow(s) red` : "green"}`,
      "",
      `Depth: ${depth} (${origin}); reviewers on ${model}${full ? "; whole change reviewed" : ""}`,
      `Change: ${change.fileCount} file(s), ${change.lines} line(s), ${change.commitCount} commit(s); merge base ${change.mergeBase.slice(0, 12)}` +
        `${change.redacted ? `; ${change.redacted} secret-shaped line(s) redacted before sizing` : ""}${change.redactedSubjects ? `; ${change.redactedSubjects} subject(s) redacted` : ""}`,
      `Lanes selected: ${selection}; Windows runners: ${selectsWindows(lanes) ? "yes" : "no"}`,
      final ? "GitHub gate: requested; the CI flow's report says whether any lane needed it" : "GitHub gate: not run; before the squash push, run again with final:true",
      moved ? "The working tree changed while the flows ran: what they reviewed is not what was sized. Run again." : "",
      "",
      ...flowLines,
      ...red.flatMap((r) => ["", `## ${r.job}`, "", ...(r.log ? [`Full log: ${r.log} (stderr: ${r.errLog})`, ""] : []), "```", r.output, "```"]),
    ].join("\n");
    const reason = moved || red.length ? "step_failed" : "success";
    const json = {
      reason, depth, source, asked, confidence, fallbackReason, raised, model, full, final, lanes, windows: selectsWindows(lanes),
      files: change.fileCount, lines: change.lines, commits: change.commitCount, redacted: change.redacted, redactedSubjects: change.redactedSubjects,
      mergeBase: change.mergeBase, snapshot: change.snapshot, tree: change.tree, moved, results,
    };
    await f.run(`${writeFile(`${dir}/report.md`, report)} && ${writeFile(`${dir}/report.json`, JSON.stringify(json, null, 2))}`);

    const summary = `${depth} (${origin}) on ${model}; lanes: ${selection}`;
    if (moved) return f.done("step_failed", { detail: `${summary}; the working tree changed during the round; run again` });
    if (red.length) return f.done("step_failed", { detail: `${summary}; red: ${red.map((r) => r.job).join(", ")}; see ${dir}/report.md` });
    f.done("success", { detail: `${summary}; ${results.length} flow(s) green${final ? "" : "; run again with final:true for the GitHub gate before landing"}` });
  },
);
