// Finite lexical calls; receiver methods are an explicitly separate approximation.
import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit, warning, qualified } from './sanitizing_gate_support.mjs';

const diskPath = /^(?:(?:std|tokio)::fs::|fs::|(?:File::(?:open|create)|OpenOptions::new)$)/;
const bareOperation = /^(?:read|read_to_string|read_dir|write|metadata|symlink_metadata|canonicalize|copy|rename|create_dir|create_dir_all|remove_file|remove_dir|remove_dir_all|hard_link|soft_link|read_link|set_permissions|try_exists)$/;
const receiverMethod =
  /^(?:exists|try_exists|metadata|symlink_metadata|canonicalize|is_file|is_dir|read_dir)$/;
const owner = p => /^src-tauri\/src\/(?:filesystem|path_identity|path_utils)(?:\.rs$|\/)/.test(p);
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);

// #4014: a finite subset of receivers whose standard Path/PathBuf type is lexically proven.
// Unsupported or ambiguous provenance yields no site (it stays in the WARN approximation).
const stdPaths = new Set(['std::path::Path', 'std::path::PathBuf']);
const bodyItems = new Set(['fn', 'static', 'const', 'struct', 'enum', 'union', 'trait', 'impl',
  'type', 'use', 'mod', 'extern']);
const notMacro = new Set(['if', 'while', 'return', 'match', 'in', 'else', 'break', 'as', 'let',
  'mut', 'ref', 'move', 'yield']);

// Every std is a path root and no glob/unclassified use can bind it.
function stdGuard(scan) {
  const T = scan.tokens, uses = new Set(scan.items.filter(i => i.kind === 'use').map(i => i.first));
  if (scan.scopes.some(s => s.globs.length)) return false;
  return T.every((t, k) => t.kind !== 'ident' ||
    (t.name !== 'std' || T[k + 1]?.text === '::') && (t.name !== 'use' || uses.has(k)));
}

// Declaration list after fn/impl/trait: true only when absent or lifetime-only.
function lifetimeOnly(T, open) {
  if (T[open]?.text !== '<') return true;
  let depth = 0, entry = true;
  for (let k = open; k < T.length; k++) {
    const t = T[k];
    if (t.mate !== undefined && t.mate > k) { k = t.mate; entry = false; continue; }
    if (t.text === '<' && ++depth === 1) { entry = true; continue; }
    else if (t.text === '>' && --depth === 0) return true;
    else if (depth === 1 && t.text === ',') { entry = true; continue; }
    if (entry && depth === 1 && t.kind !== 'lifetime') return false;
    entry = false;
  }
  return false;
}
function declarationOpen(T, item) {
  return item.kind === 'impl' ? item.first + 1 : item.first + 2;
}

function typedPath(scan, tokens) {
  if (!tokens.length) return null;
  const lead = tokens[0].text === '::' ? 1 : 0;
  if (!tokens.every((t, n) => (n + lead) % 2 ? t.text === '::' : t.kind === 'ident'))
    return null;
  const r = scan.resolvePath(tokens, tokens[0].scope);
  return !r.ambiguous && stdPaths.has(r.path) ? r.path : null;
}

// Form 1: `Path::new(x)`, `PathBuf::from(x)` or `PathBuf::new()` immediately before `.m()`.
function constructorReceiver(scan, T, close) {
  if (T[close]?.text !== ')') return false;
  const open = T[close].mate, ctor = T[open - 1], empty = open + 1 === close;
  if (ctor?.kind !== 'ident' || T[open - 2]?.text !== '::' || T[open - 3]?.kind !== 'ident')
    return false;
  let start = open - 3;
  while (T[start - 1]?.text === '::' && T[start - 2]?.kind === 'ident') start -= 2;
  if (T[start - 1]?.text === '::') start--;
  if (['::', '>', '.'].includes(T[start - 1]?.text)) return false;
  const type = typedPath(scan, T.slice(start, open - 2));
  if (type === 'std::path::Path') return ctor.name === 'new' && !empty;
  if (type === 'std::path::PathBuf')
    return ctor.name === 'from' ? !empty : ctor.name === 'new' && empty;
  return false;
}

// Index after the optional generic prefix; it may not be '(' (callers reject that).
function parameterListOpen(T, fn) {
  let open = fn.first + 2;
  if (T[open]?.text === '<') {
    while (open < T.length && T[open].text !== '(') open = T[open].mate ?? open + 1;
  }
  return open;
}

// undefined: part does not declare name; null: it does, with an unsupported type.
function parameterPartType(scan, part, name) {
  let n = part[0]?.name === 'mut' ? 1 : 0;
  if (!(part[n]?.kind === 'ident' && part[n].name === name && part[n + 1]?.text === ':'))
    return undefined;
  n += 2;
  if (part[n]?.text === '&') {
    n++;
    if (part[n]?.kind === 'lifetime') n++;
    if (part[n]?.name === 'mut') n++;
  }
  return typedPath(scan, part.slice(n));
}

function parameterType(scan, fn, T, name) {
  const open = parameterListOpen(T, fn);
  if (T[open]?.text !== '(') return null;
  let part = [];
  for (let k = open + 1; k <= T[open].mate; k++) {
    if (k === T[open].mate || T[k].text === ',') {
      const type = parameterPartType(scan, part, name);
      if (type !== undefined) return type;
      part = [];
    } else {
      part.push(T[k]);
      if (T[k].mate > k) { part.push(...T.slice(k + 1, T[k].mate + 1)); k = T[k].mate; }
    }
  }
  return null;
}

// Form 2: a lexical prefix from the innermost fn body with no rebinding of `name`.
function parameterReceiver(scan, T, at, fns) {
  const p = T[at];
  if (p?.kind !== 'ident' || ['.', '::'].includes(T[at - 1]?.text)) return false;
  const fn = fns.filter(f => f.body < at && at < T[f.body].mate)
    .sort((a, b) => b.body - a.body)[0];
  if (!fn || !parameterType(scan, fn.item, T, p.name)) return false;
  return parameterPrefixSafe(T, fn.body, at, p.name);
}

function parameterPrefixSafe(T, body, at, name) {
  for (let k = body + 1; k < at; k++) {
    const t = T[k];
    if (t.text === '|' || t.text === '||' || ['for', 'match'].includes(t.name) ||
      bodyItems.has(t.name)) return false;
    if (t.text === '!' && T[k - 1]?.kind === 'ident' && !notMacro.has(T[k - 1].name) &&
      ['(', '[', '{'].includes(T[k + 1]?.text)) return false;
    if (t.name === 'let' && !parameterLetSafe(T, k, name)) return false;
  }
  return true;
}

// A simple let in the prefix is crossed only when it binds another name.
function parameterLetSafe(T, k, name) {
  let n = k + 1;
  if (T[n]?.name === 'ref') n++;
  if (T[n]?.name === 'mut') n++;
  if (T[n]?.kind !== 'ident' || ![':', '=', ';'].includes(T[n + 1]?.text)) return false;
  return T[n].name !== name;
}

// #4025: one immutable local, seeded directly by a standard constructor or parameter.
// This finite proof accepts PathBuf trait overrides as the documented lexical limit.
function oneArgument(T, open) {
  const close = T[open]?.mate;
  if (T[open]?.text !== '(' || close === undefined || open + 1 === close) return false;
  for (let k = open + 1; k < close; k++) {
    if (T[k].text === ',') return k === close - 1 && k > open + 1;
    if (T[k].mate > k) k = T[k].mate;
  }
  return true;
}

// Index after a standard constructor seed `T::f(...)` opened at open, or null.
function constructorSeedNext(scan, T, open) {
  const close = T[open].mate;
  if (!constructorReceiver(scan, T, close)) return null;
  if (T[open - 1].name !== 'new' || open + 1 !== close) {
    if (!oneArgument(T, open)) return null;
  }
  return close + 1;
}

// Index of the first join after a proven parameter seed at start, or null.
function parameterSeedNext(scan, T, start, fns) {
  const fn = fns.filter(f => f.body < start && start < T[f.body].mate)
    .sort((a, b) => b.body - a.body)[0];
  if (!fn || T[start]?.kind !== 'ident' ||
    !parameterType(scan, fn.item, T, T[start].name)) return null;
  for (let k = fn.body + 1; k < start; k++) {
    if (T[k].name !== 'let') continue;
    const d = localDeclaration(T, k, T[fn.body].mate);
    if (!d || T[d.name].name === T[start].name) return null;
  }
  const next = start + 1;
  // Unlike constructors, a parameter must have at least one join suffix.
  return T[next]?.text === '.' ? next : null;
}

function localInitializer(scan, T, start, end, fns) {
  let next = start;
  while (next < end && (T[next].kind === 'ident' || T[next].text === '::')) next++;
  if (T[next]?.text === '(' && T[next].mate < end) next = constructorSeedNext(scan, T, next);
  else next = parameterSeedNext(scan, T, start, fns);
  if (next === null) return false;
  while (next < end) {
    if (T[next]?.text !== '.' || T[next + 1]?.name !== 'join' ||
      !oneArgument(T, next + 2)) return false;
    next = T[next + 2].mate + 1;
  }
  return next === end;
}

// A complete ordinary let can be crossed, but only its exact immutable form is a seed.
function localDeclaration(T, at, limit) {
  let name = at + 1;
  if (T[name]?.name === 'ref') name++;
  if (T[name]?.name === 'mut') name++;
  if (T[name]?.kind !== 'ident' || ![':', '=', ';'].includes(T[name + 1]?.text))
    return null;
  if (['if', 'while', '&&'].includes(T[at - 1]?.text)) return null;
  let equal = null;
  for (let k = name + 1; k < limit; k++) {
    if (T[k].name === 'else') return null;
    if (T[k].text === '=' && equal === null) equal = k;
    if (T[k].text === ';') return { at, name, equal, end: k,
      exact: name === at + 1 && equal === name + 1 };
    if (T[k].mate > k) k = T[k].mate;
  }
  return null;
}

// Later items also shadow earlier bindings. Only original item declarations count.
function localBodyHasItems(scan, T, body, limit) {
  return scan.items.some(i => i.first > body && i.first < limit) ||
    T.slice(body + 1, limit).some((t, n) =>
      (t.name === 'extern' || t.name === 'macro_rules') &&
      !['.', '::'].includes(T[body + n]?.text));
}

// Lets before at, or null when a barrier, macro or non-simple let rejects the prefix.
function localPrefixDeclarations(T, body, at, limit) {
  const declarations = [];
  for (let k = body + 1; k < at; k++) {
    const t = T[k];
    if (['|', '||'].includes(t.text) || ['for', 'match'].includes(t.name)) return null;
    if (t.text === '!' && T[k - 1]?.kind === 'ident' && !notMacro.has(T[k - 1].name) &&
      ['(', '[', '{'].includes(T[k + 1]?.text)) return null;
    if (t.name !== 'let') continue;
    const declaration = localDeclaration(T, k, limit);
    if (!declaration) return null;
    declarations.push(declaration);
  }
  return declarations;
}

function localReceiver(scan, T, at, fns) {
  const p = T[at];
  if (p?.kind !== 'ident' || ['.', '::'].includes(T[at - 1]?.text)) return false;
  const fn = fns.filter(f => f.body < at && at < T[f.body].mate)
    .sort((a, b) => b.body - a.body)[0];
  if (!fn) return false;
  const limit = T[fn.body].mate;
  if (localBodyHasItems(scan, T, fn.body, limit)) return false;
  const declarations = localPrefixDeclarations(T, fn.body, at, limit);
  if (declarations === null) return false;
  const matches = declarations.filter(d => T[d.name].name === p.name);
  if (matches.length !== 1) return false;
  const d = matches[0];
  // A parameter of any type already declares the same name.
  if (T.slice(fn.item.first, fn.body).some((t, k, ts) =>
    t.name === p.name && ts[k + 1]?.text === ':')) return false;
  if (!d.exact || d.end >= at) return false;
  let scope = p.scope;
  while (scope && scope !== T[d.at].scope) scope = scope.parent;
  return !!scope && localInitializer(scan, T, d.equal + 1, d.end, fns);
}

// Form 3: associated `std::path::Path::m(x)` with exactly one argument.
function associatedPathReceiver(scan, T, k) {
  let start = k - 2;
  if (T[start]?.kind !== 'ident') return false;
  while (T[start - 1]?.text === '::' && T[start - 2]?.kind === 'ident') start -= 2;
  if (T[start - 1]?.text === '::') start--;
  if (['.', '>', '::'].includes(T[start - 1]?.text)) return false;
  return typedPath(scan, T.slice(start, k - 1)) === 'std::path::Path' && oneArgument(T, k + 1);
}

// Whether the receiver method call at k is a proven site; `::` never falls through to `.`.
function receiverAt(scan, T, k, fns) {
  if (T[k - 1]?.text === '::') return associatedPathReceiver(scan, T, k);
  if (T[k - 1]?.text !== '.' || T[k + 1].mate !== k + 2) return false;
  return constructorReceiver(scan, T, k - 2) || parameterReceiver(scan, T, k - 2, fns) ||
    localReceiver(scan, T, k - 2, fns);
}

function receiverSites(m, scan) {
  if (!stdGuard(scan)) return;
  const T = scan.tokens, ranges = [
    ...scan.groups.filter(g => T[g.start - 1]?.text === '!' || T[g.start - 2]?.text === '!'),
    ...scan.attributes.map(a => ({ start: a.first, end: a.last })),
  ];
  const generic = scan.items.filter(i => ['fn', 'impl', 'trait'].includes(i.kind) &&
    !lifetimeOnly(T, declarationOpen(T, i)));
  const fns = scan.items.filter(i => i.kind === 'fn' && i.bodyScope)
    .map(item => ({ item, body: item.bodyScope.open }));
  for (const t of scan.productionTokens) {
    const k = t.index;
    if (t.kind !== 'ident' || !receiverMethod.test(t.name) ||
      T[k + 1]?.text !== '(') continue;
    if (ranges.some(r => k > r.start && k < r.end) ||
      generic.some(i => t.start >= i.start && t.start < i.end)) continue;
    if (receiverAt(scan, T, k, fns)) hit(m, 'receiver_disk_sites', t.line);
  }
}

function receiverCall(m, ts, i, t) {
  if (receiverMethod.test(t.name) && ts[i + 1]?.text === '(')
    hit(m, 'receiver_method_approximation', t.line);
}

function measure(path, scan) {
  const m = metric(), ts = scan.productionTokens;
  if (owner(path)) return m;
  for (let i = 0; i < ts.length; i++) {
    const t = ts[i];
    if (t.kind !== 'ident') continue;
    if (ts[i - 1]?.text === '.') { receiverCall(m, ts, i, t); continue; }
    if (ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident') continue;
    const r = qualified(scan, ts, i, diskPath);
    const value = r.path ?? '';
    const ambiguousOperation = r.ambiguous && bareOperation.test(value);
    if (ts[r.end]?.text !== '(' || !(diskPath.test(value) || ambiguousOperation)) continue;
    // Imports/qualified paths identify std/tokio; bare File is the established
    // finite signal unless the resolver proves a local homonymous binding.
    warning(m, path, t.line, r);
    if (!m.counts.disk_files) hit(m, 'disk_files', t.line);
    i = r.end - 1;
  }
  receiverSites(m, scan);
  return m;
}

export function runGate(snapshot) {
  const rules = ['disk_files', 'receiver_disk_sites'];
  const result = runMeasuredGate(snapshot, {
    gate: '07', rules, newFileRules: rules,
    inUniverse, measure, extraChecks() {},
  });
  if (result.measurements) {
    result.approximation = Object.fromEntries(['base', 'head'].map(side => {
      const files = result.measurements[side].filter(m => m.counts.receiver_method_approximation);
      return [side, { files: files.length, sites: files.reduce((n, m) =>
        n + m.counts.receiver_method_approximation, 0), paths: files.map(m => m.path) }];
    }));
    result.diagnostics.push({ gate: '07', status: 'WARN', rule: 'receiver_method_approximation',
      path: '<untyped receivers>', line: 1,
      reason: 'Receiver methods are lexical approximations: only the finite proven Path/PathBuf ' +
        'subset enters receiver_disk_sites; the rest are untyped, may be homonyms, ' +
        'and none enter disk_files totals.',
      ...result.approximation.head });
  }
  return result;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
