// What a round runs: the depth (Jev's answer through the floors, or the
// author's override) and the plan that depth picks. Shared by
// sized-review.flow.ts, which runs the plan on this host, and
// plan-review.flow.ts, which only writes it down for agent/review.sh to submit
// as a Cloud run (cloud-review.flow.ts). The policy itself is lib/depth.ts.
import type { Ctx } from "@relayflows/surface";
import { classification, MODELS, resolveDepth, type ChangeSummary, type Decision, type Depth } from "./depth.ts";

export type ReviewFlow = "adversarial-review" | "context-review";

export interface Plan {
  /** The review flows the round runs; ci.flow.ts runs at every depth. */
  flows: readonly ReviewFlow[];
  model: string;
  /** Review the whole change, not only what changed since the last round. */
  full: boolean;
}

/** What each depth runs: a deep round always reads the whole change. */
export const plan = (depth: Depth, full = false): Plan => ({
  flows: depth === "small" ? ["adversarial-review"] : ["adversarial-review", "context-review"],
  model: MODELS[depth],
  full: full || depth === "deep",
});

/**
 * Size a round: the given depth skips Jev and the floors (the author's call);
 * otherwise Jev classifies the change and a small answer is floored, also by
 * the blocker or major findings a context-review left open (`open`).
 */
export async function sizeRound(
  f: Ctx,
  given: Depth | undefined,
  change: ChangeSummary,
  open: number,
  full = false,
): Promise<{ decision: Decision; plan: Plan }> {
  const decision = resolveDepth(given, given ? undefined : await f.classify(classification(change)), change, open);
  return { decision, plan: plan(decision.depth, full) };
}

/** The inputs each flow of a round gets, so a test can hold the plan to the depth. */
export function jobsFor(
  depth: Depth,
  round: { base: string; include: string[]; full?: boolean; intent?: string; intentFile?: string; final: boolean },
): { name: string; input: Record<string, unknown> }[] {
  const { flows, model, full } = plan(depth, round.full);
  const review = { base: round.base, include: round.include, model, full };
  return [
    ...flows.map((name) => ({
      name,
      input:
        name === "context-review"
          ? { ...review, ...(round.intent ? { intent: round.intent } : {}), ...(round.intentFile ? { intentFile: round.intentFile } : {}) }
          : review,
    })),
    { name: "ci", input: { base: round.base, github: round.final } },
  ];
}
