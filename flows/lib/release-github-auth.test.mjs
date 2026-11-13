import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, verify } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { withReleaseGithubToken, RELEASE_APP_SECRETS, RELEASE_REPOSITORY } from './release-github-auth.mjs';

const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const key = privateKey.export({ type: 'pkcs8', format: 'pem' });
function fixture(overrides = {}) {
  let clock = Date.now(); let sequence = 0;
  const calls = [];
  const env = { DESKTOP_RELEASE_GH_APP_ID: '987', DESKTOP_RELEASE_GH_INSTALLATION_ID: '654', DESKTOP_RELEASE_GH_APP_PRIVATE_KEY: key };
  const fetchImpl = async (url, options) => {
    calls.push({ url, ...options });
    assert.equal(options.redirect, 'error');
    if (url.endsWith('/installation/token')) return new Response(null, { status: overrides.revokeStatus ?? 204 });
    let body;
    if (url.endsWith('/access_tokens')) body = { token: `step-token-${++sequence}`, expires_at: new Date(clock + 3600_000).toISOString(), permissions: { actions: 'write', contents: 'write', metadata: 'read' }, ...overrides.minted };
    else if (url.includes('/installation/repositories')) body = { total_count: 1, repositories: [RELEASE_REPOSITORY], ...overrides.scope };
    else body = { app_id: 987, account: { login: 'AgentWorkforce' }, repository_selection: 'selected', suspended_at: null, permissions: { actions: 'write', contents: 'write', metadata: 'read' }, ...overrides.installation };
    return new Response(JSON.stringify(body));
  };
  return { calls, env, options: { env, fetchImpl, now: () => clock }, advance: n => { clock += n; } };
}

test('signs a bounded App JWT, mints exactly the release repository permissions, and revokes after success', async () => {
  const fx = fixture();
  assert.equal(await withReleaseGithubToken(token => { assert.equal(token, 'step-token-1'); return 'done'; }, fx.options), 'done');
  const jwt = fx.calls[0].headers.Authorization.slice(7);
  const [header, payload, signature] = jwt.split('.');
  assert.equal(verify('RSA-SHA256', Buffer.from(`${header}.${payload}`), publicKey, Buffer.from(signature, 'base64url')), true);
  const claims = JSON.parse(Buffer.from(payload, 'base64url'));
  assert.equal(claims.iss, '987'); assert.equal(claims.exp - claims.iat, 600);
  assert.deepEqual(JSON.parse(fx.calls[1].body), { repository_ids: [1455893451], permissions: { actions: 'write', contents: 'write' } });
  assert.equal(fx.calls.at(-1).method, 'DELETE');
  assert.equal(fx.calls.at(-1).headers.Authorization, 'Bearer step-token-1');
});

test('a later step mints a new token after the first hour rather than reusing an expired run-wide token', async () => {
  const fx = fixture(); const tokens = [];
  await withReleaseGithubToken(token => tokens.push(token), fx.options);
  fx.advance(70 * 60_000);
  await withReleaseGithubToken(token => tokens.push(token), fx.options);
  assert.deepEqual(tokens, ['step-token-1', 'step-token-2']);
  assert.equal(fx.calls.filter(c => c.method === 'DELETE').length, 2);
});

test('wrong installation owner, App, selection, suspension or extra permissions are refused before minting', async () => {
  for (const installation of [{ app_id: 1 }, { account: { login: 'elsewhere' } }, { repository_selection: 'all' }, { suspended_at: 'yesterday' }, { permissions: { actions: 'write', contents: 'write', administration: 'write' } }]) {
    const fx = fixture({ installation });
    await assert.rejects(withReleaseGithubToken(() => assert.fail('must not execute'), fx.options), /installation authority/);
    assert.equal(fx.calls.length, 1);
  }
});

test('missing, broader or short-lived token grants are refused and issued tokens are revoked', async () => {
  for (const minted of [{ permissions: { actions: 'read', contents: 'write' } }, { permissions: { actions: 'write', contents: 'write', secrets: 'write' } }, { expires_at: new Date(0).toISOString() }, { expires_at: new Date(Date.now() + 120_000).toISOString() }, { expires_at: new Date(Date.now() + 4 * 3600_000).toISOString() }]) {
    const fx = fixture({ minted });
    await assert.rejects(withReleaseGithubToken(() => assert.fail('must not execute'), fx.options), /permissions or lifetime/);
    assert.equal(fx.calls.at(-1).method, 'DELETE');
  }
});

test('repository ID/name/count must all match; invalid scope is revoked before any effect', async () => {
  for (const scope of [{ total_count: 2 }, { repositories: [] }, { repositories: [{ ...RELEASE_REPOSITORY, id: 1 }] }, { repositories: [{ ...RELEASE_REPOSITORY, full_name: 'AgentWorkforce/relay-desktop' }] }]) {
    const fx = fixture({ scope });
    await assert.rejects(withReleaseGithubToken(() => assert.fail('must not execute'), fx.options), /not restricted/);
    assert.equal(fx.calls.at(-1).method, 'DELETE');
  }
});

test('effect failures revoke the token and redact key/JWT/token material', async () => {
  const fx = fixture();
  await assert.rejects(withReleaseGithubToken(token => { throw Error(`${key} ${token} ${fx.calls[0].headers.Authorization.slice(7)}`); }, fx.options), error => {
    assert.doesNotMatch(error.message, /PRIVATE KEY|step-token|eyJ/); return true;
  });
  assert.equal(fx.calls.at(-1).method, 'DELETE');
});

test('network and revocation errors fail closed without returning provider response bodies', async () => {
  const fx = fixture({ revokeStatus: 403 });
  await assert.rejects(withReleaseGithubToken(() => 'success', fx.options), /HTTP 403/);
  await assert.rejects(withReleaseGithubToken(() => assert.fail(), { ...fx.options, fetchImpl: async () => { throw Error(key); } }), error => {
    assert.doesNotMatch(error.message, /PRIVATE KEY/); return /no credential fallback/.test(error.message);
  });
});

test('production CLI never substitutes an inherited general token for missing App credentials', () => {
  const env = { ...process.env, GH_TOKEN: 'unrelated', GITHUB_TOKEN: 'unrelated', DESKTOP_RELEASE_GH_TOKEN: 'unrelated' };
  for (const name of RELEASE_APP_SECRETS) delete env[name];
  const result = spawnSync(process.execPath, ['flows/lib/release-worker.mjs', 'inventory', 'e30='], { cwd: new URL('../..', import.meta.url), env, encoding: 'utf8' });
  assert.equal(result.status, 1); assert.match(result.stderr, /App credentials are not configured/);
});
