// Cloud owns release state and decisions. Actions only execute native work and
// independently verify the evidence/artifacts at credential boundaries.
// GH access is a Cloud-managed secret; the host's tokens are never uploaded.
import { flow, type Ctx } from '@relayflows/surface';

interface Readiness {
  sourceTree: string;
  reviewRunId: string;
  reviewReported: boolean;
  openBlockers: number;
  blockerFixesReviewed: boolean;
  localCiPassed: boolean;
}
interface ReleaseInput {
  operation: 'prepare' | 'publish' | 'tick' | 'inventory';
  channel: 'stable' | 'beta';
  requestId: string;
  ref?: string;
  profile?: string;
  gateRunId?: number;
  candidateRunId?: number;
  /** Trusted lead attestation of the ledger/round, not a claim from an author. */
  readiness?: Readiness;
}
interface Plan {
  sourceSha: string;
  sourceTree: string;
  ref?: string;
  gateRunId?: number;
  candidateRunId?: number;
  version?: string;
  current?: boolean;
}
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
async function effect<T>(f: Ctx, action: string, payload: unknown): Promise<T> {
  const encoded = Buffer.from(JSON.stringify(payload)).toString('base64');
  return JSON.parse(await f.run(`node flows/lib/release-worker.mjs ${quote(action)} ${quote(encoded)}`, {
    secrets: ['DESKTOP_RELEASE_GH_APP_ID', 'DESKTOP_RELEASE_GH_INSTALLATION_ID', 'DESKTOP_RELEASE_GH_APP_PRIVATE_KEY'], timeout: '3m',
  })) as T;
}
async function dispatch(f: Ctx, input: ReleaseInput, plan: Plan, phase: string): Promise<number> {
  const submitted = await effect<{ runId?: number; title?: string; workflow?: string; sourceSha?: string }>(f, 'dispatch', { input, plan, phase });
  if (submitted.runId) return submitted.runId;
  // Submission is a journaled step, never repeated when the run is merely
  // slow to appear. Match its explicit request ID, not the newest timestamp.
  for (let attempt = 0; attempt < 24; attempt++) {
    const found = await effect<{ runId?: number }>(f, 'find', { dispatch: submitted });
    if (found.runId) return found.runId;
    await f.run('sleep 5');
  }
  throw Error(`worker dispatch not visible (${submitted.title}); inspect before submitting another flow`);
}
async function wait(f: Ctx, runId: number): Promise<void> {
  let retried = false;
  for (let poll = 0; poll < 240; poll++) {
    const run = await effect<{ status: string; conclusion: string }>(f, 'inspect', { runId });
    if (run.status === 'completed') {
      if (run.conclusion === 'success') return;
      if (!retried) {
        const retry = await effect<{ retried: boolean }>(f, 'retry', { runId });
        if (retry.retried) { retried = true; await f.run('sleep 30'); continue; }
      }
      throw Error(`worker ${runId} ended ${run.conclusion}; release stopped`);
    }
    await f.run('sleep 30');
  }
  throw Error(`worker ${runId} exceeded the two-hour wait; inspect its existing run`);
}
export default flow<ReleaseInput>('relay-app.cloud-release', { budget: { wallclock: '180m' } }, async (f, input) => {
  if (input.operation === 'inventory') {
    const candidates = await effect<unknown[]>(f, 'inventory', { input });
    await effect(f, 'result', { result: { status: 'inventory', candidates } });
    return f.done('success', { detail: `${candidates.length} candidate(s); inventory only` });
  }
  let plan = await effect<Plan>(f, 'resolve', { input });
  if (plan.current) return f.done('success', { detail: `candidate already current for ${plan.sourceSha}` });
  if (input.operation !== 'publish') {
    if (!plan.gateRunId) plan.gateRunId = await dispatch(f, input, plan, 'gate');
    // Native compilation overlaps the gate. The prepare worker independently
    // waits for and verifies this gate before it can produce a candidate.
    const producer = await dispatch(f, input, plan, 'prepare');
    await wait(f, plan.gateRunId);
    await effect(f, 'verify-gate', { plan });
    await wait(f, producer);
    plan = { ...plan, ...await effect<Plan>(f, 'candidate', { input, plan, runId: producer }) };
    await effect(f, 'result', { result: { status: 'ready', ...plan, channel: input.channel, profile: input.profile ?? '' } });
    return f.done('success', { detail: `candidate ${producer} ready for ${plan.sourceSha}; publication is a separate request` });
  }
  if (!plan.gateRunId) throw Error('publication requires an explicit CI run for this source tree');
  await wait(f, plan.gateRunId);
  await effect(f, 'verify-gate', { plan });
  if (input.channel === 'stable' && !await f.human(`Publish stable v${plan.version}, source ${plan.sourceSha}, candidate ${plan.candidateRunId}${input.profile ? `, profile ${input.profile}` : ''}? Confirm directly; relayed approval does not count.`, { to: 'khaliq-gant' })) {
    return f.done('needs_human', { detail: 'stable publication declined' });
  }
  const promotion = await dispatch(f, input, plan, 'promote');
  await wait(f, promotion);
  await effect(f, 'tag', { input, plan, runId: promotion });
  await effect(f, 'result', { result: { status: 'published', ...plan, promoteRunId: promotion, channel: input.channel, profile: input.profile ?? '', hostedUploadRecovery: 'pending' } });
  f.done('success', { detail: `published v${plan.version}, source ${plan.sourceSha}, promote ${promotion}; hosted recovery still needs live verification` });
});
