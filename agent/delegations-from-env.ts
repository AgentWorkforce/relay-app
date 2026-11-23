/**
 * Writes the delegated-approval list (delegations.json, a lane file the kit
 * ships to the host, where delegation-decide.py reads it) from the
 * LAYERED_AGENT_DELEGATIONS variable of the agent-deploy environment, which
 * only the owner can write. The list is not tracked in the repository.
 *
 *   node delegations-from-env.ts <out.json>
 *
 * The variable holds a JSON array of grant entries; version and owners are
 * constants here, so the variable can add grants but never change who the
 * owner is. Unset or blank: the list is empty and no agent holds a permit.
 * Anything else that is not a valid array of valid entries exits 1 and writes
 * nothing. The output is deterministic (fixed key order, 2-space JSON, one
 * trailing newline), written atomically with mode 0600.
 *
 * Workflow logs are public: it prints the number of grants only, and an error
 * names the entry index and the field, never a value.
 */
import { closeSync, fsyncSync, openSync, renameSync, rmSync, writeSync, chmodSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const DELEGATIONS_VERSION = 2;
export const OWNERS: readonly string[] = ["khaliq-gant"];
export const SCOPES: readonly string[] = ["owner-equivalent"];
/** Entry keys in output order (the order the tracked file had). */
export const ENTRY_KEYS = ["agent", "agentId", "expiresAt", "grantedBy", "grantedAt", "reason", "scope", "ownsSpawned", "ownerMessageId"] as const;
export const MAX_ENTRIES = 20;
export const MAX_VALUE_BYTES = 16 * 1024;
export const MAX_GRANT_MS = 30 * 86_400_000;
const MAX_REASON = 500;

export interface Grant {
  agent: string;
  agentId: string;
  expiresAt: string;
  grantedBy: string;
  grantedAt: string;
  reason: string;
  scope: string;
  ownsSpawned: boolean;
  ownerMessageId: string;
}

export interface DelegationsDoc {
  version: number;
  owners: string[];
  delegations: Grant[];
}

/** A refusal; its message never carries a value from the variable. */
export class DelegationsError extends Error {}

const AGENT = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const DIGITS = /^[0-9]{1,20}$/;
const TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/;

/** Milliseconds of a UTC second-precision timestamp that names a real instant, or NaN. */
function instant(value: string): number {
  if (!TIMESTAMP.test(value)) return Number.NaN;
  const ms = Date.parse(value);
  // Date.parse rolls 2026-02-30 over to March: the round trip refuses it.
  if (Number.isNaN(ms) || new Date(ms).toISOString() !== value.replace("Z", ".000Z")) return Number.NaN;
  return ms;
}

function entry(raw: unknown, i: number): Grant {
  const fail = (field: string, why: string): never => { throw new DelegationsError(`entry ${i}: ${field}: ${why}`); };
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) fail("entry", "must be an object");
  const obj = raw as Record<string, unknown>;
  for (const key of Object.keys(obj)) if (!(ENTRY_KEYS as readonly string[]).includes(key)) fail(`key #${Object.keys(obj).indexOf(key)}`, "unknown key");
  for (const key of ENTRY_KEYS) if (!Object.hasOwn(obj, key)) fail(key, "missing");
  const str = (key: (typeof ENTRY_KEYS)[number]): string => {
    const v = obj[key];
    return typeof v === "string" ? v : fail(key, "must be a string");
  };
  const agent = str("agent");
  if (!AGENT.test(agent)) fail("agent", "must be 1-64 letters, digits, '.', '_' or '-', starting with a letter or digit");
  const agentId = str("agentId");
  if (!DIGITS.test(agentId)) fail("agentId", "must be 1-20 digits");
  const ownerMessageId = str("ownerMessageId");
  if (!DIGITS.test(ownerMessageId)) fail("ownerMessageId", "must be 1-20 digits");
  const grantedBy = str("grantedBy");
  if (!OWNERS.includes(grantedBy)) fail("grantedBy", "must be an owner");
  const grantedAt = str("grantedAt");
  const granted = instant(grantedAt);
  if (Number.isNaN(granted)) fail("grantedAt", "must be a UTC timestamp YYYY-MM-DDTHH:MM:SSZ");
  const expiresAt = str("expiresAt");
  const expires = instant(expiresAt);
  if (Number.isNaN(expires)) fail("expiresAt", "must be a UTC timestamp YYYY-MM-DDTHH:MM:SSZ");
  if (!(granted < expires)) fail("expiresAt", "must be after grantedAt");
  if (expires > granted + MAX_GRANT_MS) fail("expiresAt", "must be at most 30 days after grantedAt");
  const reason = str("reason");
  if (reason.trim() === "" || reason.length > MAX_REASON || /[\u0000-\u001f\u007f]/.test(reason)) fail("reason", `must be 1-${MAX_REASON} characters on one line`);
  const scope = str("scope");
  if (!SCOPES.includes(scope)) fail("scope", `must be one of ${SCOPES.join(", ")}`);
  const ownsSpawned = obj.ownsSpawned;
  if (typeof ownsSpawned !== "boolean") fail("ownsSpawned", "must be true or false");
  return { agent, agentId, expiresAt, grantedBy, grantedAt, reason, scope, ownsSpawned: ownsSpawned as boolean, ownerMessageId };
}

/** The document for a value of LAYERED_AGENT_DELEGATIONS (undefined: unset). Throws DelegationsError. */
export function delegationsFromEnv(value: string | undefined): DelegationsDoc {
  const doc: DelegationsDoc = { version: DELEGATIONS_VERSION, owners: [...OWNERS], delegations: [] };
  if (value === undefined || value.trim() === "") return doc;
  if (Buffer.byteLength(value, "utf8") > MAX_VALUE_BYTES) throw new DelegationsError(`the value is larger than ${MAX_VALUE_BYTES} bytes`);
  let parsed: unknown;
  // JSON.parse's message quotes the text: it is never passed on.
  try { parsed = JSON.parse(value); } catch { throw new DelegationsError("the value is not valid JSON"); }
  if (!Array.isArray(parsed)) throw new DelegationsError("the value must be a JSON array of grant entries");
  if (parsed.length > MAX_ENTRIES) throw new DelegationsError(`more than ${MAX_ENTRIES} entries`);
  const ids = new Set<string>();
  const agents = new Set<string>();
  parsed.forEach((raw, i) => {
    const grant = entry(raw, i);
    if (ids.has(grant.agentId)) throw new DelegationsError(`entry ${i}: agentId: same as an earlier entry`);
    if (agents.has(grant.agent)) throw new DelegationsError(`entry ${i}: agent: same as an earlier entry`);
    ids.add(grant.agentId);
    agents.add(grant.agent);
    doc.delegations.push(grant);
  });
  return doc;
}

/** The file's bytes: fixed key order, 2-space JSON, one trailing newline. */
export function render(doc: DelegationsDoc): string {
  const ordered = {
    version: doc.version,
    owners: doc.owners,
    delegations: doc.delegations.map((g) => Object.fromEntries(ENTRY_KEYS.map((k) => [k, g[k]]))),
  };
  return `${JSON.stringify(ordered, null, 2)}\n`;
}

/** Writes text to path atomically (a temporary file beside it, renamed over it), mode 0600. */
export function writeAtomic(path: string, text: string): void {
  const tmp = join(dirname(path), `.${basename(path)}.${process.pid}.tmp`);
  let fd: number | undefined;
  try {
    fd = openSync(tmp, "wx", 0o600);
    writeSync(fd, text);
    fsyncSync(fd);
    closeSync(fd);
    fd = undefined;
    chmodSync(tmp, 0o600);
    renameSync(tmp, path);
  } catch (error) {
    if (fd !== undefined) closeSync(fd);
    rmSync(tmp, { force: true });
    throw error;
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length !== 1 || args[0] === "") {
    console.error("usage: delegations-from-env.ts <out.json>  (reads LAYERED_AGENT_DELEGATIONS)");
    process.exit(64);
  }
  try {
    const doc = delegationsFromEnv(process.env.LAYERED_AGENT_DELEGATIONS);
    writeAtomic(args[0]!, render(doc));
    const n = doc.delegations.length;
    console.log(`delegation list written: ${n} grant${n === 1 ? "" : "s"}`);
  } catch (error) {
    // Only a DelegationsError's text is known to carry no value.
    const code = (error as { code?: unknown }).code;
    const why = error instanceof DelegationsError ? error.message : `could not write the file${typeof code === "string" ? ` (${code})` : ""}`;
    console.error(`refusing LAYERED_AGENT_DELEGATIONS, nothing written: ${why}`);
    process.exit(1);
  }
}
