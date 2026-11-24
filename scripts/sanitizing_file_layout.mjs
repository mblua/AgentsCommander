import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { testLocalPath } from './sanitizing_rust_scan.mjs';
import {
  runMeasuredGate,
  metric,
  hit,
  warning,
  resolveAlias as resolveGateAlias
} from './sanitizing_gate_support.mjs';

const relevantPath = /(?:^|::)(?:test)$/;
function resolveAlias(scan, input, scope) {
  return resolveGateAlias(scan, input, scope, relevantPath);
}

// Finite syntactic signals, using the shared lexer, scope and production model.
export function runGate(snapshot) {
  return runMeasuredGate(snapshot, { gate, rules, newFileRules, inUniverse, measure, extraChecks });
}

const gate = '05';
const rules = [
  'mod_rs', 'test_files', 'inline_mod', 'path', 'glob', 'pub_mod', 'pub_super_mod', 'pub_in_mod'
];
const newFileRules = rules.slice(0, 5);
const inUniverse = p => /^src-tauri\/(?:src|tests)\/.*\.rs$/.test(p);
const extraChecks = () => {};
function testAttributeLine(m, path, scan, tree) {
  let testLine;
  for (const a of scan.attributes) for (const meta of a.signals) {
    const r = resolveAlias(scan,meta.name,a.scope);
    if (['test', 'tokio::test', 'async_std::test'].includes(r.path) || r.ambiguous &&
      /(?:^|::)test$/.test(r.path ?? meta.name)) {
      testLine ??= a.line; warning(m, path, a.line, r);
    }
    // Same cut as gate 04: a test-local #[path] only shares code between test crates.
    if (meta.name === 'path' && !testLocalPath(path, scan, a, tree)) hit(m, 'path', a.line);
  }
  return testLine;
}
function modVisibility(m, path, ts, item) {
  if (item.kind !== 'mod') return;
  if (item.bodyScope) hit(m, 'inline_mod', item.line);
  // Visibility belongs to this item, including attributes preceding pub(...).
  const prefix = ts.filter(t => t.start >= item.start && t.index < item.first);
  const pub = prefix.findIndex(t => t.name === 'pub');
  if (pub >= 0) {
    if (prefix[pub + 1]?.text !== '(') {
      if (path !== 'src-tauri/src/lib.rs') hit(m, 'pub_mod', item.line);
    } else if (prefix[pub + 2]?.name === 'super') hit(m, 'pub_super_mod', item.line);
    else if (prefix[pub + 2]?.name === 'in') hit(m, 'pub_in_mod', item.line);
  }
}
function globImports(m, path, scan, ts) {
  for (const scope of scan.scopes) for (const entry of scope.imports) if (entry.glob) {
    // C6: a file under a tests/ folder is part of its tests.rs, so it may glob its parent too.
    const testsFile = path.endsWith('/tests.rs') || path.includes('/tests/');
    if (testsFile && entry.target === 'super::*') continue;
    const use = scan.items.find(i => i.kind === 'use' && i.scope === scope &&
      ts.slice(i.first, i.last).some(t => t.text === '*'));
    hit(m, 'glob', use?.line ?? 1);
  }
}
function measure(path, scan, { tree }) {
  const m = metric(), integration = path.startsWith('src-tauri/tests/') || /(?:^|\/)tests\//.test(
    path);
  if (path.endsWith('/mod.rs')) hit(m, 'mod_rs', 1);
  const testLine = testAttributeLine(m, path, scan, tree);
  if (testLine && !path.endsWith('/tests.rs') && !integration) hit(m, 'test_files', testLine);
  const ts = scan.tokens;
  for (const item of scan.items) modVisibility(m, path, ts, item);
  globImports(m, path, scan, ts);
  return m;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
