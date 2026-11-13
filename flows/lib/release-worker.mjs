// Small GitHub effects used by cloud-release.flow.ts. Decisions and waits live
// in the flow; no credentials, polling loops or release state live on the host.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { withReleaseGithubToken, RELEASE_APP_SECRETS } from './release-github-auth.mjs';

export const REPO = 'AgentWorkforce/relay-app';
const sha = /^[0-9a-f]{40}$/;
const id = /^[1-9][0-9]*$/;
export function freshCiRun(run, now = Date.now()) {
  const created = Date.parse(run.created_at);
  // Match release-ci-evidence.py: seven days old, five minutes of clock skew.
  return Number.isFinite(created) && created <= now + 5 * 60_000 && now - created <= 7 * 24 * 60 * 60_000;
}
export function validate(input) {
  if (!input || !['prepare', 'publish', 'tick', 'inventory'].includes(input.operation)) throw Error('operation must be prepare, publish, tick or inventory');
  if (!['beta', 'stable'].includes(input.channel)) throw Error('channel must be beta or stable');
  if (!/^[a-zA-Z0-9-]{8,80}$/.test(input.requestId ?? '')) throw Error('a unique request ID is required');
  if (!/^(?:|[a-z0-9]+(?:-[a-z0-9]+)*)$/.test(input.profile ?? '') || (input.profile?.length ?? 0) > 40) throw Error('invalid profile');
  if (!/^[a-zA-Z0-9][a-zA-Z0-9._/-]{0,180}$/.test(input.ref ?? 'main') || (input.ref ?? '').includes('..')) throw Error('invalid source ref');
  for (const key of ['gateRunId', 'candidateRunId']) if (input[key] !== undefined && !id.test(String(input[key]))) throw Error(`invalid ${key}`);
  return input;
}
export function candidateIdentity(run, log) {
  if (run.repository?.full_name !== REPO || run.head_repository?.full_name !== REPO || run.path !== '.github/workflows/desktop-release.yml') throw Error('not a source-repository candidate producer');
  const source = [...log.matchAll(/\bSource ([0-9a-f]{40})\b/g)].map(m => m[1]);
  const version = [...log.matchAll(/\bReleasing v([0-9]{4}\.[0-9]{1,2}\.[0-9]+(?:-beta\.[0-9]+)?)\b/g)].map(m => m[1]);
  if (new Set(source).size !== 1 || new Set(version).size !== 1) throw Error('candidate has no unambiguous Source/version lines');
  if (run.event === 'workflow_dispatch' && source[0] !== run.head_sha) throw Error('candidate Source differs from dispatched commit');
  if (!['workflow_dispatch', 'workflow_run'].includes(run.event)) throw Error('unsupported candidate event');
  const profiles = [...log.matchAll(/\bProfile ([a-z0-9-]+)\b/g)].map(m => m[1]);
  const gates = [...log.matchAll(/\bCI run ([1-9][0-9]*)\b/g)].map(m => Number(m[1]));
  if (new Set(profiles).size > 1 || new Set(gates).size > 1) throw Error('candidate has ambiguous profile/CI identity');
  return { candidateRunId: run.id, sourceSha: source[0], version: version[0], channel: version[0].includes('-beta.') ? 'beta' : 'stable', profile: profiles[0] ?? '', gateRunId: gates[0] };
}
export function assertReadiness(receipt, tree) {
  if (!receipt || receipt.sourceTree !== tree || receipt.reviewReported !== true || receipt.openBlockers !== 0 || receipt.blockerFixesReviewed !== true || receipt.localCiPassed !== true || !/^[a-zA-Z0-9-]{1,100}$/.test(receipt.reviewRunId ?? '')) throw Error('the lead must attest a reported review, no open blockers, reviewed blocker fixes and green local CI for this exact tree');
}
export const RETRY_TESTS = new Set([
  'probe_copy::tests::a_missing_or_older_copy_is_replaced_and_an_identical_one_kept',
  'session_navigation::native::output::tests::deadline_closes_reader_without_waiting_for_or_killing_a_descendant',
]);
export function knownFlake(log) {
  const failures = [...log.matchAll(/\btest ([\w:]+) \.\.\. FAILED/g)].map(m => m[1]);
  return failures.length > 0 && failures.every(name => RETRY_TESTS.has(name)) && !/error: could not compile|error\[E[0-9]+\]/.test(log);
}
export function matchingRun(runs, title, sourceSha) {
  const matches = runs.filter(run => run.display_title === title && (!sourceSha || run.head_sha === sourceSha) && run.event === 'workflow_dispatch');
  if (matches.length > 1) throw Error(`ambiguous workflow dispatch for ${title}; refusing another dispatch`);
  return matches[0];
}
const env = () => {
  if (!process.env.DESKTOP_RELEASE_GH_TOKEN) throw Error('Cloud release credential DESKTOP_RELEASE_GH_TOKEN is not configured');
  const child = { ...process.env, GH_TOKEN: process.env.DESKTOP_RELEASE_GH_TOKEN };
  for (const name of [...RELEASE_APP_SECRETS, 'DESKTOP_RELEASE_GH_TOKEN', 'GITHUB_TOKEN']) delete child[name];
  return child;
};
const command = (bin, args, input) => execFileSync(bin, args, { encoding: 'utf8', input, env: env(), maxBuffer: 64 * 1024 * 1024, stdio: ['pipe', 'pipe', 'pipe'] }).trim();
const api = (path, body) => {
  const out = command('gh', ['api', `repos/${REPO}/${path}`, ...(body === undefined ? [] : ['--method', 'POST', '--input', '-'])], body === undefined ? undefined : JSON.stringify(body));
  return out ? JSON.parse(out) : {};
};
const run = runId => api(`actions/runs/${runId}`);
const jobs = runId => {
  const all = []; let page = 1;
  for (;;) { const batch = api(`actions/runs/${runId}/jobs?filter=latest&per_page=100&page=${page++}`).jobs; all.push(...batch); if (batch.length < 100) return all; }
};
const commit = ref => api(`commits/${encodeURIComponent(ref)}`);
const ciEvidence = (runId, sourceSha) => JSON.parse(command('python3', ['scripts/release-ci-evidence.py', '--run-id', String(runId), '--source-sha', sourceSha]));
const identity = candidateRunId => {
  const producer = run(candidateRunId);
  const resolve = jobs(candidateRunId).filter(job => job.name === 'Resolve version');
  if (resolve.length !== 1 || resolve[0].conclusion !== 'success') throw Error('candidate has not resolved its identity');
  return { ...candidateIdentity(producer, command('gh', ['api', `repos/${REPO}/actions/jobs/${resolve[0].id}/logs`])), status: producer.status, conclusion: producer.conclusion };
};
const list = workflow => api(`actions/workflows/${workflow}/runs?per_page=100`).workflow_runs;
export function dispatchInputs(phase, input, plan) {
  const digest = createHash('sha256').update(JSON.stringify({ phase, source: plan.sourceSha, candidate: plan.candidateRunId, gate: plan.gateRunId, channel: input.channel, profile: input.profile ?? '' })).digest('hex').slice(0, 16);
  const requestRoot = input.operation === 'tick' ? `tick-${plan.sourceSha}-${input.channel}-${input.profile || 'default'}` : input.requestId;
  const request = `${requestRoot}-${phase}-${digest}`;
  const inputs = { 'request-id': request };
  if (phase === 'gate') Object.assign(inputs, { 'full-matrix': 'true', 'force-all': 'true', 'skip-packages': 'true' });
  if (phase === 'prepare') Object.assign(inputs, { channel: input.channel, profile: input.profile ?? '', 'prepare-only': 'true', 'ci-run-id': String(plan.gateRunId), 'expected-source-sha': plan.sourceSha });
  if (phase === 'promote') Object.assign(inputs, { channel: input.channel, profile: input.profile ?? '', version: plan.version, 'candidate-run-id': String(plan.candidateRunId) });
  return inputs;
}
export function perform(action, payload) {
  const { input, plan = {}, phase, runId } = payload;
  if (input) validate(input);
  if (action === 'inventory') {
    const rows = [];
    for (const producer of list('desktop-release.yml')) {
      try { const row = identity(producer.id); if (row.channel === input.channel && row.profile === (input.profile ?? '')) rows.push(row); } catch { /* incomplete identity is not a candidate */ }
    }
    return rows;
  }
  if (action === 'resolve') {
    if (input.operation === 'publish') {
      let candidateRunId = input.candidateRunId;
      if (!candidateRunId) {
        for (const producer of list('desktop-release.yml').filter(r => r.conclusion === 'success')) {
          try { const row = identity(producer.id); if (row.channel === input.channel && row.profile === (input.profile ?? '')) { candidateRunId = row.candidateRunId; break; } } catch { /* skipped producer or unreadable logs is never a candidate */ }
        }
      }
      if (!candidateRunId) throw Error('no ready candidate; prepare one first');
      const row = identity(candidateRunId);
      if (row.status !== 'completed' || row.conclusion !== 'success') throw Error('candidate is not ready; wait for its existing Cloud/Actions run');
      if (row.channel !== input.channel || row.profile !== (input.profile ?? '')) throw Error('candidate channel/profile mismatch');
      const sourceTree = commit(row.sourceSha).commit.tree.sha;
      assertReadiness(input.readiness, sourceTree);
      return { ...row, sourceTree, gateRunId: input.gateRunId ?? row.gateRunId };
    }
    const source = commit(input.ref ?? 'main');
    if (!sha.test(source.sha)) throw Error('source did not resolve to a commit');
    const plan = { sourceSha: source.sha, sourceTree: source.commit.tree.sha, ref: input.ref ?? 'main', gateRunId: input.gateRunId };
    if (input.operation === 'tick') {
      // Inventory is reconstructed from immutable producers, not host JSON.
      for (const producer of list('desktop-release.yml')) {
        if (producer.status === 'completed' && producer.conclusion !== 'success') continue;
        try { const row = identity(producer.id); if (row.channel === input.channel && row.profile === (input.profile ?? '')) {
          if (row.sourceSha === source.sha) return { ...plan, ...row, current: true };
          if (producer.conclusion === 'success') {
            const diff = api(`compare/${row.sourceSha}...${source.sha}`);
            if (diff.status === 'ahead' && diff.files?.length < 300) {
              const lanes = command('python3', ['scripts/ci-changes.py'], diff.files.flatMap(file => [file.filename, ...(file.previous_filename ? [file.previous_filename] : [])]).join('\0') + '\0');
              if (!lanes.includes('=true')) return { ...plan, ...row, current: true };
            }
          }
          break; // newest candidate of this channel/profile decides
        } } catch { /* no complete candidate */ }
      }
    }
    if (!plan.gateRunId) {
      const green = api(`actions/workflows/desktop-ci.yml/runs?head_sha=${source.sha}&branch=main&event=push&status=success&per_page=20`).workflow_runs;
      const reusable = green.find(run => freshCiRun(run));
      if (reusable) plan.gateRunId = reusable.id;
    }
    return plan;
  }
  if (action === 'dispatch') {
    const workflow = { gate: 'desktop-ci.yml', prepare: 'desktop-release.yml', promote: 'desktop-promote.yml' }[phase];
    if (!workflow) throw Error('invalid worker phase');
    const ref = phase === 'promote' ? 'main' : plan.ref;
    const expected = phase === 'promote' ? commit('main').sha : plan.sourceSha;
    if (phase !== 'promote' && commit(ref).sha !== expected) throw Error('source ref moved; resolve and gate the new source in a new flow');
    const inputs = dispatchInputs(phase, input, plan);
    const title = `cloud-release/${inputs['request-id']}`;
    const existing = matchingRun(list(workflow), title, phase === 'promote' ? undefined : expected);
    if (existing) return { runId: existing.id };
    // GitHub dispatch has no idempotency key. Reserve an atomic, durable ref
    // before POST. A lost response never causes a second dispatch: a retry
    // only discovers the admitted run, or stops for an operator to inspect.
    const lock = `release-requests/${inputs['request-id']}`;
    let reserved = false;
    try { api('git/refs', { ref: `refs/heads/${lock}`, sha: plan.sourceSha }); reserved = true; }
    catch (error) {
      if (!String(error.stderr).includes('422')) throw error;
      const held = api(`git/ref/heads/${lock}`);
      if (held.object.sha !== plan.sourceSha) throw Error('request reservation points to another source');
    }
    if (reserved) api(`actions/workflows/${workflow}/dispatches`, { ref, inputs });
    return { title, workflow, sourceSha: phase === 'promote' ? undefined : expected };
  }
  if (action === 'find') {
    const found = matchingRun(list(payload.dispatch.workflow), payload.dispatch.title, payload.dispatch.sourceSha);
    return found ? { runId: found.id } : {};
  }
  if (action === 'inspect') { const current = run(runId); return { status: current.status, conclusion: current.conclusion, attempt: current.run_attempt }; }
  if (action === 'verify-gate') return ciEvidence(plan.gateRunId, plan.sourceSha);
  if (action === 'candidate') {
    const row = identity(runId);
    if (row.status !== 'completed' || row.conclusion !== 'success' || row.sourceSha !== plan.sourceSha || row.channel !== input.channel || row.profile !== (input.profile ?? '')) throw Error('producer is not a successful candidate of the expected source/channel/profile');
    return row;
  }
  if (action === 'retry') {
    const current = run(runId);
    if (current.status !== 'completed' || current.conclusion !== 'failure' || current.run_attempt !== 1) return { retried: false };
    const failed = jobs(runId).filter(job => job.conclusion === 'failure');
    if (!failed.length || failed.some(job => !knownFlake(command('gh', ['api', `repos/${REPO}/actions/jobs/${job.id}/logs`])))) return { retried: false };
    // One run-level failed-job retry, only when every failure is allowlisted.
    api(`actions/runs/${runId}/rerun-failed-jobs`, {});
    return { retried: true };
  }
  if (action === 'tag') {
    const promoted = run(runId);
    if (promoted.path !== '.github/workflows/desktop-promote.yml' || promoted.repository?.full_name !== REPO || promoted.conclusion !== 'success' || promoted.status !== 'completed') throw Error('promotion has not succeeded');
    if (input.profile) return { tagged: false };
    const tag = `released-v${plan.version}`;
    // An annotated tag is created once. Existing tags are peeled and checked,
    // never force-updated, including after a lost response or flow restart.
    let existing;
    try { existing = api(`git/ref/tags/${tag}`); } catch (error) { if (!String(error.stderr).includes('404')) throw error; }
    if (existing) {
      let object = existing.object;
      while (object.type === 'tag') object = api(`git/tags/${object.sha}`).object;
      if (object.sha !== plan.sourceSha) throw Error('released tag points elsewhere; refusing to move it');
    } else {
      const annotated = api('git/tags', { tag, message: `relay-app v${plan.version}; candidate ${plan.candidateRunId}, promote ${runId}`, object: plan.sourceSha, type: 'commit' });
      api('git/refs', { ref: `refs/tags/${tag}`, sha: annotated.sha });
    }
    return { tagged: true, tag };
  }
  if (action === 'result') {
    mkdirSync('release-state', { recursive: true });
    const result = payload.result;
    if (result.status === 'published' && result.channel === 'stable' && !result.profile) {
      const directory = `release-state/promoted/v${result.version}`;
      command('gh', ['run', 'download', String(result.promoteRunId), '--repo', REPO, '--name', `promoted-changelog-v${result.version}`, '--dir', directory]);
    }
    writeFileSync('release-state/result.json', JSON.stringify(result, null, 2) + '\n');
    return result;
  }
  throw Error(`unknown release worker action: ${action}`);
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const result = await withReleaseGithubToken(token => {
      process.env.DESKTOP_RELEASE_GH_TOKEN = token;
      try { return perform(process.argv[2], JSON.parse(Buffer.from(process.argv[3], 'base64').toString('utf8'))); }
      finally { delete process.env.DESKTOP_RELEASE_GH_TOKEN; }
    });
    console.log(JSON.stringify(result));
  }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
