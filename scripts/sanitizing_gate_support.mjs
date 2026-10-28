import { createHash } from 'node:crypto';
import { codeOrder } from './sanitizing_baseline.mjs';
import { createRustScanner } from './sanitizing_rust_scan.mjs';

export function resolveAlias(scan, input, scope, relevantPath) {
  const resolved = scan.resolvePath(input,scope);
  if (!resolved.ambiguous) return resolved;
  const raw = typeof input === 'string' ? input : input.map(t => t.name ?? t.text).join('');
  const suffix = raw.split('::').slice(1).join('::');
  const candidates = [resolved.path,raw];
  for (let b = resolved.binding; b; b = b.previous) if (b.target)
    candidates.push(b.target + (suffix ? '::' + suffix : ''));
  return { ...resolved, path: candidates.find(p => p && relevantPath.test(p)) ?? resolved.path };
}

export function qualified(scan, tokens, i, relevantPath) {
  let end = i + 1;
  while (tokens[end]?.text === '::' && tokens[end + 1]?.kind === 'ident') end += 2;
  const start = tokens[i - 1]?.text === '::' && tokens[i - 2]?.kind !== 'ident' ? i - 1 : i;
  const resolved = resolveAlias(scan,tokens.slice(start, end),tokens[i].scope,relevantPath);
  return { ...resolved, end, raw: tokens.slice(start, end).map(t => t.name ?? t.text).join('') };
}

// A transfer is proven only by unchanged item tokens and resolved bindings.
// Paths, enclosing modules, whitespace and comments do not identify an occurrence.
// Unknown/ambiguous bindings get no transfer credit; this is deliberately finite.
const itemIdentities = new WeakMap();
export function occurrenceIdentity(scan, token, declaration) {
  const item = scan.items.filter(i => !['mod', 'impl', 'trait'].includes(i.kind) &&
    i.start <= token.start && token.start < i.end).sort((a,b) => (a.end-a.start)-(b.end-b.start))[0];
  if (!item && declaration) return tokenIdentity(scan,declaration,token);
  if (!item) return undefined;
  if (!itemIdentities.has(item)) {
    const tokens = scan.tokens.filter(t => t.start >= item.start && t.start < item.end);
    const hash = tokenHash(scan,tokens);
    itemIdentities.set(item, hash ? { hash, tokens } : null);
  }
  const identity = itemIdentities.get(item);
  return identity ? identity.hash + ':' + identity.tokens.indexOf(token) : undefined;
}

// Match retained sites first, then spend each removed occurrence at most once.
// Equal totals alone never prove that code in a new file was moved.
function transferredSites(byPath, rules) {
  const moved = new Set();
  for (const rule of rules) {
    const removed = new Map(), added = [];
    for (const path of new Set([...byPath[0].keys(), ...byPath[1].keys()])) {
      const available = new Map();
      for (const site of byPath[0].get(path)?.sites[rule] ?? []) if (site.identity)
        available.set(site.identity, (available.get(site.identity) ?? 0) + 1);
      for (const site of byPath[1].get(path)?.sites[rule] ?? []) {
        const n = available.get(site.identity) ?? 0;
        if (n) available.set(site.identity, n - 1);
        else added.push(site);
      }
      for (const [id,n] of available) removed.set(id, (removed.get(id) ?? 0) + n);
    }
    for (const site of added) {
      const n = removed.get(site.identity) ?? 0;
      if (n) { moved.add(site); removed.set(site.identity, n - 1); }
    }
  }
  return moved;
}

function tokenHash(scan, tokens) {
  const normalized = [];
  for (const [i,t] of tokens.entries()) {
    // A declaration's own name is not a reference to its initializer binding.
    const declaration = ['static','const'].includes(tokens[i - 1]?.name) ||
      tokens[i - 1]?.name === 'mut' && tokens[i - 2]?.name === 'static';
    const r = t.kind === 'ident' && !declaration ? scan.resolvePath(t.name,t.scope) : null;
    if (r?.ambiguous) return undefined;
    normalized.push([t.kind,t.text,r?.path ?? null]);
  }
  return createHash('sha256').update(JSON.stringify(normalized)).digest('hex');
}
function tokenIdentity(scan, tokens, token) {
  const hash = tokenHash(scan,tokens);
  return hash ? hash + ':' + tokens.indexOf(token) : undefined;
}

export function metric() { return { counts: {}, sites: {}, commands: new Map(), warnings: [] }; }

export function hit(m, rule, line, identity) {
  m.counts[rule] = (m.counts[rule] ?? 0) + 1;
  const site = { line, ...(identity ? { identity } : {}) };
  (m.sites[rule] ??= []).push(site);
  return site;
}

export function warning(m, path, line, resolved) {
  if (resolved.ambiguous) m.warnings.push({ path, line, reason: resolved.reason ?? 'ambiguous relevant alias',
    remedy: 'use an explicit unambiguous import; signal counted conservatively' });
}

export function runMeasuredGate(snapshot, { gate, rules, newFileRules, inUniverse, measure, extraChecks, recognizeMoves = false }) {
  const diagnostics = [], measurements = { base: [], head: [] };
  try {
    const trees = [snapshot.baseTree, snapshot.headTree], totals = [{}, {}], byPath = [new Map(), new Map()];
    for (const [side, tree] of trees.entries()) {
      const scan = createRustScanner(tree, snapshot.readBlob);
      for (const path of [...tree.keys()].filter(path => inUniverse(path)).sort(codeOrder)) {
        const m = measure(path, scan(path));
        byPath[side].set(path, m);
        for (const rule of rules) totals[side][rule] = (totals[side][rule] ?? 0) + (m.counts[rule] ?? 0);
        measurements[side ? 'head' : 'base'].push({ path, counts: m.counts, sites: m.sites });
        if (side) diagnostics.push(...m.warnings.map(d => ({ gate, status: 'WARN', ...d })));
      }
    }
    const violation = (rule, path, line, base, head, reason) => diagnostics.push({ gate, status: 'FAIL', rule, path, line,
      base, head, baseOid: snapshot.baseOid, headOid: snapshot.headOid, reason,
      remedy: 'remove the new signal or use the source-defined owner; do not raise the baseline' });
    for (const rule of rules) if ((totals[1][rule] ?? 0) > (totals[0][rule] ?? 0)) {
      const source = [...byPath[1]].find(([p,m]) => (m.counts[rule] ?? 0) > (byPath[0].get(p)?.counts[rule] ?? 0));
      violation(rule, source?.[0] ?? '<universe>', source?.[1].sites[rule]?.[0]?.line ?? 1,
        totals[0][rule] ?? 0, totals[1][rule], 'global subrule debt increased');
    }
    const moved = recognizeMoves ? transferredSites(byPath, rules) : new Set();
    for (const [path, m] of byPath[1]) {
      if (!snapshot.baseTree.has(path)) for (const rule of newFileRules) if (m.sites[rule]?.some(site => !moved.has(site)))
        violation(rule, path, m.sites[rule].find(site => !moved.has(site)).line, 0, m.counts[rule], 'new file must satisfy this subrule even with compensated totals');
      extraChecks(path, m, byPath[0].get(path), violation, moved);
    }
    snapshot.assertStableHead();
    const fail = diagnostics.some(d => d.status === 'FAIL');
    return { gate, status: fail ? 'FAIL' : 'PASS', exit: fail ? 1 : 0, requestedRef: snapshot.requestedRef,
      refOid: snapshot.refOid, baseOid: snapshot.baseOid, headOid: snapshot.headOid, totals, measurements, diagnostics };
  } catch (e) {
    return { gate, status: 'ERROR', exit: e.exit ?? 3, baseOid: snapshot.baseOid, headOid: snapshot.headOid,
      diagnostics: [...diagnostics, { gate, reason: e.message, remedy: 'repair inputs and rerun; no verdict available' }] };
  }
}
