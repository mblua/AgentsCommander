// Finite lexical calls; receiver methods are an explicitly separate approximation.
import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit, warning, qualified } from './sanitizing_gate_support.mjs';

const diskPath = /^(?:(?:std|tokio)::fs::|fs::|(?:File::(?:open|create)|OpenOptions::new)$)/;
const bareOperation = /^(?:read|read_to_string|read_dir|write|metadata|symlink_metadata|canonicalize|copy|rename|create_dir|create_dir_all|remove_file|remove_dir|remove_dir_all|hard_link|soft_link|read_link|set_permissions|try_exists)$/;
const receiverMethod = /^(?:exists|try_exists|metadata|symlink_metadata|canonicalize|is_file|is_dir|read_dir)$/;
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
    if (t.text === '<') { depth++; if (depth === 1) { entry = true; continue; } }
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
  if (!tokens.length || !tokens.every((t, n) =>
    (n + (tokens[0].text === '::' ? 1 : 0)) % 2 ? t.text === '::' : t.kind === 'ident'))
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

function parameterType(scan, fn, T, name) {
  let open = fn.first + 2;
  if (T[open]?.text === '<') {
    while (open < T.length && T[open].text !== '(') open = T[open].mate ?? open + 1;
  }
  if (T[open]?.text !== '(') return null;
  let part = [];
  for (let k = open + 1; k <= T[open].mate; k++) {
    if (k === T[open].mate || T[k].text === ',') {
      let n = part[0]?.name === 'mut' ? 1 : 0;
      if (part[n]?.kind === 'ident' && part[n].name === name && part[n + 1]?.text === ':') {
        n += 2;
        if (part[n]?.text === '&') {
          n++;
          if (part[n]?.kind === 'lifetime') n++;
          if (part[n]?.name === 'mut') n++;
        }
        return typedPath(scan, part.slice(n));
      }
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
  for (let k = fn.body + 1; k < at; k++) {
    const t = T[k];
    if (t.text === '|' || t.text === '||' || ['for', 'match'].includes(t.name) ||
      bodyItems.has(t.name)) return false;
    if (t.text === '!' && T[k - 1]?.kind === 'ident' && !notMacro.has(T[k - 1].name) &&
      ['(', '[', '{'].includes(T[k + 1]?.text)) return false;
    if (t.name !== 'let') continue;
    let n = k + 1;
    if (T[n]?.name === 'ref') n++;
    if (T[n]?.name === 'mut') n++;
    if (T[n]?.kind !== 'ident' || ![':', '=', ';'].includes(T[n + 1]?.text)) return false;
    if (T[n].name === p.name) return false;
  }
  return true;
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
    if (t.kind !== 'ident' || !receiverMethod.test(t.name) || T[k - 1]?.text !== '.' ||
      T[k + 1]?.text !== '(' || T[k + 1].mate !== k + 2) continue;
    if (ranges.some(r => k > r.start && k < r.end) ||
      generic.some(i => t.start >= i.start && t.start < i.end)) continue;
    if (constructorReceiver(scan, T, k - 2) || parameterReceiver(scan, T, k - 2, fns))
      hit(m, 'receiver_disk_sites', t.line);
  }
}

function measure(path, scan) {
  const m = metric(), ts = scan.productionTokens;
  if (owner(path)) return m;
  for (let i = 0; i < ts.length; i++) {
    const t = ts[i];
    if (t.kind !== 'ident') continue;
    if (ts[i - 1]?.text === '.') {
      if (receiverMethod.test(t.name) && ts[i + 1]?.text === '(')
        hit(m, 'receiver_method_approximation', t.line);
      continue;
    }
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
