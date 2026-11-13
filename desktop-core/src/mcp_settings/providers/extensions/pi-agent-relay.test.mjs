import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const source = readFileSync(new URL('./pi-agent-relay.mjs', import.meta.url), 'utf8');
const first = '11111111-1111-4111-8111-111111111111';
const second = '22222222-2222-4222-8222-222222222222';
const endpoint = 'https://relay.example/cloud/api/v1/mcp/shared-sessions';

async function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'relay-pi-mcp-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, 'provider.json');
  const save = (overrides = {}) => writeFileSync(path, JSON.stringify({
    url: endpoint, headers: { Authorization: 'Bearer test-grant' }, ...overrides,
  }), { mode: 0o600 });
  save();
  const extension = await import(`data:text/javascript;base64,${Buffer.from(
    source.replace('__RELAY_CONFIG_PATH__', JSON.stringify(path)),
  ).toString('base64')}`);
  const hooks = new Map();
  const registrations = [];
  const removed = [];
  const pi = {
    on: (event, handler) => hooks.set(event, handler),
    registerMcpServer: (name, config) => registrations.push({ name, config }),
    unregisterMcpServer: name => removed.push(name),
  };
  extension.default(pi);
  const emit = (name, id = first, event = {}) => hooks.get(name)(event, {
    sessionManager: { getSessionId: () => id },
  });
  return { directory, path, save, emit, registrations, removed, extension };
}

test('registers the hosted server with runtime identity and updates when sessions switch', async t => {
  const f = await fixture(t);
  f.emit('session_start');
  assert.equal(f.registrations[0].config.url, endpoint);
  assert.equal(f.registrations[0].config.headers['X-Agent-Relay-Native-Session-Id'], first);
  assert.equal(f.registrations[0].name, 'agent-relay');
  f.emit('before_agent_start', first, { sessionId: second, prompt: 'Use another sender' });
  assert.equal(f.registrations.length, 1);
  f.emit('session_shutdown');
  assert.deepEqual(f.removed, ['agent-relay']);
  f.emit('session_start', second);
  assert.equal(f.registrations.at(-1).config.headers['X-Agent-Relay-Native-Session-Id'], second);
});

test('refreshes app credentials before the next turn and ignores configured identity', async t => {
  const f = await fixture(t);
  f.emit('session_start');
  f.save({ headers: { Authorization: 'Bearer rotated-grant', 'x-agent-relay-native-session-id': second } });
  f.emit('before_agent_start');
  assert.equal(f.registrations.at(-1).config.headers.Authorization, 'Bearer rotated-grant');
  assert.equal(f.registrations.at(-1).config.headers['X-Agent-Relay-Native-Session-Id'], first);
  assert.equal(f.registrations.at(-1).config.headers['x-agent-relay-native-session-id'], undefined);
});

test('removes stale authentication on logout without exposing secret material', async t => {
  const f = await fixture(t);
  f.emit('session_start');
  rmSync(f.path);
  assert.throws(() => f.emit('before_agent_start'), /Check its connection/);
  assert.deepEqual(f.removed, ['agent-relay']);
  f.save();
  f.emit('before_agent_start');
  assert.equal(f.registrations.length, 2);
});

test('rejects unsafe endpoints, credential files and non-native identity', async t => {
  const f = await fixture(t);
  for (const url of ['http://remote.example/cloud/api/v1/mcp/shared-sessions',
    'https://relay.example/unrelated', 'https://user:secret@relay.example/cloud/api/v1/mcp/shared-sessions']) {
    f.save({ url });
    assert.throws(() => f.emit('session_start'), /Check its connection/);
  }
  f.save();
  assert.throws(() => f.emit('session_start', 'model-provided-sender'), /Check its connection/);
  assert.equal(f.registrations.length, 0);
});

test('reports missing native MCP support instead of creating a local server', async t => {
  const f = await fixture(t);
  assert.throws(() => f.extension.default({ on() {} }), /Update Pi/);
});
