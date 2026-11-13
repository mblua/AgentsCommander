import test from 'node:test';
import assert from 'node:assert/strict';
import { runGate } from './sanitizing_lock_poison.mjs';
import { parseAllowlist } from './sanitizing_baseline.mjs';

const p = 'src-tauri/src/p.rs';
const sig = 'fn f(m: &std::sync::Mutex<u8>)';
const body = call => `${sig} { let _g = m${call}; }`;
const ok = body('.lock().unwrap_or_else(std::sync::PoisonError::into_inner)');
function trees(base, head = base) {
  const tree = files => new Map(Object.entries(files).map(([path, text]) =>
    [path, { path, text, mode: '100644', type: 'blob' }]));
  return { baseTree: tree(base), headTree: tree(head), baseOid: 'base-fixture',
    headOid: 'head-fixture', refOid: 'ref-fixture', requestedRef: 'fixture',
    readBlob: entry => entry?.text ?? '', assertStableHead() {} };
}
function pass(base, head = base) {
  const result = runGate(trees(base, head));
  assert.equal(result.exit, 0, JSON.stringify(result));
}
function fail(base, head, path = p, line = 1) {
  const result = runGate(trees(base, head));
  assert.equal(result.exit, 1, JSON.stringify(result));
  assert.ok(result.diagnostics.some(d => d.status === 'FAIL' && d.rule === 'lock_panic' &&
    d.path === path && d.line === line), JSON.stringify(result));
}

test('10 PASS -> FAIL with diagnostic -> restored PASS', () => {
  pass({ [p]: ok });
  fail({ [p]: ok }, { [p]: body('.lock().unwrap()') });
  pass({ [p]: ok }, { [p]: ok });
});
test('10 every panicking acquisition form fails, multi-line too', () => {
  for (const call of ['.lock().unwrap()', '.lock().expect("x")', '.read().unwrap()',
    '.write().expect("x")']) fail({}, { [p]: body(call) });
  fail({}, { [p]: `${sig} {\n  m\n    .lock()\n    .unwrap();\n}` }, p, 3);
});
test('10 existing debt still fails: absolute, not a ratchet', () => {
  const debt = { [p]: body('.lock().unwrap()') };
  fail(debt, debt);
});
test('10 crates are in the universe; integration tests are not', () => {
  const c = 'crates/x/src/lib.rs';
  fail({}, { [c]: body('.lock().unwrap()') }, c);
  pass({}, { 'src-tauri/tests/it.rs': body('.lock().unwrap()') });
});
test('10 test code, comments and strings are ignored', () => {
  for (const text of [
    `#[cfg(test)]\nmod t { ${body('.lock().unwrap()')} }`,
    `#[test]\n${body('.lock().unwrap()')}`,
    `${sig} {\n  #[cfg(test)]\n  m.lock().unwrap();\n}`,
    `${sig} {\n  #[cfg(all(test, windows))]\n  let _g = m.lock().unwrap();\n}`,
    `${sig} {\n  #[cfg(test)]\n  { m.lock().unwrap(); }\n}`,
    'fn f(o: Option<&std::sync::Mutex<u8>>) {\n  #[cfg(test)]\n' +
      '  if let Some(m) = o { m.lock().unwrap(); }\n}',
    `struct S { a: u8 }\n${sig} -> S {\n  S { #[cfg(test)]\n a: *m.lock().unwrap() }\n}`,
    `${sig} {\n  match 1 {\n    #[cfg(test)]\n    1 => { m.lock().unwrap(); }\n` +
      '    _ => {}\n  }\n}',
    'fn f() { /* m.lock().unwrap() */ let _s = "m.lock().unwrap()"; }',
  ]) pass({}, { [p]: text });
});
test('10 a test-only cfg range ends at its statement; other cfgs stay production', () => {
  fail({}, { [p]: `${sig} {\n  #[cfg(test)]\n  let _a = 1;\n  let _g = m.lock().unwrap();\n}` },
    p, 4);
  fail({}, { [p]: `${sig} {\n  #[cfg(any(test, unix))]\n  let _g = m.lock().unwrap();\n}` },
    p, 3);
  fail({}, { [p]: `${sig} {\n  #[cfg(not(test))]\n  let _g = m.lock().unwrap();\n}` }, p, 3);
  fail({}, { [p]: `${sig} {\n  match 1 {\n    #[cfg(test)]\n    1 => {}\n` +
    '    _ => { m.lock().unwrap(); }\n  }\n}' }, p, 5);
});
test('10 non-panicking forms and other acquisitions pass', () => {
  for (const call of ['.lock().unwrap_or_else(|e| e.into_inner())', '.try_lock().unwrap()',
    '.lock().map_err(|_| ())']) pass({}, { [p]: body(call) });
});
test('10 the allowlist cannot accept gate 10', () => {
  assert.throws(() => parseAllowlist(JSON.stringify({ version: 1, entries: [{ gate: '10',
    rule: 'lock_panic', path: p, reason: 'x', since: '2026-11-13', issue: 98 }] })));
});
