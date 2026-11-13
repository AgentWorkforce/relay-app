import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
const source = (name: string) => readFileSync(new URL(`../${name}`, import.meta.url), 'utf8');
function executable(path: string, script: string) { writeFileSync(path, `#!/bin/bash\n${script}`, { mode: 0o755 }); }

test('release and tick switches preserve legacy, delegate cloud arguments and propagate bridge failure', () => {
  const root = mkdtempSync(join(tmpdir(), 'release-switch-'));
  try {
    const bridge = join(root, 'bridge');
    executable(bridge, 'printf bridge; for arg; do printf "|%s" "$arg"; done; printf "\\n"; exit "${BRIDGE_EXIT:-0}"\n');
    for (const name of ['release.sh', 'tick.sh']) {
      // Execute the real entry point through its dispatch boundary; the
      // unchanged legacy body is represented by a sentinel, not executed.
      const script = source(name);
      const boundary = script.indexOf('REPO=AgentWorkforce/relay-app');
      assert.ok(boundary > 0);
      const prefix = script.slice(0, boundary).replaceAll('/opt/agent-host/cloud-release.sh', bridge);
      for (const flag of [undefined, 'legacy', 'cloud']) {
        const env: NodeJS.ProcessEnv = { ...process.env, BRIDGE_EXIT: '0' };
        delete env.DESKTOP_RELEASE_ORCHESTRATOR;
        if (flag !== undefined) env.DESKTOP_RELEASE_ORCHESTRATOR = flag;
        const args = name === 'release.sh' ? ['stable', '--run', '123'] : [];
        const result = spawnSync('bash', ['-c', prefix + '\nprintf "legacy\\n"', 'entry', ...args], { env, encoding: 'utf8' });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout, flag === 'cloud' ? `bridge|${name === 'release.sh' ? 'stable|--run|123' : 'tick|beta'}\n` : 'legacy\n');
      }
      const failed = spawnSync('bash', ['-c', prefix + '\nprintf "legacy\\n"', 'entry', 'stable'], { env: { ...process.env, DESKTOP_RELEASE_ORCHESTRATOR: 'cloud', BRIDGE_EXIT: '23' }, encoding: 'utf8' });
      assert.equal(failed.status, 23);
      assert.ok(!failed.stdout.includes('legacy'));
      if (name === 'tick.sh') {
        const refresh = spawnSync('bash', ['-c', prefix, 'entry', '--refresh'], { env: { ...process.env, DESKTOP_RELEASE_ORCHESTRATOR: 'cloud' }, encoding: 'utf8' });
        assert.equal(refresh.stdout, 'bridge|inventory|beta\n');
      }
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('bootstrap cleans every source checkout on clone, npm and submit failure while retaining diagnostics', () => {
  for (const [stage, expected] of [['clone', 13], ['npm', 17], ['node', 1], ['success', 0]] as const) {
    const root = mkdtempSync(join(tmpdir(), 'release-bootstrap-'));
    try {
      const bin = join(root, 'bin'); mkdirSync(bin);
      const secrets = join(root, 'secrets.env');
      writeFileSync(secrets, "FLOWS_CLOUD_TOKEN='fixture-cloud-token'\n");
      executable(join(bin, 'flock'), 'exit 0\n');
      executable(join(bin, 'git'), `if [ "$1" = clone ]; then
  for dest; do :; done
  mkdir -p "$dest"
  printf checkout > "$dest/checkout.bin"
  [ "$FAIL_STAGE" != clone ] || exit 13
elif [[ "$*" == *rev-parse* ]]; then printf '%040d\\n' 1
fi
`);
      executable(join(bin, 'npm'), 'mkdir -p release-state; printf setup > release-state/setup.log; [ "$FAIL_STAGE" != npm ] || exit 17\n');
      executable(join(bin, 'node'), `mkdir -p release-state
printf '{"operation":"prepare"}' > release-state/request.json
printf submit > release-state/submit.log
[ "$FAIL_STAGE" != node ] || exit 19
printf '{"runId":"cloud-fixture"}' > release-state/run.json
`);
      // Isolate only the home-path reference; execute the actual bootstrap
      // and EXIT trap against fake commands, with no real clone or Cloud run.
      const script = source('cloud-release.sh').replaceAll('$HOME', '${TEST_HOME}');
      const result = spawnSync('bash', ['-c', script, 'bridge', 'prepare', 'stable', 'main'], {
        env: { ...process.env, TEST_HOME: root, REVIEW_SECRETS_FILE: secrets, FAIL_STAGE: stage, PATH: `${bin}:${process.env.PATH}` }, encoding: 'utf8',
      });
      assert.equal(result.status, expected, `${stage}: ${result.stderr}`);
      const state = join(root, 'state/cloud-releases');
      const [entry] = readdirSync(state);
      assert.ok(entry);
      const work = join(state, entry);
      assert.equal(existsSync(join(work, 'source')), false, stage);
      if (stage !== 'clone') assert.equal(readFileSync(join(work, 'release-state/setup.log'), 'utf8'), 'setup');
      if (stage === 'node' || stage === 'success') {
        assert.deepEqual(JSON.parse(readFileSync(join(work, 'request.json'), 'utf8')), { operation: 'prepare' });
        assert.equal(readFileSync(join(work, 'release-state/submit.log'), 'utf8'), 'submit');
      }
      if (stage === 'success') assert.deepEqual(JSON.parse(readFileSync(join(work, 'run.json'), 'utf8')), { runId: 'cloud-fixture' });
    } finally { rmSync(root, { recursive: true, force: true }); }
  }
});
