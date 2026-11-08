// #4014 gate 07 receiver_disk_sites matrix. Node built-ins only: each batch runs the gate
// in a child process on in-memory trees, so no gate or shared-test internals are imported.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';

const root = 'src-tauri/src/';
const gateUrl = new URL('./sanitizing_disk_access.mjs', import.meta.url).href;
const child = `
const { runGate } = await import(process.argv[1]);
const cases = JSON.parse(process.argv[2]);
const tree = files => new Map(Object.entries(files).map(([path, text]) =>
  [path, { path, text, mode: '100644', type: 'blob' }]));
console.log(JSON.stringify(cases.map(([base, head]) => {
  const r = runGate({ baseTree: tree(base), headTree: tree(head), baseOid: 'base-fixture',
    headOid: 'head-fixture', refOid: 'ref-fixture', requestedRef: 'fixture',
    readBlob: e => e?.text ?? '', assertStableHead() {} });
  return { status: r.status, exit: r.exit, totals: r.totals, approximation: r.approximation,
    fails: r.diagnostics.filter(d => d.status === 'FAIL'), errors: r.status === 'ERROR' ?
    r.diagnostics : undefined };
})));`;
function gate(cases) {
  const r = spawnSync(process.execPath,
    ['--input-type=module', '-e', child, gateUrl, JSON.stringify(cases)],
    { encoding: 'utf8', timeout: 120_000, maxBuffer: 64 * 1024 * 1024, windowsHide: true });
  assert.equal(r.error, undefined); assert.equal(r.signal, null);
  assert.equal(r.status, 0, r.stderr);
  return JSON.parse(r.stdout);
}
// Measure each source as one unchanged file; assert receiver and direct metrics separately.
function expectSites(cases, name = 'x.rs') {
  const results = gate(cases.map(([src]) => {
    const files = { [root + name]: src };
    return [files, files];
  }));
  for (const [n, [src, sites]] of cases.entries()) {
    const r = results[n];
    assert.equal(r.status, 'PASS', src + JSON.stringify(r));
    assert.equal(r.totals[1].receiver_disk_sites ?? 0, sites, src);
    assert.equal(r.totals[1].disk_files ?? 0, 0, src);
  }
  return results;
}
const methods = ['exists', 'try_exists', 'is_dir', 'is_file', 'metadata', 'symlink_metadata',
  'canonicalize', 'read_dir'];

test('07r each method on &Path and PathBuf parameters is a proven site', () => {
  expectSites(methods.flatMap(m => [
    ['use std::path::Path; fn f(p: &Path) { p.' + m + '(); }', 1],
    ['use std::path::PathBuf; fn f(p: PathBuf) { p.' + m + '(); }', 1],
    ['use std::path::Path; fn f(p: &Path) { p.' + m + '(1); }', 0],
  ]));
});

test('07r immediate standard constructors, aliases, qualified, multiline and raw names', () => {
  expectSites([
    ['fn f(x: &str) { std::path::Path::new(x).exists(); }', 1],
    ['fn f(x: &str) { ::std::path::Path::new(x).is_dir(); }', 1],
    ['use std::path::PathBuf as B; fn f(x: String) { B::from(x).is_file(); }', 1],
    ['use std::path::PathBuf; fn f() { PathBuf::new().exists(); }', 1],
    ['use std::path::{Path, PathBuf}; fn f(x: &str) { Path::new(x).exists(); '
      + 'PathBuf::from(x).exists(); }', 2],
    ['use std::path; fn f(x: &str) { path::Path::new(x).metadata(); }', 1],
    ['use std::path::{self as sp}; fn f(x: &str) { sp::PathBuf::from(x).read_dir(); }', 1],
    ['fn f(x: &str) {\n  std::path::Path\n    ::new(&x)\n    .exists();\n}', 1],
    ['use std::path::Path as r#P; fn f(x: &str) { r#P::new(x).exists(); }', 1],
    ['fn f(r#p: &std::path::Path) { r#p.exists(); }', 1],
    ['use std::path::Path; fn f() { Path::new().exists(); }', 0],
    ['use std::path::PathBuf; fn f(x: &str) { PathBuf::new(x).exists(); }', 0],
    ['use std::path::Path; fn f(x: &str) { Path::from(x).exists(); }', 0],
    ['fn f(x: &str) { Path::new(x).exists(); PathBuf::from(x).exists(); }', 0],
    ['use std::path::Path; fn f(x: &str) { Path::new(x).join("a").exists(); }', 0],
    ['use std::path::Path; fn f(x: &str) { Path::new(x).as_path().exists(); }', 0],
    ['use std::path::Path; fn f(x: &str) { Path::new(x).clone().exists(); }', 0],
    ['use std::path::Path; fn f(x: &str) { let p = Path::new(x); p.exists(); }', 1],
  ]);
});

test('07r lifetimes, mutability and lifetime-only versus generic declarations', () => {
  const use = 'use std::path::{Path, PathBuf}; ';
  expectSites([
    [use + "fn f<'a>(p: &'a Path) { p.exists(); }", 1],
    [use + "fn f<'a: 'b, 'b>(p: &'a mut PathBuf) { p.exists(); }", 1],
    [use + 'fn f(mut p: PathBuf) { p.exists(); }', 1],
    [use + "struct S<'a>(&'a u8); impl<'a> S<'a> { fn f(&self, p: &Path) { p.exists(); } }", 1],
    [use + 'trait T { fn f(&self, p: &Path) { p.exists(); } }', 1],
    [use + 'fn f(p: &Path) -> Option<Vec<u8>> { p.exists(); None }', 1],
    [use + 'struct S<T>(T); impl<T> S<T> { fn f(&self, p: &Path) { p.exists(); } }', 0],
    [use + 'struct S; impl<const N: usize> S { fn f(p: &Path) { p.exists(); } }', 0],
    [use + 'fn f<Path>(p: &Path) { p.exists(); }', 0],
    [use + 'fn f<T: Into<u8>>(p: &Path, t: T) { p.exists(); }', 0],
    [use + 'trait T<X> { fn f(p: &Path) { p.exists(); } }', 0],
    [use + 'fn f<T>(x: T) { std::path::Path::new("a").exists(); }', 0],
  ]);
});

test('07r exact real shapes before the site stay proven', () => {
  expectSites([
    ['use std::path::Path; async fn f(dir: &Path) -> R { let a = g().await?; '
      + 'let mut b: u8 = h()?; dir.is_dir(); Ok(()) }', 1],
    ['use std::path::Path; fn f(p: &Path) { if !p.exists() { return; } }', 1],
    ['fn f(x: String) { if !std::path::Path::new(&x).exists() {} }', 1],
    ['use std::path::Path; fn f(flag: bool, p: &Path) { if !(flag) { return; } p.exists(); }', 1],
    ['use std::path::Path; fn f(p: &Path) { let _ = 1; let ref r = 2; p.exists(); }', 1],
  ]);
});

test('07r std root guard rejects bindings, renamed roots and glob imports', () => {
  const site = ' fn f(p: &Path, x: &str) { p.exists(); ::std::path::Path::new(x).exists(); }';
  const results = expectSites([
    ['mod std {} use std::path::Path;' + site, 0],
    ['extern crate custom as std; use std::path::Path;' + site, 0],
    ['use custom as std; use std::path::Path;' + site, 0],
    ['use x::std; use std::path::Path;' + site, 0],
    ['use std::path::*;' + site, 0],
    ['use std::{fs, path::{*}};' + site, 0],
    ['use other::*; use std::path::Path;' + site, 0],
    ['use std::path::Path;' + site + ' fn g(std: u8) {}', 0],
    ['use std::path::Path;' + site + ' fn g() { let r#std = 1; }', 0],
    ['use std::path::Path;' + site + ' #[cfg(test)] mod tests { use super::*; }', 0],
    ['use std::path::Path;' + site + ' fn g(a: u8, b: u8) -> u8 { a * b }', 2],
  ]);
  for (const r of results) assert.equal(r.approximation.head.sites, 2);
});

test('07r real/homonym mix and homonym types never count', () => {
  const [mix] = expectSites([
    ['use std::path::Path; fn f(p: &Path, r: Record) { p.exists(); r.exists(); }', 1]]);
  assert.equal(mix.approximation.head.sites, 2);
  expectSites([
    ['use std::fs::Metadata; fn f(m: Metadata) { m.is_dir(); m.is_file(); }', 0],
    ['fn f(t: std::fs::FileType) { t.is_file(); t.is_dir(); }', 0],
    ['fn f(r: &log::Record) { r.metadata(); }', 0],
    ['fn f(b: log::RecordBuilder) { b.metadata(); }', 0],
    ['struct Record; fn f(r: Record) { r.exists(); }', 0],
    ['struct Path; fn f(p: &Path) { p.exists(); }', 0],
    ['use crate::model::Path; fn f(p: &Path) { p.exists(); }', 0],
    ['use std::path::Path; mod m { struct Path; fn f(p: &Path) { p.exists(); } }', 0],
    ['use std::path::Path; fn f(p: &Path) {} fn g<Path>(p: Path) { p.exists(); }', 0],
  ]);
});

test('07r provenance matrix retains unknowns and recognizes #4025 locals', () => {
  const use = 'use std::path::{Path, PathBuf}; ';
  const results = expectSites([
    [use + 'fn f() { dir().exists(); }', 0],
    [use + 'fn f(e: ContextEntry) { e.exists(); }', 0],
    [use + 'fn f(x: &str) { let p = PathBuf::from(x); p.exists(); }', 1],
    [use + 'fn f(p: &Path) { let q = p.join("a"); q.exists(); p.join("b").exists(); }', 1],
    [use + 'fn f(v: Vec<PathBuf>) { v[0].exists(); }', 0],
    [use + 'fn f(v: &[PathBuf]) { for p in v { p.exists(); } }', 0],
    [use + 'struct S { dir: D } impl S { fn f(&self, dir: &Path) { self.dir.is_dir(); } }', 0],
    [use + 'type P = std::path::Path; fn f(p: &P) { p.exists(); }', 0],
    [use + 'fn f(p: impl AsRef<Path>) { p.exists(); }', 0],
    [use + 'fn f((p, q): (&Path, u8)) { p.exists(); }', 0],
    [use + 'fn f(p: &&Path) { p.exists(); }', 0],
    [use + 'fn f(p: Box<Path>) { p.exists(); }', 0],
  ]);
  for (const r of results) assert.ok(r.approximation.head.sites >= 1);
});

test('07r rebinding, barriers and macros never create a false proven receiver', () => {
  const use = 'use std::path::Path; ';
  expectSites([
    [use + 'fn f(p: &Path) { let p = 1; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { let mut p: u8 = 1; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { { let p = other(); } p.exists(); }', 0],
    [use + 'fn f(p: &Path) { let p = p.exists(); }', 0],
    [use + 'fn f(p: &Path) { let (a, b) = x; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { if let Some(q) = x {} p.exists(); }', 0],
    [use + 'fn f(p: &Path) { for q in v {} p.exists(); }', 0],
    [use + 'fn f(p: &Path) { match x { p => p.exists() }; }', 0],
    [use + 'fn f(p: &Path) { let g = |p: u8| p; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { let g = || 1; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { a || b; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { fn g(p: u8) { p.exists(); } p.exists(); }', 0],
    [use + 'fn f(p: &Path) { fn g(q: &Path) { q.exists(); } }', 1],
    [use + 'fn f(p: &Path) { static p: u8 = 0; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { const C: u8 = 0; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { bind!(p); p.exists(); }', 0],
    [use + 'fn f(p: &Path) { m::bind![p]; p.exists(); }', 0],
    [use + 'fn f(p: &Path) { assert!(p.exists()); }', 0],
    [use + 'fn f(p: &Path) { #[allow(p.exists())] let a = 1; }', 0],
    [use + 'fn f(p: &Path) { g(p.exists()); }', 1],
    [use + 'fn f(p: &Path) { (p).exists(); x.p.exists(); a::p.exists(); }', 0],
    [use + 'fn f(p: &Path) {} fn g() { p.exists(); }', 0],
  ]);
});

test('07r owners and tests are excluded; non-owner lookalikes are measured', () => {
  const src = 'use std::path::Path; fn f(p: &Path) { p.exists(); }';
  const names = ['filesystem.rs', 'filesystem/read.rs', 'path_identity.rs', 'path_utils/x.rs',
    'tests.rs', 'foo/tests/read.rs', 'foo_tests.rs', 'filesystem_extra.rs', 'containers/read.rs'];
  const results = gate(names.map(name => [{ [root + name]: src }, { [root + name]: src }]));
  for (const [n, name] of names.entries())
    assert.equal(results[n].totals[1].receiver_disk_sites ?? 0, n < 7 ? 0 : 1, name);
  expectSites([
    ['#[cfg(test)] mod tests { use std::path::Path; fn f(p: &Path) { p.exists(); } }', 0],
    ['use std::path::Path; #[test] fn f() { Path::new("a").exists(); }', 0],
  ]);
});

test('07r ratchet: equal/down PASS, growth and new files FAIL, no move credit', () => {
  const p = root + 'old.rs', q = root + 'new.rs', use = 'use std::path::Path; ';
  const one = use + 'fn f(a: &Path) { a.exists(); }';
  const two = use + 'fn f(a: &Path) { a.exists(); a.is_dir(); }';
  const base = { [p]: one };
  const results = gate([
    [base, base], [base, { [p]: use }], [base, { [p]: two }],
    [base, { [p]: use, [q]: one }], [base, { [q]: one }], [base, { [p]: one, [q]: one }],
    [{ [p]: two }, { [p]: one, [q]: one }],
  ]);
  assert.deepEqual(results.map(r => r.exit), [0, 0, 1, 1, 1, 1, 1]);
  assert.ok(results[2].fails.some(d => d.rule === 'receiver_disk_sites' && d.path === p &&
    d.reason === 'global subrule debt increased'));
  for (const r of results.slice(3)) assert.ok(r.fails.some(d =>
    d.rule === 'receiver_disk_sites' && d.path === q), JSON.stringify(r));
  assert.ok(results[6].fails.every(d => d.reason.startsWith('new file')));
  for (const r of results) assert.equal(r.totals[1].disk_files ?? 0, 0);
});

test('07r positive control: 0 to 1 proven site FAILs only through receiver_disk_sites', () => {
  const p = root + 'control.rs';
  const before = 'use std::path::Path;\nfn f(p: &Path) {\n  let _ = p;\n}\n';
  const after = 'use std::path::Path;\nfn f(p: &Path) {\n  let _ = 1;\n  p.exists();\n}\n';
  const [clean, bad, restored] = gate([
    [{ [p]: before }, { [p]: before }], [{ [p]: before }, { [p]: after }],
    [{ [p]: before }, { [p]: before }]]);
  assert.equal(clean.exit, 0); assert.equal(restored.exit, 0);
  assert.deepEqual([bad.totals[0].receiver_disk_sites ?? 0, bad.totals[1].receiver_disk_sites],
    [0, 1]);
  assert.equal(bad.status, 'FAIL'); assert.equal(bad.exit, 1);
  assert.deepEqual([bad.totals[0].disk_files ?? 0, bad.totals[1].disk_files ?? 0], [0, 0]);
  assert.equal(bad.fails.length, 1);
  const [d] = bad.fails;
  assert.deepEqual([d.rule, d.path, d.line, d.base, d.head], ['receiver_disk_sites', p, 4, 0, 1]);
  assert.ok(d.remedy);
});

test('07r direct calls, unsupported receivers and malformed input keep their verdicts', () => {
  const p = root + 'disk.rs';
  const direct = 'fn f(r: Record) { std::fs::read("x"); r.exists(); }';
  const [fail, error, unknown] = gate([
    [{ [p]: 'fn f() {}' }, { [p]: direct }],
    [{ [p]: 'fn f() {}' }, { [p]: 'fn f() { "unterminated }' }],
    [{ [p]: 'fn f() {}' }, { [p]: 'fn f(r: Record) { r.exists(); }' }],
  ]);
  assert.equal(fail.exit, 1);
  assert.deepEqual(fail.fails.map(d => d.rule), ['disk_files']);
  assert.equal(fail.totals[1].receiver_disk_sites ?? 0, 0);
  assert.equal(error.status, 'ERROR');
  assert.equal(unknown.exit, 0);
  assert.equal(unknown.approximation.head.sites, 1);
});

// #4025 extends only the local shapes above; old immediate and parameter controls remain.
const localC = 'std::path::Path::new("x")';
const localP = 'p.join("x")';
function localSource(seed, prefix = '', call = 'q.exists();', declaration = 'let q') {
  return 'fn f(p: &std::path::Path, flag: bool) { ' + prefix + ' ' + declaration +
    ' = ' + seed + '; ' + call + ' }';
}

test('07l each method on constructor and parameter locals, zero to two joins', () => {
  expectSites(methods.flatMap(m => [localC, localC + '.join("a")',
    localC + '.join("a").join("b")', 'std::path::PathBuf::new()',
    'std::path::PathBuf::from("x")', localP, localP + '.join("b")']
    .map(seed => [localSource(seed, '', 'q.' + m + '();'), 1])));
  expectSites([
    ['use std::path::PathBuf as B; fn f(x: String) { let q = B::from(x); q.exists(); }', 1],
    ['use std::path::Path as r#P; fn f(x: &str) { let r#q = r#P::new(x); '
      + 'r#q.is_dir(); }', 1],
    ['fn f(x: &str) { let q = ::std::path::Path::new(\n x,\n).join(g(1, 2),); '
      + 'q.is_file(); }', 1],
    ["fn f<'a>(p: &'a std::path::Path) { let q = p.join(g()); q.exists(); }", 1],
    ["struct S<'a>(&'a u8); impl<'a> S<'a> { fn f(&self, p: &std::path::Path) { "
      + 'let q = p.join("x"); q.exists(); } }', 1],
    ['fn f(p: std::path::PathBuf) { let q = p.join("x"); { q.exists(); } }', 1],
    ['fn f(p: &std::path::Path) { { let q = p.join("x"); q.exists(); } }', 1],
    ...[localC, localP].map(seed => [localSource(seed,
      'let a = g().await?; let mut b: u8 = h()?; if !(flag) { return; }'), 1]),
  ]);
});

test('07l C and P controls identify the exact receiver failure and restoration', () => {
  const p = root + 'control.rs';
  for (const seed of [localC, localP]) {
    const before = { [p]: 'fn f() {}' };
    const after = { [p]: 'fn f(p: &std::path::Path) {\n  let q = ' + seed +
      ';\n  q.exists();\n}' };
    const [clean, bad, restored] = gate([[before, before], [before, after], [before, before]]);
    assert.equal(clean.exit, 0); assert.equal(restored.exit, 0);
    assert.equal(bad.status, 'FAIL'); assert.equal(bad.exit, 1);
    assert.deepEqual(bad.totals.map(t => t.receiver_disk_sites ?? 0), [0, 1]);
    assert.deepEqual(bad.totals.map(t => t.disk_files ?? 0), [0, 0]);
    assert.equal(bad.fails.length, 1);
    const [d] = bad.fails;
    assert.deepEqual([d.rule, d.path, d.line, d.base, d.head],
      ['receiver_disk_sites', p, 3, 0, 1]);
    assert.ok(d.remedy);
  }
});

test('07l pipe barriers C/P each have a pipe-free one-site control', () => {
  for (const seed of [localC, localP]) expectSites([
    [localSource(seed), 1],
    ...['a || b;', 'let g = || 1;', 'let g = |a| a;', 'let g = |a||b| b;']
      .map(prefix => [localSource(seed, prefix), 0]),
  ]);
});

test('07l immutable simple declarations, exact initializers and lexical visibility', () => {
  for (const seed of [localC, localP]) expectSites([
    ...['let mut q', 'let ref q', 'let q: std::path::PathBuf', 'let (q, r)']
      .map(decl => [localSource(seed, '', 'q.exists();', decl), 0]),
    ...['?', '.await', '.as_path()', '.parent()', '.clone()']
      .map(suffix => [localSource(seed + suffix), 0]),
    [localSource('(' + seed + ')'), 0],
    [localSource(seed, '', 'let r = q; r.exists();'), 0],
    [localSource(seed, '', 'self.q.exists(); x.q.exists(); a::q.exists(); (q).exists();'), 0],
    [localSource(seed, '', 'q.exists(1);'), 0],
    [localSource(seed, 'q.exists();', ''), 0],
    [localSource(seed, '', 'let q = other(); q.exists();'), 0],
    [localSource(seed, '{ let q = other(); }'), 0],
    [localSource(seed, '', '{ let q = other(); } q.exists();'), 0],
    ['fn f(p: &std::path::Path) { { let q = ' + seed + '; } q.exists(); }', 0],
    [localSource(seed, '', 'fn inner() { q.exists(); }'), 0],
    [localSource(seed, '', 'm!(q.exists());'), 0],
    [localSource(seed, '', '#[attr(q.exists())] let x = 1;'), 0],
    ...['let (a, b) = x;', 'if let Some(a) = x {}', 'let a = x else { return; };',
      'for a in x {}', 'match x { _ => () };', 'bind!(a);', 'm::bind![a];',
      'g({ let (a,b) = x; });', 'g({ let a = || 1; });']
      .map(prefix => [localSource(seed, prefix), 0]),
    ...['const q: u8 = 1;', 'static q: u8 = 1;', 'type q = u8;', 'use m::q;',
      'struct Q;', 'enum Q {}', 'union Q { x: u8 }', 'trait Q {}', 'impl Q {}',
      'extern "C" {}', 'mod m {}', 'macro_rules! m { () => {} }']
      .map(item => [localSource(seed, '', 'q.exists(); ' + item), 0]),
    ['fn f(q: Other, p: &std::path::Path) { let q = ' + seed + '; q.exists(); }', 0],
    ['fn f<T>(p: &std::path::Path) { let q = ' + seed + '; q.exists(); }', 0],
    ['#[cfg(test)] mod tests { ' + localSource(seed) + ' }', 0],
  ]);
});

test('07l constructor identity, arity and parameter seed rejection', () => {
  expectSites([
    [localSource('Path::new("x")'), 0],
    ['struct Path; fn f() { let q = Path::new("x"); q.exists(); }', 0],
    ['mod std {} ' + localSource(localC), 0],
    ['extern crate custom as std; ' + localSource(localC), 0],
    ['use other::*; ' + localSource(localC), 0],
    ['fn f<Path>() { let q = Path::new("x"); q.exists(); }', 0],
    ['fn f() { type P = std::path::Path; let q = P::new("x"); q.exists(); }', 0],
    ...['std::path::Path::new()', 'std::path::Path::new("x", "y")',
      'std::path::PathBuf::new("x")', 'std::path::PathBuf::from()',
      'std::path::PathBuf::from("x", "y")', 'std::path::Path::new("x").join()',
      'p.join()', 'p.join("x", "y")', 'p', 'helper().join("x")', 'p.parent()',
      'self.p.join("x")'].map(seed => [localSource(seed), 0]),
    ...['std::fs::Metadata', 'std::fs::FileType', 'log::Record', 'Custom', 'Path']
      .map(type => ['fn f(p: ' + type + ') { let q = p.join("x"); q.exists(); }', 0]),
    [localSource(localP, 'let p = other();'), 0],
    [localSource(localP, '{ let p = other(); }'), 0],
    [localSource('std::path::Path::new(g({ let (a,b) = x; a }))'), 0],
    [localSource('p.join(g({ let (a,b) = x; a }))'), 0],
  ]);
});

const traitJ = 'trait J { fn join(&self, s: &str) -> std::fs::Metadata; } '
  + 'impl J for std::path::PathBuf { fn join(&self, s: &str) -> std::fs::Metadata '
  + '{ panic!() } } ';
const traitD = 'trait D { fn is_dir(&self) -> bool; } '
  + 'impl D for std::path::PathBuf { fn is_dir(&self) -> bool { false } } ';

test('07l accepted trait limit exposes false FAIL and compensating false PASS', () => {
  const p = root + 'limit.rs';
  const falseSite = traitJ + 'fn f() { let m = std::path::PathBuf::from("x").join("y"); '
    + 'm.is_dir(); }';
  const overridden = traitD + 'fn f() { let q = std::path::Path::new("x").join("y"); '
    + 'q.is_dir(); }';
  expectSites([[falseSite, 1], [overridden, 1],
    [traitD + 'fn f(p: &std::path::Path) { p.is_dir(); }', 1],
    [localSource(localP, '', 'q.is_dir();'), 1]]);
  const base = falseSite + ' fn b(p: &std::path::Path) {}';
  const head = traitJ + 'fn b(p: &std::path::Path) { p.is_dir(); }';
  const results = gate([
    [{ [p]: '' }, { [p]: falseSite }], [{ [p]: '' }, { [p]: overridden }],
    [{ [p]: base }, { [p]: head }],
    // Equivalent old-only counts: the local was unsupported, the parameter still counts.
    [{ [p]: traitJ + 'fn b(p: &std::path::Path) {}' }, { [p]: head }],
  ]);
  assert.deepEqual(results.map(r => r.exit), [1, 1, 0, 1]);
  assert.deepEqual(results[2].totals.map(t => t.receiver_disk_sites), [1, 1]);
  assert.deepEqual(results[3].totals.map(t => t.receiver_disk_sites ?? 0), [0, 1]);
  for (const r of results) assert.ok(r.totals.every(t => (t.disk_files ?? 0) === 0));
  for (const r of [results[0], results[1], results[3]])
    assert.ok(r.fails.some(d => d.rule === 'receiver_disk_sites'));
});

test('07l local ratchet and unchanged old/direct forms in unsupported functions', () => {
  const p = root + 'old.rs', q = root + 'new.rs', one = localSource(localP);
  const two = localSource(localP, '', 'q.exists(); q.is_dir();');
  const base = { [p]: one };
  const results = gate([[base, base], [base, { [p]: '' }], [base, { [p]: two }],
    [base, { [p]: '', [q]: one }], [base, { [q]: one }],
    [base, { [p]: one, [q]: one }], [base,
      base]]);
  assert.deepEqual(results.map(r => r.exit), [0, 0, 1, 1, 1, 1, 0]);
  for (const r of results) assert.equal(r.totals[1].disk_files ?? 0, 0);
  expectSites([['fn f(p: &std::path::Path) { p.exists(); let (a,b) = x; '
    + 'std::path::Path::new("x").exists(); let q = p.join("x"); q.exists(); }', 2]]);
  const [direct] = gate([[{ [p]: '' }, { [p]: 'use other::*; fn f() { '
    + 'std::fs::read("x"); let q = std::path::Path::new("x"); q.exists(); }' }]]);
  assert.equal(direct.exit, 1);
  assert.deepEqual(direct.fails.map(d => d.rule), ['disk_files']);
});

test('07l unrelated function-pointer types and path keywords are not item declarations', () => {
  for (const seed of [localC, localP]) expectSites([
    [localSource(seed, 'let handler: fn(u8) = h;'), 1],
    [localSource(seed, 'let x = obj.r#type();'), 1],
    [localSource(seed, 'let x = module::r#const();'), 1],
    [localSource(seed + '.join(g({ let a = 1; a }))'), 1],
  ]);
});

test('4086 associated witness: an existing-file Path query grows only receiver debt', () => {
  const p = root + 'associated.rs';
  const before = { [p]: 'fn f(p: &std::path::Path) {}' };
  const after = { [p]: 'fn f(p: &std::path::Path) {\nstd::path::Path::exists(p);\n}' };
  const [r] = gate([[before, after]]);
  assert.equal(r.status, 'FAIL'); assert.equal(r.exit, 1);
  assert.deepEqual(r.totals.map(t => t.receiver_disk_sites ?? 0), [0, 1]);
  assert.deepEqual(r.totals.map(t => t.disk_files ?? 0), [0, 0]);
  assert.deepEqual(r.fails.map(d => [d.rule, d.path, d.line]),
    [['receiver_disk_sites', p, 2]]);
});

test('4086 associated finite methods, aliases and single argument forms', () => {
  expectSites(methods.flatMap(m => [
    ['fn f(p: &std::path::Path) { std::path::Path::' + m + '(p); }', 1],
    ['fn f(p: &std::path::Path) { ::std::path::Path::' + m + '(p,); }', 1],
    ['use std::path::{Path as P}; fn f() { P::' + m + '(g(1, 2)); }', 1],
    ['use std::path::Path; fn f() { Path::' + m + '(p); }', 1],
    ['use std::path as sp; fn f() { sp::Path::' + m + '(p); }', 1],
    ['use std::path::{self as sp}; fn f() { sp::Path::' + m + '(p); }', 1],
    ['use std::path::Path as r#P; fn f() { r#P::r#' + m + '\n(p); }', 1],
    ['fn f() { std::path::Path\n::' + m + '(\np,\n); }', 1],
  ]));
});

test('4086 associated excludes unsupported identities, contexts and arities', () => {
  const call = 'std::path::Path::exists(p);';
  expectSites([
    ['struct Path; fn f() { Path::exists(p); }', 0],
    ['use custom::Path; fn f() { Path::exists(p); }', 0],
    ['use std::path::Path; mod m { struct Path; fn f() { Path::exists(p); } }', 0],
    ['use std::path::Path; use custom::Path; fn f() { Path::exists(p); }', 0],
    ['mod std {} fn f() { ' + call + ' }', 0],
    ['fn f(std: u8) { ' + call + ' }', 0],
    ['use other::*; fn f() { ' + call + ' }', 0],
    ['use std::path::*; fn f() { Path::exists(p); }', 0],
    ['fn f<T>() { ' + call + ' }', 0],
    ['struct S<T>(T); impl<T> S<T> { fn f() { ' + call + ' } }', 0],
    ['trait T<X> { fn f() { ' + call + ' } }', 0],
    ['fn f() { /* ' + call + ' */ let s = "' + call + '"; }', 0],
    ['fn f() { m!(' + call + '); }', 0],
    ['#[attr(std::path::Path::exists(p))] fn f() {}', 0],
    ['#[cfg(test)] mod tests { fn f() { ' + call + ' } }', 0],
    ['#[test] fn f() { ' + call + ' }', 0],
    ...['std::path::PathBuf::exists(p)', '<std::path::Path>::exists(p)',
      '<std::path::Path as T>::exists(p)', 'std::path::Path::exists::<T>(p)',
      'std::path::Path::exists()', 'std::path::Path::exists(p, q)',
      'std::path::Path::exists', 'std::path::Path::new(p).join("x")',
      'obj.std::path::Path::exists(p)']
      .map(expr => ['fn f() { ' + expr + '; }', 0]),
  ]);
  for (const name of ['filesystem.rs', 'path_identity/a.rs', 'path_utils.rs',
    'tests.rs', 'foo/tests/a.rs', 'foo_tests.rs']) expectSites([['fn f() { ' + call + ' }', 0]],
    name);
});

test('4086 associated ratchet and mixed sites preserve separate old metrics', () => {
  const p = root + 'old.rs', q = root + 'new.rs';
  const one = 'fn f(p: &std::path::Path) { std::path::Path::exists(p); }';
  const two = one + ' fn g() { std::path::Path::is_dir(p); }';
  const base = { [p]: one };
  const results = gate([[base, base], [base, { [p]: '' }], [base, { [p]: two }],
    [base, { [p]: '', [q]: one }], [base, { [q]: one }]]);
  assert.deepEqual(results.map(r => r.exit), [0, 0, 1, 1, 1]);
  assert.ok(results[3].fails.some(d => d.path === q && d.rule === 'receiver_disk_sites'));
  const mixed = 'fn f(p: &std::path::Path) { p.exists(); std::path::Path::exists(p); '
    + 'std::fs::read("x"); std::path::Path::metadata(std::path::Path::canonicalize(p)); }';
  const [r] = gate([[{ [p]: mixed }, { [p]: mixed }]]);
  assert.equal(r.exit, 0);
  assert.equal(r.totals[1].receiver_disk_sites, 4);
  assert.equal(r.totals[1].disk_files, 1);
  assert.equal(r.approximation.head.sites, 1);
});
