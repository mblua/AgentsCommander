// Script line-length debt ratchet (#4130): a changed script under scripts/ may keep or
// reduce its count of lines over 100 Unicode characters, never grow it. Counts are per
// file, so debt removed in one script never compensates debt added to another.
import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { lineLengths } from './sanitizing_line_metrics.mjs';

const GATE = 'script_line_length';
const RULE = 'long_lines';
const LIMIT = 100;
const SUFFIXES = ['.mjs', '.cjs', '.js'];

export function isScriptPath(path) {
  return path.startsWith('scripts/') && SUFFIXES.some(suffix => path.endsWith(suffix));
}
export function longLineNumbers(text) {
  return lineLengths(text).flatMap((n, i) => n > LIMIT ? [i + 1] : []);
}
function measure(snapshot, path) {
  const before = longLineNumbers(snapshot.readBlob(snapshot.baseTree.get(path)));
  const after = longLineNumbers(snapshot.readBlob(snapshot.headTree.get(path)));
  const base = before.length, head = after.length, status = head > base ? 'FAIL' : 'PASS';
  return {
    metric: { path, base, head },
    diagnostic: { gate: GATE, rule: RULE, path, base, head, status, lines: after,
      reason: status === 'FAIL' ? 'script line-length debt increased'
        : 'script line-length debt preserved or reduced',
      remedy: `wrap new long lines to at most ${LIMIT} Unicode characters` },
  };
}
export function runGate(snapshot) {
  const metrics = [], diagnostics = [];
  try {
    for (const { path } of snapshot.changed.filter(entry => isScriptPath(entry.path))) {
      const { metric, diagnostic } = measure(snapshot, path);
      metrics.push(metric);
      diagnostics.push(diagnostic);
    }
    snapshot.assertStableHead();
    const fail = diagnostics.some(d => d.status === 'FAIL');
    return { gate: GATE, status: fail ? 'FAIL' : 'PASS', exit: fail ? 1 : 0,
      requestedRef: snapshot.requestedRef, refOid: snapshot.refOid, baseOid: snapshot.baseOid,
      headOid: snapshot.headOid, changedScriptFiles: metrics.length, metrics, diagnostics };
  } catch (e) {
    return { gate: GATE, status: 'ERROR', exit: e.exit ?? 3,
      requestedRef: snapshot.requestedRef, refOid: snapshot.refOid, baseOid: snapshot.baseOid,
      headOid: snapshot.headOid,
      diagnostics: [...diagnostics,
        { gate: GATE, reason: e.message, remedy: 'repair inputs and rerun; no verdict available' }] };
  }
}
if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
