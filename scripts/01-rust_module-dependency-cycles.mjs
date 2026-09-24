#!/usr/bin/env node
/*
 * rust-module-dependency-cycles
 *
 * Single-file, zero-dependency Node.js ESM analyzer that detects dependency
 * cycles in a Rust codebase at module and function granularity.
 *
 * A dependency cycle is a static structure: node A references node B and B
 * references back to A, directly or through a chain. Recursion is a runtime
 * property of a function invoking itself; it is NOT a dependency cycle and is
 * reported separately under `recursion`. Each tangled group is a strongly
 * connected component (SCC), and that is the term this tool reports in.
 *
 * Extraction is regex and character-scanner based. There is no Rust parser,
 * no toolchain invocation, and no requirement that the target project compile.
 * That buys portability at a real cost in visibility, so every pass that
 * cannot see something increments a counter and the result is always printed.
 * Read `blindSpots` before trusting a zero.
 *
 * Node baseline: >= 18. Run as `node 01-rust_module-dependency-cycles.mjs <path>`.
 */

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import process from 'node:process';
import { createHash } from 'node:crypto';

/* ------------------------------------------------------------------ *
 * 1. Constants
 * ------------------------------------------------------------------ */

const TOOL_NAME = 'rust-module-dependency-cycles';
const TOOL_VERSION = '1.1.1';
const SCHEMA_VERSION = 1;
const BASELINE_SCHEMA_VERSION = 1;
const BASELINE_KIND = 'rust-cycles-baseline';
// The graph file is a separate contract with a separate consumer, so it carries
// its own version and kind, exactly as the baseline already does. `kind` names
// the contract and `language` names the producer, which is what lets a future
// non-Rust extractor emit a file the same consumer can read.
const GRAPH_SCHEMA_VERSION = 1;
const GRAPH_KIND = 'dependency-graph';
const GRAPH_LANGUAGE = 'rust';

// Every sort in this file orders strings by UTF-16 code unit, the order the
// default Array.prototype.sort already used; naming it keeps the output stable
// and explicit. localeCompare is not used: it would reorder case and punctuation.
function compareCodeUnits(a, b) {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

/** Always applied, matched on any path segment. */
const DEFAULT_EXCLUDES = ['target', '.git', 'node_modules', '.direnv', '.cargo', 'vendor'];

const LEVELS = ['module', 'function', 'all'];
const FUNCTION_SCOPES = ['cross-module', 'all'];
const FAIL_ON = ['none', 'module', 'any'];

const BLIND_SPOT_NOTES = [
  'Function-level results are a lower bound. Zero function cycles is not proof of absence.',
  'Method calls (x.foo()), trait dispatch and generic dispatch are not resolvable without type information.',
  'Calls inside macro invocations are not analyzed.',
];

const LIMITATIONS = [
  'No type resolution: x.foo(), trait dispatch and generic dispatch are invisible at function level.',
  'Calls inside macro invocations are not analyzed.',
  'macro_rules! bodies are not analyzed at all.',
  'mod foo; nested inside an inline mod block is not resolved.',
  '#[path] on an inline mod block is not supported.',
  'Explicit [[bin]] / [[test]] manifest sections with custom path values are not read; only conventional target locations are discovered.',
  '#[cfg] is not evaluated. Non-test gated code is included and tagged; test-gated code is excluded by default.',
  'Function-level output is a lower bound in every mode.',
];

/** Item modifiers that may precede the keyword that names the item kind. */
const MODIFIERS = new Set(['pub', 'async', 'unsafe', 'const', 'extern', 'default', 'auto', 'static']);

/** Words that can sit immediately before a `(` without being a call. */
const NON_CALL_WORDS = new Set([
  'if', 'while', 'for', 'match', 'return', 'let', 'else', 'in', 'as', 'break',
  'continue', 'fn', 'loop', 'move', 'mut', 'ref', 'where', 'impl', 'dyn', 'yield',
  'await', 'unsafe', 'const', 'static', 'type', 'struct', 'enum', 'union', 'trait',
  'use', 'mod', 'pub', 'crate', 'super', 'self', 'Self', 'true', 'false', 'and', 'or',
]);

const CFG_TEST_RE = /\bcfg\s*\(\s*test\s*\)/;
const CFG_ANY_RE = /\bcfg\s*\(/;

class UsageError extends Error {}
class AnalysisError extends Error {}

/* ------------------------------------------------------------------ *
 * 2. CLI surface
 * ------------------------------------------------------------------ */

function printHelp() {
  const lines = [];
  lines.push(`${TOOL_NAME} ${TOOL_VERSION}`);
  lines.push('');
  lines.push('Detect Rust dependency cycles at module and function granularity.');
  lines.push('Zero dependencies, no build step, no Rust toolchain, target need not compile.');
  lines.push('');
  lines.push('USAGE');
  lines.push('  node 01-rust_module-dependency-cycles.mjs <path> [options]');
  lines.push('  node 01-rust_module-dependency-cycles.mjs --self-test');
  lines.push('  node 01-rust_module-dependency-cycles.mjs --help | --version');
  lines.push('');
  lines.push('  <path> is a directory or a single .rs file. Required except with');
  lines.push('  --self-test, --help and --version. A second positional is a usage error.');
  lines.push('');
  lines.push('OPTIONS');
  lines.push('  --json                     emit JSON on stdout and nothing else on stdout');
  lines.push('  --level <v>                module | function | all            (default: all)');
  lines.push('  --function-scope <v>       cross-module | all        (default: cross-module)');
  lines.push('  --fail-on <v>              none | module | any           (default: module)');
  lines.push('  --baseline <file>          compare against a baseline; only new cycles gate');
  lines.push('  --write-baseline <file>    write the current cycles as a baseline and exit');
  lines.push('  --emit-graph <file>        write the complete dependency graph to that file');
  lines.push('  --include-tests            include #[cfg(test)] items, mod tests blocks and');
  lines.push('                             test/bench/example crate targets');
  lines.push('  --exclude <pattern>        skip matching paths (repeatable)');
  lines.push('  --quiet                    suppress the human summary; exit code only');
  lines.push('  --self-test                run the embedded fixture suite');
  lines.push('  -h, --help                 print this help and exit 0');
  lines.push('  -V, --version              print the version and exit 0');
  lines.push('');
  lines.push('  Both --flag value and --flag=value are accepted.');
  lines.push('');
  lines.push('EXIT CODES');
  lines.push('  0  analysis succeeded and nothing gates; also --help, --version,');
  lines.push('     and a successful --write-baseline');
  lines.push('  1  analysis succeeded and gating cycles were found');
  lines.push('  2  usage error (unknown flag, bad value, bad or incompatible baseline file,');
  lines.push('     --emit-graph together with --write-baseline)');
  lines.push('  3  analysis error (path missing, no .rs files, no crate root, read error,');
  lines.push('     or the graph file cannot be written or fails its integrity check,');
  lines.push('     or any unexpected exception)');
  lines.push('  4  --self-test had at least one failing assertion');
  lines.push('');
  lines.push('  1 and 3 are deliberately distinct: "there are cycles" and "I could not');
  lines.push('  look properly" must never be confused by a CI gate.');
  lines.push('');
  lines.push('BASELINE');
  lines.push('  There is no implicit baseline path and no auto-discovery. A baseline that');
  lines.push('  is picked up silently is a way to hide cycles by accident.');
  lines.push('    node 01-rust_module-dependency-cycles.mjs . --write-baseline .rust-cycles-baseline.json');
  lines.push('    node 01-rust_module-dependency-cycles.mjs . --baseline .rust-cycles-baseline.json');
  lines.push('');
  lines.push('GRAPH');
  lines.push('  There is no implicit graph path and no auto-discovery, for the same reason');
  lines.push('  as the baseline. The file carries every node and every edge, one record per');
  lines.push('  reference site, never deduplicated. It never goes to stdout.');
  lines.push('    node 01-rust_module-dependency-cycles.mjs . --emit-graph graph.json');
  lines.push('  --emit-graph and --write-baseline are mutually exclusive (exit 2). No graph');
  lines.push('  is written when the analysis produced error-level diagnostics.');
  lines.push('');
  lines.push('LIMITATIONS (what this tool structurally cannot see)');
  for (let i = 0; i < LIMITATIONS.length; i++) {
    lines.push(`  ${i + 1}. ${LIMITATIONS[i]}`);
  }
  lines.push('');
  lines.push('  The BLIND SPOTS block is printed always, including when every count is');
  lines.push('  zero and zero cycles were found. Only --quiet suppresses it.');
  lines.push('');
  return lines.join('\n') + '\n';
}

function parseArgs(argv) {
  const opts = {
    pathArg: null,
    json: false,
    level: 'all',
    functionScope: 'cross-module',
    failOn: 'module',
    baseline: null,
    writeBaseline: null,
    emitGraph: null,
    includeTests: false,
    excludes: [],
    quiet: false,
    selfTest: false,
    help: false,
    version: false,
  };
  const positionals = [];

  const needValue = (token, inlineValue, iter) => {
    // `--flag=` is a missing value, not an empty one. Accepting the empty string
    // makes `--write-baseline=` a silent no-op that reads as a recorded baseline,
    // and lets `--baseline= --write-baseline=x` slip past the exclusivity check.
    const value = inlineValue !== null ? inlineValue : iter.next();
    if (value === undefined || value === '') throw new UsageError(`missing value for ${token}`);
    return value;
  };

  let i = 0;
  const iter = { next: () => (i < argv.length ? argv[i++] : undefined) };

  for (;;) {
    const token = iter.next();
    if (token === undefined) break;
    if (token === '--') {
      for (;;) {
        const rest = iter.next();
        if (rest === undefined) break;
        positionals.push(rest);
      }
      break;
    }
    if (!token.startsWith('-') || token === '-') {
      positionals.push(token);
      continue;
    }
    const eq = token.indexOf('=');
    const name = eq >= 0 ? token.slice(0, eq) : token;
    const inline = eq >= 0 ? token.slice(eq + 1) : null;

    switch (name) {
      case '--json':
        opts.json = true;
        break;
      case '--include-tests':
        opts.includeTests = true;
        break;
      case '--quiet':
        opts.quiet = true;
        break;
      case '--self-test':
        opts.selfTest = true;
        break;
      case '-h':
      case '--help':
        opts.help = true;
        break;
      case '-V':
      case '--version':
        opts.version = true;
        break;
      case '--level': {
        const v = needValue(name, inline, iter);
        if (!LEVELS.includes(v)) throw new UsageError(`invalid value for --level: ${v} (expected ${LEVELS.join(' | ')})`);
        opts.level = v;
        break;
      }
      case '--function-scope': {
        const v = needValue(name, inline, iter);
        if (!FUNCTION_SCOPES.includes(v)) {
          throw new UsageError(`invalid value for --function-scope: ${v} (expected ${FUNCTION_SCOPES.join(' | ')})`);
        }
        opts.functionScope = v;
        break;
      }
      case '--fail-on': {
        const v = needValue(name, inline, iter);
        if (!FAIL_ON.includes(v)) throw new UsageError(`invalid value for --fail-on: ${v} (expected ${FAIL_ON.join(' | ')})`);
        opts.failOn = v;
        break;
      }
      case '--baseline':
        opts.baseline = needValue(name, inline, iter);
        break;
      case '--write-baseline':
        opts.writeBaseline = needValue(name, inline, iter);
        break;
      case '--emit-graph':
        opts.emitGraph = needValue(name, inline, iter);
        break;
      case '--exclude':
        opts.excludes.push(needValue(name, inline, iter));
        break;
      default:
        throw new UsageError(`unknown flag: ${name}`);
    }
  }

  if (opts.help || opts.version || opts.selfTest) return opts;

  if (opts.baseline && opts.writeBaseline) {
    throw new UsageError('--baseline and --write-baseline are mutually exclusive');
  }
  // --write-baseline is a run mode, not a flag: it renders no report and exits 0
  // even with cycles, because recording is not a verdict. --emit-graph is a side
  // output on a run whose exit code IS a verdict. Allowing both would leave one
  // exit code meaning two things.
  if (opts.emitGraph && opts.writeBaseline) {
    throw new UsageError('--emit-graph and --write-baseline are mutually exclusive');
  }
  if (positionals.length === 0) throw new UsageError('missing <path>');
  if (positionals.length > 1) throw new UsageError(`unexpected extra positional: ${positionals[1]}`);
  opts.pathArg = positionals[0];
  return opts;
}

/* ------------------------------------------------------------------ *
 * 3. Masking pass
 * ------------------------------------------------------------------ */

function isIdentStart(c) {
  return c !== undefined && ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c === '_');
}

function isIdentChar(c) {
  return c !== undefined && ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c === '_');
}

/**
 * Replace every comment body, string body, char literal and their delimiters
 * with U+0020, preserving newlines in place and the exact input length.
 *
 * Equal length plus preserved newlines is mandatory: byte offsets in the
 * masked source map 1:1 to the original, so file:line:column stays exact and
 * edge snippets can be sliced from the original at masked offsets. After this
 * pass every remaining brace, paren, bracket and semicolon is real code, which
 * is what makes delimiter matching trivially correct everywhere else.
 */
function stripCommentsAndStrings(src) {
  const n = src.length;
  const out = src.split('');
  const blank = (from, to) => {
    const end = Math.min(to, n);
    for (let k = from; k < end; k++) {
      if (out[k] !== '\n') out[k] = ' ';
    }
  };

  let i = 0;
  while (i < n) {
    const c = src[i];

    // Line comment, including /// and //!.
    if (c === '/' && src[i + 1] === '/') {
      let j = i;
      while (j < n && src[j] !== '\n') j++;
      blank(i, j);
      i = j;
      continue;
    }

    // Block comment. Rust permits nesting, so track depth.
    if (c === '/' && src[i + 1] === '*') {
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (src[j] === '/' && src[j + 1] === '*') {
          depth++;
          j += 2;
        } else if (src[j] === '*' && src[j + 1] === '/') {
          depth--;
          j += 2;
        } else {
          j++;
        }
      }
      blank(i, j);
      i = j;
      continue;
    }

    if (isIdentStart(c)) {
      let j = i;
      while (j < n && isIdentChar(src[j])) j++;
      const word = src.slice(i, j);

      if (word === 'r' || word === 'br' || word === 'cr') {
        let k = j;
        while (k < n && src[k] === '#') k++;
        const hashes = k - j;
        if (src[k] === '"') {
          // Raw string: ends at the first '"' followed by exactly `hashes` '#'.
          let m = k + 1;
          let endIdx = n;
          while (m < n) {
            if (src[m] === '"') {
              let h = 0;
              while (h < hashes && src[m + 1 + h] === '#') h++;
              if (h === hashes) {
                endIdx = m + 1 + hashes;
                break;
              }
            }
            m++;
          }
          blank(i, endIdx);
          i = endIdx;
          continue;
        }
        if (hashes > 0 && word === 'r') {
          // Raw identifier such as r#fn or r#type. Not a string; emit unchanged.
          i = k;
          continue;
        }
      }
      // Plain identifier, or a b"/c"/b' prefix whose delimiter is handled next.
      i = j;
      continue;
    }

    if (c === '"') {
      let j = i + 1;
      while (j < n) {
        if (src[j] === '\\') {
          j += 2;
          continue;
        }
        if (src[j] === '"') {
          j++;
          break;
        }
        j++;
      }
      blank(i, j);
      i = j;
      continue;
    }

    if (c === "'") {
      // Char literal only when the following text is an optional backslash
      // escape, one character, then a closing quote. Everything else is a
      // lifetime and must be left alone, or an unterminated "char literal"
      // swallows the rest of the file.
      let end = -1;
      if (src[i + 1] === '\\') {
        if (src[i + 3] === "'") end = i + 4;
      } else if (src[i + 2] === "'") {
        end = i + 3;
      }
      if (end > 0) {
        blank(i, end);
        i = end;
        continue;
      }
      i++;
      continue;
    }

    i++;
  }

  return out.join('');
}

/* ------------------------------------------------------------------ *
 * 4. Delimiter matching over masked source
 * ------------------------------------------------------------------ */

const CLOSERS = { '(': ')', '[': ']', '{': '}' };

function matchDelimiter(masked, openIndex) {
  const open = masked[openIndex];
  const close = CLOSERS[open];
  if (!close) return -1;
  let depth = 0;
  for (let i = openIndex, n = masked.length; i < n; i++) {
    const c = masked[i];
    if (c === open) {
      depth++;
    } else if (c === close) {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

function skipWs(s, i, to) {
  let j = i;
  while (j < to) {
    const c = s[j];
    if (c === ' ' || c === '\t' || c === '\n' || c === '\r') j++;
    else break;
  }
  return j;
}

/** Read an identifier, transparently accepting the raw form `r#name`. */
function readIdent(s, i, to) {
  let j = i;
  if (s[j] === 'r' && s[j + 1] === '#' && isIdentStart(s[j + 2])) j += 2;
  if (!isIdentStart(s[j]) || j >= to) return null;
  const start = j;
  while (j < to && isIdentChar(s[j])) j++;
  return { name: s.slice(start, j), start: i, end: j };
}

/* ------------------------------------------------------------------ *
 * 5. Line and column
 * ------------------------------------------------------------------ */

/** Prefix table of line start offsets. Built once per file. */
function buildLineIndex(src) {
  const starts = [0];
  for (let i = 0, n = src.length; i < n; i++) {
    if (src[i] === '\n') starts.push(i + 1);
  }
  return starts;
}

/**
 * Physical lines of a file: blanks and comments included, submodules excluded.
 * buildLineIndex records line START offsets, so a source ending in a newline
 * has one entry for a line that has no content and must not be counted.
 */
function lineCountOf(fc) {
  if (fc.src.length === 0) return 0;
  return fc.src.endsWith('\n') ? fc.lineIndex.length - 1 : fc.lineIndex.length;
}

/** 1-based line and column for `offset`, against a table from buildLineIndex. */
function lineColOf(lineIndex, offset) {
  let lo = 0;
  let hi = lineIndex.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (lineIndex[mid] <= offset) lo = mid;
    else hi = mid - 1;
  }
  return { line: lo + 1, column: offset - lineIndex[lo] + 1 };
}

/* ------------------------------------------------------------------ *
 * 6. Path handling
 * ------------------------------------------------------------------ */

function toPosix(p) {
  return p.replace(/\\/g, '/');
}

function relPath(root, abs) {
  let r = path.relative(root, abs);
  if (r === '' || r === '.') r = path.basename(abs);
  return toPosix(r);
}

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function compileExcludes(patterns) {
  return patterns.map((raw) => {
    const p = toPosix(String(raw)).replace(/^\.\//, '').replace(/\/+$/, '');
    if (!/[*?]/.test(p)) {
      return { kind: 'literal', raw: p, segments: p.split('/').filter(Boolean) };
    }
    let re = '';
    for (let i = 0; i < p.length; i++) {
      const c = p[i];
      if (c === '*') {
        if (p[i + 1] === '*') {
          re += '.*';
          i++;
        } else {
          re += '[^/]*';
        }
      } else if (c === '?') {
        re += '[^/]';
      } else {
        re += escapeRe(c);
      }
    }
    return { kind: 'glob', raw: p, re: new RegExp('^' + re + '$') };
  });
}

function isExcluded(relPosix, compiled) {
  for (const ex of compiled) {
    if (ex.kind === 'glob') {
      if (ex.re.test(relPosix)) return true;
      continue;
    }
    if (relPosix === ex.raw) return true;
    if (relPosix.startsWith(ex.raw + '/')) return true;
    if (ex.segments.length === 1 && relPosix.split('/').includes(ex.raw)) return true;
  }
  return false;
}

function hasDefaultExcludedSegment(relPosix) {
  const segments = relPosix.split('/');
  for (const seg of segments) {
    if (DEFAULT_EXCLUDES.includes(seg)) return true;
  }
  return false;
}

/* ------------------------------------------------------------------ *
 * 7. File walk
 * ------------------------------------------------------------------ */

/**
 * Walk `root`, collecting .rs files and Cargo.toml manifests in one pass.
 * Directory listings are sorted so two runs see files in the same order.
 */
function findRustFiles(root, compiledExcludes, ctx) {
  const rustFiles = [];
  const manifests = [];
  const stack = [root];

  while (stack.length > 0) {
    const dir = stack.pop();
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (err) {
      ctx.diagnostics.push({
        level: 'error',
        code: 'directory-read-error',
        message: `cannot read directory: ${err.message}`,
        file: relPath(root, dir),
      });
      continue;
    }
    entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));

    for (const entry of entries) {
      const abs = path.join(dir, entry.name);
      const rel = relPath(root, abs);
      if (hasDefaultExcludedSegment(rel)) continue;
      if (isExcluded(rel, compiledExcludes)) continue;

      if (entry.isSymbolicLink()) {
        // Not followed: a filesystem cycle is a different problem from the one
        // being solved here.
        ctx.diagnostics.push({
          level: 'warn',
          code: 'symlink-skipped',
          message: 'symlink not followed',
          file: rel,
        });
        continue;
      }
      if (entry.isDirectory()) {
        stack.push(abs);
        continue;
      }
      if (!entry.isFile()) continue;
      if (entry.name.endsWith('.rs')) rustFiles.push(abs);
      else if (entry.name === 'Cargo.toml') manifests.push(abs);
    }
  }

  rustFiles.sort(compareCodeUnits);
  manifests.sort(compareCodeUnits);
  return { rustFiles, manifests };
}

/* ------------------------------------------------------------------ *
 * 8. Minimal TOML reading
 * ------------------------------------------------------------------ */

function stripTomlComment(line) {
  let inString = false;
  let quote = '';
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (inString) {
      if (c === '\\' && quote === '"') i++;
      else if (c === quote) inString = false;
      continue;
    }
    if (c === '"' || c === "'") {
      inString = true;
      quote = c;
      continue;
    }
    if (c === '#') return line.slice(0, i);
  }
  return line;
}

function unquoteTomlKey(key) {
  if ((key.startsWith('"') && key.endsWith('"')) || (key.startsWith("'") && key.endsWith("'"))) {
    return key.slice(1, -1);
  }
  return key;
}

/**
 * Deliberately shallow. Only [package] name/edition, [lib] name/path and the
 * key names of dependency tables are needed. Unknown syntax is skipped and
 * this must never throw on a manifest it does not fully understand.
 */
function parseMinimalToml(text) {
  const tables = new Map();
  let current = '';
  tables.set('', new Map());

  for (const rawLine of text.split(/\r?\n/)) {
    const line = stripTomlComment(rawLine).trim();
    if (line === '') continue;

    let m = /^\[\[([^\]]+)\]\]\s*$/.exec(line);
    if (!m) m = /^\[([^\]]+)\]\s*$/.exec(line);
    if (m) {
      current = m[1].trim();
      if (!tables.has(current)) tables.set(current, new Map());
      continue;
    }

    m = /^([A-Za-z0-9_.-]+|"[^"]*"|'[^']*')\s*=\s*(.*)$/.exec(line);
    if (!m) continue;
    const key = unquoteTomlKey(m[1]);
    const rest = m[2].trim();
    const sv = /^"([^"]*)"$/.exec(rest) || /^'([^']*)'$/.exec(rest);
    tables.get(current).set(key, sv ? sv[1] : null);
  }

  return tables;
}

const DEP_TABLE_NAMES = ['dependencies', 'dev-dependencies', 'build-dependencies'];

/** Collect dependency crate names out of every dependency table shape. */
function collectDependencyNames(tables) {
  const names = new Set();
  for (const [tableName, keys] of tables) {
    if (tableName === '') continue;
    const segments = tableName.split('.').map(unquoteTomlKey);
    const idx = segments.findIndex((s) => DEP_TABLE_NAMES.includes(s));
    if (idx < 0) continue;
    if (idx === segments.length - 1) {
      for (const key of keys.keys()) names.add(key);
    } else {
      names.add(segments[idx + 1]);
    }
  }
  return names;
}

function identify(name) {
  return String(name).replace(/-/g, '_');
}

/* ------------------------------------------------------------------ *
 * 9. Crate target discovery
 * ------------------------------------------------------------------ */

function listRootFilesIn(dir) {
  // `tests/foo.rs` and `tests/foo/main.rs` shaped target roots.
  const found = [];
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return found;
  }
  entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  for (const entry of entries) {
    if (entry.isSymbolicLink()) continue;
    const abs = path.join(dir, entry.name);
    if (entry.isFile() && entry.name.endsWith('.rs')) {
      found.push({ stem: entry.name.slice(0, -3), file: abs });
    } else if (entry.isDirectory()) {
      const mainRs = path.join(abs, 'main.rs');
      if (isFile(mainRs)) found.push({ stem: entry.name, file: mainRs });
    }
  }
  return found;
}

function isFile(p) {
  try {
    return fs.statSync(p).isFile();
  } catch {
    return false;
  }
}

function isDirectory(p) {
  try {
    return fs.statSync(p).isDirectory();
  } catch {
    return false;
  }
}

function makeTarget(fields) {
  return {
    targetId: fields.targetId,
    kind: fields.kind,
    packageName: fields.packageName,
    pkgIdent: fields.pkgIdent,
    libName: fields.libName,
    manifestPath: fields.manifestPath,
    rootFile: fields.rootFile,
    sourceRoot: fields.sourceRoot,
    edition: fields.edition,
    externCrates: fields.externCrates,
    enabled: fields.enabled,
  };
}

/**
 * Manifest with no [package] contributes its [workspace.dependencies] keys to
 * the known-external set and yields no target. `[lib] name` wins over the
 * package name for the lib target identifier: a package called
 * `agentscommander-new` can expose a lib called `agentscommander_lib`, and
 * deriving the crate identifier from the package name would misresolve every
 * `use agentscommander_lib::...`.
 */
function discoverTargets(rootAbs, manifests, opts, ctx) {
  const workspaceDeps = new Set();
  const parsed = [];

  for (const manifestAbs of manifests) {
    let text;
    try {
      text = fs.readFileSync(manifestAbs, 'utf8');
    } catch (err) {
      ctx.diagnostics.push({
        level: 'error',
        code: 'file-read-error',
        message: `cannot read manifest: ${err.message}`,
        file: relPath(rootAbs, manifestAbs),
      });
      continue;
    }
    const tables = parseMinimalToml(text);
    for (const dep of collectDependencyNames(tables)) workspaceDeps.add(identify(dep));
    parsed.push({ manifestAbs, tables });
  }

  const targets = [];

  for (const { manifestAbs, tables } of parsed) {
    const pkg = tables.get('package');
    if (!pkg) continue;
    const packageName = pkg.get('name');
    if (!packageName) {
      ctx.diagnostics.push({
        level: 'warn',
        code: 'manifest-missing-package-name',
        message: '[package] has no name; no target derived from this manifest',
        file: relPath(rootAbs, manifestAbs),
      });
      continue;
    }
    // Cargo's own default when `edition` is absent is 2015, and bare-path
    // `use` resolution differs between 2015 and 2018+.
    const edition = pkg.get('edition') || '2015';
    const dir = path.dirname(manifestAbs);
    const manifestRel = relPath(rootAbs, manifestAbs);
    const pkgIdent = identify(packageName);

    const externCrates = new Set(workspaceDeps);
    for (const dep of collectDependencyNames(tables)) externCrates.add(identify(dep));

    const lib = tables.get('lib');
    const libName = lib && lib.get('name') ? lib.get('name') : pkgIdent;
    const libPath = lib && lib.get('path') ? path.resolve(dir, lib.get('path')) : path.join(dir, 'src', 'lib.rs');

    const common = {
      packageName,
      pkgIdent,
      libName,
      manifestPath: manifestRel,
      edition,
      externCrates,
    };

    if (isFile(libPath)) {
      targets.push(makeTarget({
        ...common,
        targetId: libName,
        kind: 'lib',
        rootFile: libPath,
        sourceRoot: sourceRootFor(dir, libPath),
        enabled: true,
      }));
    }

    const mainRs = path.join(dir, 'src', 'main.rs');
    if (isFile(mainRs)) {
      targets.push(makeTarget({
        ...common,
        targetId: `${pkgIdent}[bin:main]`,
        kind: 'bin',
        rootFile: mainRs,
        sourceRoot: sourceRootFor(dir, mainRs),
        enabled: true,
      }));
    }

    for (const { stem, file } of listRootFilesIn(path.join(dir, 'src', 'bin'))) {
      targets.push(makeTarget({
        ...common,
        targetId: `${pkgIdent}[bin:${stem}]`,
        kind: 'bin',
        rootFile: file,
        sourceRoot: sourceRootFor(dir, file),
        enabled: true,
      }));
    }

    // Integration test, bench and example targets are separate leaf crates.
    // They cannot participate in a cycle with the lib, and a test referencing
    // production code is not a coupling constraint on splitting the crate.
    for (const [subdir, kind] of [['tests', 'test'], ['benches', 'bench'], ['examples', 'example']]) {
      for (const { stem, file } of listRootFilesIn(path.join(dir, subdir))) {
        targets.push(makeTarget({
          ...common,
          targetId: `${pkgIdent}[${kind}:${stem}]`,
          kind,
          rootFile: file,
          sourceRoot: sourceRootFor(dir, file),
          enabled: opts.includeTests,
        }));
      }
    }
  }

  targets.sort((a, b) => (a.targetId < b.targetId ? -1 : a.targetId > b.targetId ? 1 : 0));
  return targets;
}

/**
 * The boundary a #[path] attribute may not escape. `src/` when the root file
 * lives under it, so `src/bin/x.rs` can still reach `../shared.rs`; otherwise
 * the directory holding the root file.
 */
function sourceRootFor(packageDir, rootFile) {
  const srcDir = path.join(packageDir, 'src');
  const rel = path.relative(srcDir, rootFile);
  if (rel && !rel.startsWith('..') && !path.isAbsolute(rel)) return srcDir;
  return path.dirname(rootFile);
}

function fallbackTarget(rootAbs) {
  const candidates = [
    path.join(rootAbs, 'lib.rs'),
    path.join(rootAbs, 'main.rs'),
    path.join(rootAbs, 'src', 'lib.rs'),
    path.join(rootAbs, 'src', 'main.rs'),
  ];
  for (const candidate of candidates) {
    if (isFile(candidate)) {
      return makeTarget({
        targetId: 'crate',
        kind: path.basename(candidate) === 'lib.rs' ? 'lib' : 'bin',
        packageName: null,
        pkgIdent: 'crate',
        libName: 'crate',
        manifestPath: null,
        rootFile: candidate,
        sourceRoot: path.dirname(candidate),
        edition: '2021',
        externCrates: new Set(),
        enabled: true,
      });
    }
  }
  return null;
}

/* ------------------------------------------------------------------ *
 * 10. Item extraction
 * ------------------------------------------------------------------ */

/** Where an item ends: the next `;` or the end of the next `{...}` block. */
function findItemBody(m, from, to) {
  let i = from;
  while (i < to) {
    const c = m[i];
    if (c === ';') return { kind: 'semi', end: i + 1, bodyStart: -1, bodyEnd: -1 };
    if (c === '{') {
      const e = matchDelimiter(m, i);
      const close = e < 0 || e > to ? to : e;
      return { kind: 'block', end: Math.min(close + 1, to), bodyStart: i + 1, bodyEnd: close };
    }
    if (c === '(' || c === '[') {
      const e = matchDelimiter(m, i);
      i = e < 0 || e > to ? i + 1 : e + 1;
      continue;
    }
    if (c === '}') return { kind: 'none', end: i, bodyStart: -1, bodyEnd: -1 };
    i++;
  }
  return { kind: 'none', end: to, bodyStart: -1, bodyEnd: -1 };
}

/**
 * Self type of an impl block, reduced to the last path segment with generic
 * arguments stripped: `impl<T> Foo<T> for Bar<T>` yields `Bar`.
 */
function parseImplSelfType(header) {
  let h = header.trim();

  if (h.startsWith('<')) {
    let depth = 0;
    let k = 0;
    for (; k < h.length; k++) {
      if (h[k] === '<') depth++;
      else if (h[k] === '>') {
        depth--;
        if (depth <= 0) {
          k++;
          break;
        }
      }
    }
    h = h.slice(k);
  }

  const whereIdx = h.search(/\bwhere\b/);
  if (whereIdx >= 0) h = h.slice(0, whereIdx);

  let depth = 0;
  let forIdx = -1;
  for (let k = 0; k < h.length; k++) {
    const ch = h[k];
    if (ch === '<' || ch === '(' || ch === '[') depth++;
    else if (ch === '>' || ch === ')' || ch === ']') depth = depth > 0 ? depth - 1 : 0;
    else if (depth === 0 && /\s/.test(ch) && h.startsWith('for', k + 1) && /[\s<]/.test(h[k + 4] || ' ')) {
      forIdx = k + 4;
    }
  }
  if (forIdx >= 0) h = h.slice(forIdx);

  h = h.trim();
  h = h.replace(/^(?:&\s*(?:'[A-Za-z_][A-Za-z0-9_]*\s*)?(?:mut\s+)?)+/, '');
  h = h.replace(/^\(\s*/, '').replace(/\s*\)\s*$/, '');
  const lt = h.indexOf('<');
  if (lt >= 0) h = h.slice(0, lt);
  h = h.trim();

  const segments = h.split('::').map((s) => s.trim()).filter(Boolean);
  const last = segments.length > 0 ? segments[segments.length - 1] : '';
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(last) ? last : null;
}

function newItems() {
  return {
    scopeDepthCapHits: 0,
    fileCfgTest: false,
    useDecls: [],
    modDecls: [],
    inlineMods: [],
    fns: [],
    typeDefs: [],
    macroRulesRanges: [],
    cfgTestRanges: [],
    cfgGatedRanges: [],
    constStaticRanges: [],
  };
}

/**
 * Walk a file with delimiter matching and record every structure the graphs
 * need. Attribute association is textual: an attribute run immediately
 * preceding an item, with only whitespace and already-masked comments in
 * between, belongs to that item.
 */
function extractItems(src, masked) {
  const st = { src, masked, out: newItems() };
  scanScope(st, 0, masked.length, { kind: 'file', modStack: [], ownerType: null }, 0);
  st.out.useDecls.sort((a, b) => a.start - b.start);
  st.out.modDecls.sort((a, b) => a.start - b.start);
  st.out.inlineMods.sort((a, b) => a.start - b.start);
  st.out.fns.sort((a, b) => a.start - b.start);
  return st.out;
}

/**
 * Recursion bound for nested scopes. The cap keeps a pathological file from
 * exhausting the JS stack, but it must never fire silently: a cap that drops
 * items while the output still reads "covered everything" is the exact failure
 * mode this tool exists to prevent, so every hit is surfaced as a diagnostic.
 */
const SCAN_DEPTH_CAP = 32;

function scanScope(st, from, to, ctx, depth) {
  if (depth > SCAN_DEPTH_CAP) {
    st.out.scopeDepthCapHits++;
    return;
  }
  const m = st.masked;
  let i = from;
  let attrs = [];
  let attrStart = -1;

  while (i < to) {
    const c = m[i];
    if (c === ' ' || c === '\t' || c === '\n' || c === '\r') {
      i++;
      continue;
    }

    if (c === '#') {
      let j = i + 1;
      const inner = m[j] === '!';
      if (inner) j++;
      if (m[j] === '[') {
        const e = matchDelimiter(m, j);
        if (e < 0 || e >= to) {
          i = j + 1;
          continue;
        }
        const raw = st.src.slice(i, e + 1);
        if (inner) {
          if (ctx.kind === 'file' && CFG_TEST_RE.test(raw)) st.out.fileCfgTest = true;
        } else {
          if (attrs.length === 0) attrStart = i;
          attrs.push(raw);
        }
        i = e + 1;
        continue;
      }
      i++;
      continue;
    }

    const itemStart = attrs.length > 0 ? attrStart : i;
    const next = scanItem(st, i, to, ctx, attrs, itemStart, depth);
    attrs = [];
    attrStart = -1;
    i = next > i ? next : i + 1;
  }
}

function scanItem(st, i, to, ctx, attrs, itemStart, depth) {
  const m = st.masked;
  let j = i;
  let sawValueKeyword = false;

  for (;;) {
    j = skipWs(m, j, to);
    const id = readIdent(m, j, to);
    if (!id || !MODIFIERS.has(id.name)) break;
    j = id.end;
    if (id.name === 'const' || id.name === 'static') sawValueKeyword = true;
    if (id.name === 'pub') {
      const k = skipWs(m, j, to);
      if (m[k] === '(') {
        const e = matchDelimiter(m, k);
        if (e > 0 && e < to) j = e + 1;
      }
    }
  }

  j = skipWs(m, j, to);
  const kw = readIdent(m, j, to);
  const attrText = attrs.join(' ');
  const cfgTest = CFG_TEST_RE.test(attrText);
  const cfgGated = !cfgTest && CFG_ANY_RE.test(attrText);
  const pathAttrRaw = attrs.find((a) => /#\s*\[\s*path\s*=/.test(a)) || null;

  const record = (end) => {
    if (cfgTest) st.out.cfgTestRanges.push([itemStart, end]);
    else if (cfgGated) st.out.cfgGatedRanges.push([itemStart, end]);
  };

  if (!kw) {
    const body = findItemBody(m, j, to);
    record(body.end);
    return body.end;
  }

  const name = kw.name;
  const afterKw = kw.end;

  if (name === 'macro_rules') {
    // The body is a template whose expansion site is unknown, so a path inside
    // it cannot be attributed to a real module pair. Skipped wholesale.
    let k = skipWs(m, afterKw, to);
    if (m[k] === '!') k = skipWs(m, k + 1, to);
    const nameId = readIdent(m, k, to);
    const body = findItemBody(m, nameId ? nameId.end : k, to);
    st.out.macroRulesRanges.push([itemStart, body.end]);
    record(body.end);
    return body.end;
  }

  if (name === 'use') {
    const body = findItemBody(m, afterKw, to);
    st.out.useDecls.push({
      start: itemStart,
      keywordStart: kw.start,
      end: body.end,
      bodyStart: afterKw,
      bodyEnd: body.kind === 'semi' ? body.end - 1 : body.end,
      modStack: ctx.modStack.slice(),
      cfgTest,
      cfgGated,
    });
    record(body.end);
    return body.end;
  }

  if (name === 'mod') {
    const nameId = readIdent(m, skipWs(m, afterKw, to), to);
    if (!nameId) {
      const body = findItemBody(m, afterKw, to);
      record(body.end);
      return body.end;
    }
    const body = findItemBody(m, nameId.end, to);
    if (body.kind === 'block') {
      st.out.inlineMods.push({
        name: nameId.name,
        start: itemStart,
        end: body.end,
        bodyStart: body.bodyStart,
        bodyEnd: body.bodyEnd,
        modStack: ctx.modStack.slice(),
        innerStack: ctx.modStack.concat([nameId.name]),
        cfgTest,
        cfgGated,
        pathAttrRaw,
      });
      record(body.end);
      scanScope(
        st,
        body.bodyStart,
        body.bodyEnd,
        { kind: 'mod', modStack: ctx.modStack.concat([nameId.name]), ownerType: null },
        depth + 1,
      );
      return body.end;
    }
    st.out.modDecls.push({
      name: nameId.name,
      start: itemStart,
      keywordStart: kw.start,
      end: body.end,
      modStack: ctx.modStack.slice(),
      cfgTest,
      cfgGated,
      pathAttrRaw,
    });
    record(body.end);
    return body.end;
  }

  if (name === 'fn') {
    const nameId = readIdent(m, skipWs(m, afterKw, to), to);
    const body = findItemBody(m, nameId ? nameId.end : afterKw, to);
    if (nameId) {
      st.out.fns.push({
        name: nameId.name,
        start: itemStart,
        nameOffset: nameId.start,
        end: body.end,
        bodyStart: body.kind === 'block' ? body.bodyStart : -1,
        bodyEnd: body.kind === 'block' ? body.bodyEnd : -1,
        modStack: ctx.modStack.slice(),
        ownerType: ctx.ownerType,
        cfgTest,
        cfgGated,
      });
    }
    record(body.end);
    return body.end;
  }

  if (name === 'impl') {
    const body = findItemBody(m, afterKw, to);
    record(body.end);
    if (body.kind === 'block') {
      const header = m.slice(afterKw, body.bodyStart - 1);
      const selfType = parseImplSelfType(header);
      scanScope(
        st,
        body.bodyStart,
        body.bodyEnd,
        { kind: 'impl', modStack: ctx.modStack.slice(), ownerType: selfType },
        depth + 1,
      );
    }
    return body.end;
  }

  if (name === 'trait') {
    const nameId = readIdent(m, skipWs(m, afterKw, to), to);
    const body = findItemBody(m, nameId ? nameId.end : afterKw, to);
    record(body.end);
    if (nameId) st.out.typeDefs.push({ name: nameId.name, modStack: ctx.modStack.slice() });
    if (body.kind === 'block') {
      scanScope(
        st,
        body.bodyStart,
        body.bodyEnd,
        { kind: 'trait', modStack: ctx.modStack.slice(), ownerType: nameId ? nameId.name : null },
        depth + 1,
      );
    }
    return body.end;
  }

  if (name === 'struct' || name === 'enum' || name === 'union' || name === 'type') {
    const nameId = readIdent(m, skipWs(m, afterKw, to), to);
    const body = findItemBody(m, nameId ? nameId.end : afterKw, to);
    if (nameId) st.out.typeDefs.push({ name: nameId.name, modStack: ctx.modStack.slice() });
    record(body.end);
    return body.end;
  }

  const body = findItemBody(m, afterKw, to);
  // A `const` or `static` initializer can hold call syntax that belongs to no
  // function body. Recorded so those sites are counted, not attributed.
  if (sawValueKeyword) st.out.constStaticRanges.push([itemStart, body.end]);
  record(body.end);
  return body.end;
}

function inAnyRange(offset, ranges) {
  for (const [start, end] of ranges) {
    if (offset >= start && offset < end) return true;
  }
  return false;
}

/* ------------------------------------------------------------------ *
 * 11. use tree flattening
 * ------------------------------------------------------------------ */

function splitTopLevel(masked, from, to, sep) {
  const parts = [];
  let depth = 0;
  let start = from;
  for (let i = from; i < to; i++) {
    const c = masked[i];
    if (c === '{' || c === '(' || c === '[') depth++;
    else if (c === '}' || c === ')' || c === ']') depth = depth > 0 ? depth - 1 : 0;
    else if (c === sep && depth === 0) {
      parts.push([start, i]);
      start = i + 1;
    }
  }
  if (start < to) parts.push([start, to]);
  return parts;
}

/**
 * Flatten `use crate::{a::{b, c as d}, e::*};` into leaves. Regex cannot
 * handle the nesting, so this is a small brace walker.
 */
const USE_TREE_DEPTH_CAP = 16;

/**
 * `ctx` and `file` are required so the depth cap can never fire silently,
 * whichever graph builder is doing the expanding.
 */
function expandUseTree(masked, bodyStart, bodyEnd, ctx, file) {
  const leaves = [];
  const capHits = { count: 0 };
  parseUseNode(masked, bodyStart, bodyEnd, [], false, leaves, 0, capHits);
  if (capHits.count > 0) {
    ctx.diagnostics.push({
      // error, not warn: dropping a `use` can erase an edge, delete the cycle
      // that depended on it and flip the gate to green. §6.1 turns this into
      // exit 3 so a provably incomplete analysis cannot report a clean bill.
      level: 'error',
      code: 'use-tree-depth-cap',
      message:
        `use tree nesting exceeded ${USE_TREE_DEPTH_CAP} levels; ` +
        `${capHits.count} branch(es) were not expanded and any reference below them is missing`,
      file,
    });
  }
  return leaves;
}

function parseUseNode(masked, from, to, prefix, leadingColons, out, depth, capHits) {
  // Same rule as SCAN_DEPTH_CAP: the branch below this depth is dropped, and
  // dropping a `use` can erase an edge and flip the gate, so the caller reports
  // every hit rather than letting the reference vanish quietly.
  if (depth > USE_TREE_DEPTH_CAP) {
    capHits.count++;
    return;
  }
  const segments = prefix.slice();
  let leading = leadingColons;
  let i = skipWs(masked, from, to);

  if (masked[i] === ':' && masked[i + 1] === ':') {
    leading = true;
    i = skipWs(masked, i + 2, to);
  }

  while (i < to) {
    i = skipWs(masked, i, to);
    if (i >= to) break;
    const c = masked[i];

    if (c === '{') {
      const e = matchDelimiter(masked, i);
      if (e < 0 || e > to) return;
      for (const [s, t] of splitTopLevel(masked, i + 1, e, ',')) {
        parseUseNode(masked, s, t, segments, leading, out, depth + 1, capHits);
      }
      return;
    }

    if (c === '*') {
      out.push({ segments: segments.slice(), alias: null, isGlob: true, leadingColons: leading });
      return;
    }

    const id = readIdent(masked, i, to);
    if (!id) {
      i++;
      continue;
    }
    i = id.end;

    if (id.name === 'self' && segments.length > 0) {
      // `use crate::a::{self, b};` yields crate::a and crate::a::b.
      const alias = readTrailingAlias(masked, i, to);
      out.push({ segments: segments.slice(), alias, isGlob: false, leadingColons: leading });
      return;
    }
    segments.push(id.name);

    const k = skipWs(masked, i, to);
    if (masked[k] === ':' && masked[k + 1] === ':') {
      i = k + 2;
      continue;
    }
    const alias = readTrailingAlias(masked, i, to);
    out.push({ segments: segments.slice(), alias, isGlob: false, leadingColons: leading });
    return;
  }

  if (segments.length > prefix.length) {
    out.push({ segments: segments.slice(), alias: null, isGlob: false, leadingColons: leading });
  }
}

function readTrailingAlias(masked, i, to) {
  const k = skipWs(masked, i, to);
  const asId = readIdent(masked, k, to);
  if (!asId || asId.name !== 'as') return null;
  const aliasId = readIdent(masked, skipWs(masked, asId.end, to), to);
  return aliasId ? aliasId.name : null;
}

/* ------------------------------------------------------------------ *
 * 12. Inline qualified path discovery
 * ------------------------------------------------------------------ */

function qualifiedPathRegex(target) {
  const anchors = ['crate', 'super', 'self'];
  if (target.kind !== 'lib') {
    if (target.libName && !anchors.includes(target.libName)) anchors.push(target.libName);
    if (target.pkgIdent && !anchors.includes(target.pkgIdent)) anchors.push(target.pkgIdent);
  }
  return new RegExp(
    '\\b(?:' + anchors.map(escapeRe).join('|') + ')(?:\\s*::\\s*(?:r#)?[A-Za-z_][A-Za-z0-9_]*)+',
    'g',
  );
}

function extractQualifiedPaths(masked, skipRanges, target) {
  const re = qualifiedPathRegex(target);
  const found = [];
  let m;
  while ((m = re.exec(masked)) !== null) {
    const start = m.index;
    if (inAnyRange(start, skipRanges)) continue;
    const segments = m[0]
      .split('::')
      .map((s) => s.trim().replace(/^r#/, ''))
      .filter(Boolean);
    found.push({ start, end: start + m[0].length, segments });
  }
  return found;
}

/* ------------------------------------------------------------------ *
 * 13. Module identity resolution
 * ------------------------------------------------------------------ */

function moduleIdFor(target, segments) {
  return segments.length === 0 ? target.targetId : `${target.targetId}::${segments.join('::')}`;
}

/**
 * Directory a `mod foo;` inside `file` is resolved against. A crate root or a
 * `mod.rs` searches its own directory; `p/bar.rs` searches `p/bar/`.
 */
function moduleSearchDir(fileAbs, segments) {
  const base = path.basename(fileAbs);
  if (segments.length === 0 || base === 'mod.rs') return path.dirname(fileAbs);
  return path.join(path.dirname(fileAbs), base.replace(/\.rs$/i, ''));
}

function withinRoot(rootAbs, candidate) {
  const rel = path.relative(rootAbs, candidate);
  return rel === '' || (!rel.startsWith('..') && !path.isAbsolute(rel));
}

/**
 * Walk the module tree from the target's root file following file-level `mod`
 * declarations. `#[cfg(...)]` on a mod declaration is ignored for traversal,
 * so a `#[cfg(windows)] mod windows;` is still analyzed on Linux; `cfg(test)`
 * is the one exception and only marks the subtree, so its files stay reachable
 * instead of being reported as unreachable.
 */
function buildModuleTree(target, opts, ctx) {
  const modules = new Map();
  const fileToModule = new Map();
  const visitedFiles = new Set();
  const queue = [{ file: target.rootFile, segments: [], testOnly: false }];

  while (queue.length > 0) {
    const cur = queue.shift();
    const fileAbs = cur.file;
    if (visitedFiles.has(fileAbs)) {
      ctx.diagnostics.push({
        level: 'warn',
        code: 'duplicate-module-file',
        message: `file already bound to module ${fileToModule.get(fileAbs)}; second binding ignored`,
        file: relPath(ctx.root, fileAbs),
      });
      continue;
    }
    visitedFiles.add(fileAbs);

    const id = moduleIdFor(target, cur.segments);
    const rel = relPath(ctx.root, fileAbs);
    modules.set(id, {
      id,
      segments: cur.segments.slice(),
      file: fileAbs,
      relFile: rel,
      children: new Map(),
      testOnly: cur.testOnly,
    });
    fileToModule.set(fileAbs, id);

    if (cur.segments.length > 0) {
      const parentId = moduleIdFor(target, cur.segments.slice(0, -1));
      const parent = modules.get(parentId);
      if (parent) parent.children.set(cur.segments[cur.segments.length - 1], id);
    }

    const fc = getFile(fileAbs, ctx);
    if (!fc) continue;

    for (const inline of fc.items.inlineMods) {
      if (inline.pathAttrRaw) {
        ctx.blindSpots.unsupportedPathAttributes.push({
          file: rel,
          line: lineColOf(fc.lineIndex, inline.start).line,
          raw: collapseText(inline.pathAttrRaw),
        });
      }
    }

    for (const decl of fc.items.modDecls) {
      const line = lineColOf(fc.lineIndex, decl.start).line;
      if (decl.modStack.length > 0) {
        // Rust resolves this against a directory named after the inline module
        // chain. Guessing produces wrong module identities, so declare the gap.
        ctx.blindSpots.inlineModuleFileDeclarations.push({
          file: rel,
          line,
          raw: collapseText(fc.src.slice(decl.keywordStart, decl.end)),
        });
        continue;
      }

      const searchDir = moduleSearchDir(fileAbs, cur.segments);
      let resolved = null;

      if (decl.pathAttrRaw) {
        const pm = /#\s*\[\s*path\s*=\s*"([^"]*)"\s*\]/.exec(decl.pathAttrRaw);
        const raw = pm ? pm[1] : null;
        const candidate = raw ? path.resolve(searchDir, raw) : null;
        const bounded =
          candidate !== null && withinRoot(target.sourceRoot, candidate) && withinRoot(ctx.root, candidate);
        if (bounded && isFile(candidate)) {
          resolved = candidate;
        } else if (bounded && isDirectory(candidate) && isFile(path.join(candidate, 'mod.rs'))) {
          resolved = path.join(candidate, 'mod.rs');
        } else {
          ctx.blindSpots.unsupportedPathAttributes.push({
            file: rel,
            line,
            raw: collapseText(`${decl.pathAttrRaw} ${fc.src.slice(decl.keywordStart, decl.end)}`),
          });
          ctx.diagnostics.push({
            level: 'warn',
            code: 'unsupported-path-attribute',
            message: bounded === false ? '#[path] escapes the crate source root' : '#[path] target does not exist',
            file: rel,
          });
          continue;
        }
      } else {
        const flat = path.join(searchDir, `${decl.name}.rs`);
        const nested = path.join(searchDir, decl.name, 'mod.rs');
        const flatExists = isFile(flat);
        const nestedExists = isFile(nested);
        if (flatExists && nestedExists) {
          // Real rustc rejects this. Picking one silently would be a lie.
          ctx.diagnostics.push({
            level: 'warn',
            code: 'ambiguous-module-file',
            message: `both ${decl.name}.rs and ${decl.name}/mod.rs exist; using ${decl.name}.rs`,
            file: rel,
          });
          resolved = flat;
        } else if (flatExists) {
          resolved = flat;
        } else if (nestedExists) {
          resolved = nested;
        } else {
          ctx.diagnostics.push({
            level: 'warn',
            code: 'unresolved-mod-declaration',
            message: `mod ${decl.name}; resolves to no file`,
            file: rel,
          });
          continue;
        }
      }

      queue.push({
        file: resolved,
        segments: cur.segments.concat([decl.name]),
        testOnly: cur.testOnly || decl.cfgTest,
      });
    }
  }

  return { modules, fileToModule, visitedFiles };
}

/* ------------------------------------------------------------------ *
 * 14. File cache
 * ------------------------------------------------------------------ */

function collapseText(text) {
  const one = String(text).replace(/\s+/g, ' ').trim();
  return one.length > 160 ? one.slice(0, 160) : one;
}

function getFile(abs, ctx) {
  if (ctx.fileCache.has(abs)) return ctx.fileCache.get(abs);
  let entry = null;
  try {
    let buf = fs.readFileSync(abs);
    if (buf.length >= 3 && buf[0] === 0xef && buf[1] === 0xbb && buf[2] === 0xbf) {
      buf = buf.subarray(3);
    }
    const src = buf.toString('utf8');
    if (!Buffer.from(src, 'utf8').equals(buf)) {
      ctx.diagnostics.push({
        level: 'warn',
        code: 'non-utf8-file',
        message: 'file is not valid UTF-8; decoded with replacement characters',
        file: relPath(ctx.root, abs),
      });
    }
    const masked = stripCommentsAndStrings(src);
    entry = {
      abs,
      src,
      masked,
      lineIndex: buildLineIndex(src),
      items: extractItems(src, masked),
    };
    if (entry.items.scopeDepthCapHits > 0) {
      ctx.diagnostics.push({
        // error for the same reason as use-tree-depth-cap: items below the cap
        // are simply not recorded, so the answer is incomplete in a way that
        // could mislead. §6.1 turns it into exit 3.
        level: 'error',
        code: 'scan-depth-cap',
        message:
          `item scan stopped at ${SCAN_DEPTH_CAP} levels of nesting ${entry.items.scopeDepthCapHits} time(s); ` +
          'items below that depth were not recorded',
        file: relPath(ctx.root, abs),
      });
    }
  } catch (err) {
    // An incomplete analysis must never report a clean bill of health, so this
    // is error level and forces exit 3, but the walk continues so one bad file
    // yields a complete diagnostic list rather than an abort.
    ctx.diagnostics.push({
      level: 'error',
      code: 'file-read-error',
      message: `cannot read file: ${err.message}`,
      file: relPath(ctx.root, abs),
    });
  }
  ctx.fileCache.set(abs, entry);
  return entry;
}

/* ------------------------------------------------------------------ *
 * 15. Path resolution
 * ------------------------------------------------------------------ */

function foldToFileModule(tree, target, segments) {
  for (let k = segments.length; k >= 0; k--) {
    const id = moduleIdFor(target, segments.slice(0, k));
    if (tree.modules.has(id)) return id;
  }
  return target.targetId;
}

/**
 * Resolve a written path to the deepest module it reaches, plus the segments
 * left over. Trailing segments name items (types, functions, constants); a
 * reference to an item still counts as a reference to the module holding it.
 *
 * Two leftover arrays, not one. `rest` is what the module walk stopped at;
 * `folded` is what the inline-module fold dropped. They are never both
 * non-empty. Keeping the fold's segments out of `rest` is deliberate:
 * buildCallGraph builds a callee id out of `rest`, so returning them there
 * would fabricate call edges that no source line wrote.
 */
function resolvePathDetailed(segments, contextSegments, target, tree, ctx) {
  if (segments.length === 0) return null;
  const first = segments[0];
  let base = null;
  let rest = null;

  if (first === 'crate') {
    base = [];
    rest = segments.slice(1);
  } else if (first === 'self') {
    base = contextSegments.slice();
    rest = segments.slice(1);
  } else if (first === 'super') {
    let k = 0;
    let up = contextSegments.slice();
    while (k < segments.length && segments[k] === 'super') {
      if (up.length === 0) {
        ctx.stats.unresolvedInternalPaths++;
        return null;
      }
      up.pop();
      k++;
    }
    base = up;
    rest = segments.slice(k);
  } else if (target.kind !== 'lib' && (first === target.libName || first === target.pkgIdent)) {
    base = [];
    rest = segments.slice(1);
  } else {
    const isExternal = target.externCrates.has(first);
    const rootModule = tree.modules.get(target.targetId);
    const isTopLevelModule = rootModule ? rootModule.children.has(first) : false;
    if (isExternal && isTopLevelModule) {
      // Rust 2018 uniform paths make this genuinely ambiguous. Never guess.
      ctx.blindSpots.ambiguousBarePaths++;
      return null;
    }
    if (isExternal) {
      ctx.stats.externalReferences++;
      return null;
    }
    if (isTopLevelModule) {
      ctx.stats.barePathResolutions++;
      base = [];
      rest = segments.slice();
    } else {
      ctx.stats.externalReferences++;
      return null;
    }
  }

  const baseId = moduleIdFor(target, base);
  if (!tree.modules.has(baseId)) {
    // The base sits inside an inline module, which has no graph node. Fold to
    // the nearest file-backed ancestor and do not walk further: `self::x`
    // inside `mod inner` does not mean `<file module>::x`.
    return { moduleId: foldToFileModule(tree, target, base), rest: [], folded: rest };
  }

  let currentId = baseId;
  let idx = 0;
  while (idx < rest.length) {
    const child = tree.modules.get(currentId).children.get(rest[idx]);
    if (!child) break;
    currentId = child;
    idx++;
  }
  return { moduleId: currentId, rest: rest.slice(idx), folded: [] };
}

/** Innermost inline module containing `offset`, as full context segments. */
function contextSegmentsAt(moduleSegments, inlineMods, offset) {
  let best = null;
  for (const inline of inlineMods) {
    if (offset >= inline.bodyStart && offset < inline.bodyEnd) {
      if (best === null || inline.bodyStart > best.bodyStart) best = inline;
    }
  }
  return best ? moduleSegments.concat(best.innerStack) : moduleSegments.slice();
}

/* ------------------------------------------------------------------ *
 * 16. Module graph
 * ------------------------------------------------------------------ */

function skipRangesFor(items, includeTests) {
  const ranges = items.macroRulesRanges.slice();
  if (!includeTests) {
    for (const range of items.cfgTestRanges) ranges.push(range);
  }
  return ranges;
}

function buildModuleGraph(targets, trees, opts, ctx) {
  const nodes = [];
  const edges = [];
  const adjacency = new Map();
  const nodeMeta = new Map();

  for (const target of targets) {
    if (!target.enabled) continue;
    const tree = trees.get(target.targetId);
    if (!tree) continue;

    for (const mod of sortedModules(tree)) {
      if (mod.testOnly && !opts.includeTests) continue;
      nodes.push(mod.id);
      if (!adjacency.has(mod.id)) adjacency.set(mod.id, new Set());
      nodeMeta.set(mod.id, {
        parent: mod.segments.length === 0 ? null : moduleIdFor(target, mod.segments.slice(0, -1)),
        file: mod.relFile,
        crateTarget: target.targetId,
        loc: null,
      });

      const fc = getFile(mod.file, ctx);
      if (!fc) continue;
      // Only reachable with a file that was read. The one path where getFile
      // returns null pushes an error diagnostic, and an error means no graph is
      // written at all, so a null `loc` can never reach a file. Defaulting it to
      // 0 would turn an unreadable file into a zero-line module, which is the
      // failure mode this tool exists to refuse.
      nodeMeta.get(mod.id).loc = lineCountOf(fc);
      const items = fc.items;
      if (items.fileCfgTest && !opts.includeTests) continue;

      const skip = skipRangesFor(items, opts.includeTests);
      const useRanges = [];

      for (const decl of items.useDecls) {
        useRanges.push([decl.start, decl.end]);
        if (inAnyRange(decl.start, skip)) continue;
        const context = mod.segments.concat(decl.modStack);
        const leaves = expandUseTree(fc.masked, decl.bodyStart, decl.bodyEnd, ctx, mod.relFile);
        const pos = lineColOf(fc.lineIndex, decl.keywordStart);
        const text = collapseText(fc.src.slice(decl.start, decl.end));
        const cfgGated = decl.cfgGated || inAnyRange(decl.start, items.cfgGatedRanges);

        for (const leaf of leaves) {
          if (leaf.isGlob) ctx.blindSpots.globImports++;
          if (leaf.leadingColons) {
            ctx.stats.externalReferences++;
            continue;
          }
          const detailed = resolvePathDetailed(leaf.segments, context, target, tree, ctx);
          if (!detailed) continue;
          addModuleEdge(edges, adjacency, {
            from: mod.id,
            to: detailed.moduleId,
            item: itemOf(detailed),
            file: mod.relFile,
            line: pos.line,
            column: pos.column,
            kind: leaf.isGlob ? 'use-glob' : 'use',
            cfgGated,
            text,
          }, opts, ctx);
        }
      }

      // A path written inside println!("{}", crate::a::b::VALUE) is a genuine
      // reference at that exact site, so macro invocations are not skipped at
      // module level. Function level treats them differently.
      const paths = extractQualifiedPaths(fc.masked, skip.concat(useRanges), target);
      for (const found of paths) {
        const context = contextSegmentsAt(mod.segments, items.inlineMods, found.start);
        const detailed = resolvePathDetailed(found.segments, context, target, tree, ctx);
        if (!detailed) continue;
        const pos = lineColOf(fc.lineIndex, found.start);
        addModuleEdge(edges, adjacency, {
          from: mod.id,
          to: detailed.moduleId,
          item: itemOf(detailed),
          file: mod.relFile,
          line: pos.line,
          column: pos.column,
          kind: 'path',
          cfgGated: inAnyRange(found.start, items.cfgGatedRanges),
          text: collapseText(fc.src.slice(found.start, found.end)),
        }, opts, ctx);
      }
    }
  }

  nodes.sort(compareCodeUnits);
  return { nodes, edges, adjacency, nodeMeta };
}

/**
 * `item` is the first segment that module resolution did not consume, or null
 * when resolution consumed every segment. Reading `folded` as well is what
 * makes "did not consume" mean the same thing in both branches of
 * resolvePathDetailed: between them the two arrays hold every segment that did
 * not become a module, so a null here is a path that named a module and never
 * an item the tool could not name.
 */
function itemOf(detailed) {
  return detailed.rest[0] ?? detailed.folded[0] ?? null;
}

function addModuleEdge(edges, adjacency, edge, opts, ctx) {
  // A module referencing itself is meaningless for coupling.
  if (edge.from === edge.to) return;
  if (!adjacency.has(edge.from)) adjacency.set(edge.from, new Set());
  adjacency.get(edge.from).add(edge.to);
  edges.push(edge);
  ctx.stats.moduleEdgeSites++;
}

function sortedModules(tree) {
  return Array.from(tree.modules.values()).sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

/* ------------------------------------------------------------------ *
 * 17. Function graph
 * ------------------------------------------------------------------ */

const MACRO_INVOCATION_RE = /\b([A-Za-z_][A-Za-z0-9_]*)\s*!\s*[([{]/g;
const METHOD_CALL_RE = /[A-Za-z0-9_)\]]\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)\s*(?:::\s*<[^<>;{}]*>\s*)?\(/g;
// The lookbehind keeps this to a bare first segment. `std::fs::File::open(`
// must not read as `File::open(` on a locally defined `File`: without type
// information that is a guess, and a guess is how an invented edge happens.
const TYPE_CALL_RE = /(?<![.:\w])([A-Za-z_][A-Za-z0-9_]*)\s*::\s*([A-Za-z_][A-Za-z0-9_]*)\s*(?:::\s*<[^<>;{}]*>\s*)?\(/g;
const BARE_CALL_RE = /(?<![.:\w])([A-Za-z_][A-Za-z0-9_]*)\s*(?:::\s*<[^<>;{}]*>\s*)?\(/g;

/** Argument groups of top-level macro invocations, which are skipped wholesale. */
function macroInvocationRanges(masked, skip, ctx) {
  const ranges = [];
  MACRO_INVOCATION_RE.lastIndex = 0;
  let m;
  while ((m = MACRO_INVOCATION_RE.exec(masked)) !== null) {
    if (NON_CALL_WORDS.has(m[1])) continue;
    const start = m.index;
    if (inAnyRange(start, skip)) continue;
    if (inAnyRange(start, ranges)) continue;
    const open = start + m[0].length - 1;
    const close = matchDelimiter(masked, open);
    if (close < 0) continue;
    ranges.push([start, close + 1]);
    ctx.blindSpots.macroInvocationsSkipped++;
  }
  return ranges;
}

function callText(src, masked, start) {
  const open = masked.indexOf('(', start);
  if (open < 0) return collapseText(src.slice(start, start + 80));
  const close = matchDelimiter(masked, open);
  const end = close < 0 ? Math.min(open + 80, src.length) : close + 1;
  return collapseText(src.slice(start, end));
}

function functionNodeId(moduleId, ownerType, name) {
  return ownerType ? `${moduleId}::${ownerType}::${name}` : `${moduleId}::${name}`;
}

function buildCallGraph(targets, trees, opts, ctx) {
  const nodes = new Map();
  const edges = [];
  const adjacency = new Map();
  const perTarget = new Map();

  // Pass one: every function node, so a call can only ever resolve to a node
  // that actually exists.
  for (const target of targets) {
    if (!target.enabled) continue;
    const tree = trees.get(target.targetId);
    if (!tree) continue;
    const index = {
      target,
      tree,
      freeFns: new Map(),
      typeMethods: new Map(),
      typesByModule: new Map(),
      // Definitions that lost a node-id collision, per module. Their bodies must
      // not be attributed to the surviving node.
      discardedFns: new Map(),
    };
    perTarget.set(target.targetId, index);

    for (const mod of sortedModules(tree)) {
      if (mod.testOnly && !opts.includeTests) continue;
      const fc = getFile(mod.file, ctx);
      if (!fc) continue;
      const items = fc.items;
      if (items.fileCfgTest && !opts.includeTests) continue;
      const skip = skipRangesFor(items, opts.includeTests);

      const types = new Set();
      for (const def of items.typeDefs) types.add(def.name);
      index.typesByModule.set(mod.id, types);

      for (const fn of items.fns) {
        if (inAnyRange(fn.start, skip)) continue;
        if (fn.bodyStart < 0) continue;
        const id = functionNodeId(mod.id, fn.ownerType, fn.name);
        if (nodes.has(id)) {
          // Two definitions collapse to one node id: `#[cfg]` twins, or trait
          // impl overloads such as `From<A>` and `From<B>` for the same type.
          // The second definition is not analysed, and §4.9's attribution rule
          // does not authorise charging its body to the surviving node, so its
          // body range is excluded from call attribution below. Silence here
          // would let the tool report full visibility while blind, and would
          // fabricate edges from the discarded body.
          const kept = nodes.get(id);
          const dup = lineColOf(fc.lineIndex, fn.nameOffset);
          ctx.diagnostics.push({
            level: 'warn',
            code: 'duplicate-function-node',
            message:
              `${id} is already bound to ${kept.file}:${kept.line}; the definition at line ${dup.line} ` +
              'is not analysed and its body is excluded from call attribution',
            file: mod.relFile,
          });
          if (!index.discardedFns.has(mod.id)) index.discardedFns.set(mod.id, []);
          index.discardedFns.get(mod.id).push(fn);
          continue;
        }
        const pos = lineColOf(fc.lineIndex, fn.nameOffset);
        nodes.set(id, {
          id,
          name: fn.name,
          ownerType: fn.ownerType,
          moduleId: mod.id,
          file: mod.relFile,
          line: pos.line,
        });
        if (fn.ownerType) {
          const key = `${mod.id}::${fn.ownerType}`;
          if (!index.typeMethods.has(key)) index.typeMethods.set(key, new Map());
          index.typeMethods.get(key).set(fn.name, id);
          types.add(fn.ownerType);
        } else {
          index.freeFns.set(`${mod.id}::${fn.name}`, id);
        }
      }
    }
  }

  // Pass two: call sites.
  for (const target of targets) {
    if (!target.enabled) continue;
    const index = perTarget.get(target.targetId);
    if (!index) continue;
    const tree = index.tree;

    for (const mod of sortedModules(tree)) {
      if (mod.testOnly && !opts.includeTests) continue;
      const fc = getFile(mod.file, ctx);
      if (!fc) continue;
      const items = fc.items;
      if (items.fileCfgTest && !opts.includeTests) continue;

      const skip = skipRangesFor(items, opts.includeTests);
      const macroRanges = macroInvocationRanges(fc.masked, skip, ctx);
      // A discarded definition's body belongs to no node, so nothing inside it
      // may be attributed: keeping it out of `bodies` is what stops the collision
      // from manufacturing edges out of the surviving node. It is deliberately
      // NOT part of `dead`, because unanalysable is not the same as invisible.
      // A call the tool stopped looking at still belongs in the blind-spot
      // tally, otherwise the surface shrinks exactly as visibility shrinks.
      const discarded = index.discardedFns.get(mod.id) || [];
      const discardedStarts = new Set(discarded.map((fn) => fn.start));
      const discardedBodies = discarded.map((fn) => [fn.bodyStart, fn.bodyEnd]);
      const dead = skip.concat(macroRanges);
      const importMap = buildImportMap(fc, mod, target, tree, ctx, skip);
      const bodies = items.fns
        .filter((fn) => fn.bodyStart >= 0 && !inAnyRange(fn.start, skip) && !discardedStarts.has(fn.start))
        .map((fn) => ({ fn, id: functionNodeId(mod.id, fn.ownerType, fn.name) }));

      const enclosing = (offset) => {
        let best = null;
        for (const entry of bodies) {
          if (offset >= entry.fn.bodyStart && offset < entry.fn.bodyEnd) {
            if (best === null || entry.fn.bodyStart > best.fn.bodyStart) best = entry;
          }
        }
        return best;
      };

      const emit = (fromEntry, toId, offset, kind) => {
        if (!toId || !nodes.has(toId)) return false;
        const pos = lineColOf(fc.lineIndex, offset);
        if (!adjacency.has(fromEntry.id)) adjacency.set(fromEntry.id, new Set());
        adjacency.get(fromEntry.id).add(toId);
        edges.push({
          from: fromEntry.id,
          to: toId,
          file: mod.relFile,
          line: pos.line,
          column: pos.column,
          kind,
          cfgGated: inAnyRange(offset, items.cfgGatedRanges),
          text: callText(fc.src, fc.masked, offset),
        });
        return true;
      };

      // Method-call syntax: the receiver type is unknown without type
      // information, so this is structurally out of reach.
      METHOD_CALL_RE.lastIndex = 0;
      let m;
      while ((m = METHOD_CALL_RE.exec(fc.masked)) !== null) {
        const offset = m.index;
        if (inAnyRange(offset, dead)) continue;
        if (!enclosing(offset) && !inAnyRange(offset, discardedBodies)) continue;
        ctx.blindSpots.unresolvedMethodCalls++;
      }

      // Call syntax that belongs to no function body: counted, never attributed.
      for (const re of [METHOD_CALL_RE, TYPE_CALL_RE, BARE_CALL_RE]) {
        re.lastIndex = 0;
        let outside;
        while ((outside = re.exec(fc.masked)) !== null) {
          const offset = outside.index;
          if (!inAnyRange(offset, items.constStaticRanges)) continue;
          if (inAnyRange(offset, dead)) continue;
          if (re === BARE_CALL_RE && NON_CALL_WORDS.has(outside[1])) continue;
          ctx.blindSpots.callsOutsideFunctionBodies++;
        }
      }

      const consumed = [];
      const qualified = extractQualifiedPaths(fc.masked, dead, target);
      for (const found of qualified) {
        const after = skipWs(fc.masked, found.end, fc.masked.length);
        const isCall = fc.masked[after] === '(' || (fc.masked[after] === ':' && fc.masked[after + 1] === ':');
        if (!isCall) continue;
        const openIdx = fc.masked[after] === '(' ? after : findTurbofishParen(fc.masked, after);
        if (openIdx < 0) continue;
        consumed.push([found.start, openIdx + 1]);
        const from = enclosing(found.start);
        if (!from) {
          if (inAnyRange(found.start, discardedBodies)) ctx.blindSpots.unresolvedBareCalls++;
          continue;
        }
        const context = contextSegmentsAt(mod.segments, items.inlineMods, found.start);
        const detailed = resolvePathDetailed(found.segments, context, target, tree, ctx);
        if (!detailed) continue;
        let toId = null;
        if (detailed.rest.length === 1) toId = `${detailed.moduleId}::${detailed.rest[0]}`;
        else if (detailed.rest.length === 2) toId = `${detailed.moduleId}::${detailed.rest[0]}::${detailed.rest[1]}`;
        if (!emit(from, toId, found.start, 'path-call')) ctx.blindSpots.unresolvedBareCalls++;
      }

      TYPE_CALL_RE.lastIndex = 0;
      while ((m = TYPE_CALL_RE.exec(fc.masked)) !== null) {
        const offset = m.index;
        if (inAnyRange(offset, dead) || inAnyRange(offset, consumed)) continue;
        const typeName = m[1];
        const method = m[2];
        if (typeName === 'crate' || typeName === 'super' || typeName === 'self') continue;
        const from = enclosing(offset);
        const inDiscarded = !from && inAnyRange(offset, discardedBodies);
        if (!from && !inDiscarded) continue;
        // Claim the range either way, so the bare-call pass below cannot count
        // the same site a second time.
        consumed.push([offset, offset + m[0].length]);
        if (inDiscarded) {
          ctx.blindSpots.unresolvedBareCalls++;
          continue;
        }
        let toId = null;
        if (typeName === 'Self') {
          const owner = from.fn.ownerType;
          if (owner) toId = `${mod.id}::${owner}::${method}`;
        } else {
          const imported = importMap.get(typeName);
          if (imported && imported.moduleId) {
            toId = `${imported.moduleId}::${typeName}::${method}`;
          }
          const localTypes = index.typesByModule.get(mod.id);
          if ((!toId || !nodes.has(toId)) && localTypes && localTypes.has(typeName)) {
            toId = `${mod.id}::${typeName}::${method}`;
          }
        }
        if (!emit(from, toId, offset, 'type-call')) ctx.blindSpots.unresolvedBareCalls++;
      }

      BARE_CALL_RE.lastIndex = 0;
      while ((m = BARE_CALL_RE.exec(fc.masked)) !== null) {
        const offset = m.index;
        if (inAnyRange(offset, dead) || inAnyRange(offset, consumed)) continue;
        const name = m[1];
        if (NON_CALL_WORDS.has(name)) continue;
        const from = enclosing(offset);
        if (!from) {
          if (inAnyRange(offset, discardedBodies)) ctx.blindSpots.unresolvedBareCalls++;
          continue;
        }
        let toId = index.freeFns.get(`${mod.id}::${name}`) || null;
        if (!toId) {
          const imported = importMap.get(name);
          if (imported && imported.moduleId) toId = `${imported.moduleId}::${imported.name}`;
        }
        if (!emit(from, toId, offset, 'call')) ctx.blindSpots.unresolvedBareCalls++;
      }
    }
  }

  return { nodes, edges, adjacency };
}

function findTurbofishParen(masked, from) {
  // `foo::<T>(` - step over the turbofish and land on the call paren.
  let i = from;
  if (masked[i] === ':' && masked[i + 1] === ':') i += 2;
  i = skipWs(masked, i, masked.length);
  if (masked[i] !== '<') return -1;
  let depth = 0;
  for (; i < masked.length; i++) {
    if (masked[i] === '<') depth++;
    else if (masked[i] === '>') {
      depth--;
      if (depth === 0) {
        i++;
        break;
      }
    } else if (masked[i] === ';' || masked[i] === '{') {
      return -1;
    }
  }
  i = skipWs(masked, i, masked.length);
  return masked[i] === '(' ? i : -1;
}

/**
 * Names a file brings into scope, mapped to the module that owns them. This is
 * what makes `use crate::a::foo as bar; bar();` resolve.
 */
function buildImportMap(fc, mod, target, tree, ctx, skip) {
  const map = new Map();
  for (const decl of fc.items.useDecls) {
    if (inAnyRange(decl.start, skip)) continue;
    if (decl.modStack.length > 0) continue;
    for (const leaf of expandUseTree(fc.masked, decl.bodyStart, decl.bodyEnd, ctx, mod.relFile)) {
      if (leaf.isGlob || leaf.leadingColons) continue;
      if (leaf.segments.length === 0) continue;
      const local = leaf.alias || leaf.segments[leaf.segments.length - 1];
      const silent = { stats: silentStats(), blindSpots: silentStats() };
      const detailed = resolvePathDetailed(leaf.segments, mod.segments, target, tree, silent);
      if (!detailed) continue;
      map.set(local, {
        moduleId: detailed.moduleId,
        name: leaf.segments[leaf.segments.length - 1],
      });
    }
  }
  return map;
}

/** Counter sink: import-map resolution must not double-count blind spots. */
function silentStats() {
  return new Proxy(
    {},
    {
      get: () => 0,
      set: () => true,
    },
  );
}

/* ------------------------------------------------------------------ *
 * 18. Tarjan
 * ------------------------------------------------------------------ */

/** Iterative, so a large graph cannot blow the JS stack. */
function tarjan(nodeIds, adjacency) {
  const ids = nodeIds.slice().sort(compareCodeUnits);
  const neighbours = new Map();
  for (const id of ids) {
    const set = adjacency.get(id);
    neighbours.set(id, set ? Array.from(set).filter((n) => adjacency.has(n) || ids.includes(n)).sort(compareCodeUnits) : []);
  }

  const index = new Map();
  const lowlink = new Map();
  const onStack = new Set();
  const stack = [];
  const sccs = [];
  let counter = 0;

  for (const root of ids) {
    if (index.has(root)) continue;
    const work = [{ node: root, edge: 0 }];
    index.set(root, counter);
    lowlink.set(root, counter);
    counter++;
    stack.push(root);
    onStack.add(root);

    while (work.length > 0) {
      const frame = work[work.length - 1];
      const list = neighbours.get(frame.node) || [];
      if (frame.edge < list.length) {
        const next = list[frame.edge];
        frame.edge++;
        if (!index.has(next)) {
          index.set(next, counter);
          lowlink.set(next, counter);
          counter++;
          stack.push(next);
          onStack.add(next);
          work.push({ node: next, edge: 0 });
        } else if (onStack.has(next)) {
          lowlink.set(frame.node, Math.min(lowlink.get(frame.node), index.get(next)));
        }
        continue;
      }

      work.pop();
      if (work.length > 0) {
        const parent = work[work.length - 1].node;
        lowlink.set(parent, Math.min(lowlink.get(parent), lowlink.get(frame.node)));
      }
      if (lowlink.get(frame.node) === index.get(frame.node)) {
        const component = [];
        for (;;) {
          const popped = stack.pop();
          onStack.delete(popped);
          component.push(popped);
          if (popped === frame.node) break;
        }
        component.sort(compareCodeUnits);
        sccs.push(component);
      }
    }
  }

  sccs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  return sccs;
}

function hasSelfEdge(adjacency, id) {
  const set = adjacency.get(id);
  return set ? set.has(id) : false;
}

/* ------------------------------------------------------------------ *
 * 19. Stable cycle identity
 * ------------------------------------------------------------------ */

/**
 * Identity depends only on the sorted member set, so reordering code or moving
 * a line does not change it, while adding or removing a member does.
 */
function cycleId(members) {
  return createHash('sha256').update(members.slice().sort(compareCodeUnits).join('\n')).digest('hex').slice(0, 16);
}

function edgesWithin(edges, memberSet) {
  return edges
    .filter((e) => memberSet.has(e.from) && memberSet.has(e.to))
    .slice()
    .sort(compareEdges);
}

function compareEdges(a, b) {
  if (a.file !== b.file) return a.file < b.file ? -1 : 1;
  if (a.line !== b.line) return a.line - b.line;
  if (a.column !== b.column) return a.column - b.column;
  if (a.from !== b.from) return a.from < b.from ? -1 : 1;
  if (a.to !== b.to) return a.to < b.to ? -1 : 1;
  return 0;
}

function compareCycles(a, b) {
  const am = a.members[0] || '';
  const bm = b.members[0] || '';
  if (am !== bm) return am < bm ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

function buildModuleCycles(graph) {
  const sccs = tarjan(graph.nodes, graph.adjacency);
  const cycles = [];
  for (const members of sccs) {
    if (members.length < 2) continue; // module self-edges are dropped entirely
    const memberSet = new Set(members);
    cycles.push({
      id: cycleId(members),
      size: members.length,
      members: members.slice().sort(compareCodeUnits),
      edges: edgesWithin(graph.edges, memberSet),
    });
  }
  cycles.sort(compareCycles);
  return cycles;
}

/* ------------------------------------------------------------------ *
 * 20. Function cycle classification
 * ------------------------------------------------------------------ */

/**
 * An SCC of size 1 with a self-edge is a function calling itself. That is
 * recursion, a runtime property, not a dependency cycle: it goes into its own
 * array and never gates.
 */
function classifyFunctionCycles(graph, ctx) {
  const nodeIds = Array.from(graph.nodes.keys());
  const sccs = tarjan(nodeIds, graph.adjacency);
  const cycles = [];
  const recursion = [];

  for (const members of sccs) {
    if (members.length === 1) {
      if (hasSelfEdge(graph.adjacency, members[0])) {
        const node = graph.nodes.get(members[0]);
        recursion.push({ function: members[0], file: node ? node.file : '', line: node ? node.line : 0 });
      }
      continue;
    }
    const memberSet = new Set(members);
    const modules = Array.from(new Set(members.map((id) => graph.nodes.get(id).moduleId))).sort(compareCodeUnits);
    cycles.push({
      id: cycleId(members),
      scope: modules.length === 1 ? 'intra-module' : 'cross-module',
      size: members.length,
      members: members.slice().sort(compareCodeUnits),
      modules,
      edges: edgesWithin(graph.edges, memberSet),
    });
  }

  cycles.sort(compareCycles);
  recursion.sort((a, b) => (a.function < b.function ? -1 : a.function > b.function ? 1 : 0));
  return { cycles, recursion };
}

/* ------------------------------------------------------------------ *
 * 21. Baseline
 * ------------------------------------------------------------------ */

function loadBaseline(file) {
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (err) {
    return { ok: false, message: `cannot read baseline file ${file}: ${err.message}` };
  }
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (err) {
    return { ok: false, message: `baseline file ${file} is not valid JSON: ${err.message}` };
  }
  if (!parsed || typeof parsed !== 'object' || parsed.kind !== BASELINE_KIND) {
    return { ok: false, message: `baseline file ${file} is missing kind "${BASELINE_KIND}"` };
  }
  if (parsed.schemaVersion !== BASELINE_SCHEMA_VERSION) {
    return {
      ok: false,
      message:
        `baseline file ${file} has schemaVersion ${parsed.schemaVersion}, this tool writes ` +
        `${BASELINE_SCHEMA_VERSION}; regenerate it with --write-baseline`,
    };
  }
  return { ok: true, baseline: parsed };
}

function writeBaseline(file, result, opts) {
  const payload = {
    schemaVersion: BASELINE_SCHEMA_VERSION,
    kind: BASELINE_KIND,
    generatedFrom: {
      toolVersion: TOOL_VERSION,
      functionScope: opts.functionScope,
      includeTests: opts.includeTests,
    },
    moduleCycles: result.moduleCycles.map((c) => ({ id: c.id, members: c.members })),
    functionCycles: result.functionCycles.map((c) => ({ id: c.id, scope: c.scope, members: c.members })),
  };
  fs.writeFileSync(file, JSON.stringify(payload, null, 2) + '\n', 'utf8');
  return payload;
}

/** Matching is by id only; members are stored for readability and git diff. */
function applyBaseline(result, baseline, baselinePath, opts, ctx) {
  if (!baseline) {
    for (const cycle of result.moduleCycles) cycle.status = 'unbaselined';
    for (const cycle of result.functionCycles) cycle.status = 'unbaselined';
    return { used: false };
  }

  const from = baseline.generatedFrom || {};
  if (from.functionScope !== opts.functionScope || from.includeTests !== opts.includeTests) {
    ctx.diagnostics.push({
      level: 'warn',
      code: 'baseline-settings-mismatch',
      message:
        `baseline was generated with functionScope=${from.functionScope} includeTests=${from.includeTests}; ` +
        `this run uses functionScope=${opts.functionScope} includeTests=${opts.includeTests}`,
      file: toPosix(baselinePath),
    });
  }

  const moduleIds = new Set((baseline.moduleCycles || []).map((c) => c.id));
  const functionIds = new Set((baseline.functionCycles || []).map((c) => c.id));
  const currentModule = new Set(result.moduleCycles.map((c) => c.id));
  const currentFunction = new Set(result.functionCycles.map((c) => c.id));

  let knownModule = 0;
  let newModule = 0;
  let knownFunction = 0;
  let newFunction = 0;

  for (const cycle of result.moduleCycles) {
    if (moduleIds.has(cycle.id)) {
      cycle.status = 'known';
      knownModule++;
    } else {
      cycle.status = 'new';
      newModule++;
    }
  }
  for (const cycle of result.functionCycles) {
    if (functionIds.has(cycle.id)) {
      cycle.status = 'known';
      knownFunction++;
    } else {
      cycle.status = 'new';
      newFunction++;
    }
  }

  const resolvedCycles = [];
  for (const cycle of baseline.moduleCycles || []) {
    if (!currentModule.has(cycle.id)) {
      resolvedCycles.push({ graph: 'module', id: cycle.id, members: cycle.members || [] });
    }
  }
  for (const cycle of baseline.functionCycles || []) {
    if (!currentFunction.has(cycle.id)) {
      resolvedCycles.push({ graph: 'function', id: cycle.id, members: cycle.members || [] });
    }
  }
  resolvedCycles.sort((a, b) => (a.graph !== b.graph ? (a.graph < b.graph ? -1 : 1) : a.id < b.id ? -1 : 1));

  return {
    used: true,
    path: toPosix(baselinePath),
    knownModuleCycles: knownModule,
    newModuleCycles: newModule,
    knownFunctionCycles: knownFunction,
    newFunctionCycles: newFunction,
    resolvedCycles,
  };
}

/* ------------------------------------------------------------------ *
 * 22. Analysis
 * ------------------------------------------------------------------ */

function newContext(rootAbs) {
  return {
    root: rootAbs,
    fileCache: new Map(),
    diagnostics: [],
    stats: {
      externalReferences: 0,
      barePathResolutions: 0,
      unresolvedInternalPaths: 0,
      moduleEdgeSites: 0,
    },
    blindSpots: {
      unresolvedMethodCalls: 0,
      unresolvedBareCalls: 0,
      macroInvocationsSkipped: 0,
      macroDefinitionsSkipped: 0,
      callsOutsideFunctionBodies: 0,
      globImports: 0,
      ambiguousBarePaths: 0,
      unreachableFiles: [],
      unsupportedPathAttributes: [],
      inlineModuleFileDeclarations: [],
    },
  };
}

function analyze(opts) {
  const rootAbs = path.resolve(opts.pathArg);
  let stat;
  try {
    stat = fs.statSync(rootAbs);
  } catch {
    throw new AnalysisError(`path does not exist: ${toPosix(rootAbs)}`);
  }

  const ctx = newContext(rootAbs);
  const compiled = compileExcludes(opts.excludes);

  let rustFiles;
  let manifests;
  let targets;
  let crateDiscovery;

  if (stat.isFile()) {
    if (!rootAbs.endsWith('.rs')) throw new AnalysisError(`not a .rs file: ${toPosix(rootAbs)}`);
    rustFiles = [rootAbs];
    manifests = [];
    ctx.root = path.dirname(rootAbs);
    targets = [
      makeTarget({
        targetId: 'crate',
        kind: path.basename(rootAbs) === 'lib.rs' ? 'lib' : 'bin',
        packageName: null,
        pkgIdent: 'crate',
        libName: 'crate',
        manifestPath: null,
        rootFile: rootAbs,
        sourceRoot: path.dirname(rootAbs),
        edition: '2021',
        externCrates: new Set(),
        enabled: true,
      }),
    ];
    crateDiscovery = 'fallback';
  } else {
    const walk = findRustFiles(rootAbs, compiled, ctx);
    rustFiles = walk.rustFiles;
    manifests = walk.manifests;
    if (rustFiles.length === 0) throw new AnalysisError(`no .rs files found under ${toPosix(rootAbs)}`);
    targets = discoverTargets(rootAbs, manifests, opts, ctx);
    crateDiscovery = 'manifest';
    if (targets.length === 0) {
      const fallback = fallbackTarget(rootAbs);
      if (!fallback) {
        throw new AnalysisError(
          manifests.length === 0
            ? `no Cargo.toml and no crate root (lib.rs / main.rs) found under ${toPosix(rootAbs)}`
            : `no crate target found under ${toPosix(rootAbs)}: manifests exist but none yields a lib.rs or main.rs`,
        );
      }
      targets = [fallback];
      crateDiscovery = 'fallback';
    }
  }

  // Disabled targets are still traversed, for file accounting only. Without
  // that, every file under tests/ would be reported as unreachable, which it
  // is not: it is excluded test code, a different category.
  const trees = new Map();
  const reached = new Set();
  for (const target of targets) {
    const tree = buildModuleTree(target, opts, ctx);
    trees.set(target.targetId, tree);
    for (const file of tree.visitedFiles) reached.add(file);
  }

  for (const file of rustFiles) {
    if (!reached.has(file)) ctx.blindSpots.unreachableFiles.push(relPath(rootAbs, file));
  }
  ctx.blindSpots.unreachableFiles.sort(compareCodeUnits);

  const wantModule = opts.level === 'module' || opts.level === 'all';
  const wantFunction = opts.level === 'function' || opts.level === 'all';

  const moduleGraph = wantModule
    ? buildModuleGraph(targets, trees, opts, ctx)
    // Same shape as buildModuleGraph returns, nodeMeta included. buildGraphPayload
    // never reads it in this mode, but a placeholder shaped differently from the
    // real thing is a trap for the next reader.
    : { nodes: [], edges: [], adjacency: new Map(), nodeMeta: new Map() };
  const callGraph = wantFunction
    ? buildCallGraph(targets, trees, opts, ctx)
    : { nodes: new Map(), edges: [], adjacency: new Map() };

  for (const [, fc] of ctx.fileCache) {
    if (fc) ctx.blindSpots.macroDefinitionsSkipped += fc.items.macroRulesRanges.length;
  }

  const moduleCycles = wantModule ? buildModuleCycles(moduleGraph) : [];
  const classified = wantFunction
    ? classifyFunctionCycles(callGraph, ctx)
    : { cycles: [], recursion: [] };

  const crossModule = classified.cycles.filter((c) => c.scope === 'cross-module');
  const intraModule = classified.cycles.filter((c) => c.scope === 'intra-module');
  const reportedFunctionCycles =
    opts.functionScope === 'all' ? classified.cycles.slice() : crossModule.slice();

  const enabledTargets = targets.filter((t) => t.enabled);
  const crates = enabledTargets.map((target) => {
    const tree = trees.get(target.targetId);
    const count = tree
      ? Array.from(tree.modules.values()).filter((m) => opts.includeTests || !m.testOnly).length
      : 0;
    return {
      targetId: target.targetId,
      kind: target.kind,
      packageName: target.packageName,
      manifestPath: target.manifestPath,
      rootFile: relPath(rootAbs, target.rootFile),
      edition: target.edition,
      modules: count,
      files: count,
    };
  });
  crates.sort((a, b) => (a.targetId < b.targetId ? -1 : a.targetId > b.targetId ? 1 : 0));

  return {
    ctx,
    opts,
    rootAbs,
    rootPosix: toPosix(rootAbs),
    crateDiscovery,
    targets,
    trees,
    crates,
    moduleGraph,
    callGraph,
    moduleCycles,
    functionCycles: reportedFunctionCycles,
    allFunctionCycles: classified.cycles,
    recursion: classified.recursion,
    summary: {
      filesScanned: rustFiles.length,
      modulesResolved: moduleGraph.nodes.length,
      functionsResolved: callGraph.nodes.size,
      moduleEdges: moduleGraph.edges.length,
      functionEdges: callGraph.edges.length,
      moduleCycles: moduleCycles.length,
      functionCyclesCrossModule: crossModule.length,
      functionCyclesIntraModule: intraModule.length,
      recursiveFunctions: classified.recursion.length,
    },
  };
}

/* ------------------------------------------------------------------ *
 * 23. Exit code
 * ------------------------------------------------------------------ */

function computeExitCode(result, baselineInfo, opts) {
  // An incomplete analysis must never report a clean bill of health.
  if (result.ctx.diagnostics.some((d) => d.level === 'error')) return 3;
  if (opts.failOn === 'none') return 0;

  const usingBaseline = baselineInfo && baselineInfo.used;
  const gates = (cycle) => !usingBaseline || cycle.status === 'new';

  let gating = result.moduleCycles.filter(gates).length;
  if (opts.failOn === 'any') {
    // Only cross-module function cycles are eligible to gate. `--function-scope`
    // decides what is reported, not what fails the build, so this reads the full
    // classified set rather than the reported one.
    gating += result.allFunctionCycles.filter((c) => c.scope === 'cross-module').filter(gates).length;
  }
  return gating > 0 ? 1 : 0;
}

/* ------------------------------------------------------------------ *
 * 24. Output
 * ------------------------------------------------------------------ */

function blindSpotsPayload(ctx) {
  return {
    unresolvedMethodCalls: ctx.blindSpots.unresolvedMethodCalls,
    unresolvedBareCalls: ctx.blindSpots.unresolvedBareCalls,
    macroInvocationsSkipped: ctx.blindSpots.macroInvocationsSkipped,
    macroDefinitionsSkipped: ctx.blindSpots.macroDefinitionsSkipped,
    callsOutsideFunctionBodies: ctx.blindSpots.callsOutsideFunctionBodies,
    globImports: ctx.blindSpots.globImports,
    ambiguousBarePaths: ctx.blindSpots.ambiguousBarePaths,
    unresolvedInternalPaths: ctx.stats.unresolvedInternalPaths,
    externalReferences: ctx.stats.externalReferences,
    barePathResolutions: ctx.stats.barePathResolutions,
    unreachableFiles: ctx.blindSpots.unreachableFiles.slice(),
    unsupportedPathAttributes: ctx.blindSpots.unsupportedPathAttributes.slice(),
    inlineModuleFileDeclarations: ctx.blindSpots.inlineModuleFileDeclarations.slice(),
    notes: BLIND_SPOT_NOTES.slice(),
  };
}

function edgePayload(edge) {
  return {
    from: edge.from,
    to: edge.to,
    file: edge.file,
    line: edge.line,
    column: edge.column,
    kind: edge.kind,
    cfgGated: edge.cfgGated,
    text: edge.text,
  };
}

/**
 * Module edges only. Function edges have no item concept, since their `to`
 * already names the callee, and emitting `item: null` there would invent a
 * field that means nothing. This is also the dump's edge payload: the two are
 * one function only while their key order agrees, because key order is part of
 * byte-identical determinism. A future edit that reorders one must fork them.
 */
function moduleEdgePayload(edge) {
  return {
    from: edge.from,
    to: edge.to,
    item: edge.item ?? null,
    file: edge.file,
    line: edge.line,
    column: edge.column,
    kind: edge.kind,
    cfgGated: edge.cfgGated,
    text: edge.text,
  };
}

/**
 * One comparator for both edge arrays of the graph dump. `from` leads rather
 * than `file`, the order compareEdges uses, because a dump is read by node:
 * grouping a module's outgoing edges together is what a consumer wants and what
 * makes a diff between two dumps readable. compareEdges itself is untouched, so
 * the per-cycle edge lists in the report keep their current order.
 *
 * Total up to record equality: two records tying on all seven keys came from
 * one declaration and therefore share cfgGated and text as well, so they are
 * byte-identical and their relative order cannot change the output.
 */
function compareGraphEdges(a, b) {
  if (a.from !== b.from) return a.from < b.from ? -1 : 1;
  if (a.to !== b.to) return a.to < b.to ? -1 : 1;
  if (a.file !== b.file) return a.file < b.file ? -1 : 1;
  if (a.line !== b.line) return a.line - b.line;
  if (a.column !== b.column) return a.column - b.column;
  if (a.kind !== b.kind) return a.kind < b.kind ? -1 : 1;
  // Function edges have no item, and a module reference sorts before any item
  // reference at the same site.
  const ai = a.item ?? '';
  const bi = b.item ?? '';
  if (ai !== bi) return ai < bi ? -1 : 1;
  return 0;
}

/**
 * Project the analysis into the graph file. A projection and nothing else: it
 * sorts, it maps, it writes nothing, and it never deduplicates, filters or
 * merges. Every judgement about which edges matter belongs to the consumer.
 *
 * A section the run did not build is an ABSENT key, never an empty array: an
 * empty array reads as "computed, and there are none", which is exactly the
 * zero-as-proof-of-absence this tool refuses. That is also why this cannot be
 * one object literal, since key order is part of byte-identical determinism and
 * a literal cannot omit a key: build the head, then the conditional sections,
 * then blindSpots and diagnostics last, so the key order holds at every level.
 */
function buildGraphPayload(result) {
  const { opts, ctx } = result;
  const wantModule = opts.level === 'module' || opts.level === 'all';
  const wantFunction = opts.level === 'function' || opts.level === 'all';

  const payload = {
    schemaVersion: GRAPH_SCHEMA_VERSION,
    kind: GRAPH_KIND,
    language: GRAPH_LANGUAGE,
    tool: { name: TOOL_NAME, version: TOOL_VERSION },
    target: {
      // failOn is deliberately absent: it decides the exit code and never a node
      // or an edge. excludes IS here, because two runs with different excludes
      // are two different graphs.
      rootPath: result.rootPosix,
      crateDiscovery: result.crateDiscovery,
      level: opts.level,
      functionScope: opts.functionScope,
      includeTests: opts.includeTests,
      excludes: opts.excludes.slice(),
    },
    crates: result.crates,
    summary: result.summary,
  };

  if (wantModule) {
    payload.modules = result.moduleGraph.nodes.map((id) => {
      const meta = result.moduleGraph.nodeMeta.get(id);
      return {
        id,
        parent: meta.parent,
        file: meta.file,
        crateTarget: meta.crateTarget,
        loc: meta.loc,
      };
    });
    payload.edges = result.moduleGraph.edges.slice().sort(compareGraphEdges).map(moduleEdgePayload);
  }

  if (wantFunction) {
    payload.functions = Array.from(result.callGraph.nodes.values())
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
      .map((fn) => ({
        id: fn.id,
        name: fn.name,
        ownerType: fn.ownerType,
        moduleId: fn.moduleId,
        file: fn.file,
        line: fn.line,
      }));
    payload.functionEdges = result.callGraph.edges.slice().sort(compareGraphEdges).map(edgePayload);
  }

  payload.blindSpots = blindSpotsPayload(ctx);
  payload.diagnostics = ctx.diagnostics;
  return payload;
}

/**
 * Closure: every id a record names must exist as a node in the same file.
 *
 * Only check 1 guards a hole that is reachable today, where non-test code
 * references a `#[cfg(test)] mod` file module: resolvePathDetailed walks the
 * full module tree while buildModuleGraph filters testOnly out of the nodes.
 * Checks 2 to 4 hold by construction and are the cheapest statement of the
 * schema's closure property, so they stay as regression guards. The four mean
 * four different things, which is why a problem records which one it broke: a
 * check-1 failure has a known workaround, the other three are a tool bug.
 */
function checkGraphIntegrity(payload) {
  const problems = [];
  const hasModules = Object.prototype.hasOwnProperty.call(payload, 'modules');
  const hasFunctions = Object.prototype.hasOwnProperty.call(payload, 'functions');
  const moduleIdSet = hasModules ? new Set(payload.modules.map((m) => m.id)) : null;
  const functionIdSet = hasFunctions ? new Set(payload.functions.map((f) => f.id)) : null;

  if (hasModules) {
    for (const edge of payload.edges) {
      if (!moduleIdSet.has(edge.to)) problems.push({ check: 1, from: edge.from, to: edge.to });
    }
    for (const mod of payload.modules) {
      if (mod.parent !== null && !moduleIdSet.has(mod.parent)) {
        problems.push({ check: 2, from: mod.id, to: mod.parent });
      }
    }
  }
  if (hasFunctions) {
    for (const edge of payload.functionEdges) {
      if (!functionIdSet.has(edge.from) || !functionIdSet.has(edge.to)) {
        problems.push({ check: 3, from: edge.from, to: edge.to });
      }
    }
    if (hasModules) {
      for (const fn of payload.functions) {
        if (!moduleIdSet.has(fn.moduleId)) problems.push({ check: 4, from: fn.id, to: fn.moduleId });
      }
    }
  }
  return problems;
}

/** Only the sections actually present, for the confirmation line. */
function graphCounts(payload) {
  const parts = [];
  if (payload.modules) parts.push(`${payload.modules.length} modules`);
  if (payload.edges) parts.push(`${payload.edges.length} edges`);
  if (payload.functions) parts.push(`${payload.functions.length} functions`);
  if (payload.functionEdges) parts.push(`${payload.functionEdges.length} function edges`);
  return parts.join(', ');
}

/**
 * Serialization is the report's and the baseline's, byte for byte. The write
 * failure is rethrown as an AnalysisError so the operator gets the path and the
 * cause instead of `internal error` plus a stack trace.
 *
 * On a failure the destination is left in an unspecified state and must not be
 * consumed: writeFileSync truncates before it writes, so a failure at open
 * leaves the previous contents and a failure mid-write leaves a partial file.
 */
function writeGraph(file, payload) {
  try {
    fs.writeFileSync(file, JSON.stringify(payload, null, 2) + '\n', 'utf8');
  } catch (err) {
    throw new AnalysisError(`cannot write graph file ${toPosix(file)}: ${err.message}`);
  }
}

/**
 * No timestamps, no durations, nothing machine-specific beyond target.rootPath.
 * Two consecutive runs over an unchanged tree must produce byte-identical JSON.
 */
function renderJson(result, baselineInfo, exitCode) {
  const { opts, ctx } = result;
  const summary = { ...result.summary };
  if (baselineInfo.used) {
    summary.newModuleCycles = baselineInfo.newModuleCycles;
    summary.newFunctionCycles = baselineInfo.newFunctionCycles;
  }

  const payload = {
    schemaVersion: SCHEMA_VERSION,
    tool: { name: TOOL_NAME, version: TOOL_VERSION },
    target: {
      rootPath: result.rootPosix,
      crateDiscovery: result.crateDiscovery,
      level: opts.level,
      functionScope: opts.functionScope,
      includeTests: opts.includeTests,
      failOn: opts.failOn,
    },
    crates: result.crates,
    summary,
    moduleCycles: result.moduleCycles.map((c) => ({
      id: c.id,
      status: c.status,
      size: c.size,
      members: c.members,
      edges: c.edges.map(moduleEdgePayload),
    })),
    functionCycles: result.functionCycles.map((c) => ({
      id: c.id,
      status: c.status,
      scope: c.scope,
      size: c.size,
      members: c.members,
      modules: c.modules,
      edges: c.edges.map(edgePayload),
    })),
    recursion: result.recursion,
    blindSpots: blindSpotsPayload(ctx),
    baseline: baselineInfo,
    diagnostics: ctx.diagnostics,
    exitCode,
  };

  return JSON.stringify(payload, null, 2) + '\n';
}

function leader(label, width) {
  const dots = Math.max(1, width - label.length);
  return `${label} ${'.'.repeat(dots)}`;
}

function renderHuman(result, baselineInfo, exitCode) {
  const { opts, ctx, summary } = result;
  const out = [];
  const targetCount = result.crates.length;

  out.push(`${TOOL_NAME} ${TOOL_VERSION}`);
  out.push(
    `target: ${path.basename(result.rootAbs)}  (${result.crateDiscovery} discovery, ` +
      `${targetCount} crate target${targetCount === 1 ? '' : 's'})`,
  );
  out.push('');
  out.push(
    `scanned ${summary.filesScanned} files, resolved ${summary.modulesResolved} modules ` +
      `and ${summary.functionsResolved} functions`,
  );
  out.push('');

  const newSuffix = (cycles) =>
    baselineInfo.used ? ` (${cycles.filter((c) => c.status === 'new').length} new)` : '';

  if (opts.level === 'function') {
    out.push('MODULE CYCLES: not analyzed (--level function)');
  } else {
    out.push(`MODULE CYCLES: ${result.moduleCycles.length}${newSuffix(result.moduleCycles)}`);
    for (const cycle of result.moduleCycles) {
      out.push(`  [${cycle.id.slice(0, 8)}] size ${cycle.size}`);
      for (const member of cycle.members) out.push(`    ${member}`);
      for (const edge of cycle.edges) {
        out.push(`    edge ${edge.file}:${edge.line}:${edge.column}  ${edge.text}`);
      }
    }
  }
  out.push('');

  if (opts.level === 'module') {
    out.push('FUNCTION CYCLES: not analyzed (--level module)');
  } else {
    out.push(
      `FUNCTION CYCLES (${opts.functionScope}): ${result.functionCycles.length}` +
        newSuffix(result.functionCycles),
    );
    for (const cycle of result.functionCycles) {
      out.push(`  [${cycle.id.slice(0, 8)}] size ${cycle.size} ${cycle.scope}`);
      for (const member of cycle.members) out.push(`    ${member}`);
      for (const edge of cycle.edges) {
        out.push(`    edge ${edge.file}:${edge.line}:${edge.column}  ${edge.text}`);
      }
    }
  }
  out.push('');
  out.push(
    `RECURSION (not a dependency cycle, never gates): ${result.recursion.length} function` +
      `${result.recursion.length === 1 ? '' : 's'}`,
  );
  for (const entry of result.recursion) {
    out.push(`  ${entry.function}  ${entry.file}:${entry.line}`);
  }
  out.push('');

  // Printed always, including when every count is zero and zero cycles were
  // found. Suppressing it when the news is good is exactly how a zero comes to
  // be misread as proof of absence.
  const bs = blindSpotsPayload(ctx);
  const width = 40;
  out.push('BLIND SPOTS - what this analysis could NOT see');
  out.push(`  ${leader('unresolved method calls (x.foo())', width)} ${bs.unresolvedMethodCalls}`);
  out.push(`  ${leader('calls inside macro invocations', width)} ${bs.macroInvocationsSkipped} invocations skipped`);
  out.push(`  ${leader('macro_rules! bodies skipped', width)} ${bs.macroDefinitionsSkipped}`);
  out.push(`  ${leader('unresolved bare calls', width)} ${bs.unresolvedBareCalls}`);
  out.push(`  ${leader('calls outside function bodies', width)} ${bs.callsOutsideFunctionBodies}`);
  out.push(`  ${leader('glob imports in scope', width)} ${bs.globImports}`);
  out.push(`  ${leader('ambiguous bare paths (no edge)', width)} ${bs.ambiguousBarePaths}`);
  out.push(`  ${leader('unresolved internal paths', width)} ${bs.unresolvedInternalPaths}`);
  out.push(`  ${leader('external references ignored', width)} ${bs.externalReferences}`);
  out.push(`  ${leader('unreachable files excluded', width)} ${bs.unreachableFiles.length}`);
  out.push(`  ${leader('unsupported #[path] attributes', width)} ${bs.unsupportedPathAttributes.length}`);
  out.push(`  ${leader('mod decls inside inline mod blocks', width)} ${bs.inlineModuleFileDeclarations.length}`);
  out.push('  Function-level results are a LOWER BOUND. Zero is not proof of absence.');
  out.push('');
  out.push(verdictLine(result, baselineInfo, exitCode));

  return out.join('\n') + '\n';
}

function verdictLine(result, baselineInfo, exitCode) {
  const parts = [];
  const moduleCount = result.moduleCycles.length;
  parts.push(`${moduleCount} module cycle${moduleCount === 1 ? '' : 's'} found`);
  if (baselineInfo.used) {
    parts.push(`${baselineInfo.newModuleCycles} new against baseline`);
  } else {
    parts.push('no baseline');
  }
  if (result.opts.failOn === 'any') {
    const fnNew = baselineInfo.used
      ? result.functionCycles.filter((c) => c.status === 'new').length
      : result.functionCycles.length;
    parts.push(`${fnNew} gating function cycle${fnNew === 1 ? '' : 's'}`);
  }
  if (exitCode === 3) return `exit 3: analysis incomplete, see diagnostics on stderr`;
  parts.push(exitCode === 1 ? 'gate fails' : 'gate passes');
  return `exit ${exitCode}: ${parts.join(', ')}`;
}

function renderDiagnostics(ctx) {
  const lines = [];
  for (const diag of ctx.diagnostics) {
    lines.push(`${diag.level}: ${diag.code}: ${diag.message}${diag.file ? ` (${diag.file})` : ''}`);
  }
  return lines.length > 0 ? lines.join('\n') + '\n' : '';
}

/* ------------------------------------------------------------------ *
 * 25. Run pipeline
 * ------------------------------------------------------------------ */

/** Full analysis plus baseline application. Never writes to stdout or stderr. */
function runAnalysis(opts) {
  let baseline = null;
  if (opts.baseline) {
    const loaded = loadBaseline(opts.baseline);
    if (!loaded.ok) throw new UsageError(loaded.message);
    baseline = loaded.baseline;
  }
  const result = analyze(opts);
  const baselineInfo = applyBaseline(result, baseline, opts.baseline || '', opts, result.ctx);
  const exitCode = computeExitCode(result, baselineInfo, opts);
  return { result, baselineInfo, exitCode };
}

function main(argv) {
  let opts;
  try {
    opts = parseArgs(argv);
  } catch (err) {
    process.stderr.write(`usage error: ${err.message}\n`);
    return 2;
  }

  if (opts.help) {
    process.stdout.write(printHelp());
    return 0;
  }
  if (opts.version) {
    process.stdout.write(`${TOOL_VERSION}\n`);
    return 0;
  }
  if (opts.selfTest) {
    return runSelfTest();
  }

  try {
    if (opts.writeBaseline) {
      const result = analyze(opts);
      applyBaseline(result, null, '', opts, result.ctx);
      process.stderr.write(renderDiagnostics(result.ctx));
      if (result.ctx.diagnostics.some((d) => d.level === 'error')) {
        process.stderr.write('analysis produced error-level diagnostics; baseline not written\n');
        return 3;
      }
      writeBaseline(opts.writeBaseline, result, opts);
      // Recording is not a verdict, so this exits 0 even when cycles exist.
      process.stderr.write(
        `wrote baseline ${toPosix(opts.writeBaseline)}: ` +
          `${result.moduleCycles.length} module cycles, ${result.functionCycles.length} function cycles\n`,
      );
      return 0;
    }

    const { result, baselineInfo, exitCode } = runAnalysis(opts);
    process.stderr.write(renderDiagnostics(result.ctx));

    if (opts.emitGraph) {
      if (result.ctx.diagnostics.some((d) => d.level === 'error')) {
        // Refusing for the same reason --write-baseline refuses: a graph taken
        // from an incomplete analysis has holes, and a consumer would rank
        // modules over it and report a clean gate. The report is still printed,
        // deliberately: this run exits 3 with or without the flag, and
        // --emit-graph must not change the stdout of a failure it did not cause.
        // The two cases below are failures the flag itself introduced, and only
        // those suppress output.
        process.stderr.write('analysis produced error-level diagnostics; graph not written\n');
      } else {
        const payload = buildGraphPayload(result);
        const problems = checkGraphIntegrity(payload);
        if (problems.length > 0) {
          process.stderr.write(
            `graph integrity check failed: ${problems.length} closure violation(s) ` +
              `(check ${problems[0].check}, first: ${problems[0].from} -> ${problems[0].to}); ` +
              'graph not written\n',
          );
          return 3;
        }
        writeGraph(opts.emitGraph, payload); // throws AnalysisError -> caught below -> exit 3
        process.stderr.write(`wrote graph ${toPosix(opts.emitGraph)}: ${graphCounts(payload)}\n`);
      }
    }

    if (opts.json) {
      process.stdout.write(renderJson(result, baselineInfo, exitCode));
      return exitCode;
    }
    if (!opts.quiet) process.stdout.write(renderHuman(result, baselineInfo, exitCode));
    return exitCode;
  } catch (err) {
    if (err instanceof UsageError) {
      process.stderr.write(`usage error: ${err.message}\n`);
      return 2;
    }
    if (err instanceof AnalysisError) {
      process.stderr.write(`analysis error: ${err.message}\n`);
      return 3;
    }
    process.stderr.write(`internal error: ${err && err.message}\n`);
    if (err && err.stack) process.stderr.write(`${err.stack}\n`);
    return 3;
  }
}

/* ------------------------------------------------------------------ *
 * 26. Embedded self test
 * ------------------------------------------------------------------ */

const CRATE_ABC = 'mod a;\nmod b;\nmod c;\n';

const FIXTURES = [
  {
    name: 'F1 line comment is not a reference',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': '// use crate::b::T;\npub struct A;\n',
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.no(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge from a line comment');
    },
  },
  {
    name: 'F2 string literal is not a reference',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'pub fn f() -> &\'static str { "use crate::b::T;" }\n',
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.no(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge from a string literal');
    },
  },
  {
    name: 'F3 raw string is not a reference',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'pub fn f() -> &\'static str { r#"use crate::b::T;"# }\n',
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.no(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge from a raw string');
    },
  },
  {
    name: 'F4 nested block comment closes correctly',
    files: {
      'lib.rs': CRATE_ABC,
      'a.rs': '/* /* use crate::b::T; */ */\nuse crate::c::C;\npub struct A;\n',
      'b.rs': 'pub struct T;\n',
      'c.rs': 'pub struct C;\n',
    },
    run: [],
    assert(res, h) {
      h.no(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge from inside a nested comment');
      h.yes(hasModuleEdge(res, 'crate::a', 'crate::c'), 'a -> c edge after the nested comment');
    },
  },
  {
    name: 'F5 cfg(test) mod is excluded by default and included on demand',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': '#[cfg(test)]\nmod tests {\n    use crate::b::T;\n}\npub struct A;\n',
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.no(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge from #[cfg(test)] mod by default');
      const withTests = h.run(['--include-tests']);
      h.yes(hasModuleEdge(withTests.result, 'crate::a', 'crate::b'), 'a -> b edge under --include-tests');
    },
  },
  {
    name: 'F6 macro_rules body is skipped',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'macro_rules! m { () => { use crate::b::T; }; }\npub struct A;\n',
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.no(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge from inside a macro_rules body');
      h.atLeast(res.ctx.blindSpots.macroDefinitionsSkipped, 1, 'macroDefinitionsSkipped');
    },
  },
  {
    name: 'F7 foo.rs and foo/mod.rs resolve to the same module identity',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'use crate::b::T;\npub struct A;\n',
      'b/mod.rs': 'use crate::a::A;\npub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.eq(res.moduleCycles.length, 1, 'module cycle count');
      h.eq(res.moduleCycles[0].size, 2, 'cycle size');
      h.eqJson(res.moduleCycles[0].members, ['crate::a', 'crate::b'], 'cycle members');
    },
  },
  {
    name: 'F8 three module cycle with exact edge positions and #[path]',
    files: {
      'lib.rs': `${CRATE_ABC}#[path = "renamed.rs"]\nmod alias;\n`,
      'a.rs': 'use crate::b::B;\npub struct A;\n',
      'b.rs': 'use crate::c::C;\npub struct B;\n',
      'c.rs': 'use crate::a::A;\npub struct C;\n',
      'renamed.rs': 'pub struct Aliased;\n',
    },
    run: [],
    assert(res, h) {
      h.eq(res.moduleCycles.length, 1, 'module cycle count');
      const cycle = res.moduleCycles[0];
      h.eq(cycle.size, 3, 'cycle size');
      h.eqJson(cycle.members, ['crate::a', 'crate::b', 'crate::c'], 'cycle members');
      for (const edge of cycle.edges) {
        h.eq(edge.line, 1, `edge line in ${edge.file}`);
        h.eq(edge.column, 1, `edge column in ${edge.file}`);
      }
      h.yes(moduleIds(res).includes('crate::alias'), '#[path = "renamed.rs"] mod alias resolves');
    },
  },
  {
    name: 'F9 acyclic crate reports zero and exits 0',
    files: {
      'lib.rs': CRATE_ABC,
      'a.rs': 'use crate::b::B;\npub struct A;\n',
      'b.rs': 'use crate::c::C;\npub struct B;\n',
      'c.rs': 'pub struct C;\n',
    },
    run: [],
    assert(res, h, outcome) {
      h.eq(res.summary.moduleCycles, 0, 'summary.moduleCycles');
      h.eq(outcome.exitCode, 0, 'exit code');
    },
  },
  {
    name: 'F10 raw identifier does not swallow the file',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'use crate::b::T;\npub fn f(r#fn: u32) -> u32 { r#fn }\n',
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.yes(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge survives r#fn');
    },
  },
  {
    name: 'F11 lifetime is not an unterminated char literal',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': "pub fn f<'a>(x: &'a str) -> char { let _ = x; 'z' }\nuse crate::b::T;\n",
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      h.yes(hasModuleEdge(res, 'crate::a', 'crate::b'), 'a -> b edge survives a lifetime');
    },
  },
  {
    name: 'F12 baseline round trip',
    files: {
      'lib.rs': CRATE_ABC,
      'a.rs': 'use crate::b::B;\npub struct A;\n',
      'b.rs': 'use crate::c::C;\npub struct B;\n',
      'c.rs': 'use crate::a::A;\npub struct C;\n',
    },
    run: [],
    assert(res, h) {
      const baselinePath = path.join(h.dir, 'baseline.json');
      h.run(['--write-baseline', baselinePath]);
      const second = h.run(['--baseline', baselinePath]);
      h.eq(second.result.moduleCycles.length, 1, 'cycles after baseline');
      h.yes(second.result.moduleCycles.every((c) => c.status === 'known'), 'every cycle is known');
      h.eq(second.baselineInfo.newModuleCycles, 0, 'newModuleCycles');
      h.eq(second.exitCode, 0, 'exit code with a matching baseline');

      h.write('d.rs', 'use crate::e::E;\npub struct D;\n');
      h.write('e.rs', 'use crate::d::D;\npub struct E;\n');
      h.write('lib.rs', `${CRATE_ABC}mod d;\nmod e;\n`);
      const third = h.run(['--baseline', baselinePath]);
      h.eq(third.result.moduleCycles.filter((c) => c.status === 'new').length, 1, 'new cycles');
      h.eq(third.exitCode, 1, 'exit code with a new cycle');
    },
  },
  {
    name: 'F13 function cycles, scope split and recursion',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs':
        'pub fn f() { crate::b::g(); }\n' +
        'pub fn h() { i(); }\n' +
        'pub fn i() { h(); }\n' +
        'pub fn walk() { walk(); }\n',
      'b.rs': 'pub fn g() { crate::a::f(); }\n',
    },
    run: [],
    assert(res, h) {
      h.eq(res.functionCycles.length, 1, 'reported function cycles at default scope');
      h.eq(res.functionCycles[0].scope, 'cross-module', 'reported cycle scope');
      h.eqJson(res.functionCycles[0].members, ['crate::a::f', 'crate::b::g'], 'cross-module members');

      const all = h.run(['--function-scope', 'all']);
      const intra = all.result.functionCycles.filter((c) => c.scope === 'intra-module');
      h.eq(intra.length, 1, 'intra-module cycles under --function-scope all');
      h.eqJson(intra[0].members, ['crate::a::h', 'crate::a::i'], 'intra-module members');

      h.eqJson(res.recursion.map((r) => r.function), ['crate::a::walk'], 'recursion list');
      const inCycles = all.result.functionCycles.some((c) => c.members.includes('crate::a::walk'));
      h.no(inCycles, 'walk appears in functionCycles');
    },
  },
  {
    name: 'F14 blind spots are counted',
    files: {
      'lib.rs': 'mod a;\n',
      'a.rs':
        'pub struct Thing;\n' +
        'pub fn y() -> u32 { 1 }\n' +
        'pub fn f(x: &Thing) { x.clone(); println!("{}", y()); }\n',
    },
    run: [],
    assert(res, h) {
      h.atLeast(res.ctx.blindSpots.unresolvedMethodCalls, 1, 'unresolvedMethodCalls');
      h.atLeast(res.ctx.blindSpots.macroInvocationsSkipped, 1, 'macroInvocationsSkipped');
    },
  },
  {
    name: 'F15 posix relative paths and unreachable files',
    files: {
      'lib.rs': 'mod sub;\npub struct Root;\n',
      'sub/mod.rs': 'pub mod deep;\nuse crate::sub::deep::T;\npub fn touch(_t: &T) {}\n',
      'sub/deep.rs': 'use crate::Root;\npub struct T;\npub fn touch(_r: &Root) {}\n',
      'orphan.rs': 'use crate::sub::deep::T;\npub struct Orphan;\n',
    },
    run: [],
    assert(res, h) {
      const edges = res.moduleGraph.edges;
      h.atLeast(edges.length, 1, 'module edges');
      for (const edge of edges) {
        h.yes(/^[^\\]*$/.test(edge.file), `edge file is posix normalized: ${edge.file}`);
        h.no(path.isAbsolute(edge.file), `edge file is relative: ${edge.file}`);
        h.no(edge.file === 'orphan.rs', 'orphan.rs contributed an edge');
      }
      h.yes(res.ctx.blindSpots.unreachableFiles.includes('orphan.rs'), 'orphan.rs listed as unreachable');
    },
  },
  {
    // Pins the zero-consumed-segments rule: when no segment after the base names
    // a child module, the base itself is the target. A reference to an item is a
    // reference to the module holding it.
    name: 'F16 item reference resolves to the holding module',
    files: {
      'lib.rs': 'mod b;\npub struct Thing;\nuse crate::b::Other;\n',
      'b.rs': 'use super::Thing;\nuse crate::Thing;\npub struct Other;\n',
    },
    run: [],
    assert(res, h) {
      h.eq(res.moduleCycles.length, 1, 'module cycle count');
      const cycle = res.moduleCycles[0];
      h.eq(cycle.size, 2, 'cycle size');
      h.eqJson(cycle.members, ['crate', 'crate::b'], 'cycle members');
      const upward = cycle.edges.filter((e) => e.from === 'crate::b' && e.to === 'crate');
      h.eq(upward.length, 2, 'edge records from crate::b to crate, one per reference site');
      h.eq(res.ctx.stats.unresolvedInternalPaths, 0, 'unresolvedInternalPaths');
    },
  },
  {
    // The one case unresolvedInternalPaths exists for: the base itself does not
    // resolve, so no edge can be attributed.
    name: 'F17 super past the crate root resolves to nothing',
    files: {
      'lib.rs': 'mod b;\n',
      'b.rs': 'use super::super::Nope;\npub struct B;\n',
    },
    run: [],
    assert(res, h) {
      h.eq(res.moduleGraph.edges.length, 0, 'module edges');
      h.atLeast(res.ctx.stats.unresolvedInternalPaths, 1, 'unresolvedInternalPaths');
    },
  },
  {
    // Two `impl From<_>` blocks collapse to one node id. The real call graph
    // here is from<B> -> new -> from<A> -> helper, which closes no loop; before
    // the discarded body was excluded, its calls were charged to the surviving
    // node and manufactured a from <-> new cycle out of nothing.
    name: 'F18 a colliding definition is reported and never attributed',
    files: {
      'lib.rs': 'mod x;\n',
      'x.rs':
        'pub struct Foo;\npub struct A;\npub struct B;\n' +
        'impl From<A> for Foo { fn from(_v: A) -> Foo { helper() } }\n' +
        'impl From<B> for Foo { fn from(_v: B) -> Foo { Foo::new() } }\n' +
        'impl Foo { pub fn new() -> Foo { Foo::from(A) } }\n' +
        'pub fn helper() -> Foo { Foo }\n',
    },
    run: ['--fail-on', 'any', '--function-scope', 'all'],
    assert(res, h, outcome) {
      h.eq(res.allFunctionCycles.length, 0, 'function cycles fabricated by the collision');
      const dup = res.ctx.diagnostics.filter((d) => d.code === 'duplicate-function-node');
      h.eq(dup.length, 1, 'duplicate-function-node diagnostics');
      h.eq(dup[0].level, 'warn', 'duplicate-function-node level');
      h.eq(outcome.exitCode, 0, 'exit code on a crate with no real cycle');
    },
  },
  {
    name: 'F19 the use tree depth cap is reported, never silent',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': `use crate::b::${'{'.repeat(17)}T${'}'.repeat(17)};\npub struct A;\n`,
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      const hits = res.ctx.diagnostics.filter((d) => d.code === 'use-tree-depth-cap');
      h.atLeast(hits.length, 1, 'use-tree-depth-cap diagnostics');
      h.eq(hits[0].level, 'error', 'use-tree-depth-cap level');
    },
  },
  {
    name: 'F20 the item scan depth cap is reported, never silent',
    files: {
      'lib.rs':
        'mod b;\n' +
        Array.from({ length: 40 }, (_, i) => `mod m${i} {`).join('') +
        'use crate::b::T;' +
        '}'.repeat(40) +
        '\n',
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h, outcome) {
      const hits = res.ctx.diagnostics.filter((d) => d.code === 'scan-depth-cap');
      h.atLeast(hits.length, 1, 'scan-depth-cap diagnostics');
      h.eq(hits[0].level, 'error', 'scan-depth-cap level');
      h.eq(outcome.exitCode, 3, 'exit code on a scan depth cap hit');
    },
  },
  {
    name: 'F21 only cross-module function cycles gate',
    files: {
      'lib.rs': 'mod a;\n',
      'a.rs': 'pub fn h() { i(); }\npub fn i() { h(); }\n',
    },
    run: ['--fail-on', 'any', '--function-scope', 'all'],
    assert(res, h, outcome) {
      h.eq(res.functionCycles.length, 1, 'reported cycles under --function-scope all');
      h.eq(res.functionCycles[0].scope, 'intra-module', 'reported cycle scope');
      h.eq(outcome.exitCode, 0, 'an intra-module cycle must not gate');

      // The gate must still fire on a real cross-module cycle at the same flags.
      // --level function keeps the module graph out of it, so exit 1 can only
      // come from the function gate.
      h.write('lib.rs', 'mod a;\nmod c;\nmod d;\n');
      h.write('c.rs', 'pub fn f() { crate::d::g(); }\n');
      h.write('d.rs', 'pub fn g() { crate::c::f(); }\n');
      const gated = h.run(['--level', 'function', '--fail-on', 'any', '--function-scope', 'all']);
      h.eq(gated.exitCode, 1, 'a cross-module cycle must still gate');
    },
  },
  {
    name: 'F22 an empty inline flag value is a usage error',
    files: {
      'lib.rs': 'pub struct A;\n',
    },
    run: [],
    assert(res, h) {
      const rejects = (extra) => {
        try {
          h.run(extra);
          return false;
        } catch (err) {
          return err instanceof UsageError;
        }
      };
      h.yes(rejects(['--write-baseline=']), '--write-baseline= rejected');
      h.yes(rejects(['--baseline=']), '--baseline= rejected');
      h.yes(rejects(['--baseline=', '--write-baseline=x']), '--baseline= with --write-baseline=x rejected');
      h.yes(rejects(['--exclude=']), '--exclude= rejected');
    },
  },
  {
    // The blind-spot tally must never shrink because the tool saw less. A call
    // site inside a discarded body produces no edge, but it is exactly the kind
    // of thing "what this analysis could NOT see" is counting, so it stays in
    // the tally. The property is monotonicity, not any particular number.
    name: 'F23 a discarded definition still counts its call sites',
    files: {
      'lib.rs': 'mod a;\n',
      'a.rs':
        'pub struct T;\n' +
        '#[cfg(unix)]\npub fn twin(x: &T) { x.one(); x.two(); missing_a(); missing_b(); }\n' +
        '#[cfg(windows)]\npub fn twin(x: &T) { x.three(); missing_c(); }\n',
    },
    run: [],
    assert(res, h) {
      h.atLeast(
        res.ctx.diagnostics.filter((d) => d.code === 'duplicate-function-node').length,
        1,
        'collision is present',
      );
      const collided = {
        method: res.ctx.blindSpots.unresolvedMethodCalls,
        bare: res.ctx.blindSpots.unresolvedBareCalls,
      };

      // The same code with the second definition renamed: nothing collides and
      // nothing is discarded, so the tool sees strictly more. The tally from the
      // run where it saw less must not be the smaller of the two.
      h.write(
        'a.rs',
        'pub struct T;\n' +
          '#[cfg(unix)]\npub fn twin(x: &T) { x.one(); x.two(); missing_a(); missing_b(); }\n' +
          '#[cfg(windows)]\npub fn twin_renamed(x: &T) { x.three(); missing_c(); }\n',
      );
      const full = h.run([]);
      h.eq(
        full.result.ctx.diagnostics.filter((d) => d.code === 'duplicate-function-node').length,
        0,
        'no collision after the rename',
      );
      h.atLeast(collided.method, full.result.ctx.blindSpots.unresolvedMethodCalls, 'unresolvedMethodCalls');
      h.atLeast(collided.bare, full.result.ctx.blindSpots.unresolvedBareCalls, 'unresolvedBareCalls');
    },
  },
  {
    // A dropped reference can delete the cycle that depended on it, so a cap hit
    // must not be able to end in a green gate.
    name: 'F24 a depth cap hit forces exit 3',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': `use crate::b::${'{'.repeat(17)}T${'}'.repeat(17)};\npub struct A;\n`,
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h, outcome) {
      h.eq(res.summary.moduleCycles, 0, 'the dropped reference erased the cycle');
      h.eq(outcome.exitCode, 3, 'exit code must not be 0 on a provably incomplete analysis');
    },
  },
  {
    // The three shapes the item rule covers with no special case: a path that
    // stops at an item, a path that names a module, and a qualified path written
    // inline whose first unconsumed segment is a type.
    name: 'F25 item is the first segment module resolution did not consume',
    files: {
      'lib.rs':
        'mod a;\n' +
        'use crate::a::b::C;\n' +
        'use crate::a::b;\n' +
        'pub fn f() -> u32 { crate::a::b::T::SIZE }\n',
      'a/mod.rs': 'mod b;\n',
      'a/b.rs': 'pub struct C;\npub struct T;\n',
    },
    run: [],
    assert(res, h) {
      const edges = res.moduleGraph.edges;
      h.eq(edges.length, 3, 'module edge records');
      h.no(edges.some((e) => e.to === 'crate::a'), 'mod a; is a declaration, not a reference');
      h.eqJson(
        edges.map((e) => [e.from, e.to, e.kind, e.item]),
        [
          ['crate', 'crate::a::b', 'use', 'C'],
          ['crate', 'crate::a::b', 'use', null],
          ['crate', 'crate::a::b', 'path', 'T'],
        ],
        'from, to, kind and item of every record',
      );
    },
  },
  {
    // F16's tree exactly. Zero consumed segments is a normal outcome, not a
    // special case: the base is the answer and the leftover is the item.
    name: 'F26 zero consumed segments still names an item',
    files: {
      'lib.rs': 'mod b;\npub struct Thing;\nuse crate::b::Other;\n',
      'b.rs': 'use super::Thing;\nuse crate::Thing;\npub struct Other;\n',
    },
    run: [],
    assert(res, h) {
      const upward = res.moduleGraph.edges.filter((e) => e.from === 'crate::b' && e.to === 'crate');
      h.eq(upward.length, 2, 'edge records from crate::b to crate');
      h.eq(upward.filter((e) => e.item === 'Thing').length, 2, 'both carry item Thing');
      const downward = res.moduleGraph.edges.filter((e) => e.from === 'crate' && e.to === 'crate::b');
      h.eq(downward.length, 1, 'edge records from crate to crate::b');
      h.eq(downward[0].item, 'Other', 'item of the crate -> crate::b record');
    },
  },
  {
    // A glob over a named item attributes that item. The deliberate asymmetry:
    // both records are use-glob and bring unknown names into scope, so a
    // consumer distinguishes them by kind, never by item.
    name: 'F27 a glob over a named item does attribute that item',
    files: {
      'lib.rs': 'mod a;\nuse crate::a::*;\nuse crate::a::MyEnum::*;\n',
      'a.rs': 'pub enum MyEnum { X }\n',
    },
    run: [],
    assert(res, h) {
      const globs = res.moduleGraph.edges.filter((e) => e.kind === 'use-glob');
      h.eq(globs.length, 2, 'use-glob records');
      h.eq(globs[0].item, null, 'use crate::a::* names no item');
      h.eq(globs[1].item, 'MyEnum', 'use crate::a::MyEnum::* names MyEnum');
      h.atLeast(res.ctx.blindSpots.globImports, 2, 'globImports');
    },
  },
  {
    // Writing g.json into the scanned directory is safe only because
    // findRustFiles collects .rs files and Cargo.toml and nothing else, so the
    // artefact cannot become part of the tree it describes. If that walk ever
    // widens, the second run stops being a run over an unchanged tree and the
    // byte-identity assertion becomes wrong for a reason nobody will look for.
    name: 'F28 the dump round trips and two runs are byte identical',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'use crate::b::T;\npub struct A;\npub fn f() { crate::b::g(); }\n',
      'b.rs': 'pub struct T;\npub fn g() {}\n',
    },
    run: [],
    assert(res, h) {
      const first = path.join(h.dir, 'g.json');
      h.run(['--emit-graph', first]);
      h.yes(fs.existsSync(first), 'graph file written');
      const graph = JSON.parse(fs.readFileSync(first, 'utf8'));
      h.eq(graph.kind, 'dependency-graph', 'kind');
      h.eq(graph.schemaVersion, 1, 'schemaVersion');
      h.eq(graph.language, 'rust', 'language');
      h.eq(graph.modules.length, graph.summary.modulesResolved, 'modules against summary');
      h.eq(graph.edges.length, graph.summary.moduleEdges, 'edges against summary');
      h.yes('functions' in graph, 'functions present');
      h.yes('functionEdges' in graph, 'functionEdges present');
      h.yes('blindSpots' in graph, 'blindSpots present');
      h.yes('diagnostics' in graph, 'diagnostics present');
      const ids = new Set(graph.modules.map((m) => m.id));
      h.yes(graph.edges.every((e) => ids.has(e.to)), 'every edge target is a node');
      const second = path.join(h.dir, 'g2.json');
      h.run(['--emit-graph', second]);
      h.eq(fs.readFileSync(second, 'utf8'), fs.readFileSync(first, 'utf8'), 'two runs byte identical');
    },
  },
  {
    // Absence, not emptiness. An empty array reads as "computed, and there are
    // none", which is the zero-as-proof-of-absence this tool refuses.
    name: 'F29 a level that built no section omits its keys',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'use crate::b::T;\npub fn f() { crate::b::g(); }\n',
      'b.rs': 'pub struct T;\npub fn g() {}\n',
    },
    run: [],
    assert(res, h) {
      const read = (name, extra) => {
        const p = path.join(h.dir, name);
        h.run(['--emit-graph', p, ...extra]);
        return JSON.parse(fs.readFileSync(p, 'utf8'));
      };
      const all = read('ga.json', ['--level', 'all']);
      h.yes('modules' in all && 'edges' in all, 'module sections present at --level all');
      h.yes('functions' in all && 'functionEdges' in all, 'function sections present at --level all');
      const mod = read('gm.json', ['--level', 'module']);
      h.eq('functions' in mod, false, 'functions absent at --level module');
      h.eq('functionEdges' in mod, false, 'functionEdges absent at --level module');
      h.yes('modules' in mod, 'modules present at --level module');
      h.eq(mod.target.level, 'module', 'target.level records the cause');
      const fn = read('gf.json', ['--level', 'function']);
      h.eq('modules' in fn, false, 'modules absent at --level function');
      h.eq('edges' in fn, false, 'edges absent at --level function');
      h.yes('functions' in fn, 'functions present at --level function');
      h.eq(fn.target.level, 'function', 'target.level records the cause');
    },
  },
  {
    name: 'F30 --emit-graph is exclusive with --write-baseline and needs a value',
    files: {
      'lib.rs': 'pub struct A;\n',
    },
    run: [],
    assert(res, h) {
      const rejects = (extra) => {
        try {
          h.run(extra);
          return false;
        } catch (err) {
          return err instanceof UsageError;
        }
      };
      h.yes(
        rejects(['--emit-graph=g.json', '--write-baseline=b.json']),
        '--emit-graph with --write-baseline rejected',
      );
      h.yes(rejects(['--emit-graph=']), '--emit-graph= rejected');
      // The combination below is legal at parse time; only --write-baseline is
      // exclusive. It is asserted through parseArgs directly because h.run also
      // calls runAnalysis, which throws UsageError out of loadBaseline on the
      // missing b.json, so the rejects() idiom would pass for the wrong reason.
      const parsed = parseArgs([h.dir, '--emit-graph=g.json', '--baseline=b.json']);
      h.eq(parsed.emitGraph, 'g.json', '--emit-graph value survives alongside --baseline');
      h.eq(parsed.baseline, 'b.json', '--baseline value survives alongside --emit-graph');
      // The default is load-bearing: a non-null one would make every run try to
      // write a graph to a path nobody asked for.
      h.eq(parseArgs([h.dir]).emitGraph, null, 'emitGraph defaults to null');
    },
  },
  {
    // F24's tree: the use-tree depth cap makes the analysis incomplete, and a
    // graph taken from an incomplete analysis would let a consumer rank modules
    // over a graph with holes and report a clean gate.
    name: 'F31 an incomplete analysis writes no graph',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': `use crate::b::${'{'.repeat(17)}T${'}'.repeat(17)};\npub struct A;\n`,
      'b.rs': 'pub struct T;\n',
    },
    run: [],
    assert(res, h) {
      const p = path.join(h.dir, 'g.json');
      const outcome = h.run(['--emit-graph', p]);
      h.eq(outcome.exitCode, 3, 'exit 3');
      h.eq(fs.existsSync(p), false, 'no graph file from an incomplete analysis');
      // Positive control: the same tree without the cap hit does write one, so
      // the absence above is the refusal and not a missing feature. Without it
      // this fixture passes against a build where --emit-graph does nothing.
      h.write('a.rs', 'use crate::b::T;\npub struct A;\n');
      h.run(['--emit-graph', p]);
      h.yes(fs.existsSync(p), 'the same tree writes a graph once the analysis is complete');
      // And through the CLI, which is the only place the refusal is decided.
      // h.run carries its own copy of the gate, so without this pass every
      // mutant inside main()'s --emit-graph block survives structurally.
      h.write('a.rs', `use crate::b::${'{'.repeat(17)}T${'}'.repeat(17)};\npub struct A;\n`);
      const q = path.join(h.dir, 'g2.json');
      const r = h.main(['--emit-graph', q, '--quiet']);
      h.eq(r.code, 3, 'main() exits 3 on an incomplete analysis');
      h.eq(fs.existsSync(q), false, 'and writes nothing');
      h.yes(
        /analysis produced error-level diagnostics; graph not written/.test(r.stderr),
        'and says so',
      );
    },
  },
  {
    // The three inline sites take the three different paths through
    // resolvePathDetailed: self::Deep reaches the fold branch, because its base
    // crate::a::inner is not a graph node; super::Outer takes the normal super
    // path; self::inner::Deep takes the unconsumed-inline-module path. All three
    // land on crate::a and are dropped as self-edges.
    name: 'F32 the inline fold never leaks an unattributed item',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'b.rs': 'pub struct T;\n',
      'a.rs':
        'use crate::b::T;\n' +
        'pub struct Outer;\n' +
        'mod inner { pub struct Deep; pub fn f(_x: &super::Outer) {} pub fn h(_x: &self::Deep) {} }\n' +
        'pub fn g(_x: &self::inner::Deep) {}\n',
    },
    run: [],
    assert(res, h) {
      const edges = res.moduleGraph.edges;
      h.eq(edges.length, 1, 'module edge records');
      h.eq(edges[0].from, 'crate::a', 'the surviving edge is from crate::a');
      h.eq(edges[0].to, 'crate::b', 'the surviving edge is to crate::b');
      h.eq(edges[0].item, 'T', 'the surviving edge carries item T');
      h.no(edges.some((e) => e.from === e.to), 'no self-edge survives');
      const nodes = new Set(res.moduleGraph.nodes);
      h.yes(edges.every((e) => e.item !== null || nodes.has(e.to)), 'a null item still points at a node');
      // Reachability. Without this the fixture passes whether the fold fired and
      // was dropped or was never entered at all, and it is the only coverage the
      // fold has. crate::a::inner being no node is exactly what makes the base
      // unresolvable and forces the fold.
      h.eq(res.ctx.stats.unresolvedInternalPaths, 0, 'unresolvedInternalPaths');
      const ids = moduleIds(res);
      h.yes(ids.includes('crate::a'), 'crate::a is a module node');
      h.no(ids.includes('crate::a::inner'), 'the inline mod is not a module node');
    },
  },
  {
    // The shape that made the reachability form of the item rule false: a
    // file-backed `mod inner;` beside an inline `mod inner { ... }` puts a graph
    // node strictly between the file module and the base, so the fold lands on
    // it, the edge is not a self-edge, and it survives. Before the fold reported
    // what it dropped, this record carried item: null over a segment the tool
    // had in its hand. rustc rejects two live `mod inner`, but these sit on
    // mutually exclusive cfg branches, which this tool traverses on purpose.
    name: 'F33 a folded resolution never emits a null item over a dropped segment',
    files: {
      'lib.rs': 'mod a;\n',
      'a/inner.rs': 'pub struct FromFile;\n',
      'a.rs':
        '#[cfg(windows)]\nmod inner;\n\n' +
        '#[cfg(not(windows))]\nmod inner {\n    pub mod deep {\n        use self::Zzz;\n    }\n}\n',
    },
    run: [],
    assert(res, h) {
      const edges = res.moduleGraph.edges;
      h.eq(edges.length, 1, 'module edge records');
      h.eq(edges[0].from, 'crate::a', 'edge from');
      h.eq(edges[0].to, 'crate::a::inner', 'edge to');
      h.eq(edges[0].kind, 'use', 'edge kind');
      h.eq(edges[0].item, 'Zzz', 'the dropped segment is the item');
      // Reachability, F32's shape: without it this fixture passes whether the
      // tree still enters the fold or was rerouted down the normal path.
      const ids = moduleIds(res);
      h.yes(ids.includes('crate::a::inner'), 'the file-backed inner module is a node');
      h.yes(ids.includes('crate::a'), 'crate::a is a node');
    },
  },
  {
    // The naive `rest` regression fabricates a call edge. `self::` on an
    // intra-module call is ordinary Rust: it names the module's own item when a
    // `use` has brought a same-named item into scope, and macro expansions emit
    // it because they cannot know what the call site imported. Do NOT add the
    // disambiguating `use`: it would add a module edge and break the third
    // assertion below.
    name: 'F34 the inline fold never fabricates a call edge',
    files: {
      'lib.rs': 'mod a;\n',
      'a.rs': 'mod inner {\n    pub fn helper() {}\n    pub fn caller() { self::helper(); }\n}\n',
    },
    run: [],
    assert(res, h) {
      h.eq(res.callGraph.edges.length, 0, 'function edge records');
      h.eq(res.moduleGraph.edges.length, 0, 'module edge records');
      const fns = Array.from(res.callGraph.nodes.keys());
      h.yes(fns.includes('crate::a::caller'), 'caller is a function node');
      h.yes(fns.includes('crate::a::helper'), 'helper is a function node');
      const ids = moduleIds(res);
      h.yes(ids.includes('crate::a'), 'crate::a is a module node');
      h.no(ids.includes('crate::a::inner'), 'the inline mod is not a module node');
    },
  },
  {
    // Through the CLI, not through renderJson directly: --json reaches
    // renderJson on every run, so a direct call would give up main()'s own call
    // site for nothing. The rule is: call a function directly only where no CLI
    // invocation can reach the property, which is why F38 and F30 keep theirs.
    //
    // The two imports ARE the module cycle, so this fixture guards its own tree:
    // `remove the whole use item`, which rustc offers on both, fails the first
    // assertion at the moment of the edit.
    name: 'F35 moduleCycles edges carry item, through the CLI',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'use crate::b::T;\npub struct A;\n',
      'b.rs': 'use crate::a::A;\npub struct T;\n',
    },
    run: [],
    assert(res, h) {
      const r = h.main(['--json']);
      h.eq(r.code, 1, 'exit 1, the tree is a module cycle');
      h.eq(r.stderr, '', 'nothing on stderr');
      let payload = null;
      try {
        payload = JSON.parse(r.stdout);
      } catch {
        h.yes(false, 'stdout parses as JSON; got ' + JSON.stringify(r.stdout.slice(0, 200)));
      }
      if (!payload) return;
      h.eq(payload.moduleCycles.length, 1, 'module cycles');
      const recs = payload.moduleCycles[0].edges;
      h.eqJson(
        recs.map((e) => [e.from, e.to, e.item]),
        [['crate::a', 'crate::b', 'T'], ['crate::b', 'crate::a', 'A']],
        'cycle edge from, to and item',
      );
      h.eqJson(
        Object.keys(recs[0]),
        ['from', 'to', 'item', 'file', 'line', 'column', 'kind', 'cfgGated', 'text'],
        'cycle edge record key order',
      );
    },
  },
  {
    // The brace list in a.rs is UNSORTED ON PURPOSE. Sorting it makes emission
    // order equal sorted order, the item tiebreak stops being observable, and
    // this fixture keeps passing. The first assertion is what makes that loud.
    name: 'F36 the dump records, their key order and their sort order',
    files: {
      'lib.rs': 'mod a;\nmod alpha;\nmod zeta;\nuse crate::zeta::Z;\n',
      'a.rs': 'use crate::zeta::{Z, Y};\nuse crate::alpha::A;\n',
      'alpha.rs': 'pub struct A;\n',
      'zeta.rs': 'pub struct Z;\npub struct Y;\n',
    },
    run: [],
    assert(res, h) {
      h.eqJson(
        res.moduleGraph.edges.filter((e) => e.from === 'crate::a' && e.to === 'crate::zeta').map((e) => e.item),
        ['Z', 'Y'],
        'the brace list is unsorted, so emission order is Z then Y',
      );
      const p = path.join(h.dir, 'g.json');
      h.run(['--emit-graph', p]);
      const raw = fs.readFileSync(p, 'utf8');
      const g = JSON.parse(raw);
      h.eqJson(
        Object.keys(g),
        ['schemaVersion', 'kind', 'language', 'tool', 'target', 'crates', 'summary',
         'modules', 'edges', 'functions', 'functionEdges', 'blindSpots', 'diagnostics'],
        'top level key order',
      );
      h.eqJson(g.tool, { name: 'rust-module-dependency-cycles', version: TOOL_VERSION }, 'tool block');
      h.eqJson(
        Object.keys(g.target),
        ['rootPath', 'crateDiscovery', 'level', 'functionScope', 'includeTests', 'excludes'],
        'target key order',
      );
      h.eq(g.target.rootPath, toPosix(h.dir), 'target.rootPath');
      h.eq(g.target.crateDiscovery, 'fallback', 'target.crateDiscovery');
      h.eq(g.target.level, 'all', 'target.level');
      h.eq(g.target.functionScope, 'cross-module', 'target.functionScope');
      h.eq(g.target.includeTests, false, 'target.includeTests');
      h.eqJson(g.target.excludes, [], 'target.excludes');
      h.eqJson(
        g.crates,
        [{ targetId: 'crate', kind: 'lib', packageName: null, manifestPath: null,
           rootFile: 'lib.rs', edition: '2021', modules: 4, files: 4 }],
        'crates',
      );
      h.eqJson(Object.keys(g.modules[0]), ['id', 'parent', 'file', 'crateTarget', 'loc'], 'module key order');
      h.eqJson(
        g.modules.map((m) => [m.id, m.parent, m.file, m.crateTarget, m.loc]),
        [['crate', null, 'lib.rs', 'crate', 4],
         ['crate::a', 'crate', 'a.rs', 'crate', 2],
         ['crate::alpha', 'crate', 'alpha.rs', 'crate', 1],
         ['crate::zeta', 'crate', 'zeta.rs', 'crate', 2]],
        'module records',
      );
      h.eqJson(
        Object.keys(g.edges[0]),
        ['from', 'to', 'item', 'file', 'line', 'column', 'kind', 'cfgGated', 'text'],
        'edge key order',
      );
      h.eqJson(
        g.edges.map((e) => [e.from, e.to, e.item, e.file, e.line, e.column, e.kind, e.cfgGated, e.text]),
        [['crate', 'crate::zeta', 'Z', 'lib.rs', 4, 1, 'use', false, 'use crate::zeta::Z;'],
         ['crate::a', 'crate::alpha', 'A', 'a.rs', 2, 1, 'use', false, 'use crate::alpha::A;'],
         ['crate::a', 'crate::zeta', 'Y', 'a.rs', 1, 1, 'use', false, 'use crate::zeta::{Z, Y}'],
         ['crate::a', 'crate::zeta', 'Z', 'a.rs', 1, 1, 'use', false, 'use crate::zeta::{Z, Y}']],
        'edge records, in dump order',
      );
      h.eqJson(
        Object.keys(g.blindSpots),
        ['unresolvedMethodCalls', 'unresolvedBareCalls', 'macroInvocationsSkipped',
         'macroDefinitionsSkipped', 'callsOutsideFunctionBodies', 'globImports',
         'ambiguousBarePaths', 'unresolvedInternalPaths', 'externalReferences',
         'barePathResolutions', 'unreachableFiles', 'unsupportedPathAttributes',
         'inlineModuleFileDeclarations', 'notes'],
        'blindSpots key order',
      );
      h.eqJson(g.diagnostics, [], 'diagnostics');
      h.yes(raw.endsWith('\n') && !raw.endsWith('\n\n'), 'exactly one trailing newline');
      h.yes(raw.includes('\n  "kind": "dependency-graph"'), 'two space indentation');
    },
  },
  {
    // The one reachable closure hole, filed as #16: non-test code referencing a
    // #[cfg(test)] mod file module. It does not compile, and this tool does not
    // require the analysed project to compile: a half finished refactor is
    // exactly when someone runs a dependency tool.
    name: 'F37 the integrity gate refuses through main and names the check',
    files: {
      'lib.rs': '#[cfg(test)]\nmod t;\npub fn f() -> u32 { crate::t::VALUE }\n',
      't.rs': 'pub const VALUE: u32 = 1;\n',
    },
    run: [],
    assert(res, h) {
      const p = path.join(h.dir, 'g.json');
      const r = h.main(['--emit-graph', p, '--quiet']);
      h.eq(r.code, 3, 'exit 3');
      h.eq(fs.existsSync(p), false, 'no graph file');
      h.yes(/graph integrity check failed/.test(r.stderr), 'stderr reports the failure');
      h.yes(/check 1/.test(r.stderr), 'stderr names which check');
      h.yes(/crate -> crate::t/.test(r.stderr), 'stderr names the first offender');
      h.eq(r.stdout, '', 'nothing on stdout under --quiet');
      // Positive control, and it also pins the confirmation line, which is the
      // only place graphCounts is observable.
      const q = path.join(h.dir, 'g2.json');
      const ok = h.main(['--emit-graph', q, '--quiet', '--include-tests']);
      h.eq(ok.code, 0, 'the same tree exits 0 once the target is a node');
      h.yes(fs.existsSync(q), 'and writes a graph');
      h.yes(
        /wrote graph .*: 2 modules, 1 edges, 1 functions, 0 function edges/.test(ok.stderr),
        'the confirmation line carries every section count',
      );
    },
  },
  {
    // checkGraphIntegrity is a pure predicate over the published schema, so it
    // is tested directly, the way F30 tests parseArgs directly. Checks 2, 3 and
    // 4 hold by construction today and no Rust tree can make them fire; the
    // alternative to this fixture is three mutants nothing can reach.
    name: 'F38 the four closure checks fire, name themselves, and gate by section',
    files: { 'lib.rs': 'pub struct A;\n' },
    run: [],
    assert(res, h) {
      const only = (payload) => {
        const problems = checkGraphIntegrity(payload);
        return problems.length === 1 ? problems[0].check : problems.length;
      };
      const mods = [{ id: 'crate', parent: null }];
      h.eq(only({ modules: mods, edges: [{ from: 'crate', to: 'crate::ghost' }] }), 1, 'check 1');
      h.eq(only({ modules: [...mods, { id: 'crate::a', parent: 'crate::ghost' }], edges: [] }), 2, 'check 2');
      h.eq(only({ functions: [{ id: 'crate::f', moduleId: 'crate' }],
                  functionEdges: [{ from: 'crate::f', to: 'crate::ghost' }] }), 3, 'check 3');
      h.eq(only({ modules: mods, edges: [], functions: [{ id: 'crate::f', moduleId: 'crate::ghost' }],
                  functionEdges: [] }), 4, 'check 4');
      h.eqJson(checkGraphIntegrity({ modules: mods, edges: [], functions: [], functionEdges: [] }), [],
        'a closed payload has no problems');
      // Section gating: a payload with no modules key must not run the module
      // checks, and a payload with no functions key must not run the function
      // ones. Absent is not empty here either.
      h.eqJson(checkGraphIntegrity({ functions: [{ id: 'crate::f', moduleId: 'crate::ghost' }],
                                     functionEdges: [] }), [],
        'check 4 is skipped when modules is absent');
      h.eqJson(checkGraphIntegrity({ modules: mods, edges: [{ from: 'crate', to: 'crate::ghost' }] }).map((x) => x.check),
        [1], 'the function checks are skipped when functions is absent');
      h.eqJson(checkGraphIntegrity({ functions: [{ id: 'crate::f', moduleId: 'crate' }],
                                     functionEdges: [{ from: 'crate::f', to: 'crate::ghost' }] }).map((x) => x.check),
        [3], 'the module checks are skipped when modules is absent');
    },
  },
  {
    // A mod declaration whose file is not there is what a tree looks like mid
    // refactor. It is warn level, so the graph is still written and the record
    // has to reach the dump.
    name: 'F39 a warn diagnostic still writes a graph and reaches the dump',
    files: {
      'lib.rs': 'mod missing;\nmod b;\npub fn f() { crate::b::g(); }\npub fn h(_x: &super::Thing) {}\n',
      'b.rs': 'pub fn g() {}\n',
    },
    run: [],
    assert(res, h) {
      const p = path.join(h.dir, 'g.json');
      const r = h.main(['--emit-graph', p, '--quiet']);
      h.eq(r.code, 0, 'a warn does not stop the run');
      h.yes(fs.existsSync(p), 'the graph is written');
      const g = JSON.parse(fs.readFileSync(p, 'utf8'));
      h.eq(g.diagnostics.length, 1, 'one diagnostic in the dump');
      h.eq(g.diagnostics[0].level, 'warn', 'level');
      h.eq(g.diagnostics[0].code, 'unresolved-mod-declaration', 'code');
      // `super::` written at the crate root resolves to nothing, and it is the
      // only shape that drives a null through the QUALIFIED PATH branch of
      // buildModuleGraph: the branch only ever sees paths anchored at crate,
      // super or self, and of those only super can walk past the root. Without
      // its `if (!detailed) continue` the run throws instead of skipping, so
      // this counter is what keeps that guard exercised. Same class of tree as
      // F17 and F37: it does not compile, and this tool does not require the
      // analysed project to compile. rustc reports E0583 on the missing module
      // and E0433 on the super:: path, and offers no machine-applicable fix for
      // either, so nothing in the toolchain will rewrite this tree.
      h.atLeast(g.blindSpots.unresolvedInternalPaths, 1, 'unresolvedInternalPaths');
      h.no(g.edges.some((e) => e.item === 'Thing'), 'the unresolvable path contributes no edge');
    },
  },
  {
    // The call order inside caller() is an assertion input. buildCallGraph runs
    // one regex pass per call kind, so the path-calls emit before the type-call
    // whatever their line order; the dump then sorts by callee, and Zed sorts
    // before both lowercase names. Reorder the three calls, or rename Zed, and
    // emission order becomes the sorted order: the functionEdges sort stops
    // being observable and this fixture keeps passing. The first assertion is
    // the precondition that makes such an edit loud.
    name: 'F40 function records and function edge order',
    files: {
      'lib.rs': 'mod a;\nmod b;\n',
      'a.rs': 'use crate::b::Zed;\npub fn caller() {\n    crate::b::alpha();\n    Zed::mk();\n    crate::b::omega();\n}\n',
      'b.rs': 'pub struct Zed;\nimpl Zed { pub fn mk() -> Zed { Zed } }\npub fn alpha() {}\npub fn omega() {}\n',
    },
    run: [],
    assert(res, h) {
      h.eqJson(
        res.callGraph.edges.map((e) => e.to),
        ['crate::b::alpha', 'crate::b::omega', 'crate::b::Zed::mk'],
        'emission order is by call kind, so it is not the sorted order',
      );
      const p = path.join(h.dir, 'g.json');
      h.run(['--emit-graph', p]);
      const g = JSON.parse(fs.readFileSync(p, 'utf8'));
      h.eqJson(Object.keys(g.functions[0]), ['id', 'name', 'ownerType', 'moduleId', 'file', 'line'],
        'function key order');
      h.eqJson(
        g.functions.map((f) => [f.id, f.name, f.ownerType, f.moduleId, f.file, f.line]),
        [['crate::a::caller', 'caller', null, 'crate::a', 'a.rs', 2],
         ['crate::b::Zed::mk', 'mk', 'Zed', 'crate::b', 'b.rs', 2],
         ['crate::b::alpha', 'alpha', null, 'crate::b', 'b.rs', 3],
         ['crate::b::omega', 'omega', null, 'crate::b', 'b.rs', 4]],
        'function records',
      );
      h.eqJson(
        g.functionEdges.map((e) => [e.from, e.to, e.kind, e.line, e.column]),
        [['crate::a::caller', 'crate::b::Zed::mk', 'type-call', 4, 5],
         ['crate::a::caller', 'crate::b::alpha', 'path-call', 3, 5],
         ['crate::a::caller', 'crate::b::omega', 'path-call', 5, 5]],
        'function edge records, in dump order',
      );
    },
  },
  {
    name: 'F41 a write failure is reported, never swallowed',
    files: { 'lib.rs': 'mod a;\n', 'a.rs': 'pub struct A;\n' },
    run: [],
    assert(res, h) {
      const p = path.join(h.dir, 'no-such-dir', 'g.json');
      const r = h.main(['--emit-graph', p, '--quiet']);
      h.eq(r.code, 3, 'exit 3');
      h.eq(fs.existsSync(p), false, 'no graph file');
      h.yes(/cannot write graph file/.test(r.stderr), 'stderr reports the write failure');
      h.yes(r.stderr.includes('no-such-dir'), 'and names the path');
    },
  },
  {
    // Every line here is an assertion input, and the tree COMPILES: {B, *} is
    // the glob partner rather than {A, *}, because {A, *} beside {A, self}
    // imports A twice and rustc rejects it. That matters beyond tidiness: on a
    // non-compiling tree `cargo fix` offers a machine-applicable "remove the
    // whole use item" and deletes the lines this fixture is made of.
    //
    // The leaf order inside {A, self} is UNSORTED ON PURPOSE: it makes emission
    // order differ from sorted order, {self, A} would not, and the item
    // tiebreak would stop being observable. The first assertion is the
    // precondition, so any tidy-up fails the fixture at the moment of the edit.
    name: 'F42 every tiebreak of the dump comparator is exercised',
    files: {
      'lib.rs': 'mod a;\nmod b;\nuse crate::b::Z;\n',
      'a.rs': 'use crate::b::{B, *};\nuse crate::b::{A, self};\nuse crate::b::Z; use crate::b::Y;\npub fn zulu() {}\npub fn alpha() {}\n',
      'b.rs': 'pub struct A;\npub struct B;\npub struct Y;\npub struct Z;\n',
    },
    run: [],
    assert(res, h) {
      h.eqJson(
        res.moduleGraph.edges.filter((e) => e.line === 2).map((e) => e.item),
        ['A', null],
        'the {A, self} leaves emit in source order, A then self',
      );
      const p = path.join(h.dir, 'g.json');
      h.run(['--emit-graph', p]);
      const g = JSON.parse(fs.readFileSync(p, 'utf8'));
      h.eqJson(
        g.edges.map((e) => [e.from, e.to, e.item, e.kind, e.line, e.column]),
        [['crate', 'crate::b', 'Z', 'use', 3, 1],
         ['crate::a', 'crate::b', 'B', 'use', 1, 1],
         ['crate::a', 'crate::b', null, 'use-glob', 1, 1],
         ['crate::a', 'crate::b', null, 'use', 2, 1],
         ['crate::a', 'crate::b', 'A', 'use', 2, 1],
         ['crate::a', 'crate::b', 'Z', 'use', 3, 1],
         ['crate::a', 'crate::b', 'Y', 'use', 3, 18]],
        'every tiebreak below from and to, in dump order',
      );
      h.eqJson(
        g.functions.map((f) => f.id),
        ['crate::a::alpha', 'crate::a::zulu'],
        'functions are sorted, and zulu is declared first',
      );
    },
  },
  {
    // An empty module file and a file with no trailing newline. Both are
    // ordinary and between them they pin lineCountOf's two branches.
    //
    // one.rs having NO trailing newline is an assertion input, and it is the
    // only input in the whole suite that lacks one. Every editor and rustfmt
    // add it back; doing so leaves this fixture green and byte-identical while
    // one.rs silently stops exercising the endsWith('\n') === false branch.
    // The first two assertions are the precondition that makes that edit loud.
    name: 'F43 loc counts an empty file as zero and an unterminated file as one',
    files: {
      'lib.rs': 'mod empty;\nmod one;\n',
      'empty.rs': '',
      'one.rs': 'pub struct A;',
    },
    run: [],
    assert(res, h) {
      h.eq(fs.readFileSync(path.join(h.dir, 'empty.rs'), 'utf8'), '', 'empty.rs is empty');
      h.no(
        fs.readFileSync(path.join(h.dir, 'one.rs'), 'utf8').endsWith('\n'),
        'one.rs has no trailing newline, which is the lineCountOf branch it exists to exercise',
      );
      const p = path.join(h.dir, 'g.json');
      h.run(['--emit-graph', p]);
      const g = JSON.parse(fs.readFileSync(p, 'utf8'));
      h.eqJson(
        g.modules.map((m) => [m.id, m.loc]),
        [['crate', 2], ['crate::empty', 0], ['crate::one', 1]],
        'loc for a two line file, an empty file and an unterminated one line file',
      );
    },
  },
];

function moduleIds(res) {
  const ids = [];
  for (const tree of res.trees.values()) {
    for (const id of tree.modules.keys()) ids.push(id);
  }
  return ids;
}

function hasModuleEdge(res, from, to) {
  return res.moduleGraph.edges.some((e) => e.from === from && e.to === to);
}

function materialize(dir, files) {
  for (const rel of Object.keys(files).sort(compareCodeUnits)) {
    const abs = path.join(dir, ...rel.split('/'));
    fs.mkdirSync(path.dirname(abs), { recursive: true });
    fs.writeFileSync(abs, files[rel], 'utf8');
  }
}

function runSelfTest() {
  // The CLI entry point, captured here so h.main never has to name `main` from
  // inside a method that is itself called main. See the plan, 4.4.1.
  const cliMain = main;

  const root = path.join(os.tmpdir(), `rust-cycles-selftest-${process.pid}`);
  let passed = 0;
  let failed = 0;

  try {
    fs.mkdirSync(root, { recursive: true });

    for (let index = 0; index < FIXTURES.length; index++) {
      const fixture = FIXTURES[index];
      const dir = path.join(root, `f${index + 1}`);
      fs.mkdirSync(dir, { recursive: true });
      materialize(dir, fixture.files);

      const failures = [];
      const helper = {
        dir,
        write(rel, contents) {
          const abs = path.join(dir, ...rel.split('/'));
          fs.mkdirSync(path.dirname(abs), { recursive: true });
          fs.writeFileSync(abs, contents, 'utf8');
        },
        run(extra) {
          const opts = parseArgs([dir, ...extra]);
          if (opts.writeBaseline) {
            const result = analyze(opts);
            const baselineInfo = applyBaseline(result, null, '', opts, result.ctx);
            writeBaseline(opts.writeBaseline, result, opts);
            return { result, baselineInfo, exitCode: 0 };
          }
          // The real write path, not a reimplementation of it, so a fixture that
          // asserts on the file is asserting on what main() produces. The two
          // branches can never both be taken: parseArgs rejects the combination
          // and it runs here with selfTest false.
          if (opts.emitGraph) {
            const outcome = runAnalysis(opts);
            if (!outcome.result.ctx.diagnostics.some((d) => d.level === 'error')) {
              const payload = buildGraphPayload(outcome.result);
              if (checkGraphIntegrity(payload).length === 0) writeGraph(opts.emitGraph, payload);
            }
            return outcome;
          }
          return runAnalysis(opts);
        },
        /**
         * The CLI entry point, exactly as a user invokes it, with stdout and
         * stderr captured so a fixture can assert on the exit code and on the
         * operator-facing text. h.run stops short of main() and can see
         * neither, which is why every mutant inside main()'s --emit-graph block
         * survived the suite before this existed.
         *
         * cliMain is the top-level main(argv), aliased at the top of
         * runSelfTest. This method deliberately never writes the bare name
         * `main`, so the shorthand-versus-named-expression trap cannot be
         * reintroduced by an ordinary refactor: an object method SHORTHAND
         * creates no binding for its own name, so a bare `main` here would
         * resolve outward to the entry point, but `main: function main(...)`
         * binds its own name and recurses forever. The two forms differ by four
         * characters and the failure presents as a hang. See the plan, 4.4.1.
         *
         * Never pass --self-test here: parseArgs accepts it and main would
         * re-enter runSelfTest from inside a fixture, with both writers
         * patched, recursively.
         *
         * Restored in a finally: a leaked writer would corrupt every later
         * fixture's output rather than fail one. The guarantee rests on this
         * finally and on nothing else, because printHelp() and runSelfTest()
         * sit outside main's own try.
         */
        main(extra) {
          const outChunks = [];
          const errChunks = [];
          const realOut = process.stdout.write;
          const realErr = process.stderr.write;
          process.stdout.write = (chunk) => (outChunks.push(String(chunk)), true);
          process.stderr.write = (chunk) => (errChunks.push(String(chunk)), true);
          let code;
          try {
            code = cliMain([dir, ...extra]);
          } finally {
            process.stdout.write = realOut;
            process.stderr.write = realErr;
          }
          return { code, stdout: outChunks.join(''), stderr: errChunks.join('') };
        },
        eq(actual, expected, what) {
          if (actual !== expected) failures.push(`${what}: expected ${expected}, got ${actual}`);
        },
        eqJson(actual, expected, what) {
          const a = JSON.stringify(actual);
          const b = JSON.stringify(expected);
          if (a !== b) failures.push(`${what}: expected ${b}, got ${a}`);
        },
        atLeast(actual, minimum, what) {
          if (!(actual >= minimum)) failures.push(`${what}: expected >= ${minimum}, got ${actual}`);
        },
        yes(condition, what) {
          if (!condition) failures.push(`${what}: expected true, got false`);
        },
        no(condition, what) {
          if (condition) failures.push(`${what}: expected false, got true`);
        },
      };

      try {
        const outcome = helper.run(fixture.run);
        fixture.assert(outcome.result, helper, outcome);
      } catch (err) {
        failures.push(`threw: ${err && err.message}`);
      }

      if (failures.length === 0) {
        passed++;
        process.stdout.write(`PASS ${fixture.name}\n`);
      } else {
        failed++;
        for (const failure of failures) {
          process.stdout.write(`FAIL ${fixture.name}: ${failure}\n`);
        }
      }
    }
  } finally {
    try {
      fs.rmSync(root, { recursive: true, force: true });
    } catch {
      /* the temp tree is best effort; never mask a real failure with a cleanup error */
    }
  }

  process.stdout.write(`\n${passed} passed, ${failed} failed\n`);
  return failed === 0 ? 0 : 4;
}

/* ------------------------------------------------------------------ *
 * 27. Entry point
 * ------------------------------------------------------------------ */

// process.exitCode rather than process.exit(), so buffered stdout is flushed
// before the process ends.
process.exitCode = main(process.argv.slice(2));

