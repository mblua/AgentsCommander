// Gate 06 receiver-precision fixtures (#4017). Registered by sanitizing_tests.mjs, which
// injects its private `trees` and `control` helpers; importing this file registers nothing.
import test from 'node:test';
import assert from 'node:assert/strict';
import { runGate as edgeGate } from './sanitizing_tauri_edge.mjs';

const R = 'src-tauri/src/';
const PORTS = 'pub trait Sink: Send + Sync + \'static {\n' +
  '    fn emit(&self, v: u8);\n    fn emit_to(&self, label: &str, v: u8);\n}\n';
const files = map => Object.fromEntries(Object.entries(map).map(([p, text]) => [R + p, text]));
// A crate whose `x.rs` is reached through `lib.rs`, with the port trait in `ports.rs`.
const crate = (x, extra = {}) => files({ 'lib.rs': 'mod ports;\nmod x;\n', 'ports.rs': PORTS,
  'x.rs': x, ...extra });

export function registerTauriEdgeTests({ trees, control }) {
  const run = (base, head = base) => {
    const result = edgeGate(trees(base, head));
    assert.notEqual(result.exit, 3, JSON.stringify(result));
    return result;
  };
  const emits = result => result.totals[1].emit_files;
  const proven = (x, extra) => {
    const result = run({}, crate(x, extra));
    assert.equal(emits(result), 0, x + '\n' + JSON.stringify(result.diagnostics));
    assert.equal(result.exit, 0, x);
    return result;
  };
  const counted = (x, extra, n = 1) => {
    const result = run({}, crate(x, extra));
    assert.equal(emits(result), n, x);
    assert.equal(result.exit, 1, x);
    assert.ok(result.diagnostics.some(d => d.status === 'FAIL' && d.rule === 'emit_files' &&
      d.path.startsWith(R) && d.line > 0 && d.remedy), x);
    return result;
  };
  const use = 'use crate::ports::Sink;\n';

  test('06 receiver proof: parameters, fields, generics, imports and supertraits', () => {
    for (const x of [
      use + 'fn f(host: &dyn Sink) { host.emit(1); }',
      use + "fn f<'a>(host: &'a dyn Sink) { host.emit_to(\"main\", 1); }",
      use + 'use std::sync::Arc;\nfn f(host: Arc<dyn Sink + Send + Sync>) { host.emit(1); }',
      use + 'use std::sync::{Arc as Shared, Mutex};\nfn f(h: Shared<dyn Sink>) { h.emit(1); }',
      'fn f(host: std::sync::Arc<dyn crate::ports::Sink>) { host.emit(1); }',
      'use crate::ports::Sink as Out;\nfn f(o: &dyn Out) { o.emit(1); }',
      'use crate::ports::{self as p};\nfn f(o: &dyn p::Sink) { o.emit(1); }',
      use + 'async fn f(host: &dyn Sink) { host.emit(1); }',
      use + 'use std::sync::Arc;\nstruct G { host: Arc<dyn Sink> }\n' +
        'fn f(host: Arc<dyn Sink>) { let g = G { host: Arc::clone(&host) }; host.emit(1); }',
      use + 'use std::sync::Arc;\npub struct E { pub(crate) sink: Arc<dyn Sink>, n: u8 }\n' +
        'impl E { fn tick(&self) { if self.n > 0 { self.sink.emit(self.n); } } }',
      use + 'pub(crate) struct W<H: Sink> { pub(crate) app: H }\n' +
        'impl<H: Sink> Sink for W<H> { fn emit(&self, v: u8) { self.app.emit(v); }\n' +
        '  fn emit_to(&self, l: &str, v: u8) { self.app.emit_to(l, v); } }',
      use + 'struct W<H> { app: H }\nimpl<K> W<K> where K: Sink + Send { ' +
        'fn go(&mut self) { self.app.emit(1); } }',
      'trait Local: Send { fn emit(&self, v: u8); }\nfn f(x: &dyn Local) { x.emit(1); }',
      'use crate::ports::Sink;\ntrait Child: Sink + Sync {}\nfn f(x: &dyn Child) { x.emit(1); }',
    ]) proven(x);
    // Multi-file declarations: nested modules, super paths and an explicit reexport.
    const deep = { 'lib.rs': 'mod a;\nmod x;\n', 'a.rs': 'pub mod b;\npub use b::Deep;\n',
      'a/b.rs': 'pub trait Deep { fn emit(&self); }\n' };
    proven('use crate::a::Deep;\nfn f(d: &dyn Deep) { d.emit(); }', deep);
    proven('use super::a::b::Deep as D;\nfn f(d: &dyn D) { d.emit(); }', deep);
    const { 'a/b.rs': leaf, ...folder } = deep;
    proven('use crate::a::b::Deep;\nfn f(d: &dyn Deep) { d.emit(); }',
      { ...folder, 'a/b/mod.rs': leaf });
    // The quit-gate shape: a later macro use cannot rebind the earlier receiver.
    proven(use + 'fn f(host: &dyn Sink) { host.emit(1); m! { log(host.n()) } }');
    const late = run({}, crate(use + 'fn f(host: &dyn Sink) {\n  host.emit(1);\n' +
      '  m! { host.emit(2) }\n}'));
    assert.deepEqual(late.measurements.head.find(m => m.path === R + 'x.rs').sites.emit_files
      .map(site => site.line), [4]);
  });

  test('06 receiver proof: mixed files count only real calls; Tauri replacements count', () => {
    const mixed = use + 'fn f(host: &dyn Sink, app: tauri::AppHandle) {\n' +
      '  host.emit(1);\n  app.emit("e", 1);\n}';
    const r = run(crate(mixed));
    assert.equal(emits(r), 1);
    assert.deepEqual(r.measurements.head.find(m => m.path === R + 'x.rs').sites.emit_files
      .map(s => s.line), [4]);
    assert.ok(!r.diagnostics.some(d => d.status === 'WARN' && d.line === 4), 'REAL is not WARN');
    for (const x of [
      'fn f(host: &tauri::AppHandle) { host.emit("e", 1); }',
      'use tauri::AppHandle as H;\nfn f(host: H) { host.emit("e", 1); }',
      'use tauri::Emitter;\nstruct W<H: Emitter> { app: H }\n' +
        'impl<H: Emitter> W<H> { fn go(&self) { self.app.emit("e", 1); } }',
      'use std::sync::Arc;\nfn f(w: Arc<tauri::WebviewWindow>) { w.emit_to("m", "e", 1); }',
    ]) counted(x);
  });

  test('06 receiver proof: real emits cannot hide behind proven ports', () => {
    const port = use + 'fn f(host: &dyn Sink) { host.emit(1); }\n';
    const real = 'fn g(app: tauri::AppHandle) { app.emit("e", 1); }\n';
    // Uncompensated real addition to an existing file: PASS -> FAIL with diagnostic -> PASS.
    assert.equal(run(crate(port)).exit, 0);
    const grown = run(crate(port), crate(port + real));
    assert.equal(grown.exit, 1);
    assert.ok(grown.diagnostics.some(d => d.status === 'FAIL' && d.rule === 'emit_files' &&
      d.path === R + 'x.rs' && d.line === 3 && d.remedy), JSON.stringify(grown.diagnostics));
    assert.equal(run(crate(port), crate(port)).exit, 0);
    // A new file with a real emit fails the global ratchet as well.
    const added = run(crate(port), crate(port, { 'lib.rs': 'mod ports;\nmod x;\nmod y;\n',
      'y.rs': real }));
    assert.equal(added.exit, 1);
    assert.ok(added.diagnostics.some(d => d.rule === 'emit_files' && d.path === R + 'y.rs'));
    // Existing limitation, unchanged: compensated real moves pass, sites stay measured.
    const base = crate(port + real), head = crate(port + real.replace('fn g', 'fn h'));
    const moved = run(base, head);
    assert.equal(moved.exit, 0);
    assert.equal(moved.totals[0].emit_files, 1);
    assert.equal(emits(moved), 1);
    // Same names, changed trait in head: proof is recomputed per side.
    const changed = run(crate(port), crate(port, { 'ports.rs': PORTS.replace('Send + Sync',
      'tauri::Emitter') }));
    assert.equal(changed.totals[0].emit_files, 0);
    assert.equal(emits(changed), 1);
    assert.equal(changed.exit, 1);
    // A proven port turned real is detected through the existing counter.
    control(edgeGate, 'emit_files', 'x.rs', 'fn f() {}',
      'fn f(app: tauri::AppHandle) { app.emit("e", 1); }');
  });

  test('06 receiver proof: unproved receivers stay counted and warn', () => {
    const cases = [
      ['fn f(x: X) { x.emit(1); }', {}, 'unresolved'],
      [use + 'fn f(x: Box<dyn Sink>) { x.emit(1); }', {}, 'unresolved type path: Box'],
      [use + 'type S = std::sync::Arc<dyn Sink>;\nfn f(x: S) { x.emit(1); }', {}, ''],
      [use + 'use std::rc::Rc;\nfn f(x: Rc<dyn Sink>) { x.emit(1); }', {}, ''],
      ['use crate::ports::*;\nfn f(x: &dyn Sink) { x.emit(1); }', {}, 'glob'],
      [use + 'fn f(x: &dyn Sink) { x.emit(1); }',
        { 'lib.rs': 'mod ports;\n#[cfg(unix)]\nmod x;\n' }, 'attribute'],
      [use + 'fn f(x: &dyn Sink) { x.emit(1); }', { 'ports.rs': PORTS + PORTS }, ''],
      ['use crate::a::P;\nfn f(x: &dyn P) { x.emit(1); }',
        { 'lib.rs': 'mod a;\nmod b;\nmod x;\n', 'a.rs': 'pub use crate::b::P;\n',
          'b.rs': 'pub use crate::a::P;\n' }, 'cyclic'],
      [use + 'fn f(x: &dyn Sink) { x.emit(1); }',
        { 'ports.rs': PORTS.replace('Send + Sync', 'tauri::Emitter') }, 'Tauri'],
      [use + 'fn f(x: &dyn Sink) { x.emit(1); }',
        { 'ports.rs': PORTS.replace("'static {", "'static where Self: Sized {") }, 'where'],
      [use + 'trait Two { fn emit(&self, v: u8); }\ntrait Both: Sink + Two {}\n' +
        'fn f(x: &dyn Both) { x.emit(1); }', {}, 'conflicting'],
      [use + 'struct Arc<T>(T);\nfn f(x: Arc<dyn Sink>) { x.emit(1); }', {}, ''],
      [use + 'trait Send {}\nfn f(x: &(dyn Sink + Send)) { x.emit(1); }', {}, ''],
      [use + 'trait Send {}\nfn f(x: &dyn Sink + Send) { x.emit(1); }', {}, ''],
      [use + 'fn f(x: &dyn Sink) { m!(x.emit(1)); }', {}, 'macro'],
      [use + 'fn f(x: &dyn Sink) { m!(x); x.emit(1); }', {}, 'macro'],
      [use + 'fn f(x: &dyn Sink) { let g = || x.emit(1); }', {}, 'whole expression'],
      [use + 'fn f(x: &dyn Sink) { let g = move |v| { x.emit(v) }; }', {}, 'captured'],
      [use + 'fn f(x: &dyn Sink) { let y = async { x.emit(1) }; }', {}, 'captured'],
      [use + 'fn f(x: &dyn Sink, y: &dyn Sink) { let x = y; x.emit(1); }', {}, 'pattern'],
      [use + 'fn f(x: &dyn Sink) { x.emit(1); let x = 2; }', {}, 'pattern'],
      [use + 'fn f(x: &dyn Sink, v: Option<u8>) { match v { Some(x) => {}, _ => {} }' +
        ' x.emit(1); }', {}, 'pattern'],
      [use + 'fn f(x: &dyn Sink, v: Vec<u8>) { for x in v {} x.emit(1); }', {}, 'pattern'],
      [use + 'fn f(mut x: &dyn Sink, y: &dyn Sink) { x = y; x.emit(1); }', {}, 'parameter'],
      [use + 'fn f(x: &dyn Sink) { fn g(x: u8) {} x.emit(1); }', {}, 'nested'],
      [use + 'fn f(x: &dyn Sink) { x.clone().emit(1); }', {}, ''],
      [use + 'fn f(x: &dyn Sink) { (x).emit(1); }', {}, ''],
      [use + 'fn f(x: &dyn Sink) { *x.emit(1); }', {}, 'whole expression'],
      [use + 'struct E { a: A }\nimpl E { fn f(&self) { self.a.b.emit(1); } }', {}, ''],
      [use + 'impl dyn Sink { fn emit(&self, v: u8) {} }\nfn f(x: &dyn Sink) { x.emit(1); }',
        {}, 'dyn port'],
      [use + 'trait Q { fn emit(&self, v: u8); }\nimpl<T> Q for T {}\n' +
        'fn f(x: &dyn Sink) { x.emit(1); }', {}, 'competing'],
      [use + 'use crate::q::Q;\nfn f(x: &dyn Sink) { x.emit(1); }',
        { 'lib.rs': 'mod ports;\nmod q;\nmod x;\n',
          'q.rs': 'pub trait Q { fn emit(&self, v: u8); }\nimpl<T: ?Sized> Q for Box<T> {}\n' },
        'competing'],
      [use + 'use tauri::Emitter;\nfn f(x: &dyn Sink) { x.emit(1); }', {}, 'Tauri'],
      [use + 'struct W<H> { app: H }\nimpl<H> W<H> { fn go(&self) { self.app.emit(1); } }',
        {}, 'no port'],
      [use + 'struct W<H: Sink> { app: H }\nimpl<K: Sink> W<u8> { fn go(&self) ' +
        '{ self.app.emit(1); } }', {}, 'impl generic'],
      [use + 'struct W<H: Sink> { app: H }\nimpl<H: Sink> W<H> { fn go(&self) ' +
        '{ self.app = todo(); self.app.emit(1); } }', {}, 'written'],
      [use + 'fn f(x: &dyn Sink) { x.emit(1); }',
        { 'lib.rs': 'mod ports;\nmod x;\nmacro_rules! m { () => {} }\n',
          'ports.rs': PORTS + 'm!();\n' }, 'macro'],
      [use + '#[cfg(unix)]\nfn f(x: &dyn Sink) { x.emit(1); }',
        { 'ports.rs': '#[my_attr]\n' + PORTS }, 'attribute'],
      [use + 'fn f(x: &dyn Sink) { x.emit(1); }',
        { 'ports.rs': PORTS.replace('&self, v', 'self, v') }, 'self'],
    ];
    for (const [x, extra, reason] of cases) {
      const result = counted(x, extra);
      const warn = result.diagnostics.find(d => d.status === 'WARN' && d.path === R + 'x.rs' &&
        /emit receiver not proved/.test(d.reason));
      assert.ok(warn && warn.line > 0 && warn.remedy, x);
      assert.ok(warn.reason.includes(reason), x + ' -> ' + warn.reason);
    }
  });

  test('06 receiver proof: other counters, edges and app_run stay unchanged', () => {
    const port = use + 'fn f(host: &dyn Sink) { host.emit(1); }\n';
    const r = run({}, crate(port + 'fn s(x: X) { x.state::<X>(); x.try_state::<X>(); }\n' +
      '#[tauri::command] fn c() { std::fs::read("a"); }\n'));
    assert.deepEqual(r.totals[1], { tauri_files: 1, emit_files: 0, state_files: 2,
      command_files: 1, disk_commands: 1 });
    // Exact edges stay exempt without any receiver proof.
    for (const p of ['lib.rs', 'commands/a.rs', 'events.rs'])
      assert.equal(emits(run({}, files({ [p]: 'fn f(x: X) { x.emit("a", 0); }' }))), 0, p);
    // app_run and other core folders stay counted, whatever the receiver.
    for (const p of ['app_run/setup.rs', 'app_run/extra.rs', 'app_run/a/b.rs', 'window.rs'])
      control(edgeGate, 'emit_files', p, 'fn f() {}', 'fn f(app: X) { app.emit("a", 0); }');
    // The real selection/setup receiver shapes stay measured.
    counted('fn f(app: &tauri::AppHandle, v: u8) { if let Err(e) = app.emit("e", v) {} }');
    counted('fn f(app: tauri::AppHandle) { let a = app.clone(); ' +
      'spawn(move || { let _ = a.emit("e", 1); }); }');
  });
}
