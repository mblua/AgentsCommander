// Pure functions ported from project-shared/Guidelines/_sanitizing/line-metrics.mjs:10–24.
// Count Unicode code points, exclude CRLF terminators and one final empty segment.
export function lineLengths(text) {
  const lines = text.split(/\r?\n/);
  if (lines.at(-1) === '') lines.pop();
  return lines.map(line => [...line].length);
}
export function fileMetrics(text) {
  const lengths = lineLengths(text);
  const nonBlank = lengths.filter(n => n > 0);
  const avg = values => values.length ? values.reduce((sum, n) => sum + n, 0) / values.length : 0;
  return { lines: lengths.length, max: lengths.length ? Math.max(...lengths) : 0,
    mean: avg(lengths), meanNonBlank: avg(nonBlank) };
}
// Shared traversal keeps both text gates identical about Git inputs and error precedence.
export function textGate(snapshot, gate, measure, violates) {
  const diagnostics = [], metrics = [];
  let fail = false;
  try {
    for (const { path } of snapshot.changed.filter(entry => entry.path.endsWith('.rs'))) {
      const before = snapshot.readBlob(snapshot.baseTree.get(path));
      const after = snapshot.readBlob(snapshot.headTree.get(path));
      const base = measure(before), head = measure(after);
      const exceeded = lineLengths(after).flatMap((n, i) => n > 100 ? [i + 1] : []);
      const added = gate === '01' ? snapshot.addedLines(path) : new Set();
      const violation = violates(base, head);
      fail ||= violation;
      metrics.push({ path, base, head });
      diagnostics.push({ gate, path, base, head, status: violation ? 'FAIL' : 'PASS',
        ...(gate === '01' ? { lines: exceeded.map(line => ({ line, added: added.has(line) })) } : {}),
        reason: violation ? 'text debt increased' : 'text debt preserved or reduced',
        remedy: gate === '01' ? 'wrap new long lines to at most 100 Unicode characters' : 'split the file or reduce it to at most 1000 lines' });
    }
    snapshot.assertStableHead();
    return { gate, status: fail ? 'FAIL' : 'PASS', exit: fail ? 1 : 0,
      requestedRef: snapshot.requestedRef, refOid: snapshot.refOid, baseOid: snapshot.baseOid,
      headOid: snapshot.headOid, changedRustFiles: metrics.length, metrics, diagnostics };
  } catch (e) {
    return { gate, status: 'ERROR', exit: e.exit ?? 3, baseOid: snapshot.baseOid, headOid: snapshot.headOid,
      diagnostics: [...diagnostics, { gate, reason: e.message, remedy: 'repair inputs and rerun; no verdict available' }] };
  }
}
