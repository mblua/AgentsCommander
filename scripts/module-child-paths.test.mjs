// Child-path tests for the module cycle detector (#3998).
// Each case is a tiny throwaway lib crate in OS temp. The real detector runs on
// it, and the test checks the exact module arcs and cyclic member sets.
// Run: node --test scripts/module-child-paths.test.mjs
//
// Case format, one text block per case:
//   line 1      `<id> <title>`
//   `arcs:`     expected arcs, space separated (crate name stripped)
//   `cycles:`   expected cyclic member sets, `|` between sets, space between members
//   `tests`     optional: also run with --include-tests, expecting `tarcs:`/`tcycles:`
//   `deps:`     optional Cargo.toml dependency line
//   `known:`    optional: KNOWN LIMITATION, the line says what true Rust gives
//   `== <path>` starts a source file; following lines are its content
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const DETECTOR = path.join(here, '01-rust_module-dependency-cycles.mjs');

const CASES = [
  `C01 bare child use
arcs: a->a::child
== src/lib.rs
mod a;
== src/a.rs
mod child;
use child::f;
pub fn g() { f() }
== src/a/child.rs
pub fn f() {}`,

  `C04 nested child paths
arcs: a->a::b::c a::b->a::b::c
== src/lib.rs
mod a;
== src/a.rs
mod b;
use b::c::deep;
pub fn g() { deep() }
== src/a/b.rs
pub mod c;
use c::deep as d2;
pub fn h() { d2() }
== src/a/b/c.rs
pub fn deep() {}`,

  `C05 child uses super, parent uses child: real 2-cycle
arcs: a->a::child a::child->a
cycles: a a::child
== src/lib.rs
mod a;
== src/a.rs
mod child;
use child::f;
pub struct S;
pub fn g() { f() }
== src/a/child.rs
use super::S;
pub fn f() { let _ = S; }`,

  `C06 grandchild uses super::super
arcs: a->a::b::c a::b::c->a
cycles: a a::b::c
== src/lib.rs
mod a;
== src/a.rs
mod b;
use b::c::deep;
pub struct S;
pub fn g() { deep() }
== src/a/b.rs
pub mod c;
== src/a/b/c.rs
use super::super::S;
pub fn deep() { let _ = S; }`,

  `C07 crate path from a child
arcs: a->a::child a::child->other
== src/lib.rs
mod a;
mod other;
== src/a.rs
mod child;
use child::f;
pub fn g() { f() }
== src/a/child.rs
use crate::other::X;
pub fn f() { let _ = X; }
== src/other.rs
pub struct X;`,

  `C09 pub use child::T re-export
arcs: consumer->parent parent->parent::child
== src/lib.rs
mod parent;
mod consumer;
== src/parent.rs
mod child;
pub use child::T;
== src/parent/child.rs
pub struct T;
== src/consumer.rs
use crate::parent::T;
pub fn g(_: T) {}`,

  `C10 glob re-export
arcs: consumer->parent parent->parent::child
== src/lib.rs
mod parent;
mod consumer;
== src/parent.rs
mod child;
pub use child::*;
== src/parent/child.rs
pub struct T;
== src/consumer.rs
use crate::parent::T;
pub fn g(_: T) {}`,

  `C11 renamed pub(crate) re-export
arcs: consumer->parent parent->parent::child
== src/lib.rs
mod parent;
mod consumer;
== src/parent.rs
mod child;
pub(crate) use child::T as Renamed;
== src/parent/child.rs
pub struct T;
== src/consumer.rs
use crate::parent::Renamed;
pub fn g(_: Renamed) {}`,

  `C12 facade re-export, child uses super
arcs: consumer->parent parent->parent::child parent::child->parent
cycles: parent parent::child
== src/lib.rs
mod parent;
mod consumer;
== src/parent.rs
mod child;
pub use child::T;
pub struct Helper;
== src/parent/child.rs
use super::Helper;
pub struct T(pub Helper);
== src/consumer.rs
use crate::parent::T;
pub fn g(_: T) {}`,

  `C14 test-gated child use: excluded by default, included with --include-tests
arcs: a::child->a
tests
tarcs: a->a::child a::child->a
tcycles: a a::child
== src/lib.rs
mod a;
== src/a.rs
mod child;
pub struct S;
#[cfg(all(test, windows))]
use child::f;
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn t() { f(); }
}
== src/a/child.rs
use super::S;
pub fn f() { let _ = S; }`,

  `C19 local child named like an external crate
arcs: a->a::log
deps: log = "0.4"
== src/lib.rs
mod a;
mod b;
== src/a.rs
mod log;
use log::info;
pub fn g() { info() }
== src/a/log.rs
pub fn info() {}
== src/b.rs
use log::info;
pub fn g() { info!("x") }`,

  `C20 local child named like a crate-root module
arcs: a->a::util a::b->a::b::util
== src/lib.rs
mod a;
mod util;
== src/util.rs
pub fn top() {}
== src/a.rs
mod b;
mod util;
use util::y;
pub fn g() { y() }
== src/a/util.rs
pub fn y() {}
== src/a/b.rs
mod util;
use util::x;
pub fn g() { x() }
== src/a/b/util.rs
pub fn x() {}`,

  `C24 grouped use with nested self alias
arcs: a->a::child a->a::child::sub
== src/lib.rs
mod a;
== src/a.rs
mod child;
use child::{f, sub::{self as subm}};
pub fn g(_: subm::X) { f() }
== src/a/child.rs
pub mod sub;
pub fn f() {}
== src/a/child/sub.rs
pub struct X;`,

  // KNOWN LIMITATION cases pin current detector output that is NOT true Rust
  // semantics, so a change in behavior is noticed. They are not semantic PASS.
  `C08b D1 #[path] decoy file closes a false cycle
known: rustc reads src/imp_x.rs, so true Rust has a->a::imp only and no cycle
arcs: a->a::imp a::imp->a
cycles: a a::imp
== src/lib.rs
mod a;
== src/a.rs
#[path = "imp_x.rs"]
mod imp;
use imp::f;
pub struct S;
pub fn g() { f() }
== src/imp_x.rs
pub fn f() {}
== src/a/imp_x.rs
use super::S;
pub fn f() { let _ = S; }`,

  `C21 D2 import alias named like a crate-root module
known: use std::fmt as util binds the alias, so true Rust has util->a only
arcs: a->util util->a
cycles: a util
== src/lib.rs
pub mod util;
pub mod a;
== src/util.rs
use crate::a::A;
pub fn x(_: A) {}
== src/a.rs
use std::fmt as util;
use util::Display;
pub struct A;
pub fn g<T: Display>(_: T) {}`,

  `C18 D3 exclusive cfg arcs are unioned into one cycle
known: parent arc is windows-only, child arc unix-only; no platform has the cycle
arcs: a->a::child a::child->a
cycles: a a::child
== src/lib.rs
mod a;
== src/a.rs
mod child;
pub struct S;
#[cfg(windows)]
use child::f;
#[cfg(windows)]
pub fn g() { f() }
== src/a/child.rs
#[cfg(unix)]
use super::S;
pub fn f() {}
#[cfg(unix)]
pub fn h() { let _ = S; }`,
];

const words = (s) => (s ? s.split(' ').filter(Boolean) : []);
const sets = (s) => (s ? s.split('|').map((m) => words(m).sort()) : []);

/** Parses one case block into its header fields and source files. */
function parseCase(text) {
  const [title, ...lines] = text.split('\n');
  const head = {};
  const files = {};
  let file = null;
  for (const line of lines) {
    if (line.startsWith('== ')) {
      file = line.slice(3);
      files[file] = '';
    } else if (file) {
      files[file] += `${line}\n`;
    } else {
      const at = line.indexOf(':');
      if (at < 0) head[line] = '';
      else head[line.slice(0, at)] = line.slice(at + 1).trim();
    }
  }
  return { id: title.split(' ')[0], title, head, files };
}

/** Writes one fixture crate, runs the detector and returns arcs and cycles. */
function analyze(c, includeTests) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `child-paths-${c.id}-`));
  try {
    const name = `fx_${c.id.toLowerCase()}`;
    const deps = c.head.deps ? `\n[dependencies]\n${c.head.deps}\n` : '';
    fs.writeFileSync(path.join(dir, 'Cargo.toml'),
      `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n${deps}`);
    for (const [rel, body] of Object.entries(c.files)) {
      const abs = path.join(dir, rel);
      fs.mkdirSync(path.dirname(abs), { recursive: true });
      fs.writeFileSync(abs, body);
    }
    const graphPath = path.join(dir, 'graph.json');
    const args = [DETECTOR, dir, '--json', '--fail-on', 'none', '--emit-graph', graphPath];
    if (includeTests) args.push('--include-tests');
    const run = spawnSync(process.execPath, args, {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: 60_000,
    });
    assert.equal(run.status, 0, `detector failed: ${run.stderr}`);
    const report = JSON.parse(run.stdout);
    const graph = JSON.parse(fs.readFileSync(graphPath, 'utf8'));
    const strip = (m) => (m === name ? '<root>' : m.slice(name.length + 2));
    const arcs = [...new Set(graph.edges.map((e) => `${strip(e.from)}->${strip(e.to)}`))].sort();
    const cycles = report.moduleCycles.map((cy) => cy.members.map(strip).sort());
    return { arcs, cycles: cycles.sort((a, b) => a.join().localeCompare(b.join())) };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

function check(c, includeTests, arcs, cycles) {
  const res = analyze(c, includeTests);
  assert.deepEqual(res.arcs, words(arcs).sort());
  assert.deepEqual(res.cycles, sets(cycles));
}

for (const text of CASES) {
  const c = parseCase(text);
  const label = 'known' in c.head ? `KNOWN LIMITATION ${c.title}` : c.title;
  test(label, () => check(c, false, c.head.arcs, c.head.cycles));
  if ('tests' in c.head) {
    test(`${c.title} (--include-tests)`, () => check(c, true, c.head.tarcs, c.head.tcycles));
  }
}
