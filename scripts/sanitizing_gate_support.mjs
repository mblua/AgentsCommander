import { codeOrder } from './sanitizing_baseline.mjs';
import { scanRust } from './sanitizing_rust_scan.mjs';

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

export function metric() { return { counts: {}, sites: {}, commands: new Map(), warnings: [] }; }

export function hit(m, rule, line, identity) {
  m.counts[rule] = (m.counts[rule] ?? 0) + 1;
  (m.sites[rule] ??= []).push({ line, ...(identity ? { identity } : {}) });
}

export function warning(m, path, line, resolved) {
  if (resolved.ambiguous) m.warnings.push({ path, line, reason: resolved.reason ?? 'ambiguous relevant alias',
    remedy: 'use an explicit unambiguous import; signal counted conservatively' });
}

export function runMeasuredGate(snapshot, { gate, rules, newFileRules, inUniverse, measure, extraChecks }) {
  const diagnostics = [], measurements = { base: [], head: [] };
  try {
    const trees = [snapshot.baseTree, snapshot.headTree], totals = [{}, {}], byPath = [new Map(), new Map()];
    for (const [side, tree] of trees.entries()) {
      for (const path of [...tree.keys()].filter(inUniverse).sort(codeOrder)) {
        const m = measure(path, scanRust(path, snapshot.readBlob(tree.get(path))));
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
    for (const [path, m] of byPath[1]) {
      if (!snapshot.baseTree.has(path)) for (const rule of newFileRules) if (m.counts[rule])
        violation(rule, path, m.sites[rule][0].line, 0, m.counts[rule], 'new file must satisfy this subrule even with compensated totals');
      extraChecks(path, m, byPath[0].get(path), violation);
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
