import {spawnSync} from 'node:child_process';
import * as rust from './sanitizing_rust_scan.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {runGate as diskGate} from './sanitizing_disk_access.mjs';
import {measureSource, runGate as containersGate} from './sanitizing_containers.mjs';
const rustRoot = 'src-tauri/src/';
function trees(base, head=base) {
 const tree = files => new Map(Object.entries(files).map(([path,text])=>[path,{path,text}]));
 return {baseTree:tree(base),headTree:tree(head),readBlob:e=>e.text,assertStableHead(){}};
}

test('3330 inherited test modules and production controls', async () => {
  const m = await import('./sanitizing_rust_scan.mjs');
  const createRustScanner = m.createRustScanner ??
    ((tree, read) => p => m.scanRust(p, read(tree.get(p))));
  const body = 'static X: std::sync::Mutex<u8> = std::sync::Mutex::new(0); '
    + 'fn f() { std::fs::write("state.json", "x"); std::env::set_var("A", "B"); }';
  const files = {
    [rustRoot + 'lib.rs']: '#[cfg(test)] pub(crate) mod harness; mod production;',
    [rustRoot + 'harness.rs']: body + ' mod child;',
    [rustRoot + 'harness/child.rs']: body,
    [rustRoot + 'production.rs']: body,
  };
  const s = trees(files), scan = createRustScanner(s.headTree, s.readBlob);
  for (const p of ['harness.rs', 'harness/child.rs']) {
    const result = scan(rustRoot + p);
    assert.equal(result.productionTokens.length, 0);
    assert.ok(result.items.every(i => i.testOnly));
    assert.equal(result.isTestOffset(0), true);
  }
  assert.ok(scan(rustRoot + 'production.rs').productionTokens.length > 0);
  const disk = diskGate(s);
  assert.equal(disk.exit, 0);
  assert.equal(disk.totals[1].disk_files, 1);
  // Both sides independently classify the same child from their own parent blob.
  const changed = { ...files, [rustRoot + 'lib.rs']: 'mod harness; mod production;' };
  const delta = diskGate(trees(files, changed));
  assert.equal(delta.exit, 1);
  assert.equal(delta.totals[0].disk_files, 1);
  assert.equal(delta.totals[1].disk_files, 3);
});
test('3330 cfg, module roots, ambiguity and unsupported syntax stay conservative', async () => {
  const m = await import('./sanitizing_rust_scan.mjs');
  const createRustScanner = m.createRustScanner ??
    ((tree, read) => p => m.scanRust(p, read(tree.get(p))));
  const body = 'fn f() { std::fs::write("x", "y"); }';
  for (const declaration of [
    '#[cfg(test)] mod r#harness;',
    '#[allow(dead_code)] #[cfg(all(test, unix))] pub(crate) mod harness;',
    '#[cfg(any(test, all(test, unix)))] mod harness;',
  ]) {
    const s = trees({[rustRoot + 'lib.rs']: declaration, [rustRoot + 'harness/mod.rs']: body});
    assert.equal(createRustScanner(s.headTree, s.readBlob)(rustRoot + 'harness/mod.rs')
      .productionTokens.length, 0);
  }
  for (const declaration of [
    'mod harness;', '#[cfg(any(test, unix))] mod harness;',
    '#[cfg(not(test))] mod harness;', '#[cfg_attr(unix, cfg(test))] mod harness;',
    '#[cfg(test)] mod harness; mod harness;',
    '#[cfg(test)] mod harness; #[path="harness.rs"] mod other;',
    '#[cfg(test)] mod harness; mod inline { mod x; }',
    '#[cfg(test)] mod harness; include!("generated.rs");',
    '#[cfg(test)] mod harness; macro_rules! m { () => { mod harness; } }',
  ]) {
    const s = trees({[rustRoot + 'lib.rs']: declaration, [rustRoot + 'harness.rs']: body});
    assert.ok(createRustScanner(s.headTree, s.readBlob)(rustRoot + 'harness.rs')
      .productionTokens.length > 0, declaration);
  }
  const ambiguous = trees({[rustRoot + 'lib.rs']: '#[cfg(test)] mod harness;',
    [rustRoot + 'harness.rs']:body, [rustRoot + 'harness/mod.rs']:body});
  assert.ok(createRustScanner(ambiguous.headTree, ambiguous.readBlob)(rustRoot + 'harness.rs')
    .productionTokens.length > 0);
  const orphan = trees({[rustRoot + 'orphan.rs']:body});
  assert.ok(createRustScanner(orphan.headTree, orphan.readBlob)(rustRoot + 'orphan.rs')
    .productionTokens.length > 0);
});

test('3330 containers contextual scan, standalone API and head integration', async () => {
  const m = await import('./sanitizing_rust_scan.mjs');
  const factory = m.createRustScanner ?? ((tree, read) => p => m.scanRust(p, read(tree.get(p))));
  const body = 'fn f() { std::fs::write("state.json", "x"); }';
  const files = {[rustRoot+'lib.rs']: '#[cfg(test)] mod h; mod real;',
    [rustRoot+'h.rs']: body, [rustRoot+'real.rs']: body,
    [rustRoot+'store/owner.rs']: '',
    'scripts/containers.json': JSON.stringify({version: 1, containers: [{
      id: 'State', name: 'state.json', scope: rustRoot,
      owner: rustRoot+'store/owner.rs', externalLiteralBaseline: 0,
      writerInventory: {files: 0, sites: 0}
    }]})};
  const s = trees(files), scan = factory(s.headTree, s.readBlob);
  assert.equal(measureSource(rustRoot+'h.rs', body).writers.length, 1);
  assert.equal(measureSource(rustRoot+'h.rs', body, scan(rustRoot+'h.rs')).writers.length, 0);
  assert.equal(measureSource(rustRoot+'real.rs', body, scan(rustRoot+'real.rs')).writers.length, 1);
  const result = containersGate(s);
  assert.notEqual(result.status, 'ERROR', JSON.stringify(result));
  assert.deepEqual(result.inventory, {files: 1, sites: 1});
  assert.equal(result.diagnostics.filter(d => d.rule === 'external_container_literal').length, 1);
});

const prefix = rustRoot;
const body = 'fn f() { std::fs::write("x", "y"); }';
function scan(files) {
  const tree = new Map(Object.entries(files).map(([path,text]) => [path,{path,text}]));
  return rust.createRustScanner ? rust.createRustScanner(tree, e => e.text)
    : p => rust.scanRust(p, tree.get(p).text);
}
test('custom crate roots retain production; ordinary nested lib/main resolve as modules',()=>{
 for(const cargo of ['[lib]\npath="src/h.rs"','[[bin]]\npath="src/h.rs"',
  '[[example]]\npath="src/h.rs"','[package]\nbuild="src/h.rs"']) {
  const factory=scan({[prefix+'lib.rs']:'#[cfg(test)] mod h;',
   [prefix+'h.rs']:body,'src-tauri/Cargo.toml':cargo});
  assert.ok(factory(prefix+'h.rs').productionTokens.length>0);
 }
 const files={[prefix+'lib.rs']:'#[cfg(test)] mod h;',
  [prefix+'h.rs']:'mod lib; mod main;',
  [prefix+'h/lib.rs']:'mod child;', [prefix+'h/main.rs']:'mod child;',
  [prefix+'h/lib/child.rs']:body,[prefix+'h/main/child.rs']:body};
 const factory=scan(files);
 assert.equal(factory(prefix+'h/lib/child.rs').productionTokens.length,0);
 assert.equal(factory(prefix+'h/main/child.rs').productionTokens.length,0);
 files[prefix+'bin/tool.rs']=body;
 assert.ok(scan(files)(prefix+'h/lib/child.rs').productionTokens.length>0);
});

test('3330 custom manifest spellings retain the production crate root', () => {
  const common = '[package]\nname="repro"\nversion="0.1.0"\nedition="2021"\n';
  const manifests = [
    common + '[lib] # custom production root\npath="src/h.rs"\n',
    common + '[ lib ] # spaced production root\npath="src/h.rs"\n',
    common + '[lib]\n"path"="src/h.rs"\n',
    common + '["lib"] # quoted production root\n"path"="src/h.rs"\n',
    common + "['lib']\n'path'='src/h.rs'\n",
    'lib.path="src/h.rs"\n' + common,
    'lib = { path="src/h.rs" }\n' + common,
    common + '[[ bin ]] # spaced production root\nname="tool"\npath="src/h.rs"\n',
    common + '[[ "bin" ]]\nname="tool"\npath="src/h.rs"\n',
    common + '["l\\u0069b"]\npath="src/h.rs"\n',
  ];
  for (const cargo of manifests) {
    const files = {[prefix + 'lib.rs']: '#[cfg(test)] mod h;',
      [prefix + 'h.rs']: body, 'src-tauri/Cargo.toml': cargo};
    assert.ok(scan(files)(prefix + 'h.rs').productionTokens.length > 0, cargo);
    const result = diskGate(trees(files));
    assert.equal(result.exit, 0, JSON.stringify(result));
    assert.equal(result.totals[1].disk_files, 1, cargo);
  }
  const ordinary = {[prefix + 'lib.rs']: '#[cfg(test)] mod h;',
    [prefix + 'h.rs']: body, 'src-tauri/Cargo.toml': common
      + '[ lib ] # ordinary\ncrate-type = [\n "lib",\n "cdylib",\n]\n'};
  assert.equal(scan(ordinary)(prefix + 'h.rs').productionTokens.length, 0);
});

const packageHeader = '[package]\nname="repro"\nversion="0.1.0"\nedition="2021"\n';
function manifestProof(cargo, retained) {
  const files = {[prefix + 'lib.rs']: '#[cfg(test)] mod h;',
    [prefix + 'h.rs']: body, 'src-tauri/Cargo.toml': cargo};
  const tokens = scan(files)(prefix + 'h.rs').productionTokens.length;
  assert.equal(tokens > 0, retained, cargo);
  const result = diskGate(trees(files));
  assert.equal(result.exit, 0, JSON.stringify(result));
  assert.equal(result.totals[1].disk_files, retained ? 1 : 0, cargo);
}
test('3330 R4-B1 nested arrays never hide a custom production root', () => {
  for (const nested of ['[]', '[1, 2]', '[true]', '[{ a = 1 }]'])
    manifestProof(packageHeader + '[lib]\nx = [\n ' + nested
      + ',\n]\npath="src/h.rs"\n', true);
});
test('3330 unknown headers alone disable inheritance', () => {
  for (const header of ['["lib"]', "[ 'lib' ]", '["l\\u0069b"]', '[[bin]]', '[lib.x]'])
    manifestProof(packageHeader + header + '\npath="src/h.rs"\n', true);
});
test('3330 unknown statements alone disable inheritance', () => {
  for (const entry of ['"path"="src/h.rs"', "'path'='src/h.rs'",
    'note="""\n[x]\n"""\npath="src/h.rs"',
    "note='''\n[x]\n'''\npath=\"src/h.rs\"",
    'name="l\\u0069b"\npath="src/h.rs"',
    'crate-type=[\n []\n]\npath="src/h.rs"'])
    manifestProof(packageHeader + '[lib]\n' + entry + '\n', true);
});
test('3330 lib assignment allowlist alone retains custom paths', () => {
  manifestProof(packageHeader + '[lib]\npath="src/h.rs"\n', true);
});
test('3330 package build policy alone retains production targets', () => {
  manifestProof(packageHeader + 'build="src/h.rs"\n', true);
});
test('3330 root assignments alone disable inheritance', () => {
  manifestProof('lib={path="src/h.rs"}\n' + packageHeader, true);
});
test('3330 understood complete statements permit inheritance', () => {
  const dependency = 'serde={version="1", features=["derive"]}\n';
  const target = "[target.'cfg(target_os = \"windows\")'.dependencies]\n";
  const ordinary = packageHeader + 'build=false\n[dependencies]\n' + dependency
    + target + dependency + '[ lib ] # comment\nname="ordinary # value"\n'
    + 'crate-type=[\n "lib", # comment\n "cdylib",\n]\n';
  manifestProof(ordinary, false);
  manifestProof(ordinary.replaceAll('\n', '\r\n'), false);
});

test('3330 quoted package build keys are production roots', () => {
  for (const key of ['"build"', "'build'"])
    manifestProof(packageHeader + key + '="src/h.rs"\n', true);
});
function externalProof(extra, retained = true) {
  const files = {[prefix + 'lib.rs']: '#[cfg(test)] mod h;', [prefix + 'h.rs']: body, ...extra};
  assert.equal(scan(files)(prefix + 'h.rs').productionTokens.length > 0, retained,
    JSON.stringify(extra));
  const result = diskGate(trees(files));
  assert.equal(result.exit, 0, JSON.stringify(result));
  assert.equal(result.totals[1].disk_files, retained ? 1 : 0);
}
test('3330 external Rust declarators retain production throughout the snapshot', () => {
  for (const path of ['src-tauri/build.rs', 'src-tauri/tests/external.rs',
    'crates/x/src/lib.rs', 'elsewhere/carrier.rs']) {
    for (const text of ['include!("src/h.rs");', '#[path="src/h.rs"] mod h;',
      '#[cfg_attr(unix, path="src/h.rs")] mod h;'])
      externalProof({[path]: text});
  }
  externalProof({'src-tauri/build.rs': 'mod src { mod h; }'});
  externalProof({'src-tauri/build.rs': 'mod src;', [prefix + 'mod.rs']: 'mod h;'});
  externalProof({'elsewhere/macros.rs': 'macro_rules! m { () => { mod src { mod h; } } }'});
});
test('3330 all Cargo manifests receive the complete conservative policy', () => {
  externalProof({'src-tauri/src/Cargo.toml': '[[bin]]\nname="prod"\npath="h.rs"\n'});
  const implicit = {[prefix + 'lib.rs']: '#[cfg(test)] mod src;',
    [prefix + 'src.rs']: 'mod lib;', [prefix + 'src/lib.rs']: body,
    [prefix + 'Cargo.toml']: packageHeader};
  assert.ok(scan(implicit)(prefix + 'src/lib.rs').productionTokens.length > 0);
  const gate = diskGate(trees(implicit));
  assert.equal(gate.exit, 0, JSON.stringify(gate));
  assert.equal(gate.totals[1].disk_files, 1);
  for (const path of ['Cargo.toml', 'crates/x/Cargo.toml', 'other/Cargo.toml',
    'src-tauri/src/Cargo.toml']) {
    for (const cargo of ['[lib]\npath="../../src-tauri/src/h.rs"\n',
      '[[bin]]\npath="src-tauri/src/h.rs"\n',
      '[[test]]\npath="../../src-tauri/src/h.rs"\n',
      '[package]\nbuild="src-tauri/src/h.rs"\n',
      '[package]\n"build"="src-tauri/src/h.rs"\n',
      "[package]\n'build'='src-tauri/src/h.rs'\n", '[unknown]\nx=1\n',
      '[[test]]\n"path"="../../src-tauri/src/h.rs"\n'])
      externalProof({[path]: cargo});
  }
});
test('3330 ordinary workspace, sibling crates and local fixture targets permit inheritance', () => {
  externalProof({'Cargo.toml': '[workspace]\nmembers=["src-tauri", "crates/x"]\n'
    + 'default-members=["src-tauri"]\nresolver="2"\n',
    'src-tauri/Cargo.toml': packageHeader,
    'src-tauri/build.rs': 'fn main() { println!("mod src; include! is text"); }',
    'crates/x/Cargo.toml': packageHeader + '[lib]\ndoctest=false\n',
    'crates/x/src/lib.rs': 'mod safe;', 'crates/x/src/safe.rs': 'fn f() {}',
    'crates/x/tests/fixtures/probe/Cargo.toml': packageHeader
      + '[workspace]\n[[test]]\nname="probe"\npath="failures.rs"\n',
    'crates/x/tests/fixtures/probe/failures.rs': '#[test] fn f() {}',
  }, false);
});
test('3330 workspace members cannot escape snapshot coverage', () => {
  for (const member of ['../outside', '/absolute', 'crates/*', 'missing'])
    externalProof({'Cargo.toml': '[workspace]\nmembers=["' + member + '"]\n'});
  externalProof({'Cargo.toml': '[workspace]\nunknown="src-tauri/src/h.rs"\n'});
  externalProof({'Cargo.toml': '[workspace]\nunknown=[]\n'});
  externalProof({'Cargo.toml': '[workspace]\nmembers="src-tauri"\n'});
});
test('3330 manifest comments cannot force exponential fallback', () => {
  const cargo = packageHeader + '[lib]\ncrate-type=[\n "lib", # '
    + '#'.repeat(256) + '\n []\n]\n';
  const files = {[prefix + 'lib.rs']: '#[cfg(test)] mod h;',
    [prefix + 'h.rs']: body, 'src-tauri/Cargo.toml': cargo};
  const code = 'import * as rust from '
    + JSON.stringify(new URL('./sanitizing_rust_scan.mjs', import.meta.url).href) + ';'
    + 'const tree = new Map(Object.entries(' + JSON.stringify(files)
    + ').map(([p,text]) => [p,{text}])); rust.createRustScanner?.(tree,e=>e.text);';
  const result = spawnSync(process.execPath, ['--input-type=module', '-e', code],
    {timeout: 2000, encoding: 'utf8', windowsHide: true});
  assert.equal(result.status, 0, result.error?.message ?? result.stderr);
});

test('3330 composition roots never inherit from another root', () => {
  for (const [parent, target] of [['lib', 'main'], ['main', 'lib']]) {
    const files = {[prefix + parent + '.rs']: '#[cfg(test)] mod ' + target + ';',
      [prefix + target + '.rs']: body};
    assert.ok(scan(files)(prefix + target + '.rs').productionTokens.length > 0, target);
    const result = diskGate(trees(files));
    assert.equal(result.exit, 0, JSON.stringify(result));
    assert.equal(result.totals[1].disk_files, 1, target);
  }
});
test('3330 heuristic test names cannot prove a cfgTest incoming edge', () => {
  for (const [name, parent] of [['integration_tests', 'integration_tests.rs']]) {
    const target = prefix + name + '/h.rs';
    const files = {[prefix + 'lib.rs']: 'pub mod ' + name + ';',
      [prefix + parent]: 'mod h;', [target]: body};
    assert.ok(scan(files)(target).productionTokens.length > 0, parent);
    const result = diskGate(trees(files));
    assert.equal(result.exit, 0, JSON.stringify(result));
    assert.equal(result.totals[1].disk_files, 1, parent);
  }
});
test('3330 mod.rs parents resolve children without inheriting a false nested target', () => {
  const files = {[prefix + 'lib.rs']: '#[cfg(test)] mod h;',
    [prefix + 'h/mod.rs']: 'mod child;', [prefix + 'h/child.rs']: body,
    [prefix + 'h/mod/child.rs']: body};
  const factory = scan(files);
  assert.equal(factory(prefix + 'h/child.rs').productionTokens.length, 0);
  assert.ok(factory(prefix + 'h/mod/child.rs').productionTokens.length > 0);
  const result = diskGate(trees(files));
  assert.equal(result.exit, 0, JSON.stringify(result));
  assert.equal(result.totals[1].disk_files, 1);
});

test('3330 triple-string backtracking never swallows a production target', () => {
  const tail = '[lib]\npath="src/h.rs"\n[features]\ndefault=[]';
  for (const [value, suffix] of [
    ['["""a""", 1e5]', ' # """]\n'],
    ['["""a""", {b=1}]', ' # """]\n'],
    ["['''a''', 1e5]", " # ''']\n"],
    ['{a="""x""", b=1e5}', ' # """}\n'],
  ]) manifestProof(packageHeader + 'x-meta=' + value + '\n' + tail + suffix, true);
});
