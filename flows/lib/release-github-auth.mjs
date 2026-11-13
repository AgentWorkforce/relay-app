// A fresh, single-repository installation token for one three-minute effect.
// Only this process sees the dedicated App key. Nothing is cached across steps.
import { createSign } from 'node:crypto';

export const RELEASE_REPOSITORY = { id: 1455893451, full_name: 'AgentWorkforce/relay-app' };
export const RELEASE_APP_SECRETS = ['DESKTOP_RELEASE_GH_APP_ID', 'DESKTOP_RELEASE_GH_INSTALLATION_ID', 'DESKTOP_RELEASE_GH_APP_PRIVATE_KEY'];
const permissions = { actions: 'write', contents: 'write' };
const exactPermissions = value => value && typeof value === 'object'
  && Object.entries(permissions).every(([key, level]) => value[key] === level)
  && Object.entries(value).every(([key, level]) => key === 'metadata' ? level === 'read' : permissions[key] === level);

export async function withReleaseGithubToken(effect, { env = process.env, fetchImpl = fetch, now = Date.now } = {}) {
  const [appId, installationId, privateKey] = RELEASE_APP_SECRETS.map(name => env[name]);
  if (!/^[1-9][0-9]*$/.test(appId ?? '') || !/^[1-9][0-9]*$/.test(installationId ?? '') || !privateKey) {
    throw Error('Dedicated release App credentials are not configured; provision the three Cloud workspace secrets');
  }
  const seconds = Math.floor(now() / 1000);
  const encode = value => Buffer.from(JSON.stringify(value)).toString('base64url');
  const unsigned = `${encode({ alg: 'RS256', typ: 'JWT' })}.${encode({ iat: seconds - 60, exp: seconds + 540, iss: appId })}`;
  let jwt;
  try { jwt = `${unsigned}.${createSign('RSA-SHA256').update(unsigned).sign(privateKey).toString('base64url')}`; }
  catch { throw Error('Could not sign the dedicated release App JWT'); }
  const request = async (path, bearer, method = 'GET', body) => {
    let response;
    try {
      response = await fetchImpl(`https://api.github.com${path}`, {
        method, redirect: 'error', signal: AbortSignal.timeout(15_000),
        headers: { Authorization: `Bearer ${bearer}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'relay-app-cloud-release', 'Content-Type': 'application/json' },
        ...(body ? { body: JSON.stringify(body) } : {}),
      });
    } catch { throw Error('Release App GitHub request failed; no credential fallback'); }
    if (!response.ok) throw Error(`Release App GitHub request failed (HTTP ${response.status}); no credential fallback`);
    if (response.status === 204) return;
    try { return await response.json(); }
    catch { throw Error('Release App GitHub returned an unreadable response'); }
  };
  let token;
  try {
    const installation = await request(`/app/installations/${installationId}`, jwt);
    if (String(installation.app_id) !== appId || installation.account?.login !== 'AgentWorkforce'
        || installation.repository_selection !== 'selected' || installation.suspended_at != null
        || !exactPermissions(installation.permissions)) throw Error('Release App installation authority does not match the dedicated release identity');
    const minted = await request(`/app/installations/${installationId}/access_tokens`, jwt, 'POST', {
      repository_ids: [RELEASE_REPOSITORY.id], permissions,
    });
    token = typeof minted.token === 'string' && minted.token ? minted.token : undefined;
    const expiry = Date.parse(minted.expires_at);
    if (!token || !exactPermissions(minted.permissions) || !Number.isFinite(expiry)
        || expiry - now() < 5 * 60_000 || expiry - now() > 61 * 60_000) throw Error('Release App token permissions or lifetime are invalid');
    const scope = await request('/installation/repositories?per_page=2', token);
    if (scope.total_count !== 1 || scope.repositories?.length !== 1
        || scope.repositories[0].id !== RELEASE_REPOSITORY.id
        || scope.repositories[0].full_name !== RELEASE_REPOSITORY.full_name) throw Error('Release App token is not restricted to the release repository');
    return await effect(token);
  } catch (error) {
    let message = error instanceof Error ? error.message : 'Release App operation failed';
    for (const value of [privateKey, jwt, token]) if (value) message = message.replaceAll(value, '[redacted]');
    throw Error(message);
  } finally {
    // Revocation is required even when validation or the effect fails. A
    // failed revoke stops the flow; immutable request refs prevent redispatch.
    if (token) await request('/installation/token', token, 'DELETE');
  }
}
