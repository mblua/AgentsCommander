// Finite lexical calls; receiver methods are an explicitly separate approximation.
import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit, warning, qualified } from './sanitizing_gate_support.mjs';

const diskPath = /^(?:(?:std|tokio)::fs::|fs::|(?:File::(?:open|create)|OpenOptions::new)$)/;
const bareOperation = /^(?:read|read_to_string|read_dir|write|metadata|symlink_metadata|canonicalize|copy|rename|create_dir|create_dir_all|remove_file|remove_dir|remove_dir_all|hard_link|soft_link|read_link|set_permissions|try_exists)$/;
const receiverMethod = /^(?:exists|try_exists|metadata|symlink_metadata|canonicalize|is_file|is_dir|read_dir)$/;
const owner = p => /^src-tauri\/src\/(?:filesystem|path_identity|path_utils)(?:\.rs$|\/)/.test(p);
const inUniverse = p => /^src-tauri\/src\/.*\.rs$/.test(p);

function measure(path, scan) {
  const m = metric(), ts = scan.productionTokens;
  if (owner(path)) return m;
  for (let i = 0; i < ts.length; i++) {
    const t = ts[i];
    if (t.kind !== 'ident') continue;
    if (ts[i - 1]?.text === '.') {
      if (receiverMethod.test(t.name) && ts[i + 1]?.text === '(')
        hit(m, 'receiver_method_approximation', t.line);
      continue;
    }
    if (ts[i - 1]?.text === '::' && ts[i - 2]?.kind === 'ident') continue;
    const r = qualified(scan, ts, i, diskPath);
    const value = r.path ?? '';
    const ambiguousOperation = r.ambiguous && bareOperation.test(value);
    if (ts[r.end]?.text !== '(' || !(diskPath.test(value) || ambiguousOperation)) continue;
    // Imports/qualified paths identify std/tokio; bare File is the established
    // finite signal unless the resolver proves a local homonymous binding.
    warning(m, path, t.line, r);
    if (!m.counts.disk_files) hit(m, 'disk_files', t.line);
    i = r.end - 1;
  }
  return m;
}

export function runGate(snapshot) {
  const result = runMeasuredGate(snapshot, {
    gate: '07', rules: ['disk_files'], newFileRules: ['disk_files'],
    inUniverse, measure, extraChecks() {},
  });
  if (result.measurements) {
    result.approximation = Object.fromEntries(['base', 'head'].map(side => {
      const files = result.measurements[side].filter(m => m.counts.receiver_method_approximation);
      return [side, { files: files.length, sites: files.reduce((n, m) =>
        n + m.counts.receiver_method_approximation, 0), paths: files.map(m => m.path) }];
    }));
    result.diagnostics.push({ gate: '07', status: 'WARN', rule: 'receiver_method_approximation',
      path: '<untyped receivers>', line: 1,
      reason: 'Receiver methods are lexical approximations, may be homonyms, and do not enter disk_files totals.',
      ...result.approximation.head });
  }
  return result;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
