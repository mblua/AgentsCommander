// Child-path tests for the module cycle detector (#3998).
// Each case is a tiny throwaway lib crate in OS temp. The real detector runs on
// it, and the test checks the exact module arcs and cyclic member sets.
// Run: node --test scripts/module-child-paths.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const DETECTOR = path.join(here, '01-rust_module-dependency-cycles.mjs');

const pkg = (name, deps = '') =>
  `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n${deps}`;

/** Writes one fixture crate, runs the detector and returns arcs and cycles. */
function analyze(id, files, { deps = '', includeTests = false } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `child-paths-${id}-`));
  try {
    const name = `fx_${id.toLowerCase()}`;
    fs.writeFileSync(path.join(dir, 'Cargo.toml'), pkg(name, deps));
    for (const [rel, text] of Object.entries(files)) {
      const abs = path.join(dir, rel);
      fs.mkdirSync(path.dirname(abs), { recursive: true });
      fs.writeFileSync(abs, text);
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
    const cycles = report.moduleCycles
      .map((c) => c.members.map(strip).sort())
      .sort((a, b) => a.join().localeCompare(b.join()));
    return { arcs, cycles };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const CASES = [
  {
    id: 'C01', title: 'bare child use',
    files: {
      'src/lib.rs': 'mod a;\n',
      'src/a.rs': 'mod child;\nuse child::f;\npub fn g() { f() }\n',
      'src/a/child.rs': 'pub fn f() {}\n',
    },
    arcs: ['a->a::child'], cycles: [],
  },
  {
    id: 'C04', title: 'nested child paths',
    files: {
      'src/lib.rs': 'mod a;\n',
      'src/a.rs': 'mod b;\nuse b::c::deep;\npub fn g() { deep() }\n',
      'src/a/b.rs': 'pub mod c;\nuse c::deep as d2;\npub fn h() { d2() }\n',
      'src/a/b/c.rs': 'pub fn deep() {}\n',
    },
    arcs: ['a->a::b::c', 'a::b->a::b::c'], cycles: [],
  },
  {
    id: 'C05', title: 'child uses super, parent uses child: real 2-cycle',
    files: {
      'src/lib.rs': 'mod a;\n',
      'src/a.rs': 'mod child;\nuse child::f;\npub struct S;\npub fn g() { f() }\n',
      'src/a/child.rs': 'use super::S;\npub fn f() { let _ = S; }\n',
    },
    arcs: ['a->a::child', 'a::child->a'], cycles: [['a', 'a::child']],
  },
  {
    id: 'C06', title: 'grandchild uses super::super',
    files: {
      'src/lib.rs': 'mod a;\n',
      'src/a.rs': 'mod b;\nuse b::c::deep;\npub struct S;\npub fn g() { deep() }\n',
      'src/a/b.rs': 'pub mod c;\n',
      'src/a/b/c.rs': 'use super::super::S;\npub fn deep() { let _ = S; }\n',
    },
    arcs: ['a->a::b::c', 'a::b::c->a'], cycles: [['a', 'a::b::c']],
  },
  {
    id: 'C07', title: 'crate path from a child',
    files: {
      'src/lib.rs': 'mod a;\nmod other;\n',
      'src/a.rs': 'mod child;\nuse child::f;\npub fn g() { f() }\n',
      'src/a/child.rs': 'use crate::other::X;\npub fn f() { let _ = X; }\n',
      'src/other.rs': 'pub struct X;\n',
    },
    arcs: ['a->a::child', 'a::child->other'], cycles: [],
  },
  {
    id: 'C09', title: 'pub use child::T re-export',
    files: {
      'src/lib.rs': 'mod parent;\nmod consumer;\n',
      'src/parent.rs': 'mod child;\npub use child::T;\n',
      'src/parent/child.rs': 'pub struct T;\n',
      'src/consumer.rs': 'use crate::parent::T;\npub fn g(_: T) {}\n',
    },
    arcs: ['consumer->parent', 'parent->parent::child'], cycles: [],
  },
  {
    id: 'C10', title: 'glob re-export',
    files: {
      'src/lib.rs': 'mod parent;\nmod consumer;\n',
      'src/parent.rs': 'mod child;\npub use child::*;\n',
      'src/parent/child.rs': 'pub struct T;\n',
      'src/consumer.rs': 'use crate::parent::T;\npub fn g(_: T) {}\n',
    },
    arcs: ['consumer->parent', 'parent->parent::child'], cycles: [],
  },
  {
    id: 'C11', title: 'renamed pub(crate) re-export',
    files: {
      'src/lib.rs': 'mod parent;\nmod consumer;\n',
      'src/parent.rs': 'mod child;\npub(crate) use child::T as Renamed;\n',
      'src/parent/child.rs': 'pub struct T;\n',
      'src/consumer.rs': 'use crate::parent::Renamed;\npub fn g(_: Renamed) {}\n',
    },
    arcs: ['consumer->parent', 'parent->parent::child'], cycles: [],
  },
  {
    id: 'C12', title: 'facade re-export, child uses super',
    files: {
      'src/lib.rs': 'mod parent;\nmod consumer;\n',
      'src/parent.rs': 'mod child;\npub use child::T;\npub struct Helper;\n',
      'src/parent/child.rs': 'use super::Helper;\npub struct T(pub Helper);\n',
      'src/consumer.rs': 'use crate::parent::T;\npub fn g(_: T) {}\n',
    },
    arcs: ['consumer->parent', 'parent->parent::child', 'parent::child->parent'],
    cycles: [['parent', 'parent::child']],
  },
  {
    id: 'C19', title: 'local child named like an external crate',
    deps: '\n[dependencies]\nlog = "0.4"\n',
    files: {
      'src/lib.rs': 'mod a;\nmod b;\n',
      'src/a.rs': 'mod log;\nuse log::info;\npub fn g() { info() }\n',
      'src/a/log.rs': 'pub fn info() {}\n',
      'src/b.rs': 'use log::info;\npub fn g() { info!("x") }\n',
    },
    arcs: ['a->a::log'], cycles: [],
  },
  {
    id: 'C20', title: 'local child named like a crate-root module',
    files: {
      'src/lib.rs': 'mod a;\nmod util;\n',
      'src/util.rs': 'pub fn top() {}\n',
      'src/a.rs': 'mod b;\nmod util;\nuse util::y;\npub fn g() { y() }\n',
      'src/a/util.rs': 'pub fn y() {}\n',
      'src/a/b.rs': 'mod util;\nuse util::x;\npub fn g() { x() }\n',
      'src/a/b/util.rs': 'pub fn x() {}\n',
    },
    arcs: ['a->a::util', 'a::b->a::b::util'], cycles: [],
  },
  {
    id: 'C24', title: 'grouped use with nested self alias',
    files: {
      'src/lib.rs': 'mod a;\n',
      'src/a.rs': 'mod child;\nuse child::{f, sub::{self as subm}};\n'
        + 'pub fn g(_: subm::X) { f() }\n',
      'src/a/child.rs': 'pub mod sub;\npub fn f() {}\n',
      'src/a/child/sub.rs': 'pub struct X;\n',
    },
    arcs: ['a->a::child', 'a->a::child::sub'], cycles: [],
  },
];

const C14_FILES = {
  'src/lib.rs': 'mod a;\n',
  'src/a.rs': 'mod child;\npub struct S;\n#[cfg(all(test, windows))]\nuse child::f;\n'
    + '#[cfg(all(test, windows))]\nmod tests {\n    use super::*;\n    #[test]\n'
    + '    fn t() { f(); }\n}\n',
  'src/a/child.rs': 'use super::S;\npub fn f() { let _ = S; }\n',
};

for (const c of CASES) {
  test(`${c.id} ${c.title}`, () => {
    const res = analyze(c.id, c.files, { deps: c.deps });
    assert.deepEqual(res.arcs, [...c.arcs].sort());
    assert.deepEqual(res.cycles, c.cycles);
  });
}

test('C14 test-gated child use is excluded by default', () => {
  const res = analyze('C14', C14_FILES);
  assert.deepEqual(res.arcs, ['a::child->a']);
  assert.deepEqual(res.cycles, []);
});

test('C14 test-gated child use is included with --include-tests', () => {
  const res = analyze('C14t', C14_FILES, { includeTests: true });
  assert.deepEqual(res.arcs, ['a->a::child', 'a::child->a']);
  assert.deepEqual(res.cycles, [['a', 'a::child']]);
});

// KNOWN LIMITATION tests. They pin current detector output that is NOT true
// Rust semantics, so a change in behavior is noticed. They are not semantic PASS.

test('KNOWN LIMITATION D1: #[path] decoy file closes a false cycle', () => {
  // rustc reads src/imp_x.rs; the detector reads the never-compiled src/a/imp_x.rs.
  const res = analyze('C08b', {
    'src/lib.rs': 'mod a;\n',
    'src/a.rs': '#[path = "imp_x.rs"]\nmod imp;\nuse imp::f;\npub struct S;\npub fn g() { f() }\n',
    'src/imp_x.rs': 'pub fn f() {}\n',
    'src/a/imp_x.rs': 'use super::S;\npub fn f() { let _ = S; }\n',
  });
  assert.deepEqual(res.arcs, ['a->a::imp', 'a::imp->a']); // true Rust: a->a::imp only
  assert.deepEqual(res.cycles, [['a', 'a::imp']]);
});

test('KNOWN LIMITATION D2: import alias named like a crate-root module', () => {
  // `use std::fmt as util; use util::Display;` binds the alias, not crate::util.
  const res = analyze('C21', {
    'src/lib.rs': 'pub mod util;\npub mod a;\n',
    'src/util.rs': 'use crate::a::A;\npub fn x(_: A) {}\n',
    'src/a.rs': 'use std::fmt as util;\nuse util::Display;\npub struct A;\n'
      + 'pub fn g<T: Display>(_: T) {}\n',
  });
  assert.deepEqual(res.arcs, ['a->util', 'util->a']); // true Rust: util->a only
  assert.deepEqual(res.cycles, [['a', 'util']]);
});

test('KNOWN LIMITATION D3: exclusive cfg arcs are unioned into one cycle', () => {
  // The parent arc is windows-only and the child arc unix-only: no single
  // platform has the cycle, but the detector does not evaluate cfg.
  const res = analyze('C18', {
    'src/lib.rs': 'mod a;\n',
    'src/a.rs': 'mod child;\npub struct S;\n#[cfg(windows)]\nuse child::f;\n'
      + '#[cfg(windows)]\npub fn g() { f() }\n',
    'src/a/child.rs': '#[cfg(unix)]\nuse super::S;\npub fn f() {}\n'
      + '#[cfg(unix)]\npub fn h() { let _ = S; }\n',
  });
  assert.deepEqual(res.arcs, ['a->a::child', 'a::child->a']);
  assert.deepEqual(res.cycles, [['a', 'a::child']]);
});
