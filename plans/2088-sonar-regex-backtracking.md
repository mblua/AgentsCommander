# Plan #2088 (round 3): replace the super-linear `fn` regex and its round-2 scanner in `scripts/check-test-debt.mjs`

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2088. PR: https://github.com/mblua/AgentsCommander/pull/2099
  (open; do not merge). SonarCloud on the PR, line 307:
  - `javascript:S8786` key `AaCqh-uoqWlJPkFQ9mDs` — non-exponential (super-linear) backtracking;
  - `javascript:S5843` key `AaCqh-uoqWlJPkFQ9mDt` — regex complexity 49 > 20;
  - `javascript:S5852` is already clear from round 1. Gate: `new_maintainability_rating` 5 vs threshold 1.
- Repo `repo-AgentsCommander`; branch `fix/2088-sonar-regex-backtracking`; base (frozen at authoring,
  2026-09-16 UTC): `fbf1c438051f889278b2cc54662e274b78047eda` = local HEAD = remote branch head; the
  source tree under `scripts/` is `8a2f81f7` (round 1). Tracked tree clean. Every line number below
  refers to that tree; if a quoted line no longer matches, re-anchor on the quoted text, never on the
  number.
- Class: **Lite** (round 1 was Express). One source file, one matcher, structural change: the
  whole-match regex is replaced by a linear scanner plus eleven helpers (+152/-8 lines in one file),
  and `scanRustFile`'s observable behavior is proven by a differential harness. No test file, no
  dependency, no IPC, no product code. Band 1-25 unchanged; owner `ac-dev-rust-v4`, coordinator
  `ac-tech-lead-v4`; grinch reviews the proofs below.
- Review history: round 1 cleared `S5852` but left `S8786`/`S5843` (regex complexity); round 2
  replaced the regex with an `indexOf`-based scanner and passed 22.9M-input equivalence, but review
  measured the scanner **super-linear on three shapes** and slower than base on one (`§2.2`); round 3
  (this plan) memoizes every forward scan, keeps equivalence, and adds those shapes to AC6.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so commit with
  `git add -f plans/2088-sonar-regex-backtracking.md`.

## 1. Objective

Delete the single (round-1) regex `fnRe` at `scripts/check-test-debt.mjs:307` and match the same
language with a single forward scanner (`rustFnMatches` + helpers) whose every scan keeps a memoized
progress point, so:
`javascript:S8786` and `javascript:S5843` clear on PR #2099 (gate green), `javascript:S5852` stays
clear, **no measured shape is slower than base**, and `npm run test:debt` output stays byte-identical.

"Linear" is asserted only where measured: on the three adversarial shapes from review round 2 —
`'#['.repeat(n)`, `'pub('.repeat(n)`, `'fn a<'.repeat(n)` — and on their attribute-bearing variants,
head grows ~2.1x per doubling (base: ~4x, quadratic) and is 90-500x faster at the measured sizes.
Every AC6 row is faster than base: the worst margin is `'fn '` at 0.87-0.94, and the
finder-exercising rows are 31x-500x faster. Everything the old regex did outside the matcher is
untouched; the two pre-existing quadratic paths that remain are out of scope and listed in §3.

## 2. Verified cause

### 2.1 Round 2 target: `fnRe` (exact literal, base line 307)

```js
const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(/g;
```

`scslre` on that literal (the library Sonar uses for both rules, pinned 0.3.0) reports four
non-exponential causes and no exponential one:

| type | char | start quant | end quant |
|---|---|---|---|
| Trade | `' '` | `\s*@21-24` (inside the attribute group) | `\s*@49-52` (outer) |
| Trade | `' '` | `\s*@41-44` (group trailing) | `\s*@49-52` (outer) |
| Move | `' '` | — | `\s*@4-7` (group leading) |
| Move | `' '` | — | `\s*@49-52` (outer) |

`slow-regex` is silent (no exponential report) and `regex-complexity` returns 49. Sonar's `S8786`
fires on exactly this shape (`hasNonExponential && !hasExponential`); `S5843` fires above 20. A
regex-only fix cannot satisfy both, measured with the same local judge:

| candidate | scslre | complexity |
|---|---|---|
| `((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)` (attribute prefix alone) | `Move` | 22 |
| `(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(` (header, no attributes) | clean | 26 |
| `(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)` (stops before `(`) | clean | 19 |

Reason: any pattern that can begin a match with an unbounded whitespace/attribute quantifier yields
a `Move` report (the unanchored retry), and the attribute grammar alone costs 22 > 20. The matcher
therefore has to leave the regex engine.

### 2.2 Round-2 scanner: measured failure (review, round 3)

The round-2 scanner used `source.indexOf(']', open + 1)`, `source.indexOf(')', open + 1)` and a
generic-parameter loop, each restarted from every `#`/`pub`/`fn` start position. On inputs where the
scan never closes, each start position re-scans to EOF, so the scanner is quadratic — the same
failure mode as the regex, only in JavaScript. Review numbers (ms, in-process, per doubling):

| shape | 64k | 128k | 256k | growth |
|---|---|---|---|---|
| `'#['.repeat(n)` | 49.7 | 174.9 | 610.6 | ~3.5x |
| `'pub('.repeat(n)` | 109.5 | 353.2 | 1242.0 | ~3.2x |
| `'fn a<'.repeat(n)` | 11992 (64k) | — | — | ~3.9x from 32k; 32k base 1786 vs new 3072 (worse than base) |

Cause map: `'#['` → `rustAttributeEnd`'s `indexOf(']')`; `'pub('` → `rustPubBodyEnd`'s
`indexOf(')')`; `'fn a<'` → `rustFnTailAt`'s generic loop. Round 2's AC6 only probed
`'#[x] '.repeat(n)` and `' '.repeat(n)`, which are linear in the round-2 scanner, so the objective's
"linear" claim was not falsified by its own acceptance criteria. This plan fixes both the code and
the AC.

## 3. In scope / out of scope

In scope: exactly `scripts/check-test-debt.mjs` — the `scanRustFile` matcher and eleven new helper
functions (section 4). The deleted literal is the only removed line group.

Out of scope (binding): every other regex and function in the file (including `S5843` on line 287,
which is pre-existing, untouched, and not on the PR); the pre-existing super-linear body search
(`masked.indexOf('{', end)` / `findMatchingBrace` / `maskedComments.slice(...).search(...)` on
`'#[test] fn x('.repeat(n)` inputs) and `lineOf` (linear per call, quadratic over many findings);
`test-debt.allowlist.json`, `package.json`, `plans/` (except this file), any test file, any
workflow. Report format, categories, ids, allowlist semantics and `--self-test` fixtures are
preserved by construction (§5) and proven by AC1/AC2/AC7.

## 4. Decided solution (exact change; nothing is left to the implementer)

Insert the eleven helpers after `hasExecutableRustBody` (after base line 297) and before
`function scanRustFile` (base line 299):

```js
function makeForwardFinder(source, isNeedle) {
  let found = -1;
  let scannedFrom = -1;
  let lastStart = -1;
  return (start) => {
    if (start < lastStart) {
      let index = start;
      while (index < source.length && !isNeedle(source[index])) index += 1;
      return index === source.length ? -1 : index;
    }
    lastStart = start;
    if (start > scannedFrom) {
      let index = start;
      while (index < source.length && !isNeedle(source[index])) index += 1;
      scannedFrom = index;
      found = index === source.length ? -1 : index;
    }
    return found;
  };
}

function rustAttributeEnd(source, index, findBracket) {
  if (source[index] !== '#') return -1;
  const open = skipWhitespace(source, index + 1);
  if (source[open] !== '[') return -1;
  const close = findBracket(open + 1);
  return close === -1 ? -1 : close + 1;
}

function rustPubBodyEnd(source, index, findParen) {
  if (!source.startsWith('pub', index)) return -1;
  let after = index + 3;
  const open = skipWhitespace(source, after);
  if (source[open] === '(') {
    const close = findParen(open + 1);
    if (close !== -1) after = close + 1;
  }
  const body = skipWhitespace(source, after);
  return body > after ? body : -1;
}

function rustFnHeaderAt(source, index, scans) {
  const pubBody = rustPubBodyEnd(source, index, scans.paren);
  if (pubBody !== -1) {
    const header = rustFnTailAt(source, pubBody, scans.genericClose);
    if (header !== null) return header;
  }
  return rustFnTailAt(source, index, scans.genericClose);
}

function isRustIdentifierStart(code) {
  return (code >= 97 && code <= 122) || (code >= 65 && code <= 90) || code === 95;
}

function rustIdentifierEnd(source, start) {
  let end = start;
  for (;;) {
    const code = source.charCodeAt(end);
    if ((code >= 97 && code <= 122) || (code >= 65 && code <= 90) || (code >= 48 && code <= 57) || code === 95) end += 1;
    else break;
  }
  return end;
}

function rustFnTailAt(source, start, findGenericClose) {
  let cursor = start;
  if (source.startsWith('async', cursor)) {
    const afterAsync = skipWhitespace(source, cursor + 5);
    if (afterAsync > cursor + 5) cursor = afterAsync;
  }
  if (!source.startsWith('fn', cursor)) return null;
  const nameStart = skipWhitespace(source, cursor + 2);
  if (nameStart === cursor + 2 || !isRustIdentifierStart(source.charCodeAt(nameStart))) return null;
  const nameEnd = rustIdentifierEnd(source, nameStart + 1);
  let open = skipWhitespace(source, nameEnd);
  if (source[open] === '<') {
    const close = findGenericClose(open + 1);
    if (close === -1 || source[close] !== '>') return null;
    open = skipWhitespace(source, close + 1);
  }
  if (source[open] !== '(') return null;
  return { name: source.slice(nameStart, nameEnd), end: open + 1 };
}

function rustWhitespaceStep(cursor, state) {
  if (state.runStart === -1) state.runStart = cursor;
  return cursor + 1;
}

function rustAttributeStep(source, cursor, state, scans) {
  const attrEnd = rustAttributeEnd(source, cursor, scans.bracket);
  if (attrEnd === -1) {
    state.runStart = -1;
    state.runHasAttr = false;
    return cursor + 1;
  }
  if (state.runStart === -1) state.runStart = cursor;
  state.runHasAttr = true;
  return attrEnd;
}

function rustWordStep(source, cursor, state, scans, matches) {
  const header = rustFnHeaderAt(source, cursor, scans);
  if (header === null) {
    state.runStart = -1;
    state.runHasAttr = false;
    return cursor + 1;
  }
  const index = state.runStart === -1 ? cursor : state.runStart;
  const attrs = state.runHasAttr ? source.slice(index, cursor) : '';
  matches.push({ index, attrs, end: header.end, name: header.name });
  state.runStart = -1;
  state.runHasAttr = false;
  return header.end;
}

function rustFnMatches(source) {
  if (source.indexOf('#') === -1) return [];
  const scans = {
    bracket: makeForwardFinder(source, (ch) => ch === ']'),
    paren: makeForwardFinder(source, (ch) => ch === ')'),
    genericClose: makeForwardFinder(source, (ch) => ch === '>' || ch === '{' || ch === '}'),
  };
  const state = { runStart: -1, runHasAttr: false };
  const matches = [];
  let cursor = 0;
  while (cursor < source.length) {
    const code = source.charCodeAt(cursor);
    if (code === 32 || (code >= 9 && code <= 13) || (code > 127 && /\s/.test(source[cursor]))) {
      cursor = rustWhitespaceStep(cursor, state);
      continue;
    }
    if (code === 35) {
      cursor = rustAttributeStep(source, cursor, state, scans);
      continue;
    }
    if (code === 112 || code === 97 || code === 102) {
      cursor = rustWordStep(source, cursor, state, scans, matches);
      continue;
    }
    state.runStart = -1;
    state.runHasAttr = false;
    cursor += 1;
  }
  return matches;
}
```

Then replace exactly base lines 307-315 of `scanRustFile`:

```js
  const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(/g;
  let match;

  while ((match = fnRe.exec(masked)) !== null) {
    const attrs = match[1] || '';
    if (!/#\s*\[\s*test\b/.test(attrs)) continue;
    const fnName = match[2];
    const fnStart = match.index + match[0].lastIndexOf('fn ');
    const bodyOpen = masked.indexOf('{', fnRe.lastIndex);
```

with:

```js
  for (const match of rustFnMatches(masked)) {
    const attrs = match.attrs;
    if (!/#\s*\[\s*test\b/.test(attrs)) continue;
    const fnName = match.name;
    const fnStart = match.index + masked.slice(match.index, match.end).lastIndexOf('fn ');
    const bodyOpen = masked.indexOf('{', match.end);
```

No other edit. Decision notes:

- D1 — attributes are consumed whole by `rustAttributeEnd` (`#\s*\[[^\]]*\]`), so `fn` text inside an
  attribute's `[^\]]*` content is skipped instead of matched; the forward run state
  (`runStart`/`runHasAttr`) replaces the greedy prefix capture.
- D2 — `rustAttributeStep`/`rustWordStep`/`rustWhitespaceStep` are separate functions only to keep
  every new function's cognitive complexity ≤ 15. Measured with `eslint-plugin-sonarjs` 4.2.1 at
  threshold 15: the changed file has exactly the four pre-existing violations (lines 87:30, 124:31,
  299→446:18 `scanRustFile`, 501→645:24 `addFrontendPlaceholderFindings`); every new helper and
  `rustFnMatches` are below.
- D3 — `rustPubBodyEnd`/`rustFnTailAt` mirror the old modifier grammar exactly, including the
  `pub (crate)` whitespace, the `pub` fall-through to `async`/`fn`, and the `<[^>{}]*>` generics that
  stop at `>`/`{`/`}`. `isRustIdentifierStart`/`rustIdentifierEnd` are the char-code forms of the old
  `/[A-Za-z_]/`/`/[A-Za-z0-9_]/`; `\s` in the loop and `skipWhitespace` are unchanged.
- D4 — `rustFnMatches` returns an array of the same four values the body uses (`index`, `end`,
  `attrs`, `name`); the body keeps its formulas (`fnStart = index + slice(index, end).lastIndexOf('fn ')`,
  body search from `end`, `lineOf(source, index)`).
- D5 — each forward scan is a `makeForwardFinder` closure that remembers where its last scan stopped.
  When calls arrive with non-decreasing starts (the invariant proven in §5.2, and the only pattern the
  caller produces), each source position is visited once per finder: the three round-2 quadratic
  shapes become linear. A non-monotone call falls back to a plain scan, so output is exact regardless
  of the invariant; only the linearity bound relies on it.
- D6 — `rustFnMatches` returns `[]` when the masked source contains no `#`: with no `#` there can be
  no attribute, so `attrs` is always empty and `/#\s*\[\s*test\b/` rejects every record — findings and
  warnings are identical, and attribute-free inputs (including `'fn '.repeat(n)` and `'pub('.repeat(n)`)
  no longer enter the scanner. This is a necessary-condition fast path, not a semantic branch; AC6
  exercises the finders with `#[x]`-bearing shapes as well.
- D7 — whitespace detection uses a char-code fast path for ASCII (`32`, `9-13`) and falls back to
  `/\s/` above 127, preserving ECMAScript `\s` exactly; the `p`/`a`/`f` trigger set is exact because
  only `pub`, `async`, `fn` can start a header.

Rejected alternatives (no open decision):
- round-2 scanner (`indexOf`/forward loops restarted per start): super-linear, measured in §2.2;
- next-occurrence tables (three `Int32Array` passes per file): correct but slower than the finders
  (fn 122.1 vs 115.3 ms at 256k, attr 194.4 vs 179.0 ms, corpus 2964.6 vs 2733.1 ms) and 12 bytes per
  char; rejected;
- generator + per-char `rustStepAt` object: allocates per character (matcher 36.9 ms vs 16.5 ms on
  768 KB of `'fn '`), and the round-2 split did not bound the scans anyway;
- lazy/possessive regex or atomic-group emulation: keeps `Move`/complexity and changes captures;
- matching `fn` first and reconstructing the prefix backwards: reorders matches for `fn` text inside
  attribute content (counterexample: `#[x(fn b()] fn a(`); rejected.

## 5. Why this is equivalent

Old match records are `(index, end, attrs, name)`: the greedy regex starts at the beginning of the
absorbable run (whitespace and complete attributes) and ends after `(`; downstream code uses only
`attrs` (filter `/#\s*\[\s*test\b/`), `name`, `index`, `end` (via `fnStart`, `lineOf`, body search).
The new scanner reproduces every old record whose `attrs` passes that filter, exactly. Records it
drops are (a) old intermediate matches of `fn` text inside a complete `#[...]` attribute (e.g.
`#[x(fn b()] ...`), whose capture is whitespace-only and can never pass the filter, and (b) records
from sources with no `#` at all (D6), which likewise can never carry a test attribute. The new
scanner never adds a record. Findings, warnings, ids, lines and allowlist comparison are identical.

### 5.1 Finder lemma

`makeForwardFinder(source, isNeedle)` returns the first `isNeedle` position ≥ `start` for every call
whose `start` is ≥ the previous call's `start`; a call with a smaller `start` takes the exact
fallback scan. Proof of the fast path: after a scan from `s` stopped at `p` (a needle) or at EOF,
positions `[s, p)` contain no needle. For the next call with `start' ≥ s`: if `start' > p` the loop
re-scans (correct), otherwise `start' ≤ p` and the memoized `p` is the first needle ≥ `start'`
because no needle lies in `[start', p)`. EOF: after any scan reaches EOF no needle exists in
`[s, length)`, so every later `start' ≥ s` returns `-1`; a smaller `start` uses the fallback.

### 5.2 Start monotonicity (why the fast path is the only one taken)

The loop advances `cursor` strictly; every finder call start is ≥ the current `cursor`. For each
finder the next call cannot start before the previous one:
- bracket (`rustAttributeEnd` at `#` at `c`): a call means `[` is the first non-whitespace after
  `c`, so no `#` exists in `(c, open)`; the next `#` is at ≥ `open + 1` = the previous start;
- paren (`rustPubBodyEnd` at `pub` at `c`): a call means `(` is the first non-whitespace after
  `pub` (or after `pub(...)`); any later `pub` that can call is beyond that `(`;
- generic (`rustFnTailAt`): a call parses a complete `[pub[..]] [async] fn name` tail ending in `<`;
  at most one such call per `cursor` (the pub-path fall-through never reaches the generic scan), and
  after the `<` the next keyword start is beyond the previous start.
Planning-time this invariant was not just argued: a scratch copy whose finder throws on any
non-monotone start was swept over the full 22,942,418 inputs and the 922-file repo corpus (raw and
masked) with no throw, so the fallback is dead code on the tested space (it remains as an exactness
guard). Only §5.2 makes the fast path O(N); §5.1 makes the fallback exact.

### 5.3 Planning-time evidence (all scratch only, never committed)

| Harness | Volume | Result |
|---|---|---|
| Full `scanRustFile` output, old vs new | exhaustive A8 = `' # [ ] f n ( )'` len ≤ 8: 19,173,961 strings | 0 failures |
| Full `scanRustFile` output, old vs new | exhaustive A20 len ≤ 5: 3,368,421 strings | 0 failures |
| Full `scanRustFile` output, old vs new | token fuzz 200,000 + structured fuzz 200,000 | 0 failures |
| Full output, review shapes | 12 shapes x 3 sizes (plain and `#[x]`-bearing) | 0 failures; `tested=22942418 failures=0` |
| Non-monotone assertion build | same 22,942,418 inputs | 0 throws (fast path always taken) |
| Full output, repo corpus | 922 files raw + 922 masked (`.rs .ts .tsx .md .json .yml .yaml .html .css .mjs .js`) | 0 failures; masking byte-identical |
| Corpus timing (416 scanned files) | old vs new | 5835.2 ms → 2723.2 ms; output sha256 `42167de47f2a9ff0` both |
| CLI `npm run test:debt` stdout/stderr/exit | repo | byte-identical vs base; 34 stdout lines, exit 0 |

Edge cases proven equal in the same sweep: `pub (crate)   async  fn f<T>(`, `#[a]#[b]fn f(`,
`# [test]`, `#[x(fn b()] fn a(`, `#[cfg(#[test fn b()] fn c(`, `fn foo<fn bar>(`,
`pub(crate)async fn`, `fn` inside comments (masked), unterminated attributes/parens/generics, and
every record whose `attrs` matches `/#\s*\[\s*test\b/`.

## 6. Verification (objective acceptance criteria)

Run every command from the repo root (`REPO="$(pwd)"`). Scratch artifacts live in the replica-local
scratch dir (allowed zone, never committed):

```bash
SCRATCH="$AGENTSCOMMANDER_ROOT/scratch/2088-proof"
mkdir -p "$SCRATCH/sonar"
cp scripts/check-test-debt.mjs "$SCRATCH/base.mjs"
```

**AC1 — self-test.** `npm run test:debt:self` prints `check-test-debt self-test passed` and exits 0.

**AC2 — report identity.** Capture BEFORE on the untouched base, apply section 4, capture AFTER:

```bash
npm run --silent test:debt > "$SCRATCH/debt-before.out" 2> "$SCRATCH/debt-before.err"; echo $? > "$SCRATCH/debt-before.code"
# apply section 4
npm run --silent test:debt > "$SCRATCH/debt-after.out"  2> "$SCRATCH/debt-after.err";  echo $? > "$SCRATCH/debt-after.code"
cmp "$SCRATCH/debt-before.out" "$SCRATCH/debt-after.out" && cmp "$SCRATCH/debt-before.err" "$SCRATCH/debt-after.err" \
  && cmp "$SCRATCH/debt-before.code" "$SCRATCH/debt-after.code" && echo IDENTICAL
npm run test:debt; echo "npm exit=$?"
```

Expected: `IDENTICAL`; `npm exit=0`; 34 stdout lines incl. `Ignored Rust tests: 24 discovered, 24 allowlisted, 0 unallowlisted`,
`Placeholder tests: 7 discovered, 7 allowlisted, 0 unallowlisted`,
`Skipped frontend tests: 0 discovered, 0 allowlisted, 0 unallowlisted`.

**AC3-AC5 — the three Sonar rules, local judge (same engine as SonarCloud).**

```bash
cd "$SCRATCH/sonar" && npm init -y >/dev/null
npm i --no-save eslint@9.39.1 eslint-plugin-sonarjs@4.2.1 scslre@0.3.0 @eslint-community/regexpp@4.12.2
cp "$SCRATCH/base.mjs" base.mjs && cp "$REPO/scripts/check-test-debt.mjs" head.mjs
```

Calibration (why this judge is admissible): on the untouched base it reports exactly
`regex-complexity 287 (23)`, `super-linear-regex 307`, `regex-complexity 307 (49)` — matching the
SonarCloud PR keys and the main-branch line 287. `$SCRATCH/sonar/sonar-check.mjs`:

```js
import fs from 'node:fs';
import { Linter } from 'eslint';
import plugin from 'eslint-plugin-sonarjs';

const linter = new Linter({ configType: 'flat' });
const config = {
  plugins: { sonarjs: plugin },
  rules: {
    'sonarjs/regex-complexity': 'error',
    'sonarjs/slow-regex': 'error',
    'sonarjs/super-linear-regex': 'error',
  },
};
for (const file of process.argv.slice(2)) {
  const messages = linter.verify(fs.readFileSync(file, 'utf8'), config);
  console.log(`=== ${file}: ${messages.length} ===`);
  for (const m of messages) console.log(`  ${m.ruleId} line ${m.line}:${m.column} ${m.message}`);
}
```

Run `node sonar-check.mjs base.mjs head.mjs` from `$SCRATCH/sonar`. AC3 (S5852): no `slow-regex`
report on the changed file, and `scslre` returns zero reports for every regex literal in the changed
code (verified above: `/\s/` and the unchanged `skipWhitespace` `/\s/` are `[]`). AC4 (S8786): base
has `super-linear-regex` at 307; changed file has none. AC5 (S5843): changed file has exactly one
report — `regex-complexity 287 (23)`, pre-existing and not in the PR; no changed line has a regex
literal above 20. Direct `scslre` cross-check (write as `$SCRATCH/sonar/scslre-literals.mjs`):

```js
import { analyse } from 'scslre';
for (const source of ['\\s']) console.log(source, analyse({ source, flags: '' }).reports);
```

Cognitive complexity (new-code gate): run the same plugin with
`'sonarjs/cognitive-complexity': ['error', 15]` on `head.mjs`; expected exactly the four
pre-existing violations (87, 124, `scanRustFile`, `addFrontendPlaceholderFindings`), none on a new
helper.

**AC6 — runtime matrix (the concern behind S8786).** `$SCRATCH/bench.mjs` (scratch only; run with
cwd `$SCRATCH`), built on the AC7 libraries:

```js
import fs from 'node:fs';
import * as oldLib from './old-lib.mjs';
import * as newLib from './new-lib.mjs';

// N is the repeat count; len = N * unit (plus tail). The `#`-bearing shapes reach
// rustFnMatches and exercise the attribute/paren/generic finders; the `#`-less ones
// take the no-attribute fast path.
const shapes = {
  "attr '#[x] '": { make: (n) => '#[x] '.repeat(n) + 'y', sizes: [4000, 8000, 16000, 32000] },
  ws: { make: (n) => ' '.repeat(n) + 'y', sizes: [32000, 64000, 128000, 256000] },
  "bracket '#['": { make: (n) => '#['.repeat(n), sizes: [64000, 128000, 256000] },
  "pub 'pub('": { make: (n) => 'pub('.repeat(n), sizes: [64000, 128000] },
  "generic 'fn a<'": { make: (n) => 'fn a<'.repeat(n), sizes: [32000, 64000, 128000] },
  "fn 'fn '": { make: (n) => 'fn '.repeat(n) + 'y', sizes: [64000, 128000, 256000, 512000] },
  "attr+pub '#[x]pub('": { make: (n) => '#[x]pub('.repeat(n), sizes: [16000, 32000, 64000] },
  "attr+generic '#[x]fn a<'": { make: (n) => '#[x]fn a<'.repeat(n), sizes: [16000, 32000, 64000] },
  "attr+bracket '#[x]#['": { make: (n) => '#[x]#['.repeat(n), sizes: [16000, 32000, 64000] },
  "attr+pub-close '#[x]pub('+')'": { make: (n) => '#[x]pub('.repeat(n) + ')', sizes: [8000, 16000, 32000] },
  "attr+generic-close '#[x]fn a<'+'>'": { make: (n) => '#[x]fn a<'.repeat(n) + '>', sizes: [8000, 16000, 32000] },
};

const realRead = fs.readFileSync;
let source = '';
fs.readFileSync = (p, ...rest) => (p === '/fake/x.rs' ? source : realRead(p, ...rest));

function best(lib, n, make) {
  source = make(n);
  let ms = Infinity;
  for (let i = 0; i < 3; i += 1) {
    const t0 = process.hrtime.bigint();
    lib.scanRustFile('/fake', '/fake/x.rs');
    ms = Math.min(ms, Number(process.hrtime.bigint() - t0) / 1e6);
  }
  return ms;
}

for (const [name, shape] of Object.entries(shapes)) {
  for (const n of shape.sizes) {
    const base = best(oldLib, n, shape.make);
    const head = best(newLib, n, shape.make);
    console.log(`${name}\tN=${n}\tbase=${base.toFixed(1)}ms\thead=${head.toFixed(1)}ms\thead/base=${(head / base).toFixed(3)}`);
  }
}
```

Run `cd "$SCRATCH" && node bench.mjs`. Pass conditions:
1. **No shape slower than base**: `head/base ≤ 1.0` on every row (planning-time worst row: `fn 'fn '`
   N=64k, 0.865; the quadratic families are ≤ 0.032).
2. **Linear head**: head grows ≤ 2.75x per doubling on every shape (planning-time: ~2.1x everywhere)
   while base grows ~4x on the quadratic families (e.g. `'#['`: 2030.3 → 8092.6 → 32319.1 ms). This
   separates linear from quadratic with a full 1.2x margin below the measured base growth.
3. **The finds are real**: the `#[x]`-bearing rows exercise all three finders; base there is
   quadratic (16k/32k/64k for `#[x]pub(`: 1040.0/4129.9/16638.8 ms) while head is
   18.4/39.2/76.2 ms.

Planning-time full output (min of 3, Node v22.23.2, in-process):

| shape | N | base | head | head/base |
|---|---|---|---|---|
| `'#[x] '` | 4k / 8k / 16k / 32k | 252.6 / 955.4 / 3919.6 / 15633.5 | 3.0 / 6.3 / 10.4 / 20.0 | 0.012 / 0.007 / 0.003 / 0.001 |
| `' '` | 32k / 64k / 128k / 256k | 513.9 / 2067.3 / 8471.4 / 35806.7 | 2.9 / 5.8 / 13.9 / 31.7 | 0.006 / 0.003 / 0.002 / 0.001 |
| `'#['` | 64k / 128k / 256k | 2030.3 / 8092.6 / 32319.1 | 16.3 / 35.3 / 67.3 | 0.008 / 0.004 / 0.002 |
| `'pub('` | 64k / 128k | 4086.0 / 16849.1 | 32.7 / 63.8 | 0.008 / 0.004 |
| `'fn a<'` | 32k / 64k / 128k | 1727.2 / 6854.1 / 27258.8 | 19.5 / 42.7 / 81.5 | 0.011 / 0.006 / 0.003 |
| `'fn '` | 64k / 128k / 256k / 512k | 25.0 / 50.1 / 108.3 / 218.9 | 21.6 / 47.1 / 100.5 / 194.8 | 0.865 / 0.939 / 0.928 / 0.890 |
| `'#[x]pub('` | 16k / 32k / 64k | 1040.0 / 4129.9 / 16638.8 | 18.4 / 39.2 / 76.2 | 0.018 / 0.009 / 0.005 |
| `'#[x]fn a<'` | 16k / 32k / 64k | 1553.1 / 6166.8 / 24483.5 | 21.2 / 41.6 / 86.0 | 0.014 / 0.007 / 0.004 |
| `'#[x]#['` | 16k / 32k / 64k | 1441.1 / 5811.2 / 23165.1 | 11.4 / 25.1 / 49.6 | 0.008 / 0.004 / 0.002 |
| `'#[x]pub('+')'` | 8k / 16k / 32k | 260.2 / 1029.5 / 4094.6 | 8.4 / 17.4 / 35.8 | 0.032 / 0.017 / 0.009 |
| `'#[x]fn a<'+'>'` | 8k / 16k / 32k | 391.6 / 1564.6 / 6168.4 | 9.8 / 18.7 / 39.5 | 0.025 / 0.012 / 0.006 |

**AC7 — equivalence harness (reproduces §5.3).** Build the two libs from the base snapshot and the
changed file:

```bash
sed '/^try {$/,$d' "$SCRATCH/base.mjs" > "$SCRATCH/old-lib.mjs"
printf 'export { scanRustFile, scanFrontendFile, scan, maskComments, maskCommentsAndStrings, lineOf, skipWhitespace };\n' >> "$SCRATCH/old-lib.mjs"
sed '/^try {$/,$d' "$REPO/scripts/check-test-debt.mjs" > "$SCRATCH/new-lib.mjs"
printf 'export { scanRustFile, maskComments, maskCommentsAndStrings, rustFnMatches, rustAttributeEnd, rustPubBodyEnd, rustFnHeaderAt, rustFnTailAt, rustAttributeStep, rustWordStep, isRustIdentifierStart, rustIdentifierEnd, makeForwardFinder };\n' >> "$SCRATCH/new-lib.mjs"
```

`$SCRATCH/equiv.mjs` (scratch only; run with cwd `$SCRATCH`):

```js
import fs from 'node:fs';
import * as oldLib from './old-lib.mjs';
import * as newLib from './new-lib.mjs';

const realRead = fs.readFileSync;
let source = '';
fs.readFileSync = (p, ...rest) => (p === '/fake/x.rs' ? source : realRead(p, ...rest));

let tested = 0;
function check(text) {
  source = text;
  tested += 1;
  const a = JSON.stringify(oldLib.scanRustFile('/fake', '/fake/x.rs'));
  const b = JSON.stringify(newLib.scanRustFile('/fake', '/fake/x.rs'));
  if (a !== b) {
    console.log('MISMATCH', JSON.stringify(text));
    console.log('old', a);
    console.log('new', b);
    process.exit(1);
  }
}

function exhaustive(alphabet, maxLen) {
  const n = alphabet.length;
  const idx = new Array(maxLen).fill(0);
  for (let len = 0; len <= maxLen; len += 1) {
    idx.fill(0);
    for (;;) {
      let s = '';
      for (let i = 0; i < len; i += 1) s += alphabet[idx[i]];
      check(s);
      let pos = len - 1;
      while (pos >= 0) {
        idx[pos] += 1;
        if (idx[pos] < n) break;
        idx[pos] = 0;
        pos -= 1;
      }
      if (pos < 0) break;
    }
  }
}

let seed = 123456789;
const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
function fuzz(tokens, iterations, maxTokens) {
  for (let i = 0; i < iterations; i += 1) {
    const count = 1 + Math.floor(rnd() * maxTokens);
    let s = '';
    for (let j = 0; j < count; j += 1) s += tokens[Math.floor(rnd() * tokens.length)];
    check(s);
  }
}

exhaustive([' ', '#', '[', ']', 'f', 'n', '(', ')'], 8);
exhaustive([' ', '\n', '#', '[', ']', 'f', 'n', 'p', 'u', 'b', 'a', 's', 'y', 'c', '(', ')', '<', '>', '_', '{'], 5);
fuzz(['#[', ']', ' ', '\n', '\t', 'fn ', 'fn', 'foo(', 'pub', 'pub ', 'pub(crate) ', 'async ', 'async', '#[test]', 'x', '(', ')', '<T>', '<', '>', '{', '}', '_', '#[a]#[b]'], 200000, 10);
fuzz(['#[test]\n', '#[ignore]\n', '#[cfg(test)]\n', '#[tokio::test]\n', 'fn ', 'foo', 'bar', '(', ')', ' { ', '}', '\n', 'pub ', 'async ', 'pub(crate) ', ' ', '  ', '\n\n', '<T>', '', 'x', '_9', 'fn ', '#[', ']', '[x', '] ', 'fn x('], 200000, 14);
// adversarial shapes from review round 2 (plain and #-bearing)
for (const n of [10, 50, 200]) {
  for (const make of [
    (k) => '#['.repeat(k),
    (k) => 'pub('.repeat(k),
    (k) => 'fn a<'.repeat(k),
    (k) => '#['.repeat(k) + ']',
    (k) => 'pub('.repeat(k) + ')',
    (k) => 'fn a<'.repeat(k) + '>',
    (k) => '#[x]pub('.repeat(k),
    (k) => '#[x]fn a<'.repeat(k),
    (k) => '#[x]#['.repeat(k),
    (k) => '#[x] '.repeat(k),
    (k) => ' '.repeat(k),
    (k) => 'fn '.repeat(k),
  ]) check(make(n));
}
console.log(`tested=${tested} failures=0`);
```

Run `cd "$SCRATCH" && node equiv.mjs`. Pass condition: `tested=22942418 failures=0`
(19,173,961 + 3,368,421 + 200,000 + 200,000 + 36 shapes). Also run the repo corpus comparison
(922 files raw + 922 masked, `JSON.stringify` per file, masking byte-identical) and
`node scripts/check-test-debt.mjs` vs `node "$SCRATCH/base.mjs"` output (`cmp`). Planning-time:
`files=922 maskedFiles=922 failures=0`; 416 scanned files 5835.2 ms → 2723.2 ms with identical
output. Optional non-monotone check: a scratch copy of `new-lib.mjs` whose finder throws instead of
falling back must complete the same sweep without throwing (§5.2).

**AC8 — footprint.**

```bash
git diff --stat        # 1 file changed, 152 insertions(+), 8 deletions(-)
git diff --name-only   # scripts/check-test-debt.mjs
git diff --check       # no output
git status --porcelain # only the plan (force-added) and no source changes
```

**AC9 — SonarCloud final (the actual judge).** After the implementation commit is pushed:

```bash
curl -s "https://sonarcloud.io/api/qualitygates/project_status?projectKey=mblua_AgentsCommander&pullRequest=2099"
curl -s "https://sonarcloud.io/api/issues/search?componentKeys=mblua_AgentsCommander&pullRequest=2099&resolved=false"
```

Pass condition: `"status":"OK"` (new maintainability rating 1) and the issues search shows neither
`AaCqh-uoqWlJPkFQ9mDs` nor `AaCqh-uoqWlJPkFQ9mDt` (expected `total: 0`).

## 7. Risks and rollback

- Blast radius: the developer debt report only. A wrong matcher would change findings/ids/lines, but
  AC2 pins the report byte-for-byte and AC7 sweeps ~23M inputs plus the repo corpus (raw and masked).
- The one non-local mechanism is §5.2 (monotone starts make the memoized finders linear). It is
  argued, swept with an assertion build, and the fallback keeps output exact even if it were ever
  violated; the only consequence would be losing the linear bound, not a wrong report.
- D6 drops the matcher for `#`-free sources; a report difference is impossible because no record can
  carry a test attribute there (proved by AC2/AC7, which include `#`-free alphabets).
- New-code smells: every new helper is below the SonarJS thresholds (cognitive ≤ 15; `/\s/` only).
  The only remaining file issues are pre-existing and untouched (line 287 `S5843`, `scanRustFile`
  `S3776`).
- No product, IPC, persistence, release or CI-contract change; the script has no dependencies.
- Revert is one commit; no migration, no rollout.

## 8. Implementation order

1. Snapshot the base script and capture the BEFORE report (AC2); build the scratch judge (AC3-AC5).
2. Apply exactly section 4.
3. Run AC1, AC2, AC3-AC5, AC6, AC7, AC8; keep raw outputs.
4. Commit the source as `fix(2088): bound every scan in the test-debt fn matcher` plus the plan
   (`git add -f plans/2088-sonar-regex-backtracking.md`); push the branch.
5. Poll AC9 until SonarCloud re-analyses the PR; keep the API JSON as evidence.
6. Reply to `ac-tech-lead-v4` with plan path, commit SHA, Lite class, AC1-AC9 evidence and the
   SonarCloud gate JSON.

## Plan Contract

No TBD, no open decision, no competing alternative: section 4 is the sole change. The single changed
file is `scripts/check-test-debt.mjs`; the matcher moves from regex to code, with the same findings
and warnings. Every acceptance criterion is a command with an objective pass condition; AC6 now
covers the three shapes review round 2 found super-linear (plus their `#`-bearing variants, which
exercise the finders) and requires head ≤ base on every row; AC9 is the external judge. The only new
file is this plan.
