# Plan #2088 (round 5, bar B): replace the `fn` regex in `scripts/check-test-debt.mjs` with an exact, memoized matcher

Status: READY_FOR_IMPLEMENTATION (conditional: coordinator accepts AC6 condition 3, see Plan Contract)

- Issue: https://github.com/mblua/AgentsCommander/issues/2088. PR: https://github.com/mblua/AgentsCommander/pull/2099
  (open; do not merge). Branch `fix/2088-sonar-regex-backtracking`, pinned plan base
  `bd7fe5cc` (= remote head at authoring, 2026-09-16 UTC). Source tree under `scripts/` is `8a2f81f7`
  (round-1 regex); PR merge base `5203c4e3` ("main"). Line numbers refer to the `8a2f81f7` tree;
  if a quoted line moved, re-anchor on the quoted text.
- Requirement: clear SonarCloud `javascript:S5852` key `AaBBZr7CbbRnCQnTRHkL` (main, line 307, `fnRe` in
  `scanRustFile`) with the PR quality gate green: no new `S8786`, `S5843`, `S3776` or other new-code
  issue; `npm run test:debt` unchanged; no suppression, no threshold change.
- User decision (round 4): **bar B** — exact equivalence; worst case O(n^2) accepted and documented;
  acceptance = no input shape slower than base; the round-2 `fn a<` regression fixed.
- Class **Lite**; band 1-25; threat model **routine** (developer script, no product/IPC/release
  surface; no enhanced delivery control applies). Owner `ac-dev-rust-v4`, coordinator
  `ac-tech-lead-v4`; grinch reviews the proofs. Partition: not required (Lite, one file, one phase).
- `/plans/` is gitignored: commit with `git add -f plans/2088-sonar-regex-backtracking.md`.

## 1. Decision

Delete the round-1 literal `fnRe` and its `exec` loop. Add eight small functions and six single-purpose
regex constants that reproduce **only the matches that carry attributes**, in source order, with the
exact `index`, `end`, `attrs` and `name` the old regex produced. Attribute-free matches are still
skipped over (they consume text exactly as before) but are not returned: the caller discards them
anyway, because `/#\s*\[\s*test\b/` cannot match an empty capture.

Every unbounded forward search (`]`, `)`, `>`/`{`/`}`) goes through one cache, `rustScanFrom`; failed
attribute chains are cached in a `Set`; the leftmost attribute-free header is cached until the cursor
passes it. Result: +125/-8 lines in one file, not the ~90 estimated in round 4 — the extra ~35 lines
are the caches, without which review shapes stay as slow as base (measured, §2.3).

Worst case is **O(n^2)** and documented in the code: each cached search is at most O(n) and a cache
miss happens at most once per call, with O(n) calls. On every super-linear review shape (§6 AC6) head
grows ~2x per doubling and is ≤ 0.05x base; on linear shapes it is within measurement noise of base.

## 2. Verified cause and history

### 2.1 Why the regex must go

On the `8a2f81f7` literal, `eslint-plugin-sonarjs@4.2.1` (same engine as SonarCloud, `scslre@0.3.0`)
reports `super-linear-regex` (S8786) and `regex-complexity` 49 (S5843); on the `5203c4e3` literal it
reports `slow-regex` (S5852) and complexity 36. A regex-only rewrite cannot clear both: the attribute
prefix alone has complexity 22 and any unanchored `\s*`-prefixed start yields a `Move` report
(round-3 table, unchanged).

### 2.2 Rounds 2-3 failed, and round 3 was also not equivalent

- Round 2 (`fbf1c438`, per-start `indexOf` scanner): quadratic on `'#['`, `'pub('`, `'fn a<'`; slower
  than base on `'fn a<'` (32k: base 1786 ms, R2 3072 ms).
- Round 3 (`bd7fe5cc`, memoized forward scanner): super-linear on `'#pub(fn a<) fn b<'.repeat(n)+'>'`.
  **New in round 5:** round 3 was also not exact. It resumed after a failed attribute run instead of
  retrying inside the attribute text, as the regex does. Counterexample, verified against `5203c4e3`:
  `#[a fn b<] x #[test] fn t(>( {}` — base reports no finding (its `fn b<...>(` match inside the first
  attribute swallows `#[test]`), round 3 reports `placeholder-rust-test` `rust:x.rs::t`. Its
  22.9M-input sweep never reached that length.
- `5203c4e3` itself is **exponential** on `'#[x] '.repeat(k)` followed by a non-header (k=25: 6.3 s),
  i.e. S5852 is real. Round 3's "base" column was `8a2f81f7`. Round 5 measures against both.

### 2.3 Planning-time prototypes rejected in round 5

| candidate | outcome |
|---|---|
| R3 structure with native `indexOf` finders (~90 lines) | not exact (counterexample above) |
| exact leftmost matcher without caches (+90 lines) | `'#[x]#['` 4.33x slower than base; `'#[x]pub('` 0.97-1.00x |
| char-by-char JS scanner | 1.05-1.2x slower than base on `'#' + 'fn '.repeat(n)` |
| exact matcher + caches, first cut (+118 lines) | ≤ base on adversarial rows; matcher-only 1.4x slower than the regex loop on dense attribute-free headers |
| the decided design (+125 lines) | exact on 23.2M inputs; adversarial rows ≤ 0.05x; linear rows 0.89-1.16 (noise) |

## 3. Scope

In scope: `scripts/check-test-debt.mjs` only — the helper block below and the matcher lines of
`scanRustFile`. Out of scope (binding): every other function and regex (including line-287 `S5843`,
pre-existing on main), the pre-existing quadratic body search (`masked.indexOf('{')`,
`findMatchingBrace`, `lineOf`), `test-debt.allowlist.json`, `package.json`, tests, workflows.

## 4. Exact change

### 4.1 Insert before `function scanRustFile(root, filePath) {` (base line 299), after the blank line that follows `hasExecutableRustBody`

```js
// Pieces of the former single `fn` regex. Every unbounded close search goes through rustScanFrom.
const RUST_BRACKET_CLOSE = /\]/g;
const RUST_PAREN_CLOSE = /\)/g;
const RUST_GENERICS_STOP = /[>{}]/g;
const RUST_FN_CANDIDATE = /pub\s*\(|(?:async\s+)?fn\s+[A-Za-z_]\w*\s*[(<]/g;
const RUST_FN_TAIL = /(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)\s*[(<]/y;
const RUST_FN_TAIL_AFTER_PUB_PAREN = /(?:async\s+)?fn\s+([A-Za-z_]\w*)\s*[(<]/y;

// First `stop` match at or after `from`, or -1. `scan` keeps the last answer: no match lies in
// [scan.from, scan.at), so it is reused for any `from` in [scan.from, scan.at].
function rustScanFrom(source, from, scan, stop) {
  if (from < scan.from || (scan.at !== -1 && from > scan.at)) {
    stop.lastIndex = from;
    const hit = stop.exec(source);
    scan.from = from;
    scan.at = hit === null ? -1 : hit.index;
  }
  return scan.at;
}

function rustFnTailAt(source, index, tail, memo) {
  tail.lastIndex = index;
  const match = tail.exec(source);
  if (match === null) return null;
  let open = tail.lastIndex - 1;
  if (source[open] === '<') {
    const close = rustScanFrom(source, open + 1, memo.generics, RUST_GENERICS_STOP);
    if (close === -1 || source[close] !== '>') return null;
    open = skipWhitespace(source, close + 1);
  }
  return source[open] === '(' ? { end: open + 1, name: match[1] } : null;
}

// `pub(?:\s*\([^)]*\))?\s+`, `async\s+`, `fn name`, `<[^>{}]*>`, `(` at `index`: { end, name } or null.
function rustFnHeaderAt(source, index, memo) {
  if (source.startsWith('pub', index)) {
    const open = skipWhitespace(source, index + 3);
    const close = source[open] === '(' ? rustScanFrom(source, open + 1, memo.parens, RUST_PAREN_CLOSE) : -1;
    const body = close === -1 ? -1 : skipWhitespace(source, close + 1);
    const header = body > close + 1 ? rustFnTailAt(source, body, RUST_FN_TAIL_AFTER_PUB_PAREN, memo) : null;
    if (header !== null) return header;
  }
  return rustFnTailAt(source, index, RUST_FN_TAIL, memo);
}

// Leftmost fn header at or after `from`, kept in memo.headerIndex/memo.headerEnd (-1: none).
function rustFindFnHeader(source, from, memo) {
  memo.headerIndex = -1;
  RUST_FN_CANDIDATE.lastIndex = from;
  for (let hit = RUST_FN_CANDIDATE.exec(source); hit !== null; hit = RUST_FN_CANDIDATE.exec(source)) {
    const simple = hit[0][0] !== 'p' && hit[0].endsWith('(');
    const header = simple ? null : rustFnHeaderAt(source, hit.index, memo);
    if (simple || header !== null) {
      memo.headerIndex = hit.index;
      memo.headerEnd = simple ? RUST_FN_CANDIDATE.lastIndex : header.end;
      return;
    }
    RUST_FN_CANDIDATE.lastIndex = hit.index + 1;
  }
}

// End of the leftmost fn header starting in [cursor, hash), or -1. A `pub ` or `pub async ` prefix is
// found at its `async`/`fn`, which cannot change the comparison with `hash`.
function rustFnHeaderEndBefore(source, cursor, hash, memo) {
  if (memo.headerIndex === -2 || (memo.headerIndex >= 0 && memo.headerIndex < cursor)) {
    rustFindFnHeader(source, cursor, memo);
  }
  return memo.headerIndex >= 0 && memo.headerIndex < hash ? memo.headerEnd : -1;
}

// End of the `#\s*\[[^\]]*\]` attribute at `index`, or -1.
function rustAttributeEnd(source, index, memo) {
  const open = skipWhitespace(source, index + 1);
  if (source[open] !== '[') return -1;
  const close = rustScanFrom(source, open + 1, memo.brackets, RUST_BRACKET_CLOSE);
  return close === -1 ? -1 : close + 1;
}

// The former regex's match at the whitespace before `hash` (not before `cursor`): attributes from `hash`,
// whitespace, a fn header. Positions whose attribute chain already failed are kept in memo.failed.
function rustAttributedFnAt(source, cursor, hash, memo) {
  const chain = [];
  let end = hash;
  let attrEnd = rustAttributeEnd(source, hash, memo);
  while (attrEnd !== -1 && !memo.failed.has(end)) {
    chain.push(end);
    end = skipWhitespace(source, attrEnd);
    attrEnd = source[end] === '#' ? rustAttributeEnd(source, end, memo) : -1;
  }
  const header = chain.length === 0 || memo.failed.has(end) ? null : rustFnHeaderAt(source, end, memo);
  if (header === null) {
    for (const position of chain) memo.failed.add(position);
    memo.failed.add(end);
    return null;
  }
  let runStart = hash;
  while (runStart > cursor && /\s/.test(source[runStart - 1])) runStart -= 1;
  return { index: runStart, end: header.end, attrs: source.slice(runStart, end), name: header.name };
}

// Matches of the former `fn` regex that carry attributes ({ index, end, attrs, name }), in source order;
// attribute-free matches are only skipped. Worst case O(n^2) when close searches restart behind their
// cached answer; see plans/2088-sonar-regex-backtracking.md.
function rustAttributedFnMatches(source) {
  const matches = [];
  const scan = () => ({ from: Infinity, at: -1 });
  const memo = { headerIndex: -2, headerEnd: -1, failed: new Set(), brackets: scan(), parens: scan(), generics: scan() };
  let cursor = 0;
  let hash = source.indexOf('#');
  while (hash !== -1) {
    const headerEnd = cursor < hash ? rustFnHeaderEndBefore(source, cursor, hash, memo) : -1;
    const match = headerEnd === -1 ? rustAttributedFnAt(source, cursor, hash, memo) : null;
    if (match !== null) matches.push(match);
    if (headerEnd !== -1) cursor = headerEnd;
    else cursor = match === null ? hash + 1 : match.end;
    if (hash < cursor) hash = source.indexOf('#', cursor);
  }
  return matches;
}
```

(The block ends with one blank line before `function scanRustFile`.)

### 4.2 Replace base lines 307-315

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

with

```js
  for (const match of rustAttributedFnMatches(masked)) {
    const attrs = match.attrs;
    if (!/#\s*\[\s*test\b/.test(attrs)) continue;
    const fnName = match.name;
    const fnStart = match.index + masked.slice(match.index, match.end).lastIndexOf('fn ');
    const bodyOpen = masked.indexOf('{', match.end);
```

No other edit. The loop body after `bodyOpen` is untouched (`continue` keeps working in `for...of`).
Expected result: `sha256sum scripts/check-test-debt.mjs` =
`3e476802dd3dc1e4496ebaaa2047c9788082d6a07932bb32e80c2ed1756ad0ae` (base `8a2f81f7` file: `1cce75b522832f09854455b6bf6e6d9fb13371b92bc1e76d0c3f134d7dda9dfc`).

## 5. Why it is exact

Write the old regex as `R = W (A)? W H` with `W = \s*`, `A` = one or more `#\s*\[[^\]]*\]\s*`, and
`H = pub(?:\s*\([^)]*\))?\s+)? (async\s+)? fn\s+NAME\s*(<[^>{}]*>\s*)?\(`. `exec` returns the match at
the leftmost start `p ≥ lastIndex`; the loop resumes at its end.

**L1 — one outcome per start.** At start `p` let `q` be the first non-whitespace position. If
`source[q]` is `#`, `R` matches iff greedy attribute parsing from `q` reads ≥ 1 attribute, ends
(after whitespace) at `e`, and `H` matches at `e`; the capture is `source[p, e)`. Otherwise `R` matches
iff `H` matches at `q`, with an empty capture. Backtracking cannot create another outcome: fewer
attributes leave `H` facing `#`; shorter whitespace leaves `H`/`A` facing whitespace; `[^\]]*\]` and
`[^)]*\)` and `[^>{}]*>` each have one end; in `H`, dropping `pub(...)` leaves `pub\s+` facing `(`,
dropping `pub`/`async` leaves `fn` facing `p`/`a`, and a shorter `\s+`/`NAME`/`\s*` leaves `<`/`(`
facing a name or space character.

**L2 — `H` pieces.** `rustFnHeaderAt(i)` tries `pub` + optional whitespace + `(` + the first `)` + ≥ 1
whitespace + `RUST_FN_TAIL_AFTER_PUB_PAREN`, then `RUST_FN_TAIL` at `i`; `rustFnTailAt` then checks
`<` → first of `>{}` must be `>` → whitespace → `(`. That is `H` at `i` by L1's alternative order. The
sticky regexes end in `[(<]`, which only moves the old `\s*(?:<...>\s*)?\(` decision one character
earlier.

**L3 — the loop is the regex loop, filtered.** Let `c` be the cursor (old `lastIndex`) and `h` the
first `#` at or after `c`. By L1, a start before `h` can only produce an attribute-free match, and a
start in the whitespace run immediately before `h` (not before `c`) produces the same outcome as `h`.
- If an `H` match starts at some `i` in `[c, h)` (non-whitespace, so before that run), the leftmost
  old match is attribute-free and ends where that header ends: skip to its end, return nothing.
  `RUST_FN_CANDIDATE` finds every such `i` (`pub\s*\(`, or `[async ]fn NAME [(<]`); a `pub ` /
  `pub async ` prefix is found at its `async`/`fn`, which is still before `h` because only whitespace
  separates them. A candidate not starting with `p` and ending in `(` is already a complete `H`
  (the `simple` path); any other candidate is checked by `rustFnHeaderAt`. Rejected candidates are
  retried from `i + 1`, so the first accepted one is leftmost.
- Otherwise try `h`. Success returns `{ index: run start, end, attrs: source[run start, e), name }`
  and resumes at `end`. Failure means every start in `[c, h]` fails, so resume at `h + 1` — including
  the text inside the attribute, which is what round 3 skipped.
- No `#` at or after `c`: nothing further can carry an attribute; stop.

**L4 — caches are transparent.** `rustScanFrom` returns the cached `at` only for `from` in
`[scan.from, scan.at]` (or `from ≥ scan.from` when `at = -1`); no match lies in `[scan.from, at)`, so
that is the first match at or after `from`; any other `from` re-searches. `memo.headerIndex` /
`memo.headerEnd` hold the leftmost accepted candidate at or after the cursor that computed it (`-2`
not searched, `-1` none) and stay valid until the cursor passes that index (the cursor never
decreases). `memo.failed` holds positions `e` for which "parse
attributes from `e`, then `H`" failed; that outcome depends on `e` only, so a later chain reaching `e`
fails identically.

**L5 — the caller.** Old records with an empty capture never pass `/#\s*\[\s*test\b/`; all others are
returned with identical `index`, `end`, `attrs`, `name`. `fnStart` uses `slice(index, end)` (=
`match[0]`), `bodyOpen` searches from `end` (= `lastIndex`), `lineOf(source, index)` is unchanged.
Findings, warnings, ids, lines and order are identical.

Machine evidence (AC7): 23,242,436 inputs, 0 differences against `5203c4e3`, including the round-3
counterexample and five more inside-attribute cases; 914 repo files raw + masked (1,828 inputs),
0 differences; CLI output byte-identical.

## 6. Acceptance criteria

Run from the repo root (`REPO="$(pwd)"`). Scratch lives outside the repo, in the implementer's replica:

```bash
SCRATCH="$AGENTSCOMMANDER_ROOT/scratch/2088-proof"
mkdir -p "$SCRATCH/sonar"
git show 5203c4e3:scripts/check-test-debt.mjs > "$SCRATCH/main.mjs"
git show 8a2f81f7:scripts/check-test-debt.mjs > "$SCRATCH/r1.mjs"
```

**AC0 — preconditions.** `git status --porcelain` empty; `git rev-parse HEAD` = the pinned plan commit
on `fix/2088-sonar-regex-backtracking`; `sha256sum scripts/check-test-debt.mjs` = `1cce75b522832f09854455b6bf6e6d9fb13371b92bc1e76d0c3f134d7dda9dfc`.
Any mismatch: stop and report (no edits).

**AC1 — self-test.** After §4: `npm run test:debt:self` prints `check-test-debt self-test passed`, exit 0.

**AC2 — CLI byte identity.**

```bash
npm run --silent test:debt > "$SCRATCH/before.out" 2> "$SCRATCH/before.err"; echo $? > "$SCRATCH/before.code"   # before §4
npm run --silent test:debt > "$SCRATCH/after.out"  2> "$SCRATCH/after.err";  echo $? > "$SCRATCH/after.code"    # after §4
cmp "$SCRATCH/before.out" "$SCRATCH/after.out" && cmp "$SCRATCH/before.err" "$SCRATCH/after.err" \
  && cmp "$SCRATCH/before.code" "$SCRATCH/after.code" && echo IDENTICAL
node "$SCRATCH/main.mjs" | cmp - "$SCRATCH/after.out" && echo SAME_AS_MAIN
```

Expected: `IDENTICAL`, `SAME_AS_MAIN`, exit code `0`, 34 stdout lines, stdout sha256 prefix
`4cec91b4d16b7b99` at the pinned base.

**AC3-AC5 — local Sonar judge (S5852, S8786, S5843, S3776).**

```bash
cd "$SCRATCH/sonar" && npm init -y >/dev/null
npm i --no-save eslint@9.39.1 eslint-plugin-sonarjs@4.2.1 scslre@0.3.0 @eslint-community/regexpp@4.12.2
cp "$SCRATCH/main.mjs" main.mjs && cp "$SCRATCH/r1.mjs" r1.mjs && cp "$REPO/scripts/check-test-debt.mjs" head.mjs
node sonar-check.mjs main.mjs r1.mjs head.mjs && node scslre-literals.mjs
```

`sonar-check.mjs`:

```js
import fs from 'node:fs';
import { Linter } from 'eslint';
import plugin from 'eslint-plugin-sonarjs';
const linter = new Linter({ configType: 'flat' });
const config = { plugins: { sonarjs: plugin }, rules: { 'sonarjs/regex-complexity': 'error', 'sonarjs/slow-regex': 'error', 'sonarjs/super-linear-regex': 'error', 'sonarjs/cognitive-complexity': ['error', 15] } };
for (const file of process.argv.slice(2)) {
  const messages = linter.verify(fs.readFileSync(file, 'utf8'), config);
  console.log(`=== ${file}: ${messages.length} ===`);
  for (const m of messages) console.log(`  ${m.ruleId} line ${m.line}:${m.column} ${m.message}`);
}
```

`scslre-literals.mjs`:

```js
import { analyse } from 'scslre';
const literals = [
  ['\\]', 'g'],
  ['\\)', 'g'],
  ['[>{}]', 'g'],
  ['pub\\s*\\(|(?:async\\s+)?fn\\s+[A-Za-z_]\\w*\\s*[(<]', 'g'],
  ['(?:pub\\s+)?(?:async\\s+)?fn\\s+([A-Za-z_]\\w*)\\s*[(<]', 'y'],
  ['(?:async\\s+)?fn\\s+([A-Za-z_]\\w*)\\s*[(<]', 'y'],
];
for (const [source, flags] of literals) {
  console.log(`/${source}/${flags}`, JSON.stringify(analyse({ source, flags }).reports.map((r) => r.type)));
}
```

Calibration: `main.mjs` reports `slow-regex 307` and `regex-complexity 307 (36)`; `r1.mjs` reports
`super-linear-regex 307` and `regex-complexity 307 (49)`; both also report the pre-existing
`regex-complexity 287 (23)` and cognitive complexity at lines 87 (30), 124 (31), 299 (18), 501 (24).
Pass: `head.mjs` reports **exactly five** messages — `cognitive-complexity` 87 (30), 124 (31),
`scanRustFile` (18, unchanged value, line moves to 419), `addFrontendPlaceholderFindings` (24, line
618), and `regex-complexity 287 (23)` — and no `slow-regex`, `super-linear-regex`, or
`regex-complexity` on a new line; every new function is ≤ 15. `scslre-literals.mjs` prints `[]` for
all six literals.

**AC6 — runtime matrix (bar B).** Build libraries, then run the bench (cwd `$SCRATCH`, ~35 min):

```bash
for v in main r1; do sed '/^try {$/,$d' "$SCRATCH/$v.mjs" > "$SCRATCH/$v-lib.mjs"; printf 'export { scanRustFile };\n' >> "$SCRATCH/$v-lib.mjs"; done
sed '/^try {$/,$d' "$REPO/scripts/check-test-debt.mjs" > "$SCRATCH/head-lib.mjs"; printf 'export { scanRustFile };\n' >> "$SCRATCH/head-lib.mjs"
cd "$SCRATCH" && node bench.mjs | tee bench.out
```

`bench.mjs`:

```js
import fs from 'node:fs';
import * as mainLib from './main-lib.mjs';
import * as r1Lib from './r1-lib.mjs';
import * as headLib from './head-lib.mjs';

// [make, sizes, class]; class 'super' = base is super-linear there, 'linear' = base is linear,
// 'exp' = main (5203c4e3) is exponential, so only r1 (8a2f81f7) is timed as base.
const shapes = {
  "'#['": [(n) => '#['.repeat(n), [16000, 32000, 64000], 'super'],
  "'pub('": [(n) => 'pub('.repeat(n), [16000, 32000, 64000], 'super'],
  "'fn a<'": [(n) => 'fn a<'.repeat(n), [8000, 16000, 32000], 'super'],
  "'#pub(fn a<) fn b<'+'>'": [(n) => '#pub(fn a<) fn b<'.repeat(n) + '>', [4000, 8000, 16000], 'super'],
  "'#pub(fn a<) fn b<'": [(n) => '#pub(fn a<) fn b<'.repeat(n), [4000, 8000, 16000], 'super'],
  "'#[x]pub('": [(n) => '#[x]pub('.repeat(n), [8000, 16000, 32000], 'super'],
  "'#[x]pub('+')'": [(n) => '#[x]pub('.repeat(n) + ')', [8000, 16000, 32000], 'super'],
  "'#[x]fn a<'": [(n) => '#[x]fn a<'.repeat(n), [8000, 16000, 32000], 'super'],
  "'#[x]fn a<'+'>'": [(n) => '#[x]fn a<'.repeat(n) + '>', [8000, 16000, 32000], 'super'],
  "'#[x]#['": [(n) => '#[x]#['.repeat(n), [8000, 16000, 32000], 'super'],
  "'#['+']'": [(n) => '#['.repeat(n) + ']', [16000, 32000, 64000], 'super'],
  "'#[x]'+'z'": [(n) => '#[x]'.repeat(n) + 'z', [8000, 16000, 32000], 'super'],
  "' '+'y#'": [(n) => ' '.repeat(n) + 'y#', [16000, 32000, 64000], 'super'],
  "'#[test] fn a<'": [(n) => '#[test] fn a<'.repeat(n), [8000, 16000, 32000], 'super'],
  "'#[test] pub('": [(n) => '#[test] pub('.repeat(n), [8000, 16000, 32000], 'super'],
  "'pub(#'": [(n) => 'pub(#'.repeat(n), [16000, 32000, 64000], 'super'],
  "'fn a<#'": [(n) => 'fn a<#'.repeat(n), [8000, 16000, 32000], 'super'],
  "'#[fn a<'": [(n) => '#[fn a<'.repeat(n), [8000, 16000, 32000], 'super'],
  "'#[x] '+'y'": [(n) => '#[x] '.repeat(n) + 'y', [2000, 4000, 8000], 'exp'],
  "'fn '": [(n) => 'fn '.repeat(n) + 'y', [64000, 128000, 256000], 'linear'],
  "'#'+'fn '": [(n) => '#' + 'fn '.repeat(n), [64000, 128000, 256000], 'linear'],
  "'fn '+'#'": [(n) => 'fn '.repeat(n) + '#', [64000, 128000, 256000], 'linear'],
  "'#'+'a'": [(n) => '#' + 'a'.repeat(n), [128000, 256000, 512000], 'linear'],
  "'fn a() '+ws+'#[test] fn t() {}'": [(n) => 'fn a() '.repeat(n) + ' '.repeat(n) + '#[test] fn t() {}', [16000, 32000, 64000], 'linear'],
  "test fn": [(n) => '#[test] fn t() { assert!(true); }\n'.repeat(n), [2000, 4000, 8000], 'linear'],
  "test module": [(n) => '#[test]\n#[ignore]\npub async fn test_x() {\n  assert!(true);\n}\n\nfn helper(a: u32) -> u32 { a }\n'.repeat(n), [1000, 2000, 4000], 'linear'],
};

const only = process.argv[2];
const realRead = fs.readFileSync;
let source = '';
fs.readFileSync = (p, ...rest) => (p === '/fake/x.rs' ? source : realRead(p, ...rest));
function once(lib) {
  const t0 = process.hrtime.bigint();
  lib.scanRustFile('/fake', '/fake/x.rs');
  return Number(process.hrtime.bigint() - t0) / 1e6;
}
for (const [name, [make, sizes, kind]] of Object.entries(shapes)) {
  if (only && name !== only) continue;
  const libs = kind === 'exp' ? { r1: r1Lib, head: headLib } : { main: mainLib, r1: r1Lib, head: headLib };
  const runs = kind === 'linear' ? 9 : 3;
  for (const n of sizes) {
    source = make(n);
    const best = {};
    for (const lib of Object.values(libs)) once(lib);
    for (let r = 0; r < runs; r += 1) {
      const order = r % 2 === 0 ? Object.keys(libs) : Object.keys(libs).reverse();
      for (const k of order) best[k] = Math.min(best[k] ?? Infinity, once(libs[k]));
    }
    const base = Math.min(best.main ?? Infinity, best.r1);
    const cols = Object.entries(best).map(([k, v]) => `${k}=${v.toFixed(1)}`).join('\t');
    console.log(`${name}\t${kind}\tN=${n}\t${cols}\thead/base=${(best.head / base).toFixed(3)}`);
  }
}
```

Pass conditions (idle machine; each row prints `head/base`, base = faster of `main` and `r1`; the
`exp` row times `r1` only because `main` is exponential there):
1. every `super` and `exp` row: `head/base ≤ 0.10`, and head grows ≤ 2.75x per doubling;
2. round-2 shapes (`'#['`, `'pub('`, `'fn a<'`) and round-3 shapes (`'#pub(fn a<) fn b<'` with and
   without `'>'`) are among them; `'fn a<'` N=32000 head ≤ 100 ms (base ~1800 ms, R2 3072 ms);
3. every `linear` row: `head/base ≤ 1.10`. **Disclosed residual:** on these rows both versions do the
   same linear work plus the unchanged, dominant masking/body code; the matcher alone is 0.1-0.4 ms
   faster on attribute-free text and 0.3-1 ms *slower* per 8,000 dense `#[test]` functions (JS calls
   vs one native `exec`), i.e. <0.1% of those scans. Planning-time end-to-end ratios on these rows
   were 0.89-1.16, with the >1.00 values not reproducible run to run. The 10% band is the measurement
   resolution, not a performance budget; it is the one interpretation of bar B the coordinator must
   accept (see Plan Contract).

Planning-time results (min of 3 alternated runs for `super`/`exp`, 9 for `linear`; Node v22.23.2):

| shape | class | N | main ms | r1 ms | head ms | head/base |
|---|---|---|---|---|---|---|
| `'#['` | super | 16000 | 133.4 | 133.0 | 4.1 | 0.031 |
| `'#['` | super | 32000 | 527.5 | 525.8 | 8.7 | 0.016 |
| `'#['` | super | 64000 | 2064.7 | 2079.9 | 18.2 | 0.009 |
| `'pub('` | super | 16000 | 262.9 | 263.9 | 7.2 | 0.028 |
| `'pub('` | super | 32000 | 1045.4 | 1045.6 | 14.7 | 0.014 |
| `'pub('` | super | 64000 | 4147.1 | 4150.7 | 32.6 | 0.008 |
| `'fn a<'` | super | 8000 | 116.5 | 117.0 | 3.9 | 0.034 |
| `'fn a<'` | super | 16000 | 450.6 | 442.1 | 8.9 | 0.020 |
| `'fn a<'` | super | 32000 | 1797.6 | 1768.3 | 19.5 | 0.011 |
| `'#pub(fn a<) fn b<'+'>'` | super | 4000 | 388.1 | 379.2 | 9.7 | 0.025 |
| `'#pub(fn a<) fn b<'+'>'` | super | 8000 | 1549.2 | 1507.4 | 19.3 | 0.013 |
| `'#pub(fn a<) fn b<'+'>'` | super | 16000 | 6281.2 | 5999.9 | 41.9 | 0.007 |
| `'#pub(fn a<) fn b<'` | super | 4000 | 381.3 | 374.0 | 10.1 | 0.027 |
| `'#pub(fn a<) fn b<'` | super | 8000 | 1526.1 | 1491.0 | 19.5 | 0.013 |
| `'#pub(fn a<) fn b<'` | super | 16000 | 6073.0 | 5962.7 | 42.4 | 0.007 |
| `'#[x]pub('` | super | 8000 | 282.1 | 265.1 | 9.9 | 0.037 |
| `'#[x]pub('` | super | 16000 | 1117.1 | 1032.1 | 19.3 | 0.019 |
| `'#[x]pub('` | super | 32000 | 4350.5 | 4145.5 | 42.5 | 0.010 |
| `'#[x]pub('+')'` | super | 8000 | 276.1 | 265.2 | 10.3 | 0.039 |
| `'#[x]pub('+')'` | super | 16000 | 1102.7 | 1052.9 | 20.4 | 0.019 |
| `'#[x]pub('+')'` | super | 32000 | 4332.7 | 4122.1 | 41.5 | 0.010 |
| `'#[x]fn a<'` | super | 8000 | 420.9 | 400.3 | 11.1 | 0.028 |
| `'#[x]fn a<'` | super | 16000 | 1673.2 | 1570.2 | 23.3 | 0.015 |
| `'#[x]fn a<'` | super | 32000 | 6763.5 | 6285.8 | 44.3 | 0.007 |
| `'#[x]fn a<'+'>'` | super | 8000 | 424.5 | 397.6 | 11.3 | 0.028 |
| `'#[x]fn a<'+'>'` | super | 16000 | 1708.2 | 1572.7 | 22.4 | 0.014 |
| `'#[x]fn a<'+'>'` | super | 32000 | 6888.2 | 6259.3 | 46.3 | 0.007 |
| `'#[x]#['` | super | 8000 | 476.1 | 370.9 | 7.6 | 0.020 |
| `'#[x]#['` | super | 16000 | 1875.1 | 1458.1 | 15.5 | 0.011 |
| `'#[x]#['` | super | 32000 | 7432.8 | 5826.7 | 31.3 | 0.005 |
| `'#['+']'` | super | 16000 | 128.6 | 129.8 | 4.4 | 0.034 |
| `'#['+']'` | super | 32000 | 506.5 | 512.5 | 9.4 | 0.019 |
| `'#['+']'` | super | 64000 | 2030.8 | 2044.6 | 19.1 | 0.009 |
| `'#[x]'+'z'` | super | 8000 | 193.5 | 161.0 | 4.6 | 0.028 |
| `'#[x]'+'z'` | super | 16000 | 749.3 | 632.9 | 9.7 | 0.015 |
| `'#[x]'+'z'` | super | 32000 | 2992.3 | 2495.2 | 20.6 | 0.008 |
| `' '+'y#'` | super | 16000 | 131.7 | 131.0 | 1.3 | 0.010 |
| `' '+'y#'` | super | 32000 | 510.5 | 516.9 | 3.3 | 0.006 |
| `' '+'y#'` | super | 64000 | 2032.0 | 2111.6 | 6.5 | 0.003 |
| `'#[test] fn a<'` | super | 8000 | 1145.4 | 1136.3 | 14.8 | 0.013 |
| `'#[test] fn a<'` | super | 16000 | 4629.7 | 4488.7 | 31.8 | 0.007 |
| `'#[test] fn a<'` | super | 32000 | 18800.1 | 18022.5 | 62.8 | 0.003 |
| `'#[test] pub('` | super | 8000 | 776.4 | 774.3 | 13.2 | 0.017 |
| `'#[test] pub('` | super | 16000 | 3087.2 | 3064.8 | 27.0 | 0.009 |
| `'#[test] pub('` | super | 32000 | 12168.2 | 12221.5 | 66.0 | 0.005 |
| `'pub(#'` | super | 16000 | 326.4 | 325.2 | 11.0 | 0.034 |
| `'pub(#'` | super | 32000 | 1289.1 | 1293.1 | 22.5 | 0.017 |
| `'pub(#'` | super | 64000 | 5097.6 | 5112.8 | 48.3 | 0.009 |
| `'fn a<#'` | super | 8000 | 139.5 | 137.4 | 6.4 | 0.046 |
| `'fn a<#'` | super | 16000 | 537.6 | 530.9 | 13.6 | 0.026 |
| `'fn a<#'` | super | 32000 | 2151.6 | 2109.2 | 32.6 | 0.015 |
| `'#[fn a<'` | super | 8000 | 271.0 | 268.6 | 7.9 | 0.029 |
| `'#[fn a<'` | super | 16000 | 1070.8 | 1064.3 | 16.5 | 0.016 |
| `'#[fn a<'` | super | 32000 | 4281.4 | 4191.1 | 33.3 | 0.008 |
| `'#[x] '+'y'` | exp | 2000 | exp. | 61.9 | 1.2 | 0.019 |
| `'#[x] '+'y'` | exp | 4000 | exp. | 247.2 | 2.3 | 0.009 |
| `'#[x] '+'y'` | exp | 8000 | exp. | 993.9 | 5.3 | 0.005 |
| `'fn '` | linear | 64000 | 24.7 | 23.8 | 21.7 | 0.913 |
| `'fn '` | linear | 128000 | 51.8 | 50.9 | 45.6 | 0.896 |
| `'fn '` | linear | 256000 | 108.4 | 103.5 | 95.5 | 0.923 |
| `'#'+'fn '` | linear | 64000 | 24.5 | 24.0 | 21.4 | 0.891 |
| `'#'+'fn '` | linear | 128000 | 52.8 | 51.5 | 46.6 | 0.905 |
| `'#'+'fn '` | linear | 256000 | 107.7 | 104.5 | 95.4 | 0.913 |
| `'fn '+'#'` | linear | 64000 | 24.8 | 23.6 | 22.3 | 0.946 |
| `'fn '+'#'` | linear | 128000 | 52.0 | 50.3 | 47.6 | 0.946 |
| `'fn '+'#'` | linear | 256000 | 107.1 | 103.6 | 96.2 | 0.928 |
| `'#'+'a'` | linear | 128000 | 13.9 | 13.2 | 13.3 | 1.009 |
| `'#'+'a'` | linear | 256000 | 29.6 | 29.2 | 29.2 | 1.001 |
| `'#'+'a'` | linear | 512000 | 61.3 | 60.1 | 59.7 | 0.993 |
| `'fn a() '+ws+'#[test] fn t() {}'` | linear | 16000 | 16.1 | 16.0 | 15.7 | 0.983 |
| `'fn a() '+ws+'#[test] fn t() {}'` | linear | 32000 | 34.5 | 34.7 | 33.8 | 0.980 |
| `'fn a() '+ws+'#[test] fn t() {}'` | linear | 64000 | 71.5 | 71.2 | 69.9 | 0.983 |
| `test fn` | linear | 2000 | 97.4 | 92.6 | 89.1 | 0.962 |
| `test fn` | linear | 4000 | 346.0 | 356.0 | 350.5 | 1.013 |
| `test fn` | linear | 8000 | 1271.6 | 1309.2 | 1340.7 | 1.054 |
| `test module` | linear | 1000 | 82.1 | 69.7 | 80.8 | 1.160 |
| `test module` | linear | 2000 | 300.6 | 298.7 | 299.3 | 1.002 |
| `test module` | linear | 4000 | 1140.7 | 1136.2 | 1137.5 | 1.001 |

**AC7 — equivalence sweep.** `equiv.mjs` (cwd `$SCRATCH`, sharded; ~10 min on 12 cores):

```js
import fs from 'node:fs';
import * as oldLib from './main-lib.mjs';
import * as r1Lib from './r1-lib.mjs';
import * as newLib from './head-lib.mjs';

const realRead = fs.readFileSync;
let source = '';
fs.readFileSync = (p, ...rest) => (p === '/fake/x.rs' ? source : realRead(p, ...rest));

const SHARDS = Number(process.env.SHARDS ?? 1);
const SHARD = Number(process.env.SHARD ?? 0);
let tested = 0;
let generated = 0;
function check(text, oracle = oldLib) {
  generated += 1;
  if (generated % SHARDS !== SHARD) return;
  source = text;
  tested += 1;
  const a = JSON.stringify(oracle.scanRustFile('/fake', '/fake/x.rs'));
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
    (k) => '#pub(fn a<) fn b<'.repeat(k) + '>',
    (k) => '#pub(fn a<) fn b<'.repeat(k),
    (k) => '#[x]pub(fn a<) fn b<'.repeat(k) + '>',
    (k) => '#[test] pub(crate) async fn a<T>() {}\n'.repeat(k),
  ]) check(make(n), make(3) === '#[x] #[x] #[x] ' && n >= 50 ? r1Lib : oldLib); // main is exponential there (S5852)
}
for (const t of ['#[a fn b<] x #[test] fn t(>( {}', '#[a pub(] x #[test] fn t() {} ) fn u() {}', '#[x fn a(] #[test] fn b() {}', 'pub(#[test] fn a() {}) fn c(', 'fn a<#[test] fn b() {}>(', 'x#[test]#[y fn z(] fn t() {}']) check(t);
fuzz(['#[', '#[test]', ']', ' ', 'fn a', 'fn b<', '>', '(', ')', 'pub(', 'pub ', 'async ', '{', '}', 'x', '#'], 300000, 16);
console.log(`shard=${SHARD}/${SHARDS} generated=${generated} tested=${tested} failures=0`);
```

```bash
cd "$SCRATCH" && for i in $(seq 0 11); do SHARDS=12 SHARD=$i node equiv.mjs > "eq-$i.out" 2>&1 & done; wait
cat eq-*.out; grep -h tested= eq-*.out | sed 's/.*tested=\([0-9]*\).*/\1/' | awk "{ s += \$1 } END { print s }"
```

Pass: 12 lines `... failures=0`, sum `23242436`, no `MISMATCH`. Corpus (`corpus.mjs`, cwd `$SCRATCH`):

```js
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
const repo = process.argv[2];
const oldLib = await import('./main-lib.mjs');
const newLib = await import('./head-lib.mjs');
const code = fs.readFileSync(new URL('./main.mjs', import.meta.url), 'utf8').replace(/^try \{$[\s\S]*/m, '') + '\nexport { maskCommentsAndStrings };\n';
fs.writeFileSync(new URL('./mask-lib.mjs', import.meta.url), code);
const { maskCommentsAndStrings } = await import('./mask-lib.mjs');
const files = execFileSync('git', ['-C', repo, 'ls-files'], { encoding: 'utf8' }).split('\n').filter((f) => /\.(rs|ts|tsx|md|json|ya?ml|html|css|mjs|js)$/.test(f));
const realRead = fs.readFileSync;
let source = '';
fs.readFileSync = (p, ...r) => (p === '/fake/x.rs' ? source : realRead(p, ...r));
let failures = 0; let n = 0;
for (const f of files) {
  const text = realRead(path.join(repo, f), 'utf8');
  for (const s of [text, maskCommentsAndStrings(text, { singleQuote: false })]) {
    source = s; n += 1;
    if (JSON.stringify(oldLib.scanRustFile('/fake', '/fake/x.rs')) !== JSON.stringify(newLib.scanRustFile('/fake', '/fake/x.rs'))) { failures += 1; console.log('MISMATCH', f); }
  }
}
console.log(`files=${files.length} inputs=${n} failures=${failures}`);
```

`node corpus.mjs "$REPO"` → `files=914 inputs=1828 failures=0` at the pinned base (file count follows
the tracked tree; `failures=0` is the pass condition).

**AC8 — footprint.** `git diff --stat` → `1 file changed, 125 insertions(+), 8 deletions(-)`;
`git diff --name-only` → `scripts/check-test-debt.mjs`; `git diff --check` → no output; no untracked
files in the repo.

**AC9 — SonarCloud (the judge).** After the implementation commit is pushed and analysed:

```bash
curl -s "https://sonarcloud.io/api/qualitygates/project_status?projectKey=mblua_AgentsCommander&pullRequest=2099"
curl -s "https://sonarcloud.io/api/issues/search?componentKeys=mblua_AgentsCommander&pullRequest=2099&resolved=false"
```

Pass: gate `"status":"OK"`; issues search `total: 0` (neither `AaCqh-uoqWlJPkFQ9mDs` nor
`AaCqh-uoqWlJPkFQ9mDt`, nothing new). After merge, main key `AaBBZr7CbbRnCQnTRHkL` closes; that is
observed post-merge by the coordinator, not a PR gate. Every triggered and required GitHub check on the
exact pushed head SHA must be green (`gh pr checks 2099`).

## 7. Delivery gates (delivery-nonfunctional-invariants)

| gate | evidence / owner / failure behavior |
|---|---|
| 1 CI parity | AC1/AC2 locally (the scripts CI invokes); AC9 + `gh pr checks 2099` on the exact head SHA; owner implementer then coordinator. Any red or missing check blocks. |
| 2 toolchain | Node v22 as in the repo; judge pinned to eslint 9.39.1, sonarjs 4.2.1, scslre 0.3.0, regexpp 4.12.2 in scratch (never in `package.json`). |
| 3 Git | existing issue branch, PR #2099, no push to `main`, no force-push; one commit on top of the pinned plan commit. |
| 4 cwd/state | commands from repo root; all scratch under `$AGENTSCOMMANDER_ROOT/scratch/2088-proof`. |
| 5 scope | AC0 hash before, AC8 after; only `scripts/check-test-debt.mjs` changes. |
| 6 recovery | if any AC fails before commit: `git checkout -- scripts/check-test-debt.mjs` (only that path, only this run's edit) and report; after push: revert commit. |
| 7 bounded runs | bench/equiv are finite; keep `*.out` files until the reply is sent. |
| 8 evidence | reply carries AC outputs, commit SHA, Sonar JSON. |

Enhanced controls: not applicable (no release, signing, untrusted host, or security boundary).

## 8. Risks and rollback

- Wrong matcher would change the debt report: AC2 pins it byte-for-byte, AC7 sweeps 23.2M inputs plus
  the corpus, §5 gives the argument.
- Timing noise near 1.00 on linear rows: covered by the AC6 band and its disclosed residual.
- Module-level `g`/`y` regex constants carry `lastIndex`; every use sets `lastIndex` right before
  `exec`/`test` and nothing is re-entrant.
- Rollback: revert the one commit.

## 9. Implementation order

1. AC0; capture AC2 "before"; build the scratch judge and libraries.
2. Apply §4 exactly; check the §4 sha256.
3. Run AC1, AC2, AC3-AC5, AC7, AC6, AC8; keep outputs.
4. Commit `fix(2088): replace backtracking fn regex with exact memoized matcher`; push the branch.
5. Poll AC9; reply to `ac-tech-lead-v4` with commit SHA, AC outputs, Sonar JSON.

## Plan Contract

One decided change (§4), one file, no open decision. Equivalence is argued (§5) and swept (AC7);
bar B is measured against both bases on the round-2 and round-3 shapes (AC6); SonarCloud is the final
judge (AC9). Worst case O(n^2) is stated in the code comment and here. Coordinator decision needed
before implementation: accept AC6 condition 3 (linear rows within 10% measurement band, matcher
residual ≤ ~1 ms per 8,000 dense tests) as satisfying "no input shape slower than base"; if not
accepted, this plan is not ready and bar A (table-driven matcher) is the fallback.
