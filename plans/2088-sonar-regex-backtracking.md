# Plan #2088 (round 2): replace the super-linear `fnRe` with a linear scanner in `scripts/check-test-debt.mjs`

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2088. PR: https://github.com/mblua/AgentsCommander/pull/2099
  (open; do not merge). SonarCloud on the PR, line 307:
  - `javascript:S8786` key `AaCqh-uoqWlJPkFQ9mDs` — non-exponential (super-linear) backtracking;
  - `javascript:S5843` key `AaCqh-uoqWlJPkFQ9mDt` — regex complexity 49 > 20;
  - `javascript:S5852` is already clear from round 1. Gate: `new_maintainability_rating` 5 vs threshold 1.
- Repo `repo-AgentsCommander`; branch `fix/2088-sonar-regex-backtracking`; base (frozen at authoring,
  2026-09-16 UTC): `8a2f81f7f726d4ec282cd46bde149c06ba9e3048` = local HEAD = remote branch head.
  Tracked tree clean. Every line number below refers to that SHA; if a quoted line no longer matches,
  re-anchor on the quoted text, never on the number.
- Class: **Lite** (round 1 was Express). One source file and one function's matcher, but the change is
  structural: the whole-match regex is replaced by a linear scanner plus five helpers (+87/-8 lines in
  one file), and `scanRustFile`'s observable behavior is proven by a differential harness. No test
  file, no dependency, no IPC, no product code. Band 1-25 unchanged; owner `ac-dev-rust-v4`,
  coordinator `ac-tech-lead-v4`; grinch reviews the proofs below.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so commit with
  `git add -f plans/2088-sonar-regex-backtracking.md`.

## 1. Objective

Delete the single (round-1) regex `fnRe` at `scripts/check-test-debt.mjs:307` and scan the same
language with linear JavaScript, so `javascript:S8786` and `javascript:S5843` clear on PR #2099
(gate green), `javascript:S5852` stays clear, runtime is linear on adversarial inputs, and
`npm run test:debt` output stays byte-identical.

## 2. Round-2 verified cause

`fnRe` (exact literal, base line 307):

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

## 3. In scope / out of scope

In scope: exactly `scripts/check-test-debt.mjs` — the `scanRustFile` matcher and five new helper
functions (section 4). The deleted literal is the only removed line group.

Out of scope (binding): every other regex and function in the file (including `S5843` on line 287,
which is pre-existing, untouched, and not on the PR); `test-debt.allowlist.json`, `package.json`,
`plans/` (except this file), any test file, any workflow. The report format, categories, ids,
allowlist semantics and `--self-test` fixtures are preserved by construction (section 5) and proven
by AC1/AC2/AC7.

## 4. Decided solution (exact change; nothing is left to the implementer)

Insert the five helpers after `hasExecutableRustBody` (after base line 297) and before
`function scanRustFile` (base line 299):

```js
function rustAttributeEnd(source, index) {
  if (source[index] !== '#') return -1;
  const open = skipWhitespace(source, index + 1);
  if (source[open] !== '[') return -1;
  const close = source.indexOf(']', open + 1);
  return close === -1 ? -1 : close + 1;
}

function rustPubBodyEnd(source, index) {
  if (!source.startsWith('pub', index)) return -1;
  let after = index + 3;
  const open = skipWhitespace(source, after);
  if (source[open] === '(') {
    const close = source.indexOf(')', open + 1);
    if (close !== -1) after = close + 1;
  }
  const body = skipWhitespace(source, after);
  return body > after ? body : -1;
}

function rustFnHeaderAt(source, index) {
  const pubBody = rustPubBodyEnd(source, index);
  if (pubBody !== -1) {
    const header = rustFnTailAt(source, pubBody);
    if (header !== null) return header;
  }
  return rustFnTailAt(source, index);
}

function rustFnTailAt(source, start) {
  let cursor = start;
  if (source.startsWith('async', cursor)) {
    const afterAsync = skipWhitespace(source, cursor + 5);
    if (afterAsync > cursor + 5) cursor = afterAsync;
  }
  if (!source.startsWith('fn', cursor)) return null;
  const nameStart = skipWhitespace(source, cursor + 2);
  if (nameStart === cursor + 2) return null;
  if (!/[A-Za-z_]/.test(source[nameStart] ?? '')) return null;
  let nameEnd = nameStart + 1;
  while (isIdentifierChar(source[nameEnd])) nameEnd += 1;
  let open = skipWhitespace(source, nameEnd);
  if (source[open] === '<') {
    let close = open + 1;
    while (close < source.length && source[close] !== '>' && source[close] !== '{' && source[close] !== '}') close += 1;
    if (source[close] !== '>') return null;
    open = skipWhitespace(source, close + 1);
  }
  if (source[open] !== '(') return null;
  return { name: source.slice(nameStart, nameEnd), end: open + 1 };
}

function rustStepAt(source, cursor, runStart, runHasAttr) {
  const ch = source[cursor];
  if (/\s/.test(ch)) {
    return { next: cursor + 1, runStart: runStart === -1 ? cursor : runStart, runHasAttr };
  }
  if (ch === '#') {
    const attrEnd = rustAttributeEnd(source, cursor);
    if (attrEnd === -1) return { next: cursor + 1, runStart: -1, runHasAttr: false };
    return { next: attrEnd, runStart: runStart === -1 ? cursor : runStart, runHasAttr: true };
  }
  const header = rustFnHeaderAt(source, cursor);
  if (header === null) return { next: cursor + 1, runStart: -1, runHasAttr: false };
  const index = runStart === -1 ? cursor : runStart;
  const attrs = runHasAttr ? source.slice(index, cursor) : '';
  return { match: { index, attrs, end: header.end, name: header.name }, next: header.end, runStart: -1, runHasAttr: false };
}

function* rustFnMatches(source) {
  let cursor = 0;
  let runStart = -1;
  let runHasAttr = false;
  while (cursor < source.length) {
    const step = rustStepAt(source, cursor, runStart, runHasAttr);
    if (step.match) yield step.match;
    cursor = step.next;
    runStart = step.runStart;
    runHasAttr = step.runHasAttr;
  }
}
```

Then replace exactly these lines of `scanRustFile` (base lines 307-315):

```js
  const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(/g;
  let match;

  while ((match = fnRe.exec(masked)) !== null) {
    const attrs = match[1] || '';```

with:

```js
  for (const match of rustFnMatches(masked)) {
    const attrs = match.attrs;```

No other edit. Decision notes:

- D1 — attributes are consumed whole by `rustAttributeEnd` (`#\s*\[[^\]]*\]`), so `fn` text inside an
  attribute's `[^\]]*` content is skipped instead of matched; the forward run state
  (`runStart`/`runHasAttr`) replaces the greedy prefix capture.
- D2 — `rustStepAt` is a separate function only to keep every new function's cognitive complexity
  ≤ 15 (the SonarJS judge rates the inline loop 28; with the split, all five helpers are clean).
- D3 — `rustPubBodyEnd`/`rustFnTailAt` mirror the old modifier grammar exactly, including the
  `pub (crate)` whitespace, the `pub` fall-through to `async`/`fn`, and the `<[^>{}]*>` generics that
  stop at `>`/`{`/`}`.
- D4 — the loop yields the same four values the body uses: `index`, `end`, `attrs`, `name`; the body
  keeps its formulas (`fnStart = index + slice(index, end).lastIndexOf('fn ')`, body search from
  `end`, `lineOf(source, index)`).

Rejected alternatives (no open decision): a lazy/possessive regex or atomic-group emulation keeps
`Move`/complexity and changes captures; matching `fn` first and reconstructing the prefix backwards
reorders matches for `fn` text inside attribute content (verified counterexample:
`#[x(fn b()] fn a(`).

## 5. Why this is equivalent (argument + evidence)

Old match records are `(index, end, attrs, name)`: the greedy regex starts at the beginning of the
absorbable run (whitespace and complete attributes) and ends after `(`; downstream code uses only
`attrs` (filter `/#\s*\[\s*test\b/`), `name`, `index`, `end` (via `fnStart`, `lineOf`, body search).
The new scanner reproduces every old record whose `attrs` passes that filter, exactly; the only
records it drops are old intermediate matches of `fn` text inside a complete `#[...]` attribute
(e.g. `#[x(fn b()] ...`), whose capture is whitespace-only and therefore can never pass the filter;
the new scanner never adds a record. So findings, warnings, ids, lines and allowlist comparison are
identical.

Planning-time evidence (all scratch only, never committed):

| Harness | Volume | Result |
|---|---|---|
| Full `scanRustFile` output, old vs new | exhaustive A8 = `' # [ ] f n ( )'` len ≤ 8: 19,173,961 strings | 0 failures |
| Full `scanRustFile` output, old vs new | exhaustive A20 len ≤ 5: 3,368,421 strings | 0 failures |
| Full `scanRustFile` output, old vs new | token fuzz 200,000 + structured fuzz 200,000 | 0 failures |
| Full `scanRustFile` output, old vs new | repo corpus 922 files raw + 922 masked (`.rs .ts .tsx .md .json .yml .yaml .html .css .mjs .js`) | 0 failures |
| Record-level (filtered) | 6,065,166 strings | filtered mismatches 0; new-only 0; old-only 224, all `attrs` never matching the filter |
| CLI `npm run test:debt` stdout/stderr/exit | repo | byte-identical; `check-test-debt self-test passed` |

Edge cases proven equal in the same sweep: `pub (crate)   async  fn f<T>(`, `#[a]#[b]fn f(`,
`# [test]`, `#[x(fn b()] fn a(`, `#[cfg(#[test fn b()] fn c(`, `fn foo<fn bar>(`,
`pub(crate)async fn`, `fn` inside comments (masked), unterminated attributes, and every record whose
`attrs` matches `/#\s*\[\s*test\b/`.

## 6. Verification (objective acceptance criteria)

Run every command from the repo root (`REPO="$(pwd)"`). Scratch artifacts live in the replica-local
scratch dir (allowed zone, never committed):

```bash
SCRATCH="$AGENTSCOMMANDER_ROOT/scratch/2088-proof"
mkdir -p "$SCRATCH/sonar" "$SCRATCH/perf/src-tauri/src"
cp scripts/check-test-debt.mjs "$SCRATCH/base.mjs"
printf '{ "version": 1, "entries": [] }\n' > "$SCRATCH/perf/test-debt.allowlist.json"
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
code (verified above: `/\s/`, `/[A-Za-z_]/`, and the untouched `/#\s*\[\s*test\b/` all `[]`).
AC4 (S8786): base has `super-linear-regex` at 307; changed file has none. AC5 (S5843): changed file
has exactly one report — `regex-complexity 287 (23)`, pre-existing and not in the PR; no changed line
has a regex literal above 20. Direct `scslre` cross-check (write as `$SCRATCH/sonar/scslre-literals.mjs`):

```js
import { analyse } from 'scslre';
for (const source of ['\\s', '[A-Za-z_]']) console.log(source, analyse({ source, flags: '' }).reports);
```

**AC6 — linear runtime (the concern behind S8786).** `$SCRATCH/bench.mjs` (scratch only; run with
cwd `$SCRATCH`):

```js
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';

const [, , script, kind, ...sizes] = process.argv;
const file = './perf/src-tauri/src/pathological.rs';
for (const n of sizes.map(Number)) {
  fs.writeFileSync(file, (kind === 'attr' ? '#[x] ' : ' ').repeat(n) + 'y');
  const start = process.hrtime.bigint();
  execFileSync('node', [script, '--root', './perf'], { stdio: 'ignore' });
  const ms = Number(process.hrtime.bigint() - start) / 1e6;
  console.log(`${kind} N=${n} ${ms.toFixed(1)}ms`);
}
```

```bash
cd "$SCRATCH"
node bench.mjs base.mjs attr 2000 4000 8000 16000 32000
node bench.mjs "$REPO/scripts/check-test-debt.mjs" attr 2000 4000 8000 16000 32000 64000 128000 256000
node bench.mjs base.mjs ws 2000 4000 8000 16000 32000
node bench.mjs "$REPO/scripts/check-test-debt.mjs" ws 2000 4000 8000 16000 32000 64000 128000 256000
```

Planning-time (Node v22.23.2) `attr` input: base 89.6 / 275.7 / 1044.4 / 4141.0 / 16318.7 ms at
2k/4k/8k/16k/32k (≈4x per doubling, quadratic); new 27.7 / 30.0 / 36.3 / 75.5 / 94.2 / 107.1 /
165.7 / 284.3 ms at 2k..256k (linear). `ws` input: base 21.7 / 32.8 / 57.6 / 159.5 / 577.6 ms; new
21.4 / 22.3 / 24.6 / 35.6 / 31.7 / 44.6 / 61.9 / 95.0 ms to 256k. `'fn ' * 100000 + 'y'` (300 KB):
base 105.4 ms, new 115.6 ms. Pass condition: new grows linearly with N (no ~4x per doubling) and
stays under 400 ms at 256k.

**AC7 — equivalence harness (reproduces section 5).** Build the two libs from the base snapshot and
the changed file:

```bash
sed '/^try {$/,$d' "$SCRATCH/base.mjs" > "$SCRATCH/old-lib.mjs"
printf 'export { scanRustFile, scanFrontendFile, scan, maskComments, maskCommentsAndStrings, lineOf, skipWhitespace };\n' >> "$SCRATCH/old-lib.mjs"
sed '/^try {$/,$d' "$REPO/scripts/check-test-debt.mjs" > "$SCRATCH/new-lib.mjs"
printf 'export { scanRustFile, scanFrontendFile, scan, maskComments, maskCommentsAndStrings, lineOf, skipWhitespace, rustFnHeaderAt, rustAttributeEnd, rustStepAt, rustFnMatches };\n' >> "$SCRATCH/new-lib.mjs"
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
console.log(`tested=${tested} failures=0`);
```

Run `cd "$SCRATCH" && node equiv.mjs`. Pass condition: `tested=22942382 failures=0`
(19,173,961 + 3,368,421 + 200,000 + 200,000). Also run the repo corpus comparison (922 files raw +
922 masked, `JSON.stringify` per file) and `node scripts/check-test-debt.mjs` vs
`node "$SCRATCH/base.mjs"` output (`cmp`).

**AC8 — footprint.**

```bash
git diff --stat        # 1 file changed, 87 insertions(+), 8 deletions(-)
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
  AC2 pins the report byte-for-byte and AC7 sweeps ~23M inputs plus the repo corpus.
- New-code smells: the five helpers are each below the SonarJS thresholds (cognitive ≤ 15; no regex
  literal above complexity 20; no `slow-regex`/`super-linear-regex` report). The only remaining file
  issues are pre-existing and untouched (line 287 `S5843`, line 299 `S3776` on the base).
- No product, IPC, persistence, release or CI-contract change; the script has no dependencies.
- Revert is one commit; no migration, no rollout.

## 8. Implementation order

1. Snapshot the base script and capture the BEFORE report (AC2); build the scratch judge (AC3-AC5).
2. Apply exactly section 4.
3. Run AC1, AC2, AC3-AC5, AC6, AC7, AC8; keep raw outputs.
4. Commit the source as `fix(2088): replace the super-linear fnRe with a linear test-debt scanner`
   plus the plan (`git add -f plans/2088-sonar-regex-backtracking.md`); push the branch.
5. Poll AC9 until SonarCloud re-analyses the PR; keep the API JSON as evidence.
6. Reply to `ac-tech-lead-v4` with plan path, commit SHA, Express/Lite, AC1-AC9 evidence and the
   SonarCloud gate JSON.

## Plan Contract

No TBD, no open decision, no competing alternative: section 4 is the sole change. The single changed
file is `scripts/check-test-debt.mjs`; the matcher moves from regex to code, with the same findings
and warnings. Every acceptance criterion is a command with an objective pass condition; AC9 is the
external judge. The only new file is this plan. Round-1's V8 timing/backtrack probe is obsolete and is
replaced by AC6 (linear scaling) and AC3-AC5 (the pinned SonarJS/scslre judge).
