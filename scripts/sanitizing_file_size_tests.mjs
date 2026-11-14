import test from 'node:test';
import assert from 'node:assert/strict';
import { fixture, text, sequence } from './sanitizing_tests.mjs';
import { runGate as sizeGate } from './sanitizing_file_size.mjs';
import { runGate as lengthGate } from './sanitizing_line_length.mjs';

test('02 TS/TSX ratchet reports growing debt and permits shrinking debt', t => {
  const f = fixture(t);
  for (const [base, head] of [[1000, 1001], [1001, 1002]]) {
    const result = sequence(f, sizeGate, 'x.ts', text(base), text(head));
    const diagnostic = result.diagnostics.find(entry => entry.path === 'x.ts');
    assert.equal(diagnostic.rule, 'file_lines');
    assert.equal(diagnostic.status, 'FAIL');
    assert.equal(diagnostic.base, base);
    assert.equal(diagnostic.head, head);
  }
  sequence(f, sizeGate, 'x.tsx', text(1002), text(1001), 0);
});

test('02 new TSX files fail above 1000 lines and pass at the limit', t => {
  const f = fixture(t);
  for (const [lines, exit] of [[1001, 1], [1000, 0]]) {
    f.write('new.tsx', text(lines)); f.commit();
    assert.equal(sizeGate(f.snapshot(f.base)).exit, exit);
  }
});

test('02 existing TS debt permits edits without growth and reductions', t => {
  const f = fixture(t);
  sequence(f, sizeGate, 'big.ts', text(1500), 'b\n' + text(1499), 0);
  sequence(f, sizeGate, 'big.ts', text(1500), text(1400), 0);
});

test('02 mixed Rust and TS growth preserves separate file counts', t => {
  const f = fixture(t);
  for (const name of ['x.rs', 'x.ts']) f.write(name, text(1000));
  const base = f.commit();
  for (const name of ['x.rs', 'x.ts']) f.write(name, text(1001));
  f.commit();
  const result = sizeGate(f.snapshot(base));
  assert.equal(result.exit, 1);
  assert.equal(result.changedRustFiles, 1);
  assert.equal(result.changedTsFiles, 1);
  assert.ok(result.diagnostics.every(entry => entry.status === 'FAIL'));
});

test('01 continues ignoring TS and omits the TS file count', t => {
  const f = fixture(t);
  f.write('a.ts', text(1, 101)); f.commit();
  const result = lengthGate(f.snapshot());
  assert.equal(result.exit, 0);
  assert.equal(result.changedRustFiles, 0);
  assert.equal(Object.hasOwn(result, 'changedTsFiles'), false);
});

test('02 ignores new JS and MTS files above the limit', t => {
  const f = fixture(t);
  for (const name of ['new.js', 'new.mts']) f.write(name, text(1001));
  f.commit();
  assert.equal(sizeGate(f.snapshot()).exit, 0);
});

test('02 treats a TS symlink as ERROR', t => {
  const f = fixture(t), blob = f.git('hash-object', '-w', '--stdin');
  f.git('update-index', '--add', '--cacheinfo', '120000', blob, 'special.ts');
  f.git('-c', 'core.hooksPath=', 'commit', '-m', 'symlink');
  assert.equal(sizeGate(f.snapshot()).exit, 3);
});
