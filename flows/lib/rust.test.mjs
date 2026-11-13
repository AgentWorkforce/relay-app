import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parse } from "yaml";

import { rustSteps, VOUCHING_JOB } from "./rust.ts";

const workflow = parse(readFileSync(new URL("../../.github/workflows/desktop-ci.yml", import.meta.url), "utf8"));
// Flags that change how a run reports, not which tests it runs.
const REPORTING = new Set(["--no-fail-fast", "--offline", "--nocapture"]);
const words = (command) => command.split(/\s+/).filter((word) => word && !REPORTING.has(word));
const everyLane = { probe: true, macos_core: true };

for (const [os, linux] of [["linux", true], ["macos", false]]) {
  test(`every vouched ${os} step is a command Desktop CI's ${VOUCHING_JOB[os]} job runs`, () => {
    const job = workflow.jobs[VOUCHING_JOB[os]];
    assert.ok(job, `desktop-ci.yml has no ${VOUCHING_JOB[os]} job`);
    const runs = job.steps.filter((step) => step.run).map((step) => words(step.run.replace(/\\\n/g, " ")).join(" "));
    const vouched = rustSteps(everyLane, linux, ".relayflow/ci").filter((step) => step.vouched);
    assert.ok(vouched.length > 0);
    for (const step of vouched) {
      assert.ok(runs.includes(words(step.command).join(" ")), `${step.name}: \`${step.command}\` is not run by ${VOUCHING_JOB[os]}`);
    }
  });
}

test(`${VOUCHING_JOB.linux} runs on Linux and ${VOUCHING_JOB.macos} on macOS`, () => {
  assert.match(workflow.jobs[VOUCHING_JOB.linux]["runs-on"], /ubuntu|LINUX/);
  const matrix = JSON.stringify(workflow.jobs[VOUCHING_JOB.macos].strategy?.matrix ?? {});
  assert.match(matrix, /macos/i);
});

test("the generated contract step runs desktop-core's generator, not the pinned core's copy", () => {
  const step = rustSteps(everyLane, true, ".relayflow/ci").find((candidate) => candidate.name === "generated contract");
  assert.ok(step);
  assert.match(step.command, /--bin agent-relay-desktop-contract/);
  assert.doesNotMatch(step.command, /core-dir/);
});

test("the code-quality ratchet installs the pinned rust-oleum over any other version", () => {
  const step = rustSteps(everyLane, true, ".relayflow/ci").find((candidate) => candidate.name === "code-quality ratchet");
  assert.ok(step);
  assert.match(step.command, /"rust-oleum 0\.4\.0"/);
  assert.match(step.command, /--force/);
});
