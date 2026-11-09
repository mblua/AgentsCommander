import test from 'node:test';
import assert from 'node:assert/strict';
import { runGate as layoutGate } from './sanitizing_file_layout.mjs';
import { runGate as edgeGate } from './sanitizing_tauri_edge.mjs';
import { runGate as globalsGate } from './sanitizing_global_state.mjs';
import { runGate as processGate } from './sanitizing_process_env.mjs';
import { applyAllowlist, formatWarning } from './sanitizing_baseline.mjs';
import { runGate as containersGate, parseRegistry,
    measureSource } from './sanitizing_containers.mjs';
import { runGate as diskGate } from './sanitizing_disk_access.mjs';

// Batch 7: occurrence totals are invariant under splits, with one-use transfer credit.
export function registerSnapshotTransferTests({trees, rustRoot, allow}) {
  test('06/08/09 splits preserve debt; extra occurrences in existing/new files fail', () => {
    const p=rustRoot+'old.rs', q=rustRoot+'new.rs', r=rustRoot+'copy.rs';
    for (const [gate,rule,a,b,extra] of [
      [edgeGate,'tauri_files','fn a(x: tauri::AppHandle) {}','fn b(x: tauri::AppHandle) {}',
          'fn c(x: tauri::AppHandle) {}'],
      [edgeGate,'emit_files','fn a(x: X) { x.emit("a",0); }','fn b(x: X) { x.emit_to("b",0); }',
          'fn c(x: X) { x.emit("c",0); }'],
      [edgeGate,'state_files','fn a(x: X) { x.state::<X>(); }','fn b(x: X) { x.state::<X>(); }',
          'fn c(x: X) { x.state::<X>(); }'],
      [edgeGate,'state_files','fn a(x: X) { x.try_state::<X>(); }',
          'fn b(x: X) { x.try_state::<X>(); }','fn c(x: X) { x.try_state::<X>(); }'],
      [edgeGate,'command_files','#[tauri::command] fn a() {}','#[tauri::command] fn b() {}',
          '#[tauri::command] fn c() {}'],
      [globalsGate,'globals','static A: Mutex<u8> = init();','static B: Mutex<u8> = init();',
          'static C: Mutex<u8> = init();'],
      [processGate,'launch_files','fn a() { Command::new("a"); }','fn b() { Command::new("b"); }',
          'fn c() { Command::new("c"); }'],
      [processGate,'env_read_files','fn a() { env::var("a"); }','fn b() { env::var_os("b"); }',
          'fn c() { env::var("c"); }'],
    ]) {
      const base={[p]:a+'\n'+b};
      const split=gate(trees(base,{[p]:a,[q]:b}));
      assert.equal(split.exit,0,JSON.stringify(split));
      assert.equal(split.totals[0][rule],2); assert.equal(split.totals[1][rule],2);
      for (const head of [{[p]:a+extra,[q]:b},{[p]:a,[q]:b+extra},{[p]:a,[q]:b,[r]:b}]) {
        const bad=gate(trees(base,head)); assert.equal(bad.exit,1,JSON.stringify(bad));
        assert.ok(bad.diagnostics.some(d=>d.rule===rule && d.status==='FAIL'));
      }
      const compensated=gate(trees(base,{[p]:a,[q]:extra}));
      // Strict new-file rules require an actual match, even with equal totals.
      if (['tauri_files','globals','launch_files','env_read_files'].includes(rule))
        assert.equal(compensated.exit,1,JSON.stringify(compensated));
    }
  });
  test('06 disk commands may move unchanged, but new identities/copies still fail', () => {
    const p=rustRoot+'commands.rs',q=rustRoot+'commands/disk.rs';
    const a='#[tauri::command] fn a() { fs::read("a"); }', b=
        '#[tauri::command] fn b() { fs::read("b"); }';
    const base={[p]:a+b};
    assert.equal(edgeGate(trees(base,{[p]:a,[q]:b})).exit,0);
    for (const head of [{[p]:a,[q]:b.replace('fn b()', 'fn fresh()')},{[p]:a+b,[q]:b}])
      assert.equal(edgeGate(trees(base,head)).exit,1);
  });
  test('transfer matching reserves retained debt before spending removed occurrences', () => {
    const p=rustRoot+'a.rs',q=rustRoot+'b.rs',r=rustRoot+'c.rs',code=
        'static A: Mutex<u8> = init();';
    const result=globalsGate(trees({[p]:code,[q]:code},{[r]:code,[q]:code}));
    assert.equal(result.exit,0,JSON.stringify(result));
    assert.equal(globalsGate(trees({[p]:code,[q]:code},{[r]:code+code,[q]:code})).exit,1);
  });
  test('moved aliases require matching bindings and production ownership', () => {
    const p=rustRoot+'a.rs',q=rustRoot+'b.rs',code='fn f(){ C::new("a"); }';
    const imports='use std::process::Command as C;';
    assert.equal(processGate(trees({[p]:imports+code},{[q]:imports+'\n// moved\n'+code})).exit,0);
    assert.equal(processGate(trees({[p]:imports+'#[cfg(test)] '+code},{[q]:imports+code})).exit,1);
    assert.equal(processGate(trees({[rustRoot+'process.rs']:imports+code},{[q]:imports+code})).exit,
        1);
    assert.equal(processGate(trees({[p]:imports+code},{[q]:'use tokio::process::Command as C;'+
        code})).exit,1);
  });
  test('occurrence increases still apply path-scoped allowlist without hiding unlisted debt', () =>
      {
    const p=rustRoot+'a.rs',q=rustRoot+'b.rs',code='fn f(){ env::var("a"); }';
    const result=processGate(trees({[p]:code},{[p]:code+code}));
    assert.equal(result.exit,1);
    assert.equal(applyAllowlist(result,[allow('09','env_read_files',p)]).exit,0);
    const unlisted=processGate(trees({[p]:code},{[p]:code,[q]:code}));
    assert.equal(applyAllowlist(unlisted,[allow('09','env_read_files',p)]).exit,1);
  });
}

export function registerSnapshotBoundaryTests({trees, control, fixture, rustRoot, allow}) {
  test('05 formatting real glob and stale warnings preserves PASS and FAIL exits', () => {
    const p=rustRoot+'glob.rs',source='use unknown::*; #[test] fn f() {}';
    for (const base of [{}, {[p]:source}]) {
      const raw=layoutGate(trees(base,{[p]:source}));
      const result=applyAllowlist(raw,[allow('05','inline_mod',p)]);
      assert.equal(result.exit,raw.exit);
      assert.equal(result.exit,Object.keys(base).length ? 0 : 1);
      const ambiguity=result.diagnostics.find(d=>d.status==='WARN' &&
          d.reason==='glob import may bind path');
      assert.ok(ambiguity);
      assert.match(formatWarning(ambiguity),/^WARN scan ambiguity:/);
      assert.doesNotMatch(formatWarning(ambiguity),/stale allowlist/);
      const stale=result.diagnostics.find(d=>d.rule==='stale_allowlist');
      assert.ok(stale);
      assert.match(formatWarning(stale),/^WARN stale allowlist entry:/);
    }
  });
  test('08 thread_local declarations can split without hiding an added static', () => {
    const p=rustRoot+'a.rs',q=rustRoot+'b.rs';
    const a='static A: Cell<u8> = Cell::new(0);',b='static B: Cell<u8> = Cell::new(0);';
    const wrap=s=>'thread_local! {'+s+'}';
    const base={[p]:wrap(a+b)},head={[p]:wrap(a),[q]:wrap(b)};
    assert.equal(globalsGate(trees(base,head)).exit,0);
    assert.equal(globalsGate(trees(base,{...head,[q]:wrap(b+b)})).exit,1);
  });

  test('06 splitting aliased Tauri uses does not count the repeated supporting import', () => {
    const p=rustRoot+'a.rs',q=rustRoot+'b.rs',imports='use tauri::AppHandle as H;';
    const a='fn a(x: H) {}',b='fn b(x: H) {}';
    const base={[p]:imports+a+b},head={[p]:imports+a,[q]:imports+b};
    const result=edgeGate(trees(base,head)); assert.equal(result.exit,0,JSON.stringify(result));
    assert.equal(result.totals[0].tauri_files,2); assert.equal(result.totals[1].tauri_files,2);
    assert.equal(edgeGate(trees(base,{...head,[q]:imports+b+'fn c(x: H) {}'})).exit,1);
    assert.equal(edgeGate(trees({}, {[q]:imports})).exit,1,'an unused new import remains a signal');
  });

  test('ambiguous moved aliases never receive transfer credit', () => {
    const p=rustRoot+'a.rs',q=rustRoot+'b.rs';
    const code='use std::process::Command as C; use other::C; fn f(){ C::new("a"); }';
    const result=processGate(trees({[p]:code},{[q]:code}));
    assert.equal(result.exit,1); assert.ok(result.diagnostics.some(d=>d.status==='WARN'));
  });

  const attemptsScope = rustRoot + 'cli/role_experiment/';
  const attemptsRegistry = () => ({
    version: 1,
    containers: [{
      id: 'RoleExperiment/Attempts', name: 'attempts.jsonl', scope: attemptsScope,
      owner: attemptsScope + 'storage.rs', externalLiteralBaseline: 0,
      writerInventory: { files: 0, sites: 0 },
    }],
  });
  function containerTree(files = {}, registry = attemptsRegistry()) {
    return trees({
      'scripts/containers.json': JSON.stringify(registry),
      [attemptsScope + 'storage.rs']: 'const NAME: &str = "attempts.jsonl";',
      ...files,
    });
  }
  test('04 registered literal: external fails, owner and test fixtures pass', () => {
    const consumer = attemptsScope + 'resume.rs';
    assert.equal(containersGate(containerTree()).exit, 0);
    const bad = containersGate(containerTree({ [
        consumer]: 'fn f() { let p = "attempts.jsonl"; }' }));
    assert.equal(bad.exit, 1);
    assert.equal(bad.metrics[0].externalLiterals, 1);
    assert.equal(containersGate(containerTree({
      [attemptsScope + 'tests.rs']: 'fn fixture() { let p = "attempts.jsonl"; }',
      [consumer]: '#[cfg(test)] fn fixture() { let p = "attempts.jsonl"; }',
    })).exit, 0);
    assert.equal(containersGate(containerTree({
      [consumer]: '#[cfg(any(test, windows))] fn f() { let p = r#"attempts.jsonl"#; }',
    })).exit, 1);
    assert.equal(containersGate(containerTree({
      [consumer]: '// "attempts.jsonl"\nfn f() { let p = "other.jsonl"; }',
    })).exit, 0);
  });
  test('04 writer inventory recognizes aliases, excludes cfg and warns unknown identities', () => {
    const source = 'use std::fs::{write as save, File as F, OpenOptions as O}; ' +
      'fn f() { save("unknown.json", ""); F::create("x"); O::new(); } ' +
      '#[cfg(test)] fn fixture() { std::fs::write("fixture", ""); }';
    assert.equal(measureSource(rustRoot + 'example.rs', source).writers.length, 3);
    assert.equal(measureSource(rustRoot + 'example/tests.rs', source).writers.length, 0);
    const result = containersGate(containerTree({ [rustRoot + 'example.rs']: source }));
    assert.equal(result.exit, 0);
    assert.equal(result.inventory.sites, 3);
    assert.equal(result.diagnostics.find(d => d.rule === 'unclassified_writers').files, 1);
  });
  test('04 rejects invalid registries and missing owners rather than passing silently', () => {
    for (const mutate of [
      r => { r.version = 2; },
      r => { r.containers[0].externalLiteralBaseline = 1; },
      r => { r.containers[0].owner = '../outside.rs'; },
      r => { r.containers[0].writerInventory.sites = -1; },
      r => { r.containers.push({ ...r.containers[0] }); },
    ]) {
      const registry = attemptsRegistry(); mutate(registry);
      assert.throws(() => parseRegistry(JSON.stringify(registry)));
      assert.equal(containersGate(containerTree({}, registry)).status, 'ERROR');
    }
    assert.throws(() => parseRegistry('{'));
    const snapshot = containerTree();
    snapshot.headTree.delete(attemptsScope + 'storage.rs');
    assert.equal(containersGate(snapshot).status, 'ERROR');
  });

  test('07 calls, aliases, multiline, ambiguity and shadowing', () => {
    for (const bad of [
      'fn f() { std::fs::read("x"); }',
      'use std::fs as disk; fn f() { disk::read("x"); }',
      'use std::fs::read as load; fn f() { load("x"); }',
      'use std::fs::{File as F, OpenOptions as O}; fn f() { F::open("x"); O::new(); }',
      'fn f() { File::create("x"); OpenOptions::new(); }',
      'fn f() { std\n::fs\n::read\n("x"); }',
    ]) control(diskGate, 'disk_files', 'disk.rs', 'fn f() {}', bad);
    const p = rustRoot + 'disk.rs';
    const ambiguous = diskGate(trees({}, {[p]:
      'use std::fs as disk; use other as disk; fn f() { disk::read("x"); }'}));
    assert.equal(ambiguous.exit, 1);
    assert.ok(ambiguous.diagnostics.some(d => d.status === 'WARN' &&
        d.reason === 'duplicate binding'));
    const glob = diskGate(trees({}, {[p]:'use std::fs::*; fn f() { read("x"); }'}));
    assert.equal(glob.exit, 1);
    assert.ok(glob.diagnostics.some(d => d.reason === 'glob import may bind path'));
    for (const clean of [
      'fn f() { read("x"); }',
      'fn f() { /* std::fs::read("x"); */ let s = "File::open"; }',
      '#[cfg(test)] mod tests { fn f() { std::fs::read("x"); } }',
      'mod File { fn open() {} } fn f() { File::open(); }',
      'use custom::fs; fn f() { fs::read("x"); }',
      'use std::fs::read as load; fn f() { let load = || (); load(); }',
    ]) assert.equal(diskGate(trees({}, {[p]:clean})).exit, 0, clean);
  });
  test('07 exact owners, tests and separate untyped receiver approximation', () => {
    const bad = 'fn f() { std::fs::read("x"); }';
    for (const name of ['filesystem.rs', 'filesystem/read.rs', 'path_identity.rs',
      'path_identity/read.rs', 'path_utils.rs', 'path_utils/read.rs', 'tests.rs',
      'foo/tests/read.rs', 'foo_tests.rs'])
      assert.equal(diskGate(trees({}, {[rustRoot + name]:bad})).exit, 0, name);
    for (const name of ['filesystem_extra.rs', 'store.rs', 'containers/read.rs'])
      assert.equal(diskGate(trees({}, {[rustRoot + name]:bad})).exit, 1, name);
    const result = diskGate(trees({}, {[rustRoot + 'model.rs']:
      'fn f(record: Record) { record.metadata(); record.exists(); record.canonicalize(); }'}));
    assert.equal(result.exit, 0);
    assert.equal(result.totals[1].disk_files, 0);
    assert.deepEqual(result.approximation.head, {files:1, sites:3, paths:[rustRoot + 'model.rs']});
  });
  test('07 file counts equal/down/up, no compensated new file or move credit', () => {
    const p = rustRoot + 'old.rs', q = rustRoot + 'new.rs';
    const bad = 'fn f() { std::fs::read("x"); }';
    const base = {[p]:bad};
    assert.equal(diskGate(trees(base, {[p]:bad + bad})).exit, 0);
    assert.equal(diskGate(trees(base, {[p]:''})).exit, 0);
    assert.equal(diskGate(trees(base, {[p]:bad, [q]:bad})).exit, 1);
    assert.equal(diskGate(trees(base, {[p]:'', [q]:bad})).exit, 1);
    assert.equal(diskGate(trees(base, {[q]:bad})).exit, 1);
  });
  test('07 immutable real Git base, working tree ignored, drift and blob errors', t => {
    const f = fixture(t), p = rustRoot + 'disk.rs';
    f.write(p, 'fn f() {}'); const base = f.commit();
    f.write(p, 'fn f() { std::fs::read("x"); }'); f.commit();
    const snapshot = f.snapshot(base);
    f.write(p, 'fn f() {}');
    assert.equal(diskGate(snapshot).exit, 1);
    f.write('next', 'next'); f.commit();
    assert.equal(diskGate(snapshot).exit, 3);
    const broken = trees({}, {[p]:'x'});
    broken.readBlob = () => { throw Error('missing blob'); };
    assert.equal(diskGate(broken).status, 'ERROR');
  });
}
