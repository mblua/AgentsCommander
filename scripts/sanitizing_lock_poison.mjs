import { gateCli, isMain } from './sanitizing_baseline.mjs';
import { runMeasuredGate, metric, hit } from './sanitizing_gate_support.mjs';

// #98: `.lock()/.read()/.write()` + `.unwrap()/.expect(..)` panics on a poisoned lock, so one
// panic cascades into every later caller. Forbidden in production code, existing debt included.
export function runGate(snapshot) {
  return runMeasuredGate(snapshot, { gate, rules, newFileRules: rules, inUniverse, measure,
    extraChecks });
}

const gate = '10';
const rules = ['lock_panic'];
const inUniverse = p => /^(?:src-tauri|crates\/[^/]+)\/src\/.*\.rs$/.test(p);
const acquire = new Set(['lock', 'read', 'write']);
const continuation = new Set(['else', '.', '?', '=>', 'as']);
function extraChecks(path, m, before, violation) {
  const site = m.sites.lock_panic?.[0];
  if (site) violation('lock_panic', path, site.line, before?.counts.lock_panic ?? 0,
    m.counts.lock_panic,
    'poison-panicking lock; use .unwrap_or_else(std::sync::PoisonError::into_inner)');
}
// Comma-separated arguments of the group opened at `open`, as lists of token indexes.
function args(ts, open) {
  const out = [[]];
  for (let i = open + 1; i < ts[open].mate; i++) {
    if (ts[i].text === ',') { out.push([]); continue; }
    out.at(-1).push(i);
    if (['(', '[', '{'].includes(ts[i].text)) i = ts[i].mate;
  }
  return out.filter(a => a.length);
}
// True when the cfg predicate holds only under `test`: test, all(.., test, ..), any(all test).
function requiresTest(ts, a) {
  const t = ts[a[0]];
  if (a.length === 1) return t.name === 'test';
  if (ts[a[0] + 1]?.text !== '(') return false;
  const kids = args(ts, a[0] + 1);
  if (t.name === 'all') return kids.some(k => requiresTest(ts, k));
  if (t.name === 'any') return kids.length > 0 && kids.every(k => requiresTest(ts, k));
  return false;
}
// Statement, field, arm and expression `#[cfg(test)]`, which the shared item model skips:
// the range ends at `;` or `,`, before a closing bracket, or after a `{..}` not continued.
function testRanges(ts) {
  const ranges = [];
  for (let i = 0; i + 3 < ts.length; i++) {
    if (ts[i].text !== '#' || ts[i + 1].text !== '[' || ts[i + 2].name !== 'cfg' ||
      ts[i + 3].text !== '(') continue;
    const pred = args(ts, i + 3);
    if (pred.length !== 1 || !requiresTest(ts, pred[0])) continue;
    let k = ts[i + 1].mate + 1;
    while (ts[k]?.text === '#' && ts[k + 1]?.text === '[') k = ts[k + 1].mate + 1;
    for (; k < ts.length; k++) {
      const t = ts[k];
      if (t.text === ';' || t.text === ',') break;
      if (t.text === ')' || t.text === ']' || t.text === '}') { k--; break; }
      if (t.text === '(' || t.text === '[') { k = t.mate; continue; }
      if (t.text === '{') { k = t.mate; if (!continuation.has(ts[k + 1]?.text)) break; }
    }
    ranges.push([ts[i].start, ts[Math.min(k, ts.length - 1)].end]);
  }
  return ranges;
}
function measure(path, scan) {
  const m = metric(), ts = scan.tokens, ranges = testRanges(ts);
  const isTest = t => scan.isTestOffset(t.start) ||
    ranges.some(([a, b]) => t.start >= a && t.start < b);
  for (let i = 1; i + 5 < ts.length; i++) {
    const t = ts[i], next = ts[i + 4].name;
    if (ts[i - 1].text !== '.' || !acquire.has(t.name) || ts[i + 1].text !== '(' ||
      ts[i + 2].text !== ')' || ts[i + 3].text !== '.') continue;
    if ((next === 'unwrap' || next === 'expect') && ts[i + 5].text === '(' && !isTest(t))
      hit(m, 'lock_panic', t.line);
  }
  return m;
}

if (isMain(import.meta.url)) process.exitCode = await gateCli(runGate);
