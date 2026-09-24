// Shared by the #2447 gates (check-module-arcs.mjs, check-module-cycles.mjs): the real detector
// runner and the self-test loop.

import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export function realRunner(argv) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, argv, { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => { stdout += d; });
    child.stderr.on('data', (d) => { stderr += d; });
    child.on('error', reject);
    child.on('close', (code) => resolve({ exit: code, stdout, stderr }));
  });
}

export async function runSelfTest(cases) {
  let failed = 0;
  for (const [name, run] of cases) {
    try {
      await run();
      console.log(`ok   ${name}`);
    } catch (err) {
      failed += 1;
      console.log(`FAIL ${name}: ${err.message}`);
    }
  }
  console.log(`${cases.length - failed}/${cases.length} self-test cases passed`);
  return failed === 0 ? 0 : 4;
}
