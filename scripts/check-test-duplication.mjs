#!/usr/bin/env node
/**
 * #2155 - new test-code duplication gate.
 *
 * `npm run dup:changed` runs `jscpd` over the test files `.jscpd.json`
 * matches, scoped to new code: the 148 baseline clones recorded by #2155 do
 * not fail, but a PR that edits inside one of them or adds a new clone does.
 *
 * The duplicated-code decisions are jscpd's; this script only resolves inputs
 * and turns jscpd's exit code into the gate's verdict:
 *
 * - `--fail-on-new-clones` makes the exit code mean something (a bare jscpd
 *   run exits 0 even with 148 clones).
 * - `--fail-on-empty` makes a scan of zero sources red instead of a warning.
 * - `--no-gitignore` keeps an ignore entry from silently shrinking the scan.
 *
 * The scan set is an explicit file list from git (#4115): `git ls-files
 * --cached --others --exclude-standard` over the config's `pattern`, minus
 * files missing from disk. Tracked files are listed even under a gitignored
 * path, untracked new tests are listed unless ignored, and only untracked
 * *and* ignored files (local evidence copies under `target/`, `artifacts/`,
 * ...) are left out, with no denylist. jscpd still filters the list through
 * `pattern`/`ignore`, so the list cannot widen the scan.
 *
 * The base ref is always resolved through `git merge-base`, never forwarded
 * raw: a raw branch tip would report the base branch's own commits as new.
 *
 * No path is derived from `process.cwd()`; `toolRoot` locates the installed
 * jscpd runner and `.jscpd.json`, `workRoot` is the scanned git work tree and
 * the child's cwd. The self-test keeps `toolRoot` at its default and varies
 * only `workRoot`, so the runner, version probe, config bytes and argv under
 * test are the shipped ones.
 *
 * `git` is never searched through `PATH`: every call site spawns the absolute
 * path returned by `resolveGit()` — a fixed system location, or the
 * `GATE_GIT_BIN` override, which must itself be absolute. `GATE_BASE_REF` and
 * `GATE_GIT_BIN` are the only two environment controls this script reads.
 *
 * See `docs/testing/test-code-duplication.md` for the convention and the
 * recorded baseline.
 */
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { resolveGit } from './resolve-git.mjs';

export { resolveGit };

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const JSCPD_RELATIVE = ['node_modules', 'jscpd', 'run-jscpd.js'];
const CONFIG_RELATIVE = '.jscpd.json';
const PINNED_VERSION = '5.2.1';
const DEFAULT_BASE_REF = 'origin/main';
const DOC_PATH = 'docs/testing/test-code-duplication.md';
// CreateProcess limit for the whole command line. The child is spawned with
// `shell: false` straight from `process.execPath`, never through `npx`, a
// `.cmd` shim or `shell: true`, so cmd.exe's 8,191 limit does not apply.
export const MAX_COMMAND_LINE = 32_767;

export const GATE_FLAGS = Object.freeze([
  '--fail-on-new-clones',
  '--fail-on-empty',
  '--no-gitignore',
]);

function fail(message) {
  process.stderr.write(`check:test-duplication FAILED\n${message}\n`);
  return 1;
}

function resolveRunner(toolRoot) {
  return path.join(toolRoot, ...JSCPD_RELATIVE);
}

/**
 * Git pathspecs equal to the config's `pattern`. One `{a,b}` group expands to
 * one pathspec per alternative; `:(glob)` makes `**` + `/` also match zero
 * folders, as jscpd's pattern does.
 */
export function pathspecsFromPattern(pattern) {
  const group = /\{([^{}]*)\}/.exec(pattern);
  const expanded = group
    ? group[1].split(',').map((option) => pattern.replace(group[0], option))
    : [pattern];
  if (expanded.some((entry) => /[{}]/.test(entry))) {
    throw new Error(`unsupported .jscpd.json pattern '${pattern}': at most one {a,b} group.`);
  }
  return expanded.map((entry) => `:(glob)${entry}`);
}

/**
 * Files the gate scans: tracked plus untracked-not-ignored files matching the
 * config's `pattern`, minus those missing from disk (an unstaged deletion is
 * still in `--cached`, and jscpd errors on a missing path). Sorted, unique.
 */
export function listScanFiles({ git, workRoot, configPath }) {
  const { pattern } = JSON.parse(fs.readFileSync(configPath, 'utf8'));
  const args = ['ls-files', '-z', '--cached', '--others', '--exclude-standard', '--'];
  const listed = spawnSync(git, [...args, ...pathspecsFromPattern(pattern)], {
    cwd: workRoot,
    shell: false,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  if (listed.error) throw new Error(`cannot spawn git ls-files: ${listed.error.message}`);
  if (listed.status !== 0) {
    throw new Error(
      `'git ls-files' failed in ${workRoot} (git exit ${listed.status}): ` +
        `${String(listed.stderr ?? '').trim()}`,
    );
  }
  const files = String(listed.stdout ?? '').split('\0').filter(Boolean);
  return [...new Set(files)]
    .filter((file) => fs.existsSync(path.join(workRoot, file)))
    .sort(byCodeUnit);
}

/** Locale-independent order (UTF-16 code units), the same as a bare `.sort()`. */
function byCodeUnit(a, b) {
  if (a < b) return -1;
  return a > b ? 1 : 0;
}

/**
 * The console run's argv for the git file list, or `{ error }` when the list
 * cannot be read or the full command line passes `maxCommandLine`.
 */
function buildScanArgs({ git, workRoot, configPath, runner, base, flags, maxCommandLine }) {
  let files;
  try {
    files = listScanFiles({ git, workRoot, configPath });
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
  const args = [
    runner,
    '--config',
    configPath,
    '--reporters',
    'console',
    '--baseline-from-ref',
    base,
    ...flags,
    ...scanPaths(files),
  ];
  const length = commandLineLength(process.execPath, args);
  if (length > maxCommandLine) {
    return {
      error:
        `the jscpd command line for ${files.length} files is ${length} characters, over the ` +
        `${maxCommandLine} limit; refusing to truncate the scan. jscpd ${PINNED_VERSION} has no ` +
        `file-list input, so the gate needs another way to pass the list (see ${DOC_PATH}).`,
    };
  }
  console.log(
    `test-code duplication scan: ${files.length} files (tracked + untracked, not ignored)`,
  );
  return { args };
}

/**
 * jscpd's positional paths. With no file, `.git` stands in: it exists inside
 * the work tree (jscpd finds the repository for `--baseline-from-ref` from the
 * first path) and never matches `pattern`, so jscpd analyzes no files (and
 * `--fail-on-empty` turns that red) instead of scanning its whole cwd.
 */
function scanPaths(files) {
  return files.length > 0 ? files : ['.git'];
}

/** Length of the command line `spawnSync` builds on Windows, every argument quoted. */
export function commandLineLength(command, args) {
  return [command, ...args].reduce((total, arg) => total + arg.length + 3, 0);
}

/**
 * Run the gate. Returns the exit code the CLI should exit with.
 *
 * @param {object} [options]
 * @param {string} [options.toolRoot] installed repository holding the runner and config
 * @param {string} [options.workRoot] scanned git work tree; defaults to toolRoot
 * @param {string} [options.baseRef] base ref; defaults to GATE_BASE_REF or origin/main
 * @param {string} [options.configPath] absolute .jscpd.json path; defaults under toolRoot
 * @param {readonly string[]} [options.flags] jscpd gating flags; defaults to GATE_FLAGS
 * @param {Function} [options.buildScan] scan argv builder (self-test seam);
 *   defaults to buildScanArgs
 */
export async function runGate({
  toolRoot = REPO_ROOT,
  workRoot = toolRoot,
  baseRef,
  configPath = path.join(toolRoot, CONFIG_RELATIVE),
  flags = GATE_FLAGS,
  maxCommandLine = MAX_COMMAND_LINE,
  buildScan = buildScanArgs,
} = {}) {
  const runner = resolveRunner(toolRoot);
  if (!fs.existsSync(runner)) {
    return fail(
      `pinned jscpd runner not found at ${runner}; run 'npm ci' before this gate.`,
    );
  }

  const version = spawnSync(process.execPath, [runner, '--version'], {
    cwd: toolRoot,
    shell: false,
    encoding: 'utf8',
  });
  if (version.error) {
    return fail(`cannot spawn pinned jscpd at ${runner}: ${version.error.message}`);
  }
  if (version.status !== 0) {
    return fail(
      `pinned jscpd at ${runner} failed its --version probe (exit ${version.status}): ` +
        `${String(version.stderr ?? '').trim()}`,
    );
  }
  const reported = String(version.stdout ?? '').trim().replace(/^jscpd\s+/i, '');
  if (reported !== PINNED_VERSION) {
    return fail(
      `pinned jscpd version assertion failed: resolved ${reported || '(no output)'}, ` +
        `expected exactly ${PINNED_VERSION}.`,
    );
  }

  const requestedRef =
    baseRef ?? ((process.env.GATE_BASE_REF ?? '').trim() || DEFAULT_BASE_REF);
  let git;
  try {
    git = resolveGit();
  } catch (error) {
    return fail(error instanceof Error ? error.message : String(error));
  }
  const merged = spawnSync(git, ['merge-base', 'HEAD', requestedRef], {
    cwd: workRoot,
    shell: false,
    encoding: 'utf8',
  });
  if (merged.error) {
    return fail(`cannot spawn git to resolve base ref '${requestedRef}': ${merged.error.message}`);
  }
  if (merged.status !== 0) {
    return fail(
      `cannot resolve base ref '${requestedRef}' with 'git merge-base HEAD ${requestedRef}' ` +
        `in ${workRoot} (git exit ${merged.status}): ${String(merged.stderr ?? '').trim()}\n` +
        `GATE_BASE_REF sets the base ref; fetch the base branch (or use actions/checkout ` +
        `with fetch-depth: 0) so '${requestedRef}' resolves.`,
    );
  }
  const base = String(merged.stdout ?? '').trim();
  if (!/^[0-9a-f]{40}$/i.test(base)) {
    return fail(`'git merge-base HEAD ${requestedRef}' returned an unusable base: '${base}'.`);
  }
  console.log(`test-code duplication base: ${base} (from '${requestedRef}')`);

  const scan = buildScan({ git, workRoot, configPath, runner, base, flags, maxCommandLine });
  if ('error' in scan) return fail(scan.error);
  const { args } = scan;

  const child = spawnSync(process.execPath, args, {
    cwd: workRoot,
    shell: false,
    stdio: 'inherit',
  });
  if (child.error) {
    return fail(`cannot spawn pinned jscpd at ${runner}: ${child.error.message}`);
  }
  if (child.status === null) {
    return fail(
      `pinned jscpd did not exit normally (signal ${child.signal ?? 'unknown'}); treating the run as failed.`,
    );
  }
  if (child.status !== 0) {
    process.stderr.write(
      `New test-code duplication was reported by jscpd (exit ${child.status}). ` +
        `See ${DOC_PATH} for the convention and the recorded baseline.\n`,
    );
  }
  return child.status;
}

// ---------------------------------------------------------------------------
// Self-test (#2155 plan section 5). Builds real temporary git repositories
// under os.tmpdir(), runs the shipped gate against them through the
// { workRoot, baseRef } seam, and asserts the exit code plus the sources /
// clones / new-clone counts from a parallel `--reporters json` run.
// ---------------------------------------------------------------------------

// The fixture bytes are pinned by the plan; A and B never match each other at
// the configured thresholds (minLines 10 / minTokens 100).
const A = (name) => `  it('${name}', () => {
    const store = createStore({
      id: 'alpha',
      title: 'Alpha project',
      agents: ['one', 'two', 'three'],
      flags: { archived: false, pinned: true, hidden: false },
    });
    store.select('one');
    store.select('two');
    store.toggle('pinned');
    expect(store.state.id).toBe('alpha');
    expect(store.state.title).toBe('Alpha project');
    expect(store.state.agents).toHaveLength(3);
    expect(store.state.selected).toEqual(['one', 'two']);
    expect(store.state.flags.pinned).toBe(false);
    expect(store.state.flags.archived).toBe(false);
    expect(store.state.flags.hidden).toBe(false);
    expect(store.history.at(-1)).toBe('toggle:pinned');
    expect(store.history).toHaveLength(3);
  });`;

const B = (name) => `  it('${name}', () => {
    const panel = renderPanel({
      key: 'beta',
      heading: 'Beta panel',
      rows: ['red', 'green', 'blue'],
      view: { collapsed: true, focused: false, dirty: true },
    });
    panel.click('red');
    panel.click('green');
    panel.toggle('collapsed');
    expect(panel.model.key).toBe('beta');
    expect(panel.model.heading).toBe('Beta panel');
    expect(panel.model.rows).toHaveLength(3);
    expect(panel.model.clicked).toEqual(['red', 'green']);
    expect(panel.model.view.collapsed).toBe(false);
    expect(panel.model.view.focused).toBe(false);
    expect(panel.model.view.dirty).toBe(true);
    expect(panel.log.at(-1)).toBe('toggle:collapsed');
    expect(panel.log).toHaveLength(3);
  });`;

const head = (name, importSymbol, importPath, blocks) => `import { describe, expect, it } from 'vitest';
import { ${importSymbol} } from '${importPath}';

describe('${name}', () => {
${blocks.join('\n\n')}
});
`;

const rootTs = (names) => head('root', 'createStore', './store', names.map(A));
const deepTsx = (names) => head('deep', 'renderPanel', '../panel', names.map(B));
const crossTsx = (names) => head('crossnew', 'createStore', '../store', names.map(A));

const BASE_FILES = {
  'src/root.test.ts': rootTs(['a', 'a2']),
  'src/sub/deep.test.tsx': deepTsx(['b', 'b2']),
};
const EMPTY_SCAN_FILES = {
  'src/root.spec.ts': rootTs(['a', 'a2']),
  'src/sub/deep.spec.tsx': deepTsx(['b', 'b2']),
};
const BAD_REF = 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeef';
const SHIPPED_CONFIG = path.join(REPO_ROOT, CONFIG_RELATIVE);
const IDENTITY = [
  '-c',
  'user.name=check-test-duplication',
  '-c',
  'user.email=check-test-duplication@example.invalid',
];

function runGit(cwd, args) {
  const result = spawnSync(resolveGit(), [...IDENTITY, ...args], {
    cwd,
    shell: false,
    encoding: 'utf8',
  });
  if (result.error) {
    throw new Error(`git ${args.join(' ')} failed to spawn: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`git ${args.join(' ')} exited ${result.status}: ${String(result.stderr ?? '').trim()}`);
  }
  return String(result.stdout ?? '').trim();
}

function writeFiles(directory, files) {
  for (const [relative, content] of Object.entries(files)) {
    const file = path.join(directory, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, content);
  }
}

function createRepo(directory, files, { force = false } = {}) {
  runGit(directory, ['init', '-q']);
  writeFiles(directory, files);
  runGit(directory, ['add', '-A', ...(force ? ['-f'] : [])]);
  runGit(directory, ['commit', '-q', '-m', 'base']);
  return runGit(directory, ['rev-parse', 'HEAD']);
}

function commitChanges(directory, files, { force = false, allowEmpty = false } = {}) {
  writeFiles(directory, files);
  runGit(directory, ['add', '-A', ...(force ? ['-f'] : [])]);
  runGit(directory, ['commit', '-q', ...(allowEmpty ? ['--allow-empty'] : []), '-m', 'changed']);
}

function replaceLine(directory, relative, lineNumber, replacement) {
  const file = path.join(directory, relative);
  const lines = fs.readFileSync(file, 'utf8').split('\n');
  lines.splice(lineNumber - 1, 0, replacement);
  fs.writeFileSync(file, lines.join('\n'));
}

/**
 * Parallel `--reporters json` run of the same fixture and flags. Returns the
 * exit code, captured stderr and the statistics the case assertions read.
 */
function measure({ workRoot, base, flags = GATE_FLAGS, configPath = SHIPPED_CONFIG }) {
  const files = listScanFiles({ git: resolveGit(), workRoot, configPath });
  const outputDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ac-dup-report-'));
  try {
    const result = spawnSync(
      process.execPath,
      [
        resolveRunner(REPO_ROOT),
        '--config',
        configPath,
        '--reporters',
        'json',
        '--output',
        outputDir,
        '--baseline-from-ref',
        base,
        ...flags,
        ...scanPaths(files),
      ],
      { cwd: workRoot, shell: false, encoding: 'utf8' },
    );
    const reportPath = path.join(outputDir, 'jscpd-report.json');
    let statistics = null;
    if (fs.existsSync(reportPath)) {
      const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
      const duplicates = Array.isArray(report.duplicates) ? report.duplicates : [];
      statistics = {
        sources: report.statistics.total.sources,
        clones: report.statistics.total.clones,
        newClones: duplicates.filter((duplicate) => duplicate.isNew).length,
        duplicates,
      };
    }
    return {
      status: result.status,
      stderr: String(result.stderr ?? ''),
      statistics,
    };
  } finally {
    fs.rmSync(outputDir, { recursive: true, force: true });
  }
}

async function captureScriptStderr(run) {
  const original = process.stderr.write;
  let text = '';
  process.stderr.write = (chunk) => {
    text += String(chunk);
    return true;
  };
  try {
    const code = await run();
    return { code, text };
  } finally {
    process.stderr.write = original;
  }
}

export async function runSelfTest() {
  const failures = [];
  const record = (message) => failures.push(message);

  const expectEqual = (label, actual, expected) => {
    if (actual !== expected) {
      record(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
  };
  const expectCounts = (label, statistics, expected) => {
    if (!statistics) {
      record(`${label}: no jscpd JSON report was produced`);
      return;
    }
    expectEqual(`${label} sources`, statistics.sources, expected.sources);
    expectEqual(`${label} clones`, statistics.clones, expected.clones);
    expectEqual(`${label} new clones`, statistics.newClones, expected.newClones);
  };
  const expectContains = (label, text, needle) => {
    if (!text.includes(needle)) {
      record(`${label}: output does not contain ${JSON.stringify(needle)}`);
    }
  };

  const withCase = async (name, body) => {
    const before = failures.length;
    const temporary = [];
    const temp = (prefix) => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), prefix));
      temporary.push(directory);
      return directory;
    };
    try {
      await body(temp);
    } catch (error) {
      record(`${name}: threw ${error instanceof Error ? error.message : String(error)}`);
    }
    const passed = failures.length === before;
    console.log(`${passed ? 'PASS' : 'FAIL'} ${name}`);
    if (!passed) {
      for (const failure of failures.slice(before)) console.error(`  ${failure}`);
      for (const directory of temporary) console.error(`  fixture directory: ${directory}`);
    }
    for (const directory of temporary) {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  };

  /**
   * Cases 10 and 11: a `GATE_GIT_BIN` value the script must reject. The
   * fixture, the environment save/restore and the failure-banner assertion are
   * constant; the override and the messages that distinguish the cases are
   * data, written literally at the call site. The helper applies one uniform
   * assertion per message and never branches on which case it is.
   */
  const expectGitOverrideFailure = async (temp, { override, expectedMessages }) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    const previous = process.env.GATE_GIT_BIN;
    process.env.GATE_GIT_BIN = override;
    let run;
    try {
      run = await captureScriptStderr(() => runGate({ workRoot: directory, baseRef: base }));
    } finally {
      if (previous === undefined) delete process.env.GATE_GIT_BIN;
      else process.env.GATE_GIT_BIN = previous;
    }
    expectEqual('exit code', run.code, 1);
    expectContains('message', run.text, 'check:test-duplication FAILED');
    for (const message of expectedMessages) {
      expectContains('message', run.text, message);
    }
  };

  await withCase('GATE_FLAGS is the shipped literal', async () => {
    expectEqual(
      'GATE_FLAGS',
      JSON.stringify(GATE_FLAGS),
      JSON.stringify(['--fail-on-new-clones', '--fail-on-empty', '--no-gitignore']),
    );
  });

  await withCase('case 1: third copy appended to the root-level fixture', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    commitChanges(directory, { 'src/root.test.ts': rootTs(['a', 'a2', 'a3']) });
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 1);
    expectCounts('counts', measured.statistics, { sources: 2, clones: 3, newClones: 2 });
    expectContains('output', measured.stderr, 'found 2 new clones');
  });

  await withCase('case 2: clean empty second commit still passes', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    commitChanges(directory, {}, { allowEmpty: true });
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 0);
    expectCounts('counts', measured.statistics, { sources: 2, clones: 2, newClones: 0 });
  });

  await withCase('case 3: edit inside a baseline clone is red', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    replaceLine(directory, 'src/root.test.ts', 10, "    store.select('three');");
    commitChanges(directory, {});
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 1);
    expectCounts('counts', measured.statistics, { sources: 2, clones: 2, newClones: 1 });
  });

  await withCase('case 4: unrelated line motion stays green', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    commitChanges(directory, {
      'src/root.test.ts': `// unrelated header comment\n${BASE_FILES['src/root.test.ts']}`,
    });
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 0);
    expectCounts('counts', measured.statistics, { sources: 2, clones: 2, newClones: 0 });
  });

  await withCase('case 5: unresolvable base ref fails in step 2', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    createRepo(directory, BASE_FILES);
    const previous = process.env.GATE_BASE_REF;
    process.env.GATE_BASE_REF = BAD_REF;
    let run;
    try {
      run = await captureScriptStderr(() => runGate({ workRoot: directory }));
    } finally {
      if (previous === undefined) delete process.env.GATE_BASE_REF;
      else process.env.GATE_BASE_REF = previous;
    }
    expectEqual('exit code', run.code, 1);
    expectContains('message', run.text, BAD_REF);
    expectContains('message', run.text, '128');
    expectContains('message', run.text, 'GATE_BASE_REF');
  });

  await withCase('case 6: --fail-on-new-clones is the verdict', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    commitChanges(directory, { 'src/root.test.ts': rootTs(['a', 'a2', 'a3']) });
    const flags = GATE_FLAGS.filter((flag) => flag !== '--fail-on-new-clones');
    const code = await runGate({ workRoot: directory, baseRef: base, flags });
    const measured = measure({ workRoot: directory, base, flags });
    expectEqual('exit code', code, 0);
    expectCounts('counts', measured.statistics, { sources: 2, clones: 3, newClones: 2 });
  });

  await withCase('case 7: cross-format detection (contrast pair)', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    commitChanges(directory, { 'src/sub/crossnew.test.tsx': crossTsx(['x']) });

    const withCross = await runGate({ workRoot: directory, baseRef: base });
    const withCrossMeasured = measure({ workRoot: directory, base });
    expectEqual('crossFormats exit code', withCross, 1);
    expectCounts('crossFormats counts', withCrossMeasured.statistics, {
      sources: 3,
      clones: 3,
      newClones: 2,
    });
    if (withCrossMeasured.statistics) {
      const cross = withCrossMeasured.statistics.duplicates.filter((duplicate) => duplicate.isNew);
      expectEqual('cross-format new clones', cross.length, 2);
      expectEqual(
        'cross-format extensions',
        cross.every(
          (duplicate) =>
            path.extname(duplicate.firstFile.name) !== path.extname(duplicate.secondFile.name),
        ),
        true,
      );
      expectEqual(
        'cross-format tokens',
        JSON.stringify(cross.map((duplicate) => duplicate.tokens).sort((a, b) => a - b)),
        JSON.stringify([210, 213]),
      );
    }

    const modified = JSON.parse(fs.readFileSync(SHIPPED_CONFIG, 'utf8'));
    delete modified.crossFormats;
    const configPath = path.join(temp('ac-dup-config-'), '.jscpd.json');
    fs.writeFileSync(configPath, `${JSON.stringify(modified, null, 2)}\n`);
    const withoutCross = await runGate({ workRoot: directory, baseRef: base, configPath });
    const withoutCrossMeasured = measure({ workRoot: directory, base, configPath });
    expectEqual('no-crossFormats exit code', withoutCross, 0);
    expectCounts('no-crossFormats counts', withoutCrossMeasured.statistics, {
      sources: 3,
      clones: 2,
      newClones: 0,
    });
  });

  await withCase('case 8: empty scan is red (contrast pair)', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, EMPTY_SCAN_FILES);

    const withFlag = await runGate({ workRoot: directory, baseRef: base });
    const withFlagMeasured = measure({ workRoot: directory, base });
    expectEqual('--fail-on-empty exit code', withFlag, 1);
    expectCounts('--fail-on-empty counts', withFlagMeasured.statistics, {
      sources: 0,
      clones: 0,
      newClones: 0,
    });
    expectContains(
      '--fail-on-empty output',
      withFlagMeasured.stderr,
      'jscpd analyzed no files (--fail-on-empty)',
    );

    const flags = GATE_FLAGS.filter((flag) => flag !== '--fail-on-empty');
    const withoutFlag = await runGate({ workRoot: directory, baseRef: base, flags });
    const withoutFlagMeasured = measure({ workRoot: directory, base, flags });
    expectEqual('empty-scan exit code without the flag', withoutFlag, 0);
    expectCounts('empty-scan counts without the flag', withoutFlagMeasured.statistics, {
      sources: 0,
      clones: 0,
      newClones: 0,
    });
    expectContains('empty-scan output', withoutFlagMeasured.stderr, 'Warning:');
    expectContains(
      'empty-scan output',
      withoutFlagMeasured.stderr,
      'jscpd analyzed no files',
    );
  });

  await withCase('case 9: --no-gitignore holds the scan set (contrast pair)', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(
      directory,
      { ...BASE_FILES, '.gitignore': 'src/sub/\n' },
      { force: true },
    );
    commitChanges(
      directory,
      { 'src/sub/deep.test.tsx': deepTsx(['b', 'b2', 'b3']) },
      { force: true },
    );

    const withFlag = await runGate({ workRoot: directory, baseRef: base });
    const withFlagMeasured = measure({ workRoot: directory, base });
    expectEqual('--no-gitignore exit code', withFlag, 1);
    expectCounts('--no-gitignore counts', withFlagMeasured.statistics, {
      sources: 2,
      clones: 3,
      newClones: 2,
    });
    expectContains('--no-gitignore output', withFlagMeasured.stderr, 'found 2 new clones');

    // #4115: the explicit git file list already holds the tracked file, so the
    // scan set no longer shrinks without the flag; the flag stays as a second
    // guard and this half pins that the list alone keeps the file.
    const flags = GATE_FLAGS.filter((flag) => flag !== '--no-gitignore');
    const withoutFlag = await runGate({ workRoot: directory, baseRef: base, flags });
    const withoutFlagMeasured = measure({ workRoot: directory, base, flags });
    expectEqual('file-list-holds exit code', withoutFlag, 1);
    expectCounts('file-list-holds counts', withoutFlagMeasured.statistics, {
      sources: 2,
      clones: 3,
      newClones: 2,
    });
  });

  const case12 = 'case 12: untracked, ignored evidence stays out of the scan (contrast pair)';
  await withCase(case12, async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, { ...BASE_FILES, '.gitignore': 'src/evidence/\n' });
    writeFiles(directory, { 'src/evidence/9999/root.test.ts': rootTs(['a', 'a2']) });

    const ignored = await runGate({ workRoot: directory, baseRef: base });
    const ignoredMeasured = measure({ workRoot: directory, base });
    expectEqual('ignored-copy exit code', ignored, 0);
    expectCounts('ignored-copy counts', ignoredMeasured.statistics, {
      sources: 2,
      clones: 2,
      newClones: 0,
    });

    fs.rmSync(path.join(directory, '.gitignore'));
    const unignored = await runGate({ workRoot: directory, baseRef: base });
    const unignoredMeasured = measure({ workRoot: directory, base });
    expectEqual('unignored-copy exit code', unignored, 1);
    expectEqual('unignored-copy sources', unignoredMeasured.statistics?.sources, 3);
  });

  const case13 = 'case 13: work tree under a parent folder named artifacts is scanned';
  await withCase(case13, async (temp) => {
    const directory = path.join(temp('ac-dup-fixture-'), 'artifacts', 'repo');
    fs.mkdirSync(directory, { recursive: true });
    const base = createRepo(directory, BASE_FILES);
    commitChanges(directory, { 'src/root.test.ts': rootTs(['a', 'a2', 'a3']) });
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 1);
    expectCounts('counts', measured.statistics, { sources: 2, clones: 3, newClones: 2 });
  });

  await withCase('case 14: a tracked file deleted without staging is skipped', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    fs.rmSync(path.join(directory, 'src/sub/deep.test.tsx'));
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 0);
    expectCounts('counts', measured.statistics, { sources: 1, clones: 1, newClones: 0 });
  });

  const case17 = 'case 17: untracked, not-ignored target/<n>/src copy is out of pattern';
  await withCase(case17, async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    writeFiles(directory, { 'target/9999/src/root.test.ts': rootTs(['a', 'a2']) });
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 0);
    expectCounts('counts', measured.statistics, { sources: 2, clones: 2, newClones: 0 });
  });

  const case15 = 'case 15: an untracked, not-ignored new test with a clone is red';
  await withCase(case15, async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    writeFiles(directory, { 'src/copy.test.ts': rootTs(['c']) });
    const code = await runGate({ workRoot: directory, baseRef: base });
    const measured = measure({ workRoot: directory, base });
    expectEqual('exit code', code, 1);
    expectEqual('sources', measured.statistics?.sources, 3);
    expectEqual('new clones > 0', (measured.statistics?.newClones ?? 0) > 0, true);
  });

  await withCase('case 16: an over-long command line is red, never truncated', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    const run = await captureScriptStderr(() =>
      runGate({ workRoot: directory, baseRef: base, maxCommandLine: 100 }),
    );
    expectEqual('exit code', run.code, 1);
    expectContains('message', run.text, 'check:test-duplication FAILED');
    expectContains('message', run.text, 'over the 100 limit; refusing to truncate the scan');
  });

  await withCase('case 18: an empty scan error is red and spawns nothing', async (temp) => {
    const directory = temp('ac-dup-fixture-');
    const base = createRepo(directory, BASE_FILES);
    // A spawned scan would exit 7: the gate must stop on the key, not its truthiness.
    const buildScan = () => ({ error: '', args: ['-e', 'process.exit(7)'] });
    const run = await captureScriptStderr(() =>
      runGate({ workRoot: directory, baseRef: base, buildScan }),
    );
    expectEqual('exit code', run.code, 1);
    expectContains('message', run.text, 'check:test-duplication FAILED');
  });

  await withCase('pathspecs mirror the shipped pattern', async () => {
    const { pattern } = JSON.parse(fs.readFileSync(SHIPPED_CONFIG, 'utf8'));
    expectEqual(
      'pathspecs',
      JSON.stringify(pathspecsFromPattern(pattern)),
      JSON.stringify([':(glob)src/**/*.test.ts', ':(glob)src/**/*.test.tsx']),
    );
  });

  await withCase('case 10: no git at a fixed location is a hard failure', async (temp) => {
    await expectGitOverrideFailure(temp, {
      override: '/nonexistent/git',
      expectedMessages: [
        'no executable git found at any of: /nonexistent/git.',
        'Set GATE_GIT_BIN to an absolute path to git.',
      ],
    });
  });

  await withCase('case 11: a relative git override is rejected', async (temp) => {
    await expectGitOverrideFailure(temp, {
      override: 'git',
      expectedMessages: [
        "GATE_GIT_BIN must be an absolute path to a git executable; got 'git'.",
      ],
    });
  });

  if (failures.length > 0) {
    console.error(`check-test-duplication self-test: ${failures.length} assertion(s) failed`);
    return 1;
  }
  console.log('check-test-duplication self-test: all cases passed');
  return 0;
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 0) return runGate();
  if (args.length === 1 && args[0] === '--self-test') return runSelfTest();
  process.stderr.write('Usage: node scripts/check-test-duplication.mjs [--self-test]\n');
  return 1;
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exit(await main());
}
