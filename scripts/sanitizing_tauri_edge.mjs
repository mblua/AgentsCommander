import { codeOrder, gateCli, isMain } from './sanitizing_baseline.mjs';
import { scanRust } from './sanitizing_rust_scan.mjs';

const relevantPath = new RegExp("^(?:tauri(?:::|$)|(?:(?:std|tokio)::)?fs::|(?:(?:std|tokio)::fs::)?(?:AppHandle|File|OpenOptions|read_dir)(?:::|$))");
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
function signals(scan, item) {
  return item.attributes.flatMap(a => a.signals.map(meta => ({ ...resolveAlias(scan,meta.name,a.scope), meta, line: a.line })));
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

const gate = '06';
const rules = ['tauri_files', 'emit_files', 'state_files', 'command_files', 'disk_commands'];
const newFileRules = ['tauri_files'];
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);
function extraChecks(path, m, before, violation) {
  for (const [id, line] of m.commands) if (!before?.commandIds.has(id))
    violation('new_disk_command', path, line, 0, 1, `new command with disk access: ${id}`);
}
function measure(path, scan) {
  const m = metric(), ts = scan.productionTokens;
  const edge = /^src-tauri\/src\/(?:(?:lib|main|commands|events)\.rs$|(?:commands|events)\/)/.test(path);
  let tauri, emit, state, command;
  for (let i = 0; i < ts.length; i++) {
    const t = ts[i];
    if (t.text === '.' && ['emit','emit_to'].includes(ts[i + 1]?.name) && ts[i + 2]?.text === '(') emit ??= t.line;
    if (t.text === '.' && ts[i + 1]?.name === 'state' && ts[i + 2]?.text === '::' && ts[i + 3]?.text === '<') state ??= t.line;
    if (t.kind !== 'ident' || (ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident')) continue;
    const r = qualified(scan,ts,i), value = r.path ?? (r.ambiguous ? r.raw : '');
    if (value.startsWith('tauri::') || value === 'tauri' || /(?:^|::)AppHandle$/.test(value)) {
      tauri ??= t.line; warning(m,path,t.line,r);
    }
    i = r.end - 1;
  }
  const ordinals = new Map(); m.commandIds = new Set();
  for (const item of scan.items) {
    if (item.kind !== 'fn' || item.testOnly) continue;
    const containers = scan.items.filter(c => c !== item && c.start <= item.start && c.end >= item.end)
      .map(c => `${c.kind}:${c.name ?? scan.tokens.slice(c.first,c.last).filter(t => t.start < c.bodyStart).map(t => t.text).join('')}`);
    const key = [...containers,item.name].join('/'), ordinal = ordinals.get(key) ?? 0;
    ordinals.set(key,ordinal + 1);
    const attrs = signals(scan,item).filter(a => a.path === 'tauri::command' || a.ambiguous && /(?:^|::)command$/.test(a.path ?? a.meta.name));
    if (!attrs.length) continue;
    command ??= item.line;
    for (const a of attrs) warning(m,path,a.line,a);
    const identity = `${path}:${key}#${ordinal}`; m.commandIds.add(identity);
    let disk = false;
    for (let i = 0; i < ts.length; i++) {
      const t = ts[i];
      if (item.bodyStart === null || t.start <= item.bodyStart || t.start >= item.end || t.kind !== 'ident' || (ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident')) continue;
      const r = qualified(scan,ts,i), value = r.path ?? (r.ambiguous ? r.raw : '');
      if (/^(?:(?:std|tokio)::)?fs::/.test(value) || /^(?:(?:std|tokio)::fs::)?File::(?:open|create)$/.test(value) ||
        /^(?:(?:std|tokio)::fs::)?OpenOptions(?:::|$)/.test(value) || /^(?:(?:std|tokio)::fs::)?read_dir$/.test(value)) {
        disk = true; warning(m,path,t.line,r);
      }
      i = r.end - 1;
    }
    if (disk) { const id = identity; m.commands.set(id,item.line); hit(m,'disk_commands',item.line,id); }
  }
  if (!edge) for (const [rule,line] of [['tauri_files',tauri],['emit_files',emit],['state_files',state],['command_files',command]]) if (line) hit(m,rule,line);
  return m;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
