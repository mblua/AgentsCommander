#!/usr/bin/env node
// Runs the cheap pull-request gates locally before a push: every command of the
// `test-debt` job, `cargo fmt --check` (the `rust-fmt` job) and the clippy run with
// its cognitive-complexity check (the clippy leg of rust-regression-windows,
// clippy-linux and rust-regression-macos). It runs every check, even after a
// failure, so one run lists every problem; it exits 1 if any check failed. The
// complexity check only warns, because CI runs it with continue-on-error.
//
// The test-debt commands are read from the workflow on each run, so this list
// cannot drift from CI. Clippy uses the toolchain pinned in
// cognitive-complexity.baseline.json, in its own target directory so switching
// toolchains does not rebuild your normal target/.
//
// The ratchet checks (`--base-ref`) compare the last commit with the base, as CI
// does, so commit your work before running this or they will not see it.
//
// Usage: npm run ci:local [-- --no-rust] [-- --base-ref <ref>]

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const WORKFLOW = path.join(ROOT, '.github', 'workflows', 'pr-regression-gates.yml');
const BASELINE = path.join(ROOT, 'cognitive-complexity.baseline.json');
const PLATFORM = { win32: 'windows', linux: 'linux', darwin: 'macos' }[process.platform];

const USAGE = `Usage: node scripts/ci-local.mjs [--no-rust] [--base-ref <ref>]

  --no-rust         Skip cargo fmt and clippy.
  --base-ref <ref>  The ref the ratchet checks compare against; default
                    origin/main. Run \`git fetch origin main\` first so it is current.
  --help            Print this usage and exit 0.`;

function parseArgs(argv) {
  const opts = { rust: true, baseRef: 'origin/main' };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--no-rust') opts.rust = false;
    else if (arg === '--base-ref') {
      opts.baseRef = argv[i + 1];
      i += 1;
      if (!opts.baseRef) throw new Error('--base-ref requires a value');
    } else if (arg === '--help') {
      console.log(USAGE);
      process.exit(0);
    } else throw new Error(`unknown argument ${JSON.stringify(arg)}\n${USAGE}`);
  }
  return opts;
}

// Every `npm run` / `npm test` line inside the test-debt job, with the base-ref
// expression CI computes replaced by the local base ref.
export function testDebtCommands(workflowText, baseRef) {
  const lines = workflowText.split(/\r?\n/);
  const start = lines.indexOf('  test-debt:');
  if (start < 0) throw new Error('no test-debt job in pr-regression-gates.yml');
  const commands = [];
  for (let i = start + 1; i < lines.length && !/^ {2}[a-z0-9-]+:\s*$/.test(lines[i]); i += 1) {
    const match = /^\s+(?:run:\s+)?(npm (?:run|test) .*)$/.exec(lines[i]);
    if (match) commands.push(match[1].replaceAll('"${GATE_BASE_REF:-origin/main}"', `"${baseRef}"`));
  }
  if (commands.length === 0) throw new Error('found no npm commands in the test-debt job');
  const unresolved = commands.find((c) => c.includes('${'));
  if (unresolved) throw new Error(`cannot run a CI expression locally: ${unresolved}`);
  return commands;
}

function run(command, options = {}) {
  const started = Date.now();
  console.log(`\n=== ${command}`);
  const result = spawnSync(command, { cwd: ROOT, shell: true, stdio: 'inherit', ...options });
  return { ok: result.status === 0, status: result.status ?? 1, seconds: (Date.now() - started) / 1000 };
}

function capture(command, cwd) {
  const result = spawnSync(command, { cwd, shell: true, encoding: 'utf8' });
  return result.status === 0 ? result.stdout : null;
}

function rustChecks(results) {
  const srcTauri = path.join(ROOT, 'src-tauri');
  results.push(['cargo fmt --all -- --check', run('cargo fmt --all -- --check', { cwd: srcTauri })]);

  const pin = JSON.parse(fs.readFileSync(BASELINE, 'utf8')).toolchain;
  const toolchains = capture('rustup toolchain list', ROOT) ?? '';
  const pinned = toolchains.split(/\r?\n/).some((line) => line.startsWith(`${pin}-`));
  if (!pinned) {
    console.log(`\nRust ${pin} (the CI pin) is not installed; run \`rustup toolchain install ${pin} --component clippy\`.`);
    results.push([`cargo +${pin} clippy`, { ok: false, seconds: 0 }]);
    return;
  }
  const targetDir = path.join(ROOT, 'target', `ci-local-${pin}`);
  const jsonl = path.join(os.tmpdir(), `ac-ci-local-cog-${process.pid}.jsonl`);
  const clippy = `cargo +${pin} clippy --locked --workspace --all-targets --message-format=json `
    + `--target-dir "${targetDir}" -- -D warnings --force-warn clippy::cognitive_complexity > "${jsonl}"`;
  const clippyResult = run(clippy, { cwd: srcTauri });
  // The JSON capture holds clippy's human text; the check reprints it and applies the baseline.
  const cog = `node scripts/check-cognitive-complexity.mjs --capture "${jsonl}" `
    + `--clippy-exit ${clippyResult.status} --platform ${PLATFORM} --rustc-version ${pin}`;
  const cogResult = run(cog);
  fs.rmSync(jsonl, { force: true });
  // CI runs the complexity step with continue-on-error, so a red result warns and does not fail.
  results.push(
    [`cargo +${pin} clippy`, clippyResult],
    ['complexity check (advisory, as in CI)', { ...cogResult, advisory: true }],
  );
}

function summaryLabel({ ok, advisory }) {
  if (ok) return 'ok  ';
  return advisory ? 'WARN' : 'FAIL';
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (!fs.existsSync(path.join(ROOT, 'node_modules', '.bin'))) {
    throw new Error('node_modules is missing; run `npm ci` first');
  }
  const base = capture(`git rev-parse --short ${opts.baseRef}`, ROOT);
  if (!base) throw new Error(`cannot resolve ${opts.baseRef}; run \`git fetch origin main\` or pass --base-ref`);
  console.log(`Comparing against ${opts.baseRef} (${base.trim()}).`);
  if (capture('git status --porcelain --untracked-files=no', ROOT)?.trim()) {
    console.log('Note: you have uncommitted changes. The --base-ref checks read the last commit, not them.');
  }

  const results = [];
  for (const command of testDebtCommands(fs.readFileSync(WORKFLOW, 'utf8'), opts.baseRef)) {
    results.push([command, run(command)]);
  }
  if (opts.rust) rustChecks(results);

  console.log('\n=== Summary');
  for (const [name, result] of results) {
    console.log(`${summaryLabel(result)} ${result.seconds.toFixed(0).padStart(4)}s  ${String(name)}`);
  }
  const failed = results.filter(([, r]) => !r.ok && !r.advisory).length;
  console.log(failed === 0 ? 'All checks passed.' : `${failed} check(s) failed.`);
  return failed === 0 ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main();
  } catch (err) {
    console.error(`ci-local: ${err.message}`);
    process.exitCode = 2;
  }
}
