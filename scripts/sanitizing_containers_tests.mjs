import test from 'node:test';
import assert from 'node:assert/strict';
import { parseRegistry, runGate as containersGate } from './sanitizing_containers.mjs';

const attemptsScope = 'src-tauri/src/' + 'cli/role_experiment/';
const attemptsRegistry = () => ({
  version: 1,
  containers: [{
    id: 'RoleExperiment/Attempts', name: 'attempts.jsonl', scope: attemptsScope,
    owner: attemptsScope + 'storage.rs', externalLiteralBaseline: 0,
    writerInventory: { files: 0, sites: 0 },
  }],
});
function containerTree(files = {}, registry = attemptsRegistry()) {
  const blobs = {
    'scripts/containers.json': JSON.stringify(registry),
    [attemptsScope + 'storage.rs']: 'const NAME: &str = "attempts.jsonl";',
    ...files,
  };
  const headTree = new Map(Object.keys(blobs).map(path => [path, path]));
  return { headTree, readBlob: entry => blobs[entry], assertStableHead() {} };
}
test('04 accepts safe basenames and enforces their scoped owner', () => {
  for (const name of [
    'config.json', 'attempts.jsonl', '.team-config-write.lock', 'conventions.md',
    'Role.md', 'TASK.md', 'README', 'a_b-1.txt',
  ]) {
    const registry = attemptsRegistry();
    registry.containers[0].name = name;
    assert.equal(parseRegistry(JSON.stringify(registry))[0].name, name);
    const literal = 'const NAME: &str = "' + name + '";';
    const files = { [attemptsScope + 'storage.rs']: literal };
    assert.equal(containersGate(containerTree(files, registry)).exit, 0, name);
    files[attemptsScope + 'resume.rs'] = literal;
    const result = containersGate(containerTree(files, registry));
    assert.equal(result.exit, 1, name);
    assert.ok(result.diagnostics.some(d => d.rule === 'external_container_literal'), name);
  }
});
test('04 rejects unsafe basenames and nonstrings', () => {
  for (const name of [
    '', '.', '..', '../Role.md', './Role.md', 'a/b', 'a\\b', 'C:Role.md',
    'a b', ' a', 'a ', '\n', 'a\n', 'a:b', 'a*', 'é.md', null, 1, true, {}, [],
  ]) {
    const registry = attemptsRegistry();
    registry.containers[0].name = name;
    assert.throws(() => parseRegistry(JSON.stringify(registry)), String(name));
    assert.equal(containersGate(containerTree({}, registry)).status, 'ERROR');
  }
});

// Policy comes from the immutable candidate registry, never a fixture copy.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createSnapshot, gitRunner } from './sanitizing_baseline.mjs';
const repo = fileURLToPath(new URL('../', import.meta.url));
const candidateSnapshot = () => {
  const oid = gitRunner(['rev-parse', 'HEAD'], { cwd: repo });
  assert.equal(oid.status, 0);
  const head = oid.stdout.toString().trim();
  const snapshot = createSnapshot({ cwd: repo, baseRef: head });
  assert.equal(snapshot.headOid, head);
  return snapshot;
};
function settingsPolicy() {
  const s = candidateSnapshot();
  const entries = parseRegistry(s.readBlob(s.headTree.get('scripts/containers.json')));
  const selected = entries.filter(c => c.id === 'Instance/Settings');
  assert.equal(selected.length, 1);
  const c = selected[0];
  assert.equal(c.ownerModule, true);
  assert.equal(c.scope, 'src-tauri/src/');
  assert.equal(c.owner, 'src-tauri/src/config/settings.rs');
  assert.equal(c.name, 'settings.30.instance.no-git.json');
  assert.equal(c.metadataPolicy, 'instance-settings-v1');
  assert.deepEqual(c.constantPaths, ['', 'names::'].flatMap(prefix =>
    ['SETTINGS_FILE_NAME', 'SETTINGS_TARGET_NAME'].map(n =>
      'crate::config::instance_artifacts::' + prefix + n)));
  return c;
}
const root = 'src-tauri/src/';
const namesPath = root + 'config/instance_artifacts/names.rs';
const artifactsPath = root + 'config/instance_artifacts.rs';
const namesSource = [
  'pub(crate) const SETTINGS_FILE_NAME: &str = SETTINGS_TARGET_NAME;',
  'pub(crate) const SETTINGS_TARGET_NAME: &str =',
  '    layered_name!("settings", instance, no_git, "json");',
].join('\n');
const artifactsSource = [
  'mod names;',
  'pub(crate) use names::{SETTINGS_FILE_NAME};',
  '#[cfg(test)] pub(crate) use names::{SETTINGS_TARGET_NAME};',
  'pub(crate) const INSTANCE_ARTIFACTS: &[InstanceArtifact] = &[',
  '    InstanceArtifact {',
  '        name: SETTINGS_FILE_NAME,',
  '        kind: ArtifactKind::File,',
  '        disposition: Disposition::Ignore,',
  '        comment: "# AgentsCommander: application settings",',
  '    },',
  '];',
].join('\n');
function settingsTree(files = {}, policy = settingsPolicy()) {
  const blobs = {
    [root + 'lib.rs']: 'mod config;',
    [root + 'config.rs']: 'mod instance_artifacts; mod settings;',
    [policy.owner]: '', [root + 'config/settings/child.rs']: '',
    [namesPath]: namesSource, [artifactsPath]: artifactsSource, ...files,
  };
  return containerTree(blobs, { version: 1, containers: [policy] });
}
function control(t, label, files, expected, site, policy) {
  const result = containersGate(settingsTree(files, policy));
  t.diagnostic(JSON.stringify({ control: label, status: result.status, exit: result.exit,
    sites: result.diagnostics.filter(d => d.identity === 'Instance/Settings') }));
  assert.equal(result.exit, expected, label + ': ' + JSON.stringify(result.diagnostics));
  assert.equal(result.status, expected === 0 ? 'PASS' : expected === 1 ? 'FAIL' : 'ERROR');
  if (site) assert.ok(result.diagnostics.some(d => d.status === 'FAIL' &&
    d.identity === 'Instance/Settings' && d.path === site.path && d.line === site.line), label);
  return result;
}
const direct = 'crate::config::instance_artifacts::SETTINGS_FILE_NAME';
const runtime = 'fn runtime() { let _ = ' + direct + '; }';
const site = (p, line = 1) => ({ path: root + p, line });

test('04 settings owner boundary', t => {
  for (const p of ['config/settings.rs', 'config/settings/child.rs'])
    control(t, p, { [root + p]: runtime }, 0);
  for (const p of ['config/settings_extra.rs', 'config/settings_extra/child.rs'])
    control(t, p, { [root + p]: runtime }, 1, site(p));
});
test('04 settings metadata occurrences', t => {
  control(t, 'real shaped declarations', {}, 0);
  const s = candidateSnapshot();
  control(t, 'actual names and registry declarations', {
    [namesPath]: s.readBlob(s.headTree.get(namesPath)),
    [artifactsPath]: s.readBlob(s.headTree.get(artifactsPath)),
  }, 0);
  for (const [p, source] of [[namesPath, namesSource], [artifactsPath, artifactsSource]]) {
    control(t, p + ' extra runtime', { [p]: source + '\n' + runtime }, 1,
      { path: p, line: source.split('\n').length + 1 });
  }
  for (const [label, source] of [
    ['altered initializer', namesSource.replace('= SETTINGS_TARGET_NAME;', '= "changed";')],
    ['duplicate declaration', namesSource + '\n' + namesSource.split('\n')[0]],
  ]) control(t, label, { [namesPath]: source }, 3);
  control(t, 'altered row', {
    [artifactsPath]: artifactsSource.replace('Disposition::Ignore', 'Disposition::Track'),
  }, 3);
  control(t, 'duplicate row', { [artifactsPath]: artifactsSource.replace('];',
    artifactsSource.slice(artifactsSource.indexOf('    InstanceArtifact'),
      artifactsSource.lastIndexOf('];')) + '];') }, 3);
  control(t, 'nonexempt expression within item', {
    [namesPath]: namesSource.replace('= SETTINGS_TARGET_NAME;',
      '= { let _ = SETTINGS_TARGET_NAME; SETTINGS_TARGET_NAME };'),
  }, 3);
});
test('04 settings canonical references', t => {
  const cases = [
    ['direct qualified', runtime],
    ['grouped import', 'use crate::config::instance_artifacts::{SETTINGS_FILE_NAME};'],
    ['renamed import', 'use ' + direct + ' as N; fn f() { let _ = N; }'],
    ['module alias', 'use crate::config::instance_artifacts as a; ' +
      'fn f() { let _ = a::SETTINGS_FILE_NAME; }'],
    ['raw identifier', runtime.replace('SETTINGS_FILE_NAME', 'r#SETTINGS_FILE_NAME')],
    ['const alias', 'const N: &str = ' + direct + '; fn f() { let _ = N; }'],
    ['local alias', 'fn f() { let n = ' + direct + '; let _ = n; }'],
    ['external reexport', 'pub use ' + direct + ';'],
    ['macro argument', 'fn f() { observe!(' + direct + '); }'],
    ['reference initializer', 'const N: &&str = &' + direct + ';'],
    ['relative import', 'use super::config::instance_artifacts::SETTINGS_FILE_NAME;'],
    ['relative renamed', 'use super::config::instance_artifacts::SETTINGS_FILE_NAME as N;'],
    ['relative qualified', 'fn f() { let _ = super::config::instance_artifacts::SETTINGS_FILE_NAME; }'],
  ];
  for (const [label, source] of cases) {
    const r = control(t, label, { [root + 'other.rs']: source }, 1, site('other.rs'));
    assert.ok(r.diagnostics.some(d => d.symbol?.includes('crate::config::instance_artifacts')));
  }
  for (const source of [
    'mod config; use config::instance_artifacts::SETTINGS_FILE_NAME; fn f(){let _=SETTINGS_FILE_NAME;}',
    'mod config; fn f(){let _=config::instance_artifacts::SETTINGS_FILE_NAME;}',
  ]) control(t, 'bare crate root', { [root + 'lib.rs']: source }, 1, site('lib.rs'));
  for (const source of [
    'mod instance_artifacts; fn f(){let _=instance_artifacts::SETTINGS_FILE_NAME;}',
    'mod instance_artifacts; use instance_artifacts::SETTINGS_FILE_NAME as N; fn f(){let _=N;}',
  ]) control(t, 'bare config module', { [root + 'config.rs']: source }, 1, site('config.rs'));
  control(t, 'bare names module', { [artifactsPath]: artifactsSource +
    '\nfn f(){let _=names::SETTINGS_FILE_NAME;}' }, 1,
  { path: artifactsPath, line: artifactsSource.split('\n').length + 1 });
  control(t, 'bare declared constant identity', { [namesPath]: namesSource +
    '\nfn f(){let _=SETTINGS_TARGET_NAME;}' }, 1,
  { path: namesPath, line: namesSource.split('\n').length + 1 });
});
test('04 settings literal and homonyms', t => {
  control(t, 'exact filename', { [root + 'other.rs']:
    'const X: &str = "settings.30.instance.no-git.json";' }, 1, site('other.rs'));
  for (const [label, source] of [
    ['comment', '// ' + direct], ['constant spelling string', 'const X: &str="SETTINGS_FILE_NAME";'],
    ['unrelated homonym', 'const SETTINGS_FILE_NAME: &str="other"; fn f(){let _=SETTINGS_FILE_NAME;}'],
    ['local shadow', 'use crate::config::instance_artifacts as a; ' +
      'fn f(a: Unrelated){let _=a::SETTINGS_FILE_NAME;}'],
    ['local let shadow', 'fn f(){let SETTINGS_FILE_NAME="other";let _=SETTINGS_FILE_NAME;}'],
  ]) control(t, label, { [root + 'other.rs']: source }, 0);
});
test('04 settings cfg and ambiguity', t => {
  for (const cfg of ['test', 'all(test,windows)'])
    control(t, cfg, { [root + 'other.rs']: '#[cfg(' + cfg + ')] ' + runtime }, 0);
  control(t, 'tests.rs', { [root + 'tests.rs']: runtime }, 0);
  control(t, 'reachable test module', {
    [root + 'lib.rs']: 'mod config; #[cfg(test)] mod proof;', [root + 'proof.rs']: runtime,
  }, 0);
  control(t, 'any test windows', {
    [root + 'other.rs']: '#[cfg(any(test,windows))] ' + runtime,
  }, 1, site('other.rs'));
  const r = control(t, 'ambiguous alias', { [root + 'other.rs']:
    'use ' + direct + ' as N; use other::N; fn f(){let _=N;}' }, 1, site('other.rs'));
  assert.ok(r.diagnostics.some(d => d.ambiguous && d.reason.includes('ambiguous')));
  control(t, 'relevant glob', { [root + 'other.rs']:
    'use crate::config::instance_artifacts::*; fn f(){let _=SETTINGS_FILE_NAME;}'
  }, 1, site('other.rs'));
});
function gitFixture(t) {
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), 'settings-gate-'));
  const env = { ...process.env, GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: path.join(cwd, 'no-global-config'),
    GIT_TEMPLATE_DIR: path.join(cwd, 'empty-template') };
  fs.mkdirSync(env.GIT_TEMPLATE_DIR);
  t.after(() => {
    const resolved = fs.realpathSync(cwd), parent = fs.realpathSync(os.tmpdir());
    assert.equal(path.dirname(resolved), parent);
    fs.rmSync(resolved, { recursive: true });
  });
  const runner = (args, options) => gitRunner(args, { ...options, env });
  const git = (...args) => {
    const r = runner(args, { cwd });
    assert.equal(r.error, undefined); assert.equal(r.signal, null);
    assert.equal(r.status, 0, r.stderr?.toString()); return r.stdout.toString().trim();
  };
  git('init', '--initial-branch=main'); git('config', 'user.name', 'Settings fixture');
  git('config', 'user.email', 'settings@example.invalid');
  git('config', 'commit.gpgsign', 'false'); git('config', 'core.autocrlf', 'false');
  const write = (p, text) => {
    const target = path.join(cwd, p); fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, text);
  };
  const commit = () => {
    git('add', '--all'); git('-c', 'core.hooksPath=', 'commit', '-m', 'fixture');
    return git('rev-parse', 'HEAD');
  };
  return { cwd, write, commit, snapshot: baseRef => createSnapshot({ cwd, baseRef, runner }) };
}
function compareInventories(withSettings, withoutSettings) {
  assert.deepEqual(withSettings.inventory, withoutSettings.inventory);
  const unknown = r => r.diagnostics.find(d => d.rule === 'unclassified_writers');
  assert.deepEqual(unknown(withSettings), unknown(withoutSettings));
  assert.deepEqual(withSettings.metrics.filter(m => m.id !== 'Instance/Settings'),
    withoutSettings.metrics);
}
test('04 settings immutable git and errors', t => {
  const c = settingsPolicy();
  for (const mutate of [
    p => delete p.ownerModule, p => p.ownerModule = false,
    p => p.ownerModule = 'true', p => p.constantPaths = [],
    p => p.constantPaths.push(p.constantPaths[0]), p => p.constantPaths = ['crate::../bad'],
    p => p.metadataPolicy = 'unknown', p => p.owner = root + 'other.rs',
    p => p.scope = root + '../',
  ]) {
    const policy = structuredClone(c); mutate(policy);
    control(t, 'malformed policy', {}, 3, undefined, policy);
  }
  const unstable = settingsTree(); unstable.assertStableHead = () => { throw Error('HEAD drift'); };
  assert.equal(containersGate(unstable).exit, 3);
  const f = gitFixture(t), fixture = settingsTree();
  for (const [p, entry] of fixture.headTree) f.write(p, fixture.readBlob(entry));
  const base = f.commit();
  assert.equal(containersGate(f.snapshot(base)).exit, 0);
  f.write(root + 'other.rs', runtime);
  assert.equal(containersGate(f.snapshot(base)).exit, 0, 'working tree ignored');
  f.commit();
  const committed = containersGate(f.snapshot(base));
  assert.equal(committed.exit, 1);
  assert.ok(committed.diagnostics.some(d => d.path === root + 'other.rs' && d.line === 1));
  f.write(root + 'other.rs', ''); f.commit();
  assert.equal(containersGate(f.snapshot(base)).exit, 0);
  t.diagnostic('Git snapshot sequence PASS/0 -> worktree PASS/0 -> commit FAIL/1 -> removal PASS/0');
  const s = candidateSnapshot(), registryEntry = s.headTree.get('scripts/containers.json');
  const bytes = s.readBlob(registryEntry);
  const before = containersGate(s), settings = r => r.metrics.find(m => m.id === c.id);
  assert.equal(settings(before).externalUses, 0);
  assert.deepEqual(settings(before).writerInventory, { files: 0, sites: 0 });
  assert.ok(!before.diagnostics.some(d => d.status === 'FAIL' && d.identity === c.id));
  const lib = s.headTree.get(root + 'lib.rs'), original = s.readBlob(lib);
  const line = original.split('\n').length + 1;
  const injected = { ...s, readBlob: e => e === lib ? original + '\n' + runtime : s.readBlob(e) };
  const after = containersGate(injected);
  assert.ok(after.diagnostics.some(d => d.status === 'FAIL' && d.identity === c.id &&
    d.path === root + 'lib.rs' && d.line === line));
  assert.equal(injected.headOid, s.headOid);
  assert.equal(injected.readBlob(registryEntry), bytes);
  t.diagnostic(JSON.stringify({ control: 'candidate HEAD memory injection', oid: s.headOid,
    settingsBefore: settings(before), settingsAfter: settings(after), site: { path: root + 'lib.rs', line } }));
  const without = snapshot => ({ ...snapshot, readBlob: e => e === registryEntry ?
    JSON.stringify({ version: 1, containers: parseRegistry(bytes).filter(p => p.id !== c.id) }) :
    snapshot.readBlob(e) });
  compareInventories(before, containersGate(without(s)));
  for (const [p, expected] of [[root + 'external_writer.rs', 0], [c.owner, 1]]) {
    const tree = new Map(s.headTree), entry = { path: p };
    tree.set(p, entry);
    const writer = { ...s, headTree: tree, readBlob: e => e === entry ?
      'fn f(){ std::fs::write("other", "value"); }' : s.readBlob(e) };
    const result = containersGate(writer);
    compareInventories(result, containersGate(without(writer)));
    assert.deepEqual(settings(result).writerInventory, { files: expected, sites: expected });
    if (!expected) assert.ok(result.diagnostics.find(d => d.rule === 'unclassified_writers')
      .paths.includes(p));
  }
});
