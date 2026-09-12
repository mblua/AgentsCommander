#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_INVENTORY = 'remote-resources/SERVED-PATHS.md';
const INVENTORY_HEADER = '| Path | Serving ref | First version that fetched it |';
const INVENTORY_SEPARATOR = '|---|---|---|';
const INVENTORY_ROW_RE = /^\| `([^`|]+)` \| `([^`|]+)` \| ([^|]*[^|\s])\s*\|$/;
const LITERAL_RE = /https?:\/\/raw\.githubusercontent\.com\/([^\/\s"'`<>()\\]+)\/([^\/\s"'`<>()\\]+)\/([^\/\s"'`<>()\\]+)\/([^\s"'`<>()\\]+)/g;
const SCAN_PREFIXES = ['src-tauri/', 'src/'];
const OWNER = 'mblua';
const REPO = 'agentscommander';
const TRAILING_PATH_PUNCTUATION = /[.,;:]+$/;

const USAGE = `Usage: node scripts/check-served-paths.mjs [--inventory <file>] [--self-test] [--help]

Checks that every path in the served-path inventory is a tracked file, and that
every raw.githubusercontent.com/mblua/AgentsCommander fetch literal under
src-tauri/ or src/ has a matching row with the same path and serving ref.

  --inventory <file>  Read the inventory from <file> instead of
                      ${DEFAULT_INVENTORY} (relative paths resolve
                      against the current directory).
  --self-test         Run the in-memory self-test (12 cases).
  --help              Print this usage and exit 0.`;

function lineOf(source, index) {
  let line = 1;
  for (let i = 0; i < index && i < source.length; i += 1) {
    if (source.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

export function checkServedPaths({ inventoryText, inventoryName, trackedFiles, readSource }) {
  const errors = [];
  const rows = [];
  const inventoryLines = String(inventoryText)
    .split('\n')
    .map((line) => (line.endsWith('\r') ? line.slice(0, -1) : line));

  inventoryLines.forEach((line, index) => {
    if (!line.startsWith('|')) return;
    if (line === INVENTORY_HEADER || line === INVENTORY_SEPARATOR) return;
    const match = INVENTORY_ROW_RE.exec(line);
    if (!match) {
      errors.push(`malformed inventory row at ${inventoryName}:${index + 1}`);
      return;
    }
    rows.push({ path: match[1], ref: match[2], line: index + 1 });
  });
  if (rows.length === 0) errors.push('inventory has no rows');

  const tracked = new Set(trackedFiles);
  const literals = [];
  for (const file of trackedFiles) {
    if (!SCAN_PREFIXES.some((prefix) => file.startsWith(prefix))) continue;
    const source = readSource(file);
    LITERAL_RE.lastIndex = 0;
    let match;
    while ((match = LITERAL_RE.exec(source)) !== null) {
      if (match[1].toLowerCase() !== OWNER || match[2].toLowerCase() !== REPO) continue;
      literals.push({
        file,
        line: lineOf(source, match.index),
        ref: match[3],
        path: match[4].replace(TRAILING_PATH_PUNCTUATION, ''),
      });
    }
  }

  for (const row of rows) {
    if (!tracked.has(row.path)) {
      errors.push(`inventory path ${row.path} (${inventoryName}:${row.line}) is not a tracked file on this tree`);
    }
  }
  for (const literal of literals) {
    if (!rows.some((row) => row.path === literal.path && row.ref === literal.ref)) {
      errors.push(`${literal.file}:${literal.line} fetches ${literal.ref}/${literal.path}, which has no row in ${inventoryName}`);
    }
  }
  if (literals.length === 0) {
    errors.push('no raw.githubusercontent.com/mblua/AgentsCommander literal found under src-tauri/ or src/; the scan is broken');
  }

  return { rows, literals, errors };
}

function loadTrackedFiles() {
  const output = execFileSync('git', ['ls-files', '-z'], { cwd: ROOT, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  return output.split('\0').filter((file) => file.length > 0);
}

function runCheck(options) {
  const inventoryPath = options.inventory === undefined
    ? path.join(ROOT, DEFAULT_INVENTORY)
    : path.resolve(options.inventory);
  const inventoryName = options.inventory === undefined ? DEFAULT_INVENTORY : inventoryPath;

  let inventoryText;
  try {
    inventoryText = fs.readFileSync(inventoryPath, 'utf8');
  } catch (error) {
    console.error(`cannot read inventory ${inventoryPath}: ${error instanceof Error ? error.message : String(error)}`);
    return 3;
  }

  let trackedFiles;
  try {
    trackedFiles = loadTrackedFiles();
  } catch (error) {
    console.error(`git ls-files failed: ${error instanceof Error ? error.message : String(error)}`);
    return 3;
  }

  let result;
  try {
    result = checkServedPaths({
      inventoryText,
      inventoryName,
      trackedFiles,
      readSource: (file) => fs.readFileSync(path.join(ROOT, file), 'utf8'),
    });
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 3;
  }

  if (result.errors.length === 0) {
    console.log(`served-paths: ${result.rows.length} rows, ${result.literals.length} literals, consistent`);
    return 0;
  }
  const prefix = process.env.GITHUB_ACTIONS === 'true' ? '::error::' : '';
  for (const error of result.errors) console.error(`${prefix}${error}`);
  return 1;
}

const SELF_TEST_INVENTORY_NAME = 'remote-resources/SERVED-PATHS.md';
const SELF_TEST_ROWS = [
  '| `docs/home-en.md` | `main` | 0.8.43 |',
  '| `remote-resources/blocking-menus/v1/settings-blocking-menus.json` | `main` | 1.0.0 |',
];
const SELF_TEST_TRACKED = [
  'docs/home-en.md',
  'remote-resources/blocking-menus/v1/settings-blocking-menus.json',
  'src/main.ts',
];
const HOME_LITERAL = 'https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/home-en.md';
const MENUS_LITERAL = 'https://raw.githubusercontent.com/mblua/AgentsCommander/main/remote-resources/blocking-menus/v1/settings-blocking-menus.json';

function selfTestInventory(rows) {
  return ['# Served paths', '', INVENTORY_HEADER, INVENTORY_SEPARATOR, ...rows, ''].join('\n');
}

const SELF_TEST_GOOD_INVENTORY = selfTestInventory(SELF_TEST_ROWS);
const SELF_TEST_GOOD_SOURCE = [
  `// ${HOME_LITERAL}`,
  `// ${MENUS_LITERAL}`,
  '// https://raw.githubusercontent.com/d3/d3-shape/master/img/x.png',
  'export {};',
  '',
].join('\n');

function runServedPathsFixture(overrides = {}) {
  const sources = overrides.sources ?? { 'src/main.ts': SELF_TEST_GOOD_SOURCE };
  return checkServedPaths({
    inventoryText: overrides.inventoryText ?? SELF_TEST_GOOD_INVENTORY,
    inventoryName: SELF_TEST_INVENTORY_NAME,
    trackedFiles: overrides.trackedFiles ?? SELF_TEST_TRACKED,
    readSource: (file) => {
      if (!(file in sources)) throw new Error(`fixture is missing source for ${file}`);
      return sources[file];
    },
  });
}

function expectNoErrors(result) {
  if (result.errors.length !== 0) {
    throw new Error(`expected no errors, got ${JSON.stringify(result.errors)}`);
  }
}

function expectError(result, substring) {
  if (!result.errors.some((error) => error.includes(substring))) {
    throw new Error(`expected an error containing ${JSON.stringify(substring)}, got ${JSON.stringify(result.errors)}`);
  }
}

function selfTestCases() {
  return [
    ['case 1: good fixture stays silent', () => {
      const result = runServedPathsFixture();
      expectNoErrors(result);
      if (result.rows.length !== 2 || result.literals.length !== 2) {
        throw new Error(`expected 2 rows and 2 literals, got ${result.rows.length} and ${result.literals.length}`);
      }
    }],
    ['case 2: a row path that is not tracked', () => {
      const result = runServedPathsFixture({
        trackedFiles: SELF_TEST_TRACKED.filter((file) => file !== 'docs/home-en.md'),
      });
      expectError(result, 'is not a tracked file');
    }],
    ['case 3: a fetched path with no row', () => {
      const result = runServedPathsFixture({
        sources: { 'src/main.ts': `// ${HOME_LITERAL}\n// https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/new.md\n` },
      });
      expectError(result, 'has no row');
    }],
    ['case 4: a fetched ref that differs from the row', () => {
      const result = runServedPathsFixture({
        sources: { 'src/main.ts': '// https://raw.githubusercontent.com/mblua/AgentsCommander/dev/docs/home-en.md\n' },
      });
      expectError(result, 'has no row');
    }],
    ['case 5: owner and repo are compared case-insensitively', () => {
      const result = runServedPathsFixture({
        sources: { 'src/main.ts': '// https://raw.githubusercontent.com/MBLUA/agentscommander/main/docs/new.md\n' },
      });
      expectError(result, 'has no row');
    }],
    ['case 6: a case-insensitive literal matches its row and is counted', () => {
      const result = runServedPathsFixture({
        inventoryText: selfTestInventory(['| `docs/home-en.md` | `main` | 0.8.43 |']),
        trackedFiles: ['docs/home-en.md', 'src/main.ts'],
        sources: { 'src/main.ts': '// https://raw.githubusercontent.com/MBLUA/agentscommander/main/docs/home-en.md\n' },
      });
      expectNoErrors(result);
      if (result.literals.length !== 1) {
        throw new Error(`expected exactly 1 literal, got ${result.literals.length}`);
      }
    }],
    ['case 7: a malformed inventory row', () => {
      const result = runServedPathsFixture({
        inventoryText: selfTestInventory([...SELF_TEST_ROWS, '| docs/home-en.md | main | 0.8.43 |']),
      });
      expectError(result, 'malformed inventory row');
    }],
    ['case 8: an inventory with no rows', () => {
      const result = runServedPathsFixture({ inventoryText: selfTestInventory([]) });
      expectError(result, 'inventory has no rows');
    }],
    ['case 9: no matching literal anywhere', () => {
      const result = runServedPathsFixture({ sources: { 'src/main.ts': 'export {};\n' } });
      expectError(result, 'the scan is broken');
    }],
    ['case 10: a trailing dot after a literal path still matches its row', () => {
      const result = runServedPathsFixture({
        sources: { 'src/main.ts': `// see ${HOME_LITERAL}.\n` },
      });
      expectNoErrors(result);
    }],
    ['case 11: a templated literal is never exempt', () => {
      const result = runServedPathsFixture({
        sources: { 'src/main.ts': '// https://raw.githubusercontent.com/mblua/AgentsCommander/{SOURCE_REF}/docs/home-en.md\n' },
      });
      expectError(result, 'has no row');
    }],
    ['case 12: CRLF inventory and source behave like case 1', () => {
      const crlf = (text) => text.replace(/\n/g, '\r\n');
      const baseline = runServedPathsFixture();
      const result = runServedPathsFixture({
        inventoryText: crlf(SELF_TEST_GOOD_INVENTORY),
        sources: { 'src/main.ts': crlf(SELF_TEST_GOOD_SOURCE) },
      });
      expectNoErrors(result);
      if (result.rows.length !== baseline.rows.length || result.literals.length !== baseline.literals.length) {
        throw new Error(`expected ${baseline.rows.length} rows and ${baseline.literals.length} literals, got ${result.rows.length} and ${result.literals.length}`);
      }
    }],
  ];
}

function selfTest() {
  const failures = [];
  for (const [name, run] of selfTestCases()) {
    try {
      run();
    } catch (error) {
      failures.push(`${name}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  if (failures.length > 0) {
    for (const failure of failures) console.error(`check-served-paths self-test failed: ${failure}`);
    return 4;
  }
  console.log(`check-served-paths self-test passed (${selfTestCases().length} cases)`);
  return 0;
}

function parseArgs(argv) {
  const options = { inventory: undefined, selfTest: false, help: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--self-test') {
      options.selfTest = true;
    } else if (arg === '--help') {
      options.help = true;
    } else if (arg === '--inventory') {
      const value = argv[i + 1];
      if (value === undefined) throw new Error('--inventory requires a file path');
      options.inventory = value;
      i += 1;
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return options;
}

function main(argv) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    console.error(USAGE);
    return 2;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }
  if (options.selfTest) return selfTest();
  return runCheck(options);
}

process.exitCode = main(process.argv.slice(2));
