import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { kindOf, languageOf, parseArgs, renderText, summarize } from './loc-budget-report.mjs';
import { fileMetrics } from './sanitizing_line_metrics.mjs';

const SCRIPT = fileURLToPath(new URL('./loc-budget-report.mjs', import.meta.url));

test('classifies language and kind', () => {
  assert.equal(languageOf('src-tauri/src/lib.rs'), 'rust');
  assert.equal(languageOf('src/App.tsx'), 'ts');
  assert.equal(languageOf('README.md'), null);
  assert.equal(kindOf('src-tauri/src/pty/tests/a.rs'), 'test');
  assert.equal(kindOf('src/sidebar/x.test.tsx'), 'test');
  assert.equal(kindOf('src/shared/generated/remote-activity.ts'), 'generated');
  assert.equal(kindOf('src-tauri/src/lib.rs'), 'nonTest');
});

test('counts lines with the 1000-line gate rule', () => {
  assert.equal(fileMetrics('a\r\nb\n').lines, 2);
  assert.equal(fileMetrics('a\nb').lines, 2);
});

test('summarizes thresholds, totals and top files', () => {
  const report = summarize([
    { path: 'src-tauri/src/a.rs', lines: 1000 },
    { path: 'src-tauri/src/b.rs', lines: 901 },
    { path: 'src-tauri/src/c.rs', lines: 801 },
    { path: 'src-tauri/src/d.rs', lines: 800 },
    { path: 'src-tauri/tests/e.rs', lines: 950 },
    { path: 'src/f.ts', lines: 10 },
    { path: 'docs/x.md', lines: 5000 },
  ], 2);
  const rustNon = report.totals.rust.nonTest;
  assert.deepEqual(rustNon, { files: 4, lines: 3502, over800: 3, over900: 2, atOrOver1000: 1 });
  assert.deepEqual(report.totals.rust.test,
    { files: 1, lines: 950, over800: 1, over900: 1, atOrOver1000: 0 });
  assert.equal(report.totals.ts.nonTest.lines, 10);
  assert.deepEqual(report.top.map(f => f.path), ['src-tauri/src/a.rs', 'src-tauri/tests/e.rs']);
  assert.match(renderText(report), /Top 2 files:/);
});

test('parses arguments and rejects bad ones', () => {
  assert.deepEqual(parseArgs(['--json', '--top', '5', '--root', '/x']),
    { json: true, top: 5, root: '/x' });
  assert.throws(() => parseArgs(['--top', 'x']), /whole number/);
  assert.throws(() => parseArgs(['--nope']), /unknown argument/);
});

test('CLI prints JSON for this repository and exits 2 on bad usage', () => {
  const ok = spawnSync(process.execPath, [SCRIPT, '--json', '--top', '3'], { encoding: 'utf8' });
  assert.equal(ok.status, 0, ok.stderr);
  const report = JSON.parse(ok.stdout);
  assert.equal(report.top.length, 3);
  assert.ok(report.totals.rust.nonTest.files > 0);
  const bad = spawnSync(process.execPath, [SCRIPT, '--bogus'], { encoding: 'utf8' });
  assert.equal(bad.status, 2);
});
