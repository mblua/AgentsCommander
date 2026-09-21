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
// The source lexer and the identity rule (#2254, phase 2 of #2234)
//
// This is the part of the gate that decides what a function is. It reads a
// Rust source file as text, walks it once as a lexer (comments, strings and
// literals are opaque), and records one frame per item header that ends in
// `{`; a header ended by a depth-0 `;` opens nothing. `containerAt` returns
// the `::`-joined tokens of the frames open at a site, `idFor` builds the
// stable id and `anchorFor` hashes the text from a site to its body brace.
// Everything reads through an injectable reader so the self-test runs in
// memory; the new code is inert until phase 3 calls it.
// ---------------------------------------------------------------------------

const ITEM_KEYWORDS = new Set(['fn', 'impl', 'trait', 'mod']);
const ITEM_PREFIX_WORDS = new Set([
  'pub', 'pub(...)', 'unsafe', 'default', 'async', 'const', 'extern', 'extern-string',
]);
const NAME_RE = /^(r#)?[A-Za-z_][A-Za-z0-9_]*$/;
const ANCHOR_CAP = 400;

/** The error class phase 3 turns into a failed run. */
export class CaptureError extends Error {
  constructor(message) {
    super(message);
    this.name = 'CAPTURE';
    this.code = 'CAPTURE';
  }
}

function lineStartsOf(source) {
  const starts = [0];
  for (let index = 0; index < source.length; index += 1) {
    if (source[index] === '\n') starts.push(index + 1);
  }
  return starts;
}

function lineNumberAt(starts, offset) {
  let low = 0;
  let high = starts.length - 1;
  while (low < high) {
    const middle = (low + high + 1) >> 1;
    if (starts[middle] <= offset) low = middle;
    else high = middle - 1;
  }
  return low + 1;
}

/** Returns the offset just past a nested block comment, or end of source. */
function skipBlockComment(source, start) {
  let index = start;
  let depth = 0;
  while (index < source.length) {
    if (source[index] === '/' && source[index + 1] === '*') {
      depth += 1;
      index += 2;
      continue;
    }
    if (source[index] === '*' && source[index + 1] === '/') {
      depth -= 1;
      index += 2;
      if (depth === 0) return index;
      continue;
    }
    index += 1;
  }
  return source.length;
}

/**
 * Returns the offset just past a raw or byte-raw string opened at `start`,
 * where `start` points at the `r` of `r`/`br`, or -1 when the prefix is not a
 * string. A nested `#` count must close with the same count.
 */
function rawStringEnd(source, start) {
  let index = source[start] === 'r' ? start + 1 : start + 2;
  let hashes = 0;
  while (source[index] === '#') {
    hashes += 1;
    index += 1;
  }
  if (source[index] !== '"') return -1;
  const terminator = `"${'#'.repeat(hashes)}`;
  const close = source.indexOf(terminator, index + 1);
  return close === -1 ? source.length : close + terminator.length;
}

/** Reads an identifier at `start`, keeping a raw prefix as written. */
function readIdentifier(source, start) {
  let index = start;
  while (index < source.length && isIdentContinue(source[index])) index += 1;
  let value = source.slice(start, index);
  if (value === 'r' && source[index] === '#' && isIdentStart(source[index + 1])) {
    let rawEnd = index + 1;
    while (rawEnd < source.length && isIdentContinue(source[rawEnd])) rawEnd += 1;
    value = source.slice(start, rawEnd);
    index = rawEnd;
  }
  return { value, end: index };
}

function atItemPosition(previousSignificantChar, previousWord) {
  if (previousSignificantChar === '') return true;
  if (
    (previousSignificantChar === '{' || previousSignificantChar === '}'
      || previousSignificantChar === ';' || previousSignificantChar === ']')
    && previousWord === ''
  ) {
    return true;
  }
  return ITEM_PREFIX_WORDS.has(previousWord);
}

/** True when the next code token after a `fn`/`mod`/`trait` is an identifier. */
function nextItemNameStarts(source, start) {
  let index = start;
  for (;;) {
    while (index < source.length && /\s/.test(source[index])) index += 1;
    if (source[index] === '/' && source[index + 1] === '/') {
      while (index < source.length && source[index] !== '\n') index += 1;
      continue;
    }
    if (source[index] === '/' && source[index + 1] === '*') {
      index = skipBlockComment(source, index);
      continue;
    }
    break;
  }
  if (isIdentStart(source[index])) return true;
  return source[index] === 'r' && source[index + 1] === '#' && isIdentStart(source[index + 2]);
}

function normalizeImplHeader(text) {
  return text
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/(^|[^\w#])where\b[\s\S]*$/, '$1')
    .trim();
}

/**
 * Returns the offset just past the comment or literal opened at `index`, with
 * its kind, or null when `index` does not open one. Comments leave the
 * previous significant character untouched; strings clear or replace it.
 */
function opaqueEnd(source, index) {
  const ch = source[index];
  if (ch === '/' && source[index + 1] === '/') {
    let cursor = index + 2;
    while (cursor < source.length && source[cursor] !== '\n') cursor += 1;
    return { kind: 'comment', end: cursor };
  }
  if (ch === '/' && source[index + 1] === '*') {
    return { kind: 'comment', end: skipBlockComment(source, index) };
  }
  if ((ch === 'r' || (ch === 'b' && source[index + 1] === 'r')) && !isIdentContinue(source[index - 1])) {
    const end = rawStringEnd(source, index);
    if (end !== -1) return { kind: 'string', end };
  }
  if (ch === '"') {
    let cursor = index + 1;
    while (cursor < source.length) {
      if (source[cursor] === '\\') {
        cursor = Math.min(source.length, cursor + 2);
        continue;
      }
      if (source[cursor] === '"') {
        cursor += 1;
        break;
      }
      cursor += 1;
    }
    return { kind: 'string', end: cursor };
  }
  return null;
}

/**
 * Walks the whole file as a lexer and returns the item frames, outermost
 * first. The frame opens at its `{` and closes at the matching `}`; a site is
 * attributed to a frame only while `open < site < close`, which keeps an item
 * out of its own frame while its inner items land under it.
 */
function lexSource(source, starts) {
  const frames = [];
  const stack = [];
  const length = source.length;
  let index = 0;
  let braceDepth = 0;
  let parenDepth = 0;
  let previousSignificantChar = '';
  let previousWord = '';
  let pending = null;
  let pubParenArmed = false;
  let pubParenDepth = -1;

  const append = (text) => {
    if (pending !== null) pending.text += text;
  };
  const resetWord = (ch) => {
    previousSignificantChar = ch;
    previousWord = '';
  };

  while (index < length) {
    const opaque = opaqueEnd(source, index);
    if (opaque !== null) {
      if (opaque.kind === 'string') {
        if (source[index] === '"') {
          const externBefore = previousWord === 'extern';
          previousSignificantChar = '"';
          previousWord = externBefore ? 'extern-string' : '';
        } else {
          resetWord('"');
        }
      }
      index = opaque.end;
      continue;
    }

    const ch = source[index];
    if (ch === "'") {
      if (isIdentContinue(source[index + 1]) && source[index + 2] !== "'") {
        append("'");
        previousSignificantChar = "'";
        index += 1;
        continue;
      }
      let cursor = index + 1;
      if (source[cursor] === '\\') cursor += 2;
      else cursor += 1;
      while (cursor < length && source[cursor] !== "'" && source[cursor] !== '\n') cursor += 1;
      index = cursor >= length ? length : cursor + 1;
      resetWord("'");
      continue;
    }

    if (isIdentStart(ch) && !isIdentContinue(source[index - 1])) {
      const { value, end } = readIdentifier(source, index);
      if (pending !== null) {
        if (pending.kind !== 'impl' && pending.name === null) pending.name = value;
        pending.text += value;
      } else if (
        ITEM_KEYWORDS.has(value)
        && atItemPosition(previousSignificantChar, previousWord)
        && (value === 'impl' || nextItemNameStarts(source, end))
      ) {
        pending = { kind: value, name: null, text: '', kwLine: lineNumberAt(starts, index), nest: 0 };
      }
      previousWord = value;
      previousSignificantChar = value[value.length - 1];
      pubParenArmed = value === 'pub';
      index = end;
      continue;
    }

    if (ch === ' ' || ch === '\t' || ch === '\r' || ch === '\n') {
      append(ch);
      index += 1;
      continue;
    }
    if (ch === '(') {
      parenDepth += 1;
      if (pubParenArmed) {
        pubParenDepth = parenDepth;
        pubParenArmed = false;
      }
      if (pending !== null) pending.nest += 1;
      append(ch);
      previousSignificantChar = '(';
      previousWord = pubParenDepth === parenDepth ? 'pub(' : '';
      index += 1;
      continue;
    }
    if (ch === ')') {
      const closesPub = pubParenDepth === parenDepth;
      parenDepth -= 1;
      if (pending !== null) pending.nest -= 1;
      append(ch);
      previousSignificantChar = ')';
      previousWord = closesPub ? 'pub(...)' : '';
      if (closesPub) pubParenDepth = -1;
      index += 1;
      continue;
    }
    if (ch === '[') {
      if (pending !== null) pending.nest += 1;
      append(ch);
      resetWord(ch);
      index += 1;
      continue;
    }
    if (ch === ']') {
      if (pending !== null) pending.nest -= 1;
      append(ch);
      resetWord(ch);
      index += 1;
      continue;
    }
    if (ch === '{') {
      braceDepth += 1;
      if (pending !== null && pending.nest <= 0) {
        const token = pending.kind === 'impl'
          ? `impl:${normalizeImplHeader(pending.text)}`
          : `${pending.kind}:${pending.name}`;
        const frame = {
          kind: pending.kind,
          name: pending.kind === 'impl' ? null : pending.name,
          token,
          kwLine: pending.kwLine,
          open: index,
          close: length,
          braceDepth,
        };
        frames.push(frame);
        stack.push(frame);
        pending = null;
      } else if (pending !== null) {
        append(ch);
      }
      resetWord(ch);
      index += 1;
      continue;
    }
    if (ch === '}') {
      append(ch);
      braceDepth -= 1;
      if (braceDepth < 0) braceDepth = 0;
      while (stack.length > 0 && stack[stack.length - 1].braceDepth > braceDepth) {
        stack.pop().close = index;
      }
      resetWord(ch);
      index += 1;
      continue;
    }
    if (ch === ';') {
      if (pending !== null && pending.nest <= 0) pending = null;
      else append(ch);
      resetWord(ch);
      index += 1;
      continue;
    }
    if (pubParenArmed) pubParenArmed = false;
    append(ch);
    resetWord(ch);
    index += 1;
  }

  return frames;
}

const lexCache = new WeakMap();

function loadLexed(path, readSource) {
  let byPath = lexCache.get(readSource);
  if (byPath === undefined) {
    byPath = new Map();
    lexCache.set(readSource, byPath);
  }
  let entry = byPath.get(path);
  if (entry === undefined) {
    let source;
    try {
      source = readSource(path);
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      throw new CaptureError(`cannot read source file ${path}: ${reason}`);
    }
    const starts = lineStartsOf(source);
    entry = { source, starts, frames: lexSource(source, starts) };
    byPath.set(path, entry);
  }
  return entry;
}

function offsetAt(entry, line, column) {
  const starts = entry.starts;
  if (!Number.isInteger(line) || line < 1 || line > starts.length) return entry.source.length;
  let offset = starts[line - 1];
  let remaining = Math.max(0, column - 1);
  while (remaining > 0 && offset < entry.source.length && entry.source[offset] !== '\n') {
    const codePoint = entry.source.codePointAt(offset);
    offset += codePoint > 0xffff ? 2 : 1;
    remaining -= 1;
  }
  return offset;
}

/**
 * The item frames of a file, cached per reader and path. `path` is the
 * repo-relative POSIX path the caller supplies; an unreadable file raises
 * CAPTURE rather than a silently empty result.
 */
export function lexFile(path, readSource = defaultReadSource) {
  return loadLexed(path, readSource).frames;
}

/** The `::`-joined tokens of the frames open at a site; "" at file scope. */
export function containerAt(path, line, column, readSource = defaultReadSource) {
  const entry = loadLexed(path, readSource);
  const offset = offsetAt(entry, line, column);
  return entry.frames
    .filter((frame) => frame.open < offset && offset < frame.close)
    .map((frame) => frame.token)
    .join('::');
}

/** The stable id `rust:<file>::<container>::<name>`, container omitted when empty. */
export function idFor(path, line, column, sliceText, readSource = defaultReadSource) {
  const name = typeof sliceText === 'string' && NAME_RE.test(sliceText) ? sliceText : '{closure}';
  const container = containerAt(path, line, column, readSource);
  return container === '' ? `rust:${path}::${name}` : `rust:${path}::${container}::${name}`;
}

/** The normalised code text from a site to its body brace, empty when no brace. */
function anchorText(source, start) {
  const length = source.length;
  let index = start;
  let nest = 0;
  let text = '';
  while (index < length && text.length < ANCHOR_CAP) {
    const opaque = opaqueEnd(source, index);
    if (opaque !== null) {
      index = opaque.end;
      continue;
    }
    const ch = source[index];
    if (ch === "'") {
      if (isIdentContinue(source[index + 1]) && source[index + 2] !== "'") {
        text += "'";
        index += 1;
        continue;
      }
      let cursor = index + 1;
      if (source[cursor] === '\\') cursor += 2;
      else cursor += 1;
      while (cursor < length && source[cursor] !== "'" && source[cursor] !== '\n') cursor += 1;
      index = cursor >= length ? length : cursor + 1;
      continue;
    }
    if (ch === '(' || ch === '[') {
      nest += 1;
      text += ch;
      index += 1;
      continue;
    }
    if (ch === ')' || ch === ']') {
      nest = Math.max(0, nest - 1);
      text += ch;
      index += 1;
      continue;
    }
    if (ch === '{') {
      text += ch;
      index += 1;
      if (nest === 0) break;
      continue;
    }
    text += ch;
    index += 1;
  }
  return text.replace(/\s+/g, ' ').trim();
}

/** The first 12 lowercase hex characters of the site's anchor text. */
export function anchorFor(path, line, column, readSource = defaultReadSource) {
  const entry = loadLexed(path, readSource);
  const offset = offsetAt(entry, line, column);
  return sha256(Buffer.from(anchorText(entry.source, offset), 'utf8')).slice(0, 12);
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

function fixtureReader(sources) {
  return (file) => {
    if (!(file in sources)) throw new Error(`fixture is missing source for ${file}`);
    return sources[file];
  };
}

/** The 1-based (line, column) of the `occurrence`-th `needle`, line first. */
function sourceSite(source, needle, occurrence = 1) {
  let index = -1;
  for (let seen = 0; seen < occurrence; seen += 1) {
    index = source.indexOf(needle, index + 1);
    if (index === -1) throw new Error(`fixture is missing ${JSON.stringify(needle)}`);
  }
  let line = 1;
  let lineStart = 0;
  for (let cursor = 0; cursor < index; cursor += 1) {
    if (source[cursor] === '\n') {
      line += 1;
      lineStart = cursor + 1;
    }
  }
  let column = 1;
  let offset = lineStart;
  while (offset < index) {
    const codePoint = source.codePointAt(offset);
    offset += codePoint > 0xffff ? 2 : 1;
    column += 1;
  }
  return { line, column };
}

/**
 * One in-memory fixture: a file path, its source and a reader over it. A new
 * fixture gets a fresh reader, so the lexer cache never leaks between cases.
 */
function fixture(file, source) {
  return { file, source, reader: fixtureReader({ [file]: source }) };
}

function idAt(fix, needle, sliceText, occurrence = 1) {
  const site = sourceSite(fix.source, needle, occurrence);
  return idFor(fix.file, site.line, site.column, sliceText, fix.reader);
}

function containerAtNeedle(fix, needle, occurrence = 1) {
  const site = sourceSite(fix.source, needle, occurrence);
  return containerAt(fix.file, site.line, site.column, fix.reader);
}

function anchorAt(fix, needle, occurrence = 1) {
  const site = sourceSite(fix.source, needle, occurrence);
  return anchorFor(fix.file, site.line, site.column, fix.reader);
}

function expectEqual(actual, expected, label) {
  if (actual !== expected) throw new Error(`${label}: expected ${expected}, got ${actual}`);
}

function expectDifferent(left, right, label) {
  if (left === right) throw new Error(`${label}: expected two different values, got ${left}`);
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
    ['case 29: a raw-identifier span slice is kept as the name, not {closure}', () => {
      const fix = fixture('src/raw.rs', 'fn r#match() {}\n');
      expectEqual(idAt(fix, 'r#match', 'r#match'), 'rust:src/raw.rs::r#match', 'r#match name');
    }],
    ['case 30: two handle methods in impl A and impl B get two ids', () => {
      const fix = fixture('src/two.rs', `impl A {
    fn handle(&self) {}
}
impl B {
    fn handle(&self) {}
}
`);
      const ids = [1, 2].map((occurrence) => idAt(fix, 'handle(&self)', 'handle', occurrence));
      expectDifferent(ids[0], ids[1], 'impl A and impl B');
      expectEqual(ids[0], 'rust:src/two.rs::impl:A::handle', 'impl A handle');
    }],
    ['case 31: impl Tr for T, trait Tr and mod m yield their three containers', () => {
      const fix = fixture('src/three.rs', `impl Tr for T {
    fn a(&self) {}
}
trait Tr {
    fn b(&self) {}
}
mod m {
    fn c(&self) {}
}
`);
      const containers = ['a(&self)', 'b(&self)', 'c(&self)'].map((needle) => containerAtNeedle(fix, needle));
      expectEqual(containers.join(' | '), 'impl:Tr for T | trait:Tr | mod:m', 'the three containers');
    }],
    ['case 32: a free function id carries no container segment', () => {
      const fix = fixture('src/free.rs', 'fn free() {}\n');
      expectEqual(idAt(fix, 'free()', 'free'), 'rust:src/free.rs::free', 'free id');
    }],
    ['case 33: a function inside another function inside impl A keeps both frames', () => {
      const fix = fixture('src/nested.rs', `impl A {
    fn outer() {
        fn inner() {}
    }
}
`);
      expectEqual(containerAtNeedle(fix, 'inner()'), 'impl:A::fn:outer', 'nested container');
      expectEqual(idAt(fix, 'inner()', 'inner'), 'rust:src/nested.rs::impl:A::fn:outer::inner', 'nested id');
    }],
    ['case 34: an impl where clause is dropped and its generics kept', () => {
      const fix = fixture('src/where.rs', `impl<T> Foo<T> where T: X {
    fn f() {}
}
`);
      expectEqual(containerAtNeedle(fix, 'f()'), 'impl:<T> Foo<T>', 'where clause dropped');
    }],
    ['case 35: an unreadable source raises CAPTURE instead of inventing file scope', () => {
      let caught = null;
      try {
        containerAt('src/missing.rs', 1, 1, () => {
          throw new Error('missing fixture');
        });
      } catch (error) {
        caught = error;
      }
      if (!(caught instanceof CaptureError) || caught.code !== 'CAPTURE') {
        throw new Error(`expected a CAPTURE error, got ${caught}`);
      }
    }],
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
    ['case 55: a function and closure after a complete impl block stay at file scope', () => {
      const fix = fixture('src/after.rs', `impl A {
    fn method(&self) {}
}
fn free() {
    let c = |x| x;
}
`);
      const containers = ['free()', '|x|'].map((needle) => containerAtNeedle(fix, needle));
      expectEqual(JSON.stringify(containers), JSON.stringify(['', 'fn:free']), 'free scope and closure scope');
    }],
    ['case 56: a closure under a module declaration stays under its function', () => {
      const fix = fixture('src/below.rs', `pub mod web;
fn free() {
    let c = |x| x;
}
`);
      expectEqual(containerAtNeedle(fix, 'free()'), '', 'module declaration opens nothing');
      expectEqual(idAt(fix, '|x|', '|x|'), 'rust:src/below.rs::fn:free::{closure}', 'closure id');
    }],
    ['case 57: four same-named methods under four different impls get four ids', () => {
      const fix = fixture('src/four.rs', `impl Display for F {
    fn fmt(&self) {}
}
impl Debug for F {
    fn fmt(&self) {}
}
impl From<String> for E {
    fn from(value: String) {}
}
impl From<&str> for E {
    fn from(value: &str) {}
}
`);
      const ids = [
        ['fmt(&self)', 'fmt', 1],
        ['fmt(&self)', 'fmt', 2],
        ['from(value: String)', 'from', 1],
        ['from(value: &str)', 'from', 1],
      ].map(([needle, sliceText, occurrence]) => idAt(fix, needle, sliceText, occurrence));
      if (new Set(ids).size !== 4) throw new Error(`expected four distinct ids, got ${JSON.stringify(ids)}`);
      expectEqual(ids[2], 'rust:src/four.rs::impl:From<String> for E::from', 'String impl');
      expectEqual(ids[3], 'rust:src/four.rs::impl:From<&str> for E::from', '&str impl');
    }],
    ['case 58: module paths separate siblings, and cfg-alternate siblings share one id', () => {
      const split = fixture('src/mods.rs', `mod a {
    mod inner {
        fn f() {}
    }
}
mod b {
    mod inner {
        fn f() {}
    }
}
`);
      expectDifferent(idAt(split, 'f()', 'f', 1), idAt(split, 'f()', 'f', 2), 'module paths');
      expectEqual(idAt(split, 'f()', 'f', 1), 'rust:src/mods.rs::mod:a::mod:inner::f', 'mod a path');
      expectEqual(idAt(split, 'f()', 'f', 2), 'rust:src/mods.rs::mod:b::mod:inner::f', 'mod b path');
      const shared = fixture('src/shared.rs', `mod p {
    mod inner {
        fn f() {}
    }
    mod inner {
        fn f() {}
    }
}
`);
      expectEqual(idAt(shared, 'f()', 'f', 1), 'rust:src/shared.rs::mod:p::mod:inner::f', 'shared path');
      expectEqual(idAt(shared, 'f()', 'f', 1), idAt(shared, 'f()', 'f', 2), 'cfg-alternate pair');
    }],
    ['case 59: a three-line impl header is joined and collapsed', () => {
      const fix = fixture('src/joined.rs', `impl<T>
    Trait<T>
    for Foo<T>
{
    fn f() {}
}
`);
      expectEqual(containerAtNeedle(fix, 'f()'), 'impl:<T> Trait<T> for Foo<T>', 'joined header');
    }],
    ['case 60: nested generics in an impl header survive whole', () => {
      const fix = fixture('src/generics.rs', `impl<T: Into<Vec<u8>>> Foo<T> {
    fn f() {}
}
`);
      expectEqual(containerAtNeedle(fix, 'f()'), 'impl:<T: Into<Vec<u8>>> Foo<T>', 'nested generics');
    }],
    ['case 61: trailing comments, raw strings and commented mod lines open nothing', () => {
      const fix = fixture('src/lexed.rs', `impl Foo for Bar // for Baz
{
    fn m(&self) {}
}
fn host() {
r#"impl Other {"#;
}
fn after() {}
fn host2() {
// mod x {
}
fn after2() {}
`);
      expectEqual(containerAtNeedle(fix, 'm(&self)'), 'impl:Foo for Bar', 'trailing comment');
      expectEqual(containerAtNeedle(fix, 'after()'), '', 'raw string opens nothing');
      expectEqual(containerAtNeedle(fix, 'after2()'), '', 'commented mod opens nothing');
    }],
    ['case 62: inserting an unrelated item changes the id set by exactly one id', () => {
      const base = fixture('src/insert.rs', `fn a() {}
impl A {
    fn b(&self) {}
}
`);
      const inserted = fixture('src/insert.rs', `struct P;
impl P {
    fn p(&self) {}
}
${base.source}`);
      const baseIds = new Set([idAt(base, 'a()', 'a'), idAt(base, 'b(&self)', 'b')]);
      const insertedIds = new Set([
        idAt(inserted, 'a()', 'a'),
        idAt(inserted, 'b(&self)', 'b'),
        idAt(inserted, 'p(&self)', 'p'),
      ]);
      const added = [...insertedIds].filter((id) => !baseIds.has(id));
      const removed = [...baseIds].filter((id) => !insertedIds.has(id));
      if (added.length !== 1 || added[0] !== 'rust:src/insert.rs::impl:P::p') {
        throw new Error(`expected exactly impl:P::p inserted, got ${JSON.stringify(added)}`);
      }
      if (removed.length !== 0) throw new Error(`unrelated ids moved: ${JSON.stringify(removed)}`);
      if (insertedIds.size !== 3) throw new Error(`struct P must contribute no id, got ${JSON.stringify([...insertedIds])}`);
    }],
    ['case 63: closures on their fn header line stay under their own method', () => {
      const fix = fixture('src/same-line.rs', `impl P {
    fn m1(&self) { let c = |x| x; }
    fn m2(&self) { let c = |x| x; }
}
`);
      expectEqual(idAt(fix, '|x|', '|x|', 1), 'rust:src/same-line.rs::impl:P::fn:m1::{closure}', 'm1 closure');
      expectEqual(idAt(fix, '|x|', '|x|', 2), 'rust:src/same-line.rs::impl:P::fn:m2::{closure}', 'm2 closure');
    }],
    ['case 64: headers ended by a semicolon open no scope', () => {
      const fix = fixture('src/semis.rs', `mod web;
fn f() {}
trait T {
    fn g(&self);
    fn h(&self) {}
}
`);
      expectEqual(containerAtNeedle(fix, 'f()'), '', 'mod declaration');
      expectEqual(containerAtNeedle(fix, 'h(&self)'), 'trait:T', 'trait method');
    }],
    ['case 65: closure anchors split shared ids, and identical closures share one anchor', () => {
      const different = fixture('src/anchors.rs', `fn f() {
    let a = |x: u32| { x };
    let b = |y: u32| { y };
}
`);
      expectEqual(idAt(different, '|x: u32|', '|x: u32|'), idAt(different, '|y: u32|', '|y: u32|'), 'shared closure id');
      expectDifferent(anchorAt(different, '|x: u32|'), anchorAt(different, '|y: u32|'), 'parameter lists');
      const identical = fixture('src/anchors.rs', `fn f() {
    let a = |x: u32| { x };
    let b = |x: u32| { y };
}
`);
      expectEqual(anchorAt(identical, '|x: u32|', 1), anchorAt(identical, '|x: u32|', 2), 'identical closures');
    }],
    ['case 66: a const-generic brace ends the impl header early', () => {
      const fix = fixture('src/const-generic.rs', `impl Foo<{N + 1}> {
    fn f() {}
}
`);
      const frames = lexFile(fix.file, fix.reader);
      if (frames.length === 0 || frames[0].token !== 'impl:Foo<') {
        throw new Error(`expected the header to end at the const-generic brace, got ${JSON.stringify(frames)}`);
      }
      expectEqual(containerAtNeedle(fix, 'f()'), '', 'truncated impl frame is closed');
    }],
    ['case 69: array types in signatures do not drop their frames', () => {
      const fix = fixture('src/arrays.rs', `fn f(x: u32) -> [u32; 3] { let c = |y| y; }
fn g(x: u32) -> [u32; 3] { let c = |y| y; }
impl Tr for [u8; 4] { fn m(&self) { let c = |y| y; } }
`);
      const containers = [1, 2, 3].map((occurrence) => containerAtNeedle(fix, '|y|', occurrence));
      const ids = [1, 2, 3].map((occurrence) => idAt(fix, '|y|', '|y|', occurrence));
      expectEqual(containers[0], 'fn:f', 'fn f container');
      expectEqual(containers[1], 'fn:g', 'fn g container');
      expectEqual(containers[2], 'impl:Tr for [u8; 4]::fn:m', 'impl method container');
      if (new Set(ids).size !== 3) throw new Error(`expected three distinct ids, got ${JSON.stringify(ids)}`);
      if (containers.some((container) => container === '')) throw new Error('a frame was lost to the array semicolon');
    }],
    ['case 72: a body edit leaves the anchor, a signature edit moves it', () => {
      const before = fixture('src/edit.rs', 'fn f() { let a = 1; }\n');
      const bodied = fixture('src/edit.rs', 'fn f() { let a = 1; let b = 2; }\n');
      const signature = fixture('src/edit.rs', 'fn f(x: u32) { let a = 1; }\n');
      expectEqual(anchorAt(before, 'f('), anchorAt(bodied, 'f('), 'body edit');
      expectDifferent(anchorAt(before, 'f('), anchorAt(signature, 'f('), 'signature edit');
    }],
    ['case 73: closure anchors split, and a lexed header ends at the real brace', () => {
      const split = fixture('src/decoy.rs', `fn f() {
    let a = |x: u32| { x };
    let b = |y: u32| { y };
}
`);
      expectDifferent(anchorAt(split, '|x: u32|'), anchorAt(split, '|y: u32|'), 'closure parameters');
      const decoy = fixture('src/decoy-header.rs', `fn f() -> [u8; "{".len()] // {
{ 0 }
`);
      const expected = sha256(Buffer.from('f() -> [u8; .len()] {', 'utf8')).slice(0, 12);
      expectEqual(anchorAt(decoy, 'f('), expected, 'anchor ends at the real brace');
    }],
    ['case 74: a braceless tail anchors over the capped text without raising', () => {
      const fix = fixture('src/capped.rs', `fn f() ${'x'.repeat(600)}\n`);
      const anchor = anchorAt(fix, 'f(');
      if (!/^[0-9a-f]{12}$/.test(anchor)) throw new Error(`expected 12 hex characters, got ${JSON.stringify(anchor)}`);
      const expected = sha256(Buffer.from(`f() ${'x'.repeat(396)}`, 'utf8')).slice(0, 12);
      expectEqual(anchor, expected, 'capped anchor');
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
