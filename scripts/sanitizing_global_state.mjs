import { codeOrder, gateCli, isMain } from './sanitizing_baseline.mjs';
import { scanRust } from './sanitizing_rust_scan.mjs';

const relevantPath = /(?:^|::)(?:Mutex|RwLock|Atomic\w*|OnceLock|OnceCell|Lazy|LazyLock|Cell|RefCell)$/;
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
function qualified(scan, tokens, i) {
  let end = i + 1;
  while (tokens[end]?.text === '::' && tokens[end + 1]?.kind === 'ident') end += 2;
  const start = tokens[i - 1]?.text === '::' && tokens[i - 2]?.kind !== 'ident' ? i - 1 : i;
  const resolved = resolveAlias(scan,tokens.slice(start, end),tokens[i].scope);
  return { ...resolved, end, raw: tokens.slice(start, end).map(t => t.name ?? t.text).join('') };
}
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

const gate = '08';
const rules = ['globals'];
const newFileRules = rules;
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);
const extraChecks = () => {};
const stateType = /(?:^|::)(?:Mutex|RwLock|Atomic\w*|OnceLock|OnceCell|Lazy|LazyLock)$/;
function canonical(scan, tokens) {
  const out = [];
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i].text === '::' && tokens[i + 1]?.kind === 'ident' && tokens[i - 1]?.kind !== 'ident') i++;
    const t = tokens[i];
    if (t.kind === 'string') { out.push('@literal'); continue; }
    if (t.kind === 'ident' && tokens[i - 1]?.text !== '.' && !(tokens[i - 1]?.text === '::' && tokens[i - 2]?.kind === 'ident')) {
      const r = qualified(scan,tokens,i);
      if (r.ambiguous || r.shadowed || !r.path) return null;
      out.push(r.path); i = r.end - 1;
    } else out.push(t.name ?? t.text);
  }
  return out.join(' ');
}
function literalClosure(scan, tokens) {
  if (tokens[0]?.text !== '||') return false;
  let body = tokens.slice(1);
  while (body[0]?.text === '{' && body[0].mate === body.at(-1)?.index) body = body.slice(1,-1);
  const value = canonical(scan,body);
  return value === 'regex::Regex::new ( @literal ) . unwrap ( )' ||
    value === 'regex::Regex::new ( @literal ) . expect ( @literal )';
}
function readonlyRegex(scan, item, type, init) {
  const typeText = canonical(scan,type);
  if (typeText === 'std::sync::LazyLock < regex::Regex >') {
    const open = init.findIndex(t => t.text === '(');
    if (open < 0 || init[open].mate !== init.at(-1)?.index) return false;
    return canonical(scan,init.slice(0,open)) === 'std::sync::LazyLock::new' && literalClosure(scan,init.slice(open + 1,-1));
  }
  if (typeText !== 'std::sync::OnceLock < regex::Regex >' || !item ||
    canonical(scan,init) !== 'std::sync::OnceLock::new ( )') return false;
  const ts = scan.tokens;
  if (ts.filter(t => t.start >= item.start && t.index < item.first).some(t => t.name === 'pub')) return false;
  const fn = scan.items.filter(i => i.kind === 'fn' && i.bodyStart < item.start && i.end > item.end).at(-1);
  if (!fn) return false;
  const refs = ts.filter(t => t.start > fn.bodyStart && t.start < fn.end && t.name === item.name &&
    !(t.index >= item.first && t.index <= item.last));
  if (!refs.length) return false;
  for (const ref of refs) {
    const i = ref.index;
    if ((ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident') || ts[i + 1]?.text !== '.' || ts[i + 2]?.name !== 'get_or_init' || ts[i + 3]?.text !== '(') return false;
    const end = ts[i + 3].mate;
    if (!literalClosure(scan,ts.slice(i + 4,end))) return false;
  }
  return true;
}
function measure(path, scan) {
  const m = metric(), ts = scan.tokens;
  // The scanner deliberately does not expand macros. Inspect only the explicit
  // static declarations in thread_local!'s token tree, never arbitrary macros.
  const threadGroups = scan.groups.filter(g => ts[g.start - 1]?.text === '!' && ts[g.start - 2]?.name === 'thread_local');
  const declarations = scan.items.filter(i => i.kind === 'static').map(item => ({ item, first: item.first, last: item.last, thread: false }));
  for (const group of threadGroups) for (let i = group.start + 1; i < group.end; i++) if (ts[i].name === 'static') {
    let end = i + 1;
    for (; end < group.end && ts[end].text !== ';'; end++) if (['(','[','{'].includes(ts[end].text)) end = ts[end].mate;
    if (end === group.end) throw new Error(`${path}:${ts[i].line}: thread_local static not delimitable`);
    declarations.push({ item: null, first: i, last: end, thread: true }); i = end;
  }
  for (const d of declarations) {
    if (scan.isTestOffset(ts[d.first].start)) continue;
    const segment = ts.slice(d.first,d.last), colon = segment.findIndex(t => t.text === ':'), eq = segment.findIndex(t => t.text === '=');
    if (colon < 0 || eq < colon) throw new Error(`${path}:${ts[d.first].line}: static type/initializer not delimitable`);
    const type = segment.slice(colon + 1,eq), init = segment.slice(eq + 1);
    let state = segment[1]?.name === 'mut';
    for (let i = 0; i < type.length; i++) if (type[i].kind === 'ident' && !(type[i - 1]?.text === '::' && type[i - 2]?.kind === 'ident')) {
      const r = qualified(scan,type,i), value = r.path ?? (r.ambiguous ? r.raw : '');
      if (stateType.test(value) || d.thread && /(?:^|::)(?:Cell|RefCell)$/.test(value)) {
        state = true; warning(m,path,type[i].line,r);
      }
      i = r.end - 1;
    }
    if (!state) continue;
    if (segment[1]?.name !== 'mut' && readonlyRegex(scan,d.item,type,init)) continue;
    hit(m,'globals',ts[d.first].line);
  }
  return m;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
