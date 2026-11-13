// Host bridge: submit trusted main's flow; retain only the Cloud run pointer.
import { randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { validate } from './release-worker.mjs';
import { RELEASE_APP_SECRETS } from './release-github-auth.mjs';
export function requestFromArgs(args, read = path => JSON.parse(readFileSync(path, 'utf8')), requestId = randomUUID()) {
  const argv = [...args]; const mode = argv.shift();
  const operation = mode === 'prepare' ? 'prepare' : mode === 'tick' ? 'tick' : mode === 'inventory' ? 'inventory' : 'publish';
  const channel = operation === 'publish' ? mode : (argv.shift() ?? 'beta');
  const input = { operation, channel, requestId, profile: '' };
  if (operation === 'prepare') input.ref = argv.shift();
  while (argv.length) {
    const name = argv.shift(); const value = argv.shift();
    if (!value) throw Error(`${name} needs a value`);
    if (name === '--profile') input.profile = value;
    else if (name === '--run') input.candidateRunId = value;
    else if (name === '--gate') input.gateRunId = value;
    else if (name === '--readiness') input.readiness = read(value);
    else if (name === '--request-id') input.requestId = value;
    else throw Error(`unknown release option ${name}`);
  }
  if (operation === 'prepare' && !input.ref) throw Error('prepare needs a source ref');
  if (operation === 'publish' && !input.readiness) throw Error('publication needs --readiness <lead-receipt.json>');
  return validate(input);
}
export function submission(out) {
  const result = JSON.parse(out);
  if (result.ok !== true || typeof result.runId !== 'string' || !result.runId) throw Error(`Cloud did not acknowledge a release run (${result.code ?? 'unreadable'}). Do not resubmit blindly; inspect for an admitted run.`);
  return result;
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const input = requestFromArgs(process.argv.slice(2));
    if (!process.env.FLOWS_CLOUD_TOKEN) throw Error('Cloud release refused: missing FLOWS_CLOUD_TOKEN; no local fallback');
    mkdirSync('release-state', { recursive: true });
    writeFileSync('release-state/request.json', JSON.stringify(input, null, 2) + '\n');
    const clean = { ...process.env }; delete clean.GH_TOKEN; delete clean.GITHUB_TOKEN; delete clean.DESKTOP_RELEASE_GH_TOKEN;
    for (const name of RELEASE_APP_SECRETS) delete clean[name];
    const out = execFileSync('flows/node_modules/.bin/flows', ['run', '--cloud', '--json', '--sync-code', '--no-connect', '--budget-minutes', '180', 'flows/cloud-release.flow.ts', '--input', JSON.stringify(input)], { encoding: 'utf8', env: clean, stdio: ['ignore', 'pipe', 'inherit'] });
    const accepted = submission(out);
    writeFileSync('release-state/run.json', JSON.stringify(accepted, null, 2) + '\n');
    console.log(JSON.stringify(accepted));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
