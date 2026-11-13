import test from 'node:test';
import assert from 'node:assert/strict';
import { REPO, validate, candidateIdentity, assertReadiness, knownFlake, matchingRun, dispatchInputs, freshCiRun } from './release-worker.mjs';
import { requestFromArgs, submission } from './release-launcher.mjs';
const input = { operation: 'prepare', channel: 'stable', requestId: 'request-12345', ref: 'main' };
const plan = { sourceSha: 'a'.repeat(40), sourceTree: 'b'.repeat(40), gateRunId: 123, candidateRunId: 456, version: '2026.11.5' };
test('discovered CI freshness matches the verifier boundaries, including invalid clocks', () => {
  const now = Date.parse('2026-11-11T00:00:00Z');
  for (const [age, expected] of [[7 * 86400_000, true], [7 * 86400_000 + 1, false], [-5 * 60_000, true], [-5 * 60_000 - 1, false]]) {
    assert.equal(freshCiRun({ created_at: new Date(now - age).toISOString() }, now), expected);
  }
  assert.equal(freshCiRun({ created_at: 'invalid' }, now), false);
});
test('candidate identity uses the Source line, never an automatic run newer head SHA', () => {
  const producer = { id: 45, repository: { full_name: REPO }, head_repository: { full_name: REPO }, path: '.github/workflows/desktop-release.yml', event: 'workflow_run', head_sha: 'd'.repeat(40) };
  const log = `Releasing v2026.11.5\nSource ${plan.sourceSha}\nCI run 123\n`;
  assert.equal(candidateIdentity(producer, log).sourceSha, plan.sourceSha);
  assert.throws(() => candidateIdentity({ ...producer, event: 'workflow_dispatch' }, log), /differs/);
  assert.throws(() => candidateIdentity(producer, log + `Source ${'f'.repeat(40)}\n`), /unambiguous/);
  assert.throws(() => candidateIdentity({ ...producer, repository: { full_name: 'fork/repo' } }, log));
});
test('readiness must be an explicit lead attestation for the exact tree', () => {
  const receipt = { sourceTree: plan.sourceTree, reviewRunId: 'cloud-round-1', reviewReported: true, openBlockers: 0, blockerFixesReviewed: true, localCiPassed: true };
  assert.doesNotThrow(() => assertReadiness(receipt, plan.sourceTree));
  for (const [name, value] of [['sourceTree', 'wrong'], ['reviewReported', false], ['openBlockers', 1], ['blockerFixesReviewed', false], ['localCiPassed', false]]) assert.throws(() => assertReadiness({ ...receipt, [name]: value }, plan.sourceTree));
  assert.throws(() => assertReadiness(undefined, plan.sourceTree));
});
test('only named known flaky failures qualify; mixed failures and compilation errors stop', () => {
  const log = 'test probe_copy::tests::a_missing_or_older_copy_is_replaced_and_an_identical_one_kept ... FAILED';
  assert.equal(knownFlake(log), true);
  assert.equal(knownFlake(log + '\ntest real::bug ... FAILED'), false);
  assert.equal(knownFlake(log + '\nerror: could not compile'), false);
  assert.equal(knownFlake('transport failed'), false);
});
test('request correlation binds candidate, channel, profile and gate; no newest-run guessing', () => {
  const fields = dispatchInputs('prepare', input, plan);
  assert.equal(fields['ci-run-id'], '123');
  assert.equal(fields['expected-source-sha'], plan.sourceSha);
  assert.equal(fields['prepare-only'], 'true');
  assert.notEqual(fields['request-id'], dispatchInputs('prepare', { ...input, profile: 'acme' }, plan)['request-id']);
  assert.notEqual(fields['request-id'], dispatchInputs('prepare', input, { ...plan, gateRunId: 124 })['request-id']);
  assert.equal(dispatchInputs('gate', { ...input, operation: 'tick' }, plan)['request-id'], dispatchInputs('gate', { ...input, operation: 'tick', requestId: 'different-request' }, plan)['request-id']);
  const run = { id: 1, display_title: 'title', head_sha: plan.sourceSha, event: 'workflow_dispatch' };
  assert.equal(matchingRun([run, { ...run, display_title: 'other', id: 2 }], 'title', plan.sourceSha).id, 1);
  assert.throws(() => matchingRun([run, run], 'title', plan.sourceSha), /ambiguous/);
});
test('bridge parses existing commands and refuses publication without readiness', () => {
  assert.equal(requestFromArgs(['prepare', 'stable', 'hotfix/2026.11.4-fix', '--gate', '123'], undefined, 'request-12345').gateRunId, '123');
  assert.throws(() => requestFromArgs(['stable', '--run', '456']), /readiness/);
  assert.throws(() => requestFromArgs(['prepare', 'stable', 'main', '--force', 'true']), /unknown/);
  assert.throws(() => validate({ ...input, ref: 'main;bad' }));
  assert.throws(() => validate({ ...input, candidateRunId: '-1' }));
  assert.equal(submission('{"ok":true,"runId":"cloud-1"}').runId, 'cloud-1');
  assert.throws(() => submission('{"ok":false,"code":"cloud_unreachable"}'), /Do not resubmit blindly/);
});

// Exercise the actual effect helper against a local fake GitHub executable.
// No network, workflow dispatch, tag mutation or real credential is used.
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
function effectsFixture(overrides = {}) {
  const root = mkdtempSync(join(tmpdir(), 'release-effects-')); const statePath = join(root, 'state.json');
  writeFileSync(statePath, JSON.stringify({ calls: [], refs: {}, sourceSha: plan.sourceSha, runs: [], ...overrides }));
  const script = `#!/usr/bin/env node
const fs=require('node:fs'); const s=JSON.parse(fs.readFileSync(process.env.FAKE_GITHUB,'utf8'));
const argv=process.argv.slice(2); const path=argv[1]; const body=argv.includes('--input')?JSON.parse(fs.readFileSync(0,'utf8')):undefined;
s.calls.push({path,body}); let out={}; let failure='';
if(Object.hasOwn(s.responses??{},path)){const response=s.responses[path];if(response.error)failure=response.error;else out=response;}
else if(path.includes('/commits/'))out={sha:s.sourceSha,commit:{tree:{sha:'${plan.sourceTree}'}}};
else if(path.includes('/actions/workflows/')&&path.includes('/runs?'))out={workflow_runs:s.runs};
else if(path.endsWith('/git/refs')) { if(s.refs[body.ref])failure='HTTP 422';else s.refs[body.ref]=body.sha; }
else if(path.includes('/git/ref/heads/'))out={object:{sha:s.refs['refs/heads/'+path.split('/git/ref/heads/')[1]]}};
else if(path.includes('/actions/runs/'))out=s.producer??{};
else if(path.includes('/git/ref/tags/'))out={object:{type:'commit',sha:'f'.repeat(40)}};
fs.writeFileSync(process.env.FAKE_GITHUB,JSON.stringify(s)); if(failure){console.error(failure);process.exit(1)}console.log(typeof out==='string'?out:JSON.stringify(out));
`;
  writeFileSync(join(root, 'gh'), script, { mode: 0o755 });
  return {
    call(action, payload, token = 'fake-only') { return spawnSync(process.execPath, ['flows/lib/release-worker.mjs', action, Buffer.from(JSON.stringify(payload)).toString('base64')], { cwd: new URL('../..', import.meta.url), encoding: 'utf8', env: { ...process.env, PATH: `${root}:${process.env.PATH}`, FAKE_GITHUB: statePath, DESKTOP_RELEASE_GH_TOKEN: token } }); },
    state() { return JSON.parse(readFileSync(statePath, 'utf8')); },
    close() { rmSync(root, { recursive: true, force: true }); },
  };
}
test('an uncertain dispatch can be discovered but never POSTed a second time', () => {
  const fx = effectsFixture();
  try {
    const first = fx.call('dispatch', { input, plan, phase: 'prepare' });
    assert.equal(first.status, 0, first.stderr);
    // Simulate an admitted dispatch whose worker is not yet visible.
    const second = fx.call('dispatch', { input, plan, phase: 'prepare' });
    assert.equal(second.status, 0, second.stderr);
    assert.deepEqual(JSON.parse(second.stdout), JSON.parse(first.stdout));
    assert.equal(fx.state().calls.filter(c => c.path.endsWith('/dispatches')).length, 1);
  } finally { fx.close(); }
});
test('a moved source ref is refused before reservation or dispatch', () => {
  const fx = effectsFixture({ sourceSha: 'c'.repeat(40) });
  try {
    const result = fx.call('dispatch', { input, plan, phase: 'prepare' });
    assert.equal(result.status, 1); assert.match(result.stderr, /source ref moved/);
    assert.ok(!fx.state().calls.some(c => c.body));
  } finally { fx.close(); }
});
test('a conflicting released tag is a hard stop and is never updated', () => {
  const fx = effectsFixture({ producer: { path: '.github/workflows/desktop-promote.yml', repository: { full_name: REPO }, status: 'completed', conclusion: 'success' } });
  try {
    const result = fx.call('tag', { input: { ...input, operation: 'publish' }, plan, runId: 789 });
    assert.equal(result.status, 1); assert.match(result.stderr, /refusing to move/);
    assert.ok(!fx.state().calls.some(c => c.body));
  } finally { fx.close(); }
});

test('an inherited general GitHub token cannot replace the dedicated Cloud binding', () => {
  const fx = effectsFixture();
  try {
    const result = fx.call('dispatch', { input, plan, phase: 'prepare' }, '');
    assert.equal(result.status, 1);
    assert.match(result.stderr, /DESKTOP_RELEASE_GH_TOKEN is not configured/);
    assert.equal(fx.state().calls.length, 0);
  } finally { fx.close(); }
});

const endpoint = path => `repos/${REPO}/${path}`;
test('resolve leaves the gate unset after a quiet week, and selects a fresh discovered run', () => {
  const stale = { id: 81, created_at: new Date(Date.now() - 8 * 86400_000).toISOString() };
  const fresh = { id: 82, created_at: new Date().toISOString() };
  for (const [runs, expected] of [[[stale], undefined], [[stale, fresh], 82]]) {
    const fx = effectsFixture({ runs });
    try {
      const result = fx.call('resolve', { input });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(JSON.parse(result.stdout).gateRunId, expected);
      assert.ok(!fx.state().calls.some(c => c.body));
    } finally { fx.close(); }
  }
});
test('missing shipped tag creates the exact annotated tag and ref; existing tags are only read', () => {
  const producer = { path: '.github/workflows/desktop-promote.yml', repository: { full_name: REPO }, status: 'completed', conclusion: 'success' };
  const tagRef = endpoint(`git/ref/tags/released-v${plan.version}`);
  const tagObject = 'e'.repeat(40);
  for (const kind of ['missing', 'lightweight', 'annotated']) {
    const fx = effectsFixture({ producer, responses: {
      [tagRef]: kind === 'missing' ? { error: 'HTTP 404' } : { object: { type: kind === 'annotated' ? 'tag' : 'commit', sha: kind === 'annotated' ? tagObject : plan.sourceSha } },
      [endpoint(`git/tags/${tagObject}`)]: { object: { type: 'commit', sha: plan.sourceSha } },
      [endpoint('git/tags')]: { sha: tagObject },
    } });
    try {
      const result = fx.call('tag', { input: { ...input, operation: 'publish' }, plan, runId: 789 });
      assert.equal(result.status, 0, result.stderr);
      const writes = fx.state().calls.filter(c => c.body);
      assert.deepEqual(writes, kind === 'missing' ? [
        { path: endpoint('git/tags'), body: { tag: `released-v${plan.version}`, message: `relay-desktop v${plan.version}; candidate 456, promote 789`, object: plan.sourceSha, type: 'commit' } },
        { path: endpoint('git/refs'), body: { ref: `refs/tags/released-v${plan.version}`, sha: tagObject } },
      ] : []);
    } finally { fx.close(); }
  }
});
test('the real retry effect reruns only first-attempt allowlisted failures', () => {
  const known = 'test probe_copy::tests::a_missing_or_older_copy_is_replaced_and_an_identical_one_kept ... FAILED';
  for (const [name, conclusion, attempt, log, expected] of [
    ['allowlisted', 'failure', 1, known, true], ['mixed', 'failure', 1, known + '\ntest real::bug ... FAILED', false],
    ['cancelled', 'cancelled', 1, known, false], ['attempt-two', 'failure', 2, known, false],
  ]) {
    const fx = effectsFixture({ responses: {
      [endpoint('actions/runs/9')]: { status: 'completed', conclusion, run_attempt: attempt },
      [endpoint('actions/runs/9/jobs?filter=latest&per_page=100&page=1')]: { jobs: [{ id: 91, conclusion: 'failure' }] },
      [endpoint('actions/jobs/91/logs')]: log,
    } });
    try {
      const result = fx.call('retry', { runId: 9 });
      assert.equal(result.status, 0, `${name}: ${result.stderr}`);
      assert.equal(JSON.parse(result.stdout).retried, expected, name);
      const writes = fx.state().calls.filter(c => c.body);
      assert.deepEqual(writes, expected ? [{ path: endpoint('actions/runs/9/rerun-failed-jobs'), body: {} }] : []);
    } finally { fx.close(); }
  }
});
test('tick and inventory use candidate Source identity from real worker API data', () => {
  const producer = { id: 20, repository: { full_name: REPO }, head_repository: { full_name: REPO }, path: '.github/workflows/desktop-release.yml', event: 'workflow_run', head_sha: 'd'.repeat(40), status: 'completed', conclusion: 'success' };
  const fx = effectsFixture({ responses: {
    [endpoint('actions/workflows/desktop-release.yml/runs?per_page=100')]: { workflow_runs: [producer] },
    [endpoint('actions/runs/20')]: producer,
    [endpoint('actions/runs/20/jobs?filter=latest&per_page=100&page=1')]: { jobs: [{ id: 90, name: 'Resolve version', conclusion: 'success' }] },
    [endpoint('actions/jobs/90/logs')]: `Releasing v${plan.version}\nSource ${plan.sourceSha}\nCI run 123\n`,
  } });
  try {
    const tick = fx.call('resolve', { input: { ...input, operation: 'tick' } });
    assert.equal(tick.status, 0, tick.stderr);
    assert.equal(JSON.parse(tick.stdout).current, true);
    assert.equal(JSON.parse(tick.stdout).sourceSha, plan.sourceSha);
    const inventory = fx.call('inventory', { input: { ...input, operation: 'inventory' } });
    assert.equal(inventory.status, 0, inventory.stderr);
    assert.equal(JSON.parse(inventory.stdout)[0].candidateRunId, 20);
    const otherChannel = fx.call('inventory', { input: { ...input, operation: 'inventory', channel: 'beta' } });
    assert.deepEqual(JSON.parse(otherChannel.stdout), []);
    assert.ok(!fx.state().calls.some(c => c.body));
  } finally { fx.close(); }
});
