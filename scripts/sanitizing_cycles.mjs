// 03 reuses the existing working-tree wrappers and ratchets immutable Git blobs.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';
import { GateError, codeOrder, gitRunner, gateCli, isMain, decode } from './sanitizing_baseline.mjs';
import { scanRust, lexRust } from './sanitizing_rust_scan.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BASELINE = 'src-tauri/.rust-cycles-baseline.json';
const RECORD = 'src-tauri/module-arcs.txt';
const excluded = new Set(['target','.git','node_modules','.direnv','.cargo','vendor']);
const traversed = p => p.startsWith('src-tauri/') && !p.split('/').some(s => excluded.has(s)) &&
  (p.endsWith('.rs') || path.posix.basename(p) === 'Cargo.toml');
const support = p => p === 'package.json' || p === BASELINE || p === RECORD ||
  /^scripts\/sanitizing_.*\.mjs$/.test(p) || p === 'scripts/containers.json' ||
  ['scripts/check-module-cycles.mjs','scripts/check-module-arcs.mjs','scripts/01-rust_module-dependency-cycles.mjs',
    'scripts/02-module-arc-record.mjs','scripts/gate-support.mjs'].includes(p);

export function readCycles(text) {
  let value;
  try { value = JSON.parse(text); } catch { throw new GateError('03 baseline invalid JSON',2); }
  const from = value.generatedFrom;
  if (value.schemaVersion !== 1 || value.kind !== 'rust-cycles-baseline' || from?.toolVersion !== '1.1.1' ||
      from.functionScope !== 'cross-module' || from.includeTests !== false || !Array.isArray(value.moduleCycles))
    throw new GateError('03 baseline incompatible schema/settings',2);
  const seen = new Set(), ids = new Set();
  return value.moduleCycles.map(c => {
    if (typeof c.id !== 'string' || !c.id || ids.has(c.id) || !Array.isArray(c.members) || !c.members.length)
      throw new GateError('03 invalid/duplicate SCC id or empty members',2);
    ids.add(c.id);
    for (const member of c.members) {
      if (typeof member !== 'string' || !member || seen.has(member))
        throw new GateError('03 empty/duplicate SCC member',2);
      seen.add(member);
    }
    return { id:c.id, members:new Set(c.members) };
  });
}
export function readArcs(text) {
  if (text.includes('\r') || text && !text.endsWith('\n')) throw new GateError('03 record requires LF/trailing LF',2);
  let previous; const arcs = [];
  for (const line of text.split('\n').slice(0,-1)) {
    const match = /^(\S+) -> (\S+)$/.exec(line);
    if (!match || match[1] === match[2] || previous !== undefined && codeOrder(previous,line) >= 0)
      throw new GateError('03 malformed/self/unsorted/duplicate record pair',2);
    previous = line; arcs.push([match[1],match[2]]);
  }
  return arcs;
}
function required(snapshot,tree,name) {
  if (!tree.has(name)) throw new GateError(`03 missing input ${name}`);
  return snapshot.readBlob(tree.get(name));
}
export function ratchet(snapshot) {
  const before = readCycles(required(snapshot,snapshot.baseTree,BASELINE));
  const after = readCycles(required(snapshot,snapshot.headTree,BASELINE));
  const baseArcs = readArcs(required(snapshot,snapshot.baseTree,RECORD));
  const headArcs = readArcs(required(snapshot,snapshot.headTree,RECORD));
  const diagnostics = [], measurements = [];
  for (const c of before) {
    const count = arcs => arcs.filter(([a,b])=>c.members.has(a)&&c.members.has(b)).length;
    const base = count(baseArcs), head = count(headArcs);
    measurements.push({id:c.id,base,head});
    if (head > base) diagnostics.push({rule:'internal_arcs',status:'FAIL',id:c.id,base,head,
      reason:'internal arc debt increased inside fixed BASE SCC',remedy:'remove new internal arcs'});
  }
  for (const c of after) if (!before.some(b => [...c.members].every(m=>b.members.has(m))))
    diagnostics.push({rule:'scc_members',status:'FAIL',id:c.id,members:[...c.members],
      reason:'HEAD SCC is not a subset of one BASE SCC',remedy:'remove new SCC growth/union'});
  return {diagnostics,measurements};
}

export function preflight(snapshot, {cwd=ROOT, runner=gitRunner}={}) {
  snapshot.assertStableHead();
  const expected = [...snapshot.headTree.keys()].filter(traversed).sort(codeOrder), actual = [];
  const walk = dir => {
    for (const e of fs.readdirSync(path.join(cwd,dir),{withFileTypes:true})) {
      if (excluded.has(e.name)) continue;
      const p = `${dir}/${e.name}`;
      if (e.isSymbolicLink()) throw new GateError(`03 unproved nonregular input ${p}`);
      if (e.isDirectory()) walk(p);
      else if (traversed(p)) {
        if (!e.isFile()) throw new GateError(`03 nonregular input ${p}`);
        actual.push(p);
      }
    }
  };
  walk('src-tauri'); actual.sort(codeOrder);
  if (JSON.stringify(expected)!==JSON.stringify(actual))
    throw new GateError('03 detector traversal differs from HEAD: extra/missing/ignored Rust or manifest');
  const rootFiles=new Set(expected.filter(p=>/\/(?:lib|main|mod)\.rs$/.test(p)||
    /\/(?:bin|tests|benches|examples)\/[^/]+\.rs$/.test(p)));
  for(const manifest of expected.filter(p=>path.posix.basename(p)==='Cargo.toml')) {
    let section='';
    for(const line of snapshot.readBlob(snapshot.headTree.get(manifest)).split('\n')) {
      if(/^\s*\[/.test(line))section=line.trim().replace(/\s*#.*$/,'');
      const m=/^\s*path\s*=\s*["']([^"']+)["']/.exec(line);
      if(m&&section==='[lib]') {
        const target=path.resolve(cwd,path.posix.dirname(manifest),m[1]);
        const rel=path.relative(cwd,target).replaceAll(String.fromCharCode(92),'/');
        if(!expected.includes(rel))throw new GateError(`03 manifest target outside proved traversal ${manifest}: ${m[1]}`);
        rootFiles.add(rel);
      }
    }
  }
  const inputs = [...snapshot.headTree.keys()].filter(p=>traversed(p)||support(p)).sort(codeOrder);
  for (const p of inputs) {
    const entry = snapshot.headTree.get(p);
    snapshot.readBlob(entry);
    if (!fs.lstatSync(path.join(cwd,p)).isFile()) throw new GateError(`03 nonregular working input ${p}`);
    const source = snapshot.readBlob(entry);
    if (p.endsWith('.rs')) {
      const {tokens:lexed}=lexRust(p,source);
      if(!lexed.some((t,i)=>t.name==='path'&&lexed[i-1]?.text==='['&&lexed[i-2]?.text==='#'))continue;
      const scan = scanRust(p,source);
      // Every path override must remain within the proved detector universe. The detector
      // itself resolves inline module bases; reject a lexical escape before it can read host IO.
      for (const a of scan.attributes) if (a.meta.name === 'path') {
        const tokens = scan.tokens.slice(a.first,a.last+1), literal = tokens.find(t=>t.kind==='string');
        if (!literal) throw new GateError(`03 unproved #[path] ${p}:${a.line}`);
        const containing = scan.items.filter(i=>i.kind==='mod'&&i.bodyStart!==null&&i.start<a.start&&i.end>a.end);
        if (containing.some(i=>i.attributes.some(attr=>attr.meta.name==='path')))
          throw new GateError(`03 unproved #[path] ${p}:${a.line}`);
        const basename=path.posix.basename(p);
        const isRoot=rootFiles.has(p);
        const directory=isRoot?path.dirname(p):path.join(path.dirname(p),basename.slice(0,-3));
        const target=path.resolve(cwd,directory,...containing.map(i=>i.name),literal.value);
        const rel=path.relative(cwd,target).replaceAll(String.fromCharCode(92),'/');
        if (!expected.includes(rel)&&!expected.includes(`${rel}/mod.rs`))
          throw new GateError(`03 #[path] outside proved traversal ${p}:${a.line}`);
      }
    }
  }
  const r = runner(['diff','--exit-code',snapshot.headOid,'--',...inputs],{cwd});
  if (r.error || r.signal || r.status!==0) throw new GateError(`03 working inputs differ from HEAD exit=${r.status} signal=${r.signal??'-'} ${r.error?.message??''}`);
  // New executable scripts are also inputs, including ignored files absent from Git.
  for (const e of fs.readdirSync(path.join(cwd,'scripts'),{withFileTypes:true})) {
    const p=`scripts/${e.name}`;
    if (support(p)&&!snapshot.headTree.has(p)) throw new GateError(`03 extra untracked/ignored input ${p}`);
  }
  snapshot.assertStableHead();
}
export function wrapperRunner(script,{cwd=ROOT,timeout=300_000,outputLimit=64*1024*1024}={}) {
  const systemRoot=process.env.SystemRoot??process.env.SYSTEMROOT;
  if(process.platform==='win32'&&!systemRoot)throw new GateError('03 SystemRoot unavailable for bounded process-tree cleanup');
  return new Promise(resolve=>{
    const child=spawn(process.execPath,[path.join(cwd,script)],{cwd,stdio:['ignore','pipe','pipe'],
      detached:process.platform!=='win32',windowsHide:true});
    const stdout=[],stderr=[];let bytes=0,error,cleanup,settled=false;
    const finish=(status,signal)=>{
      if(settled)return;settled=true;clearTimeout(timer);clearTimeout(fallback);
      resolve({status,signal,error,cleanup,stdout:Buffer.concat(stdout),stderr:Buffer.concat(stderr)});
    };
    let fallback;
    const terminate=reason=>{
      if(error)return;error=new GateError(reason);
      try {
        if(child.pid&&process.platform==='win32') {
          const r=spawnSync(path.join(systemRoot,'System32','taskkill.exe'),['/PID',String(child.pid),'/T','/F'],
            {stdio:['ignore','pipe','pipe'],timeout:10_000,maxBuffer:1024*1024,windowsHide:true});
          cleanup={status:r.status,signal:r.signal,error:r.error?.message??null,
            stdout:r.stdout?.toString()??'',stderr:r.stderr?.toString()??''};
          if(r.status!==0||r.error||r.signal)error=new GateError(`${reason}; process-tree cleanup failed for own child ${child.pid}`);
        } else if(child.pid)process.kill(-child.pid,'SIGKILL');
      }catch(e){error=new GateError(`${reason}; cleanup: ${e.message}`);}
      // Failed OS cleanup cannot leave this command waiting indefinitely or produce PASS.
      fallback=setTimeout(()=>finish(null,'CLEANUP_INCOMPLETE'),1000);
    };
    const receive=(chunks,data)=>{
      const remaining=outputLimit-bytes;
      if(remaining>0)chunks.push(data.subarray(0,remaining));
      bytes+=data.length;
      if(bytes>outputLimit)terminate(`03 wrapper output exceeded ${outputLimit} bytes`);
    };
    child.stdout.on('data',data=>receive(stdout,data));child.stderr.on('data',data=>receive(stderr,data));
    child.on('error',e=>{error=e;});child.on('close',(code,signal)=>finish(code,signal));
    const timer=setTimeout(()=>terminate(`03 wrapper timeout after ${timeout}ms`),timeout);
  });
}
export async function runGate(snapshot, {cwd=ROOT, preflightRun=preflight, runner=wrapperRunner}={}) {
  const diagnostics=[], wrappers=[]; let error, failed=false, measured;
  try {
    preflightRun(snapshot,{cwd});
    for (const script of ['scripts/check-module-arcs.mjs','scripts/check-module-cycles.mjs']) {
      try {
        const r=await runner(script,{cwd});
        const stdout=Buffer.isBuffer(r.stdout)?decode(r.stdout,script):r.stdout??'';
        const stderr=Buffer.isBuffer(r.stderr)?decode(r.stderr,script):r.stderr??'';
        wrappers.push({script,exit:r.status,signal:r.signal??null,error:r.error?.message??null,cleanup:r.cleanup??null,stdout,stderr});
        if(r.error||r.signal||![0,1].includes(r.status)) error??=new GateError(`03 wrapper ${script} no verdict exit=${r.status} signal=${r.signal??'-'}`);
        else failed ||= r.status===1;
      } catch(e) { error??=e; wrappers.push({script,error:e.message}); }
    }
    try { measured=ratchet(snapshot); diagnostics.push(...measured.diagnostics); } catch(e) {error??=e;}
    preflightRun(snapshot,{cwd});
    if(error)throw error;
    failed ||= diagnostics.length>0;
    return {gate:'03',status:failed?'FAIL':'PASS',exit:failed?1:0,baseOid:snapshot.baseOid,headOid:snapshot.headOid,
      diagnostics,measurements:measured.measurements,wrappers};
  } catch(e) {
    return {gate:'03',status:'ERROR',exit:[2,3,4].includes(e.exit)?e.exit:3,baseOid:snapshot.baseOid,headOid:snapshot.headOid,
      diagnostics:[...diagnostics,{reason:e.message,remedy:'repair inputs; rerun on committed equivalent working tree'}],wrappers};
  }
}
if (isMain(import.meta.url)) process.exitCode=await gateCli(runGate);
