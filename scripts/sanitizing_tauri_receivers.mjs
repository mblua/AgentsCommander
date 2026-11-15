// Gate 06 receiver proof (#4017). Pure and finite: no imports, no IO, no scan mutation.
// A `.emit`/`.emit_to` call is PROVEN_PORT only when a closed grammar proves the receiver is
// a crate-local port trait object or a generic bounded only by such ports. Everything that
// the grammar does not fully consume is UNKNOWN and stays counted, exactly like REAL.
export const REAL = 'REAL', PROVEN_PORT = 'PROVEN_PORT', UNKNOWN = 'UNKNOWN';

const TAURI_REAL = /^tauri::(?:[A-Za-z_]+::)*(?:AppHandle|App|Window|WebviewWindow|Emitter)$/;
const MARKERS = new Map([['Send', 'std::marker::Send'], ['Sync', 'std::marker::Sync']]);
const ARC = new Set(['std::sync::Arc', 'alloc::sync::Arc']);
const BOUNDARY = new Set([';', '{', '}', '(', ',', '=', '=>']);
const BOUNDARY_WORDS = new Set(['if', 'return', 'match', 'while', 'in', 'else']);
const CRATE_ROOTS = new Set(['src-tauri/src/lib.rs', 'src-tauri/src/main.rs']);
const HARMLESS_ATTRS = new Set(['doc', 'derive', 'allow', 'warn', 'deny', 'expect']);

class Unknown extends Error {}
const unknown = reason => { throw new Unknown(reason); };
const need = (ok, reason) => { if (!ok) unknown(reason); };

// Angle depth after token t; a negative depth is unbalanced.
function angleDepth(angle, t) {
  if (t.text === '<') angle++;
  else if (t.text === '>') angle--;
  else if (t.text === '>>') angle -= 2;
  else if (t.text === '->') { /* arrow is not a closing angle */ }
  if (angle < 0) unknown('unbalanced angle brackets');
  return angle;
}

// Depth-aware split of tokens on a separator, counting angle brackets as nesting.
function split(tokens, separator) {
  const parts = [[]];
  let angle = 0;
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i];
    angle = angleDepth(angle, t);
    if (['(', '[', '{'].includes(t.text) && t.mate !== undefined) {
      const end = tokens.findIndex(x => x.index === t.mate);
      need(end > i, 'unbalanced delimiter');
      parts.at(-1).push(...tokens.slice(i, end + 1));
      i = end;
      continue;
    }
    if (t.text === separator && angle === 0) parts.push([]);
    else parts.at(-1).push(t);
  }
  need(angle === 0, 'unbalanced angle brackets');
  if (!parts.at(-1).length) parts.pop();
  return parts;
}

// Index of the matching `>` for the `<` at tokens[open]; tokens are a contiguous slice.
function closeAngle(tokens, open) {
  let angle = 0;
  for (let i = open; i < tokens.length; i++) {
    const t = tokens[i];
    if (['(', '[', '{'].includes(t.text) && t.mate !== undefined) {
      i = tokens.findIndex(x => x.index === t.mate);
      need(i > 0, 'unbalanced delimiter');
      continue;
    }
    if (t.text === '<') angle++;
    else if (t.text === '>') angle--;
    else if (t.text === '>>') unknown('unsupported nested generic close');
    if (angle === 0) return i;
  }
  return unknown('unterminated generics');
}

const simplePath = tokens => tokens.length && tokens.every((t, i) => i % 2
  ? t.text === '::' : t.kind === 'ident') && tokens.length % 2 === 1;

function checkFile(scan) {
  need(!scan.attributes.some(a => a.signals?.some(m => m.name === 'path')),
    'path attribute route');
  // Root-level macro invocations may generate competing items.
  for (const t of scan.tokens) if (t.kind === 'ident' && scan.tokens[t.index + 1]?.text === '!'
    && t.scope === scan.root && scan.tokens[t.index - 1]?.text !== '#'
    && !scan.items.some(i => i.kind === 'fn' && i.start <= t.start && t.start < i.end)
    && !scan.attributes.some(a => a.start <= t.start && t.start < a.end))
    unknown('item-level macro in proof route');
}

// Exactly one trait in the closure declares `method` with a `&self`/`&mut self` receiver.
function declaring(traits, method) {
  const found = [];
  for (const trait of traits.values()) {
    const { scan, item } = trait;
    const fns = scan.items.filter(i => i.kind === 'fn' && i.name === method &&
      i.start > item.start && i.end <= item.end);
    need(!scan.items.some(i => i.kind === 'type' && i.start > item.start && i.end <= item.end),
      'associated type dispatch');
    for (const fn of fns) found.push({ trait, fn });
  }
  need(found.length === 1, found.length ? 'conflicting methods' : 'method not declared');
  const [{ trait, fn }] = found;
  const { scan } = trait;
  const open = scan.tokens.findIndex((t, k) => k > fn.first && t.text === '(');
  const params = split(scan.tokens.slice(open + 1, scan.tokens[open].mate), ',');
  const self = (params[0] ?? []).filter(t => t.kind !== 'lifetime');
  need(self.length >= 2 && self[0].text === '&' && self.at(-1).name === 'self' &&
    (self.length === 2 || self.length === 3 && self[1].name === 'mut'),
    'method without &self receiver');
  const signature = scan.tokens.filter(t => t.start >= fn.start && t.start < fn.end);
  for (let k = 0; k < signature.length; k++) {
    const t = signature[k];
    if (t.kind !== 'ident' || signature[k - 1]?.text === '::') continue;
    let end = k + 1;
    while (signature[end]?.text === '::' && signature[end + 1]?.kind === 'ident') end += 2;
    const r = scan.resolvePath(signature.slice(k, end), t.scope);
    need(!/^tauri(?:::|$)/.test(r.path ?? ''), 'Tauri type in port method signature');
    k = end - 1;
  }
  return { trait, fn };
}

function competingDynImpls(other, chosen, method) {
  for (const impl of other.items.filter(i => i.kind === 'impl')) {
    if (dynImplCompetes(other, impl, chosen, method)) {
      unknown('inherent or trait impl method on dyn port');
    }
  }
}
function dynImplCompetes(other, impl, chosen, method) {
  const head = other.tokens.slice(impl.first + 1,
    other.tokens.findIndex(t => t.start === impl.bodyStart));
  const isTraitImpl = head.some(t => t.name === 'for');
  const target = isTraitImpl ? head.slice(head.findIndex(t => t.name === 'for') + 1) : head;
  if (!target.some(t => t.name === 'dyn')) return false;
  if (!target.some(t => t.name === chosen.trait.item.name)) return false;
  return other.items.some(i => i.kind === 'fn' && i.name === method &&
    i.start > impl.start && i.end <= impl.end);
}
// Length of the optional `&['a][mut]` reference prefix.
function referenceEnd(tokens) {
  let k = 0;
  if (tokens[k]?.text === '&') {
    k++;
    if (tokens[k]?.kind === 'lifetime') k++;
    if (tokens[k]?.name === 'mut') k++;
  }
  return k;
}
// Splits an item header (tokens after the name, before the body) into generics/rest/where.
function header(scan, item, start) {
  const tokens = scan.tokens.slice(start, scan.tokens.findIndex(t => t.start === item.bodyStart));
  need(item.bodyStart !== null, 'item without body');
  let generics = [], rest = tokens;
  if (tokens[0]?.text === '<') {
    const close = closeAngle(tokens, 0);
    generics = tokens.slice(1, close);
    rest = tokens.slice(close + 1);
  }
  const w = rest.findIndex(t => t.name === 'where');
  return { generics, rest: w < 0 ? rest : rest.slice(0, w),
    where: w < 0 ? [] : rest.slice(w + 1) };
}

function patternRanges(tokens, body) {
  const ranges = [];
  for (let k = body.start; k < body.end; k++) {
    const t = tokens[k];
    if (t.name === 'let') {
      ranges.push([k, scanUntil(tokens, k + 1, body.end, x => ['=', ';'].includes(x.text))]);
    } else if (t.name === 'for') {
      ranges.push([k, scanUntil(tokens, k + 1, body.end, x => x.name === 'in')]);
    } else if (t.text === '|' || t.text === '||') {
      if (!closureStart(tokens, k)) continue;
      ranges.push([k, closurePipeEnd(tokens, body, k)]);
    } else if (t.name === 'match') {
      matchArmRanges(tokens, matchOpen(tokens, body, k), ranges);
    }
  }
  return ranges;
}
// First index in [from, end) whose token satisfies stop; end when none does.
function scanUntil(tokens, from, end, stop) {
  let e = from;
  while (e < end && !stop(tokens[e])) e++;
  return e;
}
// Index of the `{` that opens the arms of the `match` at k.
function matchOpen(tokens, body, k) {
  let open = k + 1;
  while (open < body.end && tokens[open].text !== '{') {
    if (['(', '['].includes(tokens[open].text)) open = tokens[open].mate;
    open++;
  }
  need(open < body.end, 'unparsed match');
  return open;
}
// Pushes the pattern range of every arm inside the braces opened at open.
function matchArmRanges(tokens, open, ranges) {
  let arm = open + 1, inPattern = true;
  for (let a = open + 1; a < tokens[open].mate; a++) {
    const x = tokens[a];
    if (inPattern && x.text === '=>') { ranges.push([arm, a]); inPattern = false; continue; }
    if (['(', '[', '{'].includes(x.text)) {
      const startBlock = !inPattern && tokens[a - 1]?.text === '=>' && x.text === '{';
      a = x.mate;
      if (startBlock) { inPattern = true; arm = a + 1; a = skipComma(tokens, a); }
      continue;
    }
    if (!inPattern && x.text === ',') { inPattern = true; arm = a + 1; }
  }
}
function skipComma(tokens, a) {
  return tokens[a + 1]?.text === ',' ? a + 1 : a;
}
function closureStart(tokens, k) {
  const prev = tokens[k - 1];
  return prev && (['(', ',', '=', '{', ';', '=>', '(', '['].includes(prev.text) ||
    ['move', 'return', 'async'].includes(prev.name));
}
// Closures and async blocks capturing the receiver are not proved.
function captured(tokens, body, dot) {
  for (let k = body.start; k < body.end; k++) {
    const t = tokens[k];
    if (t.name === 'async' && asyncContains(tokens, k, dot)) return true;
    if ((t.text === '|' || t.text === '||') && closureStart(tokens, k) &&
      closureContains(tokens, body, k, dot)) return true;
  }
  return false;
}
function asyncContains(tokens, k, dot) {
  let b = k + 1;
  if (tokens[b]?.name === 'move') b++;
  return tokens[b]?.text === '{' && dot > b && dot < tokens[b].mate;
}
// Index of the closing pipe (`||`: the token itself); body.end when it is missing.
function closurePipeEnd(tokens, body, k) {
  let e = k;
  if (tokens[k].text === '|') { e = k + 1; while (e < body.end && tokens[e].text !== '|') e++; }
  return e;
}
// Block closure: inside its braces only. Expression closure: up to the enclosing group's end.
function closureContains(tokens, body, k, dot) {
  const e = closurePipeEnd(tokens, body, k);
  if (tokens[e + 1]?.text === '{') return dot > e && dot < tokens[e + 1].mate;
  // Expression closure: conservatively extends to the enclosing group's end.
  const close = groupsAround(tokens, k)[0] ?? body.end;
  return dot > e && dot < close;
}
function groupsAround(tokens, k) {
  const out = [];
  for (let j = k - 1; j >= 0; j--) {
    const t = tokens[j];
    if (['(', '[', '{'].includes(t.text) && t.mate > k) out.push(t.mate);
  }
  return out;
}
function safeReceiver(scan, fn, body, name, paramIndex, dot) {
  const tokens = scan.tokens;
  const nested = scan.items.filter(i => i !== fn && i.start > tokens[body.start].start &&
    i.end <= tokens[body.end].end);
  const ranges = patternRanges(tokens, body);
  for (let k = body.start + 1; k < body.end; k++) {
    const t = tokens[k];
    if (t.kind !== 'ident' || t.name !== name || k === paramIndex) continue;
    if (tokens[k - 1]?.text === '.' || tokens[k - 1]?.text === '::') continue;
    need(!nested.some(i => i.start <= t.start && t.start < i.end), 'receiver in nested item');
    const next = tokens[k + 1]?.text;
    // Struct-literal field labels (`host: Arc::clone(&host)`) are not bindings.
    if (next === ':' && ['{', ','].includes(tokens[k - 1]?.text)) continue;
    need(!ranges.some(([a, b]) => k > a && k < b), 'receiver name in a binding pattern');
    need(!['=', '+=', '-=', '*=', '/=', '@'].includes(next) && tokens[k - 1]?.text !== '@',
      'receiver written or rebound');
    // A macro after the call cannot rebind the non-mut parameter before it runs.
    need(k > dot || !inMacro(scan, k), 'receiver inside macro before the call');
  }
}
function inMacro(scan, k) {
  return scan.groups.some(g => (scan.tokens[g.start - 1]?.text === '!' ||
    scan.tokens[g.start - 2]?.text === '!') && k > g.start && k < g.end);
}

function fnParams(scan, fn) {
  const tokens = scan.tokens;
  let open = fn.first + 2;
  if (tokens[open]?.text === '<') open = fn.first + 2 + closeAngle(tokens.slice(open), 0) + 1;
  need(tokens[open]?.text === '(', 'unparsed fn parameters');
  return { open, parts: split(tokens.slice(open + 1, tokens[open].mate), ',') };
}

function checkFieldReceiver(tokens, parts, body, field) {
  const self = (parts[0] ?? []).filter(t => t.kind !== 'lifetime');
  need(self.at(-1)?.name === 'self' && self.length <= 3, 'method without self receiver');
  for (let k = body.start + 1; k < body.end - 2; k++)
    need(!(tokens[k].name === 'self' && tokens[k + 1].text === '.' &&
      tokens[k + 2].name === field && ['=', '+=', '@'].includes(tokens[k + 3]?.text)),
      'receiver field written');
}

// Field tokens after leading attributes and `pub`/`pub(...)` visibility.
function normalizeField(p) {
  const after = (part, k) => part.findIndex(t => t.index === part[k].mate) + 1;
  let k = 0;
  while (p[k]?.text === '#' && p[k + 1]?.text === '[') k = after(p, k + 1);
  if (p[k]?.name === 'pub') { k++; if (p[k]?.text === '(') k = after(p, k); }
  return p.slice(k);
}
export function createReceiverClassifier(context) {
  const tree = context?.tree, lookup = context?.scan;
  const scans = new Map();
  const scanOf = path => {
    need(tree?.has(path), 'module file missing from snapshot');
    if (!scans.has(path)) scans.set(path, lookup(path));
    return scans.get(path);
  };

  // ---- module ancestry: only `mod x;` declared at a crate-rooted file's top level ----
  const dirOf = path => CRATE_ROOTS.has(path) || path.endsWith('/mod.rs')
    ? path.slice(0, path.lastIndexOf('/')) : path.slice(0, -3);
  const moduleCache = new Map();
  function childFile(parent, name) {
    const key = parent + '#' + name;
    if (moduleCache.has(key)) return moduleCache.get(key);
    const scan = scanOf(parent);
    const decls = scan.items.filter(i => i.kind === 'mod' && i.name === name);
    need(decls.length === 1, 'module declaration missing or duplicated: ' + name);
    const [decl] = decls;
    need(decl.scope === scan.root && decl.bodyScope === null, 'inline or nested module route');
    need(decl.attributes.every(a => HARMLESS_ATTRS.has(a.meta.name)),
      'attribute on module declaration (cfg/path alternatives)');
    const base = dirOf(parent);
    const targets = [base + '/' + name + '.rs', base + '/' + name + '/mod.rs']
      .filter(p => tree.has(p));
    need(targets.length === 1, 'module file missing or duplicated: ' + name);
    moduleCache.set(key, targets[0]);
    return targets[0];
  }
  function fileOf(root, segments) {
    let file = root;
    for (const s of segments) file = childFile(file, s);
    return file;
  }
  // Ordinary module path of a file, verified by walking `mod` declarations from a crate root.
  function moduleOf(path) {
    const rel = path.replace(/^src-tauri\/src\//, '');
    need(rel !== path, 'file outside crate source');
    const root = CRATE_ROOTS.has(path) ? path : 'src-tauri/src/lib.rs';
    if (CRATE_ROOTS.has(path)) return { root, segments: [] };
    const segments = rel.replace(/(?:\/mod)?\.rs$/, '').split('/');
    need(fileOf(root, segments) === path, 'file not reached through ordinary module tree');
    return { root, segments };
  }
  // ---- path canonicalization: crate/self/super and declared child modules only ----
  function canonical(target, module) {
    const parts = target.split('::');
    let segments;
    if (parts[0] === 'crate') { segments = []; parts.shift(); }
    else if (parts[0] === 'self') { segments = [...module.segments]; parts.shift(); }
    else if (parts[0] === 'super') {
      segments = [...module.segments];
      while (parts[0] === 'super') {
        need(segments.length, 'super above crate root');
        segments.pop(); parts.shift();
      }
    } else {
      const here = scanOf(fileOf(module.root, module.segments));
      need(here.items.some(i => i.kind === 'mod' && i.name === parts[0] && i.scope === here.root),
        'external or unresolved path: ' + target);
      segments = [...module.segments];
    }
    need(parts.length >= 1, 'empty path');
    return { root: module.root, segments: [...segments, ...parts.slice(0, -1)],
      name: parts.at(-1) };
  }

  // Resolves a name defined at a module's top level to a unique trait item.
  function traitAt(location, visited = new Set()) {
    const file = fileOf(location.root, location.segments);
    const key = file + '#' + location.name;
    need(!visited.has(key), 'cyclic import route');
    visited.add(key);
    const scan = scanOf(file);
    checkFile(scan);
    const r = scan.resolvePath(location.name, scan.root);
    need(!r.ambiguous, r.reason ?? 'ambiguous trait binding');
    const binding = r.binding;
    need(binding, 'unresolved trait name: ' + location.name);
    if (binding.kind === 'import') {
      need(!binding.glob, 'glob reexport');
      const module = { root: location.root, segments: location.segments };
      return traitAt(canonical(binding.target, module), visited);
    }
    const kinds = new Set([
      'trait', 'struct', 'enum', 'union', 'type', 'fn', 'const', 'static', 'mod',
    ]);
    const items = scan.items.filter(i => i.name === location.name && i.scope === scan.root &&
      kinds.has(i.kind));
    need(items.length === 1 && items[0].kind === 'trait', 'name is not a unique trait');
    const item = items[0];
    need(item.attributes.every(a => HARMLESS_ATTRS.has(a.meta.name)),
      'attribute on trait declaration');
    return { path: file, scan, item, module: { root: location.root, segments: location.segments } };
  }

  // Resolves a path written in `scan` at `scope` to a trait, a marker, an Arc, tauri or nothing.
  function resolveType(scan, module, tokens) {
    need(simplePath(tokens), 'non-simple type path');
    const r = scan.resolvePath(tokens, tokens[0].scope);
    need(!r.ambiguous, r.reason ?? 'ambiguous type path');
    const text = tokens.map(t => t.name ?? t.text).join('');
    if (r.binding?.kind === 'import') {
      const target = r.path;
      if (/^tauri(?:::|$)/.test(target)) return { tauri: target };
      if (ARC.has(target)) return { arc: true };
      if (/^(?:std|core)::marker::(?:Send|Sync)$/.test(target)) return { marker: target };
      if (/^(?:std|core|alloc)::/.test(target)) return { external: target };
      return { trait: traitAt(canonical(target, module)) };
    }
    if (r.binding) {
      // A same-module declaration: bare local trait, or a shadowing marker/Arc name.
      need(r.binding.kind === 'shadow' && tokens.length === 1, 'non-item local binding');
      return { trait: traitAt({ ...module, name: text }) };
    }
    if (MARKERS.has(text)) return { marker: MARKERS.get(text) };
    if (text.startsWith('tauri::')) return { tauri: text };
    if (ARC.has(text)) return { arc: true };
    if (/^(?:crate|self|super)::/.test(text)) return { trait: traitAt(canonical(text, module)) };
    return unknown('unresolved type path: ' + text);
  }

  // Bounds: lifetimes, std Send/Sync and crate-local traits. Returns the local traits.
  function bounds(scan, module, tokens) {
    const traits = [];
    for (const part of split(tokens, '+')) {
      if (part.length === 1 && part[0].kind === 'lifetime') continue;
      const r = resolveType(scan, module, part);
      if (r.tauri) return { real: r.tauri };
      need(!r.arc && !r.external, 'unsupported bound');
      if (r.marker) continue;
      traits.push(r.trait);
    }
    return { traits };
  }

  // Trait header: no generics/where; supertraits are bounds. Collects the closure.
  function closure(trait, seen = new Map()) {
    const key = trait.path + '#' + trait.item.first;
    if (seen.has(key)) return seen;
    seen.set(key, trait);
    const { scan, item } = trait;
    const header = scan.tokens.slice(item.first + 2,
      scan.tokens.findIndex(t => t.start === item.bodyStart));
    need(item.bodyStart !== null, 'trait without body');
    if (!header.length) return seen;
    need(header[0].text === ':', 'generic or where-bounded trait');
    need(!header.some(t => t.name === 'where'), 'where-bounded trait');
    const b = bounds(scan, trait.module, header.slice(1));
    need(!b.real, 'trait has Tauri supertrait');
    for (const t of b.traits) closure(t, seen);
    return seen;
  }

  // In-scope traits that could also supply `method`, and inherent impls on the object type.
  function competition(scan, module, chosen, method) {
    for (const s of scan.scopes) {
      need(!s.globs.length, 'glob import may supply competing method');
      for (const b of s.bindings.values()) competingBinding(scan, module, chosen, method, s, b);
    }
    for (const path of tree.keys()) {
      if (!/^src-tauri\/src\/.*\.rs$/.test(path)) continue;
      competingDynImpls(scanOf(path), chosen, method);
    }
  }
  function competingBinding(scan, module, chosen, method, s, b) {
    if (b.kind === 'shadow') {
      competingShadow(scan, chosen, method, s, b);
      return;
    }
    if (b.kind !== 'import') return;
    competingImport(module, chosen, method, b);
  }
  function competingShadow(scan, chosen, method, s, b) {
    // Same-file traits are in scope too.
    const local = scan.items.find(i => i.kind === 'trait' && i.name === b.name &&
      i.scope === s);
    if (!local || local === chosen.trait.item) return;
    const declares = scan.items.some(i => i.kind === 'fn' && i.name === method &&
      i.start > local.start && i.end <= local.end);
    if (declares && openImpls({ item: local })) unknown('competing same-file trait method');
  }
  function competingImport(module, chosen, method, b) {
    if (/^tauri(?:::|$)/.test(b.target ?? '')) unknown('Tauri trait import in scope');
    if (/^(?:std|core|alloc)::/.test(b.target ?? '') || b.target === undefined) return;
    const local = importedTrait(module, b.target);
    if (local === null) return;
    if (local.item === chosen.trait.item) return;
    const declares = local.scan.items.some(i => i.kind === 'fn' && i.name === method &&
      i.start > local.item.start && i.end <= local.item.end);
    if (declares && openImpls(local)) unknown('competing in-scope trait method');
  }
  // Only Unknown means the import does not resolve to a trait; other errors propagate.
  function importedTrait(module, target) {
    try { return traitAt(canonical(target, module)); }
    catch (e) { if (e instanceof Unknown) { return null; } throw e; }
  }
  // True when a trait has an impl for a generic, object, reference, Box or Arc target.
  function openImpls(trait) {
    for (const path of tree.keys()) {
      if (!/^src-tauri\/src\/.*\.rs$/.test(path)) continue;
      const scan = scanOf(path);
      for (const impl of scan.items.filter(i => i.kind === 'impl')) {
        const head = scan.tokens.slice(impl.first + 1,
          scan.tokens.findIndex(t => t.start === impl.bodyStart));
        const f = head.findIndex(t => t.name === 'for');
        if (f < 0 || !head.slice(0, f).some(t => t.name === trait.item.name)) continue;
        const target = head.slice(f + 1);
        const generics = head[0]?.text === '<'
          ? split(head.slice(1, closeAngle(head, 0)), ',').map(p => p[0]?.name) : [];
        if (['&', 'dyn', 'impl'].includes(target[0]?.text ?? target[0]?.name) ||
          generics.includes(target[0]?.name) ||
          ['Box', 'Arc', 'Rc'].includes(target[0]?.name)) return true;
      }
    }
    return false;
  }

  // Receiver type grammar: [&['a][mut]] (dyn Bounds | Arc<dyn Bounds>).
  function objectType(scan, module, tokens) {
    let rest = tokens.slice(referenceEnd(tokens));
    if (rest[0]?.name !== 'dyn') {
      const wrapped = objectWrapper(scan, module, rest);
      if (wrapped.real) return wrapped;
      rest = wrapped.rest;
    }
    const b = bounds(scan, module, rest.slice(1));
    if (b.real) return b;
    need(b.traits.length === 1, 'trait object needs exactly one port trait');
    return { traits: b.traits };
  }
  // Non-dyn receiver: a plain Tauri path ({real}) or `Arc<dyn ...>` ({rest}: the dyn tokens).
  function objectWrapper(scan, module, rest) {
    const open = rest.findIndex(t => t.text === '<');
    if (open < 0 && simplePath(rest)) {
      const plain = resolveType(scan, module, rest);
      if (plain.tauri && TAURI_REAL.test(plain.tauri)) return { real: plain.tauri };
    }
    need(open > 0 && closeAngle(rest, open) === rest.length - 1, 'unsupported receiver type');
    const head = resolveType(scan, module, rest.slice(0, open));
    if (head.tauri) return { real: head.tauri };
    need(head.arc, 'unsupported wrapper type');
    const inner = rest.slice(open + 1, -1);
    need(inner[0]?.name === 'dyn', 'Arc without trait object');
    return { rest: inner };
  }

  // Generic parameter list `<A: B, C>` plus `where` clauses; returns name -> bound tokens.
  function genericBounds(scan, header, where) {
    const map = new Map();
    if (header.length) {
      for (const part of split(header, ',')) {
        if (part[0]?.kind === 'lifetime') continue;
        need(part[0]?.kind === 'ident' && part[0].name !== 'const', 'unsupported generic param');
        need(part.length === 1 || part[1].text === ':', 'unsupported generic param');
        map.set(part[0].name, part.slice(2));
      }
    }
    for (const part of where.length ? split(where, ',') : []) {
      need(part.length > 2 && part[0].kind === 'ident' && part[1].text === ':' &&
        map.has(part[0].name), 'unsupported where clause');
      map.get(part[0].name).push({ text: '+', kind: 'punct' }, ...part.slice(2));
    }
    for (const [name, tokens] of map) {
      const clean = tokens[0]?.text === '+' ? tokens.slice(1) : tokens;
      map.set(name, clean);
    }
    return map;
  }
  // ---- receiver binding safety inside the innermost fn ----
  // Shared extraction preserves emits' safety checks, order and diagnostic text.
  function receiverBinding(path, scan, dot) {
    const tokens = scan.tokens, i = dot.index;
    let field = null, recv = i - 1, start = i - 1;
    need(tokens[recv]?.kind === 'ident', 'non-identifier receiver');
    if (tokens[recv - 1]?.text === '.') {
      need(tokens[recv - 2]?.name === 'self', 'chained or projected receiver');
      field = tokens[recv].name; start = recv - 2;
    }
    const before = tokens[start - 1];
    need(before && (BOUNDARY.has(before.text) || BOUNDARY_WORDS.has(before.name)),
      'receiver is not a whole expression');
    need(tokens[start].name !== 'self' || field, 'self receiver');
    const fns = scan.items.filter(f => f.kind === 'fn' && f.bodyStart !== null &&
      f.bodyStart < dot.start && dot.start < f.end)
      .sort((a, b) => (a.end - a.start) - (b.end - b.start));
    need(fns.length && (fns.length === 1 || fns[1].end - fns[1].start > fns[0].end - fns[0].start),
      'no unique enclosing fn');
    const fn = fns[0];
    const bodyOpen = tokens.findIndex(t => t.start === fn.bodyStart);
    const body = { start: bodyOpen, end: tokens[bodyOpen].mate };
    need(!captured(tokens, body, i), 'receiver captured by closure or async block');
    const module = moduleOf(path);
    checkFile(scan);
    const { parts } = fnParams(scan, fn);
    return { field, recv, start, fn, body, module, parts };
  }

  function stateType(scan, module, tokens) {
    let k = 0;
    if (tokens[k]?.text === '&') {
      k++;
      if (tokens[k]?.kind === 'lifetime') k++;
      if (tokens[k]?.name === 'mut') k++;
    }
    const rest = tokens.slice(k), open = rest.findIndex(t => t.text === '<');
    let head = rest;
    if (open >= 0) {
      need(open > 0 && closeAngle(rest, open) === rest.length - 1,
        'unsupported state type arguments');
      need(simplePath(rest.slice(open + 1, -1)), 'state runtime is not a simple path');
      head = rest.slice(0, open);
    }
    const r = resolveType(scan, module, head);
    need(/^tauri::(?:AppHandle|App|Window|WebviewWindow)$/.test(r.tauri ?? ''),
      'state receiver is not a concrete Tauri handle');
    return { real: r.tauri };
  }

  function proveStateUnsafe(path, scan, dot) {
    const tokens = scan.tokens, i = dot.index;
    need(tree && lookup, 'missing receiver context');
    need(['state', 'try_state'].includes(tokens[i + 1]?.name) &&
      tokens[i + 2]?.text === '(' && tokens[i + 2].mate === i + 3,
      'not an empty inferred state call');
    need(!inMacro(scan, i), 'state inside macro');
    const { field, recv, start, fn, body, module, parts } = receiverBinding(path, scan, dot);
    let result, declaration;
    if (!field) {
      const name = tokens[recv].name;
      const param = parts.filter(p => p[0]?.name === name && p[1]?.text === ':');
      need(param.length === 1, 'receiver is not a simple fn parameter');
      need(!parts.some(p => p.some(t => t.name === name) && p !== param[0]),
        'receiver name reused in parameters');
      safeReceiver(scan, fn, body, name, param[0][0].index, i);
      result = stateType(scan, module, param[0].slice(2));
      declaration = { path, line: param[0][0].line };
    } else {
      checkFieldReceiver(tokens, parts, body, field);
      result = fieldType(scan, module, fn, field, stateType);
      declaration = result.declaration;
    }
    return { proved: true, reason: 'Tauri state receiver ' + result.real, evidence: {
      receiver: { path, line: tokens[start].line, start: tokens[start].start,
        end: tokens[recv].end }, declaration, type: result.real,
    } };
  }

  function classifyUnsafe(path, scan, dot) {
    const tokens = scan.tokens, i = dot.index, method = tokens[i + 1]?.name;
    need(tree && lookup, 'missing receiver context');
    need(['emit', 'emit_to'].includes(method) && tokens[i + 2]?.text === '(', 'not an emit call');
    need(!inMacro(scan, i), 'emit inside macro');
    const { field, recv, start, fn, body, module, parts } =
      receiverBinding(path, scan, dot);
    let proof, declaration;
    if (!field) {
      const name = tokens[recv].name;
      const param = parts.filter(p => p[0]?.name === name && p[1]?.text === ':');
      need(param.length === 1, 'receiver is not a simple fn parameter');
      need(!parts.some(p => p.some(t => t.name === name) && p !== param[0]),
        'receiver name reused in parameters');
      safeReceiver(scan, fn, body, name, param[0][0].index, i);
      const r = objectType(scan, module, param[0].slice(2));
      if (r.real) return { kind: REAL, reason: 'Tauri receiver ' + r.real };
      proof = r.traits; declaration = { path, line: param[0][0].line };
    } else {
      checkFieldReceiver(tokens, parts, body, field);
      const r = fieldType(scan, module, fn, field);
      if (r.real) return { kind: REAL, reason: 'Tauri receiver ' + r.real };
      proof = r.traits; declaration = r.declaration;
    }
    need(proof.length, 'no port trait bound');
    const traits = new Map();
    for (const t of proof) closure(t, traits);
    const chosen = declaring(traits, method);
    competition(scan, module, chosen, method);
    return { kind: PROVEN_PORT, reason: 'port trait ' + chosen.trait.item.name + '::' + method,
      evidence: {
        receiver: { path, line: tokens[start].line, start: tokens[start].start,
          end: tokens[recv].end },
        declaration,
        trait: { path: chosen.trait.path, line: chosen.trait.item.line,
          name: chosen.trait.item.name },
        method: { path: chosen.trait.path, line: chosen.fn.line },
      } };
  }

  // self.field: unique impl, unique same-module named-field struct, consistent binders.
  function fieldType(scan, module, fn, field, policy = objectType) {
    const tokens = scan.tokens;
    const impls = scan.items.filter(i => i.kind === 'impl' && i.start < fn.start &&
      fn.end <= i.end);
    need(impls.length === 1 && impls[0].scope === scan.root, 'no unique enclosing impl');
    const [impl] = impls;
    const ih = header(scan, impl, impl.first + 1);
    const f = ih.rest.findIndex(t => t.name === 'for');
    const target = f < 0 ? ih.rest : ih.rest.slice(f + 1);
    need(target[0]?.kind === 'ident', 'unsupported impl target');
    let args = [];
    if (target.length > 1) {
      need(target[1].text === '<' && closeAngle(target, 1) === target.length - 1,
        'unsupported impl target');
      args = split(target.slice(2, -1), ',');
      need(args.length, 'unsupported impl target');
    }
    const structs = scan.items.filter(s => s.kind === 'struct' && s.name === target[0].name &&
      s.scope === scan.root);
    need(structs.length === 1, 'impl target is not a unique same-module struct');
    const [struct] = structs;
    const r = scan.resolvePath(target[0].name, scan.root);
    need(!r.ambiguous && r.binding?.kind === 'shadow', 'impl target name not uniquely declared');
    need(struct.bodyStart !== null, 'struct without named fields');
    const sh = header(scan, struct, struct.first + 2);
    need(!sh.rest.length, 'unsupported struct header');
    const bodyOpen = tokens.findIndex(t => t.start === struct.bodyStart);
    const fields = split(tokens.slice(bodyOpen + 1, tokens[bodyOpen].mate), ',')
      .map(p => normalizeField(p));
    const hits = fields.filter(p => p[0]?.name === field && p[1]?.text === ':');
    need(hits.length === 1, 'field not uniquely declared');
    const type = hits[0].slice(2);
    const declaration = { path: scan.path, line: hits[0][0].line };
    const structGenerics = genericBounds(scan, sh.generics, sh.where);
    if (type.length === 1 && structGenerics.has(type[0].name)) {
      need(policy === objectType, 'generic field does not prove a concrete state handle');
      return genericFieldType(scan, module, ih, args, structGenerics, type, declaration);
    }
    const o = policy(scan, module, type);
    return { ...o, declaration };
  }
  // Field typed by a struct binder: struct and matching impl bounds, in that order.
  function genericFieldType(scan, module, ih, args, structGenerics, type, declaration) {
    const position = [...structGenerics.keys()].indexOf(type[0].name);
    const implGenerics = genericBounds(scan, ih.generics, ih.where);
    need(args.length === structGenerics.size && args.every(a => a.length === 1),
      'impl target arguments do not match struct binders');
    const arg = args[position][0].name;
    need(implGenerics.has(arg), 'impl target argument is not an impl generic');
    need(new Set(args.map(a => a[0].name)).size === args.length, 'repeated impl binder');
    const all = [];
    for (const tokensOf of [structGenerics.get(type[0].name), implGenerics.get(arg)]) {
      if (!tokensOf.length) continue;
      const b = bounds(scan, module, tokensOf);
      if (b.real) return { real: b.real };
      all.push(...b.traits);
    }
    return { traits: all, declaration };
  }

  return {
    proveState(path, currentScan, dotToken) {
      try { return proveStateUnsafe(path, currentScan, dotToken); }
      catch (e) {
        if (!(e instanceof Unknown)) throw e;
        return { proved: false, reason: 'state receiver not proved: ' + e.message };
      }
    },
    classify(path, currentScan, dotToken) {
      try { return classifyUnsafe(path, currentScan, dotToken); }
      catch (e) {
        if (!(e instanceof Unknown)) throw e;
        return { kind: UNKNOWN, reason: 'emit receiver not proved: ' + e.message };
      }
    },
  };
}
