import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { fileMetrics, textGate } from './sanitizing_line_metrics.mjs';
export function runGate(snapshot) {
  return textGate(snapshot, '02', text => fileMetrics(text).lines,
    (base, head) => head > 1000 && head > base);
}
if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
