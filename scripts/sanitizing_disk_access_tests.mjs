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
    ['use std::path::Path; fn f(x: &str) { let p = Path::new(x); p.exists(); }', 0],
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

test('07r unknown provenance remains WARN-only, never a proven site', () => {
  const use = 'use std::path::{Path, PathBuf}; ';
  const results = expectSites([
    [use + 'fn f() { dir().exists(); }', 0],
    [use + 'fn f(e: ContextEntry) { e.exists(); }', 0],
    [use + 'fn f(x: &str) { let p = PathBuf::from(x); p.exists(); }', 0],
    [use + 'fn f(p: &Path) { let q = p.join("a"); q.exists(); p.join("b").exists(); }', 0],
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
