#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SKIP_DIRS = new Set(['target', 'node_modules', 'dist', '.git']);
const PLATFORMS = ['windows', 'linux', 'macos'];
const CLIPPY_TOML_NAMES = new Set(['clippy.toml', '.clippy.toml']);
const CARGO_CONFIG_RE = /(^|\/)\.cargo\/config(\.toml)?$/;
const S1_RE = /cognitive_complexity|cognitive-complexity/g;
const S2_RE = /not\(clippy\)|cfg_attr\(clippy/g;
const S4_RUSTFLAGS_RE = /^rustflags\s*=/;
const S4_ENV_RE = /^\[env\](\s*#.*)?$/;

const USAGE = `Usage: node scripts/check-cognitive-complexity.mjs --self-test
       node scripts/check-cognitive-complexity.mjs --scan-sources
       node scripts/check-cognitive-complexity.mjs --platform <platform>

Runs the in-memory self-test of the cognitive-complexity detector, or scans the
repository for Clippy cognitive-complexity suppression routes (rules S1 to S4).

  --self-test         Run the in-memory self-test (9 cases); touches no file.
  --scan-sources      Walk the repository (skipping target/, node_modules/,
                      dist/ and .git/) and fail on any S1-S4 hit.
  --platform <p>      Declare the platform: windows, linux or macos.
  --help              Print this usage and exit 0.`;

export class UsageError extends Error {}

function lineOf(source, index) {
  let line = 1;
  for (let i = 0; i < index && i < source.length; i += 1) {
    if (source.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

function collectHits(source, regex, rule, file, hits) {
  for (const match of source.matchAll(regex)) {
    hits.push({ rule, file, line: lineOf(source, match.index), text: match[0] });
  }
}

export function scanSources({ files, readSource }) {
  const hits = [];
  for (const file of files) {
    const slash = file.lastIndexOf('/');
    const name = slash === -1 ? file : file.slice(slash + 1);
    if (file.endsWith('.rs')) {
      const source = readSource(file);
      collectHits(source, S1_RE, 'S1', file, hits);
      collectHits(source, S2_RE, 'S2', file, hits);
    } else if (CLIPPY_TOML_NAMES.has(name)) {
      if (slash !== -1) hits.push({ rule: 'S3', file, line: null, text: file });
    } else if (CARGO_CONFIG_RE.test(file)) {
      const lines = readSource(file).split('\n');
      lines.forEach((raw, index) => {
        const line = raw.endsWith('\r') ? raw.slice(0, -1) : raw;
        const trimmed = line.trim();
        if (trimmed === '' || trimmed.startsWith('#')) return;
        if (S4_RUSTFLAGS_RE.test(trimmed) || S4_ENV_RE.test(trimmed)) {
          hits.push({ rule: 'S4', file, line: index + 1, text: trimmed });
        }
      });
    }
  }
  return hits;
}

function formatHit(hit) {
  const location = hit.line === null ? hit.file : `${hit.file}:${hit.line}`;
  return `${hit.rule} ${location}: ${hit.text}`;
}

function listSourceFiles() {
  const files = [];
  const walk = (relativeDir) => {
    const entries = fs.readdirSync(path.join(ROOT, relativeDir), { withFileTypes: true });
    for (const entry of entries) {
      const relative = relativeDir === '' ? entry.name : `${relativeDir}/${entry.name}`;
      if (entry.isDirectory()) {
        if (!SKIP_DIRS.has(entry.name)) walk(relative);
      } else if (entry.isFile()) {
        files.push(relative);
      }
    }
  };
  walk('');
  return files.sort();
}

function runScan({ files, readSource, isGithubActions = false, stdout = console.log, stderr = console.error }) {
  const hits = scanSources({ files, readSource });
  const prefix = isGithubActions ? '::error::' : '';
  if (hits.length === 0) {
    stdout(`cognitive-complexity scan: ${files.length} files, no suppression found`);
    return 0;
  }
  for (const hit of hits) stderr(`${prefix}${formatHit(hit)}`);
  stderr(`cognitive-complexity scan: ${hits.length} suppression${hits.length === 1 ? '' : 's'} found`);
  return 1;
}

function parseArgs(argv) {
  const flags = { help: false, selfTest: false, scanSources: false, platform: undefined };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--self-test') {
      flags.selfTest = true;
    } else if (arg === '--scan-sources') {
      flags.scanSources = true;
    } else if (arg === '--help') {
      flags.help = true;
    } else if (arg === '--platform') {
      const value = argv[i + 1];
      if (value === undefined) throw new UsageError('--platform requires a value: windows, linux or macos');
      if (!PLATFORMS.includes(value)) {
        throw new UsageError(`--platform must be one of windows, linux or macos, got ${JSON.stringify(value)}`);
      }
      flags.platform = value;
      i += 1;
    } else {
      throw new UsageError(`unknown argument: ${arg}`);
    }
  }
  if (flags.help) return { mode: 'help', platform: flags.platform };
  const modes = [];
  if (flags.selfTest) modes.push('self-test');
  if (flags.scanSources) modes.push('scan-sources');
  if (modes.length !== 1) {
    throw new UsageError('exactly one mode is required: --self-test or --scan-sources');
  }
  return { mode: modes[0], platform: flags.platform };
}

function main(argv, io = {}) {
  const stdout = io.stdout ?? ((text) => console.log(text));
  const stderr = io.stderr ?? ((text) => console.error(text));
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    if (error instanceof UsageError) {
      stderr(error.message);
      stderr(USAGE);
      return 2;
    }
    throw error;
  }
  if (options.mode === 'help') {
    stdout(USAGE);
    return 0;
  }
  if (options.mode === 'self-test') return selfTest();
  return runScan({
    files: listSourceFiles(),
    readSource: (file) => fs.readFileSync(path.join(ROOT, file), 'utf8'),
    isGithubActions: process.env.GITHUB_ACTIONS === 'true',
    stdout,
    stderr,
  });
}

function runFixture(files, contents) {
  return scanSources({
    files,
    readSource: (file) => {
      if (!(file in contents)) throw new Error(`fixture is missing source for ${file}`);
      return contents[file];
    },
  });
}

function expectRule(hits, rule, file, line) {
  const found = hits.some((hit) => hit.rule === rule && hit.file === file && hit.line === line);
  if (!found) {
    throw new Error(`expected ${rule} at ${file}${line === null ? '' : `:${line}`}, got ${JSON.stringify(hits)}`);
  }
}

function expectNoHits(hits) {
  if (hits.length !== 0) {
    throw new Error(`expected no hits, got ${JSON.stringify(hits)}`);
  }
}

// The eight real target_os = "macos" forms of
// crates/session-bridge/src/bin/agentscommander-api-helper.rs, copied verbatim.
const MACOS_FORMS = [
  '#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]',
  '    #[cfg(any(target_os = "linux", target_os = "macos"))]',
  '    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]',
  '    #[cfg(any(target_os = "linux", target_os = "macos"))]',
  '    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]',
  '#[cfg(any(target_os = "linux", target_os = "macos"))]',
  '        if cfg!(target_os = "macos") {',
  '#[cfg(all(test, unix, not(any(target_os = "linux", target_os = "macos"))))]',
].join('\n');

function selfTestCases() {
  return [
    ['case 46: the per-function cognitive_complexity attribute fails S1', () => {
      const hits = runFixture(['src/legacy.rs'], {
        'src/legacy.rs': '#[clippy::cognitive_complexity = "1000"]\nfn legacy() {}\n',
      });
      expectRule(hits, 'S1', 'src/legacy.rs', 1);
    }],
    ['case 47: the cognitive-complexity spelling fails S1', () => {
      const hits = runFixture(['src/legacy.rs'], {
        'src/legacy.rs': '// clippy: cognitive-complexity = 1000\n',
      });
      expectRule(hits, 'S1', 'src/legacy.rs', 1);
    }],
    ['case 48: cfg(not(clippy)) and cfg_attr(clippy fail S2', () => {
      const first = runFixture(['src/a.rs'], { 'src/a.rs': '#[cfg(not(clippy))]\nfn a() {}\n' });
      expectRule(first, 'S2', 'src/a.rs', 1);
      const second = runFixture(['src/b.rs'], { 'src/b.rs': '#[cfg_attr(clippy, allow(dead_code))]\n' });
      expectRule(second, 'S2', 'src/b.rs', 1);
    }],
    ['case 49: nested clippy.toml fails S3, the root one passes', () => {
      const srcTauri = runFixture(['src-tauri/clippy.toml'], {});
      expectRule(srcTauri, 'S3', 'src-tauri/clippy.toml', null);
      const nested = runFixture(['crates/x/.clippy.toml'], {});
      expectRule(nested, 'S3', 'crates/x/.clippy.toml', null);
      const root = runFixture(['clippy.toml'], {});
      expectNoHits(root);
    }],
    ['case 50: rustflags in every .cargo/config spelling and the [env] table fail S4', () => {
      const toml = runFixture(['.cargo/config.toml'], {
        '.cargo/config.toml': '[build]\nrustflags = ["-A", "clippy::all"]\n',
      });
      expectRule(toml, 'S4', '.cargo/config.toml', 2);
      const legacy = runFixture(['.cargo/config'], {
        '.cargo/config': 'rustflags = "--cap-lints allow"\n',
      });
      expectRule(legacy, 'S4', '.cargo/config', 1);
      const nested = runFixture(['crates/x/.cargo/config'], {
        'crates/x/.cargo/config': 'rustflags = []\n',
      });
      expectRule(nested, 'S4', 'crates/x/.cargo/config', 1);
      const env = runFixture(['.cargo/config.toml'], {
        '.cargo/config.toml': '[env]\nCLIPPY_CONF_DIR = "docs"\n',
      });
      expectRule(env, 'S4', '.cargo/config.toml', 1);
    }],
    ['case 51: a config.toml outside .cargo/ is not S4', () => {
      const hits = runFixture(['tools/config.toml'], {
        'tools/config.toml': 'rustflags = "-A"\n[env]\n',
      });
      expectNoHits(hits);
    }],
    ['case 52: the eight real target_os = "macos" forms fire no rule', () => {
      const hits = runFixture(['crates/session-bridge/src/bin/agentscommander-api-helper.rs'], {
        'crates/session-bridge/src/bin/agentscommander-api-helper.rs': `${MACOS_FORMS}\n`,
      });
      expectNoHits(hits);
    }],
    ['case 53: a tree that violates no rule passes', () => {
      const hits = runFixture(
        ['src/a.rs', 'clippy.toml', '.cargo/config.toml'],
        { 'src/a.rs': 'fn a() {}\n', '.cargo/config.toml': '[build]\njobs = 12\n' },
      );
      expectNoHits(hits);
    }],
    ['case 54: usage errors exit 2 and --help exits 0', () => {
      const silent = { stdout: () => {}, stderr: () => {} };
      const assertCode = (argv, expected) => {
        const code = main(argv, silent);
        if (code !== expected) {
          throw new Error(`expected exit ${expected} for ${JSON.stringify(argv)}, got ${code}`);
        }
      };
      assertCode(['--unknown'], 2);
      assertCode(['--platform'], 2);
      assertCode(['--platform', 'bogus', '--scan-sources'], 2);
      assertCode(['--help'], 0);
    }],
  ];
}

function selfTest() {
  const cases = selfTestCases();
  const failures = [];
  for (const [name, run] of cases) {
    try {
      run();
    } catch (error) {
      failures.push(`${name}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  if (failures.length > 0) {
    for (const failure of failures) console.error(`check-cognitive-complexity self-test failed: ${failure}`);
    return 1;
  }
  console.log(`check-cognitive-complexity self-test passed (${cases.length} cases)`);
  return 0;
}

process.exitCode = main(process.argv.slice(2));
