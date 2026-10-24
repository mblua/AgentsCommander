import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit, warning, occurrenceIdentity, resolveAlias as resolveGateAlias, qualified as qualifiedGatePath } from './sanitizing_gate_support.mjs';

const relevantPath = new RegExp("^(?:(?:std|tokio)::process::Command::new|Command::new|(?:std::)?env::(?:var|var_os|set_var|remove_var))$");
function resolveAlias(scan, input, scope) {
  return resolveGateAlias(scan, input, scope, relevantPath);
}

// Finite syntactic signals, using the shared lexer, scope and production model.
function qualified(scan, tokens, i) {
  return qualifiedGatePath(scan, tokens, i, relevantPath);
}
export function runGate(snapshot) {
  return runMeasuredGate(snapshot, { gate, rules, newFileRules, inUniverse, measure, extraChecks, recognizeMoves: true });
}

const gate = '09';
const rules = ['launch_files', 'env_read_files', 'env_write'];
const newFileRules = ['launch_files', 'env_read_files'];
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);
function extraChecks(path, m, before, violation) {
  if (m.counts.env_write) violation('env_write', path, m.sites.env_write[0].line,
    before?.counts.env_write ?? 0, m.counts.env_write, 'global environment mutation outside main.rs is forbidden, including existing debt');
}
function measure(path, scan) {
  const m = metric(), ts = scan.productionTokens;
  const launchOwner = /^src-tauri\/src\/(?:process\.rs$|process\/|pty\/)/.test(path);
  const readOwner = /^src-tauri\/src\/(?:config\.rs$|config\/|main\.rs$|lib\.rs$)/.test(path);
  const writeOwner = path === 'src-tauri/src/main.rs';
  const record = (rule, t) => hit(m,rule,t.line,occurrenceIdentity(scan,t));
  for (let i = 0; i < ts.length; i++) {
    if (ts[i].kind !== 'ident' || (ts[i - 1]?.text === '.' || ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident')) continue;
    const r = qualified(scan, ts, i), value = r.path ?? (r.ambiguous ? r.raw : '');
    if (ts[r.end]?.text !== '(') continue;
    if (/^(?:(?:std|tokio)::process::)?Command::new$/.test(value)) { if (!launchOwner) record('launch_files',ts[i]); warning(m,path,ts[i].line,r); }
    if (/^(?:(?:std::)?env::)?(?:var|var_os)$/.test(value) && (value.includes('env::') || r.binding?.kind === 'import')) {
      if (!readOwner) record('env_read_files',ts[i]); warning(m,path,ts[i].line,r);
    }
    if (/^(?:(?:std::)?env::)?(?:set_var|remove_var)$/.test(value) && (value.includes('env::') || r.binding?.kind === 'import')) {
      if (!writeOwner) hit(m,'env_write',ts[i].line); warning(m,path,ts[i].line,r);
    }
    i = r.end - 1;
  }
  return m;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
