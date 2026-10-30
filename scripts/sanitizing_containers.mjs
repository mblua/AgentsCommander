// Initial finite inventory: literals identify only registered, scoped containers.
// Writer calls are syntactic observations, never proof of a container's identity.
import { gateCli, isMain, GateError, codeOrder } from './sanitizing_baseline.mjs';
import { scanRust, createRustScanner } from './sanitizing_rust_scan.mjs';
import { qualified } from './sanitizing_gate_support.mjs';

const registryPath = 'scripts/containers.json';
const writerPath = /^(?:(?:std|tokio)::fs::(?:write|File::create|OpenOptions::new)|(?:fs::)?write|(?:File::create|OpenOptions::new))$/;
const rustSource = p => /^src-tauri\/src\/.*\.rs$/.test(p);
const validPath = p => typeof p === 'string' && /^src-tauri\/src\//.test(p) &&
  !p.includes('..') && !p.includes(String.fromCharCode(92));
export function parseRegistry(text) {
  let doc;
  try { doc = JSON.parse(text); } catch { throw new GateError('containers.json: invalid JSON'); }
  if (doc?.version !== 1 || !Array.isArray(doc.containers) || !doc.containers.length)
    throw new GateError('containers.json: version 1 and nonempty containers required');
  const ids = new Set(), names = new Set();
  for (const c of doc.containers) {
    if (!c || typeof c.id !== 'string' || !c.id.trim() || ids.has(c.id) ||
        typeof c.name !== 'string' || !/^(?!\.{1,2}$)[\w.-]+$/.test(c.name) ||
        !validPath(c.scope) || !c.scope.endsWith('/') ||
        !validPath(c.owner) || !c.owner.startsWith(c.scope) || !c.owner.endsWith('.rs') ||
        c.externalLiteralBaseline !== 0 ||
        !Number.isInteger(c.writerInventory?.files) || c.writerInventory.files < 0 ||
        !Number.isInteger(c.writerInventory?.sites) || c.writerInventory.sites < c.writerInventory.files ||
        names.has(c.scope + c.name))
      throw new GateError('containers.json: invalid/duplicate identity, owner, zero baseline or inventory');
    ids.add(c.id); names.add(c.scope + c.name);
  }
  return doc.containers;
}

export function measureSource(path, text, scan = scanRust(path, text)) {
  const ts = scan.productionTokens, writers = [];
  for (let i = 0; i < ts.length; i++) {
    const t = ts[i];
    if (t.kind !== 'ident' || ts[i - 1]?.text === '.' ||
        ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident') continue;
    const r = qualified(scan, ts, i, writerPath);
    if (ts[r.end]?.text === '(' && writerPath.test(r.path ?? '') &&
        ((r.path ?? '').includes('::') || r.binding?.kind === 'import'))
      writers.push({ line: t.line, symbol: r.path, ambiguous: !!r.ambiguous });
    i = r.end - 1;
  }
  return { literals: ts.filter(t => t.kind === 'string').map(t => ({ value: t.value, line: t.line })),
    writers };
}

export function runGate(snapshot) {
  const diagnostics = [], metrics = [], inventory = { files: 0, sites: 0 };
  try {
    const entry = snapshot.headTree.get(registryPath);
    if (!entry) throw new GateError('containers.json: registry missing');
    const containers = parseRegistry(snapshot.readBlob(entry));
    for (const c of containers) if (!snapshot.headTree.has(c.owner))
      throw new GateError('containers.json: owner missing: ' + c.owner);
    const measurements = new Map();
    const scan = createRustScanner(snapshot.headTree, snapshot.readBlob);
    for (const p of [...snapshot.headTree.keys()].filter(rustSource).sort(codeOrder)) {
      const m = measureSource(p, snapshot.readBlob(snapshot.headTree.get(p)), scan(p));
      measurements.set(p, m);
      if (m.writers.length) { inventory.files++; inventory.sites += m.writers.length; }
    }
    const registeredScopes = containers.map(c => c.scope);
    const unknown = [...measurements].filter(([p, m]) => m.writers.length &&
      !registeredScopes.some(scope => p.startsWith(scope)));
    diagnostics.push({ gate: '04', status: 'WARN', rule: 'unclassified_writers',
      path: '<unclassified>', line: 1,
      reason: 'Syntactic writers outside registered scopes need identity review; no global prohibition.',
      files: unknown.length, sites: unknown.reduce((n, [,m]) => n + m.writers.length, 0),
      paths: unknown.map(([p]) => p) });
    for (const c of containers) {
      const external = [], scopedWriters = { files: 0, sites: 0 };
      for (const [p, m] of measurements) {
        if (!p.startsWith(c.scope)) continue;
        if (m.writers.length) { scopedWriters.files++; scopedWriters.sites += m.writers.length; }
        if (p !== c.owner) for (const t of m.literals) if (t.value === c.name)
          external.push({ path: p, line: t.line });
      }
      metrics.push({ id: c.id, externalFiles: new Set(external.map(t => t.path)).size,
        externalLiterals: external.length, writerInventory: scopedWriters,
        recordedWriterInventory: c.writerInventory });
      for (const t of external) diagnostics.push({ gate: '04', status: 'FAIL',
        rule: 'external_container_literal', ...t, identity: c.id,
        reason: 'Registered container name belongs to ' + c.owner,
        remedy: 'Use the typed owner API or its name/path projection.' });
      if (scopedWriters.files !== c.writerInventory.files || scopedWriters.sites !== c.writerInventory.sites)
        diagnostics.push({ gate: '04', status: 'WARN', rule: 'writer_inventory_changed',
          identity: c.id, path: c.scope, line: 1, reason: 'Review syntactic scope writers; other containers are not classified yet.' });
    }
    snapshot.assertStableHead();
    const fail = diagnostics.some(d => d.status === 'FAIL');
    return { gate: '04', status: fail ? 'FAIL' : 'PASS', exit: fail ? 1 : 0,
      baseOid: snapshot.baseOid, headOid: snapshot.headOid, inventory, metrics, diagnostics };
  } catch (e) {
    return { gate: '04', status: 'ERROR', exit: e.exit ?? 3,
      diagnostics: [{ gate: '04', reason: e.message, remedy: 'Repair registry/Git input and rerun.' }] };
  }
}
if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
