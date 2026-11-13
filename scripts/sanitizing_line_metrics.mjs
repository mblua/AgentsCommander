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
function fileVerdict(snapshot, gate, path, measure, violates) {
  const before = snapshot.readBlob(snapshot.baseTree.get(path));
  const after = snapshot.readBlob(snapshot.headTree.get(path));
  const base = measure(before), head = measure(after);
  const exceeded = lineLengths(after).flatMap((n, i) => n > 100 ? [i + 1] : []);
  const added = gate === '01' ? snapshot.addedLines(path) : new Set();
  const violation = violates(base, head);
  return { violation, metric: { path, base, head },
    diagnostic: { gate, rule: gate === '01' ? 'long_lines' : 'file_lines', path, base, head,
      status: violation ? 'FAIL' : 'PASS',
      ...(gate === '01' ?
        { lines: exceeded.map(line => ({ line, added: added.has(line) })) } : {}),
      reason: violation ? 'text debt increased' : 'text debt preserved or reduced',
      remedy: gate === '01' ? 'wrap new long lines to at most 100 Unicode characters' :
        'split the file or reduce it to at most 1000 lines' } };
}
// Shared traversal keeps both text gates identical about Git inputs and error precedence.
export function textGate(snapshot, gate, measure, violates, extensions = ['.rs']) {
  const diagnostics = [], metrics = [];
  let fail = false;
  try {
    for (const { path } of snapshot.changed.filter(entry =>
      extensions.some(ext => entry.path.endsWith(ext)))) {
      const { violation, metric, diagnostic } =
        fileVerdict(snapshot, gate, path, measure, violates);
      fail ||= violation;
      metrics.push(metric);
      diagnostics.push(diagnostic);
    }
    snapshot.assertStableHead();
    return { gate, status: fail ? 'FAIL' : 'PASS', exit: fail ? 1 : 0,
      requestedRef: snapshot.requestedRef, refOid: snapshot.refOid, baseOid: snapshot.baseOid,
      headOid: snapshot.headOid,
      changedRustFiles: metrics.filter(entry => entry.path.endsWith('.rs')).length,
      ...(extensions.includes('.ts') ? { changedTsFiles: metrics.filter(entry =>
        entry.path.endsWith('.ts') || entry.path.endsWith('.tsx')).length } : {}),
      metrics, diagnostics };
  } catch (e) {
    return { gate, status: 'ERROR', exit: e.exit ?? 3,
      baseOid: snapshot.baseOid, headOid: snapshot.headOid,
      diagnostics: [...diagnostics, { gate, reason: e.message, remedy:
        'repair inputs and rerun; no verdict available' }] };
  }
}
