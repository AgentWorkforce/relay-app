// The lane scripts the user data does not carry (EC2's 16 KiB raw cap):
// setup-lane.sh and load-secrets-lane.sh install them into /opt/agent-host
// from main's agent/ in the reference clone. Their content hash rides in
// the user data as lane-scripts.sha: it is what the deploy's change detection
// compares (the kit sees only embedded files), and what the installer
// trusts (the clone is agent-owned; the user data is not).
//
//   node lane-scripts.ts        rewrite lane-scripts.sha (run after editing them)
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const LANE_SCRIPTS: readonly string[] = ["tick.sh", "review.sh", "release.sh", "hotfix.sh", "disk-cleanup.sh", "cloud-release.sh"];
export const LANE_SCRIPTS_SHA_FILE = "lane-scripts.sha";

/**
 * sha256 of the `sha256sum tick.sh review.sh release.sh hotfix.sh disk-cleanup.sh cloud-release.sh` listing
 * (one "<digest>  <name>" line per script, in LANE_SCRIPTS order), so file
 * boundaries and names are bound, not just the concatenated bytes.
 */
export function laneScriptsSha(read: (name: string) => string | Uint8Array): string {
  const listing = LANE_SCRIPTS.map((name) => `${createHash("sha256").update(read(name)).digest("hex")}  ${name}\n`).join("");
  return createHash("sha256").update(listing).digest("hex");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dir = fileURLToPath(new URL(".", import.meta.url));
  const sha = laneScriptsSha((name) => readFileSync(`${dir}${name}`));
  writeFileSync(`${dir}${LANE_SCRIPTS_SHA_FILE}`, `${sha}\n`);
  console.log(`${LANE_SCRIPTS_SHA_FILE}: ${sha}`);
}
