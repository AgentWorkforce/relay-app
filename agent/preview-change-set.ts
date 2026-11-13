/**
 * The JSON half of preview-change-set.sh (Agent Deploy's preview job). It never
 * calls AWS: the shell wrapper fetches the JSON with the AWS CLI and passes the
 * files here.
 *
 *   node preview-change-set.ts stack
 *     prints "<compute stack> <region>" from agent.config.ts
 *   node preview-change-set.ts template <deployed-template.json> <describe-stacks.json> <out.json>
 *     prints "adopted" (the pin is already deployed: nothing to preview) or
 *     "preview" after writing the template the change set previews; exits 1
 *     when the update would be a full deploy
 *   node preview-change-set.ts params <template.json>
 *     prints ParameterKey=<key>,UsePreviousValue=true for every parameter but AmiId
 *   node preview-change-set.ts assert <describe-change-set.json> <template.json>
 *     prints the verdict line; exits 1 unless the template carries the pinned
 *     AmiId default and the AgentHost pin marker, the change set modifies
 *     AgentHost in place (Action Modify, Replacement False), nothing is removed
 *     or replaced and AmiId took the template's pinned default. A FAILED "no
 *     changes" change set fails too: assert only runs before the pin is
 *     adopted, and CloudFormation would not persist an empty update. With
 *     PREVIEW_ALLOW_REPLACEMENT=true (the workflow sets it only for a
 *     workflow_dispatch with allow_restart, a human's decision to replace the
 *     host) a replacement (True or Conditional) is listed as "replacement
 *     allowed by allow_restart" instead of failing; removals, a missing
 *     AgentHost change, the status rule and the AmiId check are the same in
 *     both modes
 *
 * "template" mirrors the kit's amiPinAdoption() (src/deploy.ts, 0.1.15)
 * read-only: from a deployed template whose AmiId is SSM-typed, or already the
 * plain pin but without the marker (a 0.1.14 stack), the switched template is
 * the deployed one with AmiId set to the pin and Resources.AgentHost.Metadata
 * gaining LayeredAgentAmiPin (0.1.15 hostTemplate's marker, which makes
 * CloudFormation persist the update; 0.1.14's parameter-only switch was a
 * silent NoUpdateToPerform). It must equal the desired template except
 * UserData, compared semantically (key order does not count), and the stack
 * must already run the pinned AMI (ResolvedValue for SSM, ParameterValue for
 * the plain pin). "adopted" needs both the pinned AmiId and the marker, which
 * is what the kit's verifyAmiPin() reads back after the deploy.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { fileReader, hostTemplate, userDataBase64, withoutUserData } from "@agentworkforce/layered-agent";
import agent from "./agent.config.ts";

// AWS CLI JSON, checked field by field.
type Json = any;

export const INSTANCE = "AgentHost";

/** The AgentHost resource metadata key the kit (0.1.15 hostTemplate) sets to the pinned AMI. */
export const AMI_PIN_MARKER = "LayeredAgentAmiPin";

const SSM_AMI_TYPE = "AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>";

/** The pinned AmiId definition the kit writes (templates.ts amiIdParameter). */
export function pinnedAmiParameter(ami: string) {
  return { Type: "AWS::EC2::Image::Id", Default: ami };
}

/**
 * Paths (never values) at which two JSON values differ, semantically: the
 * same keys in another order are not a difference (isDeepStrictEqual).
 */
export function differingPaths(a: Json, b: Json, path = ""): string[] {
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object" || Array.isArray(a) !== Array.isArray(b)) {
    return isDeepStrictEqual(a, b) ? [] : [path || "/"];
  }
  const out: string[] = [];
  for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) {
    // A key present on one side only differs even when its value is undefined.
    if (Object.hasOwn(a, k) !== Object.hasOwn(b, k)) out.push(`${path}/${k}`);
    else out.push(...differingPaths(a[k], b[k], `${path}/${k}`));
  }
  if (out.length === 0 && !isDeepStrictEqual(a, b)) out.push(path || "/");
  return out;
}

export type TemplateResult = { kind: "adopted" } | { kind: "preview"; template: Json };

/**
 * The template the preview submits, or "adopted" when the deployed template
 * already carries the pinned AmiId and the marker. Throws when the update
 * would be a full deploy.
 */
export function previewTemplate(deployedRaw: Json, stacks: Json, ami: string, desired: Json): TemplateResult {
  const deployed = typeof deployedRaw === "string" ? JSON.parse(deployedRaw) : deployedRaw;
  if (!deployed || typeof deployed !== "object" || !deployed.Parameters || !deployed.Resources) throw new Error("the deployed template has no Parameters or Resources");
  if (!deployed.Resources[INSTANCE] || typeof deployed.Resources[INSTANCE] !== "object") throw new Error(`the deployed template has no ${INSTANCE}: a full deploy`);
  const pinned = pinnedAmiParameter(ami);
  const parameter = deployed.Parameters.AmiId;
  const marker = deployed.Resources[INSTANCE].Metadata?.[AMI_PIN_MARKER];
  const adopted = isDeepStrictEqual(parameter, pinned) && marker === ami;
  const fromSsm = parameter?.Type === SSM_AMI_TYPE;
  // The kit also lets a 0.1.14 stack (plain pin, no marker) acquire the marker.
  const plainWithoutMarker = parameter?.Type === "AWS::EC2::Image::Id" && parameter.Default === ami && marker === undefined;
  if (!adopted && !fromSsm && !plainWithoutMarker) {
    throw new Error(`the deployed AmiId is neither SSM-typed nor the plain pin ${ami} without the ${AMI_PIN_MARKER} marker, nor adopted: a full deploy`);
  }
  // The AMI the host runs: SSM-typed resolves through ResolvedValue, a plain
  // AMI id parameter has none and its ParameterValue is the id.
  const param = (stacks?.Stacks?.[0]?.Parameters ?? []).find((p: Json) => p.ParameterKey === "AmiId");
  const running = fromSsm ? param?.ResolvedValue : param?.ParameterValue;
  if (!running) throw new Error("the stack has no resolved AmiId, so the AMI the host runs is unknown: a full deploy");
  if (running !== ami) throw new Error(`the pin is ${ami} but the host runs ${running}: a full deploy (instance replacement)`);
  if (adopted) return { kind: "adopted" };
  const switched = JSON.parse(JSON.stringify(deployed));
  switched.Parameters.AmiId = pinned;
  switched.Resources[INSTANCE].Metadata = { ...switched.Resources[INSTANCE].Metadata, [AMI_PIN_MARKER]: ami };
  const have = withoutUserData(switched);
  const want = withoutUserData(desired);
  if (!isDeepStrictEqual(have, want)) {
    throw new Error(`the deployed template differs from the desired one beyond UserData, AmiId and the ${AMI_PIN_MARKER} marker (a full deploy) at: ${differingPaths(have, want).join(", ")}`);
  }
  return { kind: "preview", template: switched };
}

/** --parameters for create-change-set: the previous value of all but AmiId, which is left out so it takes the template default (Alchemy does not pass it). */
export function previousParameters(template: Json): string[] {
  return Object.keys(template.Parameters ?? {}).filter((k) => k !== "AmiId").map((k) => `ParameterKey=${k},UsePreviousValue=true`);
}

const NO_CHANGES = /didn't contain changes|No updates are to be performed/i;

export interface Verdict { ok: boolean; line: string; problems: string[]; allowed: string[] }

/**
 * The change-set verdict for a first adoption (assert only runs when the pin
 * is not adopted yet). allowReplacement is the allow_restart policy: a
 * replacement is then recorded in `allowed` (and the line) rather than failing.
 * It never lets a removal, a missing AgentHost change, a failed or empty
 * change set, or a wrong AmiId through.
 */
export function assertChangeSet(cs: Json, template: Json, ami: string, allowReplacement = false): Verdict {
  const problems: string[] = [];
  const allowed: string[] = [];
  const pin = template?.Parameters?.AmiId?.Default;
  if (pin !== ami) problems.push(`the template's AmiId default is ${pin}, not the pin ${ami}`);
  const marker = template?.Resources?.[INSTANCE]?.Metadata?.[AMI_PIN_MARKER];
  if (marker !== ami) problems.push(`the template's ${INSTANCE} ${AMI_PIN_MARKER} marker is ${marker}, not the pin ${ami}`);
  const status = String(cs?.Status ?? "missing");
  const changes: Json[] = Array.isArray(cs?.Changes) ? cs.Changes : [];
  let hostReplacement = "unchanged";
  let amiValue = "n/a";
  let noChanges = false;
  if (status === "FAILED" && NO_CHANGES.test(String(cs?.StatusReason ?? ""))) {
    // An empty update is CloudFormation's NoUpdateToPerform: the pin would
    // not persist, which is the defect the marker fixes.
    noChanges = true;
    problems.push("CloudFormation reports no changes, so the pin would not persist; the stored template still differs from the pin");
  } else if (status !== "CREATE_COMPLETE") {
    problems.push(`change set status ${status}: ${cs?.StatusReason ?? "no reason"}`);
  } else {
    let host: Json;
    for (const c of changes) {
      const rc = c?.ResourceChange ?? {};
      const id = String(rc.LogicalResourceId ?? c?.Type ?? "?");
      if (id === INSTANCE) {
        host = rc;
        hostReplacement = `${rc.Action ?? "?"}/${rc.Replacement ?? "n/a"}`;
      }
      if (rc.Action === "Remove") problems.push(`${id} would be removed`);
      if (rc.Replacement === "True" || rc.Replacement === "Conditional") {
        (allowReplacement ? allowed : problems).push(`${id} replacement is ${rc.Replacement}`);
      }
    }
    // The marker must reach the stack: AgentHost is modified, in place unless
    // allow_restart lets a replacement through (True and Conditional are
    // already failed or listed above; anything else but False fails here).
    if (!host) problems.push(`${INSTANCE} is not changed, so the pin would not persist`);
    else if (host.Action !== "Modify") problems.push(`${INSTANCE} action is ${host.Action}, not Modify`);
    else if (!["False", "True", "Conditional"].includes(host.Replacement)) problems.push(`${INSTANCE} replacement is ${host.Replacement}, not False`);
    const p = (cs?.Parameters ?? []).find((x: Json) => x.ParameterKey === "AmiId");
    amiValue = p ? `${p.ParameterValue}${p.ResolvedValue ? ` (resolved ${p.ResolvedValue})` : ""}` : "missing";
    if (!p) problems.push("the change set has no AmiId parameter");
    else {
      if (p.UsePreviousValue) problems.push("AmiId uses the previous value, not the template default");
      if (p.ParameterValue !== pin) problems.push(`AmiId is ${p.ParameterValue}, not the template default ${pin}`);
    }
  }
  const ok = problems.length === 0;
  const note = allowed.length > 0 ? `, replacement allowed by allow_restart (${allowed.join("; ")})` : "";
  const line = `status ${status}${noChanges ? " (no changes)" : ""}, ${INSTANCE} ${hostReplacement}, AmiId ${amiValue}${note}: ${ok ? "PASS" : `FAIL (${problems.join("; ")})`}`;
  return { ok, line, problems, allowed };
}

const readJson = (file: string): Json => JSON.parse(readFileSync(file, "utf8"));

function main(argv: string[]) {
  const [cmd, ...args] = argv;
  if (!agent.ami) throw new Error("agent.config.ts pins no ami: there is no pin to preview");
  if (cmd === "stack" && args.length === 0) {
    console.log(`${agent.names.computeStack} ${agent.region}`);
  } else if (cmd === "template" && args.length === 3) {
    const [deployed, stacks, out] = args as [string, string, string];
    const desired = hostTemplate(agent, userDataBase64(agent, fileReader(fileURLToPath(new URL(".", import.meta.url)))));
    const result = previewTemplate(readJson(deployed), readJson(stacks), agent.ami, desired);
    if (result.kind === "preview") writeFileSync(out, JSON.stringify(result.template));
    console.log(result.kind);
  } else if (cmd === "params" && args.length === 1) {
    for (const p of previousParameters(readJson(args[0]!))) console.log(p);
  } else if (cmd === "assert" && args.length === 2) {
    const cs = readJson(args[0]!);
    const allow = process.env.PREVIEW_ALLOW_REPLACEMENT ?? "false";
    if (allow !== "true" && allow !== "false") throw new Error(`PREVIEW_ALLOW_REPLACEMENT is ${JSON.stringify(allow)}, not true or false`);
    const verdict = assertChangeSet(cs, readJson(args[1]!), agent.ami, allow === "true");
    console.log(verdict.line);
    if (!verdict.ok) {
      console.error(JSON.stringify(cs.Changes ?? [], null, 2));
      process.exit(1);
    }
  } else {
    console.error("usage: preview-change-set.ts stack | template <deployed.json> <stacks.json> <out.json> | params <template.json> | assert <change-set.json> <template.json>");
    process.exit(64);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
