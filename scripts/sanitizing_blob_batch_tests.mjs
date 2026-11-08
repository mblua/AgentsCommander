// Protocol and compatibility checks for the synchronous snapshot transport.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createSnapshot, gitRunner, decode } from './sanitizing_baseline.mjs';

const id = (text, width = 40) => createHash(width === 40 ? 'sha1' : 'sha256')
  .update(text).digest('hex');
const ok = stdout => ({ status: 0, signal: null, stdout, stderr: Buffer.alloc(0) });
const entry = (name, body, options = {}) => ({
  path: name, body: Buffer.from(body), mode: '100644', type: 'blob',
  oid: id(body, options.width), ...options,
});
const treeBytes = entries => Buffer.from(entries.map(e =>
  `${e.mode} ${e.type} ${e.oid}\t${e.path}\0`).join(''));
const record = (oid, body) => Buffer.concat([
  Buffer.from(`${oid} blob ${body.length}\n`), body, Buffer.from('\n'),
]);
function fixture({ base = [], head = base, changed = [], batch, legacy } = {}) {
  const calls = [], groups = [], objects = new Map();
  for (const e of [...base, ...head]) objects.set(e.oid, e.body);
  const runner = (args, options) => {
    calls.push({ args, options });
    if (args[0] === 'rev-parse' || args[0] === 'merge-base')
      return ok(Buffer.from(id(args[0] === 'merge-base' ? 'base' : args.at(-1)) + '\n'));
    if (args[0] === 'ls-tree') {
      const prior = calls.filter(c => c.args[0] === 'ls-tree').length;
      return ok(treeBytes(prior === 1 ? base : head));
    }
    if (args[0] === 'diff')
      return ok(Buffer.from(changed.map(([status, name]) => `${status}\0${name}\0`).join('')));
    if (args[1] === '--batch') {
      assert.ok(Buffer.isBuffer(options.input));
      assert.equal(options.cwd, 'snapshot-fixture');
      const oids = options.input.toString().trimEnd().split('\n');
      groups.push(oids);
      if (batch) return batch(oids, objects);
      return ok(Buffer.concat(oids.map(oid => record(oid, objects.get(oid)))));
    }
    if (legacy) return legacy(args[2], objects);
    return ok(objects.get(args[2]));
  };
  const snapshot = createSnapshot({ cwd: 'snapshot-fixture', runner });
  return { snapshot, calls, groups, objects,
    individual: () => calls.filter(c => c.args[1] === 'blob') };
}
const names = n => Array.from({ length: n }, (_, i) =>
  entry(`src/${String(i).padStart(3, '0')}.rs`, `body ${i}`));
const readAll = f => [...f.snapshot.headTree.values()].map(e => f.snapshot.readBlob(e));

test('lazy construction, missing entries, empty trees and cache reuse', () => {
  const f = fixture();
  assert.equal(f.calls.length, 6);
  assert.equal(f.snapshot.readBlob(undefined), '');
  assert.deepEqual(f.groups, []);
  const e = entry('x.rs', 'hello');
  const g = fixture({ base: [e] });
  const actual = g.snapshot.baseTree.get(e.path);
  assert.equal(g.snapshot.readBlob(actual), 'hello');
  assert.equal(g.snapshot.readBlob({ ...actual, path: 'alias.rs' }), 'hello');
  assert.equal(g.groups.length, 1);
  assert.equal(g.individual().length, 0);
});

for (const count of [1, 128, 129, 300]) {
  test(`${count} distinct blobs: bounded multi-OID requests and fewer Git processes`, () => {
    const entries = names(count), f = fixture({ base: entries });
    assert.deepEqual(readAll(f), entries.map(e => e.body.toString()));
    assert.equal(f.groups.length, Math.ceil(count / 128));
    assert.ok(f.groups.every(g => g.length <= 128));
    if (count > 1) assert.ok(f.groups[0].length > 1);
    assert.equal(f.individual().length, 0);
    assert.equal(new Set(f.groups.flat()).size, count);
  });
}

test('changed Rust locality: head first, adjacency, additions/deletions, no unrelated fill', () => {
  const a = entry('a.rs', 'a old'), ah = entry('a.rs', 'a new');
  const d = entry('d.rs', 'deleted'), z = entry('z.rs', 'added');
  const unrelated = entry('b.rs', 'unrelated'), txt = entry('BASELINE', 'baseline');
  const unsupported = entry('e.rs', 'link', { mode: '120000' });
  const shared = entry('f.rs', 'a new');
  const f = fixture({ base: [a, unrelated, d, txt, unsupported],
    head: [ah, unrelated, shared, z, txt, unsupported],
    changed: [['A', 'z.rs'], ['M', 'a.rs'], ['D', 'd.rs'], ['T', 'e.rs'], ['A', 'f.rs']] });
  assert.equal(f.snapshot.readBlob(f.snapshot.headTree.get('a.rs')), 'a new');
  assert.deepEqual(f.groups, [[ah.oid, a.oid, d.oid, z.oid]]);
  assert.equal(f.snapshot.readBlob(f.snapshot.baseTree.get('d.rs')), 'deleted');
  assert.equal(f.snapshot.readBlob(f.snapshot.headTree.get('z.rs')), 'added');
  assert.equal(f.groups.length, 1);
  assert.equal(f.individual().length, 0);
});

test('changed sequence never wraps to earlier paths', () => {
  const es = names(4);
  const f = fixture({ base: es, changed: es.map(e => ['M', e.path]) });
  f.snapshot.readBlob(f.snapshot.baseTree.get(es[2].path));
  assert.deepEqual(f.groups, [[es[2].oid, es[3].oid]]);
});

test('same-tree scan follows sorted Rust paths only; base identity wins', () => {
  const a = entry('a.rs', 'base a'), ah = entry('a.rs', 'head a');
  const b = entry('b.txt', 'text'), c = entry('c.rs', 'base c');
  const ch = entry('c.rs', 'head c'), z = entry('z.rs', 'tail');
  const f = fixture({ base: [z, c, b, a], head: [z, ch, b, ah] });
  f.snapshot.readBlob(f.snapshot.baseTree.get('c.rs'));
  assert.deepEqual(f.groups, [[c.oid, z.oid]]);
  const g = fixture({ base: [z, c, b, a], head: [z, ch, b, ah] });
  g.snapshot.readBlob(g.snapshot.headTree.get('a.rs'));
  assert.deepEqual(g.groups, [[ah.oid, ch.oid, z.oid]]);
});

test('non-Rust and external unseen requested objects are singletons', () => {
  const a = entry('BASELINE', 'configuration'), b = entry('z.rs', 'rust');
  const f = fixture({ base: [a, b] });
  f.snapshot.readBlob(f.snapshot.baseTree.get(a.path));
  const outside = entry('external.rs', 'outside');
  f.objects.set(outside.oid, outside.body);
  assert.equal(f.snapshot.readBlob(outside), 'outside');
  assert.deepEqual(f.groups, [[a.oid], [outside.oid]]);
});

for (const width of [40, 64]) {
  test(`opaque binary framing and ${width}-character OIDs`, () => {
    const bodies = ['', '\n', '\r\n', 'no final LF', '\0x\0', 'λ日本語', '\ufeffBOM',
      `${'a'.repeat(width)} blob 55\nbody\n`];
    const entries = bodies.map((body, i) => entry(`${i}.rs`, body, { width }));
    const f = fixture({ base: entries });
    assert.deepEqual(readAll(f), bodies);
    assert.equal(f.groups.length, 1);
  });
}

test('unsupported requested modes and types fail before any blob transport', () => {
  for (const options of [{ mode: '120000' }, { type: 'tree' }, { mode: '160000' }]) {
    const e = entry('bad.rs', 'x', options), f = fixture({ base: [e] });
    assert.throws(() => f.snapshot.readBlob(f.snapshot.baseTree.get(e.path)), error => {
      assert.equal(error.exit, 3);
      assert.equal(error.message, `${e.path}: unsupported mode/type ${e.mode}/${e.type}; ` +
        'use a regular tracked file');
      return true;
    });
    assert.equal(f.calls.length, 6);
  }
});

test('speculative invalid UTF8 stays lazy and retains requested path context', () => {
  const a = entry('a.rs', 'valid'), b = entry('b.rs', Buffer.from([0xff]));
  const f = fixture({ base: [a, b] });
  assert.equal(f.snapshot.readBlob(f.snapshot.baseTree.get(a.path)), 'valid');
  assert.throws(() => f.snapshot.readBlob({ ...b, path: 'first-use.rs' }), {
    message: 'first-use.rs: invalid UTF8; repair the Git input', exit: 3,
  });
  assert.throws(() => f.snapshot.readBlob({ ...b, path: 'retry.rs' }), {
    message: 'retry.rs: invalid UTF8; repair the Git input', exit: 3,
  });
  assert.equal(f.groups.length, 1);
  assert.equal(f.individual().length, 0);
});

const corruptions = {
  empty: () => Buffer.alloc(0),
  missing: ([oid]) => Buffer.from(`${oid} missing\n`),
  header: () => Buffer.from('not a header\n'),
  type: ([oid]) => Buffer.from(`${oid} tree 0\n\n`),
  negative: ([oid]) => Buffer.from(`${oid} blob -1\n\n`),
  unsafe: ([oid]) => Buffer.from(`${oid} blob 9007199254740992\n`),
  fractional: ([oid]) => Buffer.from(`${oid} blob 1.5\nx\n`),
  truncatedHeader: ([oid]) => Buffer.from(`${oid} blob 0`),
  truncatedBody: ([oid]) => Buffer.from(`${oid} blob 20\nx\n`),
  trailer: ([oid]) => Buffer.from(`${oid} blob 1\nx!`),
  extra: (oids, objects) => Buffer.concat([
    ...oids.map(oid => record(oid, objects.get(oid))), Buffer.from('extra'),
  ]),
  reversed: (oids, objects) => Buffer.concat(
    [...oids].reverse().map(oid => record(oid, objects.get(oid)))),
  partial: ([oid]) => record(oid, Buffer.from('wrong prefetched content')),
  highBit: ([oid]) => Buffer.from(`${oid} bl\xefb 0\n\n`, 'latin1'),
};
for (const [name, corrupt] of Object.entries(corruptions)) {
  test(`malformed ${name}: atomic discard, original reads, no batch retry`, () => {
    const es = names(2), f = fixture({ base: es, batch: (oids, objects) =>
      ok(corrupt(oids, objects)) });
    assert.deepEqual(readAll(f), es.map(e => e.body.toString()));
    assert.equal(f.groups.length, 1);
    assert.equal(f.individual().length, 2);
    assert.deepEqual(f.individual().map(c => c.args[2]), es.map(e => e.oid));
  });
}

const processFailures = {
  thrown: () => { throw new Error('spawn exploded'); },
  error: () => ({ ...ok(Buffer.alloc(0)), error: new Error('buffer overflow') }),
  signal: () => ({ ...ok(Buffer.alloc(0)), signal: 'SIGTERM' }),
  nonzero: () => ({ ...ok(Buffer.alloc(0)), status: 128 }),
  nullStatus: () => ({ ...ok(Buffer.alloc(0)), status: null }),
  nonBuffer: () => ({ ...ok(Buffer.alloc(0)), stdout: 'not binary' }),
  stdoutLimit: () => ok(Buffer.alloc(64 * 1024 * 1024 + 1)),
  stderrLimit: () => ({ ...ok(Buffer.alloc(0)), stderr: { length: 64 * 1024 * 1024 + 1 } }),
};
for (const [name, fail] of Object.entries(processFailures)) {
  test(`batch process ${name} falls back to valid individual objects`, () => {
    const es = names(2), f = fixture({ base: es, batch: fail });
    assert.deepEqual(readAll(f), es.map(e => e.body.toString()));
    assert.equal(f.groups.length, 1);
    assert.equal(f.individual().length, 2);
  });
}

test('batch and legacy failure preserves the complete original GateError and exit', () => {
  const e = entry('a.rs', 'body');
  const fail = () => ({ status: 128, signal: null, stdout: Buffer.alloc(0),
    stderr: Buffer.from('fatal: missing object\n') });
  const f = fixture({ base: [e], batch: fail, legacy: fail });
  assert.throws(() => readAll(f), {
    message: 'git cat-file failed: exit=128 signal=-  fatal: missing object\n' +
      '; fetch full history/check objects', exit: 3,
  });
  assert.deepEqual(f.calls.slice(-2).map(c => c.args[1]), ['--batch', 'blob']);
});

test('legacy spawn failure and oversized individual object retain original diagnostics', () => {
  const e = entry('a.rs', 'body');
  const thrown = fixture({ base: [e], batch: processFailures.thrown,
    legacy: processFailures.thrown });
  assert.throws(() => readAll(thrown), {
    message: 'git cat-file spawn: spawn exploded', exit: 3,
  });
  const overflow = fixture({ base: [e], batch: processFailures.stdoutLimit,
    legacy: processFailures.stdoutLimit });
  assert.throws(() => readAll(overflow), {
    message: 'git cat-file failed: exit=0 signal=-  ; fetch full history/check objects', exit: 3,
  });
});

test('gitRunner accepts binary stdin with cwd/env and leaves no child; import is inert', () => {
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), 'blob-batch-'));
  try {
    const env = { ...process.env, GIT_CONFIG_COUNT: '1',
      GIT_CONFIG_KEY_0: 'test.batch', GIT_CONFIG_VALUE_0: 'forwarded' };
    const input = Buffer.from('λ\0binary\r\n');
    const hashed = gitRunner(['hash-object', '--stdin'], { cwd, env, input });
    assert.equal(hashed.status, 0);
    const expected = createHash('sha1').update(`blob ${input.length}\0`).update(input)
      .digest('hex');
    assert.equal(hashed.stdout.toString().trim(), expected);
    assert.equal(gitRunner(['config', 'test.batch'], { cwd, env }).stdout.toString(),
      'forwarded\n');
    const ignored = gitRunner(['hash-object', '--stdin'], { cwd, env });
    assert.equal(ignored.status, 0);
    assert.notEqual(ignored.stdout.toString(), hashed.stdout.toString());
    assert.equal(hashed.signal, null);
    const url = new URL('./sanitizing_baseline.mjs', import.meta.url).href;
    const imported = spawnSync(process.execPath, ['--input-type=module', '-e',
      `await import(${JSON.stringify(url)})`], { cwd, env: { ...env, PATH: '' },
      timeout: 10_000, windowsHide: true });
    assert.equal(imported.status, 0);
    assert.equal(imported.stdout.length, 0);
    assert.equal(imported.stderr.length, 0);
    assert.equal(decode(input, 'binary'), input.toString());
  } finally { fs.rmSync(cwd, { recursive: true, force: true }); }
});

test('changed base/head adjacency at the cap does not overfill or lose the next read', () => {
  const base = names(70), head = base.map(e => entry(e.path, e.body + 'changed'));
  const f = fixture({ base, head, changed: base.map(e => ['M', e.path]) });
  const first = f.snapshot.headTree.get(head[0].path);
  f.snapshot.readBlob(first);
  assert.equal(f.groups[0].length, 128);
  assert.deepEqual(f.groups[0].slice(0, 4), [head[0].oid, base[0].oid,
    base[1].oid, head[1].oid]);
  assert.deepEqual(readAll(f), head.map(e => e.body.toString()));
  assert.equal(f.groups.length, 2);
  assert.equal(f.individual().length, 0);
});

test('gitRunner keeps bounded binary spawn options and resolved Git executable', () => {
  const url = new URL('./sanitizing_baseline.mjs', import.meta.url).href;
  const source = `
    import assert from 'node:assert/strict';
    import child from 'node:child_process';
    import { syncBuiltinESMExports } from 'node:module';
    const calls = [];
    child.spawnSync = (...args) => { calls.push(args); return {status: 0}; };
    syncBuiltinESMExports();
    const {gitRunner} = await import(${JSON.stringify(url)});
    assert.equal(calls.length, 0);
    const cwd = process.cwd(), env = {...process.env}, input = Buffer.from([111, 105, 100, 10]);
    gitRunner(['cat-file', '--batch'], {cwd, env, input});
    gitRunner(['status'], {cwd, env});
    assert.equal(calls.length, 2);
    const [exe, args, options] = calls[0];
    assert.ok(exe.endsWith('git.exe') || exe.endsWith('git'));
    assert.equal(calls[1][0], exe);
    assert.deepEqual(args, ['cat-file', '--batch']);
    assert.deepEqual(options, {cwd, env, input, stdio: ['pipe','pipe','pipe'],
      timeout: 120000, maxBuffer: 64 * 1024 * 1024, windowsHide: true});
    assert.deepEqual(calls[1][2].stdio, ['ignore','pipe','pipe']);
    assert.equal(calls[1][2].input, undefined);
  `;
  const result = spawnSync(process.execPath, ['--input-type=module', '-e', source], {
    timeout: 10_000, windowsHide: true,
  });
  assert.equal(result.status, 0, result.stderr.toString());
});
