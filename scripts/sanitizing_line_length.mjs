import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { lineLengths, textGate } from './sanitizing_line_metrics.mjs';
export function runGate(snapshot) {
  return textGate(snapshot, '01', text => lineLengths(text).filter(n => n > 100).length,
    (base, head) => head > base);
}
if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
