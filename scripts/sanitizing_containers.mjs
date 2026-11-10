// Initial finite inventory: literals identify only registered, scoped containers.
// Writer calls are syntactic observations, never proof of a container's identity.
import { gateCli, isMain, GateError, codeOrder } from './sanitizing_baseline.mjs';
import { scanRust, createRustScanner } from './sanitizing_rust_scan.mjs';
import { qualified, resolveAlias } from './sanitizing_gate_support.mjs';

const registryPath = 'scripts/containers.json';
const writerPath = new RegExp('^(?:(?:std|tokio)::fs::(?:write|File::create|OpenOptions::new)|' +
  '(?:fs::)?write|(?:File::create|OpenOptions::new))$');
const rustSource = p => /^src-tauri\/src\/.*\.rs$/.test(p);
const validPath = p => typeof p === 'string' && /^src-tauri\/src\//.test(p) &&
  !p.includes('..') && !p.includes(String.fromCharCode(92)) &&
  !p.includes('//') && !p.split('/').includes('.');
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
        c.literalScope !== undefined && (!validPath(c.literalScope) ||
          !c.literalScope.endsWith('/') || !c.scope.startsWith(c.literalScope)) ||
        c.externalLiteralBaseline !== 0 ||
        !Number.isInteger(c.writerInventory?.files) || c.writerInventory.files < 0 ||
        !Number.isInteger(c.writerInventory?.sites) ||
        c.writerInventory.sites < c.writerInventory.files ||
        names.has(c.scope + c.name))
      throw new GateError('containers.json: invalid/duplicate identity, owner, ' +
        'zero baseline or inventory');
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
const settingsPolicy = 'instance-settings-v1', artifactsPolicy = 'instance-artifacts-v1';
const metadataFiles = [artifactRoot + '.rs', artifactRoot + '/names.rs'];
function validatePolicy(c) {
  if (c.metadataPolicy === artifactsPolicy) {
    if (c.literalScope === undefined || c.ownerModule !== undefined ||
        c.constantPaths !== undefined)
      throw new GateError('containers.json: instance-artifacts-v1 requires literalScope only');
    return;
  }
  const opt = c.constantPaths !== undefined || c.metadataPolicy !== undefined;
  if (c.ownerModule !== undefined && typeof c.ownerModule !== 'boolean' || opt && !c.ownerModule)
    throw new GateError('containers.json: constant policy requires ownerModule:true');
  if (c.constantPaths !== undefined && (!Array.isArray(c.constantPaths) ||
      !c.constantPaths.length || new Set(c.constantPaths).size !== c.constantPaths.length ||
      c.constantPaths.some(p => typeof p !== 'string' ||
        !p.startsWith('crate::') || !rustPath.test(p))))
    throw new GateError('containers.json: invalid constantPaths');
  if (c.metadataPolicy !== undefined && (c.metadataPolicy !== settingsPolicy ||
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
  if (c.metadataPolicy !== settingsPolicy || !metadataFiles.includes(path)) return exempt;
  const production = new Set(scan.productionTokens);
  const items = scan.items.filter(i => !i.testOnly && i.scope === scan.root);
  const tokens = i => scan.tokens.slice(i.first, i.last + 1);
  const invalid = reason => { throw new GateError(path + ': metadata shape: ' + reason); };
  const permit = ts => ts.filter(t => settingsNames.includes(t.name))
    .forEach(t => exempt.add(t.start));
  function declarations() {
    const shapes = [
      'const SETTINGS_FILE_NAME : & str = SETTINGS_TARGET_NAME ;',
      'const SETTINGS_TARGET_NAME : & str = ' +
        'layered_name ! ( "settings" , instance , no_git , "json" ) ;',
    ];
    for (const [n, name] of settingsNames.entries()) {
      const matches = items.filter(i => i.kind === 'const' && i.name === name);
      if (matches.length !== 1 || tokenText(tokens(matches[0])) !== shapes[n]) invalid(name);
      permit(tokens(matches[0]));
    }
  }
  function productionLeaf(leaf, ts) {
    if (!settingsNames.includes(leaf.name) &&
        !settingsNames.some(n => leaf.target.endsWith('::' + n)))
      return 0;
    // Scope imports include other use items; only inspect leaves actually occurring here.
    const sites = ts.filter(t => t.name === leaf.name);
    if (!sites.length) return 0;
    if (leaf.name !== 'SETTINGS_FILE_NAME' || leaf.target !== 'names::SETTINGS_FILE_NAME' ||
        ts.some(t => t.name === 'as' || t.text === '*')) invalid('production re-export');
    permit(sites);
    return sites.length;
  }
  function productionItem(item) {
    const ts = tokens(item), scope = scan.tokens[item.first].scope;
    let sites = 0;
    for (const leaf of scope.imports) sites += productionLeaf(leaf, ts);
    return sites;
  }
  function productionLeaves() {
    const imports = items.filter(i => i.kind === 'use');
    let leaves = 0;
    for (const item of imports) leaves += productionItem(item);
    if (leaves !== 1) invalid('exactly one production SETTINGS_FILE_NAME leaf required');
  }
  function settingsRow(row) {
    if (!row.some(t => t.name === 'SETTINGS_FILE_NAME')) return 0;
    const shape = 'InstanceArtifact { name : SETTINGS_FILE_NAME , ' +
      'kind : ArtifactKind :: File , ' +
      'disposition : Disposition :: Ignore , ' +
      'comment : "# AgentsCommander: application settings" , }';
    if (tokenText(row) !== shape) invalid('settings row');
    permit(row);
    return 1;
  }
  function artifactRowEnd(body, i) {
    if (body[i].name !== 'InstanceArtifact' || body[i + 1]?.text !== '{') invalid('table row');
    const end = body.findIndex(t => t.index === body[i + 1].mate);
    if (end < i) invalid('unbalanced row');
    return { end, rows: settingsRow(body.slice(i, end + 1)) };
  }
  function artifactTable() {
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
      const row = artifactRowEnd(body, i);
      rows += row.rows;
      i = row.end + 1;
      if (body[i]?.text === ',') i++;
    }
    if (rows !== 1) invalid('exactly one settings row required');
  }
  if (path.endsWith('/names.rs')) {
    declarations();
    return exempt;
  }
  productionLeaves();
  artifactTable();
  for (const start of exempt) if (!scan.tokens.some(t => t.start === start && production.has(t)))
    invalid('nonproduction exemption');
  return exempt;
}

// instance-artifacts-v1: a container name may appear in instance metadata only as the
// exact `const X: &str = "<name>";` in names.rs or as an INSTANCE_ARTIFACTS row name.
const escapeRegex = text => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
function artifactExemptions(path, scan, c) {
  const exempt = new Set();
  if (c.metadataPolicy !== artifactsPolicy || !metadataFiles.includes(path)) return exempt;
  const names = path.endsWith('/names.rs'), name = escapeRegex(JSON.stringify(c.name));
  const shape = names ? new RegExp('^const [A-Za-z_]\\w* : & str = ' + name + ' ;$') :
    new RegExp('^InstanceArtifact \\{ name : ' + name + ' , kind : ArtifactKind :: \\w+ , ' +
      'disposition : Disposition :: \\w+ , comment : "(?:[^"\\\\]|\\\\.)*" , \\}$');
  const production = new Set(scan.productionTokens);
  for (const t of scan.productionTokens) {
    if (t.kind !== 'string' || t.value !== c.name) continue;
    const site = names ? rootItem(scan, t, 'const') : artifactRow(scan, t);
    const ts = site && scan.tokens.slice(site.first, site.last + 1);
    if (ts && ts.every(x => production.has(x)) && shape.test(tokenText(ts))) exempt.add(t.start);
  }
  return exempt;
}
const rootItem = (scan, t, kind, name) => scan.items.find(i => i.kind === kind &&
  (name === undefined || i.name === name) && !i.testOnly && i.scope === scan.root &&
  i.first <= t.index && i.last >= t.index);
function artifactRow(scan, t) {
  const table = rootItem(scan, t, 'const', 'INSTANCE_ARTIFACTS');
  if (!table) return null;
  for (let i = t.index; i > table.first; i--) {
    const open = scan.tokens[i];
    if (open.text !== '{' || scan.tokens[i - 1]?.name !== 'InstanceArtifact') continue;
    return open.mate >= t.index ? { first: i - 1, last: open.mate } : null;
  }
  return null;
}
function externalLiterals(path, scan, c) {
  const exempt = artifactExemptions(path, scan, c);
  return scan.productionTokens.filter(t => t.kind === 'string' && t.value === c.name &&
    !exempt.has(t.start)).map(t => ({ path, line: t.line }));
}

// The scanner resolves lexical aliases. This adapter supplies Rust module identity,
// which matters for mod shadows, module-root constants and relative import targets.
function moduleContexts(paths, scanner) {
  const scopes = new WeakMap(), modules = new Map();
  for (const path of paths) {
    const scan = scanner(path);
    for (const scope of scan.scopes) {
      scopes.set(scope, { path, scan });
      if (!scan.productionTokens.length ||
          scope !== scan.root && (scope.item?.kind !== 'mod' || scope.item.testOnly)) continue;
      const identity = ['crate', ...moduleChain(path, scope)].join('::');
      if (!modules.has(identity)) modules.set(identity, []);
      modules.get(identity).push(scope);
    }
  }
  return { scopes, modules };
}
function nearestBinding(scope, first, token) {
  let binding, bindingScope; const globs = [];
  for (let s = scope; s; s = s.parent) {
    let b = s.bindings.get(first);
    while (b && b.start > token.start) b = b.previous;
    if (b) { binding = b; bindingScope = s; break; }
    globs.push(...s.globs.map(g => ({ ...g, scope: s })));
  }
  return { binding, bindingScope, globs };
}
const parameterShadow = (binding, bindingScope, source, first) => binding?.kind === 'shadow' &&
  bindingScope?.item?.kind === 'fn' &&
  !source.scan.items.some(i => i.name === first && i.scope === bindingScope);
function constantReferences(path, scan, c, exempt, contexts) {
  const references = [], relevant = new Set(c.constantPaths ?? []);
  if (!relevant.size) return references;
  const pattern = new RegExp('^(?:' + [...relevant].join('|') + ')$');
  const ts = scan.productionTokens;
  const itemAt = t => scan.items.find(i => i.first <= t.index && i.last >= t.index &&
    i.kind === 'use');
  function moduleScope(identity, name) {
    const choices = contexts.modules.get(identity) ?? [];
    // If no module scope binds the name, keep the first scope as the fallback.
    return choices.find(s => s.bindings.has(name)) ?? choices[0];
  }
  function ambiguousBinding(binding, parts) {
    for (let b = binding; b; b = b.previous) if (b.target) {
      const candidate = canonical([b.target, ...parts.slice(1)].join('::'),
        moduleChain(contexts.scopes.get(b.scope).path, b.scope));
      if (relevant.has(candidate)) return { path: candidate, ambiguous: true };
    }
    return null;
  }
  function bindingReference(binding, parts, token, next) {
    const target = resolve(binding.target, binding.scope, token, next);
    const expanded = target.path && parts.length > 1 ?
      resolve([target.path, ...parts.slice(1)].join('::'), binding.scope, token, next) : target;
    return { ...expanded,
      ambiguous: target.ambiguous || expanded.ambiguous || !!binding.ambiguous };
  }
  function globReference(globs, raw, token, next) {
    for (const glob of globs) {
      const target = resolve(glob.target.slice(0, -3), glob.scope, token, next);
      const candidate = target.path && target.path + '::' + raw;
      if (relevant.has(candidate)) return { path: candidate, ambiguous: true };
    }
    return null;
  }
  function absoluteReference(raw, chain, token, seen) {
    const absolute = canonical(raw, chain);
    if (relevant.has(absolute)) return { path: absolute, ambiguous: false };
    const remainder = absolute?.split('::').slice(1);
    const root = remainder?.length && moduleScope('crate', remainder[0]);
    if (!root) return { path: absolute, ambiguous: false };
    return resolve(remainder.join('::'), root, token, seen);
  }
  function resolve(raw, scope, token, seen = new Set()) {
    const parts = raw.replace(/^::/, '').split('::'), first = parts[0];
    const source = contexts.scopes.get(scope) ?? { path, scan };
    const chain = moduleChain(source.path, scope);
    if (['crate', 'self', 'super'].includes(first))
      return absoluteReference(raw, chain, token, seen);
    const { binding, bindingScope, globs } = nearestBinding(scope, first, token);
    const key = binding?.target ? binding : source.path + ':' + scope.id + ':' + raw;
    if (seen.has(key)) return { path: canonical(raw, chain), ambiguous: true };
    const next = new Set([...seen, key]);
    const hit = binding?.ambiguous ? ambiguousBinding(binding, parts) : null;
    if (hit) return hit;
    const declared = source.scan.items.find(i => i.name === first && i.scope === bindingScope &&
      ['mod', 'const', 'static'].includes(i.kind));
    function declaredReference() {
      // Module declarations and module-level const/static names have canonical identity
      // independently of their initializer (which is inspected as its own occurrence).
      if (declared?.kind === 'mod' && parts.length > 1) {
        const destination = declared.bodyScope ?? moduleScope(canonical(first, chain), parts[1]);
        if (destination) return resolve(parts.slice(1).join('::'), destination, token, next);
      }
      if (declared && (declared.kind === 'mod' || bindingScope === source.scan.root ||
          bindingScope?.item?.kind === 'mod'))
        return { path: canonical(raw, moduleChain(source.path, declared.scope)),
          ambiguous: !!binding?.ambiguous };
      return null;
    }
    function tailReference() {
      const parameter = parameterShadow(binding, bindingScope, source, first);
      if (binding && !binding.ambiguous && (binding.kind === 'local' || parameter))
        return { path: null, ambiguous: false };
      const glob = binding ? null : globReference(globs, raw, token, next);
      if (glob) return glob;
      if (!binding && parts.length > 1) {
        const destination = moduleScope(canonical(first, chain), parts[1]);
        if (destination) return resolve(parts.slice(1).join('::'), destination, token, next);
      }
      const fallback = resolveAlias(source.scan, raw, scope, pattern);
      return { path: canonical(fallback.path ?? raw, chain),
        ambiguous: globs.length > 0 || !!binding?.ambiguous || fallback.ambiguous };
    }
    const reference = declaredReference();
    if (reference) return reference;
    if (binding?.kind === 'import') return bindingReference(binding, parts, token, next);
    if (binding?.target) return bindingReference(binding, parts, token, next);
    return tailReference();
  }
  const add = (t, r) => {
    if (exempt.has(t.start)) return;
    const candidate = relevant.has(r.path);
    // The name suffix check only reports ambiguity; it never resolves a path.
    const ambiguous = r.ambiguous && (r.relevantAmbiguity || candidate ||
      settingsNames.includes(t.name) || settingsNames.includes(r.path?.split('::').at(-1)) ||
      [...relevant].some(p => p.endsWith('::' + t.name)));
    if (candidate || ambiguous) references.push({ path, line: t.line, symbol: r.path,
      ambiguous: !!r.ambiguous, reason: r.ambiguous ? 'ambiguous relevant alias' : undefined });
  };
  // Each use leaf is distinct; grouped use syntax is not an expression path.
  function useReference(leaf, scope, item) {
    const target = canonical(leaf.target, moduleChain(path, scope));
    const r = resolve(leaf.target, scope, scan.tokens[item.first]);
    const site = scan.tokens.slice(item.first, item.last + 1).find(t =>
      t.name === leaf.target.split('::').at(-1) || leaf.glob && t.text === '*');
    if (!site) return;
    const globCandidates = leaf.glob ? [...relevant].filter(p =>
      p.startsWith(r.path?.slice(0, -1))) : [];
    if (globCandidates.length) add(site, { path: globCandidates[0], ambiguous: true });
    else add(site, { path: relevant.has(target) ? target : r.path,
      ambiguous: r.ambiguous || leaf.glob });
  }
  function useReferences(item) {
    const scope = scan.tokens[item.first].scope;
    const leaves = scope.imports.filter(leaf => scan.tokens.slice(item.first, item.last + 1)
      .some(t => t.name === leaf.name || leaf.glob && t.text === '*'));
    for (const leaf of leaves) useReference(leaf, scope, item);
  }
  function expressionReference(i) {
    const t = ts[i];
    if (t.kind !== 'ident' || itemAt(t) || ts[i - 1]?.text === '::' ||
        ts[i - 1]?.text === '.' || t.declaration) return i;
    const q = qualified(scan, ts, i, pattern);
    const r = resolve(q.raw, t.scope, t);
    add(t, r);
    return q.end - 1;
  }
  for (const item of scan.items.filter(i => i.kind === 'use' && !i.testOnly)) useReferences(item);
  for (let i = 0; i < ts.length; i++) i = expressionReference(i);
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
  return { literals: ts.filter(t => t.kind === 'string')
    .map(t => ({ value: t.value, line: t.line })),
    writers };
}

// literal-files-v1: complete decoded strings, with no runtime path inference.
function literalIdentity(value) {
  if (typeof value !== 'string') return null;
  const literal = value.replaceAll(String.fromCodePoint(92), '/');
  const relative = literal.replace(/^(?:[A-Za-z]:)?\//, '');
  const segments = relative.split('/');
  const segment = /^(?:[A-Za-z0-9_.*-]|\{(?:[A-Za-z_][A-Za-z0-9_]*)?\})+$/;
  if (segments.some(s => !s || s === '.' || s === '..' ||
      !segment.test(s))) return null;
  return /^.+\.(?:jsonl?|toml|ya?ml|db|sqlite3?|lock|md|txt)$/.test(segments.at(-1))
    ? literal : null;
}
function discoverMeasurements(measurements, containers) {
  const entries = new Map();
  for (const [path, m] of measurements) for (const t of m.literals) {
    const literal = literalIdentity(t.value);
    if (literal === null || containers.some(c => t.value === c.name &&
        path.startsWith(c.literalScope ?? c.scope))) continue;
    if (!entries.has(literal)) entries.set(literal, { literal, files: new Set(), sites: [] });
    const entry = entries.get(literal);
    entry.files.add(path); entry.sites.push({ path, line: t.line });
  }
  return [...entries.values()].filter(e => e.files.size >= 2).map(e => ({ ...e,
    files: [...e.files].sort(codeOrder),
    sites: e.sites.sort((a, b) => codeOrder(a.path, b.path) || a.line - b.line),
  })).sort((a, b) => codeOrder(a.literal, b.literal));
}
function checkedReader(readBlob) {
  return entry => {
    const text = readBlob(entry);
    if (typeof text !== 'string') throw new GateError('source blob missing or non-text');
    return text;
  };
}
export function discoverUnregistered(tree, readBlob, containers) {
  readBlob = checkedReader(readBlob);
  if (!(tree instanceof Map)) throw new GateError('unregistered: tree missing');
  const scan = createRustScanner(tree, readBlob), measurements = new Map();
  for (const p of [...tree.keys()].filter(rustSource).sort(codeOrder))
    measurements.set(p, { literals: scan(p).productionTokens.filter(t => t.kind === 'string')
      .map(t => ({ value: t.value, line: t.line })) });
  return discoverMeasurements(measurements, containers);
}
const discoveryReport = entries => ({ entries, counts: {
  identities: entries.length, pairs: entries.reduce((n, e) => n + e.files.length, 0),
  sites: entries.reduce((n, e) => n + e.sites.length, 0),
} });
function unregisteredDiagnostics(base, head) {
  const previous = new Map(base.map(e => [e.literal, e])), diagnostics = [];
  for (const entry of head) {
    const old = previous.get(entry.literal);
    // Ratchet per-literal distribution size, not path continuity or semantic moves.
    if (old && entry.files.length <= old.files.length) continue;
    for (const site of entry.sites) {
      if (old?.files.includes(site.path)) continue;
      diagnostics.push({ gate: '04', status: 'FAIL',
        rule: old ? 'unregistered_container_file' : 'unregistered_container_growth',
        literal: entry.literal, ...site, baseFiles: old?.files ?? [], headFiles: entry.files,
        reason: 'Unregistered literal adds an identity or grows its production file count.',
        remedy: 'Remove distributed names, use the owner API, or approve registration separately.'
      });
    }
  }
  return diagnostics.sort((a, b) => codeOrder(a.rule, b.rule) ||
    codeOrder(a.literal, b.literal) || codeOrder(a.path, b.path) || a.line - b.line);
}

function registryContainers(snapshot, readBlob) {
  const baseEntry = snapshot.baseTree.get(registryPath);
  if (!baseEntry) throw new GateError('containers.json: base registry missing');
  const baseContainers = parseRegistry(readBlob(baseEntry));
  const entry = snapshot.headTree.get(registryPath);
  if (!entry) throw new GateError('containers.json: registry missing');
  const containers = parseRegistry(readBlob(entry));
  return { baseContainers, containers };
}
function containerPrerequisites(tree, containers) {
  for (const c of containers) if (!tree.has(c.owner))
    throw new GateError('containers.json: owner missing: ' + c.owner);
  for (const c of containers) if (c.ownerModule && ![...tree.keys()].some(p =>
    p.startsWith(c.owner.slice(0, -3) + '/')))
    throw new GateError('containers.json: owner subtree missing: ' + c.owner);
  metadataPrerequisites(tree, containers);
}
function metadataPrerequisites(tree, containers) {
  for (const c of containers) if (c.metadataPolicy)
    for (const p of metadataFiles)
      if (!tree.has(p)) throw new GateError('metadata source missing: ' + p);
}
function headMeasurements(tree, readBlob, inventory) {
  const measurements = new Map();
  const scan = createRustScanner(tree, readBlob);
  for (const p of [...tree.keys()].filter(rustSource).sort(codeOrder)) {
    const m = measureSource(p, readBlob(tree.get(p)), scan(p));
    measurements.set(p, m);
    if (m.writers.length) { inventory.files++; inventory.sites += m.writers.length; }
  }
  return { measurements, scan };
}
function unclassifiedWriters(measurements, containers) {
  const registeredScopes = containers.filter(c => !c.ownerModule).map(c => c.scope);
  const unknown = [...measurements].filter(([p, m]) => m.writers.length &&
    !registeredScopes.some(scope => p.startsWith(scope)));
  return { gate: '04', status: 'WARN', rule: 'unclassified_writers',
    path: '<unclassified>', line: 1,
    reason: 'Syntactic writers outside registered scopes need identity review; ' +
      'no global prohibition.',
    files: unknown.length, sites: unknown.reduce((n, [,m]) => n + m.writers.length, 0),
    paths: unknown.map(([p]) => p) };
}
function containerMeasurements(c, measurements, scan, contexts) {
  const external = [], constants = [], scopedWriters = { files: 0, sites: 0 };
  const literalScope = c.literalScope ?? c.scope;
  function visit(p, m) {
    if (p.startsWith(literalScope) && !owns(c, p))
      external.push(...externalLiterals(p, scan(p), c));
    if (!p.startsWith(c.scope)) return;
    if (m.writers.length && (!c.ownerModule || owns(c, p))) {
      scopedWriters.files++; scopedWriters.sites += m.writers.length;
    }
    if (c.constantPaths) {
      const source = scan(p), exempt = metadataExemptions(p, source, c);
      if (!owns(c, p)) constants.push(...constantReferences(p, source, c, exempt, contexts));
    }
  }
  for (const [p, m] of measurements) visit(p, m);
  return { external, constants, scopedWriters };
}
function recordContainer(c, measured, metrics, diagnostics) {
  const { external, constants, scopedWriters } = measured;
  metrics.push({ id: c.id,
    externalFiles: new Set([...external, ...constants].map(t => t.path)).size,
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
  if (scopedWriters.files !== c.writerInventory.files ||
      scopedWriters.sites !== c.writerInventory.sites)
    diagnostics.push({ gate: '04', status: 'WARN', rule: 'writer_inventory_changed',
      identity: c.id, path: c.scope, line: 1,
      reason: 'Review syntactic scope writers; other containers are not classified yet.' });
}

export function runGate(snapshot) {
  const diagnostics = [], metrics = [], inventory = { files: 0, sites: 0 };
  try {
    const readBlob = checkedReader(snapshot.readBlob);
    if (!(snapshot.baseTree instanceof Map) || !(snapshot.headTree instanceof Map))
      throw new GateError('unregistered: base/head tree missing');
    const { baseContainers, containers } = registryContainers(snapshot, readBlob);
    containerPrerequisites(snapshot.headTree, containers);
    const { measurements, scan } = headMeasurements(snapshot.headTree, readBlob, inventory);
    const headEntries = discoverMeasurements(measurements, containers);
    const baseEntries = discoverUnregistered(snapshot.baseTree, readBlob, baseContainers);
    const unregistered = { contract: 'literal-files-v1',
      base: discoveryReport(baseEntries), head: discoveryReport(headEntries) };
    diagnostics.push(...unregisteredDiagnostics(baseEntries, headEntries));
    const contexts = moduleContexts(measurements.keys(), scan);
    diagnostics.push(unclassifiedWriters(measurements, containers));
    for (const c of containers) {
      const measured = containerMeasurements(c, measurements, scan, contexts);
      recordContainer(c, measured, metrics, diagnostics);
    }
    snapshot.assertStableHead();
    const fail = diagnostics.some(d => d.status === 'FAIL');
    return { gate: '04', status: fail ? 'FAIL' : 'PASS', exit: fail ? 1 : 0,
      baseOid: snapshot.baseOid, headOid: snapshot.headOid, inventory, metrics, diagnostics,
      unregistered };
  } catch (e) {
    return { gate: '04', status: 'ERROR', exit: e.exit ?? 3,
      diagnostics: [...diagnostics, { gate: '04', reason: e.message,
        remedy: 'Repair registry/Git input and rerun.' }] };
  }
}
if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
