/**
 * The delegation list leaves the repository: Agent Deploy writes
 * delegations.json from the agent-deploy environment's
 * LAYERED_AGENT_DELEGATIONS secret with delegations-from-env.ts. The
 * generator is run for real (node, a temporary directory); the fixtures are
 * synthetic. Workflow logs are public, so no refusal and no success line may
 * carry a value from the secret.
 */
import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { DelegationsError, MAX_ENTRIES, MAX_VALUE_BYTES, UNSET_WARNING, delegationsFromEnv, render } from "../delegations-from-env.ts";

const script = fileURLToPath(new URL("../delegations-from-env.ts", import.meta.url));
const repo = fileURLToPath(new URL("../..", import.meta.url));
const source = (name: string) => readFileSync(new URL(`../${name}`, import.meta.url), "utf8");

const grant = (over: Record<string, unknown> = {}) => ({
  agent: "example-bot",
  agentId: "100000000000000001",
  expiresAt: "2026-12-19T00:00:00Z",
  grantedBy: "khaliq-gant",
  grantedAt: "2026-11-19T10:21:40Z",
  reason: "Synthetic fixture grant.",
  scope: "owner-equivalent",
  ownsSpawned: false,
  ownerMessageId: "200000000000000001",
  ...over,
});
const second = grant({ agent: "example-bot-2", agentId: "100000000000000002", ownsSpawned: true, ownerMessageId: "200000000000000002" });

interface Run { status: number | null; stdout: string; stderr: string; file: string | null; mode: number | null; dir: string[] }

/**
 * Runs the CLI with the secret set to value (undefined: unset) into a fresh
 * directory; flags go before the output path. link: run it through a symlink
 * to the script, made in the same directory.
 */
function run(value: string | undefined, existing?: string, flags: string[] = [], link = false): Run {
  const dir = mkdtempSync(join(tmpdir(), "delegations-"));
  try {
    const out = join(dir, "delegations.json");
    if (existing !== undefined) writeFileSync(out, existing);
    const env: NodeJS.ProcessEnv = { ...process.env };
    delete env.LAYERED_AGENT_DELEGATIONS;
    if (value !== undefined) env.LAYERED_AGENT_DELEGATIONS = value;
    let program = script;
    if (link) {
      program = join(dir, "linked-generator.ts");
      symlinkSync(script, program);
    }
    const r = spawnSync(process.execPath, [program, ...flags, out], { env, encoding: "utf8" });
    if (link) rmSync(program);
    return {
      status: r.status, stdout: r.stdout, stderr: r.stderr,
      file: existsSync(out) ? readFileSync(out, "utf8") : null,
      mode: existsSync(out) ? statSync(out).mode & 0o777 : null,
      dir: readdirSync(dir),
    };
  } finally { rmSync(dir, { recursive: true, force: true }); }
}

const empty = `${JSON.stringify({ version: 2, owners: ["khaliq-gant"], delegations: [] }, null, 2)}\n`;

test("unset or blank: the empty list, mode 0600, the count only", () => {
  for (const value of [undefined, "", "  \n\t "]) {
    const r = run(value);
    assert.equal(r.status, 0, r.stderr);
    assert.equal(r.file, empty, JSON.stringify(value));
    assert.equal(r.mode, 0o600);
    assert.deepEqual(r.dir, ["delegations.json"], "no temporary file is left");
    assert.equal(r.stdout, "delegation list written: 0 grants\n");
  }
});

test("one and two entries: fixed key order, 2-space JSON, one trailing newline, the count only", () => {
  const one = run(JSON.stringify([grant()]));
  assert.equal(one.status, 0, one.stderr);
  assert.equal(one.file, `${JSON.stringify({ version: 2, owners: ["khaliq-gant"], delegations: [grant()] }, null, 2)}\n`);
  assert.equal(one.mode, 0o600);
  assert.equal(one.stdout, "delegation list written: 1 grant\n");
  assert.equal(one.stderr, "");

  // Keys given in another order come out in the fixed order.
  const shuffled = Object.fromEntries(Object.entries(second).reverse());
  const two = run(JSON.stringify([grant(), shuffled]));
  assert.equal(two.status, 0, two.stderr);
  assert.equal(two.file, `${JSON.stringify({ version: 2, owners: ["khaliq-gant"], delegations: [grant(), second] }, null, 2)}\n`);
  assert.equal(two.stdout, "delegation list written: 2 grants\n");
  for (const r of [one, two]) {
    assert.doesNotMatch(r.stdout + r.stderr, /example-bot|100000000000000001|200000000000000001|Synthetic/);
  }
});

test("the bytes do not depend on whitespace or key order in the secret, and a rerun replaces the file", () => {
  const compact = run(JSON.stringify([grant(), second]));
  // Spaces inside the line and whitespace (a trailing newline too) at both ends are fine.
  const spaced = run(`\n ${JSON.stringify([Object.fromEntries(Object.entries(grant()).reverse()), second], null, 4).replace(/\n\s*/g, " ")} \n`);
  assert.equal(spaced.status, 0, spaced.stderr);
  assert.equal(compact.file, spaced.file);
  assert.equal(compact.file, render(delegationsFromEnv(JSON.stringify([grant(), second]))));
  const over = run(JSON.stringify([grant()]), "stale\n");
  assert.equal(over.file, render(delegationsFromEnv(JSON.stringify([grant()]))));
  assert.equal(over.mode, 0o600);
});

const refusals: Array<[string, string, RegExp]> = [
  ["JSON over several lines", JSON.stringify([grant()], null, 2), /contains a line break: the secret must be compact JSON on one line/],
  ["a CR inside the line", `[${JSON.stringify(grant())},\r${JSON.stringify(second)}]`, /contains a line break: the secret must be compact JSON on one line/],
  ["an LF inside the line, whitespace around", ` [${JSON.stringify(grant())},\n${JSON.stringify(second)}]\n`, /contains a line break: the secret must be compact JSON on one line/],
  ["not JSON", "[{\"agent\": \"example-bot\"", /not valid JSON/],
  ["an object, not an array", JSON.stringify(grant()), /must be a JSON array/],
  ["null", "null", /must be a JSON array/],
  ["an entry that is not an object", JSON.stringify(["example-bot"]), /entry 0: entry: must be an object/],
  ["an entry that is an array", JSON.stringify([[]]), /entry 0: entry: must be an object/],
  ["an unknown key", JSON.stringify([{ ...grant(), extra: "x" }]), /entry 0: key #9: unknown key/],
  ["a missing key", JSON.stringify([Object.fromEntries(Object.entries(grant()).filter(([k]) => k !== "reason"))]), /entry 0: reason: missing/],
  ["a number agentId", JSON.stringify([grant({ agentId: 100000000000000001 })]), /entry 0: agentId: must be a string/],
  ["agentId with a letter", JSON.stringify([grant({ agentId: "10000000000000000a" })]), /entry 0: agentId: must be 1-20 digits/],
  ["an empty agentId", JSON.stringify([grant({ agentId: "" })]), /entry 0: agentId: must be 1-20 digits/],
  ["agentId of 21 digits", JSON.stringify([grant({ agentId: "100000000000000000001" })]), /entry 0: agentId: must be 1-20 digits/],
  ["ownerMessageId with a sign", JSON.stringify([grant({ ownerMessageId: "-200000000000000001" })]), /entry 0: ownerMessageId: must be 1-20 digits/],
  ["an agent name with a space", JSON.stringify([grant({ agent: "example bot" })]), /entry 0: agent: must be/],
  ["an empty agent name", JSON.stringify([grant({ agent: "" })]), /entry 0: agent: must be/],
  ["grantedBy not an owner", JSON.stringify([grant({ grantedBy: "example-other" })]), /entry 0: grantedBy: must be an owner/],
  ["an unparseable grantedAt", JSON.stringify([grant({ grantedAt: "yesterday" })]), /entry 0: grantedAt: must be a UTC timestamp/],
  ["a grantedAt that is not a real day", JSON.stringify([grant({ grantedAt: "2026-02-30T00:00:00Z" })]), /entry 0: grantedAt: must be a UTC timestamp/],
  ["a non-UTC expiresAt", JSON.stringify([grant({ expiresAt: "2026-12-19T00:00:00+01:00" })]), /entry 0: expiresAt: must be a UTC timestamp/],
  ["expiresAt equal to grantedAt", JSON.stringify([grant({ expiresAt: "2026-11-19T10:21:40Z" })]), /entry 0: expiresAt: must be after grantedAt/],
  ["expiresAt before grantedAt", JSON.stringify([grant({ expiresAt: "2026-11-18T00:00:00Z" })]), /entry 0: expiresAt: must be after grantedAt/],
  ["a grant longer than 30 days", JSON.stringify([grant({ expiresAt: "2026-12-19T10:21:41Z" })]), /entry 0: expiresAt: must be at most 30 days after grantedAt/],
  ["another scope", JSON.stringify([grant({ scope: "release-only" })]), /entry 0: scope: must be one of owner-equivalent/],
  ["ownsSpawned as a string", JSON.stringify([grant({ ownsSpawned: "true" })]), /entry 0: ownsSpawned: must be true or false/],
  ["an empty reason", JSON.stringify([grant({ reason: " " })]), /entry 0: reason: must be/],
  ["a reason over two lines", JSON.stringify([grant({ reason: "one\ntwo" })]), /entry 0: reason: must be/],
  ["a reason that is too long", JSON.stringify([grant({ reason: "x".repeat(501) })]), /entry 0: reason: must be/],
  ["two entries with the same agentId", JSON.stringify([grant(), grant({ agent: "example-bot-2" })]), /entry 1: agentId: same as an earlier entry/],
  ["two entries with the same agent", JSON.stringify([grant(), grant({ agentId: "100000000000000002" })]), /entry 1: agent: same as an earlier entry/],
  ["a bad second entry", JSON.stringify([grant(), { ...second, scope: 1 }]), /entry 1: scope: must be a string/],
  ["too many entries", JSON.stringify(Array.from({ length: MAX_ENTRIES + 1 }, (_, i) => grant({ agent: `example-bot-${i}`, agentId: `1000000000000000${String(i).padStart(2, "0")}` }))), new RegExp(`more than ${MAX_ENTRIES} entries`)],
  ["a value that is too large", JSON.stringify([grant({ reason: "x".repeat(MAX_VALUE_BYTES) })]), new RegExp(`larger than ${MAX_VALUE_BYTES} bytes`)],
];

for (const [label, value, why] of refusals) {
  test(`refuses ${label}: exit 1, nothing written, no value in the output`, () => {
    assert.throws(() => delegationsFromEnv(value), DelegationsError);
    for (const existing of [undefined, "previous\n"]) {
      const r = run(value, existing);
      assert.equal(r.status, 1, r.stderr);
      assert.equal(r.stdout, "");
      assert.match(r.stderr, why);
      assert.match(r.stderr, /^refusing LAYERED_AGENT_DELEGATIONS, nothing written: /);
      assert.equal(r.file, existing ?? null, "an existing file is left as it was, none is created");
      assert.deepEqual(r.dir, existing === undefined ? [] : ["delegations.json"], "no temporary file is left");
      // No value from the secret reaches the output: names, ids, reasons, the raw text.
      assert.doesNotMatch(r.stderr, /example|1000000000000000|2000000000000000|Synthetic|xxxx|yesterday|release-only|2026-/);
      assert.ok(!r.stderr.includes(value.slice(0, 12)) || value.length < 4, "the raw value is not echoed");
    }
  });
}

test("--warn-unset: unset or blank warns once, still writes the empty list and exits 0", () => {
  for (const value of [undefined, "", " \n "]) {
    const r = run(value, undefined, ["--warn-unset"]);
    assert.equal(r.status, 0, r.stderr);
    assert.equal(r.file, empty);
    assert.equal(r.mode, 0o600);
    assert.equal(r.stdout, `${UNSET_WARNING}\ndelegation list written: 0 grants\n`);
    assert.match(r.stdout, /^::warning::The LAYERED_AGENT_DELEGATIONS secret .*the delegation list is empty and no agent holds a permit\.$/m);
    assert.equal(r.stderr, "");
  }
});

test("--warn-unset: an explicit [] or a list writes without a warning; without the flag unset does not warn", () => {
  const explicit = run("[]", undefined, ["--warn-unset"]);
  assert.equal(explicit.status, 0, explicit.stderr);
  assert.equal(explicit.file, empty);
  assert.equal(explicit.stdout, "delegation list written: 0 grants\n");
  const one = run(JSON.stringify([grant()]), undefined, ["--warn-unset"]);
  assert.equal(one.status, 0, one.stderr);
  assert.equal(one.stdout, "delegation list written: 1 grant\n");
  const plain = run(undefined);
  assert.equal(plain.status, 0, plain.stderr);
  assert.doesNotMatch(plain.stdout + plain.stderr, /::warning::/);
});

test("a run through a symlinked path writes the file", () => {
  const r = run(JSON.stringify([grant()]), undefined, [], true);
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.file, render(delegationsFromEnv(JSON.stringify([grant()]))));
  assert.equal(r.stdout, "delegation list written: 1 grant\n");
});

test("a grant of exactly 30 days is accepted; one second longer is refused", () => {
  const exact = grant({ grantedAt: "2026-11-19T10:21:40Z", expiresAt: "2026-12-19T10:21:40Z" });
  const r = run(JSON.stringify([exact]));
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.file, `${JSON.stringify({ version: 2, owners: ["khaliq-gant"], delegations: [exact] }, null, 2)}\n`);
  const longer = run(JSON.stringify([{ ...exact, expiresAt: "2026-12-19T10:21:41Z" }]));
  assert.equal(longer.status, 1);
  assert.equal(longer.file, null);
  assert.match(longer.stderr, /entry 0: expiresAt: must be at most 30 days after grantedAt/);
});

// The generator checks the shape, not the clock: delegation-decide.py on the
// host refuses an expired grant when it is used.
test("a grant that has already expired is accepted and written", () => {
  const expired = grant({ grantedAt: "2020-01-01T00:00:00Z", expiresAt: "2020-01-02T00:00:00Z" });
  const r = run(JSON.stringify([expired]));
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.file, `${JSON.stringify({ version: 2, owners: ["khaliq-gant"], delegations: [expired] }, null, 2)}\n`);
  assert.equal(r.stdout, "delegation list written: 1 grant\n");
});

test("usage: exactly one output path, the only flag is --warn-unset before it", () => {
  for (const args of [[], ["a", "b"], ["--warn-unset"], ["--warn-unset", "a", "b"], ["a", "--warn-unset"], ["--other", "a"]]) {
    const r = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
    assert.equal(r.status, 64);
    assert.match(r.stderr, /^usage: /);
  }
});

const git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" });

test("delegations.json is not tracked, is ignored, and is still a lane file", () => {
  assert.equal(git("ls-files", "--", "agent/delegations.json"), "");
  assert.equal(git("check-ignore", "agent/delegations.json").trim(), "agent/delegations.json");
  assert.match(source("agent.config.ts"), /^  laneFiles: \[[^\]\n]*"delegations\.json"[^\]\n]*\],$/m);
});

// A grant entry carries an "agentId" with a real id. Fixtures use obviously
// synthetic ids: a run of at least ten zeros.
test("no tracked file under agent/, docs/, flows/, scripts/ or .github/ carries a real-looking agentId", () => {
  const hits = git("grep", "-n", "-I", "-o", "-E", "agentId\"?[[:space:]]*[:=][[:space:]]*\"?[0-9]+", "--", "agent", "docs", "flows", "scripts", ".github").split("\n").filter(Boolean);
  const real = hits.filter((line) => !/0{10}/.test(line.replace(/^.*?agentId/, "")));
  // File and line only: the value is not printed.
  assert.deepEqual(real.map((line) => line.split(":").slice(0, 2).join(":")), []);
});

test("Agent Deploy writes the list from the secret with the same step in preview and deploy, before the kit reads agent/", () => {
  const workflow = readFileSync(join(repo, ".github/workflows/agent-deploy.yml"), "utf8");
  const step = [
    "      - name: Write the delegation list",
    "        env:",
    "          LAYERED_AGENT_DELEGATIONS: ${{ secrets.LAYERED_AGENT_DELEGATIONS }}",
    "        run: node agent/delegations-from-env.ts --warn-unset agent/delegations.json",
  ].join("\n");
  const job = (name: string) => {
    const start = workflow.indexOf(`\n  ${name}:\n`);
    assert.ok(start >= 0, name);
    const next = workflow.slice(start + 1).search(/\n  [a-z][a-z-]*:\n/);
    return next < 0 ? workflow.slice(start) : workflow.slice(start, start + 1 + next);
  };
  const preview = job("preview");
  const deploy = job("deploy");
  for (const [name, text, reader] of [["preview", preview, "- name: Preview the compute stack change set"], ["deploy", deploy, "- name: Deploy"]] as const) {
    assert.equal(text.split(step).length, 2, `${name} has the step once`);
    const at = text.indexOf(step);
    assert.ok(text.indexOf("- name: Check out\n") < at, `${name}: after the checkout`);
    assert.ok(at < text.indexOf(reader), `${name}: before ${reader}`);
    assert.ok(text.includes("node-version: '24'") && text.indexOf("node-version: '24'") < at, `${name}: node 24 is set up first`);
    // An environment secret resolves only in a job that names the environment.
    assert.match(text, /^    environment: agent-deploy$/m, `${name} runs in the agent-deploy environment`);
  }
  // The value reaches the step only through env, from a secret, never interpolated into a script.
  assert.equal(workflow.split("secrets.LAYERED_AGENT_DELEGATIONS").length, 3);
  assert.doesNotMatch(workflow, /vars\.LAYERED_AGENT_DELEGATIONS/);
  const ci = readFileSync(join(repo, ".github/workflows/agent-ci.yml"), "utf8");
  assert.doesNotMatch(ci, /LAYERED_AGENT_DELEGATIONS|^\s*environment:/m);
  assert.doesNotMatch(ci, /delegations-from-env\.ts --warn-unset/, "Agent CI does not warn");
  assert.match(ci, /node agent\/delegations-from-env\.ts agent\/delegations\.json/);
});
