#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { resolveGit } from './resolve-git.mjs';

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
String-only Rust concat! values are reconstructed (reported at the macro line).
Other concat! expressions emit warnings and retain the original literal scan.

  --inventory <file>  Read the inventory from <file> instead of
                      ${DEFAULT_INVENTORY} (relative paths resolve
                      against the current directory).
  --self-test         Run the in-memory self-test (including concat! cases).
  --help              Print this usage and exit 0.`;

function lineOf(source, index) {
  let line = 1;
  for (let i = 0; i < index && i < source.length; i += 1) {
    if (source.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

// Read Rust string tokens and balanced macro bodies without evaluating Rust.
// Unsupported concat! expressions keep the original URL scan and emit a warning.
function rustToken(source, index) {
  const rest = source.slice(index);
  const comment = /^(?:\/\/[^\n]*(?:\n|$)|\/\*[\s\S]*?\*\/|\s+)/.exec(rest);
  if (comment) return { end: index + comment[0].length, trivia: true };
  const raw = /^r(#{0,255})"/.exec(rest);
  if (raw) {
    const start = index + raw[0].length;
    const close = source.indexOf('"' + raw[1], start);
    if (close !== -1) return { end: close + 1 + raw[1].length, value: source.slice(start, close) };
  }
  const quoted = /^"(?:[^"\\]|\\[\s\S])*"/.exec(rest);
  if (!quoted) return { end: index + 1 };
  let valid = true;
  const value = quoted[0].slice(1, -1).replace(
    /\\(?:\r?\n\s*|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|[\s\S])/g,
    (escape) => {
      if (/^\\\r?\n/.test(escape)) return '';
      const simple = { '\\n': '\n', '\\r': '\r', '\\t': '\t', '\\0': '\0', '\\\\': '\\', '\\"': '"', "\\'": "'" };
      if (escape in simple) return simple[escape];
      if (escape.startsWith('\\x')) return String.fromCharCode(parseInt(escape.slice(2), 16));
      if (escape.startsWith('\\u{')) {
        const code = parseInt(escape.slice(3, -1).replaceAll('_', ''), 16);
        if (code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff)) return String.fromCodePoint(code);
      }
      valid = false;
      return escape;
    },
  );
  return { end: index + quoted[0].length, ...(valid ? { value } : {}) };
}

function concatSources(source, file, warnings) {
  const expressions = [];
  const start = /\bconcat\s*!\s*([([{])/g;
  let match;
  while ((match = start.exec(source)) !== null) {
    const stack = [({ '(': ')', '[': ']', '{': '}' })[match[1]]];
    let index = start.lastIndex;
    let value = '';
    let expectString = true;
    let valid = true;
    const strings = [];
    while (index < source.length && stack.length > 0) {
      const token = rustToken(source, index);
      const char = source[index];
      if (token.trivia) { index = token.end; continue; }
      if (token.value !== undefined) {
        strings.push({ start: index, end: token.end });
        if (!expectString || stack.length !== 1) valid = false;
        value += token.value;
        expectString = false;
      } else if (char === stack.at(-1)) {
        stack.pop();
      } else if ('([{'.includes(char)) {
        stack.push(({ '(': ')', '[': ']', '{': '}' })[char]);
        valid = false;
      } else if (char === ',' && !expectString && stack.length === 1) {
        expectString = true;
      } else {
        valid = false;
      }
      index = token.end;
    }
    if (stack.length !== 0) valid = false;
    if (valid) {
      // All reconstructed URLs report the line of concat!, including multiline macros.
      expressions.push({ start: match.index, end: index, value, strings });
    } else {
      warnings.push(
        file + ':' + lineOf(source, match.index)
        + ' concat! is outside reconstructed URL scanning: only string literals are supported;'
        + ' original literal scanning remains active',
      );
    }
    start.lastIndex = index;
  }
  return expressions;
}

function scanLiterals(trackedFiles, readSource, warnings) {
  const literals = [];
  for (const file of trackedFiles) {
    if (!SCAN_PREFIXES.some((prefix) => file.startsWith(prefix))) continue;
    const original = readSource(file);
    const expressions = concatSources(original, file, warnings);
    let source = original;
    for (const token of expressions.flatMap((expression) => expression.strings).reverse()) {
      // Keep comments visible to the original scan, including URLs in macro comments.
      source = source.slice(0, token.start)
        + source.slice(token.start, token.end).replace(/[^\n]/g, ' ')
        + source.slice(token.end);
    }
    const inputs = [
      { value: source, line: null },
      ...expressions.map((expression) => ({
        value: expression.value, line: lineOf(original, expression.start),
      })),
    ];
    for (const input of inputs) {
      LITERAL_RE.lastIndex = 0;
      let match;
      while ((match = LITERAL_RE.exec(input.value)) !== null) {
        if (match[1].toLowerCase() !== OWNER || match[2].toLowerCase() !== REPO) continue;
        literals.push({
          file,
          line: input.line ?? lineOf(source, match.index),
          ref: match[3],
          path: match[4].replace(TRAILING_PATH_PUNCTUATION, ''),
        });
      }
    }
  }
  return literals;
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
  const warnings = [];
  const literals = scanLiterals(trackedFiles, readSource, warnings);

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

  return { rows, literals, errors, warnings };
}

function loadTrackedFiles() {
  const output = execFileSync(resolveGit(), ['ls-files', '-z'], { cwd: ROOT, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
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

  for (const warning of result.warnings) console.warn(`served-paths warning: ${warning}`);
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
    ['case 13: multiline concat matches its row once at the macro line', () => {
      const result = concatFixture('main', 'docs/home-en.md');
      expectNoErrors(result);
      if (result.literals.length !== 1 || result.literals[0].line !== 2) {
        throw new Error('expected one reconstructed URL at concat! line 2');
      }
    }],
    ['case 14: concat path without a row fails', () => {
      expectError(concatFixture('main', 'docs/new.md'), 'has no row');
    }],
    ['case 15: concat ref must match its row', () => {
      expectError(concatFixture('dev', 'docs/home-en.md'), 'has no row');
    }],
    ['case 16: nonliteral concat warns and retains the original scan', () => {
      const result = runServedPathsFixture({ sources: {
        'src/main.ts': `concat!("${HOME_LITERAL}", suffix!("x", "y"));`,
      } });
      expectNoErrors(result);
      if (result.warnings.length !== 1 || result.literals.length !== 1) {
        throw new Error('expected an explicit unsupported-concat warning and the original URL');
      }
    }],
    ['case 17: raw strings, comments and three arguments are reconstructed', () => {
      const result = runServedPathsFixture({ sources: {
        'src/main.ts': 'concat![r#"https://raw.githubusercontent.com/"#, /* gap */ "mblua/AgentsCommander/", "main/docs/home-en.md",];',
      } });
      expectNoErrors(result);
      if (result.literals.length !== 1) throw new Error('expected a reconstructed raw-string URL');
    }],
    ['case 18: escaped strings are decoded before checking the path', () => {
      const result = runServedPathsFixture({ sources: {
        'src/main.ts': String.raw`concat!("https://raw.githubusercontent.com/mblua/AgentsCommander/", "main/docs/\x68ome-en.md");`,
      } });
      expectNoErrors(result);
    }],
    ['case 19: unsupported concat cannot hide a missing inventory row', () => {
      const result = runServedPathsFixture({ sources: {
        'src/main.ts': 'concat!("https://raw.githubusercontent.com/mblua/AgentsCommander/dev/docs/new.md", SUFFIX);',
      } });
      expectError(result, 'has no row');
      if (result.warnings.length !== 1) throw new Error('expected an unsupported-concat warning');
    }],
    ['case 20: concat comments retain the original URL checks', () => {
      const result = runServedPathsFixture({ sources: {
        'src/main.ts': `concat!("${HOME_LITERAL}", /* https://raw.githubusercontent.com/mblua/AgentsCommander/dev/docs/new.md */ "");`,
      } });
      expectError(result, 'fetches dev/docs/new.md');
    }],
  ];
}

function concatFixture(ref, resource) {
  return runServedPathsFixture({ sources: {
    'src/main.ts': `\nconcat!(\n"https://raw.githubusercontent.com/mblua/AgentsCommander/",\n"${ref}/",\n"${resource}",\n);`,
  } });
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
