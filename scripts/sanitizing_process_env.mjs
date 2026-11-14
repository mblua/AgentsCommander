import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit, warning, occurrenceIdentity,
  resolveAlias as resolveGateAlias,
  qualified as qualifiedGatePath } from './sanitizing_gate_support.mjs';

const COMMAND_NEW = /(?:std|tokio)::process::Command::new|Command::new/.source;
const ENV_CALL = /(?:std::)?env::(?:var|var_os|set_var|remove_var)/.source;
const relevantPath = new RegExp(`^(?:${COMMAND_NEW}|${ENV_CALL})$`);
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

const gate = '09';
const rules = ['launch_files', 'env_read_files', 'env_write'];
const newFileRules = ['launch_files', 'env_read_files'];
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);
function extraChecks(path, m, before, violation) {
  if (m.counts.env_write) violation('env_write', path, m.sites.env_write[0].line,
    before?.counts.env_write ?? 0, m.counts.env_write,
      'global environment mutation outside main.rs is forbidden, including existing debt');
}
function measure(path, scan) {
  const m = metric(), ts = scan.productionTokens;
  const owner = {
    launch: /^src-tauri\/src\/(?:process\.rs$|process\/)/.test(path),
    read: /^src-tauri\/src\/(?:config\.rs$|config\/|main\.rs$|lib\.rs$)/.test(path),
    write: path === 'src-tauri/src/main.rs',
  };
  const record = (rule, t) => hit(m,rule,t.line,occurrenceIdentity(scan,t));
  for (let i = 0; i < ts.length; i++) {
    if (ts[i].kind !== 'ident' || pathTail(ts, i)) continue;
    const r = qualified(scan, ts, i), value = r.path ?? (r.ambiguous ? r.raw : '');
    if (ts[r.end]?.text !== '(') continue;
    classifyCall({ m, path, t: ts[i], r, value, owner, record });
    i = r.end - 1;
  }
  return m;
}
// Ident after `.` or after `ident::`: not the start of a path.
function pathTail(ts, i) {
  return ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident';
}
function classifyCall({ m, path, t, r, value, owner, record }) {
  if (/^(?:(?:std|tokio)::process::)?Command::new$/.test(value)) { if (!owner.launch) { record(
    'launch_files',t); } warning(m,path,t.line,r); }
  if (/^(?:(?:std::)?env::)?(?:var|var_os)$/.test(value) && (value.includes('env::') ||
    r.binding?.kind === 'import')) {
    if (!owner.read) { record('env_read_files',t); } warning(m,path,t.line,r);
  }
  if (/^(?:(?:std::)?env::)?(?:set_var|remove_var)$/.test(value) && (value.includes('env::') ||
    r.binding?.kind === 'import')) {
    if (!owner.write) { hit(m,'env_write',t.line); } warning(m,path,t.line,r);
  }
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
