#!/usr/bin/env node
/**
 * #2253 (phase 1 of #2234) - cognitive-complexity gate skeleton and scan.
 *
 * `--scan-sources` is pure text over files and needs no Rust toolchain:
 *
 * - S1 fails on `cognitive_complexity` / `cognitive-complexity` in any `*.rs`,
 *   closing the per-function threshold attribute and every allow/expect
 *   spelling that names the lint.
 * - S2 lexes Rust attributes and fails on a `clippy` cfg identifier token in a
 *   `cfg` / `cfg_attr` attribute, including nested predicates, raw identifiers,
 *   comments and trailing commas. Strings and comments are opaque. It fails
 *   closed on an unterminated attribute/comment or an unsupported token inside
 *   a cfg predicate.
 * - S3 fails on any `clippy.toml` / `.clippy.toml` except a root `clippy.toml`
 *   whose only active line is the pinned `cognitive-complexity-threshold = 25`
 *   assignment (blank lines and whole-line `#` comments allowed).
 * - S4 is a byte allowlist for Cargo configs: the sole root
 *   `.cargo/config.toml` must hash to the recorded digest, and every other
 *   `.cargo/config(\.toml)?` path fails.
 *
 * The walker skips `target/`, `node_modules/`, `dist/` and `.git/`, and fails
 * on a symlink below its included tree so a linked Rust source cannot vanish
 * from the scan.
 *
 * The self-test runs entirely in memory through the same pure functions the
 * scan uses; every file, byte and environment read is injectable.
 *
 * See docs/quality/cognitive-complexity-gate.md (added in phase 8).
 */

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SKIP_DIRS = new Set(['target', 'node_modules', 'dist', '.git']);
const PLATFORMS = ['windows', 'linux', 'macos'];
const ROOT_CARGO_CONFIG = '.cargo/config.toml';
const ROOT_CARGO_CONFIG_SHA256 = '1a6beaf1efa85bf82baeda067d35dbae04ceab94de10e1416ce3e70956055fb5';
const PINNED_THRESHOLD_LINE = 'cognitive-complexity-threshold = 25';
const CARGO_CONFIG_RE = /(^|\/)\.cargo\/config(\.toml)?$/;
const S1_RE = /cognitive_complexity|cognitive-complexity/g;

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

class RustSourceError extends Error {
  constructor(message, offset) {
    super(message);
    this.offset = offset;
  }
}

function lineOf(source, index) {
  let line = 1;
  const limit = Math.min(index, source.length);
  for (let i = 0; i < limit; i += 1) {
    if (source.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

function isIdentStart(ch) {
  return ch !== undefined && ((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || ch === '_');
}

function isIdentContinue(ch) {
  return isIdentStart(ch) || (ch !== undefined && ch >= '0' && ch <= '9');
}

/**
 * A Rust tokenizer, just enough to walk attribute argument lists.
 *
 * Tokens are `{ type, value, start, end }`; `type` is one of `ident`,
 * `punct`, `string`, `number` or `other`. Comments and whitespace are skipped
 * (nested block comments included). Strings, raw strings, byte strings and
 * char literals are opaque. A raw identifier `r#name` is reported as the
 * identifier `name`. An unterminated comment or literal is a hard error.
 */
class RustTokenizer {
  constructor(source) {
    this.source = source;
    this.offset = 0;
  }

  error(message, offset = this.offset) {
    throw new RustSourceError(message, offset);
  }

  skipTrivia() {
    const source = this.source;
    for (;;) {
      const ch = source[this.offset];
      if (ch === undefined) return;
      if (ch === ' ' || ch === '\t' || ch === '\n' || ch === '\r') {
        this.offset += 1;
        continue;
      }
      if (ch === '/' && source[this.offset + 1] === '/') {
        this.offset += 2;
        while (this.offset < source.length && source[this.offset] !== '\n') this.offset += 1;
        continue;
      }
      if (ch === '/' && source[this.offset + 1] === '*') {
        this.skipBlockComment();
        continue;
      }
      return;
    }
  }

  skipBlockComment() {
    const source = this.source;
    const start = this.offset;
    let depth = 0;
    while (this.offset < source.length) {
      if (source[this.offset] === '/' && source[this.offset + 1] === '*') {
        depth += 1;
        this.offset += 2;
        continue;
      }
      if (source[this.offset] === '*' && source[this.offset + 1] === '/') {
        depth -= 1;
        this.offset += 2;
        if (depth === 0) return;
        continue;
      }
      this.offset += 1;
    }
    this.error('unterminated block comment', start);
  }

  readNormalString(quoteStart) {
    const source = this.source;
    let j = quoteStart + 1;
    while (j < source.length) {
      if (source[j] === '\\') {
        j += 2;
        continue;
      }
      if (source[j] === '"') {
        const end = j + 1;
        this.offset = end;
        return { type: 'string', value: source.slice(quoteStart, end), start: quoteStart, end };
      }
      j += 1;
    }
    this.error('unterminated string literal', quoteStart);
  }

  tryReadPrefixedString(start) {
    const source = this.source;
    let j = start;
    if (source[j] === 'b' || source[j] === 'c') {
      if (source[j + 1] === '"') return this.readNormalString(j + 1);
      if (source[j + 1] === 'r') j += 1;
      else return null;
    }
    if (source[j] !== 'r') return null;
    j += 1;
    let hashes = 0;
    while (source[j] === '#' && hashes < 255) {
      hashes += 1;
      j += 1;
    }
    if (source[j] !== '"') return null;
    const terminator = `"${'#'.repeat(hashes)}`;
    const close = source.indexOf(terminator, j + 1);
    if (close === -1) this.error('unterminated raw string literal', start);
    const end = close + terminator.length;
    this.offset = end;
    return { type: 'string', value: source.slice(start, end), start, end };
  }

  readCharOrLifetime(start) {
    const source = this.source;
    let j = start + 1;
    if (source[j] === '\\') {
      j += 1;
      if (source[j] === 'u' && source[j + 1] === '{') {
        j += 2;
        while (j < source.length && source[j] !== '}') j += 1;
        if (source[j] === '}') j += 1;
      } else if (j < source.length) {
        j += 1;
      }
      if (source[j] === "'") {
        this.offset = j + 1;
        return { type: 'string', value: source.slice(start, j + 1), start, end: j + 1 };
      }
    } else if (source[j] !== "'" && source[j + 1] === "'") {
      this.offset = start + 3;
      return { type: 'string', value: source.slice(start, start + 3), start, end: start + 3 };
    }
    if (isIdentStart(source[j])) {
      let k = j;
      while (isIdentContinue(source[k])) k += 1;
      this.offset = k;
      return { type: 'ident', value: source.slice(j, k), start, end: k };
    }
    this.offset = start + 1;
    return { type: 'other', value: source[start], start, end: this.offset };
  }

  readIdentOrRawIdent(start) {
    const source = this.source;
    if (source[start] === 'r' && source[start + 1] === '#' && isIdentStart(source[start + 2])) {
      let j = start + 2;
      while (isIdentContinue(source[j])) j += 1;
      this.offset = j;
      return { type: 'ident', value: source.slice(start + 2, j), start, end: j };
    }
    let j = start;
    while (isIdentContinue(source[j])) j += 1;
    this.offset = j;
    return { type: 'ident', value: source.slice(start, j), start, end: j };
  }

  readNumber(start) {
    const source = this.source;
    let j = start;
    while (j < source.length && /[0-9A-Za-z_.]/.test(source[j])) j += 1;
    this.offset = j;
    return { type: 'number', value: source.slice(start, j), start, end: j };
  }

  nextToken() {
    this.skipTrivia();
    const source = this.source;
    const start = this.offset;
    const ch = source[start];
    if (ch === undefined) return null;
    if (ch === '"') return this.readNormalString(start);
    if (ch === "'") return this.readCharOrLifetime(start);
    if (ch === 'b' || ch === 'c' || ch === 'r') {
      const stringToken = this.tryReadPrefixedString(start);
      if (stringToken !== null) return stringToken;
    }
    if (isIdentStart(ch)) return this.readIdentOrRawIdent(start);
    if (ch >= '0' && ch <= '9') return this.readNumber(start);
    this.offset += 1;
    return { type: 'punct', value: ch, start, end: this.offset };
  }

  tokenize() {
    const tokens = [];
    for (;;) {
      const token = this.nextToken();
      if (token === null) return tokens;
      tokens.push(token);
    }
  }
}

const OPEN_BRACKETS = new Map([['(', ')'], ['[', ']'], ['{', '}']]);
const CLOSE_BRACKETS = new Map([[')', '('], [']', '['], ['}', '{']]);
const CFG_ATTRIBUTES = new Set(['cfg', 'cfg_attr']);

function attributeArguments(tokens) {
  if (
    tokens.length >= 2
    && tokens[0].type === 'punct'
    && tokens[0].value === '('
    && tokens[tokens.length - 1].type === 'punct'
    && tokens[tokens.length - 1].value === ')'
  ) {
    return tokens.slice(1, -1);
  }
  return tokens;
}

function splitTopLevelCommas(tokens) {
  const chunks = [];
  let current = [];
  let depth = 0;
  for (const token of tokens) {
    if (token.type === 'punct' && OPEN_BRACKETS.has(token.value)) depth += 1;
    if (token.type === 'punct' && CLOSE_BRACKETS.has(token.value)) depth -= 1;
    if (depth === 0 && token.type === 'punct' && token.value === ',') {
      chunks.push(current);
      current = [];
      continue;
    }
    current.push(token);
  }
  if (current.length > 0) chunks.push(current);
  return chunks;
}

function isPredicateTerminator(token) {
  return token === undefined
    || (token.type === 'punct' && (token.value === ')' || token.value === ',' || token.value === '='));
}

function scanPredicateTokens(tokens, source, file, hits) {
  tokens.forEach((token, index) => {
    if (token.type === 'other') {
      hits.push({
        rule: 'S2',
        file,
        line: lineOf(source, token.start),
        text: `unsupported token ${JSON.stringify(token.value)} in a cfg predicate`,
      });
      return;
    }
    if (token.type === 'ident' && token.value === 'clippy' && isPredicateTerminator(tokens[index + 1])) {
      hits.push({ rule: 'S2', file, line: lineOf(source, token.start), text: source.slice(token.start, token.end) });
    }
  });
}

function analyzeAttributeTokens(tokens, source, file, hits) {
  if (tokens.length === 0 || tokens[0].type !== 'ident') return;
  const name = tokens[0].value;
  if (!CFG_ATTRIBUTES.has(name)) return;
  const args = attributeArguments(tokens.slice(1));
  if (name === 'cfg') {
    scanPredicateTokens(args, source, file, hits);
    return;
  }
  splitTopLevelCommas(args).forEach((chunk, index) => {
    if (chunk.length === 0) return;
    const first = chunk[0];
    if (first.type === 'ident' && CFG_ATTRIBUTES.has(first.value)) {
      analyzeAttributeTokens(chunk, source, file, hits);
      return;
    }
    if (index === 0) scanPredicateTokens(chunk, source, file, hits);
  });
}

function scanAttributes(tokens, source, file, hits) {
  let i = 0;
  while (i < tokens.length) {
    const token = tokens[i];
    if (token.type === 'punct' && token.value === '#') {
      const inner = tokens[i + 1]?.type === 'punct' && tokens[i + 1].value === '!';
      const bracket = tokens[i + (inner ? 2 : 1)];
      if (bracket?.type === 'punct' && bracket.value === '[') {
        i = parseAttribute(tokens, i + (inner ? 2 : 1), source, file, hits);
        continue;
      }
    }
    i += 1;
  }
}

function parseAttribute(tokens, openIndex, source, file, hits) {
  const stack = ['['];
  const body = [];
  let i = openIndex + 1;
  for (; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token.type === 'punct' && OPEN_BRACKETS.has(token.value)) {
      stack.push(token.value);
      body.push(token);
      continue;
    }
    if (token.type === 'punct' && CLOSE_BRACKETS.has(token.value)) {
      if (stack[stack.length - 1] !== CLOSE_BRACKETS.get(token.value)) {
        throw new RustSourceError(`unbalanced ${JSON.stringify(token.value)} in attribute`, token.start);
      }
      stack.pop();
      if (stack.length === 0) {
        i += 1;
        break;
      }
      body.push(token);
      continue;
    }
    body.push(token);
  }
  if (stack.length !== 0) {
    throw new RustSourceError('unterminated attribute', tokens[openIndex].start);
  }
  analyzeAttributeTokens(body, source, file, hits);
  return i;
}

function scanRustSuppressions(source, file) {
  const hits = [];
  try {
    const tokens = new RustTokenizer(source).tokenize();
    scanAttributes(tokens, source, file, hits);
  } catch (error) {
    if (error instanceof RustSourceError) {
      hits.push({ rule: 'S2', file, line: lineOf(source, error.offset), text: error.message });
      return hits;
    }
    throw error;
  }
  return hits;
}

function sha256(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

function validateRootClippyToml(text) {
  if (!text.endsWith('\n')) {
    return { line: null, message: 'root clippy.toml must be LF terminated' };
  }
  const lines = text.slice(0, -1).split('\n');
  let assignments = 0;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim() === '') continue;
    if (line.trimStart().startsWith('#')) continue;
    if (line === PINNED_THRESHOLD_LINE) {
      assignments += 1;
      continue;
    }
    return {
      line: index + 1,
      message: `root clippy.toml line ${index + 1} is not blank, a whole-line comment, or the pinned assignment: ${JSON.stringify(line)}`,
    };
  }
  if (assignments !== 1) {
    return {
      line: null,
      message: `root clippy.toml must carry exactly one '${PINNED_THRESHOLD_LINE}' assignment; found ${assignments}`,
    };
  }
  return null;
}

function scanClippyToml(file, readBytes, hits) {
  if (file.includes('/')) {
    hits.push({ rule: 'S3', file, line: null, text: file });
    return;
  }
  if (file === '.clippy.toml') {
    hits.push({ rule: 'S3', file, line: null, text: file });
    return;
  }
  let text;
  try {
    text = new TextDecoder('utf-8', { fatal: true }).decode(readBytes(file));
  } catch {
    hits.push({ rule: 'S3', file, line: null, text: 'root clippy.toml is not valid UTF-8' });
    return;
  }
  const problem = validateRootClippyToml(text);
  if (problem !== null) {
    hits.push({ rule: 'S3', file, line: problem.line, text: problem.message });
  }
}

function scanCargoConfig(file, readBytes, hits) {
  if (!CARGO_CONFIG_RE.test(file)) return;
  if (file !== ROOT_CARGO_CONFIG) {
    hits.push({ rule: 'S4', file, line: null, text: file });
    return;
  }
  const digest = sha256(readBytes(file));
  if (digest !== ROOT_CARGO_CONFIG_SHA256) {
    hits.push({
      rule: 'S4',
      file,
      line: null,
      text: `root .cargo/config.toml must hash to ${ROOT_CARGO_CONFIG_SHA256}; got ${digest}`,
    });
  }
}

export function scanSources({ files, readSource, readBytes }) {
  const hits = [];
  for (const file of files) {
    const slash = file.lastIndexOf('/');
    const name = slash === -1 ? file : file.slice(slash + 1);
    if (name.endsWith('.rs')) {
      const source = readSource(file);
      for (const match of source.matchAll(S1_RE)) {
        hits.push({ rule: 'S1', file, line: lineOf(source, match.index), text: match[0] });
      }
      if (source.includes('clippy')) hits.push(...scanRustSuppressions(source, file));
    } else if (name === 'clippy.toml' || name === '.clippy.toml') {
      scanClippyToml(file, readBytes, hits);
    } else if (CARGO_CONFIG_RE.test(file)) {
      scanCargoConfig(file, readBytes, hits);
    }
  }
  return hits;
}

export function collectEntries({ listDir }) {
  const files = [];
  const walkerErrors = [];
  const walk = (dir) => {
    for (const entry of listDir(dir)) {
      const relative = dir === '' ? entry.name : `${dir}/${entry.name}`;
      if (entry.kind === 'symlink') {
        walkerErrors.push(`refusing to scan symbolic link ${relative}`);
        continue;
      }
      if (entry.kind === 'dir') {
        if (!SKIP_DIRS.has(entry.name)) walk(relative);
        continue;
      }
      if (entry.kind !== 'file') {
        walkerErrors.push(`refusing to scan non-regular entry ${relative}`);
        continue;
      }
      files.push(relative);
    }
  };
  walk('');
  files.sort((a, b) => {
    if (a < b) return -1;
    if (a > b) return 1;
    return 0;
  });
  return { files, walkerErrors };
}

export function scanTree({ listDir, readSource, readBytes }) {
  const { files, walkerErrors } = collectEntries({ listDir });
  const hits = walkerErrors
    .map((message) => ({ rule: 'WALKER', file: null, line: null, text: message }))
    .concat(scanSources({ files, readSource, readBytes }));
  return { files, walkerErrors, hits };
}

function formatHit(hit) {
  if (hit.file === null) return `${hit.rule} ${hit.text}`;
  const line = hit.line === null ? '' : `:${hit.line}`;
  return `${hit.rule} ${hit.file}${line}: ${hit.text}`;
}

function runScan({ listDir, readSource, readBytes, stdout, stderr, getEnv }) {
  const { files, hits } = scanTree({ listDir, readSource, readBytes });
  const prefix = getEnv('GITHUB_ACTIONS') === 'true' ? '::error::' : '';
  if (hits.length === 0) {
    stdout(`cognitive-complexity scan: ${files.length} files, no suppression found`);
    return 0;
  }
  for (const hit of hits) stderr(`${prefix}${formatHit(hit)}`);
  stderr(`cognitive-complexity scan: ${hits.length} suppression${hits.length === 1 ? '' : 's'} found`);
  return 1;
}

export function parseArgs(argv) {
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

export function main(argv, io = {}) {
  const stdout = io.stdout ?? ((text) => console.log(text));
  const stderr = io.stderr ?? ((text) => console.error(text));
  const getEnv = io.getEnv ?? ((name) => process.env[name]);
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
  if (options.mode === 'self-test') return selfTest({ stdout, stderr });
  return runScan({
    listDir: io.listDir ?? defaultListDir,
    readSource: io.readSource ?? defaultReadSource,
    readBytes: io.readBytes ?? defaultReadBytes,
    stdout,
    stderr,
    getEnv,
  });
}

function defaultListDir(dir) {
  const entries = fs.readdirSync(dir === '' ? ROOT : path.join(ROOT, dir), { withFileTypes: true });
  return entries.map((entry) => {
    if (entry.isSymbolicLink()) return { name: entry.name, kind: 'symlink' };
    if (entry.isDirectory()) return { name: entry.name, kind: 'dir' };
    if (entry.isFile()) return { name: entry.name, kind: 'file' };
    return { name: entry.name, kind: 'other' };
  });
}

function defaultReadSource(file) {
  return fs.readFileSync(path.join(ROOT, file), 'utf8');
}

function defaultReadBytes(file) {
  return fs.readFileSync(path.join(ROOT, file));
}

// ---------------------------------------------------------------------------
// In-memory self-test
// ---------------------------------------------------------------------------

function fixtureIo({ tree = {}, sources = {}, bytes = {} }) {
  return {
    listDir: (dir) => tree[dir] ?? [],
    readSource: (file) => {
      if (!(file in sources)) throw new Error(`fixture is missing source for ${file}`);
      return sources[file];
    },
    readBytes: (file) => {
      if (file in bytes) return bytes[file];
      if (file in sources) return Buffer.from(sources[file], 'utf8');
      throw new Error(`fixture is missing bytes for ${file}`);
    },
  };
}

function runFixture(files, sources = {}, bytes = {}) {
  const io = fixtureIo({ sources, bytes });
  return scanSources({ files, readSource: io.readSource, readBytes: io.readBytes });
}

function expectHit(hits, rule, file, line) {
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

function expectExit(argv, expected, io) {
  const code = main(argv, io);
  if (code !== expected) {
    throw new Error(`expected exit ${expected} for ${JSON.stringify(argv)}, got ${code}`);
  }
}

// The four comments and pinned assignment of the phase-1 root clippy.toml.
const PINNED_CLIPPY_TOML = [
  "# Pinned for #2234 so a future change of Clippy's default cannot move the gate.",
  '# scripts/check-cognitive-complexity.mjs asserts this exact line and asserts that',
  '# every diagnostic it reads carries 25 as its denominator.',
  '# See docs/quality/cognitive-complexity-gate.md (added in phase 8).',
  'cognitive-complexity-threshold = 25',
  '',
].join('\n');

// The exact bytes of the allowlisted root .cargo/config.toml.
const ALLOWED_CARGO_CONFIG = '[build]\njobs = 12\n';

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
      expectHit(hits, 'S1', 'src/legacy.rs', 1);
    }],
    ['case 47: the cognitive-complexity spelling fails S1', () => {
      const hits = runFixture(['src/legacy.rs'], {
        'src/legacy.rs': '// clippy: cognitive-complexity = 1000\n',
      });
      expectHit(hits, 'S1', 'src/legacy.rs', 1);
    }],
    ['case 48: every clippy cfg token variant fails S2 with its exact text, the string does not', () => {
      const variants = [
        ['cfg(not(clippy))', '#[cfg(not(clippy))]\nfn a() {}\n', 'clippy'],
        ['cfg_attr(clippy, ...)', '#[cfg_attr(clippy, allow(dead_code))]\n', 'clippy'],
        ['not(r#clippy)', '#[cfg(not(r#clippy))]\n', 'r#clippy'],
        ['not(clippy /*x*/)', '#[cfg(not(clippy /*x*/))]\n', 'clippy'],
        ['not(clippy,)', '#[cfg(not(clippy,))]\n', 'clippy'],
        ['nested cfg_attr(feature = "x", cfg(not(clippy)))', '#[cfg_attr(feature = "x", cfg(not(clippy)))]\n', 'clippy'],
        ['cfg(clippy = "y")', '#[cfg(clippy = "y")]\n', 'clippy'],
        ['cfg(not(clippy = "y"))', '#[cfg(not(clippy = "y"))]\n', 'clippy'],
        ['cfg_attr(not(clippy = "y"), ...)', '#[cfg_attr(not(clippy = "y"), allow(dead_code))]\n', 'clippy'],
      ];
      for (const [label, text, expectedText] of variants) {
        const hits = runFixture(['src/variant.rs'], { 'src/variant.rs': text });
        if (!hits.some((hit) => hit.rule === 'S2' && hit.text === expectedText)) {
          throw new Error(`expected S2 text ${JSON.stringify(expectedText)} for ${label}, got ${JSON.stringify(hits)}`);
        }
      }
      const stringHits = runFixture(['src/string.rs'], {
        'src/string.rs': 'fn s() { let _ = "cfg(not(clippy))"; }\n',
      });
      expectNoHits(stringHits);
    }],
    ['case 49: nested or changed clippy.toml fails S3, the pinned root one passes', () => {
      expectHit(runFixture(['src-tauri/clippy.toml']), 'S3', 'src-tauri/clippy.toml', null);
      expectHit(runFixture(['crates/x/.clippy.toml']), 'S3', 'crates/x/.clippy.toml', null);
      expectHit(runFixture(['.clippy.toml'], { '.clippy.toml': PINNED_CLIPPY_TOML }), 'S3', '.clippy.toml', null);
      expectNoHits(runFixture(['clippy.toml'], { 'clippy.toml': PINNED_CLIPPY_TOML }));
      const commentEdit = PINNED_CLIPPY_TOML.replace('so a future change', 'so a later change');
      expectNoHits(runFixture(['clippy.toml'], { 'clippy.toml': commentEdit }));
      const changed = [
        ['changed threshold', PINNED_CLIPPY_TOML.replace('threshold = 25', 'threshold = 30')],
        ['duplicate assignment', `${PINNED_CLIPPY_TOML}${PINNED_THRESHOLD_LINE}\n`],
        ['extra TOML assignment', PINNED_CLIPPY_TOML.replace('cognitive-complexity-threshold = 25', 'too-many-arguments-threshold = 8\ncognitive-complexity-threshold = 25')],
      ];
      for (const [label, text] of changed) {
        const hits = runFixture(['clippy.toml'], { 'clippy.toml': text });
        if (!hits.some((hit) => hit.rule === 'S3')) {
          throw new Error(`expected S3 for ${label}, got ${JSON.stringify(hits)}`);
        }
      }
    }],
    ['case 50: the allowlisted Cargo config passes, every other byte or path fails S4', () => {
      expectNoHits(runFixture([ROOT_CARGO_CONFIG], { [ROOT_CARGO_CONFIG]: ALLOWED_CARGO_CONFIG }));
      const variants = [
        ['one-byte change', ROOT_CARGO_CONFIG, '[build]\njobs = 13\n'],
        ['legacy .cargo/config', '.cargo/config', ALLOWED_CARGO_CONFIG],
        ['nested .cargo/config.toml', 'crates/x/.cargo/config.toml', ALLOWED_CARGO_CONFIG],
        ['quoted rustflags', ROOT_CARGO_CONFIG, '[build]\nrustflags = "--cap-lints allow"\n'],
        ['dotted rustflags', ROOT_CARGO_CONFIG, 'build.rustflags = ["-A", "clippy::all"]\n'],
        ['inline rustflags', ROOT_CARGO_CONFIG, '[build]\nrustflags = ["-A", "clippy::all"]\n'],
        ['spaced env', ROOT_CARGO_CONFIG, '[ env ]\nCLIPPY_CONF_DIR = "docs"\n'],
        ['quoted env', ROOT_CARGO_CONFIG, '["env"]\nCLIPPY_CONF_DIR = "docs"\n'],
        ['dotted env', ROOT_CARGO_CONFIG, 'env.CLIPPY_CONF_DIR = "docs"\n'],
      ];
      for (const [label, file, text] of variants) {
        const hits = runFixture([file], { [file]: text });
        if (!hits.some((hit) => hit.rule === 'S4')) {
          throw new Error(`expected S4 for ${label}, got ${JSON.stringify(hits)}`);
        }
      }
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
    ['case 53: a clean in-memory tree passes; a symlinked .rs fails the walker unread', () => {
      const clean = fixtureIo({
        tree: {
          '': [
            { name: 'src', kind: 'dir' },
            { name: '.cargo', kind: 'dir' },
            { name: 'clippy.toml', kind: 'file' },
          ],
          src: [{ name: 'a.rs', kind: 'file' }],
          '.cargo': [{ name: 'config.toml', kind: 'file' }],
        },
        sources: { 'src/a.rs': 'fn a() {}\n', 'clippy.toml': PINNED_CLIPPY_TOML },
        bytes: { '.cargo/config.toml': Buffer.from(ALLOWED_CARGO_CONFIG, 'utf8') },
      });
      const cleanResult = scanTree(clean);
      if (cleanResult.walkerErrors.length !== 0 || cleanResult.hits.length !== 0) {
        throw new Error(`expected a clean tree, got ${JSON.stringify(cleanResult)}`);
      }
      let reads = 0;
      const symlinked = {
        listDir: (dir) => {
          if (dir === '') return [{ name: 'src', kind: 'dir' }];
          if (dir === 'src') return [{ name: 'link.rs', kind: 'symlink' }];
          return [];
        },
        readSource: () => {
          reads += 1;
          throw new Error('the symlinked source must not be read');
        },
        readBytes: () => {
          reads += 1;
          throw new Error('the symlinked source must not be read');
        },
      };
      const symlinkResult = scanTree(symlinked);
      if (symlinkResult.walkerErrors.length !== 1 || reads !== 0) {
        throw new Error(`expected one walker error and no reads, got ${JSON.stringify(symlinkResult)} and ${reads} reads`);
      }
    }],
    ['case 54: usage errors exit 2 and --help exits 0', () => {
      const silent = { stdout: () => {}, stderr: () => {}, getEnv: () => undefined };
      expectExit(['--unknown'], 2, silent);
      expectExit(['--platform'], 2, silent);
      expectExit(['--platform', 'bogus', '--scan-sources'], 2, silent);
      expectExit(['--help'], 0, silent);
    }],
  ];
}

function selfTest({ stdout, stderr }) {
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
    for (const failure of failures) stderr(`check-cognitive-complexity self-test failed: ${failure}`);
    return 1;
  }
  stdout(`check-cognitive-complexity self-test passed (${cases.length} cases; scan-contract-v2)`);
  return 0;
}

process.exitCode = main(process.argv.slice(2));
