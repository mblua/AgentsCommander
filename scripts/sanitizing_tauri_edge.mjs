import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { createReceiverClassifier, PROVEN_PORT, UNKNOWN } from './sanitizing_tauri_receivers.mjs';
import { runMeasuredGate, metric, hit, warning, occurrenceIdentity,
  resolveAlias as resolveGateAlias,
  qualified as qualifiedGatePath } from './sanitizing_gate_support.mjs';

const TAURI_PATH = /tauri(?:::|$)/.source;
const FS_PATH = /(?:(?:std|tokio)::)?fs::/.source;
const FS_ITEM = /(?:(?:std|tokio)::fs::)?(?:AppHandle|File|OpenOptions|read_dir)(?:::|$)/.source;
const relevantPath = new RegExp(`^(?:${TAURI_PATH}|${FS_PATH}|${FS_ITEM})`);
function resolveAlias(scan, input, scope) {
  return resolveGateAlias(scan, input, scope, relevantPath);
}

// Finite syntactic signals, using the shared lexer, scope and production model.
function qualified(scan, tokens, i) {
  return qualifiedGatePath(scan, tokens, i, relevantPath);
}
function signals(scan, item) {
  return item.attributes.flatMap(a => a.signals.map(meta =>
    ({ ...resolveAlias(scan,meta.name,a.scope), meta, line: a.line })));
}
export function runGate(snapshot) {
  // One receiver classifier per side, keyed by that side's scan lookup; never cross-snapshot.
  const classifiers = new WeakMap();
  const receivers = context => {
    if (!context?.scan) return null;
    if (!classifiers.has(context.scan))
      classifiers.set(context.scan, createReceiverClassifier(context));
    return classifiers.get(context.scan);
  };
  const measureFile = (path, scan, context) => {
    const m = measure(path, scan, receivers(context));
    // Production debt stays comparable. New core files must also be Tauri-free
    // in test-only code, using the same alias-aware analysis as production.
    if (!snapshot.baseTree.has(path)) {
      const testTokens = scan.tokens.filter(t => scan.isTestOffset(t.start));
      const testScan = { ...scan, productionTokens: testTokens };
      m.testTauri = measure(path, testScan, receivers(context)).sites.tauri_files;
    }
    return m;
  };
  return runMeasuredGate(snapshot, {
    gate, rules, newFileRules, inUniverse, measure: measureFile, extraChecks, recognizeMoves: true,
  });
}

const gate = '06';
const rules = ['tauri_files', 'emit_files', 'state_files', 'command_files', 'disk_commands'];
const newFileRules = ['tauri_files'];
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);
function extraChecks(path, m, before, violation, moved) {
  if (!before && m.testTauri?.length)
    violation('tauri_files', path, m.testTauri[0].line, 0, m.testTauri.length,
      'new core file must be Tauri-free, including test-only code');
  for (const [id, site] of m.commands) if (!before?.commandIds.has(id) && !moved.has(site))
    violation('new_disk_command', path, site.line, 0, 1, `new command with disk access: ${id}`);
}
function measure(path, scan, receivers) {
  const m = metric(), ts = scan.productionTokens;
  const edge = /^src-tauri\/src\/(?:(?:lib|main|commands|events)\.rs$|(?:commands|events)\/)/.test(
    path);
  const imports = scan.items.filter(item => item.kind === 'use'), imported = [], references = [];
  const record = (rule, t) => { if (!edge) hit(m,rule,t.line,occurrenceIdentity(scan,t)); };
  const ctx = { m, path, scan, ts, edge, receivers, imports, imported, references, record };
  for (let i = 0; i < ts.length; i++) {
    recordCoreUse(ctx, i);
    i = recordTauriPath(ctx, i);
  }
  for (const {t,value} of imported) if (!references.some(
    ref => ref === value || ref.startsWith(value + '::')))
    record('tauri_files',t);
  recordCommands(ctx);
  return m;
}

function recordCoreUse({ path, scan, ts, edge, receivers, m, record }, i) {
  const t = ts[i];
  const emit = ['emit','emit_to'].includes(ts[i + 1]?.name) && ts[i + 2]?.text === '(';
  if (t.text === '.' && emit && !edge) {
    // Only a proven port emit is not core Tauri use; REAL and UNKNOWN stay counted.
    const proof = receivers?.classify(path, scan, t) ??
      { kind: UNKNOWN, reason: 'missing receiver context' };
    if (proof.kind === UNKNOWN)
      warning(m, path, t.line, { ambiguous: true, reason: proof.reason });
    if (proof.kind !== PROVEN_PORT) record('emit_files', t);
  }
  if ((t.text === '.' && ['state','try_state'].includes(ts[i + 1]?.name) &&
    ts[i + 2]?.text === '::' && ts[i + 3]?.text === '<') ||
    (t.text === '.' && ['state','try_state'].includes(ts[i + 1]?.name) &&
    ts[i + 2]?.text === '(' && ts[i + 2].mate === ts[i + 3]?.index &&
    receivers?.proveState(path, scan, t).proved === true)) record('state_files',t);
}

function recordTauriPath({ path, scan, ts, imports, imported, references, m, record }, i) {
  const t = ts[i];
  if (t.kind !== 'ident' || (ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' &&
    ts[i - 2]?.kind === 'ident')) return i;
  const r = qualified(scan,ts,i), value = r.path ?? (r.ambiguous ? r.raw : '');
  if (value.startsWith('tauri::') || value === 'tauri' || /(?:^|::)AppHandle$/.test(value)) {
    // Imports establish names; repeated imports needed by a split are not
    // additional uses. An unused import still contributes its own signal.
    if (imports.some(item => item.start <= t.start && t.start < item.end))
      imported.push({t,value});
    else { references.push(value); record('tauri_files',t); }
    warning(m,path,t.line,r);
  }
  return r.end - 1;
}

function recordCommands({ path, scan, ts, m, record }) {
  const ordinals = new Map(); m.commandIds = new Set();
  for (const item of scan.items) {
    if (item.kind !== 'fn' || item.testOnly) continue;
    const containers = scan.items.filter(c => c !== item && c.start <= item.start &&
      c.end >= item.end)
      .map(c => `${c.kind}:${c.name ?? scan.tokens.slice(c.first,c.last).filter(t => t.start < c.bodyStart).map(t => t.text).join('')}`);
    const key = [...containers,item.name].join('/'), ordinal = ordinals.get(key) ?? 0;
    ordinals.set(key,ordinal + 1);
    const attrs = signals(scan,item).filter(a => a.path === 'tauri::command' || a.ambiguous &&
      /(?:^|::)command$/.test(a.path ?? a.meta.name));
    if (!attrs.length) continue;
    record('command_files',scan.tokens[item.first]);
    for (const a of attrs) warning(m,path,a.line,a);
    const identity = `${path}:${key}#${ordinal}`; m.commandIds.add(identity);
    const disk = touchesDisk(path, scan, ts, m, item);
    if (disk) m.commands.set(identity,
      hit(m,'disk_commands',item.line,occurrenceIdentity(scan,scan.tokens[item.first])));
  }
}

function touchesDisk(path, scan, ts, m, item) {
  let disk = false;
  for (let i = 0; i < ts.length; i++) {
    const t = ts[i];
    if (item.bodyStart === null || t.start <= item.bodyStart || t.start >= item.end ||
      t.kind !== 'ident' || (ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' &&
        ts[i - 2]?.kind === 'ident')) continue;
    const r = qualified(scan,ts,i), value = r.path ?? (r.ambiguous ? r.raw : '');
    if (/^(?:(?:std|tokio)::)?fs::/.test(value) ||
      /^(?:(?:std|tokio)::fs::)?File::(?:open|create)$/.test(value) ||
      /^(?:(?:std|tokio)::fs::)?OpenOptions(?:::|$)/.test(value) ||
        /^(?:(?:std|tokio)::fs::)?read_dir$/.test(value)) {
      disk = true; warning(m,path,t.line,r);
    }
    i = r.end - 1;
  }
  return disk;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
