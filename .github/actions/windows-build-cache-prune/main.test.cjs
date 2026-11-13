'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const { pruneWorkspaceArtifacts } = require('./main.cjs');

const LIB = '1111111111111111';
const APP = '2222222222222222';
const HOSTING = '3333333333333333';
const BUILD = '4444444444444444';
const DEPENDENCY = 'aaaaaaaaaaaaaaaa';

function fixture(t, profile = 'debug') {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'relay-cache-prune-'));
  t.after(() => fs.rmSync(workspace, { recursive: true, force: true }));
  function write(relative, contents = 'artifact') {
    const filename = path.join(workspace, relative);
    fs.mkdirSync(path.dirname(filename), { recursive: true });
    fs.writeFileSync(filename, contents);
    return filename;
  }
  write('desktop-core/Cargo.toml', '[package]\nname = "agent-relay-desktop-core"\nversion = "0.0.0"\n');
  const prefix = `target/${profile}`;
  return { workspace, prefix, write, exists: (relative) => fs.existsSync(path.join(workspace, relative)) };
}

test('removes workspace libraries, binaries and tests while preserving dependency products', (t) => {
  const f = fixture(t);
  for (const hash of [LIB, APP, HOSTING, BUILD]) {
    f.write(`${f.prefix}/.fingerprint/agent-relay-desktop-core-${hash}/marker`);
  }
  f.write(`${f.prefix}/build/agent-relay-desktop-core-${BUILD}/build-script-build.exe`);
  const removed = [
    `libagent_relay_desktop_core-${LIB}.rlib`, `libagent_relay_desktop_core-${LIB}.rmeta`,
    `agent_relay_desktop_core-${LIB}.d`, `agent_relay-${APP}.exe`, `agent_relay-${APP}.pdb`,
    `linux_app_hosting-${HOSTING}.exe`, `linux_app_hosting-${HOSTING}.d`,
  ];
  for (const name of removed) f.write(`${f.prefix}/deps/${name}`);
  const retained = [
    `.fingerprint/gtk4-${DEPENDENCY}/marker`,
    `build/gtk4-sys-${DEPENDENCY}/build-script-build.exe`,
    `deps/libgtk4-${DEPENDENCY}.rlib`, `deps/glib_macros-${DEPENDENCY}.dll`,
    `deps/build_script_build-${DEPENDENCY}.exe`,
    // Similar package names and non-Cargo-looking names are not ours.
    `.fingerprint/agent-relay-desktop-core-helper-${DEPENDENCY}/marker`,
    `deps/unrelated-${DEPENDENCY}.exe`, `deps/readme-${LIB}-extra.txt`,
  ];
  for (const name of retained) f.write(`${f.prefix}/${name}`);
  f.write('desktop-core/src/keep.rs', 'source');
  f.write(`target/release/deps/agent_relay-${APP}.exe`);
  const result = pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'debug' });
  assert.deepEqual(result, { fingerprints: 4, buildDirectories: 1, dependencyFiles: 7, bytes: 96 });
  for (const name of removed) assert.equal(f.exists(`${f.prefix}/deps/${name}`), false);
  for (const name of retained) assert.equal(f.exists(`${f.prefix}/${name}`), true);
  assert.equal(f.exists('desktop-core/src/keep.rs'), true);
  assert.equal(f.exists(`target/release/deps/agent_relay-${APP}.exe`), true);
});

test('release profile accepts orphaned workspace build outputs without deleting unidentified dependencies', (t) => {
  const f = fixture(t, 'release');
  f.write(`${f.prefix}/build/agent-relay-desktop-core-${BUILD}/out/resource.o`);
  f.write(`${f.prefix}/deps/agent_relay_desktop_core-${LIB}.exe`);
  const result = pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'release' });
  assert.equal(result.buildDirectories, 1);
  assert.equal(result.dependencyFiles, 0);
  assert.equal(f.exists(`${f.prefix}/deps/agent_relay_desktop_core-${LIB}.exe`), true);
});

test('missing build or artifact directories are valid', (t) => {
  const f = fixture(t);
  assert.deepEqual(pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'debug' }), {
    fingerprints: 0, buildDirectories: 0, dependencyFiles: 0, bytes: 0,
  });
  fs.mkdirSync(path.join(f.workspace, f.prefix), { recursive: true });
  assert.equal(pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'debug' }).bytes, 0);
});

test('invalid profiles and unsafe manifest names are refused', (t) => {
  const f = fixture(t);
  for (const profile of ['', '../src', 'debug/../../src', 'Debug', undefined]) {
    assert.throws(() => pruneWorkspaceArtifacts({ workspace: f.workspace, profile }), /profile/);
  }
  f.write('desktop-core/Cargo.toml', '[package]\nname = "../outside"\n');
  assert.throws(() => pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'debug' }), /safe name/);
});

for (const parent of ['desktop-core', 'target', 'target/debug', 'target/debug/deps']) {
  test(`refuses symlink build parent ${parent}`, (t) => {
    const f = fixture(t);
    const outside = path.join(f.workspace, 'outside');
    fs.mkdirSync(outside);
    fs.writeFileSync(path.join(outside, 'keep'), 'outside');
    const filename = path.join(f.workspace, parent);
    fs.rmSync(filename, { recursive: true, force: true });
    fs.mkdirSync(path.dirname(filename), { recursive: true });
    fs.symlinkSync(outside, filename, 'junction');
    assert.throws(() => pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'debug' }), /unsafe|checked-out/);
    assert.equal(fs.readFileSync(path.join(outside, 'keep'), 'utf8'), 'outside');
  });
}

test('refuses symlink products before removing any planned workspace files', (t) => {
  const f = fixture(t);
  const marker = f.write(`${f.prefix}/.fingerprint/agent-relay-desktop-core-${LIB}/marker`);
  const outside = f.write('outside', 'keep');
  const product = path.join(f.workspace, f.prefix, 'deps', `agent_relay_desktop_core-${LIB}.exe`);
  fs.mkdirSync(path.dirname(product));
  fs.symlinkSync(outside, product, 'file');
  assert.throws(() => pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'debug' }), /unsafe/);
  assert.equal(fs.existsSync(marker), true);
  assert.equal(fs.readFileSync(outside, 'utf8'), 'keep');
});

test('prunes all three local packages sharing the target without removing dependencies', (t) => {
  const f = fixture(t, 'release');
  for (const [crate, name, hash, artifact] of [
    ['desktop-core', 'agent-relay-desktop-core', LIB, 'libagent_relay_desktop_core'],
    ['gtk', 'agent-relay-app', APP, 'libagent_relay_app'],
  ]) {
    f.write(`${crate}/Cargo.toml`, `[package]\nname = "${name}"\n`);
    f.write(`${f.prefix}/.fingerprint/${name}-${hash}/marker`);
    f.write(`${f.prefix}/build/${name}-${hash}/out/resource.o`);
    f.write(`${f.prefix}/deps/${artifact}-${hash}.rlib`);
  }
  f.write(`${f.prefix}/deps/libgtk4-${DEPENDENCY}.rlib`);
  const result = pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'release' });
  assert.equal(result.fingerprints, 2);
  assert.equal(result.buildDirectories, 2);
  assert.equal(result.dependencyFiles, 2);
  assert.equal(f.exists(`${f.prefix}/deps/libgtk4-${DEPENDENCY}.rlib`), true);
});

test('refuses symlink local package manifests before pruning', (t) => {
  const f = fixture(t);
  const outside = f.write('outside/Cargo.toml', '[package]\nname = "other"\n');
  fs.mkdirSync(path.join(f.workspace, 'gtk'));
  fs.symlinkSync(outside, path.join(f.workspace, 'gtk/Cargo.toml'));
  f.write(`${f.prefix}/.fingerprint/agent-relay-desktop-core-${LIB}/marker`);
  assert.throws(() => pruneWorkspaceArtifacts({ workspace: f.workspace, profile: 'debug' }), /regular/);
  assert.equal(f.exists(`${f.prefix}/.fingerprint/agent-relay-desktop-core-${LIB}/marker`), true);
});
