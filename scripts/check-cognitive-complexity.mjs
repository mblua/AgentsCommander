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
 * #2255 (phase 3 of #2234) - the capture pipeline and the verdict.
 *
 * `--capture` reads a `cargo clippy --message-format=json` stream, checks the
 * environment and the capture's integrity, asserts the threshold from both the
 * configuration and the data, normalises paths, collapses records to distinct
 * primary spans, rebuilds each site's id and anchor with phase 2's rule, and
 * compares the observed anchors against the baseline as multisets. `--report`
 * prints notices and returns 0; the enforcing mode prints `::error::`
 * annotations with a remedial line and returns 1. `--emit` writes the observed
 * anchors, and only after every check before the comparison has passed.
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
       node scripts/check-cognitive-complexity.mjs --capture <file> --clippy-exit <n> \\
             --platform <platform> --rustc-version <v> [--report] [--emit <file>] \\
             [--workspace-root <dir>] [--commit <sha>]

Runs the in-memory self-test of the cognitive-complexity detector, scans the
repository for Clippy cognitive-complexity suppression routes (rules S1 to S4),
or reads a Clippy JSON capture and applies the baseline ratchet.

  --self-test         Run the in-memory self-test; touches no file.
  --scan-sources      Walk the repository (skipping target/, node_modules/,
                      dist/ and .git/) and fail on any S1-S4 hit.
  --capture <file>    Read the JSON capture and compare it to the baseline.
  --clippy-exit <n>   The exit code of the clippy run that produced <file>.
  --rustc-version <v> The rustc release the capture was produced with.
  --report            Print findings as notices and return 0; an absent
                      baseline is an empty baseline, and no error annotation is
                      ever emitted.
  --emit <file>       Write the observed anchors after a successful read.
  --workspace-root <dir> The root that relative file_name values are read
                      under; defaults to the repository root.
  --commit <sha>      The commit recorded in the --emit document.
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
  const flags = {
    help: false,
    selfTest: false,
    scanSources: false,
    platform: undefined,
    capture: undefined,
    clippyExit: undefined,
    rustcVersion: undefined,
    report: false,
    emit: undefined,
    workspaceRoot: undefined,
    commit: undefined,
  };
  const value = (index, name) => {
    const next = argv[index + 1];
    if (next === undefined || next.startsWith('--')) throw new UsageError(`${name} requires a value`);
    return next;
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--self-test') {
      flags.selfTest = true;
    } else if (arg === '--scan-sources') {
      flags.scanSources = true;
    } else if (arg === '--help') {
      flags.help = true;
    } else if (arg === '--report') {
      flags.report = true;
    } else if (arg === '--platform') {
      const value_ = argv[i + 1];
      if (value_ === undefined) throw new UsageError('--platform requires a value: windows, linux or macos');
      if (!PLATFORMS.includes(value_)) {
        throw new UsageError(`--platform must be one of windows, linux or macos, got ${JSON.stringify(value_)}`);
      }
      flags.platform = value_;
      i += 1;
    } else if (arg === '--capture') {
      flags.capture = value(i, '--capture');
      i += 1;
    } else if (arg === '--clippy-exit') {
      const raw = value(i, '--clippy-exit');
      if (!/^-?[0-9]+$/.test(raw)) {
        throw new UsageError(`--clippy-exit must be an integer, got ${JSON.stringify(raw)}`);
      }
      flags.clippyExit = Number(raw);
      i += 1;
    } else if (arg === '--rustc-version') {
      flags.rustcVersion = value(i, '--rustc-version');
      i += 1;
    } else if (arg === '--emit') {
      flags.emit = value(i, '--emit');
      i += 1;
    } else if (arg === '--workspace-root') {
      flags.workspaceRoot = value(i, '--workspace-root');
      i += 1;
    } else if (arg === '--commit') {
      flags.commit = value(i, '--commit');
      i += 1;
    } else {
      throw new UsageError(`unknown argument: ${arg}`);
    }
  }
  if (flags.help) return { mode: 'help', platform: flags.platform };
  const modes = [];
  if (flags.selfTest) modes.push('self-test');
  if (flags.scanSources) modes.push('scan-sources');
  if (flags.capture !== undefined) modes.push('capture');
  if (modes.length !== 1) {
    throw new UsageError('exactly one mode is required: --self-test, --scan-sources or --capture');
  }
  if (modes[0] === 'capture') {
    if (flags.platform === undefined) {
      throw new UsageError('--capture requires --platform windows, linux or macos');
    }
    if (flags.clippyExit === undefined) throw new UsageError('--capture requires --clippy-exit <n>');
    if (flags.rustcVersion === undefined) throw new UsageError('--capture requires --rustc-version <v>');
    return {
      mode: 'capture',
      captureFile: flags.capture,
      clippyExit: flags.clippyExit,
      platform: flags.platform,
      rustcVersion: flags.rustcVersion,
      report: flags.report,
      emit: flags.emit,
      workspaceRoot: flags.workspaceRoot,
      commit: flags.commit,
    };
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
  if (options.mode === 'capture') {
    try {
      return runCapture(options, {
        stderr,
        getEnv,
        readCaptureText: io.readCaptureText,
        readWorkspace: io.readWorkspace,
        readWorkspaceBytes: io.readWorkspaceBytes,
        existsWorkspace: io.existsWorkspace,
        writeText: io.writeText,
      });
    } catch (error) {
      if (
        error instanceof CaptureError
        || error instanceof ConfigError
        || error instanceof ThresholdError
      ) {
        stderr(`${error.code}: ${error.message}`);
        return 1;
      }
      throw error;
    }
  }
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

/** A failure of the gate's own configuration. */
export class ConfigError extends Error {
  constructor(message) {
    super(message);
    this.name = 'CONFIG';
    this.code = 'CONFIG';
  }
}

/** A cognitive-complexity message that does not carry the pinned denominator. */
export class ThresholdError extends Error {
  constructor(message) {
    super(message);
    this.name = 'THRESHOLD';
    this.code = 'THRESHOLD';
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
// The capture pipeline and the verdict (#2255, phase 3 of #2234)
//
// `--capture` never guesses: it reprints every non-cognitive diagnostic first
// so a broken build still shows its human output, then checks the environment,
// returns Clippy's own exit code when Clippy failed, and only then reads the
// capture as a complete record (parseable, ending in `build-finished`) whose
// outcome must agree with a zero exit. The threshold is asserted from the
// clippy.toml that Clippy actually reads, then from the denominator inside each
// cognitive message. Paths are normalised against the workspace root, records
// are collapsed to distinct primary spans so the lib / lib-test duplicate pair
// counts once, and phase 2's `idFor` and `anchorFor` rebuild the identity.
// The verdict compares anchor multisets, so an equal-count replacement is one
// NEW plus one STALE rather than a pass. `--report` prints `::notice::` lines
// and returns 0; the enforcing mode prints `::error::` annotations with the
// remedial line and returns 1 on any NEW or STALE. `--emit` writes the
// observed anchors in both modes, and only when every earlier step passed.
// ---------------------------------------------------------------------------

const COGNITIVE_CODE = 'clippy::cognitive_complexity';
const BASELINE_FILE = 'cognitive-complexity.baseline.json';
const COGNITIVE_MESSAGE_RE = /^the function has a cognitive complexity of \((\d+)\/(\d+)\)$/;
const ANCHOR_RE = /^[0-9a-f]{12}$/;
const NEW_REMEDIAL =
  'Reduce the function to 25 or below. The baseline records debt at adoption and may not grow.';
const STALE_REMEDIAL =
  "Remove this entry from cognitive-complexity.baseline.json in the same pull request if the debt is gone; if instead the site's header changed, its anchor moved and the ratchet has no re-anchoring path, so reduce the function to 25 or below.";

/** Collapses `.` and `..`; null when a `..` escapes above the start. */
function normaliseSegments(pathText) {
  const segments = [];
  for (const segment of pathText.split('/')) {
    if (segment === '' || segment === '.') continue;
    if (segment === '..') {
      if (segments.length === 0) return null;
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  return segments.join('/');
}

/**
 * The workspace-relative POSIX path of a capture `file_name`, or a CAPTURE
 * error when the path is absolute outside the root or climbs above it.
 */
function normaliseCapturePath(fileName, workspaceRoot) {
  const slashed = String(fileName).replace(/\\/g, '/');
  const root = String(workspaceRoot).replace(/\\/g, '/').replace(/\/+$/, '');
  const absolute = slashed.startsWith('/') || /^[A-Za-z]:\//.test(slashed);
  let relative = slashed;
  if (absolute) {
    if (root === '' || !(slashed === root || slashed.startsWith(`${root}/`))) {
      throw new CaptureError(
        `capture path escapes the workspace root ${JSON.stringify(workspaceRoot)}: ${JSON.stringify(fileName)}`,
      );
    }
    relative = slashed.slice(root.length).replace(/^\/+/, '');
  }
  const normalised = normaliseSegments(relative);
  if (normalised === null) {
    throw new CaptureError(
      `capture path escapes the workspace root ${JSON.stringify(workspaceRoot)}: ${JSON.stringify(fileName)}`,
    );
  }
  return normalised;
}

/** Validates section 4's schema, then the threshold and toolchain agreement. */
function validateBaseline(document, rustcVersion) {
  if (document === null || typeof document !== 'object' || Array.isArray(document)) {
    throw new ConfigError('baseline must be a JSON object');
  }
  if (document.version !== 1) {
    throw new ConfigError(`baseline version must be 1, got ${JSON.stringify(document.version)}`);
  }
  if (document.threshold !== 25) {
    throw new ConfigError(`baseline threshold must be 25, got ${JSON.stringify(document.threshold)}`);
  }
  if (document.toolchain !== rustcVersion) {
    throw new ConfigError(
      `baseline toolchain must be ${JSON.stringify(rustcVersion)}, got ${JSON.stringify(document.toolchain)}`,
    );
  }
  if (!Array.isArray(document.entries)) {
    throw new ConfigError('baseline entries must be an array');
  }
  let previousId = null;
  for (const entry of document.entries) {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new ConfigError('baseline entries must be objects');
    }
    if (typeof entry.id !== 'string' || entry.id === '') {
      throw new ConfigError('baseline entry id must be a non-empty string');
    }
    if (previousId !== null && entry.id <= previousId) {
      throw new ConfigError(
        `baseline entries must be sorted by id and unique; ${JSON.stringify(entry.id)} follows ${JSON.stringify(previousId)}`,
      );
    }
    previousId = entry.id;
    const sites = entry.sites;
    if (sites === null || typeof sites !== 'object' || Array.isArray(sites)) {
      throw new ConfigError(`baseline entry ${entry.id} sites must be an object`);
    }
    const platforms = Object.keys(sites);
    if (platforms.length === 0) {
      throw new ConfigError(`baseline entry ${entry.id} sites must not be empty`);
    }
    for (const platform of platforms) {
      if (!PLATFORMS.includes(platform)) {
        throw new ConfigError(`baseline entry ${entry.id} has unknown platform ${JSON.stringify(platform)}`);
      }
      const anchors = sites[platform];
      if (!Array.isArray(anchors) || anchors.length === 0) {
        throw new ConfigError(`baseline entry ${entry.id}.sites.${platform} must be a non-empty array`);
      }
      for (let index = 0; index < anchors.length; index += 1) {
        if (typeof anchors[index] !== 'string' || !ANCHOR_RE.test(anchors[index])) {
          throw new ConfigError(
            `baseline anchor ${JSON.stringify(anchors[index])} in ${entry.id}.sites.${platform} must be 12 lowercase hex characters`,
          );
        }
        if (index > 0 && anchors[index] < anchors[index - 1]) {
          throw new ConfigError(`baseline entry ${entry.id}.sites.${platform} must be sorted`);
        }
      }
    }
  }
}

/**
 * The `--capture` mode, in the order of section 5. Returns an exit code; every
 * failure throws a typed error that `main` prints as `<CODE>: <message>`.
 */
export function runCapture(options, io = {}) {
  const stderr = io.stderr ?? ((text) => console.error(text));
  const getEnv = io.getEnv ?? ((name) => process.env[name]);
  const workspaceRoot = options.workspaceRoot ?? ROOT;
  const readCaptureText = io.readCaptureText ?? ((file) => fs.readFileSync(file, 'utf8'));
  const readWorkspace =
    io.readWorkspace ?? ((file) => fs.readFileSync(path.join(workspaceRoot, file), 'utf8'));
  const readWorkspaceBytes =
    io.readWorkspaceBytes ?? ((file) => fs.readFileSync(path.join(workspaceRoot, file)));
  const existsWorkspace = io.existsWorkspace ?? ((file) => fs.existsSync(path.join(workspaceRoot, file)));
  const writeText = io.writeText ?? ((file, text) => fs.writeFileSync(file, text));

  // 1. Render first, before anything can fail, so a broken build still shows
  // its diagnostics. Cognitive messages are left to the verdict's own lines.
  let captureText = null;
  let readError = null;
  try {
    captureText = readCaptureText(options.captureFile);
  } catch (error) {
    readError = error;
  }
  const records = [];
  let parseError = null;
  if (typeof captureText === 'string') {
    for (const line of captureText.split('\n')) {
      if (!line.startsWith('{')) continue;
      try {
        records.push(JSON.parse(line));
      } catch {
        parseError = line;
        break;
      }
    }
  }
  for (const record of records) {
    if (record?.reason !== 'compiler-message') continue;
    if (record.message?.code?.code === COGNITIVE_CODE) continue;
    const rendered = record.message?.rendered;
    const text =
      typeof rendered === 'string' && rendered.length > 0 ? rendered : record.message?.message;
    if (typeof text === 'string' && text.length > 0) stderr(text);
  }

  // 2. Environment. Either variable silences the whole run while leaving exit 0.
  if (getEnv('CLIPPY_CONF_DIR') !== undefined) {
    throw new ConfigError('CLIPPY_CONF_DIR is set; it silences the cognitive-complexity lint');
  }
  for (const name of ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS']) {
    const value = getEnv(name);
    if (typeof value === 'string' && (value.includes('cap-lints') || value.includes('cognitive'))) {
      throw new ConfigError(
        `${name} contains ${JSON.stringify(value)}; it silences the cognitive-complexity lint`,
      );
    }
  }

  // 3. Clippy's own failure is the answer; the ratchet is not consulted.
  if (options.clippyExit !== 0) {
    stderr(`cognitive gate not evaluated: clippy exited ${options.clippyExit}`);
    return options.clippyExit;
  }

  // 4. Integrity first, then outcome. An empty capture is never "clean".
  if (readError !== null) {
    const reason = readError instanceof Error ? readError.message : String(readError);
    throw new CaptureError(`cannot read ${options.captureFile}: ${reason}`);
  }
  if (captureText.length === 0) {
    throw new CaptureError(`${options.captureFile} is empty`);
  }
  if (parseError !== null) {
    throw new CaptureError(
      `${options.captureFile} has a line starting with '{' that does not parse: ${JSON.stringify(parseError.slice(0, 120))}`,
    );
  }
  // A terminal build-finished is what makes the capture a complete record:
  // records after it mean a second run was truncated into the same file.
  const finished = records[records.length - 1];
  if (finished?.reason !== 'build-finished') {
    throw new CaptureError(`${options.captureFile} has no terminal build-finished record`);
  }
  if (finished.success !== true) {
    throw new CaptureError(
      `${options.captureFile} reports build-finished success ${JSON.stringify(finished.success)} while clippy exited 0`,
    );
  }

  // 5. Threshold file. Clippy 1.97.1 gives a root .clippy.toml precedence.
  if (existsWorkspace('.clippy.toml')) {
    throw new ConfigError('root .clippy.toml exists and takes precedence over clippy.toml; remove it');
  }
  let tomlBytes;
  try {
    tomlBytes = readWorkspaceBytes('clippy.toml');
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    throw new ConfigError(`cannot read root clippy.toml: ${reason}`);
  }
  let tomlText;
  try {
    tomlText = new TextDecoder('utf-8', { fatal: true }).decode(tomlBytes);
  } catch {
    throw new ConfigError('root clippy.toml is not valid UTF-8');
  }
  const tomlProblem = validateRootClippyToml(tomlText);
  if (tomlProblem !== null) throw new ConfigError(tomlProblem.message);

  // 6. Baseline. An absent document is only legal under --report.
  let baselineDocument;
  let baselineText = null;
  try {
    baselineText = readWorkspace(BASELINE_FILE);
  } catch {
    baselineText = null;
  }
  if (baselineText === null) {
    if (!options.report) throw new CaptureError(`${BASELINE_FILE} not found`);
    baselineDocument = {
      version: 1,
      issue: 2234,
      threshold: 25,
      toolchain: options.rustcVersion,
      capturedFrom: null,
      capturedAt: null,
      entries: [],
    };
  } else {
    let parsed;
    try {
      parsed = JSON.parse(baselineText);
    } catch {
      throw new ConfigError(`${BASELINE_FILE} is not valid JSON`);
    }
    validateBaseline(parsed, options.rustcVersion);
    baselineDocument = parsed;
  }

  // 7. Threshold in the data: every cognitive message carries the pinned 25.
  const cognitiveRecords = records.filter(
    (record) =>
      record?.reason === 'compiler-message' && record.message?.code?.code === COGNITIVE_CODE,
  );
  for (const record of cognitiveRecords) {
    const message = record.message?.message;
    const match = typeof message === 'string' ? COGNITIVE_MESSAGE_RE.exec(message) : null;
    if (match === null || match[2] !== '25') {
      throw new ThresholdError(
        `cognitive-complexity message does not carry the pinned denominator 25: ${JSON.stringify(message)}`,
      );
    }
  }

  // 8 and 9. Normalise the path, then collapse to distinct primary spans. The
  // whole span is the separating key: (file, line_start) merges two sites that
  // share a line but are separated by column.
  const sites = new Map();
  for (const record of cognitiveRecords) {
    const span = (record.message.spans ?? []).find((candidate) => candidate?.is_primary === true);
    if (span === undefined) {
      throw new CaptureError(
        `cognitive-complexity message carries no primary span: ${JSON.stringify(record.message.message)}`,
      );
    }
    const file = normaliseCapturePath(span.file_name, workspaceRoot);
    const key = [file, span.line_start, span.line_end, span.column_start, span.column_end].join('\u0000');
    if (sites.has(key)) continue;
    const selection = Array.isArray(span.text) ? span.text[0] : undefined;
    const sliceText =
      selection === undefined
        ? undefined
        : selection.text.slice(selection.highlight_start - 1, selection.highlight_end - 1);
    sites.set(key, {
      file,
      line: span.line_start,
      column: span.column_start,
      sliceText,
      rendered: typeof record.message.rendered === 'string' ? record.message.rendered : null,
    });
  }

  // 10 and 11. Phase 2's identity rule, exactly, and the sorted anchors per id.
  const observed = new Map();
  const renderedByAnchor = new Map();
  for (const site of sites.values()) {
    const id = idFor(site.file, site.line, site.column, site.sliceText, readWorkspace);
    const anchor = anchorFor(site.file, site.line, site.column, readWorkspace);
    if (!observed.has(id)) observed.set(id, []);
    observed.get(id).push(anchor);
    const anchorKey = `${id}\u0000${anchor}`;
    if (!renderedByAnchor.has(anchorKey)) renderedByAnchor.set(anchorKey, site.rendered);
  }
  for (const anchors of observed.values()) anchors.sort();

  // 14. Emission, after steps 2 to 11 all passed and before the verdict.
  if (options.emit !== undefined) {
    const entries = [...observed.keys()]
      .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
      .map((id) => ({ id, anchors: observed.get(id) }));
    const document = {
      platform: options.platform,
      commit: options.commit ?? null,
      rustcVersion: options.rustcVersion,
      entries,
    };
    writeText(options.emit, `${JSON.stringify(document, null, 2)}\n`);
  }

  // 12. Anchor multisets, not counts: an equal-count replacement is refused.
  const baselineById = new Map(baselineDocument.entries.map((entry) => [entry.id, entry]));
  const ids = [...new Set([...observed.keys(), ...baselineById.keys()])].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  const newFindings = [];
  const staleFindings = [];
  for (const id of ids) {
    const observedAnchors = observed.get(id) ?? [];
    const entry = baselineById.get(id);
    const baselineAnchors = entry === undefined ? undefined : entry.sites[options.platform];
    const remaining = new Map();
    for (const anchor of baselineAnchors ?? []) {
      remaining.set(anchor, (remaining.get(anchor) ?? 0) + 1);
    }
    for (const anchor of observedAnchors) {
      const count = remaining.get(anchor) ?? 0;
      if (count === 0) {
        newFindings.push({
          id,
          anchor,
          observed: observedAnchors.length,
          baseline: baselineAnchors === undefined ? 'absent' : baselineAnchors.length,
          rendered: renderedByAnchor.get(`${id}\u0000${anchor}`) ?? null,
        });
      } else {
        remaining.set(anchor, count - 1);
      }
    }
    if (baselineAnchors !== undefined) {
      for (const [anchor, count] of remaining) {
        if (count > 0) {
          staleFindings.push({
            id,
            anchor,
            observed: observedAnchors.length === 0 ? 'none' : observedAnchors.length,
            baseline: baselineAnchors.length,
          });
        }
      }
    }
  }

  // 13. Report mode never emits an Actions error annotation and never fails.
  const underActions = getEnv('GITHUB_ACTIONS') === 'true';
  if (options.report) {
    const prefix = underActions ? '::notice::' : '';
    for (const finding of newFindings) {
      if (finding.rendered !== null) stderr(finding.rendered);
      stderr(
        `${prefix}NEW cognitive complexity above 25: ${finding.id}#${finding.anchor} (observed ${finding.observed}, baseline ${finding.baseline}) on ${options.platform}`,
      );
    }
    for (const finding of staleFindings) {
      stderr(
        `${prefix}STALE baseline entry: ${finding.id}#${finding.anchor} (baseline ${finding.baseline}, observed ${finding.observed}) on ${options.platform}`,
      );
    }
    return 0;
  }
  const prefix = underActions ? '::error::' : '';
  for (const finding of newFindings) {
    if (finding.rendered !== null) stderr(finding.rendered);
    stderr(
      `${prefix}NEW cognitive complexity above 25: ${finding.id}#${finding.anchor} (observed ${finding.observed}, baseline ${finding.baseline}) on ${options.platform}`,
    );
    stderr(NEW_REMEDIAL);
  }
  for (const finding of staleFindings) {
    stderr(
      `${prefix}STALE baseline entry: ${finding.id}#${finding.anchor} (baseline ${finding.baseline}, observed ${finding.observed}) on ${options.platform}`,
    );
    stderr(STALE_REMEDIAL);
  }
  return newFindings.length > 0 || staleFindings.length > 0 ? 1 : 0;
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

// Phase 3 capture fixtures. Every read, the environment and the emission write
// are injectable, so the self-test exercises main() itself over the same
// in-memory tree a real capture uses.
const CAPTURE_FILE = 'capture.jsonl';
const TEST_WORKSPACE_ROOT = 'C:/ws';

function jsonl(...records) {
  return `${records.map((record) => JSON.stringify(record)).join('\n')}\n`;
}

function buildFinished(success = true) {
  return { reason: 'build-finished', success };
}

function messageRecord({
  file,
  line,
  column,
  slice,
  message,
  code,
  level = 'error',
  rendered,
  endLine,
  endColumn,
  text,
}) {
  const spanText = text ?? slice;
  return {
    reason: 'compiler-message',
    target: { name: 'app', kind: ['lib'], src_path: file },
    message: {
      rendered: rendered ?? `error: ${message}\n  --> ${file}:${line}:${column}\n`,
      level,
      message,
      code: code === undefined ? null : { code, explanation: null },
      spans: [
        {
          file_name: file,
          line_start: line,
          line_end: endLine ?? line,
          column_start: column,
          column_end: endColumn ?? column + (spanText === undefined ? 1 : spanText.length),
          is_primary: true,
          text:
            spanText === undefined
              ? []
              : [
                  {
                    text: spanText,
                    highlight_start: 1,
                    highlight_end: spanText.length + 1,
                  },
                ],
        },
      ],
    },
  };
}

function cognitiveRecord({ message = 'the function has a cognitive complexity of (31/25)', ...rest }) {
  return messageRecord({ ...rest, message, code: COGNITIVE_CODE });
}

function baselineJson(entries, overrides = {}) {
  return JSON.stringify({
    version: 1,
    issue: 2234,
    threshold: 25,
    toolchain: '1.97.1',
    capturedFrom: 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeef',
    capturedAt: '2026-01-01T00:00:00Z',
    entries,
    ...overrides,
  });
}

function otherAnchor(...exclude) {
  for (const digit of '0123456789abcdef') {
    const candidate = digit.repeat(12);
    if (!exclude.includes(candidate)) return candidate;
  }
  throw new Error('no unused anchor candidate');
}

function captureFixture({ capture = '', workspace = {}, env = {} } = {}) {
  const captureFiles = { [CAPTURE_FILE]: capture };
  const stdoutLines = [];
  const stderrLines = [];
  const writes = new Map();
  const reader = (file) => {
    if (!(file in workspace)) {
      const error = new Error(`ENOENT: no such file in the fixture workspace: ${file}`);
      error.code = 'ENOENT';
      throw error;
    }
    return workspace[file];
  };
  const io = {
    stdout: (text) => stdoutLines.push(text),
    stderr: (text) => stderrLines.push(text),
    getEnv: (name) => env[name],
    readCaptureText: (file) => {
      if (!(file in captureFiles)) {
        const error = new Error(`ENOENT: no such capture: ${file}`);
        error.code = 'ENOENT';
        throw error;
      }
      return captureFiles[file];
    },
    readWorkspace: reader,
    readWorkspaceBytes: (file) => Buffer.from(reader(file), 'utf8'),
    existsWorkspace: (file) => file in workspace,
    writeText: (file, text) => writes.set(file, text),
  };
  const baseArgs = [
    '--capture',
    CAPTURE_FILE,
    '--clippy-exit',
    '0',
    '--platform',
    'windows',
    '--rustc-version',
    '1.97.1',
    '--workspace-root',
    TEST_WORKSPACE_ROOT,
  ];
  return {
    workspace,
    reader,
    state: { stdout: stdoutLines, stderr: stderrLines, writes },
    run: (extra = []) => main([...baseArgs, ...extra], io),
  };
}

function stderrText(fixture) {
  return fixture.state.stderr.join('\n');
}

function findingLines(fixture, kind) {
  const marker = kind === 'NEW' ? 'NEW cognitive complexity above 25:' : 'STALE baseline entry:';
  return fixture.state.stderr.filter((line) => line.includes(marker));
}

const HEAVY_SOURCE = 'fn heavy() {\n    if a { }\n}\n';
const TWO_SITES_SOURCE =
  'impl A {\n    fn m(&self) { if x {} }\n    fn m(&self, x: u32) { if y {} }\n}\n';

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
    ['case 1: observed set equals the baseline for this platform', () => {
      const workspace = { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML };
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src\\a.rs', line: 1, column: 1, slice: 'heavy' }),
          cognitiveRecord({ file: 'src/a.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace,
      });
      workspace['cognitive-complexity.baseline.json'] = baselineJson([
        {
          id: 'rust:src/a.rs::heavy',
          sites: { windows: [anchorFor('src/a.rs', 1, 1, fixture.reader)] },
        },
      ]);
      expectEqual(fixture.run(), 0, 'a matching anchor passes');
      expectEqual(fixture.state.stderr.length, 0, 'a pass prints nothing');
    }],
    ['case 2: an observed id absent from the baseline is a NEW in enforcing mode', () => {
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/a.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace: { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
        env: { GITHUB_ACTIONS: 'true' },
      });
      fixture.workspace['cognitive-complexity.baseline.json'] = baselineJson([]);
      expectEqual(fixture.run(), 1, 'a NEW exits 1');
      const lines = findingLines(fixture, 'NEW');
      if (lines.length !== 1) throw new Error(`expected one NEW line, got ${JSON.stringify(lines)}`);
      if (!lines[0].startsWith('::error::')) {
        throw new Error(`expected the ::error:: annotation, got ${lines[0]}`);
      }
      if (!fixture.state.stderr.includes(NEW_REMEDIAL)) {
        throw new Error('the remedial line was not printed');
      }
    }],
    ['case 3: two sites observed against one baselined anchor is a NEW', () => {
      const first = sourceSite(TWO_SITES_SOURCE, 'm(&self)');
      const second = sourceSite(TWO_SITES_SOURCE, 'm(&self, x: u32)');
      const workspace = { 'src/two.rs': TWO_SITES_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML };
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/two.rs', line: first.line, column: first.column, slice: 'm' }),
          cognitiveRecord({ file: 'src/two.rs', line: second.line, column: second.column, slice: 'm' }),
          buildFinished(),
        ),
        workspace,
      });
      const secondAnchor = anchorFor('src/two.rs', second.line, second.column, fixture.reader);
      workspace['cognitive-complexity.baseline.json'] = baselineJson([
        {
          id: 'rust:src/two.rs::impl:A::m',
          sites: { windows: [anchorFor('src/two.rs', first.line, first.column, fixture.reader)] },
        },
      ]);
      expectEqual(fixture.run(), 1, 'the second site is a NEW');
      const lines = findingLines(fixture, 'NEW');
      if (lines.length !== 1 || !lines[0].includes(`#${secondAnchor}`)) {
        throw new Error(`expected one NEW for ${secondAnchor}, got ${JSON.stringify(lines)}`);
      }
    }],
    ['case 4: a baselined anchor with nothing observed is a STALE', () => {
      const fixture = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: {
          'clippy.toml': PINNED_CLIPPY_TOML,
          'cognitive-complexity.baseline.json': baselineJson([
            { id: 'rust:src/gone.rs::heavy', sites: { windows: ['a'.repeat(12)] } },
          ]),
        },
      });
      expectEqual(fixture.run(), 1, 'a STALE exits 1');
      const lines = findingLines(fixture, 'STALE');
      if (lines.length !== 1 || !lines[0].includes('rust:src/gone.rs::heavy#aaaaaaaaaaaa')) {
        throw new Error(`expected one STALE, got ${JSON.stringify(lines)}`);
      }
    }],
    ['case 5: two baselined anchors with one observed is a STALE', () => {
      const workspace = { 'src/one.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML };
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/one.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace,
      });
      const anchor = anchorFor('src/one.rs', 1, 1, fixture.reader);
      const missing = otherAnchor(anchor);
      workspace['cognitive-complexity.baseline.json'] = baselineJson([
        { id: 'rust:src/one.rs::heavy', sites: { windows: [anchor, missing].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)) } },
      ]);
      expectEqual(fixture.run(), 1, 'the unobserved anchor is a STALE');
      const lines = findingLines(fixture, 'STALE');
      if (lines.length !== 1 || !lines[0].includes(`#${missing}`)) {
        throw new Error(`expected one STALE for ${missing}, got ${JSON.stringify(lines)}`);
      }
    }],
    ['case 6: a windows-only entry ignored on linux is not a finding', () => {
      const fixture = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: {
          'clippy.toml': PINNED_CLIPPY_TOML,
          'cognitive-complexity.baseline.json': baselineJson([
            { id: 'rust:src/x.rs::heavy', sites: { windows: ['a'.repeat(12)] } },
          ]),
        },
      });
      expectEqual(fixture.run(['--platform', 'linux']), 0, 'an unobserved id with no linux array is ignored');
      expectEqual(fixture.state.stderr.length, 0, 'ignored means no output');
    }],
    ['case 7: the same entry evaluated on windows is a STALE', () => {
      const fixture = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: {
          'clippy.toml': PINNED_CLIPPY_TOML,
          'cognitive-complexity.baseline.json': baselineJson([
            { id: 'rust:src/x.rs::heavy', sites: { windows: ['a'.repeat(12)] } },
          ]),
        },
      });
      expectEqual(fixture.run(), 1, 'windows sees the entry');
      if (findingLines(fixture, 'STALE').length !== 1) throw new Error('expected one STALE');
    }],
    ['case 8: asymmetric platform arrays are representable', () => {
      const workspace = { 'src/eight.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML };
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/eight.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace,
      });
      const anchor = anchorFor('src/eight.rs', 1, 1, fixture.reader);
      workspace['cognitive-complexity.baseline.json'] = baselineJson([
        {
          id: 'rust:src/eight.rs::heavy',
          sites: { windows: ['1'.repeat(12), '2'.repeat(12)], linux: [anchor] },
        },
      ]);
      expectEqual(fixture.run(['--platform', 'linux']), 0, 'linux sees only its own array');
    }],
    ['case 9: a capture without a terminal build-finished record is CAPTURE', () => {
      const fixture = captureFixture({
        capture: jsonl(cognitiveRecord({ file: 'src/a.rs', line: 1, column: 1, slice: 'heavy' })),
        workspace: {},
      });
      expectEqual(fixture.run(), 1, 'a truncated capture fails');
      if (!stderrText(fixture).includes('CAPTURE')) {
        throw new Error(`expected CAPTURE, got ${stderrText(fixture)}`);
      }
      const followed = captureFixture({
        capture: jsonl(
          buildFinished(),
          cognitiveRecord({ file: 'src/a.rs', line: 1, column: 1, slice: 'heavy' }),
        ),
        workspace: { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(followed.run(['--report']), 1, 'a non-terminal build-finished fails');
      if (!stderrText(followed).includes('CAPTURE')) {
        throw new Error(`expected CAPTURE, got ${stderrText(followed)}`);
      }
    }],
    ['case 10: build-finished success:false is CAPTURE', () => {
      const fixture = captureFixture({ capture: jsonl(buildFinished(false)), workspace: {} });
      expectEqual(fixture.run(), 1, 'a contradictory outcome fails');
      if (!stderrText(fixture).includes('CAPTURE')) {
        throw new Error(`expected CAPTURE, got ${stderrText(fixture)}`);
      }
    }],
    ['case 11: an empty capture is CAPTURE, never clean', () => {
      const fixture = captureFixture({
        capture: '',
        workspace: {
          'clippy.toml': PINNED_CLIPPY_TOML,
          'cognitive-complexity.baseline.json': baselineJson([
            { id: 'rust:src/a.rs::heavy', sites: { windows: ['a'.repeat(12)] } },
          ]),
        },
      });
      expectEqual(fixture.run(), 1, 'an empty capture is never clean');
      if (!stderrText(fixture).includes('CAPTURE')) {
        throw new Error(`expected CAPTURE, got ${stderrText(fixture)}`);
      }
    }],
    ['case 12: a line starting with { that does not parse is CAPTURE', () => {
      const fixture = captureFixture({
        capture: `${jsonl(buildFinished())}{not json\n`,
        workspace: {},
      });
      expectEqual(fixture.run(), 1, 'an unparseable line fails');
      if (!stderrText(fixture).includes('CAPTURE')) {
        throw new Error(`expected CAPTURE, got ${stderrText(fixture)}`);
      }
    }],
    ['case 13: report mode never swallows an integrity error', () => {
      const fixture = captureFixture({
        capture: jsonl(cognitiveRecord({ file: 'src/a.rs', line: 1, column: 1, slice: 'heavy' })),
        workspace: {},
      });
      expectEqual(fixture.run(['--report']), 1, 'report mode still fails');
      if (!stderrText(fixture).includes('CAPTURE')) {
        throw new Error(`expected CAPTURE, got ${stderrText(fixture)}`);
      }
    }],
    ['case 14: a missing baseline without --report is CAPTURE', () => {
      const fixture = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: { 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(fixture.run(), 1, 'a missing baseline fails');
      if (!stderrText(fixture).includes('CAPTURE: cognitive-complexity.baseline.json not found')) {
        throw new Error(`expected the exact CAPTURE message, got ${stderrText(fixture)}`);
      }
    }],
    ['case 15: report mode with no baseline is an empty baseline and a notice', () => {
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/a.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace: { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
        env: { GITHUB_ACTIONS: 'true' },
      });
      if (fixture.run(['--report', '--emit', 'observed.json']) !== 0) {
        throw new Error('report mode must exit 0');
      }
      const lines = findingLines(fixture, 'NEW');
      if (lines.length !== 1 || !lines[0].includes('rust:src/a.rs::heavy#')) {
        throw new Error(`expected the observed id as one NEW, got ${JSON.stringify(lines)}`);
      }
      if (!lines.every((line) => line.startsWith('::notice::'))) {
        throw new Error('every NEW must be a notice');
      }
      if (stderrText(fixture).includes('::error::')) {
        throw new Error('report mode emitted an error annotation');
      }
      if (stderrText(fixture).includes('Reduce the function')) {
        throw new Error('report mode printed the remedial line');
      }
      const emission = fixture.state.writes.get('observed.json');
      if (emission === undefined) throw new Error('the emission was not written');
      const parsed = JSON.parse(emission);
      if (
        !Array.isArray(parsed.entries)
        || parsed.entries.length !== 1
        || parsed.entries[0].id !== 'rust:src/a.rs::heavy'
      ) {
        throw new Error(`unexpected emission ${emission}`);
      }
    }],
    ['case 16: a message with a denominator other than 25 is THRESHOLD', () => {
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({
            file: 'src/a.rs',
            line: 1,
            column: 1,
            slice: 'heavy',
            message: 'the function has a cognitive complexity of (31/40)',
          }),
          buildFinished(),
        ),
        workspace: {
          'src/a.rs': HEAVY_SOURCE,
          'clippy.toml': PINNED_CLIPPY_TOML,
          'cognitive-complexity.baseline.json': baselineJson([]),
        },
      });
      expectEqual(fixture.run(), 1, 'the wrong denominator fails');
      if (!stderrText(fixture).includes('THRESHOLD')) {
        throw new Error(`expected THRESHOLD, got ${stderrText(fixture)}`);
      }
    }],
    ['case 17: a clippy.toml with another threshold is CONFIG', () => {
      const fixture = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: { 'clippy.toml': 'cognitive-complexity-threshold = 40\n' },
      });
      expectEqual(fixture.run(), 1, 'a moved threshold fails');
      if (!stderrText(fixture).includes('CONFIG')) {
        throw new Error(`expected CONFIG, got ${stderrText(fixture)}`);
      }
    }],
    ['case 18: CLIPPY_CONF_DIR set is CONFIG', () => {
      const fixture = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: {
          'clippy.toml': PINNED_CLIPPY_TOML,
          'cognitive-complexity.baseline.json': baselineJson([]),
        },
        env: { CLIPPY_CONF_DIR: 'docs' },
      });
      expectEqual(fixture.run(), 1, 'a config directory fails');
      if (!stderrText(fixture).includes('CONFIG')) {
        throw new Error(`expected CONFIG, got ${stderrText(fixture)}`);
      }
    }],
    ['case 19: RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS are both checked', () => {
      const first = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: {},
        env: { RUSTFLAGS: '--cap-lints allow' },
      });
      expectEqual(first.run(), 1, 'RUSTFLAGS cap-lints is CONFIG');
      if (!stderrText(first).includes('CONFIG')) throw new Error('expected CONFIG for RUSTFLAGS');
      const second = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: {},
        env: { CARGO_ENCODED_RUSTFLAGS: '-A\u001fcognitive' },
      });
      expectEqual(second.run(), 1, 'CARGO_ENCODED_RUSTFLAGS cognitive is CONFIG');
      if (!stderrText(second).includes('CONFIG')) {
        throw new Error('expected CONFIG for CARGO_ENCODED_RUSTFLAGS');
      }
    }],
    ['case 20: a rustc version differing from the baseline toolchain is CONFIG', () => {
      const fixture = captureFixture({
        capture: jsonl(buildFinished()),
        workspace: {
          'clippy.toml': PINNED_CLIPPY_TOML,
          'cognitive-complexity.baseline.json': baselineJson([]),
        },
      });
      expectEqual(fixture.run(['--rustc-version', '9.9.9']), 1, 'a toolchain mismatch fails');
      if (!stderrText(fixture).includes('CONFIG')) {
        throw new Error(`expected CONFIG, got ${stderrText(fixture)}`);
      }
    }],
    ['case 21: a failed clippy returns its code and the ratchet is not consulted', () => {
      const rendered = 'error[E0308]: mismatched types\n  --> src/a.rs:1:6\n';
      const fixture = captureFixture({
        capture: jsonl(
          messageRecord({
            file: 'src/a.rs',
            line: 1,
            column: 6,
            slice: 'heavy',
            message: 'mismatched types',
            code: 'E0308',
            rendered,
          }),
        ),
        workspace: {},
      });
      expectEqual(
        fixture.run(['--clippy-exit', '101', '--emit', 'never.json']),
        101,
        'clippy code is returned',
      );
      const renderedIndex = fixture.state.stderr.findIndex((line) => line.includes('error[E0308]'));
      const gateIndex = fixture.state.stderr.findIndex((line) =>
        line.includes('cognitive gate not evaluated: clippy exited 101'));
      if (renderedIndex === -1 || gateIndex === -1 || renderedIndex >= gateIndex) {
        throw new Error(
          `expected the rendered diagnostic first, got ${JSON.stringify(fixture.state.stderr)}`,
        );
      }
      if (findingLines(fixture, 'NEW').length !== 0 || findingLines(fixture, 'STALE').length !== 0) {
        throw new Error('the ratchet was consulted on a failed clippy run');
      }
      if (fixture.state.writes.size !== 0) throw new Error('a failed clippy run wrote an emission');
    }],
    ['case 22: every baseline schema violation is CONFIG', () => {
      const id = 'rust:src/a.rs::heavy';
      const valid = 'a'.repeat(12);
      const variants = [
        [
          'duplicate id',
          baselineJson([
            { id, sites: { windows: [valid] } },
            { id, sites: { windows: [otherAnchor(valid)] } },
          ]),
        ],
        ['sites not an object', baselineJson([{ id, sites: [valid] }])],
        ['empty anchor array', baselineJson([{ id, sites: { windows: [] } }])],
        ['anchor not 12 lowercase hex', baselineJson([{ id, sites: { windows: ['A'.repeat(12)] } }])],
        [
          'unsorted array',
          baselineJson([{ id, sites: { windows: ['f'.repeat(12), '0'.repeat(12)] } }]),
        ],
        ['unknown platform key', baselineJson([{ id, sites: { freebsd: [valid] } }])],
        ['version 2', baselineJson([], { version: 2 })],
      ];
      for (const [label, text] of variants) {
        const fixture = captureFixture({
          capture: jsonl(buildFinished()),
          workspace: { 'clippy.toml': PINNED_CLIPPY_TOML, 'cognitive-complexity.baseline.json': text },
        });
        expectEqual(fixture.run(), 1, `${label} is CONFIG`);
        if (!stderrText(fixture).includes('CONFIG')) {
          throw new Error(`expected CONFIG for ${label}, got ${stderrText(fixture)}`);
        }
      }
    }],
    ['case 23: --emit writes nothing before the comparison gate has passed', () => {
      const unparseable = captureFixture({
        capture: `${jsonl(buildFinished())}{not json\n`,
        workspace: {},
      });
      expectEqual(unparseable.run(['--emit', 'never.json']), 1, 'an unparseable capture fails');
      if (unparseable.state.writes.size !== 0) {
        throw new Error('an unparseable capture wrote an emission');
      }
      const threshold = captureFixture({
        capture: jsonl(
          cognitiveRecord({
            file: 'src/a.rs',
            line: 1,
            column: 1,
            slice: 'heavy',
            message: 'the function has a cognitive complexity of (31/40)',
          }),
          buildFinished(),
        ),
        workspace: { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
        env: { GITHUB_ACTIONS: 'true' },
      });
      expectEqual(threshold.run(['--report', '--emit', 'never.json']), 1, 'a threshold failure fails');
      if (threshold.state.writes.size !== 0) {
        throw new Error('a threshold failure wrote an emission');
      }
    }],
    ['case 24: a backslash and a forward-slash path are one id', () => {
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src-tauri\\src\\a.rs', line: 1, column: 1, slice: 'heavy' }),
          cognitiveRecord({ file: 'src-tauri/src/a.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace: { 'src-tauri/src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(fixture.run(['--report']), 0, 'report mode returns 0');
      const lines = findingLines(fixture, 'NEW');
      if (lines.length !== 1 || !lines[0].includes('rust:src-tauri/src/a.rs::heavy#')) {
        throw new Error(`expected one id, got ${JSON.stringify(lines)}`);
      }
    }],
    ['case 25: an absolute path inside the root is relativised, outside it is CAPTURE', () => {
      const inside = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'C:\\ws\\src\\a.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace: { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(inside.run(['--report']), 0, 'report mode returns 0');
      const lines = findingLines(inside, 'NEW');
      if (lines.length !== 1 || !lines[0].includes('rust:src/a.rs::heavy#')) {
        throw new Error(`expected the relativised id, got ${JSON.stringify(lines)}`);
      }
      const outside = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'C:/other/src/a.rs', line: 1, column: 1, slice: 'heavy' }),
          buildFinished(),
        ),
        workspace: { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(outside.run(), 1, 'an escaping path is CAPTURE');
      if (!stderrText(outside).includes('CAPTURE')) {
        throw new Error(`expected CAPTURE, got ${stderrText(outside)}`);
      }
    }],
    ['case 26: two records with the identical primary span are one site', () => {
      const record = cognitiveRecord({ file: 'src/a.rs', line: 1, column: 1, slice: 'heavy' });
      const fixture = captureFixture({
        capture: jsonl(record, record, buildFinished()),
        workspace: { 'src/a.rs': HEAVY_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(fixture.run(['--report']), 0, 'report mode returns 0');
      if (findingLines(fixture, 'NEW').length !== 1) {
        throw new Error('the lib / lib-test pair must collapse to one site');
      }
    }],
    ['case 27: the same id on two lines is two sites', () => {
      const first = sourceSite(TWO_SITES_SOURCE, 'm(&self)');
      const second = sourceSite(TWO_SITES_SOURCE, 'm(&self, x: u32)');
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/two.rs', line: first.line, column: first.column, slice: 'm' }),
          cognitiveRecord({ file: 'src/two.rs', line: second.line, column: second.column, slice: 'm' }),
          buildFinished(),
        ),
        workspace: { 'src/two.rs': TWO_SITES_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(fixture.run(['--report']), 0, 'report mode returns 0');
      const lines = findingLines(fixture, 'NEW');
      if (lines.length !== 2 || !lines.every((line) => line.includes('::impl:A::m#'))) {
        throw new Error(`expected two sites of one id, got ${JSON.stringify(lines)}`);
      }
    }],
    ['case 28: the two closure slice shapes become one id with two sites', () => {
      const source = 'fn f() {\n    let a = |x: u32| { x };\n    let b = | { y };\n}\n';
      const first = sourceSite(source, '|x: u32|');
      const second = sourceSite(source, '|', 3);
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({
            file: 'src/closures.rs',
            line: first.line,
            column: first.column,
            slice: '|x: u32|',
          }),
          cognitiveRecord({ file: 'src/closures.rs', line: second.line, column: second.column, slice: '|' }),
          buildFinished(),
        ),
        workspace: { 'src/closures.rs': source, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(fixture.run(['--report']), 0, 'report mode returns 0');
      const lines = findingLines(fixture, 'NEW');
      const ids = new Set(lines.map((line) => line.slice(line.indexOf(': ') + 2).split('#')[0]));
      if (lines.length !== 2 || ids.size !== 1 || ![...ids][0].endsWith('::fn:f::{closure}')) {
        throw new Error(`expected one closure id with two sites, got ${JSON.stringify(lines)}`);
      }
    }],
    ['case 70: two sites sharing a line are separated by their columns', () => {
      const source =
        'fn alpha() {} fn beta() {}\nfn f() { let a = |x| { x }; let b = |y| { y }; }\n';
      const alpha = sourceSite(source, 'alpha');
      const beta = sourceSite(source, 'beta');
      const closureX = sourceSite(source, '|x|');
      const closureY = sourceSite(source, '|y|');
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/line.rs', line: alpha.line, column: alpha.column, slice: 'alpha' }),
          cognitiveRecord({ file: 'src/line.rs', line: alpha.line, column: alpha.column, slice: 'alpha' }),
          cognitiveRecord({ file: 'src/line.rs', line: beta.line, column: beta.column, slice: 'beta' }),
          cognitiveRecord({ file: 'src/line.rs', line: beta.line, column: beta.column, slice: 'beta' }),
          cognitiveRecord({
            file: 'src/line.rs',
            line: closureX.line,
            column: closureX.column,
            slice: '|x|',
          }),
          cognitiveRecord({
            file: 'src/line.rs',
            line: closureX.line,
            column: closureX.column,
            slice: '|x|',
          }),
          cognitiveRecord({
            file: 'src/line.rs',
            line: closureY.line,
            column: closureY.column,
            slice: '|y|',
          }),
          cognitiveRecord({
            file: 'src/line.rs',
            line: closureY.line,
            column: closureY.column,
            slice: '|y|',
          }),
          buildFinished(),
        ),
        workspace: { 'src/line.rs': source, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      expectEqual(fixture.run(['--report']), 0, 'report mode returns 0');
      const lines = findingLines(fixture, 'NEW');
      const alphaLines = lines.filter((line) => line.includes('::alpha#'));
      const betaLines = lines.filter((line) => line.includes('::beta#'));
      if (alphaLines.length !== 1 || betaLines.length !== 1) {
        throw new Error(`expected one site for each same-line function, got ${JSON.stringify(lines)}`);
      }
      const closures = lines.filter((line) => line.includes('::fn:f::{closure}#'));
      if (closures.length !== 2) {
        throw new Error(`expected two sites for the same-line closures, got ${JSON.stringify(lines)}`);
      }
    }],
    ['case 75: an equal-count anchor replacement is one NEW and one STALE', () => {
      const first = sourceSite(TWO_SITES_SOURCE, 'm(&self)');
      const second = sourceSite(TWO_SITES_SOURCE, 'm(&self, x: u32)');
      const workspace = { 'src/rc1.rs': TWO_SITES_SOURCE, 'clippy.toml': PINNED_CLIPPY_TOML };
      const fixture = captureFixture({
        capture: jsonl(
          cognitiveRecord({ file: 'src/rc1.rs', line: first.line, column: first.column, slice: 'm' }),
          cognitiveRecord({ file: 'src/rc1.rs', line: second.line, column: second.column, slice: 'm' }),
          buildFinished(),
        ),
        workspace,
      });
      const observed = anchorFor('src/rc1.rs', first.line, first.column, fixture.reader);
      const replacement = anchorFor('src/rc1.rs', second.line, second.column, fixture.reader);
      const missing = otherAnchor(observed, replacement);
      workspace['cognitive-complexity.baseline.json'] = baselineJson([
        { id: 'rust:src/rc1.rs::impl:A::m', sites: { windows: [observed, missing].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)) } },
      ]);
      expectEqual(fixture.run(), 1, 'R-C1 is refused');
      const newLines = findingLines(fixture, 'NEW');
      const staleLines = findingLines(fixture, 'STALE');
      if (newLines.length !== 1 || !newLines[0].includes(`#${replacement}`)) {
        throw new Error(`expected one NEW for ${replacement}, got ${JSON.stringify(newLines)}`);
      }
      if (staleLines.length !== 1 || !staleLines[0].includes(`#${missing}`)) {
        throw new Error(`expected one STALE for ${missing}, got ${JSON.stringify(staleLines)}`);
      }
    }],
    ['case 76: identical anchors match by multiplicity', () => {
      const source = 'fn f() {\n    let a = |x: u32| { x };\n    let b = |x: u32| { x };\n}\n';
      const first = sourceSite(source, '|x: u32|', 1);
      const second = sourceSite(source, '|x: u32|', 2);
      const captureText = jsonl(
        cognitiveRecord({ file: 'src/dup.rs', line: first.line, column: first.column, slice: '|x: u32|' }),
        cognitiveRecord({ file: 'src/dup.rs', line: second.line, column: second.column, slice: '|x: u32|' }),
        buildFinished(),
      );
      const reader = (file) => {
        if (file !== 'src/dup.rs') throw new Error(`unexpected workspace read ${file}`);
        return source;
      };
      const anchor = anchorFor('src/dup.rs', first.line, first.column, reader);
      const matches = captureFixture({
        capture: captureText,
        workspace: { 'src/dup.rs': source, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      matches.workspace['cognitive-complexity.baseline.json'] = baselineJson([
        {
          id: 'rust:src/dup.rs::fn:f::{closure}',
          sites: { windows: [anchor, anchor] },
        },
      ]);
      expectEqual(matches.run(), 0, 'R-C1 residual passes with multiplicity');
      const replacement = otherAnchor(anchor);
      const changed = captureFixture({
        capture: captureText,
        workspace: { 'src/dup.rs': source, 'clippy.toml': PINNED_CLIPPY_TOML },
      });
      changed.workspace['cognitive-complexity.baseline.json'] = baselineJson([
        {
          id: 'rust:src/dup.rs::fn:f::{closure}',
          sites: { windows: [anchor, replacement].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)) },
        },
      ]);
      expectEqual(changed.run(), 1, 'a changed second site is refused');
      if (findingLines(changed, 'NEW').length !== 1 || findingLines(changed, 'STALE').length !== 1) {
        throw new Error(`expected one NEW and one STALE, got ${JSON.stringify(changed.state.stderr)}`);
      }
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
