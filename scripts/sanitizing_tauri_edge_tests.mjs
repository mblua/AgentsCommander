// Gate 06 receiver-precision fixtures (#4017). Registered by sanitizing_tests.mjs, which
// injects its private `trees` and `control` helpers; importing this file registers nothing.
import test from 'node:test';
import assert from 'node:assert/strict';
import { runGate as edgeGate } from './sanitizing_tauri_edge.mjs';
import { createReceiverClassifier } from './sanitizing_tauri_receivers.mjs';
import { scanRust } from './sanitizing_rust_scan.mjs';

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
    const shapes = [
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
    ];
    // AC-4017-001: each positive shape with the port replaced by AppHandle/Emitter counts again.
    const twins = [
      'fn f(host: &tauri::AppHandle) { host.emit("e", 1); }',
      "fn f<'a>(host: &'a tauri::AppHandle) { host.emit_to(\"main\", \"e\", 1); }",
      'use std::sync::Arc;\nfn f(host: Arc<tauri::AppHandle>) { host.emit("e", 1); }',
      'use std::sync::{Arc as Shared, Mutex};\n' +
        'fn f(h: Shared<tauri::AppHandle>) { h.emit("e", 1); }',
      'fn f(host: std::sync::Arc<tauri::AppHandle>) { host.emit("e", 1); }',
      'use tauri::AppHandle as Out;\nfn f(o: &Out) { o.emit("e", 1); }',
      'use tauri::{self as p};\nfn f(o: &p::AppHandle) { o.emit("e", 1); }',
      'async fn f(host: &tauri::AppHandle) { host.emit("e", 1); }',
      'use std::sync::Arc;\nstruct G { host: Arc<tauri::AppHandle> }\nfn f(host: ' +
        'Arc<tauri::AppHandle>) { let g = G { host: Arc::clone(&host) }; host.emit("e", 1); }',
      'use std::sync::Arc;\npub struct E { pub(crate) sink: Arc<tauri::AppHandle>, n: u8 }\n' +
        'impl E { fn tick(&self) { if self.n > 0 { self.sink.emit("e", self.n); } } }',
      use + 'pub(crate) struct W<H: tauri::Emitter> { pub(crate) app: H }\n' +
        'impl<H: tauri::Emitter> Sink for W<H> { fn emit(&self, v: u8) { self.app.emit(v); }\n' +
        '  fn emit_to(&self, l: &str, v: u8) { self.app.emit_to(l, v); } }',
      'struct W<H> { app: H }\nimpl<K> W<K> where K: tauri::Emitter + Send { ' +
        'fn go(&mut self) { self.app.emit("e", 1); } }',
      'trait Local: tauri::Emitter { fn emit(&self, v: u8); }\nfn f(x: &dyn Local) { x.emit(1); }',
      'trait Child: tauri::Emitter + Sync {}\nfn f(x: &dyn Child) { x.emit(1); }',
    ];
    assert.equal(twins.length, shapes.length);
    for (const x of shapes) proven(x);
    // Every replaced call site counts (the wrapper twin has two).
    for (const x of twins) counted(x, {}, x.split('.emit').length - 1);
    // Multi-file declarations: nested modules, super paths and an explicit reexport.
    const deep = { 'lib.rs': 'mod a;\nmod x;\n', 'a.rs': 'pub mod b;\npub use b::Deep;\n',
      'a/b.rs': 'pub trait Deep { fn emit(&self); }\n' };
    const { 'a/b.rs': leaf, ...folder } = deep;
    const real = { 'a/b.rs': 'pub trait Deep: tauri::Emitter { fn emit(&self); }\n' };
    for (const [x, layout] of [
      ['use crate::a::Deep;\nfn f(d: &dyn Deep) { d.emit(); }', deep],
      ['use super::a::b::Deep as D;\nfn f(d: &dyn D) { d.emit(); }', deep],
      ['use crate::a::b::Deep;\nfn f(d: &dyn Deep) { d.emit(); }',
        { ...folder, 'a/b/mod.rs': leaf }],
    ]) {
      proven(x, layout);
      const twin = 'a/b/mod.rs' in layout ? { ...folder, 'a/b/mod.rs': real['a/b.rs'] }
        : { ...deep, ...real };
      counted(x, twin);
    }
    // The quit-gate shape: a later macro use cannot rebind the earlier receiver.
    proven(use + 'fn f(host: &dyn Sink) { host.emit(1); m! { log(host.n()) } }');
    counted('fn f(host: &tauri::AppHandle) { host.emit("e", 1); m! { log(host.n()) } }');
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

  const states = r => r.totals.map(t => t.state_files ?? 0);
  const stateSites = r => r.measurements.head.flatMap(m => (m.sites.state_files ?? [])
    .map(s => ({ path: m.path, line: s.line })));
  const stateCase = (x, expected, extra = {}) => {
    const result = run(crate(x, extra));
    assert.deepEqual(states(result), [expected, expected], x);
    assert.equal(stateSites(result).length, expected, x);
    assert.ok(stateSites(result).every(s => s.path === R + 'x.rs' && s.line > 0), x);
    const withoutCalls = run(crate(x.replace(/.(?:state|try_state)/g, '.unrelated()'), extra));
    assert.deepEqual(result.diagnostics.filter(d => d.status === 'WARN'),
      withoutCalls.diagnostics.filter(d => d.status === 'WARN'),
      'inferred calls add no warning: ' + x);
    return result;
  };

  test('06 inferred state: concrete handles, imports, references and fields', () => {
    const types = ['tauri::AppHandle', 'tauri::App', 'tauri::Window', 'tauri::WebviewWindow',
      'tauri::AppHandle<R>', 'tauri::Window<tauri::Wry>', "&'a mut tauri::AppHandle<R>",
      '&tauri::App', '&mut tauri::Window'];
    for (const type of types) for (const method of ['state', 'try_state'])
      stateCase(`async fn f<'a, R>(app: ${type}) {\n app.${method}( /* empty */ );\n}`, 1);
    for (const x of [
      'use tauri::AppHandle; fn f(app: AppHandle) { app.state(); app.try_state(); }',
      'use tauri::AppHandle as H; fn f(app: H) { app.state(); app.try_state(); }',
      'use tauri::{self as t, AppHandle as H}; fn f(app: H, w: t::Window<R>) ' +
        '{ app.state(); w.try_state(); }',
      'fn r#f(r#app: tauri::AppHandle) { r#app.r#state(); r#app.r#try_state(); }',
      'struct W { app: tauri::AppHandle } impl W { fn f(&self) ' +
        '{ self.app.state(); self.app.try_state(); } }',
      'struct W<R> { app: tauri::AppHandle<R> } impl<R> W<R> { fn f(&self) ' +
        '{ self.app.state(); self.app.try_state(); } }',
    ]) stateCase(x, 2);
    stateCase('fn f(app: tauri::AppHandle) {\n app.state();\n app.try_state();\n}', 2);
    assert.deepEqual(stateSites(run(crate('fn f(app: tauri::AppHandle) {\n' +
      ' app.state();\n app.try_state();\n}'))).map(s => s.line), [2, 3]);
  });

  test('06 inferred state: unknown and unsupported twins are silent and excluded', () => {
    const types = ['X', 'tauri::Emitter', 'tauri::State<X>', 'tauri::Unknown<R>',
      'tauri::x::AppHandle', 'dyn tauri::Manager', 'Arc<tauri::AppHandle>',
      'Box<tauri::AppHandle>', 'Rc<tauri::AppHandle>', 'tauri::AppHandle<>',
      'tauri::AppHandle<R, S>', 'tauri::AppHandle<Vec<R>>', 'tauri::AppHandle<R + Send>',
      'tauri::AppHandle<Item = R>', 'tauri::AppHandle<3>', 'tauri::AppHandle<(R,)>'];
    for (const type of types)
      stateCase(`fn f(app: ${type}) { app.state(); app.try_state(); }`, 0);
    for (const x of [
      'struct AppHandle; fn f(app: AppHandle) { app.state(); app.try_state(); }',
      'mod tauri {} fn f(app: tauri::AppHandle) { app.state(); }',
      'type H = tauri::AppHandle; fn f(app: H) { app.state(); }',
      'use tauri::*; fn f(app: AppHandle) { app.state(); }',
      'use tauri::AppHandle as H; use other::H; fn f(app: H) { app.state(); }',
      'fn f<H: tauri::Manager>(app: H) { app.state(); }',
      'fn f(app: tauri::AppHandle) { let app = x; app.state(); }',
      'fn f(app: tauri::AppHandle) { app = x; app.state(); }',
      'fn f(app: tauri::AppHandle) { app.state(); let app = x; }',
      'fn f(app: tauri::AppHandle) { app.clone().state(); }',
      'fn f(app: tauri::AppHandle) { (app).state(); }',
      'fn f(app: tauri::AppHandle) { let f = || { app.state(); }; }',
      'fn f(app: tauri::AppHandle) { let f = async { app.state(); }; }',
      'fn f(app: tauri::AppHandle) { m!(app.state()); }',
      'fn f(app: tauri::AppHandle) { m!(app); app.state(); }',
      'fn f(app: tauri::AppHandle) { app.state(1); app.try_state(x); }',
      'fn f(app: tauri::AppHandle) { app.state; app.try_state; }',
      'fn f(app: tauri::AppHandle) { tauri::Manager::state(app); }',
      'struct W<H: tauri::Manager> { app: H } impl<H: tauri::Manager> W<H> ' +
        '{ fn f(&self) { self.app.state(); } }',
      'struct W { app: tauri::AppHandle } impl W { fn f(&self) ' +
        '{ self.app = x; self.app.state(); } }',
      'struct W { app: tauri::AppHandle } impl W { fn f(&self) { self.app.x.state(); } }',
      'struct W { app: tauri::AppHandle } impl W { fn f(&self) { self.state(); } }',
      'struct Identity; impl Identity { fn state(&self) {} fn try_state(&self) {} } ' +
        'fn f(path_identity: Identity) { path_identity.state(); path_identity.try_state(); }',
    ]) stateCase(x, 0);
    for (const root of ['#[cfg(unix)] mod x;', '#[path="x.rs"] mod x;'])
      stateCase('fn f(app: tauri::AppHandle) { app.state(); }', 0,
        { 'lib.rs': root });
  });

  test('06 inferred state: explicit history, filtering and exact boundaries', () => {
    stateCase('fn f(x: X) { x.state::<X>(); x.try_state::<X>(); }', 2);
    stateCase('fn f(app: tauri::AppHandle) { app.state::<X>(); app.state(); ' +
      'app.try_state::<X>(); app.try_state(); }', 4);
    stateCase('fn f(app: tauri::AppHandle) { /* app.state(); */ ' +
      'let s = "app.state()"; let r = r###"app.try_state()"###; }', 0);
    stateCase('#[cfg(test)] mod t { fn f(app: tauri::AppHandle) { app.state(); } } ' +
      '#[test] fn f(app: tauri::AppHandle) { app.try_state(); }', 0);
    for (const p of ['lib.rs', 'main.rs', 'commands.rs', 'events.rs',
      'commands/a.rs', 'commands/a/b.rs', 'events/a.rs', 'events/a/b.rs']) {
      const result = run(files({[p]: 'fn f(app: tauri::AppHandle) { app.state(); }'}));
      assert.deepEqual(states(result), [0, 0], p);
    }
    for (const [p, layout] of [
      ['app_run.rs', {'lib.rs':'mod app_run;'}],
      ['app_run/setup.rs', {'lib.rs':'mod app_run;', 'app_run.rs':'mod setup;'}],
      ['app_run/a/b.rs', {'lib.rs':'mod app_run;', 'app_run.rs':'mod a;',
        'app_run/a.rs':'mod b;'}],
    ]) {
      const r = run(files({...layout, [p]:'fn f(app: tauri::AppHandle) { app.state(); }'}));
      assert.deepEqual(states(r), [1, 1], p);
      assert.deepEqual(stateSites(r), [{path:R+p,line:1}]);
    }
    const tests = files({'lib.rs':'mod x;', 'x.rs':
      '#[test] fn f(app: tauri::AppHandle) { app.state(); }'});
    const added = run({}, tests);
    assert.deepEqual(states(added), [0, 0]);
    assert.equal(added.exit, 1, 'new core test-only Tauri still fails');
  });

  test('06 inferred state: shared ratchet and snapshot isolation', () => {
    const make = (type, body = '') => crate(`fn f(app: ${type}) {\n${body}\n}`);
    for (const method of ['state', 'try_state']) {
      const base = make('tauri::AppHandle'), head = make('tauri::AppHandle', `app.${method}();`);
      assert.equal(run(base).exit, 0);
      const r = run(base, head);
      assert.equal(r.exit, 1);
      assert.deepEqual(states(r), [0, 1]);
      assert.ok(r.diagnostics.some(d => d.status === 'FAIL' && d.rule === 'state_files' &&
        d.path === R+'x.rs' && d.line === 2 && d.remedy));
      assert.equal(run(base, base).exit, 0);
      assert.equal(run(head, base).exit, 0);
    }
    const one = make('tauri::AppHandle', 'app.state();');
    const alternate = make('tauri::AppHandle', 'app.try_state();');
    assert.deepEqual(states(run(one, alternate)), [1, 1]);
    assert.equal(run(one, alternate).exit, 0);
    assert.deepEqual(states(run(one, make('tauri::AppHandle',
      'app.state::<X>(); app.try_state();'))), [1, 2]);
    const added = run(one, {...one, ...files({'lib.rs':'mod ports; mod x; mod y;',
      'y.rs':'fn f(app: tauri::AppHandle) { app.state(); }'})});
    assert.deepEqual(states(added), [1, 2]);
    assert.equal(added.exit, 1);
    const pairs = [
      [make('X','app.state();'), one],
      [crate('use other::AppHandle as H; fn f(app: H) { app.state(); }'),
        crate('use tauri::AppHandle as H; fn f(app: H) { app.state(); }')],
      [crate('struct W { app: X } impl W { fn f(&self) { self.app.state(); } }'),
        crate('struct W { app: tauri::AppHandle } impl W { fn f(&self) { self.app.state(); } }')],
    ];
    for (const [base, head] of pairs) {
      assert.deepEqual(states(run(base, head)), [0, 1]);
      assert.deepEqual(states(run(head, base)), [1, 0]);
      assert.deepEqual(states(run(base, head)),
 [0, 1]);
    }
  });

  test('06 shared receiver mechanics: emits preserve real, port and warning contracts', () => {
    const r = run(crate(use + 'fn f(port: &dyn Sink, app: tauri::AppHandle, x: X) {\n' +
      'port.emit(1);\napp.emit("e", 1);\nx.emit(1);\n}'));
    assert.equal(emits(r), 2);
    assert.deepEqual(r.measurements.head.find(m => m.path === R+'x.rs').sites.emit_files
      .map(s => s.line), [4, 5]);
    const warnings = r.diagnostics.filter(d => d.status === 'WARN');
    assert.equal(warnings.length, 1);
    assert.equal(warnings[0].line, 5);
    assert.equal(warnings[0].reason, 'emit receiver not proved: unresolved type path: X');
  });

  // #4201: classifier over a fixture crate with fresh scans; optional throw on a file's first scan.
  const classifyEmit = (files, throwOnce = null) => {
    let calls = 0;
    const tree = new Map(Object.keys(files).map(p => [p, { path: p }]));
    const scan = p => {
      if (p === throwOnce && calls++ === 0) throw new Error('4201 unexpected scan');
      return scanRust(p, files[p]);
    };
    const classifier = createReceiverClassifier({ tree, scan });
    const x = scanRust(R + 'x.rs', files[R + 'x.rs']);
    const dot = x.tokens.find((t, i) => t.text === '.' && x.tokens[i + 1]?.name === 'emit');
    return () => classifier.classify(R + 'x.rs', x, dot);
  };
  const warns = result => result.diagnostics.filter(d => d.status === 'WARN').map(d => d.reason);

  test('06 4201 OBJECT_REAL_FILTER: generic Tauri wrapper REAL, plain unknown path UNKNOWN', () => {
    const generic = use + 'fn f(app: tauri::Odd<R>) { app.emit(1); }';
    const plain = use + 'fn f(app: tauri::Odd) { app.emit(1); }';
    assert.deepEqual(classifyEmit(crate(generic))(),
      { kind: 'REAL', reason: 'Tauri receiver tauri::Odd' });
    assert.deepEqual(classifyEmit(crate(plain))(),
      { kind: 'UNKNOWN', reason: 'emit receiver not proved: unsupported receiver type' });
    assert.deepEqual(warns(counted(generic)), []);
    assert.deepEqual(warns(counted(plain)),
      ['emit receiver not proved: unsupported receiver type']);
  });

  test('06 4201 FIELD_BINDER_POSITION: field binder uses its own impl argument position', () => {
    const x = use + 'struct W<A: tauri::Emitter, B: Sink> { app: B }\n' +
      'impl<X: tauri::Emitter, Y: Sink> W<X,Y> { fn go(&self) { self.app.emit(1); } }';
    const proof = classifyEmit(crate(x))();
    assert.equal(proof.kind, 'PROVEN_PORT');
    assert.equal(proof.reason, 'port trait Sink::emit');
    assert.deepEqual(proof.evidence.declaration, { path: R + 'x.rs', line: 2 });
    assert.deepEqual(proof.evidence.trait, { path: R + 'ports.rs', line: 1, name: 'Sink' });
    const result = run({}, crate(x));
    assert.equal(emits(result), 0);
    assert.equal(result.measurements.head.find(m => m.path === R + 'x.rs').sites.emit_files,
      undefined);
    assert.deepEqual(warns(result), []);
  });

  test('06 4201 CAPTURE_OUTER_GROUP: only the innermost enclosing group bounds a closure', () => {
    const inner = use + 'fn f(host: &dyn Sink) { consume((|| 1)); host.emit(1); }';
    assert.equal(classifyEmit(crate(inner))().kind, 'PROVEN_PORT');
    assert.deepEqual(warns(proven(inner)), []);
    const captured = 'emit receiver not proved: receiver captured by closure or async block';
    for (const x of [use + 'fn f(host: &dyn Sink) { consume((|| (host.emit(1)))); }',
      use + 'fn f(host: &dyn Sink) { consume((|| 1)); async move { host.emit(1); }; }']) {
      assert.deepEqual(classifyEmit(crate(x))(), { kind: 'UNKNOWN', reason: captured });
      const result = counted(x);
      assert.deepEqual(warns(result), [captured]);
      assert.deepEqual(result.measurements.head.find(m => m.path === R + 'x.rs').sites.emit_files
        .map(site => site.line), [2]);
    }
  });

  test('06 4201 COMPETITION_SWALLOW_ALL: unexpected import resolution errors propagate', () => {
    const x = use + 'use crate::q::Q;\nfn f(host: &dyn Sink) { host.emit(1); }';
    const lib = 'mod ports;\nmod x;\nmod q;\n';
    const q = text => files({ 'lib.rs': lib, 'ports.rs': PORTS, 'x.rs': x, 'q.rs': text });
    const declared = q('pub trait Q { fn unrelated(&self); }\n');
    assert.throws(classifyEmit(declared, R + 'q.rs'),
      e => e.constructor === Error && e.message === '4201 unexpected scan');
    const recovered = classifyEmit(q(''))();
    assert.equal(recovered.kind, 'PROVEN_PORT');
    assert.equal(recovered.reason, 'port trait Sink::emit');
    assert.equal(classifyEmit(declared)().reason, 'port trait Sink::emit');
  });

  test('06 measure characterization: every rule, edge file, test-only Tauri', () => {
    const head = files({
      'lib.rs': 'mod ports;\nmod x;\nmod commands;\n',
      'ports.rs': PORTS,
      'x.rs': 'use tauri::AppHandle;\nuse tauri::Manager;\n' +
        'fn a(h: AppHandle) { h.emit("e", 1); let _s = h.state::<u8>(); }\n' +
        'struct S;\nimpl S { #[tauri::command] fn c(&self) { std::fs::read("a"); } }\n' +
        '#[tauri::command] fn d() {}\n' +
        '#[cfg(test)] mod t { fn z(x: tauri::AppHandle) {} }\n',
      'commands.rs': '#[tauri::command] fn e() { std::fs::read("b"); }\n',
    });
    const X = R + 'x.rs', C = R + 'commands.rs';
    const EXPECTED = {
      exit: 1,
      totals: [{}, { tauri_files: 4, emit_files: 1, state_files: 1, command_files: 2,
        disk_commands: 2 }],
      diagnostics: [
        ['FAIL', 'tauri_files', X, 3], ['FAIL', 'emit_files', X, 3],
        ['FAIL', 'state_files', X, 3], ['FAIL', 'command_files', X, 5],
        ['FAIL', 'disk_commands', C, 1], ['FAIL', 'new_disk_command', C, 1],
        ['FAIL', 'tauri_files', X, 3], ['FAIL', 'tauri_files', X, 7],
        ['FAIL', 'new_disk_command', X, 5],
      ],
    };
    const r = edgeGate(trees({}, head));
    const shape = { exit: r.exit, totals: r.totals,
      diagnostics: r.diagnostics.map(d => [d.status, d.rule, d.path, d.line]) };
    assert.deepEqual(shape, EXPECTED);
  });
}
