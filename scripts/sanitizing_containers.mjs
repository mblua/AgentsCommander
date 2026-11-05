// Initial finite inventory: literals identify only registered, scoped containers.
// Writer calls are syntactic observations, never proof of a container's identity.
import { gateCli, isMain, GateError, codeOrder } from './sanitizing_baseline.mjs';
import { scanRust, createRustScanner } from './sanitizing_rust_scan.mjs';
import { qualified, resolveAlias } from './sanitizing_gate_support.mjs';

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
    validatePolicy(c);
    ids.add(c.id); names.add(c.scope + c.name);
  }
  return doc.containers;
}

const settingsOwner = 'src-tauri/src/config/settings.rs';
const artifactRoot = 'src-tauri/src/config/instance_artifacts';
const settingsNames = ['SETTINGS_FILE_NAME', 'SETTINGS_TARGET_NAME'];
const settingsPaths = ['', 'names::'].flatMap(prefix => settingsNames.map(name =>
  'crate::config::instance_artifacts::' + prefix + name));
const owns = (c, p) => p === c.owner || c.ownerModule && p.startsWith(c.owner.slice(0, -3) + '/');
const rustPath = /^(?:crate|self|super|[A-Za-z_]\w*)(?:::[A-Za-z_]\w*)*$/;
function validatePolicy(c) {
  const opt = c.constantPaths !== undefined || c.metadataPolicy !== undefined;
  if (c.ownerModule !== undefined && typeof c.ownerModule !== 'boolean' || opt && !c.ownerModule)
    throw new GateError('containers.json: constant policy requires ownerModule:true');
  if (c.constantPaths !== undefined && (!Array.isArray(c.constantPaths) ||
      !c.constantPaths.length || new Set(c.constantPaths).size !== c.constantPaths.length ||
      c.constantPaths.some(p => typeof p !== 'string' || !p.startsWith('crate::') || !rustPath.test(p))))
    throw new GateError('containers.json: invalid constantPaths');
  if (c.metadataPolicy !== undefined && (c.metadataPolicy !== 'instance-settings-v1' ||
      c.id !== 'Instance/Settings' || c.name !== 'settings.30.instance.no-git.json' ||
      c.scope !== 'src-tauri/src/' || c.owner !== settingsOwner ||
      JSON.stringify(c.constantPaths) !== JSON.stringify(settingsPaths)))
    throw new GateError('containers.json: invalid instance-settings-v1 identity/policy');
}
function moduleChain(path, scope) {
  let relative = path.slice('src-tauri/src/'.length, -3).split('/');
  if (['lib', 'main', 'mod'].includes(relative.at(-1))) relative.pop();
  const inline = [];
  for (let s = scope; s; s = s.parent) if (s.item?.kind === 'mod') inline.unshift(s.item.name);
  return [...relative, ...inline];
}
function canonical(path, chain) {
  const parts = path.replace(/^::/, '').split('::'), root = [...chain];
  if (parts[0] === 'crate') return parts.join('::');
  if (parts[0] === 'self') parts.shift();
  while (parts[0] === 'super') { if (!root.length) return null; root.pop(); parts.shift(); }
  return ['crate', ...root, ...parts].join('::');
}
const tokenText = ts => ts.map(t => t.kind === 'string' ? JSON.stringify(t.value) :
  t.name ?? t.text).join(' ');
function metadataExemptions(path, scan, c) {
  const exempt = new Set();
  if (!c.metadataPolicy || ![artifactRoot + '.rs', artifactRoot + '/names.rs'].includes(path))
    return exempt;
  const production = new Set(scan.productionTokens);
  const items = scan.items.filter(i => !i.testOnly && i.scope === scan.root);
  const tokens = i => scan.tokens.slice(i.first, i.last + 1);
  const invalid = reason => { throw new GateError(path + ': metadata shape: ' + reason); };
  const permit = ts => ts.filter(t => settingsNames.includes(t.name)).forEach(t => exempt.add(t.start));
  if (path.endsWith('/names.rs')) {
    const shapes = [
      'const SETTINGS_FILE_NAME : & str = SETTINGS_TARGET_NAME ;',
      'const SETTINGS_TARGET_NAME : & str = layered_name ! ( "settings" , instance , no_git , "json" ) ;',
    ];
    for (const [n, name] of settingsNames.entries()) {
      const matches = items.filter(i => i.kind === 'const' && i.name === name);
      if (matches.length !== 1 || tokenText(tokens(matches[0])) !== shapes[n]) invalid(name);
      permit(tokens(matches[0]));
    }
    return exempt;
  }
  const imports = items.filter(i => i.kind === 'use');
  let leaves = 0;
  for (const item of imports) {
    const ts = tokens(item), scope = scan.tokens[item.first].scope;
    for (const leaf of scope.imports) {
      if (!settingsNames.includes(leaf.name) && !settingsNames.some(n => leaf.target.endsWith('::' + n)))
        continue;
      // Scope imports include other use items; only inspect leaves actually occurring here.
      const sites = ts.filter(t => t.name === leaf.name);
      if (!sites.length) continue;
      if (leaf.name !== 'SETTINGS_FILE_NAME' || leaf.target !== 'names::SETTINGS_FILE_NAME' ||
          ts.some(t => t.name === 'as' || t.text === '*')) invalid('production re-export');
      leaves += sites.length; permit(sites);
    }
  }
  if (leaves !== 1) invalid('exactly one production SETTINGS_FILE_NAME leaf required');
  const tables = items.filter(i => i.kind === 'const' && i.name === 'INSTANCE_ARTIFACTS');
  if (tables.length !== 1) invalid('INSTANCE_ARTIFACTS');
  const table = tokens(tables[0]), eq = table.findIndex(t => t.text === '=');
  if (tokenText(table.slice(0, eq + 1)) !==
      'const INSTANCE_ARTIFACTS : & [ InstanceArtifact ] =') invalid('table declaration');
  const body = table.slice(eq + 1);
  if (body[0]?.text !== '&' || body[1]?.text !== '[' || body.at(-2)?.text !== ']')
    invalid('table initializer');
  let rows = 0;
  for (let i = 2; i < body.length - 2;) {
    if (body[i].name !== 'InstanceArtifact' || body[i + 1]?.text !== '{') invalid('table row');
    const end = body.findIndex(t => t.index === body[i + 1].mate);
    if (end < i) invalid('unbalanced row');
    const row = body.slice(i, end + 1);
    if (row.some(t => t.name === 'SETTINGS_FILE_NAME')) {
      const shape = 'InstanceArtifact { name : SETTINGS_FILE_NAME , kind : ArtifactKind :: File , ' +
        'disposition : Disposition :: Ignore , comment : "# AgentsCommander: application settings" , }';
      if (tokenText(row) !== shape) invalid('settings row');
      rows++; permit(row);
    }
    i = end + 1;
    if (body[i]?.text === ',') i++;
  }
  if (rows !== 1) invalid('exactly one settings row required');
  for (const start of exempt) if (!scan.tokens.some(t => t.start === start && production.has(t)))
    invalid('nonproduction exemption');
  return exempt;
}

// The scanner resolves lexical aliases. This adapter supplies Rust module identity,
// which matters for mod shadows, module-root constants and relative import targets.
function constantReferences(path, scan, c, exempt) {
  const references = [], relevant = new Set(c.constantPaths ?? []);
  if (!relevant.size) return references;
  const pattern = new RegExp('^(?:' + [...relevant].join('|') + ')$');
  const ts = scan.productionTokens;
  const itemAt = t => scan.items.find(i => i.first <= t.index && i.last >= t.index &&
    i.kind === 'use');
  function resolve(raw, scope, token, seen = new Set()) {
    const parts = raw.replace(/^::/, '').split('::'), first = parts[0];
    const chain = moduleChain(path, scope);
    if (['crate', 'self', 'super'].includes(first))
      return { path: canonical(raw, chain), ambiguous: false };
    if (seen.has(first)) return { path: canonical(raw, chain), ambiguous: true };
    const next = new Set([...seen, first]);
    let binding, bindingScope, glob = false;
    for (let s = scope; s; s = s.parent) {
      let b = s.bindings.get(first);
      while (b && b.start > token.start) b = b.previous;
      if (b) { binding = b; bindingScope = s; break; }
      glob ||= s.globs.length > 0;
    }
    const declared = scan.items.find(i => i.name === first && i.scope === bindingScope &&
      ['mod', 'const', 'static'].includes(i.kind));
    // Module declarations and module-level const/static names have canonical identity
    // independently of their initializer (which is inspected as its own occurrence).
    if (declared && (declared.kind === 'mod' || bindingScope === scan.root ||
        bindingScope?.item?.kind === 'mod'))
      return { path: canonical(raw, moduleChain(path, declared.scope)),
        ambiguous: !!binding?.ambiguous };
    if (binding?.kind === 'import') {
      const target = resolve(binding.target, binding.scope, token, next);
      return { ...target, path: target.path && [target.path, ...parts.slice(1)].join('::'),
        ambiguous: target.ambiguous || !!binding.ambiguous };
    }
    if (binding?.target) {
      const target = resolve(binding.target, binding.scope, token, next);
      return { ...target, path: target.path && [target.path, ...parts.slice(1)].join('::'),
        ambiguous: target.ambiguous || !!binding.ambiguous };
    }
    if (binding && !binding.ambiguous &&
        (binding.kind === 'local' || binding.kind === 'shadow' && !declared))
      return { path: null, ambiguous: false };
    const fallback = resolveAlias(scan, raw, scope, pattern);
    return { path: canonical(fallback.path ?? raw, chain),
      ambiguous: glob || !!binding?.ambiguous || fallback.ambiguous };
  }
  const add = (t, r) => {
    if (exempt.has(t.start)) return;
    const candidate = relevant.has(r.path);
    const ambiguous = r.ambiguous && (candidate || settingsNames.includes(t.name) ||
      [...relevant].some(p => p.endsWith('::' + t.name)));
    if (candidate || ambiguous) references.push({ path, line: t.line, symbol: r.path,
      ambiguous: !!r.ambiguous, reason: r.ambiguous ? 'ambiguous relevant alias' : undefined });
  };
  // Each use leaf is distinct; grouped use syntax is not an expression path.
  for (const item of scan.items.filter(i => i.kind === 'use' && !i.testOnly)) {
    const scope = scan.tokens[item.first].scope;
    const leaves = scope.imports.filter(leaf => scan.tokens.slice(item.first, item.last + 1)
      .some(t => t.name === leaf.name || leaf.glob && t.text === '*'));
    for (const leaf of leaves) {
      const target = canonical(leaf.target, moduleChain(path, scope));
      const r = resolve(leaf.target, scope, scan.tokens[item.first]);
      const site = scan.tokens.slice(item.first, item.last + 1).find(t =>
        t.name === leaf.target.split('::').at(-1) || leaf.glob && t.text === '*');
      if (site) add(site, { path: relevant.has(target) ? target : r.path,
        ambiguous: r.ambiguous || leaf.glob && [...relevant].some(p =>
          p.startsWith(target?.slice(0, -1))) });
    }
  }
  for (let i = 0; i < ts.length; i++) {
    const t = ts[i];
    if (t.kind !== 'ident' || itemAt(t) || ts[i - 1]?.text === '::' ||
        ts[i - 1]?.text === '.' || t.declaration) continue;
    const q = qualified(scan, ts, i, pattern);
    const r = resolve(q.raw, t.scope, t);
    add(t, r); i = q.end - 1;
  }
  return references;
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
    for (const c of containers) if (c.ownerModule && ![...snapshot.headTree.keys()].some(p =>
      p.startsWith(c.owner.slice(0, -3) + '/')))
      throw new GateError('containers.json: owner subtree missing: ' + c.owner);
    const measurements = new Map();
    const scan = createRustScanner(snapshot.headTree, snapshot.readBlob);
    for (const p of [...snapshot.headTree.keys()].filter(rustSource).sort(codeOrder)) {
      const m = measureSource(p, snapshot.readBlob(snapshot.headTree.get(p)), scan(p));
      measurements.set(p, m);
      if (m.writers.length) { inventory.files++; inventory.sites += m.writers.length; }
    }
    const registeredScopes = containers.filter(c => !c.ownerModule).map(c => c.scope);
    const unknown = [...measurements].filter(([p, m]) => m.writers.length &&
      !registeredScopes.some(scope => p.startsWith(scope)));
    diagnostics.push({ gate: '04', status: 'WARN', rule: 'unclassified_writers',
      path: '<unclassified>', line: 1,
      reason: 'Syntactic writers outside registered scopes need identity review; no global prohibition.',
      files: unknown.length, sites: unknown.reduce((n, [,m]) => n + m.writers.length, 0),
      paths: unknown.map(([p]) => p) });
    for (const c of containers) {
      const external = [], constants = [], scopedWriters = { files: 0, sites: 0 };
      for (const [p, m] of measurements) {
        if (!p.startsWith(c.scope)) continue;
        if (m.writers.length && (!c.ownerModule || owns(c, p))) {
          scopedWriters.files++; scopedWriters.sites += m.writers.length;
        }
        if (c.constantPaths || c.metadataPolicy) {
          const source = scan(p), exempt = metadataExemptions(p, source, c);
          if (!owns(c, p)) constants.push(...constantReferences(p, source, c, exempt));
        }
        if (!owns(c, p)) for (const t of m.literals) if (t.value === c.name)
          external.push({ path: p, line: t.line });
      }
      metrics.push({ id: c.id, externalFiles: new Set(external.map(t => t.path)).size,
        externalLiterals: external.length,
        ...(c.ownerModule ? { externalConstants: constants.length,
          externalUses: external.length + constants.length } : {}), writerInventory: scopedWriters,
        recordedWriterInventory: c.writerInventory });
      for (const t of external) diagnostics.push({ gate: '04', status: 'FAIL',
        rule: 'external_container_literal', ...t, identity: c.id,
        reason: 'Registered container name belongs to ' + c.owner,
        remedy: 'Use the typed owner API or its name/path projection.' });
      for (const t of constants) diagnostics.push({ gate: '04', status: 'FAIL',
        rule: 'external_container_constant', ...t, identity: c.id,
        reason: t.reason ?? 'Registered constant belongs to ' + c.owner,
        remedy: 'Use the typed owner API.' });
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
