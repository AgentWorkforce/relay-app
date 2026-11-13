// What counts as part of a change in this checkout, shared by ci.flow.ts,
// sized-review.flow.ts and their tests.
import type { FleetResult } from "./fleet.ts";
import { shellWord } from "./shell.ts";

/**
 * Machine-local state that lives in the checkout but is never part of a
 * change: the relay's workspace and the agent CLIs' settings.
 */
export const MACHINE_LOCAL = [".agentworkforce", ".claude"];
// .gitignore lists the same directories (flows/.claude too, which the CLIs
// create there); this list is what the flows enforce whatever is ignored.

/**
 * New files the change has not staged, anywhere in the tree (a new top-level
 * directory or root file included). Anything else local to one machine
 * belongs in .git/info/exclude.
 */
export const UNTRACKED =
  `git ls-files --others --exclude-standard -- . ${MACHINE_LOCAL.map((p) => `':!:${p}'`).join(" ")} | head -200`;

export const GITHUB_STEP = "Actions";

/** Lane names come from scripts/ci-changes.py's own output, so a lane added there needs no second list here. */
export type Lanes = Record<string, boolean>;

/**
 * The lanes Desktop CI would run for the paths changed from the merge base to
 * the working tree: scripts/ci-changes.py, the same selector Desktop CI uses.
 * An empty diff selects nothing; the script reads empty stdin as "unknown"
 * and would select everything.
 */
export const lanesCommand = (mergeBase: string): string =>
  `mb=${shellWord(mergeBase)} && ` +
  `if [ -z "$(git diff --no-renames --name-only "$mb")" ]; then python3 scripts/ci-changes.py --base "$mb" --head "$mb"; ` +
  `else git diff --no-renames --name-only -z "$mb" | python3 scripts/ci-changes.py; fi`;

/** The selector's `lane=true|false` lines as a map. */
export const parseLanes = (selected: string): Lanes =>
  Object.fromEntries(
    selected
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const [key, value] = line.split("=");
        return [key, value === "true"];
      }),
  );

/** Whether any selected lane starts the Windows runners. */
export const selectsWindows = (lanes: Lanes): boolean => Object.entries(lanes).some(([lane, on]) => on && lane.startsWith("windows"));

/**
 * A red cargo step whose only failures are named tests is green, with that
 * said in its name, when GitHub ran that same command on this operating
 * system (the step is `vouched`) against the same snapshot and passed.
 */
export function underLocalLoad(results: FleetResult[]): FleetResult[] {
  const github = results.find((r) => r.job === "github" && r.step === GITHUB_STEP);
  if (!github?.ok) return results;
  return results.map((r) =>
    !r.ok && r.vouched && r.failedTests?.length
      ? { ...r, ok: true, step: `${r.step} (failed only under local load, passed on GitHub: ${r.failedTests.slice(0, 5).join(", ")}${r.failedTests.length > 5 ? ", ..." : ""})` }
      : r,
  );
}
