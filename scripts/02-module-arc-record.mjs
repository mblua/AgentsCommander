#!/usr/bin/env node
// Reduces a Rust dependency graph to its unique module-to-module arc set and writes that
// set to src-tauri/module-arcs.txt, for issue #1249.
//
// INPUT: a graph emitted by 01-rust_module-dependency-cycles.mjs 1.1.0, which lives outside
// this repository in the user's ObsidianVault. This script never analyses Rust, never spawns
// a process and never reads a .rs file. It is a projection of that graph, not a verdict on
// it, which is why it has no exit code 1.
//
// WHY THIS FILE EXISTS: the minimum feedback arc set is not unique, so diffing a levelizer's
// findings between two commits reports arcs as resolved and regressed when the graph did not
// change. Measured between 1b0e934 and 87514dfd over agentscommander_lib::config: the
// findings diff claimed 4 resolved and 3 regressions, while the arc set had changed by
// exactly one arc. Aggregate counters are no better: two commits with different graphs both
// reported 182 modules and 3 504 sites. The arc set is the comparison base that does not lie.
//
// RECORD FORMAT: one arc per line, "<from> -> <to>", UTF-8, LF only, one trailing LF, sorted
// ascending by the whole line, duplicates collapsed, self-arcs excluded, no header and no
// counters. An empty arc set is a zero-byte file. The separator's leading space is load
// bearing: it sorts below ':' (0x3A), the lowest character a module id can contain, so
// sorting the rendered lines is the same as sorting the (from, to) pairs. A '>' separator
// would invert every parent/descendant pair.
//
// src-tauri/module-arcs.txt is pinned to LF in .gitattributes. Without that pin,
// core.autocrlf=true checks it out as CRLF and every run rewrites all 972 lines.
//
// REGENERATING (manual; there is no CI or hook wiring, by design). From the repo root, with
// <VAULT> = repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust:
//
//   node "<VAULT>/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph graph.json --quiet
//   npm run record:arcs -- --graph graph.json
//
// then delete graph.json. It is gitignored and must never be committed: it carries the
// absolute path of the machine that produced it. The detector EXITS 1 when it finds cycles
// and writes the graph anyway; only exit 3 means no graph was written.
//
// Every flag of that command is part of the measurement, so this script gates on
// target.rootPath, target.crateDiscovery, target.includeTests and target.excludes rather
// than trusting them.
//
// Usage:
//   node scripts/02-module-arc-record.mjs --graph <file> [--out <file>]
//   node scripts/02-module-arc-record.mjs --self-test
//   node scripts/02-module-arc-record.mjs --help | --version
//
// Exit codes:
//   0 → the record was written
//   2 → usage error
//   3 → the graph could not be read, is not the expected graph, or the record could not be written
//   4 → --self-test had at least one failing assertion

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const TOOL = '02-module-arc-record';
const VERSION = '1.0.0';
const TAG = '[arc-record]';

const ARC_SEPARATOR = ' -> ';
const RECORD_RELATIVE = 'src-tauri/module-arcs.txt';

const EXPECTED_KIND = 'dependency-graph';
const EXPECTED_SCHEMA_VERSION = 1;
const EXPECTED_TARGET_SEGMENT = 'src-tauri';
const EXPECTED_CRATE_DISCOVERY = 'manifest';

const USAGE = [
  `${TOOL} ${VERSION}`,
  '',
  'Reduce a Rust dependency graph to its unique module-to-module arc set and write',
  'that set to the in-repo record.',
  '',
  'USAGE',
  '  node scripts/02-module-arc-record.mjs --graph <file> [--out <file>]',
  '  node scripts/02-module-arc-record.mjs --self-test',
  '  node scripts/02-module-arc-record.mjs --help | --version',
  '',
  'OPTIONS',
  '  --graph <file>   the dependency graph to reduce. REQUIRED. There is no default',
  '                   and no autodiscovery: a graph picked up silently is a way to',
  '                   record a measurement nobody asked for.',
  '  --out <file>     write the record here instead of the default',
  '                   src-tauri/module-arcs.txt, which is resolved from this',
  "                   script's own location and not from the working directory.",
  '  --self-test      run the embedded case suite',
  '  -h, --help       print this help and exit 0',
  '  -V, --version    print the version and exit 0',
  '',
  '  Both --flag value and --flag=value are accepted. --graph and --out may each be',
  '  given at most once. There are no positional arguments.',
  '',
  'EXIT CODES',
  '  0  the record was written',
  '  2  usage error (unknown option, missing or repeated value, positional argument)',
  '  3  the graph could not be read, is not the graph this script accepts, or the',
  '     record could not be written',
  '  4  --self-test had at least one failing assertion',
  '',
  '  0 and 3 are deliberately distinct: "I wrote the record" and "I could not read',
  '  the graph" must never be confused. There is no exit code 1, because this',
  '  script reports no findings. It is a projection of the graph, not a verdict.',
  '',
  'RECORD FORMAT',
  '  One arc per line, "<from> -> <to>", UTF-8, LF only, one trailing LF, sorted',
  '  ascending by the whole line, duplicates collapsed, self-arcs excluded, no',
  '  header and no counters. An empty arc set is a zero-byte file.',
  '',
  'REGENERATING THE RECORD',
  '  From the repository root, with <VAULT> the directory holding the instrument:',
  '',
  '    node "<VAULT>/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph graph.json --quiet',
  '    npm run record:arcs -- --graph graph.json',
  '',
  '  Then delete graph.json. It is gitignored and must never be committed: it',
  '  carries the absolute path of the machine that produced it.',
  '',
  '  The detector EXITS 1 when it finds cycles and writes the graph anyway. Only',
  '  its exit 3 means no graph was written.',
  '',
  '  Every flag of that command is part of the measurement, so this script refuses',
  '  a graph whose target, crate discovery, includeTests or excludes differ.',
  '',
  '  Did this commit change the structure?',
  '    git diff -- src-tauri/module-arcs.txt',
];

// ---------------------------------------------------------------------------
// Paths and helpers
// ---------------------------------------------------------------------------

// Resolved from the script's own location, never from process.cwd(): the default record is a
// fixed file in this repository, not "wherever the caller happened to be standing".
function defaultRecordPath() {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src-tauri', 'module-arcs.txt');
}

function printUsage(out) {
  for (const line of USAGE) out(line);
}

function errnoOf(error) {
  if (error && typeof error.code === 'string' && error.code.length > 0) return error.code;
  if (error && typeof error.message === 'string') return error.message;
  return String(error);
}

// The last path segment of target.rootPath, or null when rootPath is not a string. Windows
// separators are normalised first, because the emitter records whatever the caller typed.
function lastPathSegment(rootPath) {
  if (typeof rootPath !== 'string') return null;
  let normalised = rootPath.split('\\').join('/');
  while (normalised.endsWith('/')) normalised = normalised.slice(0, -1);
  const cut = normalised.lastIndexOf('/');
  return cut === -1 ? normalised : normalised.slice(cut + 1);
}

// Carries a message already formatted for the operator. It is the only thing readGraph,
// reduceGraph and writeRecord throw, so runCli maps exactly one class to exit 3.
class GraphError extends Error {
  constructor(message) {
    super(message);
    this.name = 'GraphError';
  }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const result = { help: false, version: false, selfTest: false, graph: null, out: null, error: null };

  // First pass: --help wins over every other argument, including an invalid one, and
  // --version wins over everything except --help.
  if (argv.includes('--help') || argv.includes('-h')) {
    result.help = true;
    return result;
  }
  if (argv.includes('--version') || argv.includes('-V')) {
    result.version = true;
    return result;
  }

  const fail = (message) => {
    result.error = message;
    return result;
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];

    if (arg === '--self-test') {
      result.selfTest = true;
      continue;
    }

    let name = null;
    if (arg === '--graph' || arg.startsWith('--graph=')) name = 'graph';
    else if (arg === '--out' || arg.startsWith('--out=')) name = 'out';

    if (name === null) {
      if (arg.startsWith('-')) return fail(`usage: unknown option '${arg}'`);
      return fail(`usage: unexpected positional argument '${arg}'`);
    }

    if (result[name] !== null) return fail(`usage: --${name} may be given at most once`);

    let value;
    if (arg === `--${name}`) {
      // A value that starts with '-' is still a value: the parser does not guess which of two
      // options the caller meant, it fails later at read time instead.
      if (index + 1 >= argv.length) return fail(`usage: --${name} requires a file path`);
      value = argv[index + 1];
      index += 1;
    } else {
      value = arg.slice(`--${name}=`.length);
    }
    if (value === '') return fail(`usage: --${name} requires a file path`);
    result[name] = value;
  }

  if (result.selfTest && argv.length > 1) {
    return fail('usage: --self-test may not be combined with any other argument');
  }
  if (!result.selfTest && result.graph === null) {
    return fail('usage: --graph <file> is required; there is no default graph path and no autodiscovery');
  }

  return result;
}

// ---------------------------------------------------------------------------
// Reading, reducing, writing
// ---------------------------------------------------------------------------

// Gates G1 to G12 of the plan, in order, first failure wins. Every message names the resolved
// path, because the caller supplied a relative one and needs to know which file was read.
function readGraph(file) {
  const resolved = path.resolve(file);

  let raw;
  try {
    raw = fs.readFileSync(resolved, 'utf8');
  } catch (error) {
    throw new GraphError(`cannot read graph file '${resolved}': ${errnoOf(error)}`);
  }

  let graph;
  try {
    graph = JSON.parse(raw);
  } catch (error) {
    throw new GraphError(`graph file '${resolved}' is not valid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }

  if (graph === null || typeof graph !== 'object' || Array.isArray(graph)) {
    throw new GraphError(`graph file '${resolved}' is not a JSON object`);
  }
  if (graph.kind !== EXPECTED_KIND) {
    throw new GraphError(`graph file '${resolved}' has kind ${JSON.stringify(graph.kind)}, expected "${EXPECTED_KIND}"`);
  }
  if (graph.schemaVersion !== EXPECTED_SCHEMA_VERSION) {
    throw new GraphError(`graph file '${resolved}' has schemaVersion ${JSON.stringify(graph.schemaVersion)}, this script reads schemaVersion ${EXPECTED_SCHEMA_VERSION}`);
  }

  const target = graph.target;
  if (target === null || typeof target !== 'object' || Array.isArray(target)) {
    throw new GraphError(`graph file '${resolved}' has no target section`);
  }

  // target.level is deliberately not gated: 'module' and 'all' emit identical edges, and
  // 'function' is caught by the edges gate below with a message that names the cause.
  if (lastPathSegment(target.rootPath) !== EXPECTED_TARGET_SEGMENT) {
    throw new GraphError(`graph file '${resolved}' was emitted for target ${JSON.stringify(target.rootPath)}, whose last path segment is not '${EXPECTED_TARGET_SEGMENT}'`);
  }
  if (target.crateDiscovery !== EXPECTED_CRATE_DISCOVERY) {
    throw new GraphError(`graph file '${resolved}' used crateDiscovery ${JSON.stringify(target.crateDiscovery)}, expected "${EXPECTED_CRATE_DISCOVERY}"`);
  }
  if (target.includeTests !== false) {
    throw new GraphError(`graph file '${resolved}' was emitted with includeTests ${JSON.stringify(target.includeTests)}; the record is defined over non-test references only`);
  }
  if (!Array.isArray(target.excludes) || target.excludes.length > 0) {
    throw new GraphError(`graph file '${resolved}' was emitted with exclude patterns; the record is defined over the unfiltered tree`);
  }

  // hasOwnProperty, not truthiness: the emitter guarantees that an ABSENT section was never
  // computed while an emitted [] means "computed, and there are none". Treating the absent
  // key as empty would write a zero-arc record and call it a success.
  if (!Object.prototype.hasOwnProperty.call(graph, 'edges')) {
    throw new GraphError(`graph file '${resolved}' has no 'edges' section: it was emitted with --level function, so module edges were never computed; re-emit with --level all or --level module`);
  }
  if (!Array.isArray(graph.edges)) {
    throw new GraphError(`graph file '${resolved}' has an 'edges' value that is not an array`);
  }

  return graph;
}

// Validates every edge, then collapses them to the unique non-self arcs, sorted. NUL joins the
// pair because it cannot occur in a module id, so the set key is unambiguous.
function reduceGraph(graph, file) {
  const seen = new Set();

  for (let index = 0; index < graph.edges.length; index += 1) {
    const edge = graph.edges[index];
    const usable = edge !== null
      && typeof edge === 'object'
      && !Array.isArray(edge)
      && typeof edge.from === 'string' && edge.from !== ''
      && typeof edge.to === 'string' && edge.to !== '';
    if (!usable) {
      throw new GraphError(`graph file '${file}' edge at index ${index} has no usable 'from' and 'to'`);
    }
    if (edge.from === edge.to) continue;
    seen.add(`${edge.from}\u0000${edge.to}`);
  }

  const lines = [];
  for (const key of seen) {
    const cut = key.indexOf('\u0000');
    lines.push(key.slice(0, cut) + ARC_SEPARATOR + key.slice(cut + 1));
  }

  // Explicit comparator: a bare sort() is implementation-defined string coercion and
  // localeCompare is locale-dependent. Neither is a reproducible artifact.
  lines.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  return lines;
}

function renderBody(lines) {
  return lines.length === 0 ? '' : `${lines.join('\n')}\n`;
}

// Sibling temp plus rename, never os.tmpdir(): a cross-device rename fails. The destination is
// a tracked file and a half-written commit candidate is worse than one extra rename.
function writeRecord(file, body) {
  const tmp = `${file}.${process.pid}.tmp`;
  try {
    fs.writeFileSync(tmp, body, 'utf8');
    fs.renameSync(tmp, file);
  } catch (error) {
    try {
      fs.rmSync(tmp, { force: true });
    } catch {
      // A temp file that resists removal must not replace the write failure that matters.
    }
    throw new GraphError(`cannot write record '${file}': ${errnoOf(error)}`);
  }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function runCli(argv, out, err) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    err(`internal error: ${error instanceof Error ? error.message : String(error)}`);
    if (error instanceof Error && error.stack) err(error.stack);
    return 2;
  }

  if (options.help) {
    printUsage(out);
    return 0;
  }
  if (options.version) {
    out(`${TOOL} ${VERSION}`);
    return 0;
  }
  if (options.error !== null) {
    err(options.error);
    return 2;
  }
  if (options.selfTest) return selfTest(out);

  const graphFile = path.resolve(options.graph);
  const recordFile = options.out === null ? defaultRecordPath() : path.resolve(options.out);

  try {
    const lines = reduceGraph(readGraph(graphFile), graphFile);
    const body = renderBody(lines);
    writeRecord(recordFile, body);
    out(`${TAG} wrote ${recordFile}: ${lines.length} arcs, ${Buffer.byteLength(body, 'utf8')} bytes`);
    return 0;
  } catch (error) {
    if (error instanceof GraphError) {
      err(error.message);
      return 3;
    }
    throw error;
  }
}

// ---------------------------------------------------------------------------
// Self-test
// ---------------------------------------------------------------------------

function assertSelf(condition, message) {
  if (!condition) throw new Error(message);
}

function tempRoot() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'ac-arc-record-'));
}

function removeRoot(root) {
  try {
    fs.rmSync(root, { recursive: true, force: true });
  } catch {
    // A fixture that resists removal must not turn a passing case into a failure.
  }
}

// A minimal graph that passes every gate, so each case changes exactly the one thing it tests.
function baseGraph(edges) {
  return {
    schemaVersion: 1,
    kind: 'dependency-graph',
    language: 'rust',
    tool: { name: 'rust-module-dependency-cycles', version: '1.1.0' },
    target: {
      rootPath: 'C:/anywhere/repo/src-tauri',
      crateDiscovery: 'manifest',
      level: 'all',
      functionScope: 'cross-module',
      includeTests: false,
      excludes: [],
    },
    edges,
  };
}

function edge(from, to, extra) {
  return { from, to, item: null, file: 'src/x.rs', line: 1, column: 1, kind: 'path', cfgGated: false, text: 't', ...extra };
}

function filesUnder(dir) {
  const found = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) found.push(...filesUnder(full));
    else found.push(full);
  }
  return found;
}

// Every case writes into its own temp root and none may omit --out: the suite must never touch
// the real src-tauri/module-arcs.txt.
function selfTest(out) {
  let total = 0;
  let failure = null;

  const run = (id, body) => {
    if (failure !== null) return;
    total += 1;
    const root = tempRoot();
    try {
      body(root);
    } catch (error) {
      failure = `case ${id}: ${error instanceof Error ? error.message : String(error)}`;
    } finally {
      removeRoot(root);
    }
  };

  const writeGraph = (root, name, value) => {
    const file = path.join(root, name);
    fs.writeFileSync(file, typeof value === 'string' ? value : JSON.stringify(value), 'utf8');
    return file;
  };

  const invoke = (argv) => {
    const outs = [];
    const errs = [];
    const code = runCli(argv, (line) => outs.push(line), (line) => errs.push(line));
    return { code, out: outs.join('\n'), err: errs.join('\n') };
  };

  // Runs a graph fixture into a fresh record inside `root` and returns the run plus the path.
  const record = (root, edges, name) => {
    const graph = writeGraph(root, `${name}.json`, baseGraph(edges));
    const target = path.join(root, `${name}.txt`);
    const result = invoke(['--graph', graph, '--out', target]);
    return { ...result, graph, target };
  };

  // ---- Group A: the CLI surface (11 cases) ----

  run('A1', () => {
    const result = invoke(['--help']);
    assertSelf(result.code === 0, '--help must exit 0');
    assertSelf(result.out.includes('EXIT CODES'), '--help must print the EXIT CODES block');
    assertSelf(result.out.includes('RECORD FORMAT'), '--help must print the RECORD FORMAT block');
  });

  run('A2', () => {
    const result = invoke(['--version']);
    assertSelf(result.code === 0, '--version must exit 0');
    assertSelf(result.out.includes(`${TOOL} ${VERSION}`), '--version must print the tool name and version');
  });

  run('A3', () => {
    const result = invoke([]);
    assertSelf(result.code === 2, 'no arguments must exit 2');
    assertSelf(result.err.includes('--graph <file> is required'), 'the message must say --graph is required');
  });

  run('A4', () => {
    const result = invoke(['--bogus']);
    assertSelf(result.code === 2, 'an unknown option must exit 2');
    assertSelf(result.err.includes("unknown option '--bogus'"), 'the message must name the unknown option');
  });

  run('A5', () => {
    const result = invoke(['--graph', 'g.json', 'extra.json']);
    assertSelf(result.code === 2, 'a positional argument must exit 2');
    assertSelf(result.err.includes("unexpected positional argument 'extra.json'"), 'the message must name the positional');
  });

  run('A6', () => {
    const bare = invoke(['--graph']);
    assertSelf(bare.code === 2, '--graph with no value must exit 2');
    assertSelf(bare.err.includes('--graph requires a file path'), 'the message must say --graph needs a path');
    const empty = invoke(['--graph=']);
    assertSelf(empty.code === 2, '--graph= must exit 2');
    assertSelf(empty.err.includes('--graph requires a file path'), 'the message must say --graph needs a path');
  });

  run('A7', () => {
    const result = invoke(['--graph', 'a', '--graph', 'b']);
    assertSelf(result.code === 2, 'a repeated --graph must exit 2');
    assertSelf(result.err.includes('--graph may be given at most once'), 'the message must say --graph is once only');
  });

  run('A8', () => {
    const bare = invoke(['--out']);
    assertSelf(bare.code === 2, '--out with no value must exit 2');
    assertSelf(bare.err.includes('--out requires a file path'), 'the message must say --out needs a path');
    const repeated = invoke(['--graph', 'g.json', '--out', 'a', '--out', 'b']);
    assertSelf(repeated.code === 2, 'a repeated --out must exit 2');
    assertSelf(repeated.err.includes('--out may be given at most once'), 'the message must say --out is once only');
  });

  run('A9', () => {
    const result = invoke(['--self-test', '--graph', 'g.json']);
    assertSelf(result.code === 2, '--self-test with another argument must exit 2');
    assertSelf(result.err.includes('may not be combined'), 'the message must say it may not be combined');
  });

  run('A10', () => {
    const result = invoke(['--help', '--bogus']);
    assertSelf(result.code === 0, '--help must win over an invalid option');
    assertSelf(result.out.includes('USAGE'), '--help must still print the usage block');
  });

  run('A11', (root) => {
    const graph = writeGraph(root, 'g.json', baseGraph([edge('a::b', 'a::c'), edge('a::c', 'a::d')]));
    const joined = path.join(root, 'joined.txt');
    const spaced = path.join(root, 'spaced.txt');
    const first = invoke([`--graph=${graph}`, `--out=${joined}`]);
    const second = invoke(['--graph', graph, '--out', spaced]);
    assertSelf(first.code === 0 && second.code === 0, 'both argument forms must exit 0');
    assertSelf(fs.readFileSync(joined).equals(fs.readFileSync(spaced)), 'both argument forms must write identical bytes');
  });

  // ---- Group B: reduction semantics (10 cases) ----

  run('B1', (root) => {
    const result = record(root, [
      edge('a::b', 'a::c', { file: 'src/one.rs', line: 10, column: 3 }),
      edge('a::b', 'a::c', { file: 'src/two.rs', line: 20, column: 7 }),
    ], 'b1');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    assertSelf(fs.readFileSync(result.target, 'utf8') === 'a::b -> a::c\n', 'two sites of one arc must collapse to one line');
  });

  run('B2', (root) => {
    const result = record(root, [
      edge('a::b', 'a::c', { kind: 'path', item: 'Alpha', cfgGated: false, text: 'crate::a::c::Alpha' }),
      edge('a::b', 'a::c', { kind: 'use', item: 'Beta', cfgGated: true, text: 'use crate::a::c::Beta' }),
    ], 'b2');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    assertSelf(fs.readFileSync(result.target, 'utf8') === 'a::b -> a::c\n', 'differing edge metadata must not defeat dedup');
  });

  run('B3', (root) => {
    const result = record(root, [edge('a::b', 'a::b'), edge('a::b', 'a::c')], 'b3');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    assertSelf(fs.readFileSync(result.target, 'utf8') === 'a::b -> a::c\n', 'a self-arc must be excluded');
  });

  run('B4', (root) => {
    const result = record(root, [edge('a::b', 'a::b'), edge('a::c', 'a::c')], 'b4');
    assertSelf(result.code === 0, 'an all-self-arc graph must still exit 0');
    assertSelf(fs.statSync(result.target).size === 0, 'an empty arc set must be a zero-byte file');
    assertSelf(result.out.includes('0 arcs, 0 bytes'), 'the confirmation must report zero arcs and zero bytes');
  });

  run('B5', (root) => {
    const result = record(root, [], 'b5');
    assertSelf(result.code === 0, 'an emitted empty edges array must exit 0');
    assertSelf(fs.statSync(result.target).size === 0, 'an empty arc set must be a zero-byte file');
  });

  run('B6', (root) => {
    const result = record(root, [edge('z', 'a'), edge('a', 'z')], 'b6');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    assertSelf(fs.readFileSync(result.target, 'utf8') === 'a -> z\nz -> a\n', 'the output must be sorted, not emission-ordered');
  });

  run('B7', (root) => {
    const six = [
      edge('m::a', 'm::b'), edge('m::b', 'm::c'), edge('m::c', 'm::d'),
      edge('m::d', 'm::e'), edge('m::e', 'm::f'), edge('m::f', 'm::a'),
    ];
    const forward = record(root, six, 'b7-forward');
    const reversed = record(root, [...six].reverse(), 'b7-reversed');
    assertSelf(forward.code === 0 && reversed.code === 0, 'both permutations must exit 0');
    assertSelf(fs.readFileSync(forward.target).equals(fs.readFileSync(reversed.target)), 'edge order must not reach the output');
  });

  run('B8', (root) => {
    const result = record(root, [edge('a::b', 'y'), edge('a', 'x')], 'b8');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    // Pins the separator: its leading 0x20 sorts below ':' (0x3A), so line order is pair
    // order. A '>' separator (0x3E) would put 'a::b -> y' first and invert every such pair.
    assertSelf(fs.readFileSync(result.target, 'utf8') === 'a -> x\na::b -> y\n', 'a parent arc must precede its descendant arc');
  });

  run('B9', (root) => {
    const result = record(root, [edge('a', 'z'), edge('a', 'b')], 'b9');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    assertSelf(fs.readFileSync(result.target, 'utf8') === 'a -> b\na -> z\n', 'arcs sharing a source must sort by target');
  });

  run('B10', (root) => {
    const result = record(root, [edge('a::b_2', 'a::c')], 'b10');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    assertSelf(fs.readFileSync(result.target, 'utf8') === 'a::b_2 -> a::c\n', 'module ids must be recorded verbatim');
  });

  // ---- Group C: byte format (4 cases) ----

  run('C1', (root) => {
    const result = record(root, [edge('a::b', 'a::c'), edge('a::c', 'a::d')], 'c1');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    const bytes = fs.readFileSync(result.target);
    assertSelf(bytes[bytes.length - 1] === 0x0a, 'the file must end with LF');
    assertSelf(bytes[bytes.length - 2] !== 0x0a, 'the file must end with exactly one LF');
    assertSelf(!bytes.includes(0x0d), 'the file must contain no CR');
  });

  run('C2', (root) => {
    const result = record(root, [edge('a::b', 'a::c'), edge('a::c', 'a::d'), edge('a::d', 'a::b')], 'c2');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    const text = fs.readFileSync(result.target, 'utf8');
    for (const line of text.slice(0, -1).split('\n')) {
      assertSelf(line.split(ARC_SEPARATOR).length === 2, `line '${line}' must contain the separator exactly once`);
    }
  });

  run('C3', (root) => {
    const edges = [edge('a::b', 'a::c'), edge('a::c', 'a::d')];
    const graph = writeGraph(root, 'c3.json', baseGraph(edges));
    const target = path.join(root, 'c3.txt');
    const first = invoke(['--graph', graph, '--out', target]);
    const firstBytes = fs.readFileSync(target);
    const second = invoke(['--graph', graph, '--out', target]);
    const secondBytes = fs.readFileSync(target);
    assertSelf(first.code === 0 && second.code === 0, 'both runs must exit 0');
    assertSelf(firstBytes.equals(secondBytes), 'a second run must produce identical bytes, not appended ones');
  });

  run('C4', (root) => {
    const result = record(root, [edge('a::b', 'a::c')], 'c4');
    assertSelf(result.code === 0, 'a valid graph must exit 0');
    assertSelf(fs.readFileSync(result.target)[0] !== 0xef, 'the file must have no BOM');
  });

  // ---- Group D: refusals (16 cases) ----

  run('D1', (root) => {
    const missing = path.join(root, 'nope.json');
    const result = invoke(['--graph', missing, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a nonexistent graph must exit 3');
    assertSelf(result.err.includes('cannot read graph file'), 'the message must say it could not read the graph');
    assertSelf(result.err.includes(missing), 'the message must name the resolved path');
  });

  run('D2', (root) => {
    const result = invoke(['--graph', root, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a directory as the graph must exit 3');
  });

  run('D3', (root) => {
    const graph = writeGraph(root, 'd3.json', '{ not json');
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'malformed JSON must exit 3');
    assertSelf(result.err.includes('is not valid JSON'), 'the message must say the JSON is invalid');
  });

  run('D4', (root) => {
    const graph = writeGraph(root, 'd4.json', []);
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a JSON array must exit 3');
    assertSelf(result.err.includes('is not a JSON object'), 'the message must say it is not an object');
  });

  run('D5', (root) => {
    const graph = writeGraph(root, 'd5.json', { ...baseGraph([edge('a', 'b')]), kind: 'baseline' });
    const target = path.join(root, 'r.txt');
    // A refusal must not truncate, replace or empty an existing record.
    fs.writeFileSync(target, 'SENTINEL', 'utf8');
    const result = invoke(['--graph', graph, '--out', target]);
    assertSelf(result.code === 3, 'the wrong kind must exit 3');
    assertSelf(result.err.includes('expected "dependency-graph"'), 'the message must name the expected kind');
    assertSelf(fs.readFileSync(target, 'utf8') === 'SENTINEL', 'a refusal must leave an existing record untouched');
  });

  run('D6', (root) => {
    const graph = writeGraph(root, 'd6.json', { ...baseGraph([edge('a', 'b')]), schemaVersion: 2 });
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'an unknown schemaVersion must exit 3');
    assertSelf(result.err.includes('schemaVersion'), 'the message must name schemaVersion');
  });

  run('D7', (root) => {
    const fixture = baseGraph([edge('a', 'b')]);
    delete fixture.edges;
    const graph = writeGraph(root, 'd7.json', fixture);
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'an absent edges section must exit 3, never an empty record');
    assertSelf(result.err.includes('--level function'), 'the message must name the cause');
  });

  run('D8', (root) => {
    const graph = writeGraph(root, 'd8.json', { ...baseGraph([]), edges: {} });
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a non-array edges value must exit 3');
    assertSelf(result.err.includes('not an array'), 'the message must say edges is not an array');
  });

  run('D9', (root) => {
    const broken = edge('a', 'b');
    delete broken.to;
    const graph = writeGraph(root, 'd9.json', baseGraph([edge('a', 'c'), broken]));
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'an edge without a target must exit 3');
    assertSelf(result.err.includes('index 1'), 'the message must name the offending index');
  });

  run('D10', (root) => {
    const graph = writeGraph(root, 'd10.json', baseGraph([edge(7, 'b')]));
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a non-string from must exit 3');
  });

  run('D11', (root) => {
    const graph = writeGraph(root, 'd11.json', baseGraph([edge('a', '')]));
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'an empty to must exit 3');
  });

  run('D12', (root) => {
    const fixture = baseGraph([edge('a', 'b')]);
    fixture.target.includeTests = true;
    const graph = writeGraph(root, 'd12.json', fixture);
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a graph including tests must exit 3');
    assertSelf(result.err.includes('includeTests'), 'the message must name includeTests');
  });

  run('D13', (root) => {
    const fixture = baseGraph([edge('a', 'b')]);
    fixture.target.excludes = ['x'];
    const graph = writeGraph(root, 'd13.json', fixture);
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a filtered graph must exit 3');
    assertSelf(result.err.includes('exclude'), 'the message must name the exclusion');
  });

  run('D14', (root) => {
    const fixture = baseGraph([edge('a', 'b')]);
    fixture.target.crateDiscovery = 'fallback';
    const graph = writeGraph(root, 'd14.json', fixture);
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a different crate discovery must exit 3');
    assertSelf(result.err.includes('crateDiscovery'), 'the message must name crateDiscovery');
  });

  run('D15', (root) => {
    const fixture = baseGraph([edge('a', 'b')]);
    fixture.target.rootPath = 'C:/anywhere/repo/crates/session-bridge';
    const graph = writeGraph(root, 'd15.json', fixture);
    const result = invoke(['--graph', graph, '--out', path.join(root, 'r.txt')]);
    assertSelf(result.code === 3, 'a graph of another workspace member must exit 3');
    assertSelf(result.err.includes(EXPECTED_TARGET_SEGMENT), 'the message must name the expected segment');
  });

  run('D16', (root) => {
    const graph = writeGraph(root, 'd16.json', baseGraph([edge('a', 'b')]));
    const result = invoke(['--graph', graph, '--out', path.join(root, 'missing', 'r.txt')]);
    assertSelf(result.code === 3, 'an unwritable destination must exit 3');
    assertSelf(result.err.includes('cannot write record'), 'the message must say it could not write the record');
    const leftovers = filesUnder(root).filter((file) => file.endsWith('.tmp'));
    assertSelf(leftovers.length === 0, `a failed write must leave no temp file: ${leftovers.join(', ')}`);
  });

  // ---- Group E: the default path (1 case) ----

  run('E1', () => {
    const resolved = defaultRecordPath();
    const suffix = path.join(...RECORD_RELATIVE.split('/'));
    assertSelf(resolved.endsWith(suffix), `the default record path must end with ${suffix}`);
    const scriptDir = path.dirname(fileURLToPath(import.meta.url));
    assertSelf(path.dirname(path.dirname(resolved)) === path.dirname(scriptDir), 'the default record path must be resolved from the script location');
  });

  if (failure !== null) {
    out(`${TAG} self-test FAILED: ${failure}`);
    return 4;
  }
  out(`${TAG} self-test passed: ${total} cases`);
  return 0;
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

function main() {
  const write = (line) => process.stdout.write(`${line}\n`);
  const warn = (line) => process.stderr.write(`${line}\n`);
  try {
    process.exitCode = runCli(process.argv.slice(2), write, warn);
  } catch (error) {
    warn(`internal error: ${error instanceof Error ? error.message : String(error)}`);
    if (error instanceof Error && error.stack) warn(error.stack);
    process.exitCode = 2;
  }
}

main();
