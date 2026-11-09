// Tests for the script line-length ratchet (#4130). Long inputs are built at runtime so
// this file keeps every line within 100 Unicode characters.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { GateError } from './sanitizing_baseline.mjs';
import { isScriptPath, longLineNumbers, runGate } from './sanitizing_script_line_length.mjs';

const long = (n = 101) => 'x'.repeat(n);
const lines = (count, n = 101) => Array.from({ length: count }, () => long(n)).join('\n') + '\n';

// Fake snapshot: base/head map path -> text; a missing path is absent from that tree.
function fakeSnapshot(base, head, options = {}) {
  const tree = (files, side) => new Map(Object.keys(files).map(p => [p,
    { path: p, mode: options.mode?.[p] ?? '100644', type: 'blob', oid: `${side}:${p}` }]));
  const baseTree = tree(base, 'base'), headTree = tree(head, 'head');
  const texts = new Map([...Object.entries(base).map(([p, t]) => [`base:${p}`, t]),
    ...Object.entries(head).map(([p, t]) => [`head:${p}`, t])]);
  const paths = [...new Set([...Object.keys(base), ...Object.keys(head)])].sort()
    .filter(p => base[p] !== head[p]);
  return {
    requestedRef: 'origin/main', refOid: 'r'.repeat(40), baseOid: 'b'.repeat(40),
    headOid: 'h'.repeat(40), baseTree, headTree,
    changed: paths.map(p => ({ status: !(p in base) ? 'A' : !(p in head) ? 'D' : 'M', path: p })),
    readBlob(entry) {
      if (!entry) return '';
      if (options.readError?.(entry)) throw new GateError(`${entry.path}: invalid UTF8`);
      if (entry.mode !== '100644') throw new GateError(`${entry.path}: unsupported mode`);
      return texts.get(entry.oid);
    },
    assertStableHead() { if (options.drift) throw new GateError('HEAD drift'); },
  };
}

test('100 code points pass and 101 count as long', () => {
  assert.deepEqual(longLineNumbers(`${long(100)}\n${long(101)}\n`), [2]);
});
test('astral code points count once each', () => {
  const astral = '\u{1F600}';
  assert.deepEqual(longLineNumbers(astral.repeat(100)), []);
  assert.deepEqual(longLineNumbers(astral.repeat(101)), [1]);
});
test('LF, CRLF, lone CR and missing final newline', () => {
  assert.deepEqual(longLineNumbers(`${long(100)}\r\n${long(100)}\n`), []);
  assert.deepEqual(longLineNumbers(`${long(100)}\r`), [1]);
  assert.deepEqual(longLineNumbers(`a\n${long()}`), [2]);
  assert.deepEqual(longLineNumbers(`${long(50)}\r${long(50)}\n`), [1]);
});
test('equal and lower debt pass, higher debt fails', () => {
  const p = 'scripts/a.mjs';
  assert.equal(runGate(fakeSnapshot({ [p]: lines(2) }, { [p]: lines(2) + 'x\n' })).exit, 0);
  assert.equal(runGate(fakeSnapshot({ [p]: lines(2) }, { [p]: lines(1) })).exit, 0);
  const result = runGate(fakeSnapshot({ [p]: lines(2) }, { [p]: lines(3) }));
  assert.equal(result.status, 'FAIL');
  assert.equal(result.exit, 1);
  assert.deepEqual(result.diagnostics[0].lines, [1, 2, 3]);
});
test('new, deleted and renamed files', () => {
  assert.equal(runGate(fakeSnapshot({}, { 'scripts/n.js': lines(1) })).exit, 1);
  assert.equal(runGate(fakeSnapshot({}, { 'scripts/n.js': 'ok\n' })).exit, 0);
  const deleted = runGate(fakeSnapshot({ 'scripts/d.js': lines(4) }, {}));
  assert.equal(deleted.exit, 0);
  assert.deepEqual(deleted.metrics, [{ path: 'scripts/d.js', base: 4, head: 0 }]);
  const renamed = runGate(fakeSnapshot({ 'scripts/old.js': lines(1) },
    { 'scripts/new.js': lines(1) }));
  assert.equal(renamed.exit, 1);
  assert.deepEqual(renamed.diagnostics.map(d => [d.path, d.status]),
    [['scripts/new.js', 'FAIL'], ['scripts/old.js', 'PASS']]);
});
test('cross-file compensation is rejected', () => {
  const result = runGate(fakeSnapshot(
    { 'scripts/a.mjs': lines(3), 'scripts/b.mjs': lines(1) },
    { 'scripts/a.mjs': lines(1), 'scripts/b.mjs': lines(2) }));
  assert.equal(result.status, 'FAIL');
  assert.deepEqual(result.diagnostics.map(d => d.status), ['PASS', 'FAIL']);
});
test('script selection: suffixes, subdirectories, exclusions', () => {
  for (const p of ['scripts/a.mjs', 'scripts/a.cjs', 'scripts/a.js', 'scripts/x/y/z.mjs',
    'scripts/fixtures/f.js', 'scripts/a_tests.mjs']) assert.ok(isScriptPath(p), p);
  for (const p of ['scriptsX/a.mjs', 'scripts/a.rs', 'src/a.js', 'x/scripts/a.js',
    'Scripts/a.js', 'scripts/a.JS', 'scripts/a.ts']) assert.ok(!isScriptPath(p), p);
  const head = { 'scriptsX/a.mjs': lines(1), 'scripts/a.rs': lines(1), 'src/a.js': lines(1) };
  const result = runGate(fakeSnapshot({}, head));
  assert.equal(result.status, 'PASS');
  assert.equal(result.changedScriptFiles, 0);
});
test('comments and fixture scripts are counted', () => {
  const head = { 'scripts/fixtures/f.cjs': `// ${long()}\n` };
  assert.equal(runGate(fakeSnapshot({}, head)).exit, 1);
});
test('zero changed scripts pass explicitly', () => {
  const result = runGate(fakeSnapshot({}, {}));
  assert.deepEqual([result.status, result.exit, result.changedScriptFiles], ['PASS', 0, 0]);
  assert.deepEqual(result.metrics, []);
});
test('diagnostics carry path, counts, status, lines, reason and remedy', () => {
  const p = 'scripts/a.mjs';
  const result = runGate(fakeSnapshot({ [p]: 'a\n' }, { [p]: `a\n${long()}\n` }));
  assert.equal(result.gate, 'script_line_length');
  assert.equal(result.changedScriptFiles, 1);
  assert.deepEqual(result.metrics, [{ path: p, base: 0, head: 1 }]);
  const [d] = result.diagnostics;
  assert.deepEqual([d.gate, d.rule, d.path, d.base, d.head, d.status, d.lines],
    ['script_line_length', 'long_lines', p, 0, 1, 'FAIL', [2]]);
  assert.match(d.reason, /increased/);
  assert.match(d.remedy, /100/);
  assert.deepEqual([result.requestedRef, result.refOid, result.baseOid, result.headOid],
    ['origin/main', 'r'.repeat(40), 'b'.repeat(40), 'h'.repeat(40)]);
});
test('HEAD drift overrides failure and success', () => {
  const p = 'scripts/a.mjs';
  for (const head of [lines(2), 'ok\n']) {
    const result = runGate(fakeSnapshot({ [p]: lines(1) }, { [p]: head }, { drift: true }));
    assert.deepEqual([result.status, result.exit], ['ERROR', 3]);
    assert.match(result.diagnostics.at(-1).reason, /HEAD drift/);
  }
});
test('read and mode errors override an earlier failure', () => {
  const base = { 'scripts/a.mjs': '', 'scripts/b.mjs': 'b\n' };
  const head = { 'scripts/a.mjs': lines(1), 'scripts/b.mjs': 'c\n' };
  const utf = runGate(fakeSnapshot(base, head, { readError: e => e.path === 'scripts/b.mjs' }));
  assert.deepEqual([utf.status, utf.exit], ['ERROR', 3]);
  assert.equal(utf.diagnostics[0].status, 'FAIL');
  assert.match(utf.diagnostics.at(-1).reason, /UTF8/);
  const mode = runGate(fakeSnapshot(base, head, { mode: { 'scripts/b.mjs': '120000' } }));
  assert.deepEqual([mode.status, mode.exit], ['ERROR', 3]);
});

// Isolated real Git/CLI fixture (plan option a): copies of the tested gate sources run
// in a fresh temporary repository with sanitized Git configuration.
const here = path.dirname(fileURLToPath(import.meta.url));
const COPIED = ['sanitizing_baseline.mjs', 'sanitizing_line_metrics.mjs',
  'sanitizing_script_line_length.mjs'];
function fixtureEnv(root) {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^GIT_CONFIG_(KEY|VALUE)_/.test(key) || ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE',
      'GIT_COMMON_DIR', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_CONFIG',
      'GIT_CONFIG_COUNT', 'GIT_CONFIG_PARAMETERS'].includes(key)) delete env[key];
  }
  return { ...env, GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: path.join(root, 'git-global-config'),
    GIT_TEMPLATE_DIR: path.join(root, 'git-template') };
}
test('real Git fixture: merge-base, committed blobs, dirty worktree and CLI exits', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ac-4130-'));
  try {
    const repo = path.join(root, 'repo');
    fs.mkdirSync(path.join(repo, 'scripts'), { recursive: true });
    fs.writeFileSync(path.join(root, 'git-global-config'), '');
    fs.mkdirSync(path.join(root, 'git-template'));
    fs.mkdirSync(path.join(root, 'hooks'));
    const env = fixtureEnv(root);
    const git = (args, input) => {
      const r = spawnSync('git', args, { cwd: repo, env, input, encoding: 'utf8' });
      assert.equal(r.status, 0, `git ${args.join(' ')}: ${r.stderr}`);
      return r.stdout.trim();
    };
    const cli = (...args) => spawnSync(process.execPath,
      [path.join(repo, 'scripts', 'sanitizing_script_line_length.mjs'), ...args],
      { cwd: repo, env, encoding: 'utf8' });
    git(['init', '-q']);
    for (const [key, value] of [['user.name', 'Fixture'], ['user.email', 'fixture@invalid'],
      ['commit.gpgsign', 'false'], ['core.hooksPath', path.join(root, 'hooks')],
      ['core.autocrlf', 'false']]) git(['config', key, value]);
    for (const name of COPIED) {
      fs.copyFileSync(path.join(here, name), path.join(repo, 'scripts', name));
      assert.ok(fs.readFileSync(path.join(here, name))
        .equals(fs.readFileSync(path.join(repo, 'scripts', name))), name);
    }
    const probe = path.join(repo, 'scripts', 'probe.js');
    fs.writeFileSync(probe, 'export const probe = 1;\n');
    git(['add', '-A']);
    git(['commit', '-q', '-m', 'A']);
    const a = git(['rev-parse', 'HEAD']);
    // Divergent D from A via plumbing, before the worktree is dirtied; its probe has debt.
    const blob = git(['hash-object', '-w', '--stdin'], `${long()}\n${long()}\n`);
    const entries = git(['ls-tree', 'HEAD:scripts']).split('\n')
      .filter(line => !line.endsWith('\tprobe.js'));
    const scripts = git(['mktree'], [...entries, `100644 blob ${blob}\tprobe.js`].join('\n') + '\n');
    const tree = git(['mktree'], `040000 tree ${scripts}\tscripts\n`);
    const d = git(['commit-tree', tree, '-p', a, '-m', 'D']);

    const pass = cli('--base-ref', a);
    assert.equal(pass.status, 0, pass.stderr);
    const passJson = JSON.parse(pass.stdout);
    assert.deepEqual([passJson.status, passJson.changedScriptFiles, passJson.headOid],
      ['PASS', 0, a]);

    fs.writeFileSync(probe, `export const probe = 1;\n// ${long(98)}\n`);
    git(['commit', '-q', '-am', 'B']);
    const b = git(['rev-parse', 'HEAD']);
    const expectFail = (result, refOid) => {
      assert.equal(result.status, 1, result.stderr);
      const json = JSON.parse(result.stdout);
      assert.equal(json.status, 'FAIL');
      assert.deepEqual([json.refOid, json.baseOid, json.headOid], [refOid, a, b]);
      assert.deepEqual(json.metrics, [{ path: 'scripts/probe.js', base: 0, head: 1 }]);
    };
    expectFail(cli('--base-ref', a), a);

    fs.writeFileSync(probe, 'export const probe = 1;\n');
    expectFail(cli('--base-ref', a), a);
    assert.equal(git(['rev-parse', 'HEAD']), b);
    expectFail(cli('--base-ref', d), d);

    assert.equal(cli('--bogus').status, 2);
    assert.equal(cli('--base-ref').status, 2);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
