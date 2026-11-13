import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit, warning, occurrenceIdentity,
  resolveAlias as resolveGateAlias,
    qualified as qualifiedGatePath } from './sanitizing_gate_support.mjs';

const relevantPath =
  /(?:^|::)(?:Mutex|RwLock|Atomic\w*|OnceLock|OnceCell|Lazy|LazyLock|Cell|RefCell)$/;
function resolveAlias(scan, input, scope) {
  return resolveGateAlias(scan, input, scope, relevantPath);
}

// Finite syntactic signals, using the shared lexer, scope and production model.
function qualified(scan, tokens, i) {
  return qualifiedGatePath(scan, tokens, i, relevantPath);
}
export function runGate(snapshot) {
  return runMeasuredGate(snapshot, { gate, rules, newFileRules, inUniverse, measure, extraChecks,
    recognizeMoves: true });
}

const gate = '08';
const rules = ['globals'];
const newFileRules = rules;
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);
function extraChecks(path, m, before, violation, moved) {
  const available = new Map();
  for (const site of before?.exposedGlobals ?? []) if (site.identity)
    available.set(site.identity, (available.get(site.identity) ?? 0) + 1);
  for (const site of m.exposedGlobals) {
    const count = available.get(site.identity) ?? 0;
    if (count) available.set(site.identity, count - 1);
    else if (!moved.has(site))
      violation('new_exposed_global', path, site.line, 0, 1,
        'recognized mutable global has new explicit visibility');
  }
}
function explicitVisibility(scan, item) {
  const ts = scan.tokens;
  let i = 0;
  while (i < item.first) {
    const token = ts[i++];
    if (token.start < item.start) continue;
    if (token.text === '#' && ts[i]?.text === '[') {
      i = ts[i].mate + 1;
      continue;
    }
    if (token.name === 'pub') return true;
  }
  return false;
}
function threadIdentityStart(ts, first) {
  if (ts[first - 1]?.name === 'pub') return first - 1;
  const previous = ts[first - 1];
  if (previous?.text === ')' && ts[previous.mate - 1]?.name === 'pub')
    return previous.mate - 1;
  return first;
}
const stateType = /(?:^|::)(?:Mutex|RwLock|Atomic\w*|OnceLock|OnceCell|Lazy|LazyLock)$/;
function canonical(scan, tokens) {
  const out = [];
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i].text === '::' && tokens[i + 1]?.kind === 'ident' &&
      tokens[i - 1]?.kind !== 'ident') i++;
    const t = tokens[i];
    if (t.kind === 'string') { out.push('@literal'); continue; }
    if (t.kind === 'ident' && tokens[i - 1]?.text !== '.' && !(tokens[i - 1]?.text === '::' &&
      tokens[i - 2]?.kind === 'ident')) {
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
    return canonical(scan,init.slice(0,open)) === 'std::sync::LazyLock::new' && literalClosure(scan,
      init.slice(open + 1,-1));
  }
  if (typeText !== 'std::sync::OnceLock < regex::Regex >' || !item ||
    canonical(scan,init) !== 'std::sync::OnceLock::new ( )') return false;
  const ts = scan.tokens;
  if (ts.filter(t => t.start >= item.start &&
    t.index < item.first).some(t => t.name === 'pub')) return false;
  const fn = scan.items.findLast(i => i.kind === 'fn' && i.bodyStart < item.start &&
    i.end > item.end);
  if (!fn) return false;
  const refs = ts.filter(t => t.start > fn.bodyStart && t.start < fn.end && t.name === item.name &&
    !(t.index >= item.first && t.index <= item.last));
  if (!refs.length) return false;
  for (const ref of refs) {
    const i = ref.index;
    if ((ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident') ||
      ts[i + 1]?.text !== '.' || ts[i + 2]?.name !== 'get_or_init' ||
        ts[i + 3]?.text !== '(') return false;
    const end = ts[i + 3].mate;
    if (!literalClosure(scan,ts.slice(i + 4,end))) return false;
  }
  return true;
}
function threadStaticEnd(ts, group, i, path) {
  let end = i + 1;
  for (; end < group.end && ts[end].text !== ';'; end++)
    if (['(','[','{'].includes(ts[end].text)) end = ts[end].mate;
  if (end === group.end) throw new Error(
    `${path}:${ts[i].line}: thread_local static not delimitable`);
  return end;
}
function threadLocalStatics(ts, groups, path) {
  const out = [];
  for (const group of groups) for (let i = group.start + 1; i < group.end; i++)
    if (ts[i].name === 'static') {
    const end = threadStaticEnd(ts, group, i, path);
    out.push({ item: null, first: i, last: end, thread: true }); i = end;
  }
  return out;
}
function staticTypeState(m, scan, path, type, thread) {
  let state = false;
  for (let i = 0; i < type.length; i++)
    if (type[i].kind === 'ident' && !(type[i - 1]?.text === '::' &&
      type[i - 2]?.kind === 'ident')) {
    const r = qualified(scan,type,i), value = r.path ?? (r.ambiguous ? r.raw : '');
    if (stateType.test(value) || thread && /(?:^|::)(?:Cell|RefCell)$/.test(value)) {
      state = true; warning(m,path,type[i].line,r);
    }
    i = r.end - 1;
  }
  return state;
}
function measureDeclaration(m, scan, ts, d, path) {
  if (scan.isTestOffset(ts[d.first].start)) return;
  const segment = ts.slice(d.first,d.last), colon = segment.findIndex(t => t.text === ':'),
    eq = segment.findIndex(t => t.text === '=');
  if (colon < 0 || eq < colon) throw new Error(
    `${path}:${ts[d.first].line}: static type/initializer not delimitable`);
  const type = segment.slice(colon + 1,eq), init = segment.slice(eq + 1);
  const typed = staticTypeState(m, scan, path, type, d.thread);
  const state = segment[1]?.name === 'mut' || typed;
  if (!state) return;
  if (segment[1]?.name !== 'mut' && readonlyRegex(scan,d.item,type,init)) return;
  const identityStart = d.thread ? threadIdentityStart(ts, d.first) : d.first;
  const identityTokens = d.thread ? ts.slice(identityStart, d.last) : segment;
  const site = hit(m, 'globals', ts[d.first].line,
    occurrenceIdentity(scan, ts[d.first], identityTokens));
  const exposed = d.thread ? identityStart !== d.first : explicitVisibility(scan, d.item);
  if (exposed) m.exposedGlobals.push(site);
}
function measure(path, scan) {
  const m = metric(), ts = scan.tokens;
  m.exposedGlobals = [];
  // The scanner deliberately does not expand macros. Inspect only the explicit
  // static declarations in thread_local!'s token tree, never arbitrary macros.
  const threadGroups = scan.groups.filter(g => ts[g.start - 1]?.text === '!' &&
    ts[g.start - 2]?.name === 'thread_local');
  const declarations = scan.items.filter(i => i.kind === 'static').map(item => ({ item,
    first: item.first, last: item.last, thread: false }));
  declarations.push(...threadLocalStatics(ts, threadGroups, path));
  for (const d of declarations) measureDeclaration(m, scan, ts, d, path);
  return m;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
