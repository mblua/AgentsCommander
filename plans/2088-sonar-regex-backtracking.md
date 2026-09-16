# Plan #2088: remove the exponential-backtracking regex in `scripts/check-test-debt.mjs`

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2088 — Sonar key `AaBBZr7CbbRnCQnTRHkL`,
  rule `javascript:S5852`, VULNERABILITY / CRITICAL / `SECURITY:HIGH`.
- Repo `repo-AgentsCommander`; branch `fix/2088-sonar-regex-backtracking`; base (frozen at authoring,
  2026-09-16 UTC): `5203c4e3b0b5909fdc12337194c886b1514966c3` = local HEAD = remote branch head
  (`git ls-remote origin refs/heads/fix/2088-sonar-regex-backtracking`), tracked tree clean. Every
  line number below refers to that SHA; if a quoted line no longer matches, re-anchor on the quoted
  text, never on the number.
- Class: Lite (band 1-25), Express: one source file, one line, mechanical regex replacement, no
  test-file change, no dependency, no IPC, no product code. Owner `ac-dev-rust-v4`; coordinator
  `ac-tech-lead-v4`.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so commit with
  `git add -f plans/2088-sonar-regex-backtracking.md`.

## 1. Objective

Replace the single regular expression `fnRe` at `scripts/check-test-debt.mjs:307` with a
behaviorally identical pattern that no longer backtracks exponentially, so Sonar S5852
`AaBBZr7CbbRnCQnTRHkL` clears while the test-debt report stays byte-identical.

## 2. Verified cause

Symbol: `fnRe` inside `scanRustFile` (`scripts/check-test-debt.mjs:307`), matched against the
masked source (`masked = maskCommentsAndStrings(source, { singleQuote: false })`, `:301`).

Before (exact, line 307):

```js
  const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*)*)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>)?\s*\(/g;
```

Cause: in the repeated group `(?:\s*#\s*\[[^\]]*\]\s*)*` every iteration both starts and ends with
`\s*`, and the group is followed by another `\s*`. For each whitespace gap between two attributes
(`]`…`#`) the trailing `\s*` of the previous iteration and the leading `\s*` of the next can each
take part of the same gap, so a run of g attributes has ~2^g parses. On input with many attributes
and no `fn` — the scanner is fed arbitrary file bytes — the failing match explores all of them.

Measured at planning time against the base file, with the candidate literal substituted in a
scratch copy (Node v22.23.2, Linux x86-64, min of 3 runs, input `("#[x] " x N) + "y"`):

| N | old `fnRe` | new `fnRe` | ratio |
|---|---|---|---|
| 22 | 143.8 ms | 0.012 ms | 11,655x |
| 24 | 579.2 ms | 0.014 ms | 42,755x |
| 26 | 2312.1 ms | 0.014 ms | 159,632x |

Old grows ~4x per +2 attributes (exponential); new is flat.

## 3. In scope / out of scope

In scope: exactly one line, `scripts/check-test-debt.mjs:307`.

Out of scope (binding):

- Every other regex and function in the script: `maskSource`/`maskCommentsAndStrings`,
  `findMatchingBrace`, `moduleRanges`, the frontend scanners, the allowlist loader, `printReport`.
- `test-debt.allowlist.json` (must not change), `package.json`, any test file, any workflow.
- The engine's unanchored scan retries: the fix removes the exponential per attempt; V8 still
  retries each start offset, so a hypothetical file made only of `#[x] ` repeats and no `fn`
  remains polynomial in file size (the V8 instrument of AC3b already trips at N≥500 for the
  fixed pattern). That is pre-existing, is not the S5852 exponential finding, and changing it would
  alter match boundaries and captures. Accepted boundary; probe sizes are chosen below it.
- The report format, categories, ids and allowlist semantics: preserved by construction (section 4)
  and proven by AC2.

## 4. Decided solution (exact change; nothing is left to the implementer)

After (exact, replaces line 307; no other edit):

```js
  const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(/g;
```

Diff:

```diff
-  const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*)*)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>)?\s*\(/g;
+  const fnRe = /((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(/g;
```

Both edits are required in the same one-line replacement:

- **D1 — attribute run unrolled.** `(?:\s*#\s*\[[^\]]*\]\s*)*` becomes
  `(?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?`. The first attribute keeps its leading `\s*`;
  every later attribute starts directly at `#`, so the whitespace before it can only be consumed by
  the previous attribute's trailing `\s*`. The outer `(?:...)?` keeps the group participating when
  there is no attribute, so `match[1]` stays `''` exactly as today. No iteration of a repeat
  contains a quantifier that can also match the next iteration's first token.
- **D2 — generic-list whitespace merged.** `\s*(?:<[^>{}]*>)?\s*\(` becomes
  `\s*(?:<[^>{}]*>\s*)?\(`, removing the remaining pair of adjacent whitespace quantifiers. Same
  language, same greedy end: with generics, the optional branch owns the whitespace after `>`;
  without generics the first `\s*` already consumed all whitespace before `(`.

Capture groups stay 1 and 2 with the same text; no group is added, removed or renumbered.
`match.index`, `match[0]`, `fnRe.lastIndex`, `match[0].lastIndexOf('fn ')` and the downstream
`attrs` test (`:310`, `:324`), `fnStart` (`:311`), `lineOf(source, match.index)` (`:314`) are
unchanged.

No alternative remains open: this exact literal is the change. In particular, a lazy variant,
`possessive`/atomic-group emulation, or dropping the leading whitespace from group 1 would either
keep the ambiguity, change `match[1]`, or shift `match.index` and therefore the reported line
numbers (the whitespace run can start on the line *before* the attribute, which is existing
behavior the report depends on).

## 5. Equivalence evidence

Structural argument:

- Both patterns accept the same prefix language `{ W0 A1 W1 ... Ak Wk }`, where `Ai` is
  `#\s*\[[^\]]*\]` and `Wi` is whitespace, followed by the same keyword/generic/`(` tail.
- Both loops are greedy and `fn` cannot start with `#` or whitespace, so the first successful parse
  uses the maximal attribute count and puts all trailing whitespace inside group 1; the internal
  split of each gap does not change the captured text.
- `\s*\s*` was never able to change the matched text, only the number of paths tried; D2 removes the
  paths.

Planning-time differential runs (old literal vs new literal, scratch only, no repo file):

| Corpus | Volume | Differing match lists |
|---|---|---|
| Masked sources exactly as `scanRustFile` feeds them (all `.rs` + `.test.ts`/`.test.tsx` under `src-tauri/src`, `src-tauri/tests`, `src`) | 431 files, 12,375 `fn` matches | 0 |
| Raw repo files (`.rs`, `.ts`, `.tsx`, `.md`) | 825 files | 0 |
| Exhaustive all strings, length ≤ 5, over `[space \n \t # [ ] a f n ( ) < > p u b s y c]` | 2,613,660 strings | 0 |
| Randomized token fuzz (`#[test]`, `pub`, `async`, `<T>`, whitespace, comments, quotes, braces, …) | 200,000 strings | 0 |

Every compared match included `index`, `end`, `match[0]`, `match[1]`, `match[2]`.

Edge cases, all identical in both literals (raw input; the scanner masks comments/strings first):

| Case | Result (both) |
|---|---|
| `fn foo(`, `\n\n    fn foo(`, `pub fn foo(`, `pub(crate) async fn foo<T>(`, `fn foo<T>(` | match, `match[1] = ''`, name `foo` |
| `#[test]\nfn foo(`, `#[a] #[b]    \n   fn foo(`, `#[a]#[b]\nfn foo(`, `#[a]#[b]fn foo(`, `#[test]fn foo(`, `#[]\nfn foo(`, `\r\n    #[test]\r\nfn foo(` | match; `match[1]` is the whole leading-whitespace + attribute run incl. gaps |
| `\n    #[test]\nfn foo(` | `match.index = 0` (start of the whitespace run, often the previous line) — unchanged |
| `#[a] # not-attr\nfn foo(` | no attribute captured; match starts at the whitespace before `fn` (`match[1] = ''`) |
| `#[test]` alone, `fnfoo(`, `fn foo /*c*/ (` | no match |

## 6. Verification (objective acceptance criteria)

All commands from the repo root, on branch `fix/2088-sonar-regex-backtracking` at base `5203c4e3`
plus the one-line change.

**AC1 — self-test.** `npm run test:debt:self` prints `check-test-debt self-test passed` and exits 0.

**AC2 — report identity.** Capture BEFORE on the untouched base, apply the change, capture AFTER:

```bash
npm run --silent test:debt > /tmp/2088-debt-before.out 2> /tmp/2088-debt-before.err; echo $? > /tmp/2088-debt-before.code
# apply the one-line replacement (section 4)
npm run --silent test:debt > /tmp/2088-debt-after.out  2> /tmp/2088-debt-after.err;  echo $? > /tmp/2088-debt-after.code
cmp /tmp/2088-debt-before.out /tmp/2088-debt-after.out && \
cmp /tmp/2088-debt-before.err /tmp/2088-debt-after.err && \
cmp /tmp/2088-debt-before.code /tmp/2088-debt-after.code && echo IDENTICAL
npm run test:debt; echo "npm exit=$?"
```

Expected: `IDENTICAL`; `npm exit=0`; 34 stdout lines with
`Ignored Rust tests: 24 discovered, 24 allowlisted, 0 unallowlisted`,
`Placeholder tests: 7 discovered, 7 allowlisted, 0 unallowlisted`,
`Skipped frontend tests: 0 discovered, 0 allowlisted, 0 unallowlisted`.
If BEFORE was not captured in time, reconstruct it without touching the worktree (verified at
planning time to equal `npm run --silent test:debt` on the base file):

```bash
git show 5203c4e3:scripts/check-test-debt.mjs > /tmp/2088-check-test-debt-before.mjs
node /tmp/2088-check-test-debt-before.mjs --root "$PWD" > /tmp/2088-debt-before.out 2> /tmp/2088-debt-before.err
```

**AC3 — no exponential backtracking.**

AC3a, timing probe. Write the two literals below to `/tmp/2088-redos-probe.mjs` (outside the repo;
do not commit it) and run `node /tmp/2088-redos-probe.mjs`:

```js
const OLD = /((?:\s*#\s*\[[^\]]*\]\s*)*)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>)?\s*\(/g;
const NEW = /((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(/g;
function timeOne(re, input) {
  re.lastIndex = 0;
  const t0 = process.hrtime.bigint();
  const m = re.exec(input);
  return { ms: Number(process.hrtime.bigint() - t0) / 1e6, matched: m !== null };
}
for (const n of [22, 24, 26]) {
  const input = '#[x] '.repeat(n) + 'y';
  for (const re of [OLD, NEW]) timeOne(re, input); // warm up
  let oldMin = Infinity, newMin = Infinity;
  for (let i = 0; i < 3; i += 1) {
    oldMin = Math.min(oldMin, timeOne(OLD, input).ms);
    newMin = Math.min(newMin, timeOne(NEW, input).ms);
  }
  console.log(`${n}\told=${oldMin.toFixed(1)}ms\tnew=${newMin.toFixed(3)}ms\tratio=${Math.round(oldMin / newMin)}x`);
}
```

Pass condition: new < 50 ms at every N and old > 100 ms at N=26, ratio > 1000x at N=26, and old
grows ~4x per +2 while new stays flat. Planning-time result:
`22 old=143.8 new=0.012 (11,655x)`, `24 old=579.2 new=0.014 (42,755x)`,
`26 old=2312.1 new=0.014 (159,632x)`.

AC3b, threshold instrument (timing-independent pass/fail). V8 counts backtracks and abandons the
backtracking engine above a threshold; the trace line goes to stderr. Old must fall back, new must
not:

```bash
node --enable-experimental-regexp-engine-on-excessive-backtracks \
     --regexp-backtracks-before-fallback=100000 --trace-experimental-regexp-engine -e '
const re = /((?:\s*#\s*\[[^\]]*\]\s*)*)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>)?\s*\(/g;
console.log("result:", re.exec("#[x] ".repeat(26) + "y") === null ? "null" : "match");
' 2>&1

node --enable-experimental-regexp-engine-on-excessive-backtracks \
     --regexp-backtracks-before-fallback=100000 --trace-experimental-regexp-engine -e '
const re = /((?:\s*#\s*\[[^\]]*\]\s*(?:#\s*\[[^\]]*\]\s*)*)?)\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{}]*>\s*)?\(/g;
console.log("result:", re.exec("#[x] ".repeat(26) + "y") === null ? "null" : "match");
' 2>&1
```

Expected: first command prints `Experimental execution (oneshot) of regexp …` plus `result: null`
(>100,000 backtracks); second prints only `result: null` (no fallback). Verified at planning time.

**AC4 — footprint.** After the change:

```bash
git diff --stat        # 1 file changed, 1 insertion(+), 1 deletion(-)
git diff --name-only   # scripts/check-test-debt.mjs
git diff --check       # no output
git status --porcelain # no source changes; the plan stays ignored by /plans/ until force-added
```

No test file, allowlist, `package.json` or workflow is touched.

## 7. Risks and rollback

- Blast radius: the developer debt report only. A wrong match would change findings/ids/lines, but
  the report is allowlisted exactly and AC2 pins it byte-for-byte; AC1 covers the fixtures.
- No product, IPC, persistence, release or CI-contract change; the script has no dependencies.
- Revert is a single-line revert of one commit. No migration, no rollout.

## 8. Implementation order

1. Capture the BEFORE report (AC2).
2. Apply the exact one-line replacement from section 4.
3. Run AC1, AC2, AC3a, AC3b and AC4; keep the raw outputs.
4. Commit the source line as `fix(2088): remove exponential-backtracking fn regex in test-debt scan`
   plus the plan (`git add -f plans/2088-sonar-regex-backtracking.md`); push the branch.
   No PR/review ceremony is in scope for this Express band unless the coordinator asks.
5. Reply to `ac-tech-lead-v4` with the plan path, commit SHA, the raw AC1-AC4 evidence, and the
   Express confirmation (one file, one line, no test-file change).

## Plan Contract

No TBD, no open decision, no competing alternative: the exact before/after literals in section 4 are
the sole change. The single touched symbol is `fnRe` in `scanRustFile`, `scripts/check-test-debt.mjs`
line 307 at base `5203c4e3`. The single changed file is `scripts/check-test-debt.mjs`. Every
acceptance criterion is a command with an objective pass condition (section 6). The only new file is
this plan; the implementation diff is one line.
