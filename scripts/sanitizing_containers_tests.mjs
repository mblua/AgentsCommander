import test from 'node:test';
import assert from 'node:assert/strict';
import {
  parseRegistry, discoverUnregistered, runGate as containersGate,
} from './sanitizing_containers.mjs';

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
  return { baseTree: new Map(headTree), headTree,
    readBlob: entry => blobs[entry], assertStableHead() {} };
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
test('04 delegated Attempts writers remain global while scope inventory is zero', () => {
  const documents = 'src-tauri/src/filesystem/documents.rs';
  const files = {
    [attemptsScope + 'storage.rs']: [
      'const NAME: &str = "attempts.jsonl";',
      'fn create_attempts() { documents::create_experiment_artifact(NAME); }',
      'fn replace_attempts() { documents::replace_experiment_artifact(NAME); }',
    ].join('\n'),
    [documents]: [
      'fn create_experiment_artifact(p: &str) { std::fs::OpenOptions::new().open(p); }',
      'fn replace_experiment_artifact(p: &str) { std::fs::OpenOptions::new().open(p); }',
    ].join('\n'),
  };
  const result = containersGate(containerTree(files));
  assert.equal(result.exit, 0);
  assert.deepEqual(result.metrics[0].writerInventory, { files: 0, sites: 0 });
  assert.deepEqual(result.inventory, { files: 1, sites: 2 });
  const unknown = result.diagnostics.find(d => d.rule === 'unclassified_writers');
  assert.deepEqual(unknown.paths, [documents]);
  assert.equal(unknown.sites, 2);
  assert.ok(!result.diagnostics.some(d => d.rule === 'writer_inventory_changed'));
  files[attemptsScope + 'resume.rs'] = 'const NAME: &str = "attempts.jsonl";';
  const external = containersGate(containerTree(files));
  assert.equal(external.exit, 1);
  assert.ok(external.diagnostics.some(d => d.rule === 'external_container_literal' &&
    d.identity === 'RoleExperiment/Attempts' && d.path === attemptsScope + 'resume.rs'));
});

test('04 rejects unsafe basenames and nonstrings', () => {
  for (const name of [
    '', '.', '..', '../Role.md', './Role.md', 'a/b', String.raw`a\b`, 'C:Role.md',
    'a b', ' a', 'a ', '\n', 'a\n', 'a:b', 'a*', 'é.md', null, 1, true, {}, [],
  ]) {
    const registry = attemptsRegistry();
    registry.containers[0].name = name;
    assert.throws(() => parseRegistry(JSON.stringify(registry)), JSON.stringify(name));
    assert.equal(containersGate(containerTree({}, registry)).status, 'ERROR');
  }
});

// Policy comes from the immutable candidate registry, never a fixture copy.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createSnapshot, gitRunner, parseArgs, codeOrder } from './sanitizing_baseline.mjs';
const repo = fileURLToPath(new URL('../', import.meta.url));
const candidateSnapshot = () => {
  const oid = gitRunner(['rev-parse', 'HEAD'], { cwd: repo });
  assert.equal(oid.status, 0);
  const head = oid.stdout.toString().trim();
  const snapshot = createSnapshot({ cwd: repo, baseRef: head });
  assert.equal(snapshot.headOid, head,
    'HEAD moved while taking the candidate snapshot; rerun on a stable checkout');
  return snapshot;
};
// Live-clone gate calls return ERROR if HEAD moves mid-test (#4214 diagnosis:
// _sanitizing/flaky-test04-ac4214-001). Fail with the gate reasons, not later `undefined`.
function stable(result, step) {
  if (result.status !== 'ERROR') return result;
  assert.fail(`test 04 step "${step}": gate ERROR exit ${result.exit}: ` +
    result.diagnostics.filter(d => d.reason).map(d => d.reason).join(' | '));
}
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
  let expectedStatus = 'ERROR';
  if (expected === 0) expectedStatus = 'PASS';
  else if (expected === 1) expectedStatus = 'FAIL';
  assert.equal(result.status, expectedStatus);
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
  aliasClassControls(t);
  const cases = [
    ['direct qualified', runtime],
    ['grouped import', 'use crate::config::instance_artifacts::{SETTINGS_FILE_NAME};'],
    ['renamed import', 'use ' + direct + ' as N; fn f() { let _ = N; }'],
    ['module alias', 'use crate::config::instance_artifacts as a; ' +
      'fn f() { let _ = a::SETTINGS_FILE_NAME; }'],
    ['self qualified module alias', 'use crate::config::instance_artifacts as a; ' +
      'fn f() { let _ = self::a::SETTINGS_FILE_NAME; }'],
    ['inline self module alias', 'mod inner { use crate::config::instance_artifacts as a; ' +
      'fn f(){let _=self::a::SETTINGS_FILE_NAME;} }'],
    ['alias of alias', 'use crate::config::instance_artifacts as a; use self::a as b; ' +
      'fn f(){let _=self::b::SETTINGS_FILE_NAME;}'],
    ['self renamed import', 'use ' + direct + ' as N; fn f(){let _=self::N;}'],
    ['inline parent module alias', 'use crate::config::instance_artifacts as a; ' +
      'mod inner { fn f(){let _=super::a::SETTINGS_FILE_NAME;} }'],
    ['raw identifier', runtime.replace('SETTINGS_FILE_NAME', 'r#SETTINGS_FILE_NAME')],
    ['const alias', 'const N: &str = ' + direct + '; fn f() { let _ = N; }'],
    ['local alias', 'fn f() { let n = ' + direct + '; let _ = n; }'],
    ['external reexport', 'pub use ' + direct + ';'],
    ['macro argument', 'fn f() { observe!(' + direct + '); }'],
    ['reference initializer', 'const N: &&str = &' + direct + ';'],
    ['relative import', 'use super::config::instance_artifacts::SETTINGS_FILE_NAME;'],
    ['relative renamed', 'use super::config::instance_artifacts::SETTINGS_FILE_NAME as N;'],
    ['relative qualified', 'fn f() { let _ = ' +
      'super::config::instance_artifacts::SETTINGS_FILE_NAME; }'],
  ];
  for (const [label, source] of cases) {
    const r = control(t, label, { [root + 'other.rs']: source }, 1, site('other.rs'));
    assert.ok(r.diagnostics.some(d => d.symbol === direct));
  }
  for (const source of [
    'mod config; use config::instance_artifacts::SETTINGS_FILE_NAME; ' +
      'fn f(){let _=SETTINGS_FILE_NAME;}',
    'mod config; fn f(){let _=config::instance_artifacts::SETTINGS_FILE_NAME;}',
  ]) control(t, 'bare crate root', { [root + 'lib.rs']: source }, 1, site('lib.rs'));
  for (const source of [
    'mod instance_artifacts; fn f(){let _=instance_artifacts::SETTINGS_FILE_NAME;}',
    'mod instance_artifacts; fn f(){let _=self::instance_artifacts::SETTINGS_FILE_NAME;}',
    'mod instance_artifacts; use self::instance_artifacts::SETTINGS_FILE_NAME as N; ' +
      'fn f(){let _=N;}',
    'mod instance_artifacts; use instance_artifacts::SETTINGS_FILE_NAME as N; fn f(){let _=N;}',
  ]) control(t, 'bare config module', { [root + 'config.rs']: source }, 1, site('config.rs'));
  control(t, 'bare names module', { [artifactsPath]: artifactsSource +
    '\nfn f(){let _=names::SETTINGS_FILE_NAME;}' }, 1,
  { path: artifactsPath, line: artifactsSource.split('\n').length + 1 });
  control(t, 'bare declared constant identity', { [namesPath]: namesSource +
    '\nfn f(){let _=SETTINGS_TARGET_NAME;}' }, 1,
  { path: namesPath, line: namesSource.split('\n').length + 1 });
});
function aliasClassControls(t) {
  const expand = s => s.replace(/\bIA\b/g, 'crate::config::instance_artifacts')
    .replace(/\bX\b/g, 'SETTINGS_FILE_NAME').replaceAll(' fn f', '\nfn f');
  const cases = [
    ['R0', 'use IA as a; fn f(){ let _ = self::a::X; }'],
    ['R1', 'mod m { use IA as a; fn f(){ let _ = self::a::X; } }'],
    ['R2', 'use IA as a; mod m { fn f(){ let _ = super::a::X; } }'],
    ['R3', 'use IA as a; mod m { mod n { fn f(){ let _ = super::super::a::X; } } }'],
    ['R5', 'use IA as a; use a as b; fn f(){ let _ = b::X; }'],
    ['R6', 'use IA as a; use self::a as b; fn f(){ let _ = b::X; }'],
    ['R7', 'use IA as a; fn f(){ let _ = crate::other::a::X; }'],
    ['R8', 'use IA as a; use self::a::X as N; fn f(){ let _ = N; }'],
    ['R9', 'use IA as a; use self::a::X; fn f(){ let _ = X; }'],
    ['R10', 'use IA as a; mod m { use super::a::X; fn f(){ let _ = X; } }'],
    ['R11', 'use crate::config as cfg; fn f(){ let _ = self::cfg::instance_artifacts::X; }'],
    ['R12', 'use IA as a; use self::a::*; fn f(){ let _ = X; }'],
    ['R15', 'use IA as a; use a as b; use b as c; fn f(){ let _ = c::X; }'],
    ['R16', 'use IA::names as n; fn f(){ let _ = self::n::X; }'],
    ['R17', 'fn f(){ use IA as a; let _ = a::X; }'],
    ['R18', 'pub(crate) mod m { pub(crate) use IA as a; } ' +
      'fn f(){ let _ = crate::other::m::a::X; }'],
    ['R19', 'mod m { pub(crate) use IA as a; } fn f(){ let _ = self::m::a::X; }'],
  ].map(([id, source]) => [id, { [root + 'other.rs']: expand(source) },
    root + 'other.rs', id === 'R16' ? direct.replace('::SETTINGS', '::names::SETTINGS') : direct]);
  const child = 'fn f(){ let _ = super::a::X; }';
  cases.push(
    ['R4', {
    [root + 'other.rs']: expand('pub(crate) use IA as a; mod c;'),
    [root + 'other/c.rs']: expand(child),
  }, root + 'other/c.rs', direct],
    ['R13', {
    [root + 'config.rs']: 'mod instance_artifacts; mod settings; mod x; ' +
      'use instance_artifacts as ia;',
    [root + 'config/x.rs']: expand('fn f(){ let _ = super::ia::X; }'),
  }, root + 'config/x.rs', direct],
    ['R14', {
    [root + 'config.rs']: expand('mod instance_artifacts; mod settings; ' +
      'use self::instance_artifacts as ia; fn f(){ let _ = self::ia::X; }'),
  }, root + 'config.rs', direct],
    ['b-1', {
    [artifactsPath]: artifactsSource + '\npub(crate) use names as n;',
    [root + 'other.rs']: expand('fn f(){ let _ = IA::n::X; }'),
  }, root + 'other.rs', direct.replace('::SETTINGS', '::names::SETTINGS')],
    ['b-2', {
    [root + 'config.rs']: 'mod instance_artifacts; mod settings; mod y;',
    [root + 'config/y.rs']: expand('pub(crate) use IA as ia2;'),
    [root + 'other.rs']: expand('fn f(){ let _ = crate::config::y::ia2::X; }'),
  }, root + 'other.rs', direct]
  );
  for (const [id, files, path, symbol] of cases) {
    const source = files[path], line = source.slice(0, source.indexOf('let _')).split('\n').length;
    const result = control(t, 'AC-3843-003 ' + id,
      { [root + 'lib.rs']: 'mod config; mod other;', ...files }, 1, { path, line });
    assert.ok(result.diagnostics.some(d => d.status === 'FAIL' && d.path === path &&
      d.line === line && d.symbol === symbol), id + ': canonical use site');
  }
  control(t, 'module alias declaration alone', {
    [root + 'other.rs']: expand('pub(crate) use IA as a;'),
  }, 0);
}

test('04 settings literal and homonyms', t => {
  control(t, 'exact filename', { [root + 'other.rs']:
    'const X: &str = "settings.30.instance.no-git.json";' }, 1, site('other.rs'));
  for (const [label, source] of [
    ['comment', '// ' + direct],
    ['constant spelling string', 'const X: &str="SETTINGS_FILE_NAME";'],
    ['unrelated homonym', 'const SETTINGS_FILE_NAME: &str="other"; ' +
      'fn f(){let _=SETTINGS_FILE_NAME;}'],
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
  const unstable = settingsTree();
  unstable.assertStableHead = () => {
    throw new Error('HEAD drift');
  };
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
  t.diagnostic('Git snapshot sequence PASS/0 -> worktree PASS/0 -> ' +
    'commit FAIL/1 -> removal PASS/0');
  const s = candidateSnapshot(), registryEntry = s.headTree.get('scripts/containers.json');
  const bytes = s.readBlob(registryEntry);
  const before = stable(containersGate(s), 'before');
  const settings = r => r.metrics.find(m => m.id === c.id);
  assert.equal(settings(before).externalUses, 0);
  assert.deepEqual(settings(before).writerInventory, { files: 0, sites: 0 });
  assert.ok(!before.diagnostics.some(d => d.status === 'FAIL' && d.identity === c.id));
  const lib = s.headTree.get(root + 'lib.rs'), original = s.readBlob(lib);
  const line = original.split('\n').length + 1;
  const injected = { ...s, readBlob: e => e === lib ? original + '\n' + runtime : s.readBlob(e) };
  const after = stable(containersGate(injected), 'memory injection');
  assert.ok(after.diagnostics.some(d => d.status === 'FAIL' && d.identity === c.id &&
    d.path === root + 'lib.rs' && d.line === line));
  assert.equal(injected.headOid, s.headOid);
  assert.equal(injected.readBlob(registryEntry), bytes);
  t.diagnostic(JSON.stringify({ control: 'candidate HEAD memory injection', oid: s.headOid,
    settingsBefore: settings(before), settingsAfter: settings(after),
    site: { path: root + 'lib.rs', line } }));
  const without = snapshot => ({ ...snapshot, readBlob: e => e === registryEntry ?
    JSON.stringify({ version: 1, containers: parseRegistry(bytes).filter(p => p.id !== c.id) }) :
    snapshot.readBlob(e) });
  compareInventories(before, stable(containersGate(without(s)), 'without registry'));
  for (const [p, expected] of [[root + 'external_writer.rs', 0], [c.owner, 1]]) {
    const tree = new Map(s.headTree), entry = { path: p };
    tree.set(p, entry);
    const writer = { ...s, headTree: tree, readBlob: e => e === entry ?
      'fn f(){ std::fs::write("other", "value"); }' : s.readBlob(e) };
    const result = stable(containersGate(writer), 'writer ' + p);
    compareInventories(result,
      stable(containersGate(without(writer)), 'writer ' + p + ' without registry'));
    assert.deepEqual(settings(result).writerInventory, { files: expected, sites: expected });
    t.diagnostic(JSON.stringify({ control: expected ? 'owner writer' : 'non-owner writer',
      status: result.status, exit: result.exit, inventory: result.inventory,
      settings: settings(result),
      unclassified: result.diagnostics.find(d => d.rule === 'unclassified_writers') }));
    if (!expected) assert.ok(result.diagnostics.find(d => d.rule === 'unclassified_writers')
      .paths.includes(p));
  }
});

// #3919 stage 1: literalScope widens only the literal search; instance-artifacts-v1
// exempts exact instance metadata shapes for the registered name.
const sessionsScope = root + 'config/sessions_persistence/';
const snapshotRegistry = (extra = {}) => ({ version: 1, containers: [{
  id: 'Sessions/Snapshot', name: 'sessions.json', scope: sessionsScope,
  owner: sessionsScope + 'store.rs', externalLiteralBaseline: 0,
  writerInventory: { files: 0, sites: 0 }, ...extra }] });
const widened = () => snapshotRegistry({ literalScope: root,
  metadataPolicy: 'instance-artifacts-v1' });
const snapshotNames = 'pub(crate) const SESSIONS_FILE_NAME: &str = "sessions.json";';
const snapshotArtifacts = [
  'const INSTANCE_ARTIFACTS: &[InstanceArtifact] = &[',
  '    InstanceArtifact {',
  '        name: "sessions.json",',
  '        kind: ArtifactKind::File,',
  '        disposition: Disposition::Ignore,',
  '        comment: "# AgentsCommander: persisted session state",',
  '    },',
  '];',
].join('\n');
function snapshotTree(files = {}, registry = widened()) {
  const blobs = { 'scripts/containers.json': JSON.stringify(registry),
    [sessionsScope + 'store.rs']: 'fn load() {}', [namesPath]: snapshotNames,
    [artifactsPath]: snapshotArtifacts, ...files };
  const headTree = new Map(Object.keys(blobs).map(p => [p, p]));
  return { baseTree: new Map(headTree), headTree, readBlob: e => blobs[e], assertStableHead() {} };
}
const literalSites = r => r.diagnostics.filter(d => d.rule === 'external_container_literal')
  .map(d => d.path + ':' + d.line);
test('04 literalScope finds literals outside scope; unset scope is unchanged', () => {
  const stray = { [root + 'probe.rs']: 'fn f() -> &\'static str { "sessions.json" }' };
  assert.equal(containersGate(snapshotTree()).exit, 0);
  assert.deepEqual(literalSites(containersGate(snapshotTree(stray))), [root + 'probe.rs:1']);
  const narrow = snapshotRegistry();
  assert.equal(containersGate(snapshotTree(stray, narrow)).exit, 0);
  const inside = { [sessionsScope + 'other.rs']: 'const N: &str = "sessions.json";' };
  assert.deepEqual(literalSites(containersGate(snapshotTree(inside, narrow))),
    [sessionsScope + 'other.rs:1']);
  const metrics = containersGate(snapshotTree(stray)).metrics[0];
  assert.deepEqual(metrics.writerInventory, { files: 0, sites: 0 });
  assert.equal(metrics.externalLiterals, 1);
});
test('04 instance-artifacts-v1 exempts only exact metadata shapes', () => {
  const bad = [
    [namesPath, snapshotNames.replace('"sessions.json"', 'concat!("sessions.json")'), 1],
    [namesPath, snapshotNames + '\nfn f() -> &\'static str { "sessions.json" }', 2],
    [namesPath, 'mod inner { const N: &str = "sessions.json"; }', 1],
    [artifactsPath, snapshotArtifacts.replace('kind:', 'extra: 1,\n        kind:'), 3],
    [artifactsPath, snapshotArtifacts.replace('comment: "#', 'comment: "sessions.json #'), 0],
    [artifactsPath, snapshotArtifacts + '\nconst OTHER: &str = "sessions.json";', 9],
  ];
  for (const [p, text, line] of bad) {
    const r = containersGate(snapshotTree({ [p]: text }));
    assert.equal(r.exit, line ? 1 : 0, text);
    if (line) assert.deepEqual(literalSites(r), [p + ':' + line], text);
  }
  const test = '#[cfg(test)]\nmod tests { const N: &str = "sessions.json"; }';
  assert.equal(containersGate(snapshotTree({ [namesPath]: snapshotNames + '\n' + test })).exit, 0);
});
test('04 instance-artifacts-v1 registry contract', () => {
  for (const change of [
    c => delete c.literalScope, c => c.ownerModule = true, c => c.constantPaths = [direct],
    c => c.literalScope = root + 'cli/', c => c.literalScope = 'src-tauri/src',
    c => c.literalScope = '../', c => c.literalScope = 1,
  ]) {
    const registry = widened();
    change(registry.containers[0]);
    assert.throws(() => parseRegistry(JSON.stringify(registry)));
    assert.equal(containersGate(snapshotTree({}, registry)).status, 'ERROR');
  }
  const missing = snapshotTree();
  missing.headTree.delete(namesPath);
  assert.equal(containersGate(missing).status, 'ERROR');
});
test('04 stage-1 containers scan all production sources on candidate HEAD', t => {
  const s = candidateSnapshot();
  const entries = parseRegistry(s.readBlob(s.headTree.get('scripts/containers.json')));
  const ids = new Set(['RoleExperiment/Attempts', 'Sessions/Snapshot', 'Sessions/OrphanArchive',
    'TeamCreation/ConfigMutationLock', 'TeamCreation/Conventions', 'Screenshots/Native']);
  for (const id of ids) {
    const c = entries.find(e => e.id === id);
    assert.equal(c.literalScope, root, id);
    assert.equal(c.metadataPolicy, 'instance-artifacts-v1', id);
  }
  const result = containersGate(s);
  assert.equal(result.exit, 0);
  for (const id of ids)
    assert.equal(result.metrics.find(m => m.id === id).externalLiterals, 0, id);
  t.diagnostic(JSON.stringify(result.metrics.filter(m => ids.has(m.id))));
});

// Independent immutable sides: mutating HEAD cannot mutate the base fixture's blobs.
function ratchetTree(base = {}, head = base, baseRegistry = attemptsRegistry(),
  headRegistry = baseRegistry) {
  const a = containerTree(structuredClone(base), structuredClone(baseRegistry));
  const b = containerTree(structuredClone(head), structuredClone(headRegistry));
  const blobs = new Map();
  const side = s => new Map([...s.headTree].map(([p, e]) => {
    const entry = { path: p }; blobs.set(entry, s.readBlob(e)); return [p, entry];
  }));
  return { baseTree: side(a), headTree: side(b), readBlob: e => blobs.get(e),
    baseOid: 'base-fixture', headOid: 'head-fixture', assertStableHead() {} };
}
const sourceLiteral = literal => 'const N: &str = ' + JSON.stringify(literal) + ';';
const distributed = (literal, paths = ['a.rs', 'b.rs']) =>
  Object.fromEntries(paths.map(p => [root + p, sourceLiteral(literal)]));
const debtEntry = (literal, paths = ['a.rs', 'b.rs'], line = 1) => ({ literal,
  files: paths.map(p => root + p).sort(codeOrder),
  sites: paths.map(p => ({ path: root + p, line })).sort((a, b) => a.path.localeCompare(b.path)),
});
const countsOf = entries => ({ identities: entries.length,
  pairs: entries.reduce((n, e) => n + e.files.length, 0),
  sites: entries.reduce((n, e) => n + e.sites.length, 0) });
function assertDebt(result, side, entries) {
  assert.deepEqual(result.unregistered?.[side], { entries, counts: countsOf(entries) });
}
function rejection(t, label, base, safe, bad, expected, registries = []) {
  const run = head => containersGate(ratchetTree(base, head, ...registries));
  const before = run(safe), after = run(bad), restored = run(safe);
  assert.deepEqual([before.exit, after.exit, restored.exit], [0, 1, 0],
    JSON.stringify({ label, before, after, restored }));
  const matched = after.diagnostics.filter(d => d.rule?.startsWith('unregistered_container_'));
  assert.deepEqual(matched.map(({ rule, literal, path, line }) =>
    ({ rule, literal, path, line })), expected);
  t.diagnostic(JSON.stringify({ control: label, exits: [0, 1, 0], matched,
    expectedSites: expected }));
  return after;
}
const expectedSites = (rule, literal, paths) => paths.map(path =>
  ({ rule, literal, path: root + path, line: 1 }));
const growth = 'unregistered_container_growth', newFile = 'unregistered_container_file';

test('04 unregistered explicit debt sets and shrinking controls', t => {
  const cases = [
    ['empty', {}, []],
    ['singleton', distributed('run.json', ['a.rs']), []],
    ['same file repetitions', { [root + 'a.rs']: new Array(3).fill(sourceLiteral('run.json'))
      .join('\n') }, []],
    ['unchanged', distributed('run.json'), [debtEntry('run.json')]],
  ];
  for (const [label, files, entries] of cases) {
    const result = containersGate(ratchetTree(files));
    assert.equal(result.exit, 0, JSON.stringify(result));
    assertDebt(result, 'base', entries); assertDebt(result, 'head', entries);
    assert.ok(!result.diagnostics.some(d => d.status === 'FAIL'));
    t.diagnostic(JSON.stringify({ control: label, exits: [0], entries,
      counts: countsOf(entries) }));
  }
  const three = distributed('run.json', ['a.rs', 'b.rs', 'c.rs']);
  for (const head of [distributed('run.json'), distributed('run.json', ['a.rs']), {}]) {
    const result = containersGate(ratchetTree(three, head));
    assert.equal(result.exit, 0, JSON.stringify(result));
    assertDebt(result, 'base', [debtEntry('run.json', ['a.rs', 'b.rs', 'c.rs'])]);
    assertDebt(result, 'head', Object.keys(head).length === 2 ? [debtEntry('run.json')] : []);
    t.diagnostic(JSON.stringify({ control: 'shrink/removal', exits: [0],
      head: result.unregistered.head }));
  }
  const repeated = distributed('run.json');
  repeated[root + 'a.rs'] += '\n' + sourceLiteral('run.json');
  const r = containersGate(ratchetTree(distributed('run.json'), repeated));
  const entry = debtEntry('run.json'); entry.sites.splice(1, 0, { path: root + 'a.rs', line: 2 });
  assert.equal(r.exit, 0); assertDebt(r, 'head', [entry]);
});

test('04 unregistered rejects per-literal growth and identity replacements with receipts', t => {
  const old = distributed('run.json'), fresh = distributed('ledger.json');
  rejection(t, 'new identity', {}, {}, fresh, expectedSites(growth, 'ledger.json',
    ['a.rs', 'b.rs']));
  rejection(t, 'singleton becomes distributed', distributed('run.json', ['a.rs']),
    distributed('run.json', ['a.rs']), old, expectedSites(growth, 'run.json', ['a.rs', 'b.rs']));
  rejection(t, 'extra file/copy', old, old, distributed('run.json', ['a.rs', 'b.rs', 'c.rs']),
    expectedSites(newFile, 'run.json', ['c.rs']));
  rejection(t, 'equal identity totals and compensating deletion', old, old, fresh,
    expectedSites(growth, 'ledger.json', ['a.rs', 'b.rs']));
  const moved = distributed('run.json', ['a.rs', 'c.rs']);
  const movedGrowth = rejection(t, 'move plus growth', old, moved,
    distributed('run.json', ['b.rs', 'c.rs', 'd.rs']),
    expectedSites(newFile, 'run.json', ['c.rs', 'd.rs']));
  assertDebt(movedGrowth, 'base', [debtEntry('run.json')]);
  assertDebt(movedGrowth, 'head', [debtEntry('run.json', ['b.rs', 'c.rs', 'd.rs'])]);
  rejection(t, 'repaired debt resurrection', {}, {}, old,
    expectedSites(growth, 'run.json', ['a.rs', 'b.rs']));
  const recovery = containersGate(ratchetTree(fresh));
  assert.equal(recovery.exit, 0); assertDebt(recovery, 'head', [debtEntry('ledger.json')]);
  t.diagnostic(JSON.stringify({ control: 'base advancement accepts merged debt', exits: [0],
    base: recovery.unregistered.base, head: recovery.unregistered.head }));
});

test('04 unregistered accepts relocation and shrinking per literal', t => {
  for (const [label, basePaths, headPaths] of [
    ['partial replacement', ['a.rs', 'b.rs'], ['a.rs', 'c.rs']],
    ['full replacement', ['a.rs', 'b.rs'], ['c.rs', 'd.rs']],
    ['shrinking relocation', ['a.rs', 'b.rs', 'c.rs'], ['a.rs', 'd.rs']],
  ]) {
    const result = containersGate(ratchetTree(distributed('run.json', basePaths),
      distributed('run.json', headPaths)));
    assert.equal(result.exit, 0, JSON.stringify(result));
    assert.equal(result.status, 'PASS');
    assert.ok(!result.diagnostics.some(d => d.status === 'FAIL'));
    assertDebt(result, 'base', [debtEntry('run.json', basePaths)]);
    assertDebt(result, 'head', [debtEntry('run.json', headPaths)]);
    t.diagnostic(JSON.stringify({ control: label, exits: [0],
      base: result.unregistered.base, head: result.unregistered.head }));
  }
});

test('04 literal-files-v1 exact grammar and lexical limits', t => {
  const accepted = new Set(['run.json', 'a.jsonl', 'a.toml', 'a.yaml', 'a.yml', 'a.db', 'a.sqlite',
    'a.sqlite3', '.team.lock', 'Role.md', 'a.txt', '/fixture/a.json', 'C:/fixture/a.json',
    'x' + String.fromCodePoint(92) + 'a.json', '{}.json', '{name}_*.json',
    'a/{_id9}/report.json', 'RUN.json']);
  const rejected = ['', '.json', './run.json', '../run.json', 'a//run.json',
    'a/./run.json', 'a/../run.json', 'run.JSON', 'a.csv', ' run.json', 'a b.json',
    'a\n.json', 'é.json', '{9id}.json', '{bad-name}.json', '{x.json', 'a.json/', 'C:a.json'];
  for (const value of [...accepted, ...rejected]) {
    const fixture = ratchetTree(distributed(value));
    const entries = accepted.has(value) ?
      [debtEntry(value.replaceAll(String.fromCodePoint(92), '/'))] : [];
    const result = containersGate(fixture);
    assert.equal(result.exit, 0, JSON.stringify(result));
    assertDebt(result, 'head', entries);
    assert.deepEqual(discoverUnregistered(fixture.headTree, fixture.readBlob,
      attemptsRegistry().containers), entries);
    t.diagnostic(JSON.stringify({ control: 'grammar ' + JSON.stringify(value), exits: [0],
      entries, counts: countsOf(entries) }));
  }
  const forms = [
    ['raw', 'const N: &str = r#"run.json"#;', true],
    ['escaped', String.raw`const N: &str = "run\u{2e}json";`, true],
    ['macro complete', 'observe!("run.json");', true],
    ['static', 'static N: &str = "run.json";', true],
    ['comment', '// "run.json"', false],
    ['test item', '#[cfg(test)] fn f(){let _="run.json";}', false],
    ['byte', 'const N: &[u8] = b"run.json";', false],
    ['assembled', 'concat!("run", ".json");', false],
    ['identifier consumer', 'fn f(){let _=NAME;}', false],
  ];
  for (const [label, source, counts] of forms) {
    const files = { [root + 'a.rs']: source, [root + 'b.rs']: source };
    const result = containersGate(ratchetTree(files));
    assert.equal(result.exit, 0, JSON.stringify(result));
    assertDebt(result, 'head', counts ? [debtEntry('run.json')] : []);
    t.diagnostic(JSON.stringify({ control: label, exits: [0], entries: result.unregistered.head }));
  }
});

test('04 discovery scoped exclusions, templates, metadata and test layouts', t => {
  const registry = attemptsRegistry(), c = registry.containers[0];
  const files = { ...distributed(c.name),
    [c.owner]: sourceLiteral(c.name),
    [root + 'meta.rs']: 'const INSTANCE_ARTIFACTS: &[A] = &[A{name:"artifact.json"}];',
    [root + 'names.rs']: sourceLiteral('artifact.json'),
    [root + 'template.rs']: sourceLiteral('{}.json'),
    [root + 'template2.rs']: sourceLiteral('{}.json'),
    [root + 'full.rs']: sourceLiteral('/fixture/attempts.jsonl'),
    [root + 'full2.rs']: sourceLiteral('/fixture/attempts.jsonl'),
    [root + 'lib.rs']: '#[cfg(test)] mod proof;',
    [root + 'proof.rs']: sourceLiteral('test-only.json'),
    [root + 'proof2.rs']: '#[cfg(test)] fn f(){let _="test-only.json";}',
  };
  const expected = [debtEntry('/fixture/attempts.jsonl', ['full.rs', 'full2.rs']),
    debtEntry('artifact.json', ['meta.rs', 'names.rs']), debtEntry(c.name),
    debtEntry('{}.json', ['template.rs', 'template2.rs'])];
  const result = containersGate(ratchetTree(files));
  assert.equal(result.exit, 0, JSON.stringify(result)); assertDebt(result, 'head', expected);
  // Widening the registry removes only exact decoded values, never full paths.
  const wide = structuredClone(registry); wide.containers[0].literalScope = root;
  const fixture = ratchetTree(files, files, wide, wide);
  assert.deepEqual(discoverUnregistered(fixture.headTree, fixture.readBlob, wide.containers),
    expected.filter(e => e.literal !== c.name));
  const unsupported = { ...distributed('test-only.json', ['proof.rs', 'other.rs']),
    [root + 'lib.rs']: '#[cfg(test)] mod proof; #[path="other.rs"] mod custom;' };
  const conservative = containersGate(ratchetTree(unsupported));
  assert.equal(conservative.exit, 0);
  assertDebt(conservative, 'head', [debtEntry('test-only.json', ['other.rs', 'proof.rs'])]);
  rejection(t, 'artifact metadata is counted', {}, {}, distributed('artifact.json'),
    expectedSites(growth, 'artifact.json', ['a.rs', 'b.rs']));
  rejection(t, 'template homonyms are counted', {}, {}, distributed('{}.json'),
    expectedSites(growth, '{}.json', ['a.rs', 'b.rs']));
  t.diagnostic(JSON.stringify({ control: 'scoped homonyms and unsupported layout', exits: [0],
    entries: expected, conservative: conservative.unregistered.head }));
});

test('04 independent registry snapshots, errors and ordering', t => {
  const files = distributed('run.json');
  const narrow = attemptsRegistry(), wide = structuredClone(narrow);
  wide.containers[0].name = 'run.json'; wide.containers[0].literalScope = root;
  const result = containersGate(ratchetTree(files, files, narrow, wide));
  assert.equal(result.exit, 1); // existing registered external-name check remains independent
  assertDebt(result, 'base', [debtEntry('run.json')]); assertDebt(result, 'head', []);
  const normal = () => ratchetTree();
  const errors = [
    ['missing baseTree', s => delete s.baseTree, 'tree missing'],
    ['missing headTree', s => delete s.headTree, 'tree missing'],
    ['missing base registry', s => s.baseTree.delete('scripts/containers.json'), 'base registry'],
    ['missing head registry', s => s.headTree.delete('scripts/containers.json'),
      'registry missing'],
    ['base invalid JSON', s => {
      const old = s.readBlob, e = s.baseTree.get('scripts/containers.json');
      s.readBlob = x => x === e ? '{' : old(x);
    }, 'invalid JSON'],
    ['head invalid version', s => {
      const old = s.readBlob, e = s.headTree.get('scripts/containers.json');
      s.readBlob = x => x === e ? '{"version":2,"containers":[]}' : old(x);
    }, 'version 1'],
    ['missing source blob', s => {
      s.baseTree.set(root + 'missing.rs', {});
    }, 'blob'],
    ['unsupported mode', s => { s.readBlob = () => { throw new Error('unsupported mode'); }; },
      'unsupported mode'],
    ['HEAD drift', s => { s.assertStableHead = () => { throw new Error('HEAD drift'); }; },
      'HEAD drift'],
  ];
  for (const [label, mutate, cause] of errors) {
    const s = normal(); mutate(s); const r = containersGate(s);
    assert.equal(r.exit, 3, label + JSON.stringify(r)); assert.equal(r.status, 'ERROR');
    assert.ok(r.diagnostics.some(d => d.reason.includes(cause)), JSON.stringify(r));
    assert.equal(r.unregistered, undefined);
    t.diagnostic(JSON.stringify({ control: label, exits: [3], expectedSite: null,
      matched: r.diagnostics }));
  }
  const unordered = { ...distributed('z.json', ['z.rs', 'a.rs']),
    ...distributed('a.json', ['c.rs', 'b.rs']) };
  const r = containersGate(ratchetTree(unordered));
  assertDebt(r, 'head', [debtEntry('a.json', ['b.rs', 'c.rs']),
    debtEntry('z.json', ['a.rs', 'z.rs'])]);
});

test('04 committed relocation uses immutable HEAD and base trees', t => {
  const f = gitFixture(t), initial = containerTree(distributed('run.json'));
  for (const [p, entry] of initial.headTree) f.write(p, initial.readBlob(entry));
  const base = f.commit();
  f.write(root + 'b.rs', '');
  f.write(root + 'c.rs', sourceLiteral('run.json'));
  const dirty = containersGate(f.snapshot(base));
  assert.equal(dirty.exit, 0); assert.equal(dirty.status, 'PASS');
  assertDebt(dirty, 'head', [debtEntry('run.json')]);
  f.commit();
  const moved = containersGate(f.snapshot(base));
  assert.equal(moved.exit, 0); assert.equal(moved.status, 'PASS');
  assertDebt(moved, 'base', [debtEntry('run.json')]);
  assertDebt(moved, 'head', [debtEntry('run.json', ['a.rs', 'c.rs'])]);
  f.write(root + 'd.rs', sourceLiteral('run.json'));
  const dirtyGrowth = containersGate(f.snapshot(base));
  assert.equal(dirtyGrowth.exit, 0);
  assertDebt(dirtyGrowth, 'head', [debtEntry('run.json', ['a.rs', 'c.rs'])]);
  f.commit();
  const grown = containersGate(f.snapshot(base));
  assert.equal(grown.exit, 1); assert.equal(grown.status, 'FAIL');
  assertDebt(grown, 'base', [debtEntry('run.json')]);
  assertDebt(grown, 'head', [debtEntry('run.json', ['a.rs', 'c.rs', 'd.rs'])]);
  const matched = grown.diagnostics.filter(d => d.rule?.startsWith('unregistered_container_'));
  assert.deepEqual(matched.map(({ rule, literal, path, line }) =>
    ({ rule, literal, path, line })), expectedSites(newFile, 'run.json', ['c.rs', 'd.rs']));
  f.write(root + 'd.rs', ''); f.commit();
  const restored = containersGate(f.snapshot(base));
  assert.equal(restored.exit, 0); assert.equal(restored.status, 'PASS');
  assertDebt(restored, 'head', [debtEntry('run.json', ['a.rs', 'c.rs'])]);
  t.diagnostic(JSON.stringify({ control: 'committed relocation then growth',
    exits: [moved.exit, grown.exit, restored.exit], matched,
    base: grown.unregistered.base, head: grown.unregistered.head }));
});

test('04 committed source and registry inputs stay distinct from dirty files', t => {
  const f = gitFixture(t), initial = containerTree(distributed('run.json'));
  for (const [p, entry] of initial.headTree) f.write(p, initial.readBlob(entry));
  const base = f.commit(), baseline = containersGate(f.snapshot(base));
  assert.equal(baseline.exit, 0); assertDebt(baseline, 'base', [debtEntry('run.json')]);
  f.write(root + 'c.rs', sourceLiteral('run.json'));
  assert.equal(containersGate(f.snapshot(base)).exit, 0, 'dirty source ignored');
  f.commit(); const after = containersGate(f.snapshot(base));
  assert.equal(after.exit, 1);
  const matched = after.diagnostics.filter(d => d.rule === newFile);
  assert.deepEqual(matched.map(({ rule, literal, path, line }) => ({ rule, literal, path, line })),
    expectedSites(newFile, 'run.json', ['c.rs']));
  f.write(root + 'c.rs', ''); f.commit();
  assert.equal(containersGate(f.snapshot(base)).exit, 0);
  const wide = attemptsRegistry(); wide.containers[0].name = 'run.json';
  wide.containers[0].literalScope = root;
  f.write('scripts/containers.json', JSON.stringify(wide));
  const dirty = containersGate(f.snapshot(base));
  assertDebt(dirty, 'head', [debtEntry('run.json')]);
  f.commit(); const registered = containersGate(f.snapshot(base));
  assertDebt(registered, 'base', [debtEntry('run.json')]); assertDebt(registered, 'head', []);
  t.diagnostic(JSON.stringify({ control: 'committed/dirty source and registry', exits: [0, 1, 0],
    expectedSites: expectedSites(newFile, 'run.json', ['c.rs']), matched,
    registryBase: registered.unregistered.base, registryHead: registered.unregistered.head }));
});

function directSourceWitnesses(s, tree, registries, side, literal) {
  const witnesses = [], excluded = [];
  for (const [p, entry] of tree) {
    if (!p.startsWith(attemptsScope) || !p.endsWith('.rs') || /\/tests?\//.test(p)) continue;
    const lines = s.readBlob(entry).split('\n');
    lines.forEach((line, index) => {
      if (!line.includes('"' + literal + '"') || line.trimStart().startsWith('//')) return;
      const registration = registries[side].some(c => c.name === literal &&
        p.startsWith(c.literalScope ?? c.scope));
      (registration ? excluded : witnesses).push({ path: p, line: index + 1 });
    });
  }
  return { witnesses, excluded };
}

function assertWitnessDebt(t, side, literal, witnesses, excluded, entries) {
  if (new Set(witnesses.map(w => w.path)).size >= 2) {
    const debt = entries.find(e => e.literal === literal);
    assert.ok(debt, JSON.stringify({ side, literal, witnesses, entries }));
    for (const witness of witnesses) assert.ok(debt.sites.some(site =>
      site.path === witness.path && site.line === witness.line), JSON.stringify(witness));
  }
  let reason = 'example absent from source';
  if (excluded.length) reason = 'scoped registered name';
  else if (witnesses.length) reason = 'direct source occurrences checked';
  t.diagnostic(JSON.stringify({ control: 'direct-source witnesses', side, literal,
    witnesses, excluded, reason }));
}

function injectionPlan(s, before, registries) {
  const existing = before.unregistered.head.entries[0];
  let literal = existing?.literal ?? 'proof_4024_fresh.json';
  if (!existing) {
    const occupied = new Set([...before.unregistered.base.entries.map(e => e.literal),
      ...registries.base.map(c => c.name), ...registries.head.map(c => c.name)]);
    while (occupied.has(literal)) literal = '_' + literal;
  }
  const headTree = new Map(s.headTree), additions = new Map(), sites = [];
  const old = existing && before.unregistered.base.entries
    .find(e => e.literal === existing.literal);
  const count = existing
    ? Math.max(1, (old?.files.length ?? 0) - existing.files.length + 1) : 2;
  for (let i = 0; i < count; i++) {
    let p = root + 'proof_4024_' + i + '.rs';
    while (headTree.has(p)) p = p.replace('.rs', '_fresh.rs');
    const entry = { path: p }; headTree.set(p, entry);
    additions.set(entry, sourceLiteral(literal)); sites.push({ path: p, line: 1 });
  }
  return { existing, literal, headTree, additions, sites };
}

test('04 real default snapshot exposes debt and rejects exact memory injection', t => {
  const s = createSnapshot({ cwd: repo, ...parseArgs([]) });
  const before = containersGate(s);
  assert.equal(before.exit, 0, JSON.stringify(before));
  const registries = {};
  for (const side of ['base', 'head']) {
    const tree = s[side + 'Tree'];
    registries[side] = parseRegistry(s.readBlob(tree.get('scripts/containers.json')));
    const entries = discoverUnregistered(tree, s.readBlob, registries[side]);
    assertDebt(before, side, entries);
    for (const e of entries) {
      assert.ok(e.files.length >= 2 && e.sites.length > 0);
      assert.deepEqual([...new Set(e.sites.map(site => site.path))].sort(codeOrder), e.files);
      for (const site of e.sites) assert.ok(Number.isInteger(site.line) && site.line > 0);
    }
    // Independent direct-source witnesses, without the discovery helper's matcher.
    for (const literal of ['run.json', 'report.json', 'experiment.json']) {
      const { witnesses, excluded } = directSourceWitnesses(s, tree, registries, side, literal);
      assertWitnessDebt(t, side, literal, witnesses, excluded, entries);
    }
  }
  const { existing, literal, headTree, additions, sites } = injectionPlan(s, before, registries);
  const injected = { ...s, headTree,
    readBlob: entry => additions.has(entry) ? additions.get(entry) : s.readBlob(entry) };
  const after = containersGate(injected), restored = containersGate(s);
  assert.deepEqual([before.exit, after.exit, restored.exit], [0, 1, 0], JSON.stringify(after));
  const rule = existing ? newFile : growth;
  const matched = after.diagnostics.filter(d => d.rule === rule && d.literal === literal);
  assert.deepEqual(matched.map(({ path, line }) => ({ path, line })), sites);
  for (const side of ['base', 'head']) {
    assert.equal(injected[side + 'Oid'], s[side + 'Oid']);
    const entry = s[side + 'Tree'].get('scripts/containers.json');
    assert.equal(injected.readBlob(entry), s.readBlob(entry));
  }
  assert.deepEqual(restored.unregistered, before.unregistered);
  t.diagnostic(JSON.stringify({ control: 'real snapshot inventory and injection',
    baseOid: s.baseOid, headOid: s.headOid, unregistered: before.unregistered,
    injectedSites: sites, literal, exits: [0, 1, 0], matched }));
});
