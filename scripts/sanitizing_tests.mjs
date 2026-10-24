import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createSnapshot, parseArgs, parseTree, parseChanged, gitRunner } from './sanitizing_baseline.mjs';
import { lineLengths, fileMetrics } from './sanitizing_line_metrics.mjs';
import { runGate as lengthGate } from './sanitizing_line_length.mjs';
import { runGate as sizeGate } from './sanitizing_file_size.mjs';
import { scanRust, lexRust, decodeRustString, compatibilitySites } from './sanitizing_rust_scan.mjs';

// Fixtures are isolated repositories, never a user's worktree. No network/symlinks/profiles.
function fixture(t) {
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), 'sanitizing-'));
  const env = { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: path.join(cwd, 'no-global-config'),
    GIT_TEMPLATE_DIR: path.join(cwd, 'empty-template') };
  fs.mkdirSync(env.GIT_TEMPLATE_DIR);
  const runner = (args, options) => gitRunner(args, { ...options, env });
  t.after(() => { const resolved = fs.realpathSync(cwd), parent = fs.realpathSync(os.tmpdir());
    assert.equal(path.dirname(resolved), parent); fs.rmSync(resolved, { recursive: true }); });
  const git = (...args) => {
    const r = runner(args, { cwd });
    assert.equal(r.error, undefined); assert.equal(r.signal, null); assert.equal(r.status, 0, r.stderr?.toString());
    return r.stdout.toString().trim();
  };
  git('init', '--initial-branch=main'); git('config', 'user.name', 'Sanitizing fixture');
  git('config', 'user.email', 'sanitizing@example.invalid'); git('config', 'core.autocrlf', 'false');
  git('config', 'commit.gpgsign', 'false');
  const write = (name, text) => { const target = path.join(cwd, name); fs.mkdirSync(path.dirname(target), { recursive: true }); fs.writeFileSync(target, text); };
  const commit = () => { git('add', '--all'); git('-c', 'core.hooksPath=', 'commit', '--allow-empty', '-m', 'fixture'); return git('rev-parse', 'HEAD'); };
  write('seed', 'seed'); const base = commit();
  return { cwd, git, write, commit, base, runner, snapshot: (baseRef = base, run = runner) => createSnapshot({ cwd, baseRef, runner: run }) };
}
const text = (n, width = 1) => ('a'.repeat(width) + '\n').repeat(n);
function sequence(f, gate, name, before, mutation, expected = 1) {
  f.write(name, before); const base = f.commit();
  assert.equal(gate(f.snapshot(base)).exit, 0);
  f.write(name, mutation); f.commit(); const result = gate(f.snapshot(base));
  assert.equal(result.exit, expected, JSON.stringify(result));
  f.write(name, before); f.commit(); assert.equal(gate(f.snapshot(base)).exit, 0);
  return result;
}
test('Unicode metrics match source: LF CRLF empty lone CR no final newline', () => {
  assert.deepEqual(lineLengths(''), []); assert.deepEqual(lineLengths('a\r\nb\n'), [1, 1]);
  assert.deepEqual(lineLengths('a\rb'), [3]); assert.deepEqual(lineLengths('😀'.repeat(100)), [100]);
  assert.equal(fileMetrics(text(1000)).lines, 1000); assert.equal(fileMetrics(text(1001)).lines, 1001);
});
test('01 controls: 100→101→100 astral, debt equal/down/up, comments and tests', t => {
  const f = fixture(t);
  for (const [before, after] of [['😀'.repeat(100), '😀'.repeat(101)], [text(1, 101), text(2, 101)], ['// ' + 'a'.repeat(97), '// ' + 'a'.repeat(98)], ['#[test]\nfn f() {}', '#[test]\n' + text(1, 101)]]) {
    const r = sequence(f, lengthGate, 'tests.rs', before, after);
    assert.ok(r.diagnostics[0].lines.some(x => x.added));
  }
  sequence(f, lengthGate, 'debt.rs', text(2, 101), text(1, 101), 0);
  sequence(f, lengthGate, 'crlf.rs', 'a'.repeat(100) + '\r\n', 'a'.repeat(101) + '\r\n');
});
test('02 controls: 1000→1001→1000 and existing debt equal/down/up', t => {
  const f = fixture(t);
  sequence(f, sizeGate, 'x.rs', text(1000), text(1001));
  sequence(f, sizeGate, 'x.rs', text(1001), text(1002));
  sequence(f, sizeGate, 'x.rs', text(1002), text(1001), 0);
});
test('new paths cannot offset debt with deletions; rename copy delete; exact unusual paths', t => {
  const f = fixture(t); const old = '-old space 😀.rs';
  f.write(old, text(1, 101)); const base = f.commit();
  f.git('mv', '--', old, '新 name.rs'); f.commit();
  const s = f.snapshot(base); assert.deepEqual(s.changed.map(x => x.status).sort(), ['A', 'D']);
  assert.equal(lengthGate(s).exit, 1);
  f.write('copy.rs', text(1001)); f.commit(); assert.equal(sizeGate(f.snapshot(base)).exit, 1);
  f.git('rm', '--', '新 name.rs', 'copy.rs'); f.commit(); assert.equal(lengthGate(f.snapshot(base)).exit, 0);
  // Colon is valid Git tree data even though Windows cannot materialize it.
  const oid = f.git('hash-object', '-w', '--stdin');
  f.write('tree.bin', Buffer.concat([Buffer.from('100644 a:b.rs\0'), Buffer.from(oid, 'hex')]));
  const tree = f.git('hash-object', '-t', 'tree', '-w', 'tree.bin');
  const colonCommit = f.git('commit-tree', tree, '-p', f.git('rev-parse', 'HEAD'), '-m', 'colon');
  f.git('update-ref', 'HEAD', colonCommit);
  assert.equal(lengthGate(f.snapshot(base)).exit, 0);
});
test('divergent merge-base and working tree edits ignored', t => {
  const f = fixture(t); f.git('switch', '-c', 'side'); f.write('side.rs', 'x'); const side = f.commit();
  f.git('switch', 'main'); f.write('main.rs', 'y'); f.commit();
  const s = f.snapshot(side); assert.equal(s.baseOid, f.base); assert.equal(s.changed.length, 1);
  f.write('main.rs', text(1, 101)); assert.equal(lengthGate(s).exit, 0);
});
test('explicit empty/no changes, precedence, duplicate/unknown args and invalid ref', t => {
  const f = fixture(t); assert.equal(lengthGate(f.snapshot()).changedRustFiles, 0);
  assert.equal(parseArgs([], { GATE_BASE_REF: 'topic' }).baseRef, 'topic');
  assert.equal(parseArgs([], { GATE_BASE_REF: '' }).baseRef, 'origin/main');
  assert.equal(parseArgs(['--base-ref', 'x'], { GATE_BASE_REF: 'y' }).baseRef, 'x');
  for (const args of [['--bad'], ['--base-ref'], ['--base-ref', ''], ['--base-ref', 'x', '--base-ref', 'y']]) assert.throws(() => parseArgs(args), e => e.exit === 2);
  assert.throws(() => f.snapshot('missing-ref'), e => e.exit === 3);
});
test('HEAD drift overrides a passing measurement and partial FAIL', t => {
  const f = fixture(t); f.write('x.rs', text(1, 101)); f.commit(); const s = f.snapshot();
  f.write('z', 'new'); f.commit(); assert.equal(lengthGate(s).exit, 3);
});
test('special Git modes and UTF8 are ERROR, never absent', t => {
  const f = fixture(t), blob = f.git('hash-object', '-w', '--stdin');
  for (const [mode, oid] of [['120000', blob], ['160000', f.base]]) {
    f.git('update-index', '--add', '--cacheinfo', mode, oid, 'special.rs');
    f.git('-c', 'core.hooksPath=', 'commit', '-m', mode);
    assert.equal(lengthGate(f.snapshot()).exit, 3);
  }
  f.git('update-index', '--force-remove', 'special.rs'); f.write('invalid.rs', Buffer.from([0xff])); f.commit();
  assert.equal(sizeGate(f.snapshot()).exit, 3);
  assert.throws(() => parseTree(Buffer.from([0xff, 0])), e => e.exit === 3);
  assert.throws(() => parseTree(Buffer.from('100644 blob bad\tx.rs\0')), e => e.exit === 3);
  assert.throws(() => parseChanged(Buffer.from('M\0x.rs')), e => e.exit === 3);
});
test('Git failures/spawn/signal/timeout/maxBuffer and missing object cannot give partial verdicts', t => {
  const f = fixture(t); f.write('x.rs', 'x'); f.commit();
  for (const failure of [{status:1}, {status:null,signal:'SIGTERM'}, {status:null,error:Object.assign(new Error('timeout'),{code:'ETIMEDOUT'})}, {status:null,error:Object.assign(new Error('maxBuffer'),{code:'ENOBUFS'})}]) {
    assert.throws(() => f.snapshot(f.base, () => ({stdout:Buffer.from('partial'),stderr:Buffer.from('failed'),...failure})), e => e.exit === 3);
  }
  assert.throws(() => f.snapshot(f.base, () => {throw Error('spawn');}), e => e.exit === 3);
  const runner = (args, opts) => args[0] === 'cat-file' ? {status:1,stdout:Buffer.from('partial'),stderr:Buffer.from('missing object')} : gitRunner(args, opts);
  assert.equal(lengthGate(f.snapshot(f.base,runner)).exit,3);
});

test('Git resolution selects a regular file in a directory with spaces without executing it', t => {
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), 'sanitizing-git-'));
  t.after(() => { assert.equal(path.dirname(fs.realpathSync(cwd)), fs.realpathSync(os.tmpdir()));
    fs.rmSync(cwd, {recursive:true}); });
  const name = process.platform === 'win32' ? 'git.exe' : 'git';
  const nonfile = path.join(cwd,'nonfile'), space = path.join(cwd,'Git space 😀');
  fs.mkdirSync(nonfile); fs.mkdirSync(path.join(nonfile,name)); fs.mkdirSync(space);
  const executable = path.join(space,name); fs.writeFileSync(executable,'not executed', {mode:0o755});
  const script = `
    import assert from 'node:assert/strict'; import cp from 'node:child_process';
    import {syncBuiltinESMExports} from 'node:module';
    cp.spawnSync = file => {assert.equal(file,${JSON.stringify(executable)}); return {status:0};};
    syncBuiltinESMExports();
    const {gitRunner} = await import(${JSON.stringify(new URL('./sanitizing_baseline.mjs', import.meta.url).href)});
    assert.equal(gitRunner([], {cwd:${JSON.stringify(cwd)},env:{PATH:${JSON.stringify([nonfile,space].join(path.delimiter))}}}).status,0);
  `;
  const r = spawnSync(process.execPath,['--input-type=module','-e',script], {
    stdio:['ignore','pipe','pipe'],timeout:120_000,maxBuffer:64*1024*1024,windowsHide:true,
  });
  assert.equal(r.error,undefined); assert.equal(r.signal,null); assert.equal(r.status,0,r.stderr?.toString());
});
test('Git resolution preserves env/options, PATH order and import without I/O', () => {
  // Isolate builtin stubs in a Node child: no fake executable is ever launched.
  const script = `
    import assert from 'node:assert/strict';
    import fs from 'node:fs';
    import path from 'node:path';
    import cp from 'node:child_process';
    import { syncBuiltinESMExports } from 'node:module';
    let available, attempted = [], calls = [], accessMode, failure;
    fs.statSync = p => {
      attempted.push(p);
      if (failure) throw Object.assign(new Error('resolution failed'), {code: failure});
      if (p !== available) throw Object.assign(new Error('missing'), {code: 'ENOENT'});
      return {isFile: () => true};
    };
    fs.accessSync = (p, mode) => { assert.equal(p, available); accessMode = mode; };
    cp.spawnSync = (file, args, options) => { calls.push({file,args,options}); return {status:0}; };
    syncBuiltinESMExports();
    const {gitRunner, createSnapshot} = await import(${JSON.stringify(new URL('./sanitizing_baseline.mjs', import.meta.url).href)});
    assert.deepEqual(attempted, []); assert.deepEqual(calls, []);
    const windows = process.platform === 'win32', cwd = process.cwd();
    const executable = windows ? 'git.exe' : 'git';
    const first = path.resolve(cwd, 'missing'), space = path.resolve(cwd, 'Git space 😀');
    available = path.join(space, executable);
    const env = {PATH: [first, windows ? '"' + space + '"' : space].join(path.delimiter),
      GIT_CONFIG_NOSYSTEM:'1', GIT_CONFIG_GLOBAL:'fixture-only', GIT_TEMPLATE_DIR:'fixture-template'};
    if (windows) env.Path = first; // PATH sorts before Path.
    const args = ['rev-parse', 'HEAD'];
    assert.equal(gitRunner(args, {cwd,env}).status, 0);
    assert.deepEqual(attempted, [path.join(first, executable), available]);
    assert.equal(accessMode, windows ? fs.constants.F_OK : fs.constants.X_OK);
    assert.deepEqual(calls[0], {file:available,args,options:{cwd,env,
      stdio:['ignore','pipe','pipe'],timeout:120000,maxBuffer:64*1024*1024,windowsHide:true}});
    if (windows) { const onlyPath = {Path:space}; gitRunner(args,{cwd,env:onlyPath}); assert.equal(calls.at(-1).options.env,onlyPath); }
    available = path.join(cwd, executable);
    gitRunner(args, {cwd,env:{PATH:''}}); assert.equal(calls.at(-1).file,available);
    available = path.resolve(cwd, 'relative', executable);
    gitRunner(args, {cwd,env:{PATH:'relative'}}); assert.equal(calls.at(-1).file,available);
    const inherited = windows ? process.env.PATH : '/usr/bin:/bin';
    let inheritedDir = inherited.split(path.delimiter)[0];
    if (windows && inheritedDir.startsWith('"') && inheritedDir.endsWith('"')) inheritedDir = inheritedDir.slice(1,-1);
    available = path.resolve(cwd,inheritedDir,executable);
    gitRunner(args,{cwd,env:{}}); assert.equal(calls.at(-1).file,available);
    available = undefined; const before = calls.length;
    const missing = {cwd,env:{PATH:first}};
    assert.throws(() => gitRunner(args,missing), e => e.exit === 3 && e.message.includes('install Git/check PATH'));
    assert.throws(() => createSnapshot({cwd,runner:a => gitRunner(a,missing)}), e => e.exit === 3);
    failure = 'EIO';
    assert.throws(() => gitRunner(args,missing), e => e.exit === 3 && /resolution failed/.test(e.message));
    assert.equal(calls.length,before);
  `;
  const r = spawnSync(process.execPath, ['--input-type=module', '-e', script], {
    stdio: ['ignore', 'pipe', 'pipe'], timeout: 120_000, maxBuffer: 64 * 1024 * 1024, windowsHide: true,
  });
  assert.equal(r.error, undefined); assert.equal(r.signal, null); assert.equal(r.status, 0, r.stderr?.toString());
});

test('cfg truth table never cuts later production, cfg_attr remains production', () => {
  for (const [cfg, excluded] of [['test',true],['all(test,unix)',true],['any(has_embedded_dist,test)',false],['not(test)',false],['any(all(test,unix),test)',true],['all()',false],['any()',false],['not(not(test))',false]]) {
    const a=scanRust('x.rs',`#[cfg(${cfg})] mod m { fn hidden() {} } fn visible() {}`);
    assert.equal(a.items.find(i=>i.name==='hidden').testOnly,excluded,cfg);
    assert.equal(a.items.find(i=>i.name==='visible').testOnly,false,cfg);
  }
  const a=scanRust('x.rs','#[cfg_attr(unix,test)] fn f() {} fn production() {}');
  assert.equal(a.items[0].testOnly,false);assert.ok(a.attributes[0].signals.some(s=>s.name==='test'));
});
test('inner cfg is lexical, async direct/aliased tests exclude body only, test paths', () => {
  const a=scanRust('x.rs','use tokio::{test as tt}; mod m { #![cfg(all(test,unix))] fn a() {} } #[tt] async fn b() {} fn production() {}');
  assert.equal(a.items.find(i=>i.name==='a').testOnly,true);assert.equal(a.items.find(i=>i.name==='b').directTest,true);
  assert.equal(a.items.find(i=>i.name==='production').testOnly,false);
  const b=a.items.find(i=>i.name==='b');assert.equal(a.isTestOffset(b.bodyStart+1),true);assert.equal(a.isTestOffset(b.start),false);
  for (const [paths, excluded] of [
    [['tests.rs','src/tests.rs','x_tests.rs','src/tests/x.rs'], true],
    [['contest.rs','src/tests_extra/x.rs','a_tests.rs.bak'], false],
  ]) for (const p of paths) {
    const source = scanRust(p,'fn p(){}');
    assert.equal(source.items[0].testOnly,excluded,p);
    assert.equal(source.productionTokens.length === 0,excluded,p);
  }
});
test('nested imports/self/as and nearest parameter/block bindings', () => {
  const a=scanRust('x.rs','use std::{fs::{self as disk,File as F},process::Command as C}; fn f(disk: Thing) { disk::write(); {use tokio::fs as disk;disk::write();} }');
  assert.equal(a.resolvePath('F').path,'std::fs::File'); assert.equal(a.resolvePath('C').path,'std::process::Command');
  const calls=a.tokens.filter(t=>t.name==='disk'&&a.tokens[t.index+1]?.text==='::');
  assert.equal(a.resolvePath('disk::write',calls[0].scope).shadowed,true);
  assert.equal(a.resolvePath('disk::write',calls[1].scope).path,'tokio::fs::write');
});
test('one-hop path aliases, destructuring/glob/reassignment/cyclic constants retain uncertainty', () => {
  const a=scanRust('x.rs','use std::fs as disk; const A: &str = B; const B: &str = A; fn f(){let d = disk;d::write();let (x,y)=pair; x::write();let r=disk;r=disk;r::write();}');
  const fn=a.items.find(i=>i.name==='f').bodyScope;
  assert.equal(a.resolvePath('d::write',fn).path,'std::fs::write');assert.equal(a.resolvePath('x::write',fn).ambiguous,true);
  assert.equal(a.resolvePath('r::write',fn).ambiguous,true);assert.equal(a.resolvePath('A').ambiguous,true);
  assert.equal(scanRust('x.rs','use unknown::*;fn f(){fs::write();}').resolvePath('fs::write').ambiguous,true);
});
test('literals comments raw IDs chars/lifetimes preserve offsets and decode Rust escapes', () => {
  const slash=String.fromCharCode(92);
  const source='/* outer /* inner */ */ fn r#f<\'a>(x: &\'a str) { let a = r###"/*not comment*/"###; let b = "a'+slash+'x41'+slash+'u{1F600}"; let c = \'x\';let b = b"'+slash+'xFF"; }';
  const a=scanRust('x.rs',source);assert.ok(a.tokens.some(t=>t.kind==='lifetime'));assert.ok(a.tokens.some(t=>t.name==='f'));
  assert.equal(a.tokens.find(t=>t.kind==='string'&&t.text.startsWith('"')).value,'aA😀');
  assert.ok(a.tokens.some(t=>t.kind==='byteString'&&t.opaque));
  for(const t of a.tokens)assert.equal(source.slice(t.start,t.end),t.text);
  assert.equal(decodeRustString('"a'+slash+'\n  b"'),'ab');
});
test('malformed lexer/meta/items fail closed; reversal restores scan', () => {
  for(const bad of ['fn f(){','/* no end','fn f(){let s="no end;}','fn f(){let s=r##"bad"#;}','fn f(]{}','#[cfg(any(test,))] fn f(){','#[cfg()] fn f(){}','#[cfg_attr(test)] fn f(){}','fn f()']) {
    assert.doesNotThrow(()=>scanRust('x.rs','fn f(){}'));
    assert.throws(()=>scanRust('x.rs',bad),e=>e.exit===3,bad);
    assert.doesNotThrow(()=>scanRust('x.rs','fn f(){}'));
  }
});
test('compatibility ordinals include tests and homonymous items before exclusions', () => {
  const source='#[test] fn same(){fs::write("x",b"x");} mod m{fn same(){p.join("a");File::open("a");}}';
  const a=scanRust('x.rs',source);assert.deepEqual(a.sites.map(s=>s.ordinal),[1,2,3]);
  assert.equal(a.sites[0].item,'same');assert.ok(a.sites[0].tokens.length);assert.equal(a.sites[0].evidence,source);
  assert.equal(compatibilitySites('x.rs','// fs::write("x","")\nfn f(){ let s="fs::write"; }').length,0);
});
// Documented finite limits: no type/reexport resolution or expansion of arbitrary macros.
test('limitations are signals, never a semantic proof', () => {
  const a=scanRust('x.rs','fn f(){custom!{File::open("x")}; unrelated.open(); hidden_helper();}');
  assert.equal(a.sites.length,2); // Homonymous methods remain suspicious; hidden helper is not inferred.
  assert.equal(a.resolvePath('reexport::File').path,'reexport::File');
});
test('Rust expression/type keywords and opaque macro tokens are not item declarations', () => {
  const a=scanRust('x.rs','struct S { cb: fn(&str)->bool } fn f(p: *const u8) { const { assert!(true); } let x = p as *const u8; helper!{enum identifier ignored_any}; }');
  assert.deepEqual(a.items.map(i=>i.kind),['struct','fn']);
});
test('temporal local shadowing, mutable initialization, literal constant aliases', () => {
  const a=scanRust('x.rs','use std::fs; const NAME: &str = "a.json";fn f(){ fs::write();let fs=local;fs::write();let mut disk=std::fs;disk::write();let name=NAME;}');
  const calls=a.tokens.filter(t=>t.name==='fs'&&a.tokens[t.index+1]?.text==='::'&&a.tokens[t.index+2]?.name==='write');
  assert.equal(a.resolvePath(a.tokens.slice(calls[0].index,calls[0].index+3),calls[0].scope).path,'std::fs::write');
  assert.equal(a.resolvePath(a.tokens.slice(calls[1].index,calls[1].index+3),calls[1].scope).path,'local::write');
  const scope=a.items.find(i=>i.name==='f').bodyScope;
  assert.equal(a.resolvePath('disk::write',scope).ambiguous,false);assert.equal(a.resolvePath('name',scope).value,'a.json');
});
test('missing merge-base/history and successful-but-truncated Git output are ERROR', t => {
  const f=fixture(t);f.git('switch','--orphan','other');f.git('rm','-rf','--ignore-unmatch','.');f.write('other','x');f.commit();
  assert.throws(()=>f.snapshot(f.base),e=>e.exit===3);
  f.git('switch','main');
  for(const command of ['ls-tree','diff']) {
    const runner=(args,opts)=>args[0]===command?{status:0,stdout:Buffer.from('truncated'),stderr:Buffer.alloc(0)}:f.runner(args,opts);
    assert.throws(()=>f.snapshot(f.base,runner),e=>e.exit===3);
  }
});
test('blob diff failure, malformed UTF8 path and executable regular blobs', t => {
  const f=fixture(t);f.write('x.rs','x');const base=f.commit();f.write('x.rs','xx');f.commit();
  const runner=(args,opts)=>args[0]==='diff'&&args.includes('-U0')?{status:1,stdout:Buffer.from('partial'),stderr:Buffer.from('bad diff')}:f.runner(args,opts);
  assert.equal(lengthGate(f.snapshot(base,runner)).exit,3);
  const blob=f.git('rev-parse','HEAD:x.rs');f.git('update-index','--cacheinfo','100755',blob,'x.rs');f.git('-c','core.hooksPath=','commit','-m','mode');
  assert.equal(lengthGate(f.snapshot(base)).exit,0);
  const malformed=Buffer.concat([Buffer.from('100644 blob '+blob+'\t'),Buffer.from([0xff,0])]);
  assert.throws(()=>parseTree(malformed),e=>e.exit===3);
  const n=f.git('rev-parse','HEAD');assert.equal(parseTree(Buffer.from('100644 blob '+blob+'\ta:b.rs\0')).get('a:b.rs').oid,blob);assert.ok(n);
});
test('a copied long-line file fails independently; deleting the copy reverts to PASS', t => {
  const f=fixture(t);f.write('original.rs',text(1,101));const base=f.commit();assert.equal(lengthGate(f.snapshot(base)).exit,0);
  f.write('copy.rs',text(1,101));f.commit();assert.equal(lengthGate(f.snapshot(base)).exit,1);
  f.git('rm','copy.rs');f.commit();assert.equal(lengthGate(f.snapshot(base)).exit,0);
});
test('file root inner cfg and function-local module shadowing do not leak', () => {
  const a=scanRust('x.rs','#![cfg(test)] fn a(){}');assert.equal(a.items[0].testOnly,true);
  const b=scanRust('x.rs','use std::fs;fn f(){mod fs{} fs::write();}fn p(){fs::write();}');
  assert.equal(b.resolvePath('fs::write',b.items.find(i=>i.name==='f').bodyScope).shadowed,true);
  assert.equal(b.resolvePath('fs::write',b.items.find(i=>i.name==='p').bodyScope).path,'std::fs::write');
});
test('ERROR dominates FAIL and exact path identity is preserved', t => {
  const f=fixture(t);f.write('a.rs',text(1,101));f.write('z.rs',Buffer.from([0xff]));f.commit();
  assert.equal(lengthGate(f.snapshot()).exit,3);
  const oid=f.git('rev-parse','HEAD:a.rs');
  const names=['A.rs','a.rs','é.rs','é.rs','-a b:😀.rs'];
  const map=parseTree(Buffer.from(names.map(n=>'100644 blob '+oid+'\t'+n+'\0').join('')));
  assert.equal(map.size,names.length);for(const n of names)assert.ok(map.has(n));
});
test('opaque byte literals still validate escapes; Unicode identifiers and arbitrary raw hashes', () => {
  const slash=String.fromCharCode(92);
  assert.throws(()=>scanRust('x.rs','fn f(){let b=b"'+slash+'xFF'+slash+'q";}'),e=>e.exit===3);
  const a=scanRust('x.rs','fn 𐐀(){let s=r########"raw"########;}');assert.equal(a.items[0].name,'𐐀');
});
test('cfg_attr expression-valued metadata stays production; invalid char escape errors', () => {
  const a=scanRust('x.rs','#[cfg_attr(feature="x",doc=concat!("one","two"),test)] fn f(){}');
  assert.equal(a.items[0].testOnly,false);assert.ok(a.attributes[0].signals.some(s=>s.name==='test'));
  const slash=String.fromCharCode(92);assert.throws(()=>lexRust('x.rs',"fn f(){let c='"+slash+"q';}"),e=>e.exit===3);
});

test('numeric tokens preserve ranges, tuple calls and Rust numeric literals', () => {
  const source = 'fn f(){for i in 0..MAX { self.0.write_all(b); } for j in 1..=LIMIT { x.0.1.join("a"); state.1.push(v); }}';
  const a = scanRust('x.rs', source);
  const range = a.tokens.findIndex(t => t.text === '0');
  assert.deepEqual(a.tokens.slice(range, range + 3).map(t => t.text), ['0', '..', 'MAX']);
  const inclusive = a.tokens.findIndex(t => t.text === '..=');
  assert.deepEqual(a.tokens.slice(inclusive - 1, inclusive + 2).map(t => t.text), ['1', '..=', 'LIMIT']);
  for (const name of ['write_all', 'join', 'push']) {
    assert.ok(a.productionTokens.some(t => t.name === name), name);
    assert.ok(a.sites.find(s => s.operation.includes(name)).tokens.some(t => t.name === name), name);
  }
  assert.ok(a.tokens.some((t, i) => t.text === '0' && a.tokens[i + 1]?.text === '.' && a.tokens[i + 2]?.text === '1'));
  for (const literal of ['1.5f32', '1e-3', '1E+3f64', '0x1F', '0xff_u8', '0b1010u8', '0o77', '1_000usize', '1.']) {
    const tokens = lexRust('x.rs', literal).tokens;
    assert.equal(tokens.length, 1, literal); assert.equal(tokens[0].kind, 'number'); assert.equal(tokens[0].text, literal);
  }
  for (const t of a.tokens) assert.equal(source.slice(t.start, t.end), t.text);
});

test('const generic parameters are not items; actual body constants stay visible', () => {
  const sources = [
    'impl<const N: usize> Foo<N> { const VALUE: usize = 1; fn a(){} }',
    'struct A<const N: usize> { x: [u8; N] }',
    'struct Tuple<T, const N: usize = { 2 + 1 }>([T; N]);',
    'trait A<T, const N: usize> { const VALUE: usize; fn a(); }',
    'enum A<const N: usize> { V([u8; N]) }',
    'union A<const N: usize> { x: [u8; N] }',
    'type A<const N: usize> = [u8; N];',
    'fn a<T, const N: usize>() { const VALUE: usize = 1; }',
  ];
  for (const source of sources) {
    const a = scanRust('x.rs', source);
    assert.ok(!a.items.some(i => i.kind === 'const' && i.name === 'N'), source);
    if (source.includes('VALUE')) assert.ok(a.items.some(i => i.kind === 'const' && i.name === 'VALUE'), source);
    if (source.includes('fn a')) assert.ok(a.items.some(i => i.kind === 'fn' && i.name === 'a'), source);
  }
  assert.throws(() => scanRust('x.rs', 'impl<const N: usize> Foo<N> { const VALUE: usize }'), e => e.exit === 3);
});

test('absolute test attributes resolve without shadowing or hiding later production', () => {
  for (const attribute of ['::tokio::test', '::async_std::test']) {
    const a = scanRust('x.rs', `mod tokio {} #[${attribute}] async fn t(){work();} fn production(){work();}`);
    const t = a.items.find(i => i.name === 't'), p = a.items.find(i => i.name === 'production');
    assert.equal(t.directTest, true); assert.equal(a.isTestOffset(t.bodyStart + 1), true);
    assert.equal(a.isTestOffset(t.start), false); assert.equal(p.testOnly, false);
    assert.ok(a.productionTokens.some(token => token.name === 'production'));
    const path = lexRust('x.rs', attribute).tokens;
    assert.equal(a.resolvePath(path).path, attribute.slice(2));
  }
  const a = scanRust('x.rs', '#[cfg_attr(unix,::tokio::test)] fn production(){}');
  assert.equal(a.items[0].testOnly, false); assert.ok(a.attributes[0].signals.some(s => s.name === '::tokio::test'));
  assert.throws(() => scanRust('x.rs', '#[::] fn t(){}'), e => e.exit === 3);
});

// P02 uses the same snapshot contract. Pure tree controls keep every subrule's
// counterfactual explicit; real Git controls below exercise the landed OID API.
import { runGate as layoutGate } from './sanitizing_file_layout.mjs';
import { runGate as edgeGate } from './sanitizing_tauri_edge.mjs';
import { runGate as globalsGate } from './sanitizing_global_state.mjs';
import { runGate as processGate } from './sanitizing_process_env.mjs';
const rustRoot = 'src-tauri/src/';
function trees(base, head = base) {
  const tree = files => new Map(Object.entries(files).map(([path, text]) => [path,{path,text,mode:'100644',type:'blob'}]));
  return { baseTree: tree(base), headTree: tree(head), baseOid: 'base-fixture', headOid: 'head-fixture',
    refOid: 'ref-fixture', requestedRef:'fixture', readBlob: e => e?.text ?? '', assertStableHead() {} };
}
function control(gate, rule, name, before, mutation) {
  const p = rustRoot + name, base = {[p]:before};
  assert.equal(gate(trees(base)).exit,0);
  const result = gate(trees(base,{[p]:mutation}));
  assert.equal(result.exit,1,JSON.stringify(result));
  assert.ok(result.diagnostics.some(d=>d.status==='FAIL' && d.rule===rule && d.line>0 && d.path===p && d.remedy),JSON.stringify(result));
  assert.equal(gate(trees(base,base)).exit,0);
  return result;
}
test('05 every independent counter: PASS → FAIL with diagnostic → restored PASS', () => {
  for (const [rule,p,clean,bad] of [
    ['mod_rs','mod.rs','', 'fn f() {}'],
    ['test_files','x.rs','fn f() {}', '#[test]\nfn f() {}'],
    ['inline_mod','x.rs','mod x;', 'mod x {}'],
    ['path','x.rs','mod x;', '#[path="other.rs"]\nmod x;'],
    ['glob','x.rs','use a::b;', 'use a::{b::*,c::*};'],
    ['pub_mod','x.rs','mod x;', 'pub\nmod x;'],
    ['pub_super_mod','x.rs','mod x;', 'pub(super)\nmod x;'],
    ['pub_in_mod','x.rs','mod x;', 'pub(in crate::a)\nmod x;'],
  ]) {
    if (rule==='mod_rs') {
      const old = rustRoot+'old.rs', added=rustRoot+p;
      assert.equal(layoutGate(trees({[old]:clean})).exit,0);
      const r=layoutGate(trees({[old]:clean},{[old]:clean,[added]:bad}));
      assert.equal(r.exit,1); assert.ok(r.diagnostics.some(d=>d.rule===rule));
      assert.equal(layoutGate(trees({[old]:clean})).exit,0);
    } else control(layoutGate,rule,p,clean,bad);
  }
});
test('05 grouped globs, test/path cfg_attr, async aliases, integration and visibility boundaries', () => {
  for(const attr of ['#[tokio::test]','#[async_std::test]','use tokio::test as check; #[check]','#[cfg_attr(feature="x",tokio::test)]'])
    control(layoutGate,'test_files','x.rs','fn f() {}',attr+' async fn f() {}');
  control(layoutGate,'path','x.rs','mod a;','#[cfg_attr(unix,path="a.rs")] mod a;');
  const m=layoutGate(trees({}, {[rustRoot+'tests.rs']:'use super::*; #[test] fn f() {}'}));
  assert.equal(m.exit,0,JSON.stringify(m));
  assert.equal(layoutGate(trees({}, {[rustRoot+'tests/x.rs']:'#[test] fn f() {}'})).exit,0);
  assert.equal(layoutGate(trees({}, {'src-tauri/tests/it.rs':'#[test] fn f() {}'})).exit,0);
  control(layoutGate,'glob','tests.rs','use super::*;','use super::*; use x::*;');
  // C6: foo/tests/<noun>.rs is part of foo/tests.rs; foo/x_tests.rs is not.
  const c6=layoutGate(trees({}, {[rustRoot+'foo/tests/x.rs']:'use super::*; #[test] fn f() {}'}));
  assert.equal(c6.exit,0,JSON.stringify(c6));
  control(layoutGate,'glob','foo/tests/x.rs','use super::*;','use super::*; use x::*;');
  control(layoutGate,'glob','foo/x_tests.rs','fn f() {}','use super::*;');
  control(layoutGate,'test_files','foo/x_tests.rs','fn f() {}','#[test] fn f() {}');
  assert.equal(layoutGate(trees({}, {[rustRoot+'lib.rs']:'pub mod a; pub(crate) mod b;'})).exit,0);
  const r=layoutGate(trees({}, {[rustRoot+'x.rs']:'use a::{b::*,c::*,d::{e::*,f}};'}));
  assert.equal(r.totals[1].glob,3);
});
test('05 debt equal/improved/worse and strict compensated new file, C8 ratchet only', () => {
  const p=rustRoot+'old.rs',q=rustRoot+'new.rs',bad='mod a {}';
  assert.equal(layoutGate(trees({[p]:bad},{[p]:''})).exit,0);
  control(layoutGate,'inline_mod','old.rs',bad,bad+'mod b {}');
  assert.equal(layoutGate(trees({[p]:bad},{[p]:'',[q]:bad})).exit,1);
  // Visibility has a global ratchet, not an invented strict new-file prohibition.
  assert.equal(layoutGate(trees({[p]:'pub mod a;'},{[q]:'pub mod a;'})).exit,0);
});
test('06 every counter and exact boundary, alias imports and homonymous methods', () => {
  for(const [rule,bad] of [
    ['tauri_files','fn f(x: tauri::AppHandle) {}'],
    ['emit_files','fn f(x: X) { x.emit("event",0); }'],
    ['state_files','fn f(x: X) { x.state::<X>(); }'],
    ['command_files','#[cfg_attr(unix,tauri::command)] fn f() {}'],
    ['disk_commands','#[tauri::command] fn f() { std::fs::read("a"); }'],
  ]) control(edgeGate,rule,'x.rs','fn f() {}',bad);
  for(const p of ['lib.rs','main.rs','commands.rs','events.rs','commands/a.rs','events/a.rs'])
    assert.equal(edgeGate(trees({}, {[rustRoot+p]:'fn f(x: tauri::AppHandle) { x.emit("a",0); x.state::<X>(); }'})).exit,0,p);
  for(const p of ['cli/a.rs','api/a.rs','web/a.rs','window.rs'])
    control(edgeGate,'tauri_files',p,'','use tauri::AppHandle as Handle; fn f(x: Handle) {}');
  control(edgeGate,'disk_commands','commands.rs','use tauri::command as cmd; #[cmd] fn f() {}',
    'use tauri::command as cmd; use std::fs::read as disk; #[cmd] fn f() { disk("a"); }');
});
test('06 new disk command in existing file cannot hide behind reductions; cfg identity and renames', () => {
  const p=rustRoot+'commands.rs';
  const old='#[tauri::command] fn old() { fs::read("a"); }';
  for(const bad of ['#[tauri::command] fn fresh() { fs::read("a"); }',
    '#[cfg(unix)] #[tauri::command] fn old() { fs::read("a"); } #[cfg(windows)] #[tauri::command] fn old() { fs::read("a"); }']) {
    const r=control(edgeGate,'new_disk_command','commands.rs',old,bad);
    assert.ok(r.diagnostics.some(d=>d.rule==='new_disk_command'));
  }
  assert.equal(edgeGate(trees({[p]:old},{[p]:old.replace('fs::read("a");','')})).exit,0);
  assert.equal(edgeGate(trees({}, {[p]:'#[tauri::command] fn f() { #[cfg(test)] fn test_only() { fs::read("a"); } }'})).exit,0);
});
test('08 static mutation, state wrappers/aliases/multiline and thread_local each reject increase', () => {
  for(const bad of [
    'static mut X: usize = 0;', 'static X: std::sync::Mutex<u8> = Mutex::new(0);',
    'static X: Arc<RwLock<u8>> = init();','static X: AtomicUsize = init();',
    'static X: OnceCell<X> = init();','static X: Lazy<X> = init();',
    'use std::sync::Mutex as M; static\nX: M<u8> = M::new(0);',
    'thread_local! { static X: Cell<u8> = Cell::new(0); }',
    'thread_local! { static X: RefCell<u8> = RefCell::new(0); }',
  ]) control(globalsGate,'globals','x.rs','',bad);
  const p=rustRoot+'old.rs',q=rustRoot+'new.rs',bad='static X: Mutex<u8> = init();';
  assert.equal(globalsGate(trees({[p]:bad},{[p]:''})).exit,0);
  control(globalsGate,'globals','old.rs',bad,bad+'static Y: OnceLock<X> = init();');
  assert.equal(globalsGate(trees({[p]:bad},{[q]:bad})).exit,0);
  assert.equal(globalsGate(trees({[p]:bad},{[q]:bad.replace('X:', 'Y:')})).exit,1);
  assert.equal(globalsGate(trees({}, {[p]:'static N: usize = 0; static S: &str = "x";'})).exit,0);
});
const regexImports='use std::sync::{LazyLock,OnceLock}; use regex::Regex; ';
const lazyRegex='static RE: LazyLock<Regex> = LazyLock::new(|| { Regex::new("a").unwrap() });';
const onceRegex='fn f() { static RE: OnceLock<Regex> = OnceLock::new(); RE.get_or_init(|| Regex::new("a").expect("valid")); }';
test('08 only demonstrated literal Regex initialization/usage is exempt', () => {
  for(const source of [lazyRegex,onceRegex,onceRegex.replace('RE.get_or_init','RE.get_or_init'),
    lazyRegex.replace('unwrap()','expect("valid")'),lazyRegex.replace('|| {','|| {{{').replace(' });','}} });')]) {
    const r=globalsGate(trees({}, {[rustRoot+'x.rs']:regexImports+source}));
    assert.equal(r.exit,0,JSON.stringify(r));
  }
  const variants=[
    lazyRegex.replace('"a"','runtime()'),lazyRegex.replace('Regex>','Mutex<Regex>>'),
    lazyRegex.replace('Regex::new','let x=0; Regex::new'),lazyRegex.replace('||','|x|'),
    onceRegex.replace(' RE.get_or_init(|| Regex::new("a").expect("valid"));',''),
    onceRegex.replace('get_or_init','get_or_try_init'),onceRegex.replace('RE.get_or_init','RE.set'),
    onceRegex.replace('RE.get_or_init','take(&RE); RE.get_or_init'),
    onceRegex.replace('Regex::new("a")','Regex::new(runtime())'),
    'static RE: OnceLock<Regex> = OnceLock::new();',
    'static RULES: OnceLock<CompiledRules> = OnceLock::new();',
    lazyRegex.replace('LazyLock','OnceCell'),
  ];
  for(const bad of variants) control(globalsGate,'globals','x.rs',regexImports+lazyRegex,regexImports+bad);
  const alias='use std::sync::LazyLock as L; use regex::Regex as R; static RE: L<R> = L::new(|| R::new(r"a").unwrap());';
  assert.equal(globalsGate(trees({}, {[rustRoot+'x.rs']:alias})).exit,0);
});
test('09 independent counters, absolute write, exact ownership and aliases/shadow', () => {
  control(processGate,'launch_files','x.rs','fn f() {}','use tokio::process::Command as C; fn f() { C::new("x"); }');
  control(processGate,'env_read_files','x.rs','fn f() {}','use std::env::var as read; fn f() { read("x"); }');
  control(processGate,'env_write','x.rs','fn f() {}','use std::env as e; fn f() { e::set_var("x","y"); }');
  for(const p of ['process.rs','process/a.rs','pty/mod.rs','pty/a.rs']) {
    assert.equal(processGate(trees({}, {[rustRoot+p]:'fn f() { Command::new("x").env("x","y"); }'})).exit,0);
    control(processGate,'env_read_files',p,'','fn f() { std::env::var_os("x"); }');
    control(processGate,'env_write',p,'','fn f() { std::env::remove_var("x"); }');
  }
  for(const p of ['config.rs','config/a.rs','lib.rs','main.rs']) {
    assert.equal(processGate(trees({}, {[rustRoot+p]:'fn f() { std::env::var("x"); }'})).exit,0);
    control(processGate,'launch_files',p,'','fn f() { std::process::Command::new("x"); }');
  }
  assert.equal(processGate(trees({}, {[rustRoot+'main.rs']:'fn f() { std::env::set_var("x","y"); }'})).exit,0);
  const bad={[rustRoot+'pty/a.rs']:'fn f() { env::set_var("x","y"); }'};
  assert.equal(processGate(trees(bad)).exit,1);
  const shadow='use std::env::var as read; fn f(read: F) { read("x"); }';
  assert.equal(processGate(trees({}, {[rustRoot+'x.rs']:shadow})).exit,0);
});
test('P02 production predicates preserve following code; comments/strings and tests do not signal', () => {
  for(const [gate,bad,rule] of [[edgeGate,'tauri::AppHandle::new();','tauri_files'],
    [globalsGate,'static X: Mutex<X> = init();','globals'],[processGate,'env::var("x");','env_read_files']]) {
    for(const cfg of ['test','all(test,unix)','any(test,test)']) {
      const s=`#[cfg(${cfg})] fn test_only() { ${bad} }`;
      assert.equal(gate(trees({}, {[rustRoot+'x.rs']:s})).exit,0);
      control(gate,rule,'x.rs',s,s+` fn production() { ${bad} }`);
    }
    for(const cfg of ['any(has_embedded_dist,test)','not(test)','all()','any()'])
      control(gate,rule,'x.rs','',`#[cfg(${cfg})] fn production() { ${bad} }`);
    control(gate,rule,'x.rs','',`#[cfg_attr(unix,cfg(test))] fn production() { ${bad} }`);
    for(const attr of ['#[test]','#[tokio::test]','#[async_std::test]','use tokio::test as check; #[check]']) {
      const s=`${attr} async fn test_fn() { ${bad} }`;
      assert.equal(gate(trees({}, {[rustRoot+'x.rs']:s})).exit,0);
      control(gate,rule,'x.rs',s,s+` fn production() { ${bad} }`);
    }
    const inert='// '+bad+'\nfn f() { let x = r###"'+bad+'"###; }';
    assert.equal(gate(trees({}, {[rustRoot+'x.rs']:inert})).exit,0);
    for(const p of ['tests.rs','x_tests.rs','tests/a.rs']) assert.equal(gate(trees({}, {[rustRoot+p]:`fn f() { ${bad} }`})).exit,0);
  }
});
test('09 moved occurrences pass; compensated new code and copies fail', () => {
  for(const [rule,code] of [['launch_files','Command::new("x");'],['env_read_files','env::var("x");']]) {
    const p=rustRoot+'old.rs',q=rustRoot+'new.rs',before=`fn f() { ${code} }`;
    assert.equal(processGate(trees({[p]:before},{[p]:''})).exit,0);
    const r=processGate(trees({[p]:before},{[q]:before}));
    assert.equal(r.exit,0,JSON.stringify(r));
    const changed=processGate(trees({[p]:before},{[q]:before.replace('fn f()', 'fn fresh()')}));
    assert.equal(changed.exit,1); assert.ok(changed.diagnostics.some(d=>d.rule===rule));
    const up=processGate(trees({[p]:before},{[p]:before,[q]:before}));
    assert.equal(up.exit,1); assert.ok(up.diagnostics.some(d=>d.rule===rule));
  }
});
test('P02 actual Git OID mutation/reversion and ERROR precedence for every detector', t => {
  const f=fixture(t);
  for(const [gate,p,code] of [[layoutGate,'layout.rs','mod inline {}'],[edgeGate,'edge.rs','fn f(x: tauri::AppHandle) {}'],
    [globalsGate,'global.rs','static X: Mutex<X> = init();'],[processGate,'env.rs','fn f() { env::var("x"); }']]) {
    sequence(f,gate,rustRoot+p,'',code);
    const s=f.snapshot(); s.assertStableHead=()=>{throw Error('HEAD drift');};
    assert.equal(gate(s).exit,3);
    const malformed=trees({}, {[rustRoot+p]:'#[cfg(test,unix)] fn f() {}'});
    assert.equal(gate(malformed).exit,3);
    const broken=trees({}, {[rustRoot+p]:''});broken.readBlob=()=>{throw Error('invalid UTF8/mode/blob');};
    assert.equal(gate(broken).exit,3);
  }
});

test('P02 relevant ambiguous aliases count with diagnostics instead of erasing the previous import', () => {
  for(const [gate,rule,source] of [
    [processGate,'env_read_files','use std::env::var as read; use other::read; fn f() { read("x"); }'],
    [edgeGate,'tauri_files','use tauri::AppHandle as Handle; use other::Handle; fn f(x: Handle) {}'],
    [globalsGate,'globals','use std::sync::Mutex as M; use other::M; static X: M<u8> = init();'],
    [globalsGate,'globals','use std::sync::atomic::AtomicUsize as A; use other::A; static X: A = init();'],
    [globalsGate,'globals','#[cfg(unix)] use std::sync::atomic::AtomicU64 as Ctr; #[cfg(not(unix))] use other::Ctr; static X: Ctr = Ctr::new(0);'],
    [layoutGate,'test_files','use tokio::test as check; use other::check; #[check] fn f() {}'],
  ]) {
    const r=control(gate,rule,'x.rs','',source);
    assert.ok(r.diagnostics.some(d=>d.status==='WARN'),JSON.stringify(r));
  }
});

test('P02 absolute paths retain syntactic signals', () => {
  for(const [gate,rule,code] of [[edgeGate,'tauri_files','fn f(x: ::tauri::AppHandle) {}'],
    [globalsGate,'globals','static X: ::std::sync::Mutex<u8> = init();'],
    [processGate,'env_read_files','fn f() { ::std::env::var("x"); }'],
    [processGate,'launch_files','fn f() { ::tokio::process::Command::new("x"); }']]) control(gate,rule,'x.rs','',code);
});
test('06 existing command may gain disk when another loses it; only NEW command has strict condition', () => {
  const p=rustRoot+'commands.rs';
  const base='#[tauri::command] fn a() { fs::read("a"); } #[tauri::command] fn b() {}';
  const head='#[tauri::command] fn a() {} #[tauri::command] fn b() { fs::read("a"); }';
  const r=edgeGate(trees({[p]:base},{[p]:head}));assert.equal(r.exit,0,JSON.stringify(r));
});

test('08 absolute readonly Regex aliases and conservative lock escape/alias/init cases', () => {
  const absolute='static RE: ::std::sync::LazyLock<::regex::Regex> = ::std::sync::LazyLock::new(|| ::regex::Regex::new("a").unwrap());';
  assert.equal(globalsGate(trees({}, {[rustRoot+'x.rs']:absolute})).exit,0);
  for(const bad of [onceRegex.replace('RE.get_or_init','let alias = &RE; alias.get_or_init'),
    onceRegex.replace('OnceLock::new()','runtime()'),onceRegex.replace('static RE:','pub static RE:'),
    onceRegex.replace('RE.get_or_init','let alias = RE; alias.get_or_init')])
    control(globalsGate,'globals','x.rs',regexImports+onceRegex,regexImports+bad);
});
test('P02 explicit aliases obey local shadowing and grouped imports', () => {
  const cases=[
    [processGate,'use std::{env::{self as e,var as read},process::Command as C}; fn f(){ e::var("x"); read("x"); C::new("x"); }'],
    [edgeGate,'use tauri::{self as t,AppHandle as H}; fn f(x: H){ t::f(); }'],
    [globalsGate,'use std::sync::{Mutex as M,RwLock as R}; static X: Option<M<R<u8>>> = init();'],
  ];
  for(const [gate,source] of cases) assert.equal(gate(trees({}, {[rustRoot+'x.rs']:source})).exit,1);
  for(const [gate,source] of [
    [processGate,'use std::process::Command; fn f(){ struct Command; Command::new("x"); }'],
  ]) assert.equal(gate(trees({}, {[rustRoot+'x.rs']:source})).exit,0);
  assert.equal(edgeGate(trees({}, {[rustRoot+'commands.rs']:'use std::fs::read; #[tauri::command] fn f(read: F) { read("x"); }'})).exit,0);
  assert.equal(layoutGate(trees({}, {[rustRoot+'x.rs']:'// mod a {} #[test]\nfn f(){ let x="use a::*;"; }'})).exit,0);
});

// #3182: sanitizing.allowlist.json turns listed FAILs into ACCEPTED; an unused entry only warns.
import { applyAllowlist, parseAllowlist } from './sanitizing_baseline.mjs';
const allow = (gate, rule, file) => ({ gate, rule, path: file, reason: 'moved code', issue: 3182, since: '2026-10-23' });
test('allowlist: accepted, stale entry and unlisted finding', () => {
  const p = rustRoot + 'moved.rs', q = rustRoot + 'other.rs', moved = 'use std::sync::Mutex; static M: Mutex<u8> = Mutex::new(0);';
  const result = globalsGate(trees({}, {[p]: moved, [q]: moved}));
  assert.equal(result.exit, 1, JSON.stringify(result));
  const accepted = applyAllowlist(globalsGate(trees({}, {[p]: moved})), [allow('08', 'globals', p)]);
  assert.equal(accepted.exit, 0, JSON.stringify(accepted));
  assert.equal(accepted.accepted, 2);
  assert.ok(accepted.diagnostics.every(d => d.status !== 'FAIL'));
  const unlisted = applyAllowlist(result, [allow('08', 'globals', p)]);
  assert.equal(unlisted.exit, 1);
  assert.ok(unlisted.diagnostics.some(d => d.status === 'FAIL' && d.path === q));
  const stale = applyAllowlist(globalsGate(trees({[p]: moved})), [allow('08', 'globals', p), allow('06', 'tauri_files', q)]);
  assert.equal(stale.exit, 0, JSON.stringify(stale));
  assert.equal(stale.status, 'PASS');
  assert.ok(stale.diagnostics.every(d => d.status !== 'FAIL'));
  assert.deepEqual(stale.diagnostics.filter(d => d.status === 'WARN').map(d => [d.rule, d.path]), [['stale_allowlist', p]]);
  assert.equal(stale.accepted, 0);
  assert.equal(stale.warnings, 1);
  const mixed = applyAllowlist(result, [allow('08', 'globals', rustRoot + 'gone.rs')]);
  assert.equal(mixed.exit, 1, 'a stale entry never hides a real FAIL');
  assert.equal(mixed.warnings, 1);
});
test('allowlist: text gates carry a rule and accept a listed size exception', () => {
  const name = 'src-tauri/src/big.rs', base = { [name]: text(1001) }, head = { [name]: text(1002) };
  const result = applyAllowlist(sizeGate({ ...trees(base, head), changed: [{ status: 'M', path: name }] }), [allow('02', 'file_lines', name)]);
  assert.equal(result.exit, 0, JSON.stringify(result));
  assert.equal(result.accepted, 1);
});
test('allowlist: schema refuses malformed and duplicate entries', () => {
  const doc = entries => JSON.stringify({ version: 1, entries });
  assert.deepEqual(parseAllowlist(doc([allow('08', 'globals', 'a.rs')])).length, 1);
  for (const bad of ['{', JSON.stringify({ entries: [] }), doc([{ ...allow('08', 'globals', 'a.rs'), gate: '99' }]),
    doc([{ ...allow('08', 'globals', 'a.rs'), issue: 'x' }]), doc([{ ...allow('08', 'globals', 'a.rs'), hash: 'abc' }]),
    doc([allow('cog', 'new', 'a.rs')]), doc([allow('08', 'globals', 'a.rs'), allow('08', 'globals', 'a.rs')])])
    assert.throws(() => parseAllowlist(bad));
});

// Batch 7: occurrence totals are invariant under splits, with one-use transfer credit.
test('06/08/09 splits preserve debt; extra occurrences in existing/new files fail', () => {
  const p=rustRoot+'old.rs', q=rustRoot+'new.rs', r=rustRoot+'copy.rs';
  for (const [gate,rule,a,b,extra] of [
    [edgeGate,'tauri_files','fn a(x: tauri::AppHandle) {}','fn b(x: tauri::AppHandle) {}','fn c(x: tauri::AppHandle) {}'],
    [edgeGate,'emit_files','fn a(x: X) { x.emit("a",0); }','fn b(x: X) { x.emit_to("b",0); }','fn c(x: X) { x.emit("c",0); }'],
    [edgeGate,'state_files','fn a(x: X) { x.state::<X>(); }','fn b(x: X) { x.state::<X>(); }','fn c(x: X) { x.state::<X>(); }'],
    [edgeGate,'command_files','#[tauri::command] fn a() {}','#[tauri::command] fn b() {}','#[tauri::command] fn c() {}'],
    [globalsGate,'globals','static A: Mutex<u8> = init();','static B: Mutex<u8> = init();','static C: Mutex<u8> = init();'],
    [processGate,'launch_files','fn a() { Command::new("a"); }','fn b() { Command::new("b"); }','fn c() { Command::new("c"); }'],
    [processGate,'env_read_files','fn a() { env::var("a"); }','fn b() { env::var_os("b"); }','fn c() { env::var("c"); }'],
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
  const a='#[tauri::command] fn a() { fs::read("a"); }', b='#[tauri::command] fn b() { fs::read("b"); }';
  const base={[p]:a+b};
  assert.equal(edgeGate(trees(base,{[p]:a,[q]:b})).exit,0);
  for (const head of [{[p]:a,[q]:b.replace('fn b()', 'fn fresh()')},{[p]:a+b,[q]:b}])
    assert.equal(edgeGate(trees(base,head)).exit,1);
});
test('transfer matching reserves retained debt before spending removed occurrences', () => {
  const p=rustRoot+'a.rs',q=rustRoot+'b.rs',r=rustRoot+'c.rs',code='static A: Mutex<u8> = init();';
  const result=globalsGate(trees({[p]:code,[q]:code},{[r]:code,[q]:code}));
  assert.equal(result.exit,0,JSON.stringify(result));
  assert.equal(globalsGate(trees({[p]:code,[q]:code},{[r]:code+code,[q]:code})).exit,1);
});
test('moved aliases require matching bindings and production ownership', () => {
  const p=rustRoot+'a.rs',q=rustRoot+'b.rs',code='fn f(){ C::new("a"); }';
  const imports='use std::process::Command as C;';
  assert.equal(processGate(trees({[p]:imports+code},{[q]:imports+'\n// moved\n'+code})).exit,0);
  assert.equal(processGate(trees({[p]:imports+'#[cfg(test)] '+code},{[q]:imports+code})).exit,1);
  assert.equal(processGate(trees({[rustRoot+'process.rs']:imports+code},{[q]:imports+code})).exit,1);
  assert.equal(processGate(trees({[p]:imports+code},{[q]:'use tokio::process::Command as C;'+code})).exit,1);
});
test('occurrence increases still apply path-scoped allowlist without hiding unlisted debt', () => {
  const p=rustRoot+'a.rs',q=rustRoot+'b.rs',code='fn f(){ env::var("a"); }';
  const result=processGate(trees({[p]:code},{[p]:code+code}));
  assert.equal(result.exit,1);
  assert.equal(applyAllowlist(result,[allow('09','env_read_files',p)]).exit,0);
  const unlisted=processGate(trees({[p]:code},{[p]:code,[q]:code}));
  assert.equal(applyAllowlist(unlisted,[allow('09','env_read_files',p)]).exit,1);
});

import { formatWarning } from './sanitizing_baseline.mjs';
test('05 glob ambiguity warnings are distinct from stale allowlist warnings', () => {
  const p=rustRoot+'glob.rs',result=layoutGate(trees({}, {[p]:'use unknown::*; #[test] fn f() {}'}));
  const warning=result.diagnostics.find(d=>d.status==='WARN'); assert.ok(warning);
  assert.match(formatWarning(warning),/^WARN scan ambiguity:/);
  assert.doesNotMatch(formatWarning(warning),/stale allowlist/);
  assert.match(formatWarning({gate:'05',path:p,rule:'stale_allowlist',reason:'unused'}),/^WARN stale allowlist entry:/);
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
