import { codeOrder, gateCli, isMain } from './sanitizing_baseline.mjs';
import { scanRust } from './sanitizing_rust_scan.mjs';

const relevantPath = new RegExp("(?:^|::)(?:test)$");
function resolveAlias(scan, input, scope) {
  const resolved = scan.resolvePath(input,scope);
  if (!resolved.ambiguous) return resolved;
  const raw = typeof input === 'string' ? input : input.map(t => t.name ?? t.text).join('');
  const suffix = raw.split('::').slice(1).join('::');
  const candidates = [resolved.path,raw];
  for (let b = resolved.binding; b; b = b.previous) if (b.target)
    candidates.push(b.target + (suffix ? '::' + suffix : ''));
  return { ...resolved, path: candidates.find(p => p && relevantPath.test(p)) ?? resolved.path };
}

// Finite syntactic signals, using the shared lexer, scope and production model.
function metric() { return { counts: {}, sites: {}, commands: new Map(), warnings: [] }; }
function hit(m, rule, line, identity) {
  m.counts[rule] = (m.counts[rule] ?? 0) + 1;
  (m.sites[rule] ??= []).push({ line, ...(identity ? { identity } : {}) });
}
function warning(m, path, line, resolved) {
  if (resolved.ambiguous) m.warnings.push({ path, line, reason: resolved.reason ?? 'ambiguous relevant alias',
    remedy: 'use an explicit unambiguous import; signal counted conservatively' });
}
export function runGate(snapshot) {
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

const gate = '05';
const rules = ['mod_rs', 'test_files', 'inline_mod', 'path', 'glob', 'pub_mod', 'pub_super_mod', 'pub_in_mod'];
const newFileRules = rules.slice(0, 5);
const inUniverse = p => /^src-tauri\/(?:src|tests)\/.*\.rs$/.test(p);
const extraChecks = () => {};
function measure(path, scan) {
  const m = metric(), integration = path.startsWith('src-tauri/tests/') || /(?:^|\/)tests\//.test(path);
  if (path.endsWith('/mod.rs')) hit(m, 'mod_rs', 1);
  let testLine;
  for (const a of scan.attributes) for (const meta of a.signals) {
    const r = resolveAlias(scan,meta.name,a.scope);
    if (['test', 'tokio::test', 'async_std::test'].includes(r.path) || r.ambiguous && /(?:^|::)test$/.test(r.path ?? meta.name)) {
      testLine ??= a.line; warning(m, path, a.line, r);
    }
    if (meta.name === 'path') hit(m, 'path', a.line);
  }
  if (testLine && !path.endsWith('/tests.rs') && !integration) hit(m, 'test_files', testLine);
  const ts = scan.tokens;
  for (const item of scan.items) {
    if (item.kind === 'mod') {
      if (item.bodyScope) hit(m, 'inline_mod', item.line);
      // Visibility belongs to this item, including attributes preceding pub(...).
      const prefix = ts.filter(t => t.start >= item.start && t.index < item.first);
      const pub = prefix.findIndex(t => t.name === 'pub');
      if (pub >= 0) {
        if (prefix[pub + 1]?.text !== '(') {
          if (path !== 'src-tauri/src/lib.rs') hit(m, 'pub_mod', item.line);
        } else if (prefix[pub + 2]?.name === 'super') hit(m, 'pub_super_mod', item.line);
        else if (prefix[pub + 2]?.name === 'in') hit(m, 'pub_in_mod', item.line);
      }
    }
  }
  for (const scope of scan.scopes) for (const entry of scope.imports) if (entry.glob) {
    // C6: a file under a tests/ folder is part of its tests.rs, so it may glob its parent too.
    const testsFile = path.endsWith('/tests.rs') || path.includes('/tests/');
    if (testsFile && entry.target === 'super::*') continue;
    const use = scan.items.find(i => i.kind === 'use' && i.scope === scope &&
      ts.slice(i.first, i.last).some(t => t.text === '*'));
    hit(m, 'glob', use?.line ?? 1);
  }
  return m;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
