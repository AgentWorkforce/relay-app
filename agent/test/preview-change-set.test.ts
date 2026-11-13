/**
 * Agent Deploy's change-set preview (preview-change-set.ts and .sh): the
 * adoption template is built read-only from the deployed one exactly as the
 * kit's amiPinAdoption() (0.1.15) builds it (AmiId pinned plus the AgentHost
 * LayeredAgentAmiPin marker), a full deploy is refused before any change set
 * exists, and the change-set verdict passes only when AgentHost is modified in
 * place, nothing is removed and AmiId took the pinned template default; an
 * empty ("no changes") change set fails, since the pin would not persist. The
 * deployed template is relaycast-cloud main's (the
 * equivalence fixture, read only). The shell runs against a fake `aws` on PATH
 * in a temp dir outside the repository.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { fileReader, hostTemplate, userDataBase64 } from "@agentworkforce/layered-agent";
import agent from "../agent.config.ts";
import { AMI_PIN_MARKER, assertChangeSet, differingPaths, previewTemplate, previousParameters, pinnedAmiParameter } from "../preview-change-set.ts";

const fixture = JSON.parse(readFileSync(new URL("./fixtures/relaycast-cloud-main-desktop-template.json", import.meta.url), "utf8"));
const desired = hostTemplate(agent, userDataBase64(agent, fileReader(fileURLToPath(new URL("..", import.meta.url)))));
const ami = agent.ami!;
const stacks = (params: object[]) => ({ Stacks: [{ Parameters: params }] });
const running = (value: string) => stacks([{ ParameterKey: "AmiId", ParameterValue: fixture.Parameters.AmiId.Default, ResolvedValue: value }]);
const copy = <T>(v: T): T => JSON.parse(JSON.stringify(v));
const plainRunning = (value: string) => stacks([{ ParameterKey: "AmiId", ParameterValue: value }]);
const NO_CHANGES_REASON = "The submitted information didn't contain changes. Submit different information to create a change set.";
/** The fixture as a stack deployed by kit 0.1.14 left it: AmiId plain-pinned, no marker. */
const plain014 = () => ({ ...copy(fixture), Parameters: { ...fixture.Parameters, AmiId: pinnedAmiParameter(ami) } });
/** A template carrying both the pin and the marker, i.e. already adopted. */
const adoptedTemplate = () => {
  const t = plain014();
  t.Resources.AgentHost = { ...t.Resources.AgentHost, Metadata: { [AMI_PIN_MARKER]: ami } };
  return t;
};

/** t without the marker the adoption adds (and without Metadata if that leaves it empty). */
const withoutMarker = (t: typeof fixture) => {
  const c = copy(t);
  const { [AMI_PIN_MARKER]: _marker, ...rest } = c.Resources.AgentHost.Metadata ?? {};
  if (Object.keys(rest).length > 0) c.Resources.AgentHost.Metadata = rest;
  else delete c.Resources.AgentHost.Metadata;
  return c;
};

test("the adoption template is the deployed one with AmiId pinned and exactly the marker added, its UserData kept", () => {
  for (const deployed of [fixture, JSON.stringify(fixture)]) {
    const result = previewTemplate(deployed, running(ami), ami, desired);
    assert.equal(result.kind, "preview");
    const t = (result as { template: typeof fixture }).template;
    assert.deepEqual(t.Parameters.AmiId, pinnedAmiParameter(ami));
    assert.deepEqual(t.Resources.AgentHost.Metadata, { [AMI_PIN_MARKER]: ami });
    assert.deepEqual(t.Resources.AgentHost.Properties, fixture.Resources.AgentHost.Properties);
    assert.deepEqual(t.Resources.AgentHost.Properties.UserData, fixture.Resources.AgentHost.Properties.UserData);
    assert.deepEqual(withoutMarker({ ...t, Parameters: { ...t.Parameters, AmiId: fixture.Parameters.AmiId } }), fixture);
    assert.deepEqual(previousParameters(t), ["SubnetId", "SecurityGroupId", "InstanceProfileName"].map((k) => `ParameterKey=${k},UsePreviousValue=true`));
    // What the kit's verifyAmiPin() reads back once this template is deployed.
    assert.deepEqual(previewTemplate(t, plainRunning(ami), ami, desired), { kind: "adopted" });
  }
});

test("adoption keeps other AgentHost Metadata keys and adds only the marker", () => {
  const deployed = copy(fixture);
  deployed.Resources.AgentHost = { Type: deployed.Resources.AgentHost.Type, Metadata: { Other: { keep: [1, 2] } }, Properties: deployed.Resources.AgentHost.Properties };
  const want = copy(desired) as typeof fixture;
  want.Resources.AgentHost.Metadata = { Other: { keep: [1, 2] }, [AMI_PIN_MARKER]: ami };
  const result = previewTemplate(deployed, running(ami), ami, want);
  assert.equal(result.kind, "preview");
  const t = (result as { template: typeof fixture }).template;
  assert.deepEqual(t.Resources.AgentHost.Metadata, { Other: { keep: [1, 2] }, [AMI_PIN_MARKER]: ami });
  assert.deepEqual(t.Resources.AgentHost.Properties, fixture.Resources.AgentHost.Properties);
  assert.deepEqual(withoutMarker({ ...t, Parameters: { ...t.Parameters, AmiId: fixture.Parameters.AmiId } }), deployed);
  // The desired template without the other key is a full deploy, as in the kit.
  assert.throws(() => previewTemplate(deployed, running(ami), ami, desired), /a full deploy\) at: \/Resources\/AgentHost\/Metadata\/Other$/);
});

test("a 0.1.14 stack (plain pin, no marker) acquires the marker, read from ParameterValue", () => {
  const deployed = plain014();
  const result = previewTemplate(deployed, plainRunning(ami), ami, desired);
  assert.equal(result.kind, "preview");
  const t = (result as { template: typeof fixture }).template;
  assert.deepEqual(t.Parameters.AmiId, pinnedAmiParameter(ami));
  assert.deepEqual(t.Resources.AgentHost.Metadata, { [AMI_PIN_MARKER]: ami });
  assert.deepEqual(withoutMarker(t), deployed);
  // The plain parameter has no ResolvedValue; a ResolvedValue alone is not read.
  assert.throws(() => previewTemplate(deployed, stacks([{ ParameterKey: "AmiId", ResolvedValue: ami }]), ami, desired), /no resolved AmiId/);
  assert.throws(() => previewTemplate(deployed, plainRunning("ami-0123456789abcdef0"), ami, desired), /host runs ami-0123456789abcdef0: a full deploy/);
});

test("adopted needs both the pinned AmiId and the marker", () => {
  assert.deepEqual(previewTemplate(adoptedTemplate(), plainRunning(ami), ami, desired), { kind: "adopted" });
  assert.deepEqual(previewTemplate(JSON.stringify(adoptedTemplate()), plainRunning(ami), ami, desired), { kind: "adopted" });
  // The parameter alone (0.1.14) is not adopted: the marker is still to come.
  assert.equal(previewTemplate(plain014(), plainRunning(ami), ami, desired).kind, "preview");
  // The marker alone, on an SSM-typed AmiId, is not adopted either.
  const markerOnly = copy(fixture);
  markerOnly.Resources.AgentHost.Metadata = { [AMI_PIN_MARKER]: ami };
  assert.notDeepEqual(previewTemplate(markerOnly, running(ami), ami, desired), { kind: "adopted" });
  // A marker for another AMI on the plain pin is neither adopted nor adoptable.
  const otherMarker = adoptedTemplate();
  otherMarker.Resources.AgentHost.Metadata = { [AMI_PIN_MARKER]: "ami-0123456789abcdef0" };
  assert.throws(() => previewTemplate(otherMarker, plainRunning(ami), ami, desired), /neither SSM-typed nor the plain pin .* a full deploy/);
  // An adopted template still has to run the pin.
  assert.throws(() => previewTemplate(adoptedTemplate(), plainRunning("ami-0123456789abcdef0"), ami, desired), /host runs ami-0123456789abcdef0/);
});

test("a deployed plain AmiId for another AMI is refused as a full deploy", () => {
  const other = { ...copy(fixture), Parameters: { ...fixture.Parameters, AmiId: pinnedAmiParameter("ami-0123456789abcdef0") } };
  assert.throws(() => previewTemplate(other, plainRunning("ami-0123456789abcdef0"), ami, desired), /neither SSM-typed nor the plain pin ami-08cf17e3856f70154 without the LayeredAgentAmiPin marker, nor adopted: a full deploy/);
  assert.throws(() => previewTemplate(other, plainRunning(ami), ami, desired), /a full deploy/);
});

test("key order is not a difference, only content is", () => {
  const reordered = (v: unknown): unknown => Array.isArray(v) ? v.map(reordered)
    : v && typeof v === "object" ? Object.fromEntries(Object.entries(v).reverse().map(([k, x]) => [k, reordered(x)])) : v;
  const deployed = reordered(copy(fixture)) as typeof fixture;
  assert.notEqual(JSON.stringify(deployed), JSON.stringify(fixture));
  assert.equal(previewTemplate(deployed, running(ami), ami, desired).kind, "preview");
  assert.deepEqual(differingPaths(deployed, fixture), []);
  assert.deepEqual(differingPaths({ a: 1, b: { c: 2, d: 3 } }, { b: { d: 3, c: 2 }, a: 1 }), []);
  // Paths, never values; array order still counts.
  assert.deepEqual(differingPaths({ a: 1, b: { c: "secret" } }, { b: { c: "other" }, a: 1, e: 0 }), ["/b/c", "/e"]);
  assert.deepEqual(differingPaths({ a: [1, 2] }, { a: [2, 1] }), ["/a/0", "/a/1"]);
});

test("a full deploy is refused before any change set: other AMI, unknown AMI, other template change", () => {
  assert.throws(() => previewTemplate(fixture, running("ami-0123456789abcdef0"), ami, desired), /host runs ami-0123456789abcdef0: a full deploy/);
  assert.throws(() => previewTemplate(fixture, stacks([]), ami, desired), /no resolved AmiId/);
  const bigger = copy(fixture);
  bigger.Resources.AgentHost.Properties.InstanceType = "m7i.4xlarge";
  assert.throws(() => previewTemplate(bigger, running(ami), ami, desired), /beyond UserData, AmiId and the LayeredAgentAmiPin marker .* at: \/Resources\/AgentHost\/Properties\/InstanceType$/);
});

const template = { Parameters: { AmiId: pinnedAmiParameter(ami) }, Resources: { AgentHost: { Metadata: { [AMI_PIN_MARKER]: ami } } } };
const changeSet = (host: object, amiParam: object = { ParameterKey: "AmiId", ParameterValue: ami }, extra: object[] = []) => ({
  Status: "CREATE_COMPLETE",
  Changes: [{ Type: "Resource", ResourceChange: { LogicalResourceId: "AgentHost", ...host } }, ...extra],
  Parameters: [amiParam, { ParameterKey: "SubnetId", ParameterValue: "subnet-1", UsePreviousValue: true }],
});

test("the change set passes when AgentHost is modified in place and AmiId took the default", () => {
  const v = assertChangeSet(changeSet({ Action: "Modify", Replacement: "False" }), template, ami);
  assert.ok(v.ok, v.line);
  assert.match(v.line, /AgentHost Modify\/False, AmiId ami-08cf17e3856f70154: PASS$/);
  // The marker's change is Metadata-scoped: CloudFormation lists it as a Modify without replacement.
  const scoped = assertChangeSet(changeSet({ Action: "Modify", Replacement: "False", Scope: ["Metadata"], Details: [{ Target: { Attribute: "Metadata", RequiresRecreation: "Never" }, Evaluation: "Static", ChangeSource: "DirectModification" }] }), template, ami);
  assert.ok(scoped.ok, scoped.line);
  assert.equal(scoped.line, "status CREATE_COMPLETE, AgentHost Modify/False, AmiId ami-08cf17e3856f70154: PASS");
});

test("a no-changes change set fails a first adoption in both modes: the pin would not persist", () => {
  for (const allow of [false, true]) {
    const v = assertChangeSet({ Status: "FAILED", StatusReason: NO_CHANGES_REASON }, template, ami, allow);
    assert.equal(v.ok, false);
    assert.equal(v.line, "status FAILED (no changes), AgentHost unchanged, AmiId n/a: FAIL (CloudFormation reports no changes, so the pin would not persist; the stored template still differs from the pin)");
    assert.doesNotMatch(v.line, /PASS/);
    assert.equal(assertChangeSet({ Status: "FAILED", StatusReason: "No updates are to be performed." }, template, ami, allow).ok, false);
  }
});

test("a CREATE_COMPLETE change set without an in-place AgentHost change fails in both modes", () => {
  const amiParams = [{ ParameterKey: "AmiId", ParameterValue: ami }];
  const fails = [
    { Status: "CREATE_COMPLETE", Changes: [], Parameters: amiParams },
    { Status: "CREATE_COMPLETE", Parameters: amiParams },
    { Status: "CREATE_COMPLETE", Changes: [{ Type: "Resource", ResourceChange: { LogicalResourceId: "PatchWindow", Action: "Modify", Replacement: "False" } }], Parameters: amiParams },
    changeSet({ Action: "Add" }),
    changeSet({ Action: "Dynamic", Replacement: "False" }),
    changeSet({ Action: "Modify" }),
  ];
  for (const allow of [false, true]) {
    for (const cs of fails) assert.equal(assertChangeSet(cs, template, ami, allow).ok, false, JSON.stringify(cs));
  }
  assert.match(assertChangeSet(fails[0]!, template, ami).line, /AgentHost unchanged, .*FAIL \(AgentHost is not changed, so the pin would not persist\)$/);
});

test("the template must carry both the pinned AmiId default and the marker", () => {
  const clean = changeSet({ Action: "Modify", Replacement: "False" });
  for (const allow of [false, true]) {
    const noMarker = assertChangeSet(clean, { Parameters: { AmiId: pinnedAmiParameter(ami) } }, ami, allow);
    assert.equal(noMarker.ok, false);
    assert.match(noMarker.line, /the template's AgentHost LayeredAgentAmiPin marker is undefined, not the pin ami-08cf17e3856f70154/);
    const otherMarker = { ...template, Resources: { AgentHost: { Metadata: { [AMI_PIN_MARKER]: "ami-0123456789abcdef0" } } } };
    assert.equal(assertChangeSet(clean, otherMarker, ami, allow).ok, false);
    assert.equal(assertChangeSet(clean, { ...template, Parameters: { AmiId: pinnedAmiParameter("ami-0123456789abcdef0") } }, ami, allow).ok, false);
  }
});

test("the change set fails on any replacement, removal, previous or other AmiId, or failed status", () => {
  const fails = [
    changeSet({ Action: "Modify", Replacement: "True" }),
    changeSet({ Action: "Modify", Replacement: "Conditional" }),
    changeSet({ Action: "Modify", Replacement: "False" }, undefined, [{ ResourceChange: { LogicalResourceId: "PatchWindow", Action: "Remove" } }]),
    changeSet({ Action: "Modify", Replacement: "False" }, { ParameterKey: "AmiId", ParameterValue: ami, UsePreviousValue: true }),
    changeSet({ Action: "Modify", Replacement: "False" }, { ParameterKey: "AmiId", ParameterValue: "ami-0123456789abcdef0" }),
    changeSet({ Action: "Modify", Replacement: "False" }, { ParameterKey: "SubnetId", ParameterValue: "x" }),
    { Status: "FAILED", StatusReason: "User: arn:aws:sts::1:assumed-role/x is not authorized to perform: ec2:DescribeImages" },
    { Status: "CREATE_IN_PROGRESS" },
  ];
  for (const cs of fails) assert.equal(assertChangeSet(cs, template, ami).ok, false, JSON.stringify(cs));
  assert.equal(assertChangeSet(changeSet({ Action: "Modify", Replacement: "False" }), { ...template, Parameters: { AmiId: pinnedAmiParameter("ami-0123456789abcdef0") } }, ami).ok, false);
});

test("allow_restart mode lists a replacement instead of failing, and checks everything else the same", () => {
  for (const replacement of ["True", "Conditional"]) {
    const cs = changeSet({ Action: "Modify", Replacement: replacement }, undefined, [{ ResourceChange: { LogicalResourceId: "PatchWindow", Action: "Modify", Replacement: replacement } }]);
    const strict = assertChangeSet(cs, template, ami);
    assert.equal(strict.ok, false);
    assert.doesNotMatch(strict.line, /allowed by allow_restart/);
    const v = assertChangeSet(cs, template, ami, true);
    assert.ok(v.ok, v.line);
    assert.deepEqual(v.allowed, [`AgentHost replacement is ${replacement}`, `PatchWindow replacement is ${replacement}`]);
    assert.match(v.line, new RegExp(`replacement allowed by allow_restart \\(AgentHost replacement is ${replacement}; PatchWindow replacement is ${replacement}\\): PASS$`));
  }
  // A clean change set reads the same in both modes.
  const clean = changeSet({ Action: "Modify", Replacement: "False" });
  assert.equal(assertChangeSet(clean, template, ami, true).line, assertChangeSet(clean, template, ami).line);
  // Removals, AmiId and status still fail with replacement allowed.
  const stillFail = [
    changeSet({ Action: "Modify", Replacement: "True" }, undefined, [{ ResourceChange: { LogicalResourceId: "PatchWindow", Action: "Remove" } }]),
    changeSet({ Action: "Modify", Replacement: "True" }, { ParameterKey: "AmiId", ParameterValue: ami, UsePreviousValue: true }),
    changeSet({ Action: "Modify", Replacement: "True" }, { ParameterKey: "AmiId", ParameterValue: "ami-0123456789abcdef0" }),
    changeSet({ Action: "Modify", Replacement: "True" }, { ParameterKey: "SubnetId", ParameterValue: "x" }),
    { Status: "FAILED", StatusReason: "User: arn:aws:sts::1:assumed-role/x is not authorized to perform: ec2:DescribeImages" },
    { Status: "CREATE_IN_PROGRESS" },
  ];
  for (const cs of stillFail) assert.equal(assertChangeSet(cs, template, ami, true).ok, false, JSON.stringify(cs));
  assert.equal(assertChangeSet(clean, { ...template, Parameters: { AmiId: pinnedAmiParameter("ami-0123456789abcdef0") } }, ami, true).ok, false);
  assert.equal(assertChangeSet({ Status: "FAILED", StatusReason: NO_CHANGES_REASON }, template, ami, true).ok, false);
});

// The wrapper against a fake aws that serves canned JSON and logs its calls.
function runScript(changeSetJson: object, mode = "run", opts: { deleteError?: string; env?: Record<string, string>; deployed?: object; stacks?: object } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "preview-change-set-"));
  try {
    writeFileSync(join(dir, "deployed.json"), JSON.stringify(opts.deployed ?? fixture));
    writeFileSync(join(dir, "stacks.json"), JSON.stringify(opts.stacks ?? running(ami)));
    writeFileSync(join(dir, "change-set.json"), JSON.stringify(changeSetJson));
    if (opts.deleteError !== undefined) writeFileSync(join(dir, "delete-error"), opts.deleteError);
    writeFileSync(join(dir, "aws"), [
      "#!/usr/bin/env bash",
      'echo "$*" >>"$FAKE_DIR/calls"',
      'case " $* " in',
      '  *" get-template "*) cat "$FAKE_DIR/deployed.json" ;;',
      '  *" describe-stacks "*) cat "$FAKE_DIR/stacks.json" ;;',
      '  *" create-change-set "*) for a in "$@"; do case $a in file://*) cp "${a#file://}" "$FAKE_DIR/submitted.json" ;; esac; done; echo "{}" ;;',
      '  *" wait "*) exit 255 ;;',
      '  *" describe-change-set "*) cat "$FAKE_DIR/change-set.json" ;;',
      '  *" delete-change-set "*) if [ -f "$FAKE_DIR/delete-error" ]; then cat "$FAKE_DIR/delete-error" >&2; exit 254; fi ;;',
      "  *) exit 99 ;;",
      "esac",
      "",
    ].join("\n"));
    chmodSync(join(dir, "aws"), 0o755);
    const r = spawnSync("bash", [fileURLToPath(new URL("../preview-change-set.sh", import.meta.url)), mode, "preview-1-1"], {
      cwd: dir,
      encoding: "utf8",
      env: { PATH: `${dir}:${process.env.PATH}`, HOME: dir, FAKE_DIR: dir, GITHUB_STEP_SUMMARY: join(dir, "summary"), ...opts.env },
    });
    const read = (f: string) => { try { return readFileSync(join(dir, f), "utf8"); } catch { return ""; } };
    return { status: r.status, stdout: r.stdout, stderr: r.stderr, calls: read("calls"), submitted: read("submitted.json"), summary: read("summary") };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("the wrapper submits the pin template without AmiId, never executes, and passes a clean change set", () => {
  const r = runScript(changeSet({ Action: "Modify", Replacement: "False" }));
  assert.equal(r.status, 0, r.stderr);
  const create = r.calls.split("\n").find((l) => l.includes("create-change-set"))!;
  assert.match(create, /--stack-name relaycast-agent-host-dev-relay-desktop --change-set-name preview-1-1 --change-set-type UPDATE/);
  assert.match(create, /--parameters ParameterKey=SubnetId,UsePreviousValue=true ParameterKey=SecurityGroupId,UsePreviousValue=true ParameterKey=InstanceProfileName,UsePreviousValue=true$/);
  assert.doesNotMatch(r.calls, /AmiId|execute-change-set|update-stack|ssm /);
  assert.deepEqual(JSON.parse(r.submitted).Parameters.AmiId, pinnedAmiParameter(ami));
  assert.deepEqual(JSON.parse(r.submitted).Resources.AgentHost.Metadata, { [AMI_PIN_MARKER]: ami });
  assert.match(r.summary, /stack relaycast-agent-host-dev-relay-desktop, change set preview-1-1, status CREATE_COMPLETE, AgentHost Modify\/False, .*PASS/);
});

test("the wrapper submits the marker for a 0.1.14 plain-pinned stack", () => {
  const r = runScript(changeSet({ Action: "Modify", Replacement: "False", Scope: ["Metadata"] }), "run", { deployed: plain014(), stacks: plainRunning(ami) });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.calls, /create-change-set/);
  assert.deepEqual(JSON.parse(r.submitted).Resources.AgentHost.Metadata, { [AMI_PIN_MARKER]: ami });
  assert.match(r.summary, /status CREATE_COMPLETE, AgentHost Modify\/False, .*PASS$/m);
});

test("the wrapper fails a no-changes change set and an AgentHost-less one", () => {
  const empty = runScript({ Status: "FAILED", StatusReason: NO_CHANGES_REASON });
  assert.equal(empty.status, 1);
  assert.match(empty.stdout, /status FAILED \(no changes\), AgentHost unchanged, AmiId n\/a: FAIL \(CloudFormation reports no changes, so the pin would not persist; the stored template still differs from the pin\)/);
  assert.match(empty.summary, /change set preview-1-1, status FAILED \(no changes\), .*: FAIL/);
  assert.doesNotMatch(empty.summary, /PASS/);
  const noHost = runScript({ Status: "CREATE_COMPLETE", Changes: [], Parameters: [{ ParameterKey: "AmiId", ParameterValue: ami }] });
  assert.equal(noHost.status, 1);
  assert.match(noHost.summary, /AgentHost unchanged, .*FAIL \(AgentHost is not changed, so the pin would not persist\)/);
});

test("the wrapper passes without a change set once the pin and the marker are deployed", () => {
  const r = runScript({}, "run", { deployed: adoptedTemplate(), stacks: plainRunning(ami) });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /pin already adopted on relaycast-agent-host-dev-relay-desktop, nothing to preview/);
  assert.match(r.summary, /already carries the AMI pin, nothing to preview: PASS/);
  assert.doesNotMatch(r.calls, /create-change-set|describe-change-set/);
  // The parameter alone is not adopted: the change set runs.
  const partial = runScript(changeSet({ Action: "Modify", Replacement: "False" }), "run", { deployed: plain014(), stacks: plainRunning(ami) });
  assert.match(partial.calls, /create-change-set/);
});

test("the wrapper fails on a replacement and prints the changes", () => {
  const r = runScript(changeSet({ Action: "Modify", Replacement: "True" }));
  assert.equal(r.status, 1);
  assert.match(r.stdout, /FAIL \(AgentHost replacement is True\)/);
  assert.match(r.stderr, /"Replacement": "True"/);
  assert.match(r.summary, /FAIL/);
});

test("the wrapper allows a replacement only with PREVIEW_ALLOW_REPLACEMENT=true, and lists it in the summary", () => {
  const cs = changeSet({ Action: "Modify", Replacement: "True" });
  for (const env of [{}, { PREVIEW_ALLOW_REPLACEMENT: "false" }] as Record<string, string>[]) {
    const r = runScript(cs, "run", { env });
    assert.equal(r.status, 1, JSON.stringify(env));
    assert.match(r.summary, /FAIL \(AgentHost replacement is True\)/);
  }
  const r = runScript(cs, "run", { env: { PREVIEW_ALLOW_REPLACEMENT: "true" } });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.summary, /replacement allowed by allow_restart \(AgentHost replacement is True\): PASS/);
  const removal = runScript(changeSet({ Action: "Modify", Replacement: "True" }, undefined, [{ ResourceChange: { LogicalResourceId: "PatchWindow", Action: "Remove" } }]), "run", { env: { PREVIEW_ALLOW_REPLACEMENT: "true" } });
  assert.equal(removal.status, 1);
  assert.match(removal.summary, /FAIL \(PatchWindow would be removed\)/);
  const junk = runScript(cs, "run", { env: { PREVIEW_ALLOW_REPLACEMENT: "yes" } });
  assert.equal(junk.status, 1);
  assert.match(junk.stderr, /PREVIEW_ALLOW_REPLACEMENT is "yes", not true or false/);
});

test("delete exits 0 when the change set was deleted", () => {
  const r = runScript({}, "delete");
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /deleted change set preview-1-1/);
  assert.match(r.calls, /delete-change-set --stack-name relaycast-agent-host-dev-relay-desktop --change-set-name preview-1-1/);
});

test("delete exits 0 only on a verified not-found", () => {
  for (const err of [
    "\nAn error occurred (ChangeSetNotFound) when calling the DeleteChangeSet operation: ChangeSet [preview-1-1] does not exist\n",
    "\nAn error occurred (ValidationError) when calling the DeleteChangeSet operation: ChangeSet [preview-1-1] does not exist\n",
  ]) {
    const r = runScript({}, "delete", { deleteError: err });
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /does not exist \(never created, or already gone\)/);
  }
});

test("delete fails and prints the error on a denied deletion or any other failure", () => {
  for (const err of [
    "\nAn error occurred (AccessDenied) when calling the DeleteChangeSet operation: User: arn:aws:sts::1:assumed-role/x is not authorized to perform: cloudformation:DeleteChangeSet\n",
    "\nAn error occurred (AccessDenied) when calling the DeleteChangeSet operation: ChangeSet [preview-1-1] does not exist or you are not authorized\n",
    "\nCould not connect to the endpoint URL: \"https://cloudformation.us-west-2.amazonaws.com/\"\n",
    "",
  ]) {
    const r = runScript({}, "delete", { deleteError: err });
    assert.equal(r.status, 1, JSON.stringify(err));
    assert.match(r.stderr, /could not delete change set preview-1-1 on relaycast-agent-host-dev-relay-desktop:/);
    if (err) assert.ok(r.stderr.includes(err.trim()), r.stderr);
    assert.doesNotMatch(r.stdout, /deleted|does not exist/);
  }
});
