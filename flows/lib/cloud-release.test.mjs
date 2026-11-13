import test from 'node:test';
import assert from 'node:assert/strict';
import { getFlowDefinition } from '@relayflows/surface/runtime';
import release from '../cloud-release.flow.ts';
const body = getFlowDefinition(release).body;
const source = { sourceSha: 'a'.repeat(40), sourceTree: 'b'.repeat(40), ref: 'main', gateRunId: 10, candidateRunId: 20, version: '2026.11.5' };
const input = { operation: 'prepare', channel: 'stable', requestId: 'request-12345' };
function context({ approve = true, gateFailure = false, retry = false, current = false, missingGate = false, secondFailure = false } = {}) {
  const calls = []; const completions = []; let failures = 0;
  return {
    calls, completions,
    f: {
      run: async (command, options) => {
        if (command.startsWith('sleep ')) { calls.push({ action: 'sleep' }); return ''; }
        const [, action, encoded] = command.match(/release-worker\.mjs '([^']+)' '([^']+)'/);
        const payload = JSON.parse(Buffer.from(encoded, 'base64').toString());
        calls.push({ action, payload });
        assert.deepEqual(options.secrets, ['DESKTOP_RELEASE_GH_APP_ID', 'DESKTOP_RELEASE_GH_INSTALLATION_ID', 'DESKTOP_RELEASE_GH_APP_PRIVATE_KEY']);
        let result = {};
        if (action === 'inventory') result = [];
        if (action === 'resolve') result = { ...source, current, ...(missingGate ? { gateRunId: undefined } : {}) };
        if (action === 'dispatch') result = { runId: { gate: 10, prepare: 20, promote: 30 }[payload.phase] };
        if (action === 'inspect') result = gateFailure && payload.runId === 10 && failures++ < (retry ? (secondFailure ? 2 : 1) : 3) ? { status: 'completed', conclusion: 'failure' } : { status: 'completed', conclusion: 'success' };
        if (action === 'retry') result = { retried: retry };
        if (action === 'candidate') result = source;
        return JSON.stringify(result);
      },
      human: async (question, to) => { calls.push({ action: 'human', question, to }); return approve; },
      done: (reason, detail) => completions.push({ reason, detail }),
    },
  };
}
test('native preparation overlaps the gate, but readiness waits for verified evidence and successful packaging', async () => {
  const { f, calls, completions } = context({ missingGate: true });
  await body(f, input);
  assert.deepEqual(calls.filter(c => c.action === 'dispatch').map(c => c.payload.phase), ['gate', 'prepare']);
  assert.ok(calls.findIndex(c => c.action === 'dispatch' && c.payload.phase === 'prepare') < calls.findIndex(c => c.action === 'inspect'));
  assert.ok(calls.findIndex(c => c.action === 'verify-gate') < calls.findIndex(c => c.action === 'candidate'));
  assert.equal(calls.find(c => c.action === 'result').payload.result.status, 'ready');
  assert.equal(completions[0].reason, 'success');
  assert.ok(!calls.some(c => c.action === 'human' || c.action === 'tag'));
});
test('a real gate failure prevents a candidate becoming ready or any publication', async () => {
  const { f, calls } = context({ gateFailure: true });
  await assert.rejects(body(f, input), /release stopped/);
  assert.ok(!calls.some(c => ['candidate', 'result', 'tag', 'human'].includes(c.action)));
});
test('one known-flake retry is journaled before accepting the rerun result', async () => {
  const { f, calls } = context({ gateFailure: true, retry: true });
  await body(f, input);
  assert.equal(calls.filter(c => c.action === 'retry').length, 1);
  assert.ok(calls.some(c => c.action === 'verify-gate'));
});
test('stable publication requires direct durable confirmation after readiness and gate verification', async () => {
  const { f, calls } = context();
  await body(f, { ...input, operation: 'publish' });
  const prompt = calls.find(c => c.action === 'human');
  assert.equal(prompt.to.to, 'khaliq-gant');
  assert.ok(prompt.question.includes(source.sourceSha));
  assert.ok(calls.findIndex(c => c.action === 'verify-gate') < calls.indexOf(prompt));
  assert.ok(calls.indexOf(prompt) < calls.findIndex(c => c.action === 'dispatch'));
  const result = calls.find(c => c.action === 'result').payload.result;
  assert.equal(result.status, 'published');
  assert.equal(result.hostedUploadRecovery, 'pending');
});
test('declined stable and already-current ticks dispatch nothing', async () => {
  const declined = context({ approve: false });
  await body(declined.f, { ...input, operation: 'publish' });
  assert.ok(!declined.calls.some(c => c.action === 'dispatch'));
  assert.equal(declined.completions[0].reason, 'needs_human');
  const current = context({ current: true });
  await body(current.f, { ...input, operation: 'tick' });
  assert.deepEqual(current.calls.map(c => c.action), ['resolve']);
});

test('inventory refresh is read-only and never prepares a new candidate', async () => {
  const { f, calls } = context();
  await body(f, { ...input, operation: 'inventory' });
  assert.deepEqual(calls.map(c => c.action), ['inventory', 'result']);
});

test('a second failed gate stops after its single allowed retry', async () => {
  const { f, calls } = context({ gateFailure: true, retry: true, secondFailure: true });
  await assert.rejects(body(f, input), /release stopped/);
  assert.equal(calls.filter(c => c.action === 'retry').length, 1);
  assert.ok(!calls.some(c => ['verify-gate', 'result', 'tag', 'human'].includes(c.action)));
});
