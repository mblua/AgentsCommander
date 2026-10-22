import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit, warning, resolveAlias as resolveGateAlias, qualified as qualifiedGatePath } from './sanitizing_gate_support.mjs';

const relevantPath = new RegExp("^(?:tauri(?:::|$)|(?:(?:std|tokio)::)?fs::|(?:(?:std|tokio)::fs::)?(?:AppHandle|File|OpenOptions|read_dir)(?:::|$))");
function resolveAlias(scan, input, scope) {
  return resolveGateAlias(scan, input, scope, relevantPath);
}

// Finite syntactic signals, using the shared lexer, scope and production model.
function qualified(scan, tokens, i) {
  return qualifiedGatePath(scan, tokens, i, relevantPath);
}
function signals(scan, item) {
  return item.attributes.flatMap(a => a.signals.map(meta => ({ ...resolveAlias(scan,meta.name,a.scope), meta, line: a.line })));
}
export function runGate(snapshot) {
  return runMeasuredGate(snapshot, { gate, rules, newFileRules, inUniverse, measure, extraChecks });
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
