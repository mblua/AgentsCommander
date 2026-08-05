# Implementation Plan: #1213 Cross-platform skills checker (`scripts/01-skills-checker.mjs`)

Status: READY_FOR_IMPLEMENTATION

Certified by the architect in the Step 7 consensus pass, after the Step 5 dev-rust enrichment and the
Step 6 dev-rust-grinch adversarial pass. No implementation decision is left open (Section 13, Section
16.3).

Full path. This file is the complete cold-start specification: the implementer will have only this
file, so every decision below is closed.

**Read this before Section 6.** The plan was written in four passes and each one changed rules the
previous pass got wrong. The records are Sections 14, 15 and 16; the normative text is Sections 1
through 13. Where a record and a numbered section disagree, **the numbered section wins**, because
that is what the implementer follows.

- **Step 5 (dev-rust), Section 14.** All five Section 12 unknowns resolved against pinned source with
  `file:line`, including vendored `serde_yaml 0.9.34`. Seven fidelity defects, two of them false
  passes where the checker would have approved a file the indexer rejects.
- **Step 6 (dev-rust-grinch), Section 15.** Seventeen further defects: three more false passes and
  six false positives. Codebase Memory gate verified green at `f08b8241`. The taxonomy grew from 28
  codes to 31 and the self-test from 75 cases to 109. Three of its findings change the *shape* of the
  traversal and the YAML parser, not just their wording.
- **Step 7 (architect), Section 16.** Eight cross-reference gaps the enrichment passes left behind,
  two of them behaviour-affecting: a self-test case asserting a code that no longer exists, and a
  taxonomy code with no case at all. The self-test is now **110 cases** and 14 acceptance criteria.

The single most important structural fact, because it is what produced most of the defects above: the
`.mjs` is a hand-maintained second implementation of `session_context.rs:206-714`. Every rule here is
a claim about that range. Section 10 item 4 and Section 15.5 both say the same thing about what that
costs over time.

## 1. Issue, baseline and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1213 (`Add cross-platform skills checker
  script to validate SKILL.md structure`).
- Branch: `feature/1213-skills-checker`, created from `main` at
  `f08b82419b7943d694965af000630bf053e2922a`.
- **Baseline for every coordinate in this plan: `f08b8241`.** The branch has no commits on it yet, so
  every `file:line` below is valid at branch HEAD.
- Delivery classification: FULL. One new file with a full CLI surface, a hand-written parser, a
  finding taxonomy, a machine-readable output format and a self-test harness.

**Objective.** Ship one Node script that tells a skill author, from any machine and any OS and
without launching the app, exactly which `SKILL.md` files AgentsCommander would reject and why, using
the same rules the Rust indexer enforces.

**Non-objective.** The checker does not change, wrap, call or link the Rust indexer. It is a second,
independent implementation of the same rules, and Section 11 treats the resulting drift as the
project's main long-term risk.

## 2. Evidence and the current-state gap

### 2.1 Ground truth

Every validation rule in this plan comes from the read-only, line-accurate investigation dev-rust
completed against HEAD `f08b8241`:

`.ac/wg-11-dev-v4-team/__agent_dev-rust/findings/skill-md-validation-rules.md`

That artifact is authoritative and Rule IDs used throughout (`R1`, `E1`, `F1`, `Y1`, `N1`, `D1`,
`W1`, `X1`) are its IDs, so every finding code below is traceable to a line of Rust.

**One correction, made in Step 5 by the artifact's own author:** its hard-error table is one entry
short, so its count of 22 should read 23. The missing condition and the reasoning are in
Section 14.4. Its single self-declared unknown, duplicate-key behaviour, is resolved in Section 12.1.
Everything else in it was re-checked against `f08b8241` during Step 5 and holds.

Headline facts this plan is built on:

- The whole indexer is one module: `src-tauri/src/config/session_context.rs:206-909`. The rules the
  checker mirrors live in `:206-714`.
- 23 hard-error conditions (skill not indexed; the artifact says 22 and is one short, see
  Section 14.4), 3 soft warnings (skill indexed but flagged), and 4
  rendering-time degradations that are **not** validation outcomes and are therefore out of scope.
- `SKILL.md` is compared byte-exactly at `:401`
  (`entry.file_name() != OsStr::new(SKILL_MD_FILENAME)`), so wrong casing fails on **every** platform
  including Windows. Pinned by `discover_skill_index_wrong_case_skill_md_warns_on_windows_too`
  (`:8402-8418`).
- Depth is fixed at one: `<owner-root>/skills/<skill-name>/SKILL.md`. Exactly one root is scanned per
  session. `__agent_*` replica directories are never scanned; only `_agent_*` matrices are
  (`:1437-1442`).
- `name` is optional and falls back to the containing directory name. The resolved name must match
  `^[a-z0-9-]{1,64}$` or the skill is hard-rejected, **including when the name came from the folder
  fallback** (`:464-470`, `:652-660`).
- A missing, empty or wrongly typed `description` is the only metadata problem that keeps a skill
  alive. Everything about `name`, the entrypoint, the delimiters and the YAML shape is fatal.

### 2.2 The gap

Verified at `f08b8241` by the same investigation:

- `scripts/` holds 8 `.mjs`, 4 `.ps1` and 1 `.sh`. None touches skills.
- No workflow in `.github/workflows/` validates skills.
- The only existing verification is the Rust unit module at `session_context.rs:8329-8707`, which
  validates the indexer against `tempfile` fixtures, never the skill files that ship on disk.

So a malformed skill is silent at authoring time: it simply never appears in the agent's context, and
the author learns about it only by reading startup warnings. A live instance exists in this workspace
right now: ``Skipped skill directory `tiktok-news-publisher`: missing exact SKILL.md entrypoint``.

### 2.3 Repo conventions the script must follow

Verified at `f08b8241`:

- Cross-platform tooling is `.mjs` run through `node` and wired to `package.json`
  (`scripts/check-version-sync.mjs`, `scripts/validate-branch-name.mjs`,
  `scripts/check-test-debt.mjs`). `.ps1` and `.sh` appear only for platform-specific work.
- Script header shape (`scripts/check-version-sync.mjs:1-13`): `#!/usr/bin/env node`, a one-line
  purpose naming the issue, a short rationale, a `Usage:` block, an `Exit codes:` block.
- Output is prefixed with a bracketed tag, for example `[check-version-sync] OK ...`.
- Arguments are parsed by a hand-rolled `parseArgs(process.argv.slice(2))`
  (`validate-branch-name.mjs:30-37`, `check-test-debt.mjs:860-877`).
- A `.mjs` script is self-tested through a `--self-test` flag with runtime `mkdtemp` fixtures, not
  through vitest (`check-test-debt.mjs:702-858`, wired as `test:debt:self`).
- CI pins Node 22 in every workflow (`actions/setup-node@v5`, `node-version: 22`).
- `vitest.config.ts:12` collects `['src/**/*.test.ts', 'src/**/*.test.tsx']` only. Nothing under
  `scripts/` is collected today.
- `plans/` is gitignored (`.gitignore:11`), so this plan file needs `git add -f`.

## 3. Scope

### In scope

| # | Deliverable |
| --- | --- |
| 1 | New file `scripts/01-skills-checker.mjs`. |
| 2 | Two new entries in `package.json` `scripts` (Section 5.2). |
| 3 | This plan file, force-added (Section 8, step 0). |

Zero new dependencies. `package-lock.json` is not touched. The script must run via
`node scripts/01-skills-checker.mjs` in a fresh checkout with no `npm install`.

### Out of scope

Taken from the issue's own out-of-scope list and from the evidence artifact:

- Any change to the Rust indexer or its behaviour.
- Auto-fixing malformed skill files. The checker never writes to disk outside its own temp fixtures.
- Wiring the check into CI or the husky `pre-push` hook. The `package.json` entry makes it
  attachable; attaching it is a separate issue.
- The rendering-time degradations (evidence artifact section 2, "Rendering-time degradation"): the
  1536-char trigger truncation and the 65536-byte context budget. They are not validation outcomes
  and cannot make a valid skill invalid.
- Root-resolution rules `R1` and `R2` beyond the directory-name shape: matrix canonicalization, the
  symlink test on the matrix directory, and the "canonical parent must be a workspace dir" test
  (`:1437-1479`). The checker classifies position by directory name only (Section 6.7) and never
  fails a run on it.
- Keys the indexer never reads: `allowed-tools`, `license`, `metadata`, `version` (evidence `X1`).
- Anything in the `SKILL.md` body after the closing delimiter. The indexer returns at the closing
  delimiter (`:366-368`) and imposes no body rule at all (evidence 3.4).

## 4. The decided solution

One file, `scripts/01-skills-checker.mjs`. The name and the `01-` prefix are the user's decision and
are settled.

The script walks a tree, finds every `SKILL.md` **case-insensitively**, validates each one against
the indexer's rules, classifies each one's indexability by position, and reports. Discovery is
case-insensitive so that a wrongly cased entrypoint is found rather than missed; validation is
byte-exact so that a wrongly cased entrypoint is still reported as a hard error, because the indexer
rejects it on every platform.

Decisions taken here, so that none is left to the implementer:

| # | Decision | Taken | Why |
| --- | --- | --- | --- |
| 1 | Entrypoint discovery | `fs.readdirSync(dir, { withFileTypes: true })`, then `entry.name.toLowerCase() === 'skill.md'` to discover, then `entry.name === 'SKILL.md'` to validate | `fs.existsSync(path.join(dir, 'SKILL.md'))` passes on Windows for `skill.md` and would produce a checker that accepts files the app rejects. User decision, restated as an implementation constraint. |
| 2 | Wrong casing | Hard error, and validation continues | It is fatal on every platform (`:401`). Continuing means the author gets one complete report instead of a fix-one-find-another loop. |
| 3 | Root argument | Positional, optional, defaults to `process.cwd()`. No `--root` alias | The issue says "takes a root as an argument". One way to say one thing. |
| 4 | Traversal | Recursive, no depth limit, skipping `.git`, `node_modules`, `target` **except inside a `skills/` directory, where no name filter applies at all** (6.2 step 3) | User decision 4. The checker is generic over any tree and is not limited to Agent Matrix roots. The carve-out exists because the indexer applies no name filter, so `skills/target/` is a legal skill directory (Step 6 defect G2). |
| 5 | Exit codes | `0` no error findings, `1` at least one error finding, `2` usage or IO failure | User decision 3. Warnings and informational findings never change the exit code, mirroring the indexer, where a soft warning still indexes the skill. |
| 6 | Position findings | Always severity `info`, never affect the exit code | Decides the question the coordinator left open. The checker is generic over any tree and cannot know whether a given tree is an Agent Matrix; making position fatal would fail every run over an ordinary repository. |
| 7 | YAML | Hand-written mini-parser over a fixed subset, zero dependencies | User decision 5. Anything outside the subset becomes an explicit `W-YAML-UNDECIDABLE` finding and is never silently approved. |
| 8 | Testing | In-script `--self-test` with runtime `mkdtemp` fixtures, not vitest | Section 9.1 gives the four reasons. |
| 9 | Streams | The whole report goes to stdout; only exit-2 failures write to stderr | A report is data. CI must not have to merge two streams to read it. |

### 4.1 Severity model

Three severities, and one rule that decides which applies.

| Severity | Meaning | Exit code effect |
| --- | --- | --- |
| `error` | The indexer would not index this skill | Forces exit `1` |
| `warning` | The indexer would index it but flag it, or the checker cannot judge it with certainty | None |
| `info` | Position and indexability notes; never a validation outcome | None |

**Severity assignment rule, applied uniformly:**

1. A finding about a **present entrypoint file** (its name, its type, its frontmatter, its YAML, its
   fields) takes the severity of the matching indexer outcome, **regardless of where the file sits**.
   A file named `SKILL.md` declares itself to be a skill, so it is held to the rules everywhere.
   Codes governed by rule 1: `E-ENTRYPOINT-CASE`, `E-ENTRYPOINT-LINK`, `E-ENTRYPOINT-NOT-FILE`,
   `E-ENTRYPOINT-UNREADABLE`, every `E-FM-*`, every `E-YAML-*`, every `E-NAME-*`, `W-ENTRYPOINT-CASE-SHADOWED`,
   `W-DESC-*`, `W-WHEN-NOT-STRING`, `W-YAML-UNDECIDABLE`, `W-NAME-UNDECIDABLE`, `I-KEY-HYPHEN-VARIANT`.
2. **Every other finding is gated on canonical position** (Section 6.7): `error` inside it, `warning`
   or `info` outside it. Outside canonical position the checker cannot distinguish a skills root from
   a coincidentally named folder, so it must not fail the run. This covers two kinds of finding that
   Step 4 stated as one, and the distinction matters because Step 6 added codes of the second kind:
   - **Absence-type**, where the checker cannot see what it needs: `I-NO-ENTRYPOINT` promoting to
     `E-NO-ENTRYPOINT`, and `W-DIR-UNREADABLE` promoting to `E-SKILLS-DIR-UNREADABLE` or
     `E-SKILL-DIR-UNREADABLE`.
   - **Present-entry shape**, where a directory entry exists but is the wrong kind of thing:
     `W-SKILL-DIR-LINK` promoting to `E-SKILL-DIR-LINK`, and `E-SKILLS-NOT-DIR`, which is emitted
     only inside the gate and has no ungated counterpart.

   These are not entrypoint files, so rule 1 does not reach them; they are not absences either, which
   is why Step 4's wording left them uncovered and 6.2 step 3 had to invoke a rule that did not yet
   describe its own case.

   **The `skills` name test inside the gate is deliberately not uniform**, and this is the one place
   where the two kinds diverge. Absence-type findings test `skills` **byte-exactly** (6.2 step 1), so
   a case-variant `Skills/` stays a warning: promoting it would make the same tree exit 1 on Windows
   and 0 on Linux, and Section 12.4 established that the `skills` path is a join, so its casing really
   is platform-dependent. Present-entry findings test `skills` **ASCII case-insensitively** (6.2
   steps 3 and 4), so a case-variant `Skills/` still yields the error on both platforms, which keeps
   the report stable rather than silently different. Both choices serve cross-platform stability and
   reach it from opposite directions because the failure modes are opposite: a missed absence is a
   false negative on one OS, a reported shape error is the same finding on both. Neither is an
   accident and neither may be "harmonized" by an implementer.
3. Position itself is always `info` (decision 6 above).

## 5. Affected surfaces: exact files and symbols

### 5.1 `scripts/01-skills-checker.mjs` (new, the only production file)

Module layout, in file order. These are the exact function names to implement; the self-test and the
acceptance criteria reference them.

| Symbol | Responsibility | Section |
| --- | --- | --- |
| header comment | Purpose, issue, **source of truth `src-tauri/src/config/session_context.rs:206-714`**, usage, exit codes | 5.1.1 |
| `SKILL_MD = 'SKILL.md'`, `SKILLS_DIR = 'skills'`, `FRONTMATTER_MAX_BYTES = 16384`, `FIRST_LINE_MAX_BYTES = 1024`, `NAME_RE = /^[a-z0-9-]{1,64}$/`, `SKIP_DIRS = new Set(['.git', 'node_modules', 'target'])` | Constants mirroring `session_context.rs:206-211` and `:464-470` | 6 |
| `parseArgs(argv)` | CLI parsing | 6.1 |
| `printUsage()` | `--help` text | 6.1 |
| `walk(root)` | Iterative directory walk, returns candidate records and traversal findings | 6.2 |
| `readFrontmatter(filePath)` | Byte-level frontmatter extraction | 6.4 |
| `isFrontmatterDelimiter(lineBytes, allowBom)` | Exact port of `session_context.rs:276-302` | 6.4.1 |
| `parseMiniYaml(text)` | The mini-parser | 6.5 |
| `validateEntrypoint(record)` | Entrypoint-layer rules `E1`-`E4` | 6.3 |
| `validateFields(mapping, dirName)` | Field-layer rules `N1`-`N5`, `D1`-`D2`, `W1`-`W2` | 6.6 |
| `classifyPosition(filePath)` | Indexability verdict | 6.7 |
| `detectDuplicates(candidates)` | Rule `N5`, scoped per `skills/` directory | 6.6.4 |
| `renderHuman(report)` | Default output | 6.9 |
| `renderJson(report)` | `--json` output | 6.10 |
| `selfTest()` | Section 9 | 9 |
| `main()` | Orchestration and exit code | 6.1 for the CLI and the exit-2 cases, 6.8 for the exit-code mapping |

**Step 7 correction to this table.** Step 4 wrote the last three rows as 6.8, 6.9 and 6.10, which is
off by one: 6.8 is the finding taxonomy, 6.9 is the human report and 6.10 is the JSON format, and
there is no section that owns `main()` alone. The error survived both enrichment passes because
Section 8 step 6 cites the correct sections and nobody compared the two. Corrected above. This table
is a locator, not a normative section; where it and a numbered section disagree, the numbered section
wins.

Helper functions the constraints above imply, named here so they are not improvised. Each is used in
more than one place and each has a correctness requirement in 5.1.2 or Section 7:

| Helper | Requirement |
| --- | --- |
| `trimYamlValue(s)` | The explicit Unicode `White_Space` set of 5.1.2. Never `String.prototype.trim()` |
| `asciiLower(s)` | A-Z to a-z only, mirroring Rust `to_ascii_lowercase()` |
| `compareUtf8(a, b)` | `Buffer.compare(Buffer.from(a, 'utf8'), Buffer.from(b, 'utf8'))` |
| `sanitizeForReport(s)` | Collapse C0 controls and Unicode whitespace runs to one U+0020, then truncate to 200 characters (Section 7). Presentation only; never feeds a verdict |

#### 5.1.1 Required header

The header must name the source-of-truth range verbatim, because the drift risk is the reason the
coordinator asked for it:

```js
#!/usr/bin/env node
// Validates SKILL.md structure the way AgentsCommander's indexer does, for issue #1213.
//
// SOURCE OF TRUTH: src-tauri/src/config/session_context.rs:206-714
// This script is a second, independent implementation of the rules that module enforces.
// If that range changes, this file is stale until it is changed to match.
//
// Discovery is case-insensitive so a wrongly cased entrypoint is FOUND; validation is
// byte-exact because session_context.rs:401 compares entry.file_name() against "SKILL.md"
// byte-for-byte, so wrong casing fails on every platform including Windows.
//
// The two path constants are NOT symmetric, and this is the surface's biggest trap:
//   "SKILL.md" (:401) is an entry-name comparison  -> case-sensitive on EVERY platform.
//   "skills"   (:509) is a path join               -> case-INsensitive on Windows only.
// So `_agent_x/Skills/y/SKILL.md` is indexed on Windows and invisible on Linux, while
// `_agent_x/skills/y/skill.md` is rejected on both.
//
// YAML semantics below are pinned to serde_yaml 0.9.34+deprecated (Cargo.lock:5483-5486):
// duplicate mapping keys are a HARD parse error (mapping.rs:813-822), and plain
// yes/no/on/off resolve as STRINGS, not booleans (de.rs:932-938, YAML 1.2 core schema).
// A serde_yaml bump invalidates both.
//
// Usage:
//   node scripts/01-skills-checker.mjs [root] [--json]
//   node scripts/01-skills-checker.mjs --help
//   node scripts/01-skills-checker.mjs --self-test
//
// Exit codes:
//   0 → no error findings (warnings and notes may still be present)
//   1 → at least one error finding: one or more skills would not be indexed
//   2 → usage error, or the root does not exist / is not a directory / cannot be read
```

#### 5.1.2 Node API constraints

These are correctness requirements, not style:

- Only `node:fs`, `node:path`, `node:os`, `node:url`, `node:util` (for `TextDecoder`, which is also
  global). No dependency, no dynamic import.
- Frontmatter bytes are decoded with `new TextDecoder('utf-8', { fatal: true })` inside a
  `try`/`catch`. **`buffer.toString('utf8')` is forbidden**: it replaces invalid sequences with
  U+FFFD and would silently pass a file the indexer rejects under `F8`.
- Name length is counted in Unicode scalar values, `[...name].length`, mirroring Rust
  `chars().count()` (`:465`). `name.length` counts UTF-16 code units and is wrong.
- **`String.prototype.trim()` is forbidden for field values.** `yaml_field_string` trims with Rust
  `str::trim` (`:453`), which uses the Unicode `White_Space` property. JS `trim()` uses a different
  set, and the two disagree in both directions:
  - **U+FEFF (ZWNBSP)**: JS trims it, Rust does not. `name: "﻿abc"` would trim to `abc` and pass
    the charset test in the checker, while the indexer keeps the U+FEFF, fails
    `is_valid_skill_name` and hard-rejects the skill. This is a **false pass** and it is the reason
    this constraint is not a style note.
  - **U+0085 (NEL)**: Rust trims it, JS does not. This direction only costs a false failure.

  Implement the trim against this explicit set, which is Unicode `White_Space` exactly:
  `U+0009`-`U+000D`, `U+0020`, `U+0085`, `U+00A0`, `U+1680`, `U+2000`-`U+200A`, `U+2028`, `U+2029`,
  `U+202F`, `U+205F`, `U+3000`. Note `U+180E` is **not** in it: it was removed from `White_Space` in
  Unicode 6.3 and current Rust follows current Unicode.
- **String ordering must be UTF-8 byte order, not JS code-unit order**, wherever a comparison mirrors
  Rust (`:588-591` and `:449-450`). Rust `String: Ord` compares UTF-8 bytes, which equals code-point
  order; JS `<` compares UTF-16 code units, which sorts astral characters (U+10000 and above, stored
  as surrogates in the D800-DFFF range) **below** U+E000-U+FFFF. Use
  `Buffer.compare(Buffer.from(a, 'utf8'), Buffer.from(b, 'utf8'))`. This only bites on exotic
  directory names, but the candidate sort decides who wins a duplicate-name clash, so a divergence
  there silently blames the wrong skill.
- ASCII lowercasing for sort keys and for the `skills` directory-name comparison uses an explicit
  A-Z to a-z map, not `String.prototype.toLowerCase()`, which is locale- and Unicode-aware and would
  diverge from Rust `to_ascii_lowercase()` (`:588-591`). The single exception is entrypoint
  **discovery** (`entry.name.toLowerCase() === 'skill.md'`), where over-matching is harmless because
  the byte-exact test immediately follows.
- Target runtime is Node 22, matching CI. No API newer than Node 18 is used.

### 5.2 `package.json`

Insert exactly two entries, immediately after the existing `"test:debt:self"` line
(`package.json:23`):

```diff
     "test:debt:self": "node scripts/check-test-debt.mjs --self-test",
+    "check:skills": "node scripts/01-skills-checker.mjs",
+    "check:skills:self": "node scripts/01-skills-checker.mjs --self-test",
     "test:watch": "vitest",
```

No dependency is added, so `package-lock.json` does not change and the `lockfile-check.yml` workflow
is unaffected. `scripts/check-version-sync.mjs:37` anchors on the adjacency of `"name"` and
`"version"` at the top of the file, which these edits do not disturb.

### 5.3 Files deliberately not touched

`vitest.config.ts` (Section 9.1 explains why the include list is not extended), `package-lock.json`,
`.husky/pre-push`, everything in `.github/workflows/`, and every file under `src-tauri/` and `src/`.
If the implementation needs any of them, the change was misunderstood.

## 6. Required behaviour, edge cases and failure behaviour

### 6.1 CLI surface

```
node scripts/01-skills-checker.mjs [root] [--json]
node scripts/01-skills-checker.mjs --help | -h
node scripts/01-skills-checker.mjs --self-test
```

| Input | Behaviour | Exit |
| --- | --- | --- |
| No arguments | Root is `process.cwd()`, human report | 0 or 1 |
| One positional | That positional is the root, resolved with `path.resolve()` | 0 or 1 |
| `--json` | JSON document to stdout, nothing else on stdout | 0 or 1 |
| `--help` or `-h`, anywhere in argv | Usage to stdout. Wins over every other argument, including an invalid one | 0 |
| `--self-test` | Runs the self-test (Section 9). May not be combined with any other argument | 0 or 1 |
| `--` | Terminates flag parsing; every token after it is positional. Allows a root beginning with `-` | as above |
| A second positional | `usage: at most one root may be given` to stderr | 2 |
| An unknown flag | `usage: unknown option '<flag>'` to stderr | 2 |
| `--json` given twice | Accepted, idempotent | as above |
| Root does not exist | `cannot read root '<path>': ENOENT` to stderr | 2 |
| Root is a file | `root '<path>' is not a directory` to stderr | 2 |
| Root exists but `readdirSync` on it throws | `cannot read root '<path>': <code>` to stderr | 2 |
| Any uncaught exception | `internal error: <message>` plus the stack to stderr | 2 |

Exit 2 is only ever about the invocation and the root itself. A directory that cannot be read
**inside** the walk is a finding, not an exit-2 failure (Section 6.2), because failing an entire run
because one unrelated subtree is permission-protected would make the checker unusable on a real
machine.

In `--json` mode an exit-2 failure writes a JSON object to **stderr** and nothing to stdout:

```json
{ "tool": "01-skills-checker", "version": 1, "error": "root '/x' is not a directory", "exitCode": 2 }
```

`--help` output is stdout in both modes and is never JSON.

### 6.2 Traversal

Iterative, using an explicit stack, not recursion, so a deep tree cannot exhaust the JS stack.

For each directory `D` popped from the stack:

1. `entries = fs.readdirSync(D, { withFileTypes: true })`. If it throws, in this order:
   - If `basename(D)` is `skills` byte-exactly and `basename(dirname(D))` matches `/^_agent_.+$/`,
     that is `D` is itself a canonical skills root: emit `E-SKILLS-DIR-UNREADABLE` (error), mirroring
     ``skills` directory could not be read`` and ``skills` could not be inspected``
     (`:527-530`, `:545-548`).

     **Step 6 note on the byte-exact test, because 14.5.5 argues the opposite for step 4.** Section
     12.4 established that `skills` is reached by a path **join**, so on Windows `_agent_x/Skills/`
     *is* the indexer's skills root, and an unreadable `Skills/` is a hard error there while this
     rule gives `W-DIR-UNREADABLE`. Byte-exact is kept anyway, and deliberately: promoting on a
     case variant would make the same tree exit 1 on Windows and 0 on Linux, and the absence-type
     findings are exactly the category Section 4.1 rule 2 says must not fail a run the checker
     cannot verify is a skills tree. Step 4 of this section takes the opposite call for
     `E-SKILLS-NOT-DIR`, and steps 3 and 4 both test `skills` case-insensitively, because those are
     **present-entry shape** findings rather than absence-type ones.

     **Step 7 correction.** Step 6's wording here said "so rule 1 governs it instead", which
     contradicts step 3 four paragraphs below, where the same gating is called "severity rule 2 of
     Section 4.1 applied consistently with `E-SKILLS-NOT-DIR` in step 4". Both cannot be true:
     `E-SKILLS-NOT-DIR` is emitted only when the parent matches `/^_agent_.+$/`, which is a canonical
     gate, and rule 1 is by definition ungated. Rule 2 governs it. What actually differs between
     step 1 and steps 3 and 4 is **not** whether the gate applies but how the `skills` name is
     matched inside it, byte-exact versus ASCII case-insensitive. Section 4.1 rule 2 now states both
     axes explicitly. The `W-DIR-UNREADABLE` message must still say that a case-variant `skills`
     directory may be the live skills root on Windows.
   - **Else if `D` is a skill directory in canonical position**, that is `D` satisfies both tests of
     Section 6.7 (`basename(dirname(D))` is `skills` and `basename(dirname(dirname(D)))` matches
     `/^_agent_.+$/`): emit `E-SKILL-DIR-UNREADABLE` (error). This is the indexer's
     ``Skipped skill directory `{folder}`: unable to read skill directory: {err}`` (`:396-397`),
     which is a hard error for that skill. Step 4 wrote the first test as "Section 6.7 test 1 applied
     to `D` itself", which only ever matches the `skills/` root and therefore downgraded this real
     hard error to a warning. Step 5 promoted it but reused `E-SKILLS-DIR-UNREADABLE`; Step 6 split
     the code so `indexerMessage` stays truthful (6.8).
   - Otherwise: emit `W-DIR-UNREADABLE` (warning) carrying the errno code. The walk continues.
2. Discover entrypoints in `entries` (Section 6.3).
3. For **every** entry, in this order. **The order and the guard shape are load-bearing and Step 6
   rewrote this step: the Step 4/5 wording opened with "For each entry that is a directory", which
   made the symlink branch below unreachable.** In Node, `readdirSync(D, { withFileTypes: true })`
   returns `Dirent`s with **lstat semantics**: for a symlink, `isDirectory()` is `false` and
   `isSymbolicLink()` is `true`. The two are mutually exclusive, so any symlink test nested under an
   `isDirectory()` guard never runs. Mirror the indexer's own order at `:578-585`, which is
   `if is_symlink() { warn } else if is_dir() { candidate }`:

   - **First, `entry.isSymbolicLink()`.** If true, **do not descend**, and do not apply any
     directory test to it. Node reports Windows junctions and reparse points as symbolic links, so
     this covers both platforms. Then:
     - If `D` is a `skills` directory **in canonical position** — `path.basename(D)` is `skills`
       (ASCII case-insensitive) **and** `path.basename(path.dirname(D))` matches `/^_agent_.+$/` —
       emit `E-SKILL-DIR-LINK` (error), mirroring ``Skipped linked skill directory `{folder}`:
       linked/reparse-point directories are not followed`` (`:579-581`). Note the indexer warns for
       **any** symlink entry directly under `skills/`, including a symlink whose target is a file or
       is missing: `:578` tests only `file_type.is_symlink()` and never inspects the target. So this
       finding does **not** depend on what the link points at.
     - Else if `path.basename(D)` is `skills` (ASCII case-insensitive) but the grandparent test
       fails, emit `W-SKILL-DIR-LINK` (warning). The indexer never scans that tree, so a hard error
       there would fail a run over an ordinary repository that happens to contain a `docs/skills/`
       folder. This is severity rule 2 of Section 4.1 applied consistently with `E-SKILLS-NOT-DIR`
       in step 4, which was already gated this way. Step 4/5 emitted the **error** unconditionally.
     - Otherwise emit nothing. Not descending into arbitrary links is traversal policy, not a
       finding.
   - **Else if `entry.isDirectory()`:**
     - If `path.basename(D)` is `skills` (ASCII case-insensitive), **never apply `SKIP_DIRS`**: push
       it. The indexer applies no name filter at all (evidence 3.1), and `target`, like any string
       matching `^[a-z0-9-]{1,64}$`, is a legal skill name. Skipping it would hide a real skill
       directory from the checker entirely while the indexer still rejects or indexes it. Step 4/5
       applied `SKIP_DIRS` unconditionally; see Step 6 defect G2.
     - Otherwise, if the ASCII-lowercased name is in `SKIP_DIRS` (`.git`, `node_modules`, `target`),
       skip it and do not descend. The skip list is a checker-side choice, not an indexer rule.
       Comparison is case-insensitive so `Node_Modules` is also skipped.
     - Otherwise push it onto the stack.
   - **Else** (a regular file, or anything that is neither) it is not traversed. A plain file sitting
     directly inside `skills/` produces no finding, which agrees with the indexer: `:583` is
     `else if file_type.is_dir()`, so a non-directory, non-symlink entry is silently ignored there.
     The single exception is an entry named `skills` itself, handled in step 4.
4. For each entry named `skills` (ASCII case-insensitive) whose parent matches `^_agent_.+$` and
   which is **not** a directory: emit `E-SKILLS-NOT-DIR` (error), mirroring
   ``skills` exists but is not a directory`` (`:535-538`).

   **Exception, verified in Step 5: a broken symlink is silent.** `:520` gates everything on
   `skills_path.exists()`, and Rust `Path::exists` follows symlinks, so a `skills` symlink whose
   target is missing makes `exists()` false and `discover_skill_index` returns at `:521` with **no
   warning at all**. The three cases are therefore:
   - `skills` is a file, or a symlink resolving to anything that exists: `E-SKILLS-NOT-DIR`. A
     resolving symlink reaches `:524`, and `:534` rejects it on `is_symlink()` even when the target
     is a directory.
   - `skills` is a **broken** symlink: emit nothing. The indexer is silent here and the checker must
     be too, or it invents a failure the app never reports.
   - `skills` is absent: emit nothing (`:520-522`).

   Detecting the broken-symlink case needs an explicit `fs.statSync(p, { throwIfNoEntry: false })`
   on the entry, because `Dirent.isSymbolicLink()` alone cannot tell a resolving link from a broken
   one. That single `statSync` is the only place in the walk that follows a link.

**Loop protection.** Symlinked directories are never descended into, so a symlink cycle is
unreachable. As unconditional insurance, the walker keeps a `Set` of `path.resolve(D)` strings and
refuses to push a directory already in it. If that guard ever fires it emits nothing and simply skips
the directory; it exists so that no filesystem quirk can hang the walk.

The walk counts every directory it lists into `summary.directoriesScanned`.

### 6.3 Entrypoint discovery and the casing rule

Within directory `D`, from the same listing:

```
matches = entries.filter(e => e.name.toLowerCase() === 'skill.md')
```

| Situation | Behaviour |
| --- | --- |
| `matches` is empty and `D`'s parent is named `skills` (ASCII case-insensitive) | `I-NO-ENTRYPOINT` (info), promoted to `E-NO-ENTRYPOINT` (error) when `D` is in canonical position (Section 6.7). Mirrors ``missing exact SKILL.md entrypoint`` (`:417`). This is the `tiktok-news-publisher` case from the issue. |
| `matches` is empty and `D`'s parent is not named `skills` | Nothing. `D` is an ordinary directory. |
| Exactly one match, and `e.name === 'SKILL.md'` | Validate fully. No casing finding. |
| Exactly one match, and `e.name !== 'SKILL.md'` | `E-ENTRYPOINT-CASE` (error), **then validate fully**. The author gets the casing verdict and the frontmatter verdict in one run. |
| Two or more matches (only reachable on a case-sensitive filesystem), one of them byte-exact | Validate the byte-exact one fully. Each other one gets `W-ENTRYPOINT-CASE-SHADOWED` (warning) and is not validated further: the skill does work today, and the stray file is a hazard, not an indexer rejection. |
| Two or more matches, none byte-exact | Each gets `E-ENTRYPOINT-CASE` and each is validated fully. |

For each match selected for validation, before reading it:

| Test | Finding | Indexer counterpart |
| --- | --- | --- |
| `e.isSymbolicLink()` | `E-ENTRYPOINT-LINK` (error), stop validating this file | ``exact SKILL.md entrypoint is linked/reparse-point`` (`:409`) |
| `e.isDirectory()`, or neither file nor symlink nor directory | `E-ENTRYPOINT-NOT-FILE` (error), stop | ``exact SKILL.md entrypoint is not a regular file`` (`:412`) |
| `fs.openSync` or `fs.readSync` throws | `E-ENTRYPOINT-UNREADABLE` (error) carrying the errno, stop | ``failed to open/read SKILL.md frontmatter`` (`:324-325`, `:332-334`) |

`E-ENTRYPOINT-LINK`, `E-ENTRYPOINT-NOT-FILE` and `E-ENTRYPOINT-UNREADABLE` are errors regardless of
position, per the severity rule 1: the file declares itself to be a skill.

### 6.4 Frontmatter extraction

Byte-level, streaming, bounded. Open the file, read in 64 KiB chunks with `fs.readSync`, and scan for
line terminators. **Never** read the whole file: a `SKILL.md` may have an arbitrarily large body and
the indexer never reads past the closing delimiter (`:366-368`).

State machine:

1. **First line.** Accumulate bytes until the first `\n` **inclusive**, or until EOF.
   - **The first line including its terminator must be at most 1024 bytes.** Count bytes as they are
     appended and fail the moment the running length exceeds 1024, exactly as `:340-345` does: the
     length test runs after the byte is pushed and before the `\n` short-circuit at `:353`, so the
     terminator is counted. 1020 spaces plus `---\n` is 1024 bytes and is valid; 1021 spaces plus
     `---\n` is 1025 bytes and fails. Do **not** implement this as "find `\n` within the first 1024
     bytes": that is off by the terminator byte and accepts a line the indexer rejects.
   - On exceeding it: `E-FM-FIRST-LINE-TOO-LONG` (error), stop. Mirrors rule `F7` (`:342-345`), which
     the indexer reports as ``missing opening frontmatter delimiter``; the checker gives it a distinct
     code because the cause is different and the fix is different.
   - If EOF is reached with no `\n` and the accumulated length never exceeded 1024, the whole file is
     line 1. Continue. Note this makes the limit a ceiling on whitespace padding around the opening
     `---` as well (Section 12.2).
   - A file whose line endings are bare `\r` with no `\n` has no line terminator by this definition,
     so the whole file is line 1. It will fail as `E-FM-NO-OPEN` (or `E-FM-FIRST-LINE-TOO-LONG` past
     1024 bytes) because `is_frontmatter_delimiter` strips at most one trailing `\r`. Classic-Mac
     line endings are **not** supported; rule `F3` covers `\r\n` only.
2. **Opening delimiter.** Apply `isFrontmatterDelimiter(line1, allowBom = true)` (`:358`).
   If false: `E-FM-NO-OPEN` (error), stop. Mirrors `F1` and `F2`.
3. **Body lines.** Read line by line. **Two size guards apply, and both are required.**

   **Guard A, mid-line** (`:346-351`). While accumulating the bytes of any line after the opening
   delimiter, and **before** deciding whether that line is the closing delimiter, fail with
   `E-FM-TOO-LARGE` the moment

   ```
   currentLineBytes.length > (16384 - frontmatterBuffer.length) + 8
   ```

   The `+ 8` is verbatim from `remaining.saturating_add(8)` at `:348`. This guard also covers the
   **closing delimiter line**, which never reaches the append path, so a `---` padded with enough
   leading whitespace to exceed the budget is rejected as an over-size frontmatter rather than
   accepted as a valid close. Omitting Guard A makes the checker accept a file the indexer rejects
   (Section 12.3).

   **Guard B, on append** (`:311-317`). Only after Guard A passes and the line proves not to be the
   closing delimiter, test `frontmatterBuffer.length + lineBytes.length > 16384` before appending.

   With both guards in place, apply `isFrontmatterDelimiter(line, allowBom = false)` (`:366`, `:382`):
   - True: the frontmatter block is complete. Stop reading the file. Nothing after this point is ever
     inspected: the checker imposes **no body rule at all** (evidence 3.4).
   - False: append the line's raw bytes, **including its line terminator exactly as read**, to the
     frontmatter buffer. Terminators are counted and `\r\n` costs two bytes; both delimiter lines are
     excluded. Confirmed in Section 12.3. On Guard B failing: `E-FM-TOO-LARGE` (error), stop.
     Mirrors `F6` (`:304-309`, `:313`, `:349`).
   - EOF before a closing delimiter: `E-FM-NO-CLOSE` (error), stop. Mirrors `F5` (`:388-392`).

   **EOF with an unterminated final line: `:374-386`, and it is not symmetric. Step 6 promoted this
   from a passing citation to its own rule, because the common half of it was never stated.** When
   the read loop ends with `current_line` non-empty — that is, the file does not end with `\n` — the
   indexer does **not** discard that tail. It runs the same tests on it, in this order:

   | State at EOF | Tail is a delimiter? | Indexer outcome | Checker |
   | --- | --- | --- | --- |
   | Opening never seen | yes (`allowBom = true`, `:376`) | ``missing closing frontmatter delimiter`` | `E-FM-NO-CLOSE` |
   | Opening never seen | no | ``missing opening frontmatter delimiter`` | `E-FM-NO-OPEN` |
   | Opening seen | yes (`allowBom = false`, `:382`) | **success**: returns the frontmatter | **valid, no finding** |
   | Opening seen | no | append the tail (Guard B, `:385`), then ``missing closing frontmatter delimiter`` | `E-FM-TOO-LARGE` if the append overflows, else `E-FM-NO-CLOSE` |

   Row 3 is the one that matters and the one Step 4/5 left implicit: **`---\nname: ok-name\n---`
   with no trailing newline is a fully valid skill.** Files without a final newline are produced by
   every editor that does not force one, so this is not an exotic shape. The natural
   implementations — `split('\n')` and iterate, or "a line is complete only when a `\n` is seen" —
   either drop the unterminated tail or never test it, and report `E-FM-NO-CLOSE` on a file the app
   indexes cleanly. That is a false positive on a very common file. Self-test cases 29a and 29b pin
   both halves.

   Row 4 also fixes an ordering detail: the append at `:385` happens **before** the
   ``missing closing`` return at `:389`, so an unterminated final body line that overflows the
   budget yields `E-FM-TOO-LARGE`, not `E-FM-NO-CLOSE`. Guard A has already been applied to that
   line byte by byte during accumulation, so it is Guard B that decides here.
4. **UTF-8.** Decode the accumulated buffer with `new TextDecoder('utf-8', { fatal: true })`. On
   throw: `E-FM-NOT-UTF8` (error), stop. Mirrors `F8` (`:319-321`).
   Only the frontmatter block is required to be valid UTF-8. The body may be arbitrary bytes.

#### 6.4.1 `isFrontmatterDelimiter`: exact port of `session_context.rs:276-302`

Operating on the raw line bytes, in this order. The order is load-bearing and the self-test pins it.

1. If the last byte is `\n` (0x0A), drop it.
2. Then if the last byte is `\r` (0x0D), drop it.
3. If `allowBom` is true **and** the remaining bytes start with `EF BB BF`, drop those three bytes.
4. Trim leading and trailing ASCII whitespace, defined exactly as Rust `trim_ascii`:
   `\t` (0x09), `\n` (0x0A), `\x0C` (0x0C), `\r` (0x0D), `space` (0x20).
5. Return true if and only if the remainder is exactly the three bytes `2D 2D 2D`.

**Step 5 verification: this port is correct, step for step.** `:278-280` strips `\n`, `:281-283`
strips `\r`, `:284-286` strips the BOM under `allow_bom`, `:287-293` trims leading ASCII whitespace,
`:294-300` trims trailing, `:301` compares to `b"---"`. The load-bearing claim holds: **the BOM is
stripped before the trim** (`:284-286` precedes `:287`), which is what makes `  <BOM>---` false and
`<BOM>  ---` true. The whitespace set is also exact: Rust `u8::is_ascii_whitespace` is
`\t` `\n` `\x0C` `\r` and space, and it does **not** include vertical tab `\x0B`, so a `\x0B`-padded
delimiter is correctly rejected. Do not reach for a JS regex like `\s` or `String.prototype.trim()`
here: both include Unicode whitespace and `\x0B`, and both would accept delimiters the indexer
rejects. Compare bytes, not characters.

Consequences the self-test pins:

| Input line | `allowBom` | Result |
| --- | --- | --- |
| `---\n` | either | true |
| `---\r\n` | either | true (`F3`) |
| `  ---  \n` | either | true (`F4`) |
| `<BOM>---\n` | true | true (`F2`) |
| `<BOM>---\n` | false | **false**. A BOM is tolerated on the opening delimiter only (`F5`) |
| `  <BOM>---\n` | true | **false**. Step 3 requires the BOM to be leading; leading spaces defeat it |
| `----\n` | either | false. Longer fences are rejected |
| `+++\n` | either | false. TOML delimiters are rejected |
| `\n---\n` as the file's first two lines | true | false on line 1: leading blank lines are not tolerated (`F1`) |

### 6.5 The YAML mini-parser

The parser answers exactly two questions per key: *is this key present*, and *is its value a YAML
string*. Nothing else about the value matters to the indexer, because `yaml_field_string`
(`:448-462`) only accepts `Value::String`, trims it, and maps an empty result to absent.

#### 6.5.1 The supported subset

A top-level block mapping and nothing else.

| Construct | In subset | Resolution |
| --- | --- | --- |
| Blank line (only ASCII whitespace) | yes | ignored |
| Comment line (first non-space character is `#`) | yes | ignored |
| `key: value` at column 0 | yes | mapping entry |
| `key:` at column 0 with nothing after it | yes | value is YAML `null`, therefore **not a string** |
| `key: # comment` | yes | value is `null`. An unquoted ` #` preceded by whitespace starts a comment |
| Single-quoted scalar `'...'`, opening and closing quote on the same line, `''` as the escaped quote | yes | string |
| Double-quoted scalar `"..."`, both quotes on the same line, escapes `\\` `\"` `\/` `\n` `\r` `\t` `\uXXXX` | yes | string |
| Quoted key, `'name': x` or `"name": x` | yes | mapping entry, key is the unquoted text |
| Block scalar `key: \|`, `\|-`, `\|+`, `>`, `>-`, `>+`, and the explicit-indentation forms `\|2`, `>2`, `\|2-`, `>2+` and so on, followed by an indented block | yes | string. Literal blocks join with `\n`; folded blocks join with a space and keep blank lines as `\n`. Exact folded whitespace is never load-bearing: no checker outcome depends on anything but whether the value trims to empty, and both foldings agree on that |
| Plain scalar (unquoted) resolving as a YAML 1.2 core-schema string | yes | string |
| `true` `True` `TRUE` `false` `False` `FALSE` | yes | boolean, **not a string**. This is the complete bool set: `de.rs:932-938` matches these six spellings and nothing else |
| `null` `Null` `NULL` `~` | yes | null, **not a string**. Complete null set: `de.rs:925-930` |
| `yes` `no` `on` `off` `y` `n`, any casing | yes | **string.** `serde_yaml` 0.9.34 applies the YAML 1.2 core schema, so these are not booleans (Section 12.5). `description: yes` is clean and produces no finding |
| An empty value after `key:` where the key is followed only by whitespace | yes | null, **not a string**. Distinct from `key: ''`, which is an empty string and is therefore treated as absent by `Y5`, not as a type error |
| Integer `^[-+]?[0-9]+$`, `0x...`, `0o...` | yes | number, **not a string** |
| Float `^[-+]?[0-9]*\.[0-9]*([eE][-+]?[0-9]+)?$`, `.inf` `-.inf` `.nan` and case variants | yes | number, **not a string** |

#### 6.5.1a The line grammar, stated exactly

**Step 6 addition.** The table above names the constructs but never defines how a line is split into
a key and a value, and the mini-parser's whole behaviour turns on that. Left unstated, an implementer
reaches for `line.split(': ')`, and every legal spelling the split misses falls through to
`W-YAML-UNDECIDABLE` — which, under 6.6.1, used to become a hard `E-NAME-INVALID`. Fix the grammar:

- **Key/value separator.** A mapping entry is a key, then `:`, then **either** end of line, **or** at
  least one space or **tab** (0x09). YAML permits a tab as separation whitespace, so `name:<TAB>x` is
  an ordinary mapping entry, not an undecidable line and not a parse error.
- **Trailing comments.** An unquoted `#` that is preceded by at least one space or tab starts a
  comment and terminates the value, for **every** scalar form, not only the empty one already listed
  in the table. So `name: my-skill  # renamed` has the value `my-skill`, and
  `name: "my-skill"  # renamed` has the value `my-skill`. A `#` not preceded by whitespace is an
  ordinary character: `name: a#b` is the string `a#b`.
- **Block scalar indentation.** The block's indentation is the explicit indicator when one is given
  (`|2`), otherwise the indentation of the block's first non-empty line. Every subsequent line
  indented at least that far is content, **whatever it contains**, and is consumed by the block. This
  is stated because getting it wrong is not a cosmetic error: a `description: |` block whose body
  contains a line like `name: x` would otherwise be read as a second top-level `name` key and emit a
  spurious `E-YAML-DUPLICATE-KEY`, which is an **error** and exits 1 on a completely ordinary file.
- **Tabs inside a block scalar are content, not indentation.** See the corrected rule in 6.5.3.

#### 6.5.2 Outside the subset: `W-YAML-UNDECIDABLE`

Every construct below produces `W-YAML-UNDECIDABLE` (warning) naming the line number and the
construct, and the affected key's value is recorded as `undecidable`. This is the category the user
required: the parser must never approve silently what it cannot judge.

- A line indented at top level that is not part of a block scalar (a nested mapping or sequence).
- Flow collections: a value beginning with `{` or `[`.
- Anchors `&`, aliases `*`, tags `!`, directives `%`, explicit keys `? `, merge keys `<<`, the
  document-end marker `...`.
- A quoted scalar whose closing quote is not on the same line (a legal multi-line quoted scalar).
- A double-quoted scalar containing a backslash escape outside the supported set.
- Any other line at column 0, not blank and not a comment, that the parser cannot reduce to a
  `key:` form and that is not one of the certain-error shapes in 6.5.3.

**An `undecidable` value may never produce an `error` finding. Step 6 changed this.** Step 4/5 said
the charset check is applied to the literal text of an undecidable `name` and, through 6.6.1 step 3,
that check emits `E-NAME-INVALID`, an error, exit 1. That inverts the whole point of the category and
contradicts Section 4.1, which defines `warning` as precisely "the checker cannot judge it with
certainty". Three concrete false positives it produces, all on skills the indexer indexes cleanly:

- `name: !!str my-skill` — `serde_yaml` resolves the tag to the string `my-skill`, a valid name. The
  checker sees the literal `!!str my-skill`, fails the charset test and exits 1.
- `name: &anchor my-skill` — same, via an anchor.
- `name: "my-skill"  # renamed` — a trailing comment. Under the Step 4/5 subset the quoted-scalar
  rule ("both quotes on the same line") does not match the whole remainder, so the line is
  undecidable and the literal fails the charset test. 6.5.1a now puts this back inside the subset,
  but the severity rule below is what stops the next unlisted-but-legal spelling from exiting 1.

So: when `name`'s value is `undecidable`, run the charset check for the author's benefit and report
it as **`W-NAME-UNDECIDABLE` (warning)**, never `E-NAME-INVALID`. Its message states both possible
indexer outcomes — the skill may be indexed under a name the checker could not resolve, or the
indexer may reject it with ``name must be a string`` — and says the checker cannot tell which. The
run is not failed on a value the checker has declared itself unable to judge.

An undecidable `description` or `when_to_use` needs no equivalent rule: every outcome on those two
keys is already a warning on both sides, so the severities agree whatever the value resolves to.

#### 6.5.3 Certain errors

Only these three, because each is invalid under any reading:

| Condition | Finding | Indexer counterpart |
| --- | --- | --- |
| A tab character (0x09) appears in the leading whitespace of a frontmatter line that the parser is treating as a **mapping entry**, that is, a line **not** consumed as block-scalar content | `E-YAML-PARSE` (error) | ``YAML parse error: {err}`` (`:624-628`). YAML forbids tabs in indentation |
| The frontmatter body is empty or contains only blank and comment lines | `E-YAML-NOT-MAPPING` (error) | ``frontmatter must be a YAML mapping`` (`:634-637`). `---\n---\n` parses to `Null` and fails here, which the evidence states explicitly under `Y2` |
| The first significant line at column 0 is `-` or starts with `- ` | `E-YAML-NOT-MAPPING` (error) | as above: the root is a sequence |
| **The parser recognized zero top-level mapping entries and the body has at least one significant line**, unless the exception below applies | `E-YAML-NOT-MAPPING` (error) | as above: the root is a scalar |

**Step 6 corrected rows 1 and 4.**

Row 1 was ``a tab appears in the indentation of any frontmatter line``. That fires inside block
scalars, where a tab after the block's indent is ordinary content that libyaml accepts:

```yaml
description: |
  <TAB>tab-indented example line
```

The block's indentation is two spaces; the tab is content. Under the old rule a naive `^[ \t]*`
leading-run test flags it and exits 1 on a valid file, and tab-indented content inside a
`description: |` block is common. Restricting the rule to lines the parser is treating as mapping
entries removes the false positive without weakening the real case, because a tab in the indentation
of an actual mapping line is still a certain error.

Row 4 replaces ``the whole body is a **single line** with no `:` at all``, which was a **false
pass**. A body of two or more colon-free lines is a multi-line plain scalar, so `as_mapping()` at
`:633` is `None` and the indexer hard-rejects the skill — but under the old wording neither line
reduces to a `key:` form, no row matched, and the catch-all in 6.5.2 emitted `W-YAML-UNDECIDABLE`, a
**warning**: exit 0, and the skill counted in `summary.skillsIndexable`. The shape is one an author
reaches for naturally:

```
---
This skill helps with X.
It does Y.
---
```

Restating the rule as "zero recognized entries plus significant content" catches every arity, one
line or many.

**The exception, and why row 4 needs one.** If any significant line began with `{` (a flow
collection) or with `%`, `!`, `&`, `*` or `? ` (a directive, tag, anchor, alias or explicit key),
that construct can itself resolve to a mapping — `---\n{name: x}\n---` is a perfectly good mapping
that the mini-parser cannot reduce to a `key:` form. In that case emit `W-YAML-UNDECIDABLE` and not
`E-YAML-NOT-MAPPING`. Zero recognized entries alone is not proof the root is a scalar; zero
recognized entries **and** nothing that could be a mapping is.

#### 6.5.4 Duplicate keys: a hard error

Step 4 left this open and chose a warning. **Step 5 resolved it: `serde_yaml` 0.9.34 rejects
duplicate mapping keys, so this is a hard error.** The evidence chain is in Section 12.1.

- A top-level key whose resolved name was already seen produces `E-YAML-DUPLICATE-KEY` (**error**),
  naming both line numbers, and validation of that entrypoint **stops**. The indexer never gets past
  `serde_yaml::from_str` (`:621`), so no field of that skill is ever resolved and no field-level
  finding about it would be truthful.
- `indexerMessage` is
  ``Skipped skill `{folder}`: YAML parse error: duplicate entry with key "{key}"``. `serde_yaml`
  appends a position suffix of its own, so the checker's string is a prefix of the real log line
  rather than a byte-exact copy. That is the only code in the taxonomy where this is true and it is
  called out here so nobody tries to match the two exactly.
- **There is no "first occurrence wins" rule.** Deleting it is the point: the previous wording
  implied the skill still resolves fields, and it does not.
- **Scope: every top-level key, not just the three the indexer reads.** The duplicate test lives in
  `Mapping`'s deserializer (`mapping.rs:813-822`), which runs before any field lookup, so `foo: 1`
  written twice is fatal even though `foo` is otherwise ignored under `Y6`. A checker that only
  deduped `name`, `description` and `when_to_use` would miss it.

Key identity for the comparison is the **unquoted key text**, byte-exact and case-sensitive, matching
`Value::String` equality at `:449-450`. `name` and `'name'` are the same key and collide; `name` and
`Name` do not.

#### 6.5.5 Unknown keys

Ignored with no finding, matching `Y6` and `X1` exactly (`:641-694`, test `:8501`).

One exception: a key spelled `when-to-use` (hyphenated) produces `I-KEY-HYPHEN-VARIANT` (info). The
evidence calls this out specifically: a repo-wide grep for `when-to-use` returns zero matches, only
the underscore form is read (`:687`), so the hyphenated spelling is silently ignored. Surfacing
silent traps is the entire point of this tool. No other near-miss spelling is special-cased.

### 6.6 Field validation

#### 6.6.1 `name`

Order of operations, mirroring `:641-660`:

1. Key absent → resolved name is `path.basename(skillDir)`, source `folder`. **The folder fallback is
   never trimmed**: `:652` uses `folder_name.clone()` straight from the directory entry, while only
   the `name:` value goes through `str::trim` at `:453`. On a filesystem that permits it, a directory
   named `" x "` resolves to `" x "`, fails the charset test and hard-rejects the skill. Trimming the
   fallback would hide that.
2. Key present, value not a string and not `undecidable` → `E-NAME-NOT-STRING` (error). Stop; the
   indexer skips the skill. Mirrors `N2` and ``name must be a string`` (`:460` via `:644-648`).
3. Key present, value `undecidable` → `W-YAML-UNDECIDABLE` already emitted. Run the charset check on
   the literal text, source `name-undecidable`, but report any violation as **`W-NAME-UNDECIDABLE`
   (warning), never `E-NAME-INVALID`** — see 6.5.2. This entrypoint does not participate in the
   duplicate-name race of 6.6.4 either, because the checker does not know what name it resolves to.
   Do **not** fall through to step 5.
4. Key present, value is a string → trim it (`:453`) **using the explicit Unicode `White_Space` set
   in Section 5.1.2, not `String.prototype.trim()`**. If the trimmed result is empty, treat the key
   as **absent** (`Y5`, `:452-459`) and fall back to the directory name, source `folder`.
5. Charset and length: the resolved name must satisfy `^[a-z0-9-]{1,64}$`, with the length counted as
   `[...name].length`. Violation → `E-NAME-INVALID` (error). Mirrors `N3` (`:464-470`, `:653-660`).

`E-NAME-INVALID`'s message **must state the source**, because the fallback is the trap the evidence
names: a directory called `My_Skill` with no `name:` key resolves to `My_Skill`, fails the charset
test, and the skill is hard-rejected even though nothing in the file looks wrong.

- source `key`: "the `name:` key on line N".
- source `folder`: "the containing directory name, because no usable `name:` key is present. Either
  rename the directory or add a valid `name:` key."

There is **no** requirement that `name` match the directory name (`N4`, `:652`). The checker never
reports a mismatch.

#### 6.6.2 `description`

| Situation | Finding | Indexer counterpart |
| --- | --- | --- |
| Absent, or a string that trims to empty (including `description: ''`) | `W-DESC-MISSING` (warning) | `description metadata is missing; inspect SKILL.md before use.` (`:677-679`) |
| Present, value not a string | `W-DESC-NOT-STRING` (warning) | `description must be a string; inspect SKILL.md before use.` (`:683`) |
| Present, non-empty string | none | skill indexed cleanly |

The skill is still counted as valid in the summary. This asymmetry is deliberate and is the single
most important thing for an author to understand: a bad `description` is the **only** metadata
problem that keeps a skill alive.

#### 6.6.3 `when_to_use`

| Situation | Finding |
| --- | --- |
| Absent | none (`W1`) |
| Present, value not a string | `W-WHEN-NOT-STRING` (warning), mirroring `when_to_use must be a string; omitted when_to_use metadata.` (`:691`) |
| Present, a string, including one that trims to empty | none. `Y5` maps an empty trim to absent, and absent is legal |

#### 6.6.4 Duplicate names: `N5`, and the scope question

The indexer dedupes only within one `skills/` directory, because that is the only directory it ever
lists. A recursive checker may see many. **The checker's comparison scope is one `skills/`
directory**, and nothing wider.

Algorithm:

1. Group every validated candidate by `path.dirname(skillDir)`, that is, by its parent directory.
2. Discard every group whose parent directory is not named `skills` (ASCII case-insensitive).
   Outside a `skills/` directory the indexer never compares those names at all, so a clash there is
   meaningless and no finding is emitted.
3. Within a group, sort the **skill directories** by ASCII-lowercased directory name, with the
   original directory name as tiebreak. This is the exact port of `:588-591` and it is what decides
   who wins a clash. **Step 5 verified the port against the source**, which is a two-element tuple
   comparison, `(left.to_ascii_lowercase(), left).cmp(&(right.to_ascii_lowercase(), right))`. Two
   details are load-bearing and neither is the JS default:
   - `to_ascii_lowercase` maps **A-Z only**. Section 5.1.2 already bans
     `String.prototype.toLowerCase()` here; this is the comparison it was banned for.
   - Both tuple elements compare as Rust `String`, that is in **UTF-8 byte order**. Use the
     `Buffer.compare` form from Section 5.1.2, not `<` or `localeCompare`.
   Sort the directories, not the resolved names: `:588-591` runs on `folder_name` before any
   frontmatter is read.
4. Walk the sorted list. The first directory to claim a resolved name keeps it. Every later directory
   resolving to the same name gets `E-NAME-DUPLICATE` (error), naming the winner, mirroring
   ``duplicate skill name `{name}` already used by `{first}` `` (`:663-668`).

**A candidate carrying ANY hard error does not participate**, not only a name-resolution failure.
`:595-670` is a single loop in which every failure path is a `continue`, and `seen_skill_names` is
only written at `:671`, after the entrypoint check, the frontmatter extraction, the YAML parse, the
mapping check, the `name` type check and the charset check have all passed. So a directory that fails
at any earlier layer never claims its name, and a later directory resolving to the same name is
indexed cleanly with no duplicate finding at all.

Worked example, because getting this backwards produces a finding the app would never emit: in one
`skills/` directory, `aaa/SKILL.md` has a broken frontmatter and would have resolved to `shared`, and
`bbb/SKILL.md` resolves to `shared` cleanly. The indexer skips `aaa` at the frontmatter layer and
indexes `bbb` without complaint. The checker must therefore report `E-FM-*` on `aaa` and **nothing**
on `bbb`. Step 4's wording ("every validated candidate", failing only on `E-NAME-*`) would have
reported a spurious `E-NAME-DUPLICATE` on `bbb`.

Practical rule for the implementer: build the `seen` map in sorted order and insert a name only when
the candidate reached the end of field validation with zero `error`-severity findings.

If two identical `skills/` trees exist in different matrices under the same root, they are separate
groups and neither reports a duplicate. That is correct: they are never in the same index.

### 6.7 Position and indexability

Exactly one verdict per validated entrypoint. Given an entrypoint at
`<...>/<A>/<B>/<C>/SKILL.md`, where `C` is the skill directory, `B` is its parent and `A` is `B`'s
parent:

**Canonical** requires both:

1. `path.basename(B) === 'skills'`, byte-exact.
2. `path.basename(A)` matches `/^_agent_.+$/`. A single leading underscore, so `__agent_x` fails.

Canonical produces no finding. Otherwise `I-NOT-INDEXABLE` (info) with one `reason`:

| `reason` | Condition | Message |
| --- | --- | --- |
| `not-under-skills-dir` | `basename(B)` is not `skills` in any casing | Depth is fixed at one, `<owner-root>/skills/<skill-name>/SKILL.md`. This file is structurally checkable but is never indexed |
| `skills-dir-case` | `basename(B)` equals `skills` only after ASCII lowercasing | Platform-dependent. The indexer builds this path by joining the constant `skills`, so a differently cased directory may still resolve on Windows and will not on Linux. See Section 12.4 |
| `replica-root` | `basename(A)` starts with `__agent_` | A `__agent_*` replica's own `skills/` directory is never scanned. `has_agent_matrix_dir_name` tests `starts_with("_agent_")` and `"__agent_x"` fails it (`:1437-1442`) |
| `owner-root-not-recognized` | Anything else | The owner root is not an `_agent_*` matrix directory. The Root Agent directory is a third legal owner root (`root_agent.rs:633-639`) that this checker does not attempt to identify, so this note may be a false positive there. It is informational only and never affects the exit code |

Position findings are **always `info`** and **never affect the exit code**. This closes the question
the coordinator left open. The reasoning is in decision 6 of Section 4: user decision 4 makes the
checker generic over any tree, and a generic tool cannot fail a build over a shape it cannot verify
is meant to be a skills tree.

**Canonical position for a directory, not an entrypoint.** Three findings are gated on canonical
position without any `SKILL.md` to anchor the test: `I-NO-ENTRYPOINT` (6.3), `W-DIR-UNREADABLE`
(6.2 step 1) and `W-SKILL-DIR-LINK` (6.2 step 3). For a directory `D` the same test reads: `D` is a
**skill directory** in canonical position when `path.basename(path.dirname(D))` is `skills` and
`path.basename(path.dirname(path.dirname(D)))` matches `/^_agent_.+$/`; `D` is a **skills root** in
canonical position when `path.basename(D)` is `skills` and `path.basename(path.dirname(D))` matches
`/^_agent_.+$/`. Whether the `skills` comparison is byte-exact or ASCII case-insensitive depends on
the finding, per Section 4.1 rule 2. Step 4 stated this form only inside 6.2 step 1 and left 6.3
pointing at a definition written for entrypoints; it is stated here once so both callers resolve.

The `canonical` flag is what promotes every rule-2 finding to an error. **Step 7 correction:** Step 4
wrote "the two absence-type findings (`I-NO-ENTRYPOINT` and `W-DIR-UNREADABLE`)", which Step 6 left
stale when it added `W-SKILL-DIR-LINK` (G5) and split `E-SKILL-DIR-UNREADABLE` out of
`E-SKILLS-DIR-UNREADABLE` (G16). There are now three promotion routes and one gated-only code, and
`W-SKILL-DIR-LINK` is not an absence-type finding at all. The complete list is in Section 4.1 rule 2.

### 6.8 The complete finding taxonomy

Every code, its severity, its rule ID in the evidence artifact, and the indexer message it mirrors.
Exit-code column: `1` means the code forces exit 1; a dash means it does not.

**Errors (force exit 1)**

| Code | Rule | Indexer message | Exit |
| --- | --- | --- | --- |
| `E-SKILLS-DIR-UNREADABLE` | `R4`, `R6` | `` `skills` could not be inspected: {err} `` / `` `skills` directory could not be read: {err} `` | 1 |
| `E-SKILL-DIR-UNREADABLE` | `R4` | ``Skipped skill directory `{folder}`: unable to read skill directory: {err}`` | 1 |
| `E-SKILLS-NOT-DIR` | `R5` | `` `skills` exists but is not a directory: {path} `` | 1 |
| `E-SKILL-DIR-LINK` | `R8` | ``Skipped linked skill directory `{folder}`: linked/reparse-point directories are not followed`` | 1 |
| `E-NO-ENTRYPOINT` | `E1` | ``Skipped skill directory `{folder}`: missing exact SKILL.md entrypoint`` | 1 |
| `E-ENTRYPOINT-CASE` | `E1` | as above | 1 |
| `E-ENTRYPOINT-LINK` | `E2` | ``exact SKILL.md entrypoint is linked/reparse-point`` | 1 |
| `E-ENTRYPOINT-NOT-FILE` | `E3` | ``exact SKILL.md entrypoint is not a regular file`` | 1 |
| `E-ENTRYPOINT-UNREADABLE` | `E4`, `F9` | ``failed to open SKILL.md frontmatter: {err}`` / ``failed to read SKILL.md frontmatter: {err}`` | 1 |
| `E-FM-NO-OPEN` | `F1` | ``missing opening frontmatter delimiter`` | 1 |
| `E-FM-FIRST-LINE-TOO-LONG` | `F7` | ``missing opening frontmatter delimiter`` | 1 |
| `E-FM-NO-CLOSE` | `F5` | ``missing closing frontmatter delimiter`` | 1 |
| `E-FM-TOO-LARGE` | `F6` | ``frontmatter exceeds 16384 byte limit`` | 1 |
| `E-FM-NOT-UTF8` | `F8` | ``frontmatter is not valid UTF-8: {err}`` | 1 |
| `E-YAML-PARSE` | `Y1` | ``YAML parse error: {err}`` | 1 |
| `E-YAML-DUPLICATE-KEY` | `Y1` | ``YAML parse error: duplicate entry with key "{key}"`` (position suffix appended by `serde_yaml`) | 1 |
| `E-YAML-NOT-MAPPING` | `Y2` | ``frontmatter must be a YAML mapping`` | 1 |
| `E-NAME-NOT-STRING` | `N2` | ``name must be a string`` | 1 |
| `E-NAME-INVALID` | `N3` | ``invalid skill name `{name}`; expected 1-64 lowercase ASCII letters, digits, or hyphens`` | 1 |
| `E-NAME-DUPLICATE` | `N5` | ``duplicate skill name `{name}` already used by `{first}` `` | 1 |

**Warnings (never change the exit code)**

| Code | Rule | Indexer counterpart |
| --- | --- | --- |
| `W-DESC-MISSING` | `D1` | `description metadata is missing; inspect SKILL.md before use.` |
| `W-DESC-NOT-STRING` | `D2` | `description must be a string; inspect SKILL.md before use.` |
| `W-WHEN-NOT-STRING` | `W2` | `when_to_use must be a string; omitted when_to_use metadata.` |
| `W-YAML-UNDECIDABLE` | none | none. Checker-only: the mini-parser refuses to judge |
| `W-NAME-UNDECIDABLE` | `N3` | possibly ``invalid skill name `{name}` …`` or possibly ``name must be a string``, or possibly nothing at all. The checker states all three and asserts none (6.5.2) |
| `W-ENTRYPOINT-CASE-SHADOWED` | none | none. The working `SKILL.md` exists; the stray variant is a hazard |
| `W-SKILL-DIR-LINK` | `R8` outside canonical position | none. The indexer never scans that tree (6.2 step 3) |
| `W-DIR-UNREADABLE` | `R4`, `R6` outside canonical position | none |

**Informational (never change the exit code)**

| Code | Rule | Meaning |
| --- | --- | --- |
| `I-NOT-INDEXABLE` | `R1`, `R3`, `R7` | Structurally checkable, never indexable. Carries `reason` (Section 6.7) |
| `I-NO-ENTRYPOINT` | `E1` outside canonical position | A directory under some `skills/` folder with no entrypoint in any casing |
| `I-KEY-HYPHEN-VARIANT` | `X1` | A `when-to-use` key, which the indexer never reads |

**Indexer conditions with no exact one-to-one code**, recorded so the mapping is complete rather than
silently partial. Step 5 re-enumerated the indexer's hard errors directly from `:311-418` and
`:489-670` and found **five**, not four:

- `Skipped a skills directory entry: {err}` (`:558-561`) and
  ``unable to read skill directory entry: {err}`` (`:400`) are per-`DirEntry` iteration failures.
  Node's `readdirSync(dir, { withFileTypes: true })` throws for the whole call rather than per entry,
  so both surface as `E-SKILLS-DIR-UNREADABLE` or `W-DIR-UNREADABLE` on the parent directory.
- ``could not inspect entry type: {err}`` (`:569-573`) is `entry.file_type()` failing on a skill
  directory. Node resolves the `Dirent` type inside `readdirSync`, so there is no separate per-entry
  call that can fail; the same two codes cover it.
- **``could not inspect exact SKILL.md entrypoint: {err}`` (`:405-407`)** is the same failure on the
  `SKILL.md` entry itself, and it is unmappable for the same reason. **This condition was missing
  from the evidence artifact's hard-error table and therefore from Step 4's count.** See
  Section 14.4.
- ``unable to read skill directory: {err}`` (`:396-397`) is `read_dir` on a skill directory failing.
  This one **is** mappable and is now mapped: Section 6.2 step 1 emits `E-SKILL-DIR-UNREADABLE` for
  a skill directory in canonical position. It is listed here only to record that it left this bucket.
  **Step 6 gave it its own code.** Step 5 routed it to `E-SKILLS-DIR-UNREADABLE`, which carries the
  two `` `skills` ``-root message variants. Since 6.10 promises that `indexerMessage` "reproduces the
  exact string that would appear in the startup log", one code covering two structurally different
  indexer errors guarantees the wrong string for one of them, and leaves a JSON consumer unable to
  tell a broken skills root from a broken single skill.

Corrected count: the indexer has **23** hard-error conditions, not 22, and 3 soft warnings. After
Step 5 added `E-YAML-DUPLICATE-KEY` and removed `W-YAML-DUPLICATE-KEY`, and Step 6 added
`E-SKILL-DIR-UNREADABLE`, `W-NAME-UNDECIDABLE` and `W-SKILL-DIR-LINK`, the taxonomy is **20 error
codes, 8 warning codes and 3 informational codes, 31 in total**. It accounts for all 23 hard errors
and all 3 soft warnings; the three codes Step 6 added are severity- and message-fidelity splits of
conditions that were already covered, not new indexer conditions.

### 6.9 Human report format

Written entirely to stdout.

```
[skills-check] root: C:\Users\x\0_repos\Project\.ac
[skills-check] scanned 412 directories, found 7 SKILL.md entrypoints

ERROR  _agent_architect/skills/my_skill/SKILL.md:2  E-NAME-INVALID  (rule N3)
       Resolved name `my_skill` is not valid; expected 1-64 characters from [a-z0-9-].
       Source: the `name:` key on line 2.
       Indexer: Skipped skill `my_skill`: invalid skill name `my_skill`; expected 1-64 lowercase
       ASCII letters, digits, or hyphens

ERROR  _agent_writer/skills/notes/skill.md  E-ENTRYPOINT-CASE  (rule E1)
       Entrypoint is named `skill.md`; the indexer compares byte-exactly against `SKILL.md` and
       rejects this on every platform, Windows included.
       Indexer: Skipped skill directory `notes`: missing exact SKILL.md entrypoint

WARN   _agent_writer/skills/notes/skill.md:3  W-DESC-MISSING  (rule D1)
       `description` is missing or empty. The skill is still indexed, and flagged at startup.

NOTE   docs/examples/SKILL.md  I-NOT-INDEXABLE  (reason: not-under-skills-dir)
       Depth is fixed at one: <owner-root>/skills/<skill-name>/SKILL.md. This file is structurally
       checkable but is never indexed.

[skills-check] 2 errors, 1 warning, 1 note across 7 entrypoints
[skills-check] FAIL: 2 skills would not be indexed
```

Rules for the format:

- Paths in findings are relative to the root, with `/` separators on every platform. The absolute
  root is printed once, in the header.
- `:N` is appended when the finding is anchored to a line in `SKILL.md`, and omitted otherwise.
- Severity labels are the fixed-width tokens `ERROR`, `WARN`, `NOTE`.
- The `Indexer:` line is printed only when the finding has an indexer counterpart, and reproduces the
  exact string that would appear in the startup log. This is what makes a startup warning searchable
  back to a checker finding.
- The final line is `[skills-check] OK: N skills would be indexed` on exit 0 and
  `[skills-check] FAIL: N skills would not be indexed` on exit 1. `N` counts distinct entrypoints
  carrying at least one error, not the number of error findings.
  **Step 6 exception, because that rule can print `FAIL: 0`.** Five error codes are not attached to
  any entrypoint at all: `E-SKILLS-DIR-UNREADABLE`, `E-SKILL-DIR-UNREADABLE`, `E-SKILLS-NOT-DIR`,
  `E-SKILL-DIR-LINK` and `E-NO-ENTRYPOINT`. A tree whose only fault is a `skills` file where a
  directory belongs exits 1 with zero entrypoints implicated, and the report would read
  `FAIL: 0 skills would not be indexed`, which flatly contradicts the exit code. So: when `N` is 0
  and error findings exist, print `[skills-check] FAIL: M error findings, no entrypoint implicated`
  where `M` is the error count. When both are non-zero, print
  `[skills-check] FAIL: N skills would not be indexed, and M error findings in total`.
- With zero entrypoints found the report is the two header lines plus
  `[skills-check] OK: no SKILL.md entrypoints found`, exit 0. An empty tree is not a failure.

### 6.10 JSON output format

`--json` writes exactly one JSON document to stdout, `JSON.stringify(report, null, 2)` followed by a
single `\n`, and writes nothing else to stdout.

```json
{
  "tool": "01-skills-checker",
  "version": 1,
  "sourceOfTruth": "src-tauri/src/config/session_context.rs:206-714",
  "root": "C:\\Users\\x\\0_repos\\Project\\.ac",
  "summary": {
    "directoriesScanned": 412,
    "entrypointsFound": 7,
    "skillsIndexable": 5,
    "errors": 2,
    "warnings": 1,
    "infos": 1
  },
  "findings": [
    {
      "code": "E-NAME-INVALID",
      "severity": "error",
      "rule": "N3",
      "path": "_agent_architect/skills/my_skill/SKILL.md",
      "absolutePath": "C:\\Users\\x\\0_repos\\Project\\.ac\\_agent_architect\\skills\\my_skill\\SKILL.md",
      "skillDirectory": "_agent_architect/skills/my_skill",
      "skillName": "my_skill",
      "line": 2,
      "reason": null,
      "message": "Resolved name `my_skill` is not valid; expected 1-64 characters from [a-z0-9-]. Source: the `name:` key on line 2.",
      "indexerMessage": "Skipped skill `my_skill`: invalid skill name `my_skill`; expected 1-64 lowercase ASCII letters, digits, or hyphens"
    }
  ],
  "exitCode": 1
}
```

Field contract, fixed:

| Field | Type | Notes |
| --- | --- | --- |
| `tool` | string | Always `"01-skills-checker"` |
| `version` | number | Output-format version. `1` for this implementation. Bump on any breaking field change |
| `sourceOfTruth` | string | Constant. Makes the drift risk visible to any consumer |
| `root` | string | Absolute, `path.resolve()`d, native separators |
| `summary.entrypointsFound` | number | **Every** discovered `skill.md` match in any casing, including a shadowed casing variant that was never validated (6.3 row 5). Step 6 wrote this row: 14.5.1 fixed the definition and the fix was never applied here |
| `summary.skillsIndexable` | number | Entrypoints that reached the end of field validation with zero error findings. Warnings do not disqualify, mirroring the indexer. **A shadowed casing variant is excluded**: it carries only `W-ENTRYPOINT-CASE-SHADOWED`, so the Step 4/5 wording "entrypoints with zero error findings" counted one directory as two indexable skills and `summary` could not reconcile with `findings` |
| `findings` | array | Always present, possibly empty |
| `findings[].code` | string | From Section 6.8 |
| `findings[].severity` | `"error"` &#124; `"warning"` &#124; `"info"` | |
| `findings[].rule` | string &#124; null | Evidence-artifact rule ID, or `null` for checker-only findings |
| `findings[].path` | string | Relative to `root`, always `/`-separated. This is the stable key |
| `findings[].absolutePath` | string | Native separators |
| `findings[].skillDirectory` | string &#124; null | Relative, `/`-separated. `null` when the finding is not about a skill directory |
| `findings[].skillName` | string &#124; null | The resolved name, or `null` when resolution did not complete |
| `findings[].line` | number &#124; null | 1-based line in `SKILL.md`, `null` when not line-anchored |
| `findings[].reason` | string &#124; null | Only populated for `I-NOT-INDEXABLE` (Section 6.7) |
| `findings[].message` | string | Human sentence. Not a stable contract; consumers key on `code` |
| `findings[].indexerMessage` | string &#124; null | The exact indexer string, or `null` |
| `exitCode` | number | Duplicates the process exit code so a consumer reading only stdout knows the verdict |

**Ordering is deterministic**, so the output is snapshot-comparable: sort by `path` (raw string,
code-unit order), then `line` ascending with `null` treated as `0`, then `code` ascending, then
**emission order** as the final tiebreak. The last key is required and Step 6 moved it here from
14.5.2, where it was recorded and never applied: `path`/`line`/`code` still ties for two
`W-YAML-UNDECIDABLE` findings reported against the same line, and without a total order
`Array.prototype.sort` gives no guarantee beyond stability, so self-test case 71 does not actually
pin what it claims to. Note this sort is presentation-only and is deliberately **not** the
Rust-fidelity comparison of 5.1.2: nothing in the indexer depends on it, so plain code-unit order is
correct here.

**What is and is not byte-identical across platforms. Step 6 corrected this claim.** Step 4/5 said
`path` uses `/` on every platform "specifically so a fixture snapshot is byte-identical on Windows,
Linux and macOS". The separator normalization is real, but the guarantee as stated is false: the same
document also carries `root` and `findings[].absolutePath`, both defined two rows above as **native
separators** and both absolute, plus `summary.directoriesScanned`, plus every `indexerMessage` whose
template embeds `{err}`, where the errno text is OS-specific. Self-test case 71 only asserts that two
runs on one machine agree, so nothing tests the cross-platform claim at all. The guarantee is
therefore scoped to exactly two fields: **`findings[].path` and `findings[].skillDirectory` are
`/`-separated and root-relative on every platform, and a snapshot keyed on `code` plus those two
fields is portable.** Any consumer wanting a portable snapshot of the whole document must exclude
`root`, `absolutePath`, `summary.directoriesScanned` and `indexerMessage`.

### 6.11 Windows and POSIX specifics

- Junctions and reparse points: `Dirent.isSymbolicLink()` returns true for them on Windows, so the
  symlink rules in 6.2 and 6.3 cover both platforms with one code path.
- Case sensitivity: every validity comparison uses `===` on the string `readdirSync` returned.
  `fs.existsSync(path.join(dir, 'SKILL.md'))` is forbidden anywhere in the file.
- Separators: comparisons go through `path.basename` and `path.dirname`; only the report normalizes
  `\` to `/`.
- Long paths: not special-cased. A `readdirSync` failure from a path-length limit becomes
  `W-DIR-UNREADABLE`, which is reported and does not abort the run.
- The script is never made executable-bit dependent: it is always invoked as `node scripts/...`.

## 7. Compatibility and security

- **IPC, persistence, config.** None touched. No Tauri command, event, type or TOML shape changes.
- **Runtime coupling.** The script is not imported by the app and does not run at app runtime. It
  cannot affect a session.
- **Dependencies.** Zero added. `package-lock.json` unchanged, so `lockfile-check.yml` is unaffected.
- **Filesystem writes.** The checker only ever writes inside its own `mkdtemp` directory during
  `--self-test`, and removes it in a `finally` (Section 9.2). In normal operation it opens files
  read-only and writes nothing.
- **Symlink safety.** Never following a symlinked directory means the walk cannot be redirected out
  of the given root by a crafted link, and cannot be trapped in a cycle.
- **Untrusted content.** `SKILL.md` files are untrusted input. Three consequences, all required:
  - Reads are bounded: at most 1024 bytes for the first line and 16384 bytes of frontmatter body,
    with a hard stop at the closing delimiter. A multi-gigabyte `SKILL.md` costs a bounded read.
  - No content is ever `eval`'d, imported, or passed to a shell. The mini-parser is pure string work.
  - Report text embeds file content (names, values). **Step 6 corrected the mitigation, which did
    not achieve its stated goal.** Step 4/5 said values are "printed as-is inside backticks and
    truncated to 200 characters ... so a long or **line-broken** value cannot scramble the report".
    Truncation bounds length and does nothing whatever about a `\n` at offset 10, or about a `\r`, a
    `\x1b` escape introducer, or any other C0 control character — all of which are legal inside a
    YAML double-quoted scalar or a folder name. So the required order is: first **collapse** every
    run of C0 controls and Unicode whitespace to a single U+0020 and drop the rest, then truncate to
    200 characters. That is exactly what the indexer does before rendering the same strings
    (`sanitize_skill_metadata_for_context`, `:420-446`, which ends in a `trim()`), so mirroring it
    costs nothing and keeps the two outputs comparable. Both steps are presentation only and never
    affect a verdict; in particular the charset test in 6.6.1 runs on the raw value, never on the
    sanitized one.
- **Rollback.** Deleting the script and reverting the two `package.json` lines restores the previous
  state exactly. Nothing else has to be undone.

## 8. Implementation order

0. **Commit this plan first, as its own commit.** `plans/` is gitignored (`.gitignore:11`), so it
   needs `git add -f plans/1213-skills-checker.md`; a plain `git add` silently adds nothing and the
   plan never reaches the branch.
1. Create `scripts/01-skills-checker.mjs` with the header (5.1.1), the constants, `parseArgs`,
   `printUsage` and a `main()` that resolves the root and exits 2 on every case in Section 6.1.
   Verifiable on its own: `--help`, a missing root, and a root that is a file.
2. Add `walk()` (6.2) plus entrypoint discovery (6.3), reporting only `E-ENTRYPOINT-CASE`,
   `E-NO-ENTRYPOINT`/`I-NO-ENTRYPOINT` and the symlink codes. At this point the script already
   catches the `tiktok-news-publisher` bug from the issue.
3. Add `isFrontmatterDelimiter` and `readFrontmatter` (6.4). All `E-FM-*` codes.
4. Add `parseMiniYaml` (6.5). `E-YAML-*`, `W-YAML-*`.
5. Add `validateFields` (6.6.1 to 6.6.3), `detectDuplicates` (6.6.4) and `classifyPosition` (6.7).
6. Add `renderHuman` (6.9) and `renderJson` (6.10), and wire the exit code.
7. Add `selfTest` (Section 9).
8. Add the two `package.json` entries (5.2).
9. Run the gates in Section 9.3.
10. Commit, then **push before doing anything that needs Codebase Memory**. See the note below.

Steps 1 to 8 are one commit. Splitting them would put a half-implemented checker on the branch, and a
checker that reports fewer rules than it appears to is worse than none: an author would read a clean
run as proof the skill is valid.

### 8.1 Codebase Memory after a commit: the order is commit, push, re-index, gate

**Step 7 moved this here from Section 15.2, where the implementer would not have looked for it.** It
is an operational instruction about the tooling the implementer must use, so it belongs in the
implementation order, not in a review record.

The gate asserts `base_sha == HEAD` (`cbm.ps1:2317`), and `base_sha` derives from upstream resolution.
Under either reading of that derivation a local commit that has not been pushed leaves `base_sha`
behind HEAD, so the gate throws `Codebase Memory Git state is stale`. The full argument is in
Section 15.2.

So, whenever a commit lands on this branch:

```
git commit  →  git push  →  re-index  →  gate
```

**A gate failure between the commit and the push is expected and is not a regression.** Do not
re-diagnose it, do not change the refspec, and do not change branch state. The refspec was already
repaired once for this branch (`git remote set-branches --add`, Section 15.1); the branch's fetch
refspec is not the cause of this particular failure and touching it again will not help.

This applies to step 0 as well: the plan commit is a commit like any other.

## 9. Tests and acceptance criteria

### 9.1 Where the tests live, and why not vitest

Tests live inside `scripts/01-skills-checker.mjs` behind `--self-test`, wired as
`npm run check:skills:self`. Fixtures are created at runtime under
`fs.mkdtempSync(path.join(os.tmpdir(), 'ac-skills-check-'))` and removed in a `finally`, exactly as
`scripts/check-test-debt.mjs:703` and `:855-857` do. **No fixture file is committed.**

Four reasons, all decisive:

1. **Precedent.** `check-test-debt.mjs` is the repo's existing answer to "how do you test a `.mjs`
   script", and it is already wired as `test:debt:self`. This follows it rather than inventing a
   second pattern.
2. **The script's own guarantee.** User decision 5 requires the script to run in a fresh checkout
   with no `npm install`. A vitest test could only be run after `npm install`, so the thing verifying
   the portability guarantee would itself depend on what the guarantee excludes.
3. **Blast radius.** `vitest.config.ts:12` collects `src/**/*.test.ts(x)` only. Collecting a test
   under `scripts/` means editing shared test configuration for one file. Section 5.3 keeps that file
   untouched.
4. **Fixtures that cannot be committed.** Several required fixtures do not survive a git checkout
   portably: a wrongly cased `skill.md` beside `SKILL.md`, a symlinked `SKILL.md`, invalid UTF-8
   bytes, a 16 KiB frontmatter, a file with no trailing newline, and a leading BOM. Generating them
   at runtime is the only portable way to test the rules that matter most.

### 9.2 The self-test

`selfTest()` returns `0` when every case passes and `1` on the first failure, printing the failing
case name. It uses an `assertSelf(condition, message)` helper, mirroring `check-test-debt.mjs`.

Each case builds a fixture tree, runs the checker's own pipeline in-process against it, and asserts
on the resulting finding codes. Cases that cannot be constructed on the running platform wrap
creation in `try`/`catch` and skip with a printed `SKIP` line naming the reason. A skip is not a
failure; the case list prints how many were skipped and which.

#### 9.2.1 Which cases skip, and how to stop most of them skipping

**Step 6 wrote this subsection.** Step 4/5 named exactly one skippable case (11) and 14.5.4 flagged
it as the one that matters. That undercounts, and one of the omissions is not a skip at all but a
hard failure on the team's own platform.

| Case | Needs | Without the fix |
| --- | --- | --- |
| 11, 12, 63e, 63f | a **directory** link | skips on unprivileged Windows without Developer Mode |
| 19 | a **file** symlink to `SKILL.md` | skips on unprivileged Windows without Developer Mode |
| 20 | `SKILL.md` **and** `skill.md` in one directory | **FAILS**, not skips, on any case-insensitive filesystem |
| 63c, 63g | a directory whose listing throws | not reliably constructible as the owning user |

**Case 20 is the serious one.** 6.3 row 5 says two matches are "only reachable on a case-sensitive
filesystem", and default Windows and default macOS are not. The second `writeFileSync` overwrites the
first, the directory ends up with one entry, and the assertion for `W-ENTRYPOINT-CASE-SHADOWED` fails.
Since it was never listed as skippable, `npm run check:skills:self` exits 1 on the primary
development platform, which breaks acceptance criterion 2 outright. Case 20 must probe first — create
`SKILL.md`, then `skill.md`, then re-list the directory and skip unless two entries came back — and
print `SKIP (case-insensitive filesystem)`.

**Cases 63c, 63g and 79** must skip the same way: attempt the permission change, re-list, and skip
unless the listing actually throws. `icacls` denials do not bind the owner on Windows and `chmod 000`
does not bind root on Linux. All three share one probe, so implement the probe once and let the three
cases consume its result.

**The four directory-link cases should not skip at all.** On win32, create directory links with

```js
fs.symlinkSync(path.resolve(target), linkPath, 'junction')
```

A junction requires **no** Developer Mode and no elevation, works for directories, and Node reports
it through `Dirent.isSymbolicLink()`. That converts cases 11, 12, 63f and 63e (create the junction,
then delete its target) from skips into real assertions on ordinary Windows — and doing so is
precisely the evidence 14.5.4 says is missing, since it exercises the junction behaviour the plan
leans on in three places rather than assuming it. Only case 19 still needs a file symlink and
therefore still skips.

Without this, `E-SKILL-DIR-LINK`, `W-SKILL-DIR-LINK`, `E-ENTRYPOINT-LINK`, `E-SKILLS-NOT-DIR` and
`W-ENTRYPOINT-CASE-SHADOWED` are all unverified on the platform the team develops on — five codes,
four of them errors, silently untested behind a green run.

#### 9.2.2 Codes with no case at all

Acceptance criterion 12 requires every code in 6.8 to be emitted by at least one case. Three codes had
none: `E-ENTRYPOINT-UNREADABLE` (nothing makes `openSync` or `readSync` throw), `W-DIR-UNREADABLE`
(63c covers only the canonical error variant), and the `skills`-root variant of
`E-SKILLS-DIR-UNREADABLE`.

**Step 7 correction.** Step 6 wrote that "cases 76-79 below close three of them". They do not. Case 79
closes the `E-SKILLS-DIR-UNREADABLE` skills-root variant; cases 76, 77 and 78 close the FAIL-line
wording, the `entrypointsFound`/`skillsIndexable` split and the ordering tiebreak respectively, none
of which is an unreadable-directory code. `W-DIR-UNREADABLE` was left with no case at all, so
criterion 12 remained unsatisfiable with a single named exemption. **Case 63g now closes it.**
`E-ENTRYPOINT-UNREADABLE` shares case 63c's constructibility problem and stays an explicit exemption
in criterion 12.

Full accounting, so this is checkable rather than asserted: `E-SKILLS-DIR-UNREADABLE` is case 79,
`E-SKILL-DIR-UNREADABLE` is case 63c, `W-DIR-UNREADABLE` is case 63g, and `E-ENTRYPOINT-UNREADABLE`
is exempt. Every other code in 6.8 has at least one case in the tables below; criterion 12's grep is
what verifies that claim rather than this sentence.

Required cases, by section:

**CLI (6.1)**

| # | Case | Expect |
| --- | --- | --- |
| 1 | `--help` | usage on stdout, exit 0 |
| 2 | Nonexistent root | exit 2, message names the path |
| 3 | Root is a file | exit 2 |
| 4 | Two positionals | exit 2 |
| 5 | Unknown flag | exit 2 |
| 6 | `--` followed by a root starting with `-` | treated as the root, not a flag |
| 7 | Empty directory as root | exit 0, `entrypointsFound: 0` |

**Traversal (6.2)**

| # | Case | Expect |
| --- | --- | --- |
| 8 | A `SKILL.md` inside `node_modules/`, `.git/` and `target/` | not discovered; `entrypointsFound: 0` |
| 9 | Same, in a directory named `Node_Modules` | not discovered (case-insensitive skip) |
| 10 | A valid skill nested five levels below the root | discovered (no depth limit) |
| 11 | A linked directory under `_agent_x/skills/` (junction on win32, see 9.2.1) | `E-SKILL-DIR-LINK`, walk does not descend |
| 11a | A linked directory under `docs/skills/`, that is a `skills/` folder **outside** canonical position | `W-SKILL-DIR-LINK`, **exit 0**. Step 6 added this. Under Step 4/5 this was an unconditional `E-SKILL-DIR-LINK` and any repository containing a `docs/skills/` link exited 1, which also breaks acceptance criterion 4 |
| 11b | A link under `_agent_x/skills/` whose target is a **file**, and one whose target is **missing** | `E-SKILL-DIR-LINK` in both cases. `:578` tests only `is_symlink()` and never inspects the target |
| 12 | A linked directory elsewhere (not under any `skills/`) | no finding, not descended |
| 12a | `_agent_x/skills/target/SKILL.md` with a broken frontmatter, and the same under `node_modules` and `.git` names | `E-FM-*` reported for all three. Step 6 added this. `SKIP_DIRS` must not apply inside a `skills/` directory: `target` matches `^[a-z0-9-]{1,64}$` and is a legal skill name, and under Step 4/5 the checker skipped the directory and reported **nothing** while the indexer rejects it |
| 12b | A directory link **directly named** `skills` under `_agent_x`, resolving to a real directory | `E-SKILLS-NOT-DIR` (6.2 step 4), and **not** `E-SKILL-DIR-LINK`: the symlink branch of step 3 must not claim it first |

**Entrypoint (6.3)**

| # | Case | Expect |
| --- | --- | --- |
| 13 | `skills/x/SKILL.md`, valid | no error, `skillsIndexable: 1` |
| 14 | `skills/x/skill.md`, otherwise valid | `E-ENTRYPOINT-CASE`, exit 1 |
| 15 | `skills/x/Skill.md` with a broken name too | both `E-ENTRYPOINT-CASE` and `E-NAME-INVALID`, proving validation continues after a casing error |
| 16 | `skills/x/` with no entrypoint at all, in canonical position | `E-NO-ENTRYPOINT` |
| 17 | Same, outside canonical position | `I-NO-ENTRYPOINT`, exit 0 |
| 18 | `SKILL.md` is a directory | `E-ENTRYPOINT-NOT-FILE` |
| 19 | `SKILL.md` is a symlink | `E-ENTRYPOINT-LINK` |
| 20 | Both `SKILL.md` and `skill.md` present | `W-ENTRYPOINT-CASE-SHADOWED`, exit 0 |

**Frontmatter (6.4)**

| # | Case | Expect |
| --- | --- | --- |
| 21 | No frontmatter at all | `E-FM-NO-OPEN` |
| 22 | One blank line before `---` | `E-FM-NO-OPEN` (`F1`) |
| 23 | CRLF on both delimiters | valid |
| 24 | BOM on the opening delimiter | valid (`F2`) |
| 25 | BOM on the closing delimiter | `E-FM-NO-CLOSE` (`F5`) |
| 26 | `  ---  ` with surrounding spaces | valid (`F4`) |
| 27 | `----` as the opening fence | `E-FM-NO-OPEN` |
| 28 | `+++` as the opening fence | `E-FM-NO-OPEN` |
| 29 | File is exactly `---` with no trailing newline | `E-FM-NO-CLOSE`, not `E-FM-NO-OPEN` (`:374-386`) |
| 29a | `---\nname: ok-name\n---` with **no trailing newline**, so the closing delimiter is the unterminated final line | **valid, zero findings, exit 0** (`:382-383`). Step 6 added this. It is the shape every editor without "insert final newline" produces, and the obvious implementations (`split('\n')`, or "a line is complete only when a `\n` is seen") report `E-FM-NO-CLOSE` on it. Without this case that false positive ships |
| 29b | `---\nname: ok-name\ndescription: x` with no trailing newline, so the unterminated final line is a **body** line | `E-FM-NO-CLOSE` (`:385` appends, then `:389` returns). Pins the other half of the EOF branch |
| 29c | Same as 29b but the unterminated final body line is large enough to overflow the 16384 budget | `E-FM-TOO-LARGE`, not `E-FM-NO-CLOSE`. Pins that the append at `:385` runs before the ``missing closing`` return at `:389` |
| 29d | Zero-byte file | `E-FM-NO-OPEN` (`:390-392`, `current_line` empty so the `:374` branch is skipped) |
| 30 | Opening `---` present, EOF before a closing one | `E-FM-NO-CLOSE` |
| 31 | Frontmatter body of 16385 bytes | `E-FM-TOO-LARGE` |
| 32 | Frontmatter body of 16000 bytes | valid |
| 33 | Invalid UTF-8 byte inside the frontmatter | `E-FM-NOT-UTF8` |
| 34 | Invalid UTF-8 bytes only in the body, after the closing delimiter | valid; the body is never read |
| 35 | First line of 2000 bytes with no newline, file longer | `E-FM-FIRST-LINE-TOO-LONG` |
| 35a | Opening line of 1020 spaces then `---\n`, exactly 1024 bytes | **valid**. Pins the inclusive side of the 1024 boundary (Section 12.2) |
| 35b | Opening line of 1021 spaces then `---\n`, exactly 1025 bytes | `E-FM-FIRST-LINE-TOO-LONG`. Pins the exclusive side, and proves the terminator is counted |
| 35c | Valid frontmatter whose **closing** `---` is padded with enough leading spaces to exceed `(16384 - body) + 8` | `E-FM-TOO-LARGE`, not a successful close. Pins Guard A (Section 12.3). Without Guard A the checker passes a file the indexer rejects |
| 35d | Frontmatter using `\r\n` throughout, sized so the body is 16384 bytes counting both terminator bytes per line | valid; adding one more line tips it to `E-FM-TOO-LARGE`. Pins that `\r\n` costs two bytes |
| 35e | A file whose only line endings are bare `\r` | `E-FM-NO-OPEN`. Classic-Mac endings are not supported (6.4 step 1) |
| 35f | Opening delimiter line padded with `\x0B` (vertical tab) around `---` | `E-FM-NO-OPEN`. `\x0B` is not ASCII whitespace in Rust, so a JS `\s` or `trim()` port would wrongly accept it (6.4.1) |

**YAML (6.5)**

| # | Case | Expect |
| --- | --- | --- |
| 36 | `---\n---\n`, empty frontmatter | `E-YAML-NOT-MAPPING` (`Y2`) |
| 37 | Root is a sequence (`- a`) | `E-YAML-NOT-MAPPING` |
| 38 | A tab in a line's indentation | `E-YAML-PARSE` |
| 39 | Single- and double-quoted values | parsed as strings |
| 40 | Literal (`\|`) and folded (`>`) block scalars for `description` | parsed as strings, no finding |
| 41 | `name: 42`, `name: true`, `name: null`, `name:` with no value | each `E-NAME-NOT-STRING` |
| 42 | `description: yes`, and `description: on` | **no finding**, exit 0, both are plain strings under the YAML 1.2 core schema (Section 12.5) |
| 42a | Body of two colon-free lines, e.g. `This skill helps with X.` then `It does Y.` | `E-YAML-NOT-MAPPING`, **exit 1**. Step 6 added this. The root is a multi-line plain scalar and the indexer hard-rejects it at `:633`; the Step 4/5 rule only caught the single-line form, so this exited 0 as a warning (6.5.3 row 4) |
| 42b | Body is the single line `{name: ok-name}` (a flow mapping at the root) | `W-YAML-UNDECIDABLE`, exit 0. Pins the exception to row 4: zero recognized entries is not proof the root is a scalar |
| 42c | `name: "ok-name"  # renamed`, and `name:<TAB>ok-name` | **each valid, zero findings, exit 0** (6.5.1a). Pins the separator and trailing-comment grammar |
| 42d | `description: \|` whose block content is indented two spaces and whose lines begin with a tab **after** that indent | **valid, zero findings**. Pins that a tab inside block-scalar content is not `E-YAML-PARSE` (6.5.3 row 1) |
| 42e | `description: \|` whose block content includes the line `name: shadowed`, with a real `name` key elsewhere | **no `E-YAML-DUPLICATE-KEY`**: the block consumes it as content (6.5.1a) |
| 42f | `name: !!str ok-name` (a tag), and `name: &a ok-name` (an anchor) | `W-YAML-UNDECIDABLE` plus `W-NAME-UNDECIDABLE`, **exit 0**. Step 6 added this. Both resolve to the valid name `ok-name` and the indexer indexes them cleanly; under Step 4/5 the charset check on the literal text emitted `E-NAME-INVALID` and exited 1 (6.5.2) |
| 43 | Nested mapping under a key | `W-YAML-UNDECIDABLE` |
| 44 | Flow mapping `{a: 1}` as a value | `W-YAML-UNDECIDABLE` |
| 45 | An anchor, an alias and a tag | each `W-YAML-UNDECIDABLE` |
| 46 | `name` given twice | `E-YAML-DUPLICATE-KEY`, **exit 1**, no field-level finding emitted for that entrypoint (Section 12.1) |
| 46b | An **ignored** key such as `foo` given twice | `E-YAML-DUPLICATE-KEY`, exit 1. Duplicate detection covers every top-level key, not just the three the indexer reads |
| 46c | `name` once and `'name'` once (quoted) | `E-YAML-DUPLICATE-KEY`: same key after unquoting |
| 46d | `name` and `Name` | no duplicate finding: key matching is case-sensitive (`:449-450`) |
| 47 | Unknown key `foo: bar` | no finding (`Y6`) |
| 48 | `when-to-use` key | `I-KEY-HYPHEN-VARIANT`, exit 0 |
| 49 | Comment-only and blank lines mixed in | ignored |

**Fields (6.6)**

| # | Case | Expect |
| --- | --- | --- |
| 50 | No `name`, directory `my-skill` | valid, resolved name `my-skill` (`N1`) |
| 51 | No `name`, directory `My_Skill` | `E-NAME-INVALID`, message names the folder fallback as the source (the `N3` trap) |
| 52 | `name: ''` in directory `ok-name` | falls back to `ok-name`, valid (`Y5`) |
| 53 | `name` differing from the directory name, both valid | no finding (`N4`) |
| 54 | 64-character valid name | valid |
| 55 | 65-character name | `E-NAME-INVALID` |
| 56 | `name: Café` | `E-NAME-INVALID`, and the length must be counted in scalar values |
| 57 | Two directories in one `skills/` resolving to the same name | `E-NAME-DUPLICATE` on the second in sort order, and the message names the winner |
| 58 | Same clash across two different `skills/` directories | no finding (6.6.4 step 2) |
| 59 | `description` absent | `W-DESC-MISSING`, exit 0, counted in `skillsIndexable` |
| 60 | `description: ''` | `W-DESC-MISSING` (`D1`) |
| 61 | `description: 42` | `W-DESC-NOT-STRING`, exit 0 |
| 62 | `when_to_use: 42` | `W-WHEN-NOT-STRING`, exit 0 |
| 63 | `when_to_use` absent | no finding |
| 63a | `name: "﻿ok-name"` (a leading U+FEFF inside the quoted value) | `E-NAME-INVALID`. `String.prototype.trim()` would strip the U+FEFF and wrongly pass it; Rust `str::trim` does not (Section 5.1.2) |
| 63b | `name: "ok-name"` (a leading U+0085 NEL) | **valid**, resolved name `ok-name`. Rust trims U+0085 and JS `trim()` does not, so a naive port fails this one |
| 63c | A skill directory in canonical position whose listing throws (permissions, or a path-length failure) | `E-SKILL-DIR-UNREADABLE`, exit 1, not `W-DIR-UNREADABLE` (6.2 step 1). **Step 7 corrected the code**: this row still named `E-SKILLS-DIR-UNREADABLE`, which G16 split away from it. Case 79 already refers to "case 63c's `E-SKILL-DIR-UNREADABLE`", so the two rows contradicted each other and an implementer writing the test from this row would have asserted the wrong code against a correct implementation |
| 63g | The **same** fixture as 63c but placed outside canonical position, for example `docs/skills/x/` | `W-DIR-UNREADABLE`, **exit 0**. **Step 7 added this case.** 9.2.2 stated that `W-DIR-UNREADABLE` had no case and that cases 76-79 closed it; none of them does, so acceptance criterion 12 was still unsatisfiable for that code with only one named exemption. It also pins the rule-2 gate from the warning side, which nothing else does. Skips exactly like 63c and for the same reason |
| 63d | In one `skills/`, dir `aaa` with broken frontmatter that would resolve to `shared`, and dir `bbb` resolving to `shared` cleanly | `E-FM-*` on `aaa` and **no finding on `bbb`**. Pins that a candidate with any earlier hard error never claims its name (6.6.4) |
| 63e | `_agent_x/skills` is a **broken** symlink | no finding at all, exit 0 (6.2 step 4) |
| 63f | `_agent_x/skills` is a symlink resolving to a real directory | `E-SKILLS-NOT-DIR`, exit 1 |

**Position (6.7)**

| # | Case | Expect |
| --- | --- | --- |
| 64 | `_agent_x/skills/y/SKILL.md` | canonical, no `I-NOT-INDEXABLE` |
| 65 | `__agent_x/skills/y/SKILL.md` | `I-NOT-INDEXABLE`, `reason: replica-root`, exit 0 |
| 66 | `_agent_x/skills/y/z/SKILL.md` | `reason: not-under-skills-dir` |
| 67 | `_agent_x/Skills/y/SKILL.md` | `reason: skills-dir-case` |
| 68 | `docs/examples/SKILL.md` | `reason: owner-root-not-recognized` |
| 69 | A structurally broken skill in a non-canonical position | still `E-*`, exit 1 (severity rule 1) |

**Output (6.9, 6.10)**

| # | Case | Expect |
| --- | --- | --- |
| 70 | `--json` on a tree with findings of all three severities | parses as JSON; `exitCode` matches the process exit; `summary` counts match the array |
| 71 | `--json` ordering | findings sorted by `path`, then `line`, then `code`, and running twice gives byte-identical output |
| 72 | `path` separators | every `path` and `skillDirectory` contains no `\` |
| 73 | A clean tree | exit 0, `errors: 0`, human report ends with `OK:` |
| 74 | A tree with only warnings and notes | exit 0 |
| 75 | A tree with one error | exit 1 |
| 76 | A tree whose **only** fault is `_agent_x/skills` being a plain file | exit 1, and the final human line is `FAIL: M error findings, no entrypoint implicated`, never `FAIL: 0 skills would not be indexed` (6.9). Step 6 added this |
| 77 | A directory with both `SKILL.md` and `skill.md` (skips per 9.2.1) | `entrypointsFound: 2`, `skillsIndexable: 1`. Pins the 14.5.1 definitions, which 6.10 now carries |
| 78 | Two `W-YAML-UNDECIDABLE` findings on the same line of the same file | the two orderings are stable across runs, pinning the emission-order tiebreak (6.10) |
| 79 | `_agent_x/skills/` itself unreadable (skips like 63c) | `E-SKILLS-DIR-UNREADABLE` with the `` `skills` directory could not be read `` message, distinct from case 63c's `E-SKILL-DIR-UNREADABLE`. Pins the code split (6.8) |
| 80 | A `SKILL.md` whose `name` value contains an embedded `\n` and a `\x1b`, long enough to truncate | the human report stays on the finding's own lines and contains no raw control character (Section 7) |

### 9.3 Acceptance criteria

Objective and individually checkable at `f08b8241` plus this change:

1. `node scripts/01-skills-checker.mjs --help` exits 0 and prints the usage block, **in a checkout
   where `node_modules/` does not exist**. This is the zero-dependency guarantee and it must be
   tested exactly that way.
2. `npm run check:skills:self` exits 0 and prints a pass line naming the number of cases run and the
   number skipped.
3. `npm run check:skills -- <path-to-a-matrix-with-a-wrongly-cased-entrypoint>` exits 1 and reports
   `E-ENTRYPOINT-CASE` for it. Run against the live `tiktok-news-publisher` case from the issue if it
   is still present; otherwise against a fixture reproducing it.
4. `npm run check:skills` from the repo root exits 0. The repo tree contains no `SKILL.md`, so this
   verifies the clean path and that the walk does not choke on `src-tauri/target/` or `node_modules/`.
5. `npm run check:skills -- --json .` produces output that `JSON.parse` accepts and whose
   `summary.errors` equals the number of `findings` with `severity === "error"`.
6. `npm run typecheck` exits 0, `npm test` is green with the same test count as at `f08b8241`, and
   `npm run test:debt` exits 0. None of the three should be affected; if any changes, the blast
   radius was exceeded.
7. `npm run version:check` exits 0, confirming the `package.json` edit did not disturb the anchors in
   `scripts/check-version-sync.mjs:37`.
8. `git diff --stat f08b8241..HEAD` touches exactly three files: `plans/1213-skills-checker.md`
   (force-added, Section 8 step 0), `scripts/01-skills-checker.mjs` and `package.json`. If
   `package-lock.json` appears, a dependency was added and user decision 5 was broken.
9. `grep -n "existsSync" scripts/01-skills-checker.mjs` returns nothing. The forbidden call
   (Section 5.1.2) must not be present in any form.
10. `grep -n "toString('utf8')\|toString(\"utf8\")" scripts/01-skills-checker.mjs` returns nothing,
    for the same reason under `F8`.
11. `grep -n "session_context.rs:206-714" scripts/01-skills-checker.mjs` returns at least one hit, in
    the header. This is the drift control the coordinator required.
12. Every code in Section 6.8 is emitted by at least one self-test case, **with one named exemption:
    `E-ENTRYPOINT-UNREADABLE`**, which needs a file whose `open`/`read` throws and is not reliably
    constructible as the owning user on Windows or as root on Linux (9.2.2). Verifiable by grepping
    the code list against the self-test body; the exemption must appear as a comment naming this
    criterion, so a future reader does not mistake it for an oversight.
13. **`npm run check:skills:self` exits 0 on Windows without Developer Mode**, and its skip line
    names only cases 19, 20, 63c, 63g and 79. Step 6 added this criterion: it is the one that proves
    9.2.1 was implemented, since the failure it guards against, case 20 hard-failing on a
    case-insensitive filesystem, makes criterion 2 fail on the team's own machines. **Step 7 corrected
    the list**: it named "only cases 19 and 63c", which omits case 20 (which skips on Windows by
    9.2.1's own probe, so a run that does not name it has not implemented the probe), case 79 (which
    shares 63c's unreadable-directory probe) and case 63g (added in Step 7). The parenthetical "plus
    `E-ENTRYPOINT-UNREADABLE`'s" is dropped because that code has no case to skip; it is an exemption
    under criterion 12, not a skip.
14. The self-test prints, for each skipped case, the case number and the reason. A run that skips
    silently is indistinguishable from a run that passed, which is the same false confidence this
    whole tool exists to remove.

## 10. Adjacent findings, reported and not changed

1. **The `tiktok-news-publisher` skill is broken right now** in this workspace, with exactly the
   failure this checker is built to catch. Fixing it is not part of this issue; the checker only has
   to report it.
2. **`vitest.config.ts` collects nothing outside `src/`.** Any future `.mjs` script that wants a real
   vitest suite hits the same wall this plan routes around. Worth a decision of its own, not a
   side effect of this issue.
3. **The checker cannot recognize the Root Agent directory** as a legal owner root, because that name
   is a runtime value in `root_agent.rs:633-639`. The consequence is confined to an `info` finding
   (Section 6.7, `owner-root-not-recognized`) and never a false failure. If it becomes noisy, the fix
   is a `--matrix-root-pattern` flag, which this issue does not add.
4. **Drift has no automatic detector.** The header comment naming `session_context.rs:206-714` is a
   convention, not a gate. A CI job asserting that the Rust and the `.mjs` agree on a shared fixture
   corpus would close it, and is a separate issue.

## 11. Risks

| Risk | Mitigation in this plan |
| --- | --- |
| The `.mjs` duplicates rules owned by `session_context.rs:206-714` and silently drifts | Mandatory header naming the range (5.1.1), acceptance criterion 11, rule IDs on every finding (6.8), and `indexerMessage` on every mirrored code so a startup warning is searchable back to a checker finding |
| The mini-parser disagrees with `serde_yaml` and passes a file the app rejects | Anything outside the fixed subset becomes `W-YAML-UNDECIDABLE` (6.5.2) rather than a silent pass. The three certain-error shapes (6.5.3) are the only cases where the parser asserts a hard failure |
| Case-insensitive discovery accidentally becomes case-insensitive validation | `===` on the listing name (5.1.2), `existsSync` banned, acceptance criterion 9, self-test cases 14, 15, 20 |
| A generic run over an ordinary repository fails the build on position findings | Position is always `info` (6.7), absence-type findings are promoted to `error` only in canonical position (4.1 rule 2) |
| An unverified rule (Section 12) turns into a false CI failure | Half closed. Step 5 resolved all five Section 12 items against pinned source, so no finding rests on an unverified *fact*. But Step 6 then found **six false positives** that came from elsewhere — an over-eager severity on undecidable values, an ungated symlink error, two under-specified YAML rules, and a valid file shape nobody had written down (Section 15.3, G4-G7, G13-G14). The lesson is that "every cited fact is verified" is not the same as "every rule is safe": the risk lives in the rules the plan states *without* citing a fact. Each is now corrected, and the self-test cases that pin them are 11a, 29a, 42a-42f and 76 |
| The pinned `serde_yaml` version changes and takes YAML semantics with it | Two rules in this plan are facts about `0.9.34+deprecated` specifically, not about YAML: duplicate keys are fatal (12.1) and plain `yes`/`no`/`on`/`off` are strings (12.5). Both cite the crate file and line. A `serde_yaml` bump is a reason to re-read Section 12, and the crate is deprecated upstream, so that bump will come |

## 12. Formerly unverified items: all five RESOLVED in Step 5

Step 4 could not confirm these five facts and fixed the checker's behaviour conservatively. Step 5
(dev-rust) settled every one of them against pinned source. Nothing in this section is open any more.
Where the resolved fact contradicts the Step 4 behaviour, the owning section has been corrected and
the change is named below.

The `serde_yaml` version is pinned at **0.9.34+deprecated** (`src-tauri/Cargo.toml:12` declares
`serde_yaml = "0.9"`; `Cargo.lock:5483-5486` resolves it to `0.9.34+deprecated`, on
`unsafe-libyaml 0.2.11` at `Cargo.lock:6893-6896`). Items 1 and 5 are answered from that crate's
vendored source, so they are facts about the version this repo actually builds, not about the family.

### 12.1 YAML duplicate keys: RESOLVED. They are REJECTED, so this is a hard error.

`serde_yaml` 0.9.34 rejects duplicate mapping keys. The chain is three hops and every one is source:

1. `session_context.rs:621` parses with `serde_yaml::from_str::<serde_yaml::Value>`.
2. `Value`'s map visitor delegates the whole mapping to `Mapping::deserialize`
   (`serde_yaml-0.9.34/src/value/de.rs:100-107`).
3. `Mapping`'s visitor walks entries and returns `Err(DuplicateKeyError)` the moment
   `mapping.entry(key)` is `Entry::Occupied` (`serde_yaml-0.9.34/src/mapping.rs:813-822`).

The rendered message for a string key is `duplicate entry with key "name"`
(`mapping.rs:840` writes the prefix, `:845` formats a `Value::String` key with `{:?}`); `serde_yaml`
appends its own position suffix. It surfaces through `session_context.rs:624-628` as a hard error.

**Change applied:** Section 6.5.4 now emits `E-YAML-DUPLICATE-KEY` (**error**), not
`W-YAML-DUPLICATE-KEY` (warning). Section 6.8 gains the code and loses the warning. The
"first occurrence wins" resolution rule is deleted: the indexer never resolves any field, because the
parse itself fails and the skill is skipped outright. Self-test case 46 is corrected.

Note the scope: the check is on the deserialized mapping, so it applies to **any** duplicated
top-level key, including keys the indexer never reads. `foo: 1` twice is fatal even though `foo` is
otherwise ignored under `Y6`. The checker must therefore run duplicate detection over every key, not
only over `name`, `description` and `when_to_use`.

### 12.2 The 1024-byte limit: RESOLVED. First line only, CONFIRMED, with a boundary correction.

`session_context.rs:342-351` is an if/else on `saw_opening`:

```rust
if !saw_opening {
    if current_line.len() > 1024 { return Err("missing opening frontmatter delimiter"); }
} else {
    let remaining = SKILL_FRONTMATTER_MAX_BYTES.saturating_sub(frontmatter.len());
    if current_line.len() > remaining.saturating_add(8) { return Err(frontmatter_limit_error()); }
}
```

The 1024 test is unreachable once the opening delimiter has been seen, so the plan's assumption holds:
**no per-line limit applies to body lines**, only the frontmatter-total guard in the `else` arm.

Two corrections, both boundary-exact:

- The limit **includes the line terminator**. The check runs after `current_line.push(*byte)`
  (`:340`) and before the `if *byte != b'\n'` short-circuit (`:353`), so the `\n` itself is counted.
  A first line of 1020 spaces plus `---\n` is 1024 bytes and is **valid**; 1021 spaces plus `---\n`
  is 1025 bytes and fails. The rule is: **the first line including its terminator must be at most
  1024 bytes.** Section 6.4 step 1 has been restated in those terms; its previous wording
  ("scan for the first `\n` within the first 1024 bytes") is off by the terminator byte.
- A corollary worth stating because rule `F4` invites the opposite conclusion: whitespace padding
  around the opening `---` is tolerated by the trim, but only up to this 1024-byte ceiling. Past it
  the file fails, and it fails as ``missing opening frontmatter delimiter``, not as a size error.

### 12.3 What the 16384-byte count includes: RESOLVED. Terminators counted, CONFIRMED, plus a missed rule.

`append_frontmatter_line` (`:311-317`) tests `frontmatter.len() + line.len() > 16384` and then
appends `line` verbatim. `line` is `&current_line`, and `current_line` accumulates every byte
including the `\n` (`:340`) and is only cleared after the append (`:370`). So terminators **are**
counted, `\r\n` costs two bytes, and both delimiter lines are excluded: the opening line is cleared
without appending (`:362`) and the closing line returns before the append (`:366-367`). The plan's
assumption is confirmed exactly.

**But Step 4 missed a second, independent limit** in the same `else` arm quoted in 12.2: a
**mid-line** guard that fires while a line is still being read, at
`current_line.len() > (16384 - frontmatter.len()) + 8`.

For an ordinary body line this only stops the read a few bytes earlier and produces the same message,
so it changes nothing. It matters in exactly one place: **it also applies to the closing delimiter
line**, which `append_frontmatter_line` never sees. A closing `---` padded with enough leading
whitespace to exceed `remaining + 8` is rejected with ``frontmatter exceeds 16384 byte limit``
instead of closing the block, even though `is_frontmatter_delimiter` would have accepted it.

**Change applied:** Section 6.4 step 3 now applies the guard to every post-opening line before
deciding whether that line is the closing delimiter. Without it the checker would accept a file the
indexer rejects. This is one of the two false passes named in Section 14.3.

### 12.4 How `skills` is matched: RESOLVED. It is a path join. The platform divergence is real.

`session_context.rs:509` is `let skills_path = matrix_path.join(SKILLS_DIR_NAME);`, and every
subsequent operation takes that joined path: `:520` `skills_path.exists()`, `:524`
`symlink_metadata(&skills_path)`, `:542` `read_dir(&skills_path)`. There is **no** directory listing
of the matrix root and **no** entry-name comparison anywhere in this path. So the plan is right on
both counts: it is a join, and it is the only genuinely platform-divergent rule in the surface.
`_agent_x/Skills/` resolves on Windows and does not on Linux. `reason: skills-dir-case` stays
informational, which is the correct call for a rule whose outcome depends on the reader's OS.

Contrast this with the entrypoint at `:401`, which **is** an entry-name comparison
(`entry.file_name() != OsStr::new("SKILL.md")`) and is therefore uniform on every platform. The two
constants look symmetric in the source and behave differently. That asymmetry is the single most
counter-intuitive fact in the whole surface and belongs in the script's header comment.

**Change applied:** none to behaviour. Section 6.7 `skills-dir-case` was already correct.

### 12.5 YAML 1.1 vs 1.2 scalar resolution: RESOLVED. It is YAML 1.2 core schema.

`serde_yaml-0.9.34/src/de.rs:932-938`:

```rust
fn parse_bool(scalar: &str) -> Option<bool> {
    match scalar {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}
```

and `:925-930`, `parse_null`, accepts only `null`, `Null`, `NULL`, `~`. Nothing else resolves to a
bool or a null. `yes`, `no`, `on`, `off`, `y`, `n` and every case variant resolve as **plain
strings**. This is the YAML 1.2 core schema, not YAML 1.1.

`de.rs:895-896` confirms the dispatch: only `ScalarStyle::Plain` reaches `visit_untagged_scalar`,
which is where `parse_null` and `parse_bool` are consulted. Single-quoted, double-quoted, literal and
folded scalars fall through to `visitor.visit_str` (`:898-902`) and are **always** strings. The
plan's 6.5.1 table was already right about quoted and block scalars; it is now also settled for
plain ones.

**Change applied:** `yes` / `no` / `on` / `off` / `y` / `n` are removed from the
`W-YAML-UNDECIDABLE` list in 6.5.2 and added to 6.5.1 as strings. `description: yes` is a clean
string and produces **no finding at all**. Self-test case 42 is corrected. Emitting a warning there
would have been a false positive on a spelling authors use routinely.

## 13. Open decisions

None. The CLI surface, the exit-code mapping, the full finding taxonomy, the severity of the position
category, the JSON schema and its ordering, the YAML subset and its out-of-subset behaviour,
duplicate-key handling, duplicate-name comparison scope, symlink and loop handling, the test location
and the fixture strategy are all fixed above. The five items in Section 12 are no longer unverified:
Step 5 resolved every one against pinned source, and the two that contradicted Step 4's conservative
choice have been corrected in the sections that own them.

**Step 6 opened none either.** All seventeen of its findings (Section 15.3) were decided from source
already pinned in this plan, and each was corrected in place rather than escalated. Three of them do
change decisions Step 4 recorded: `SKIP_DIRS` no longer applies inside a `skills/` directory,
`E-SKILL-DIR-LINK` is now gated on canonical position, and an undecidable value may never produce an
error. All three are narrowings of checker-side policy, not reversals of a user decision.

**Step 7 opened none either, and closed eight internal gaps.** None needed new evidence and none
changed a rule the indexer imposes; all eight were contradictions or omissions inside this document,
listed in Section 16.2. The one that changes behaviour an implementer would write is the corrected
finding code on self-test case 63c. The rest make the normative text say what the plan already
decided.

**Nothing in this file is unresolved, deferred, pending a later decision, offered as a competing
alternative, or left to the implementer's judgement.** Verified in Step 7 by reading every section
start to finish, not by grep. The wording here deliberately avoids the placeholder tokens themselves
so that a mechanical audit of this file for those tokens comes back empty.

## 14. Step 5 record (dev-rust)

### 14.1 Codebase Memory gate: DOWN, diagnosed, not worked around

The gate was run three times against `repo-AgentsCommander` and failed identically every time:

```
{"operation":"gate","error":"status.git.base_sha must be a string"}
```

**The message is misleading. `base_sha` IS a string; it is the empty string.** `index_status` for
this project returns `"base_sha": ""` alongside a correct `"head_sha":
"f08b82419b7943d694965af000630bf053e2922a"`, `"status": "ready"`, 20185 nodes and 107234 edges. The
gate's `Assert-String` rejects a whitespace-only string unless `-AllowEmpty` is passed, and the
`base_sha` assertion does not pass it. So the index itself is healthy and fresh at branch HEAD; only
the gate's precondition fails.

**Why `base_sha` is empty.** Codebase Memory populates it only when the branch's upstream resolves.
A three-arm comparison across sibling clones on this machine settles it:

| Clone | Branch | `remote.origin.fetch` | `@{u}` resolves | `base_sha` |
| --- | --- | --- | --- | --- |
| `wg-11/repo-agentscommander_webpage` | `main` | main only | yes | `6a7d3ae2`, equals head |
| `wg-2/repo-AgentsCommander` | `fix/1206-flaky-tests-parallel-load` | main **plus that branch** | yes | `f08b8241`, equals head |
| `wg-7/repo-AgentsCommander` | `feature/1173-...` | main only, no upstream configured | no | **empty** |
| `wg-11/repo-AgentsCommander` (this one) | `feature/1213-skills-checker` | main only | no | **empty** |

The wg-2 arm is the decisive one: it is on a feature branch and its gate precondition is satisfied,
so the predictor is not "the branch must be `main`". It is upstream resolvability, and wg-2 has an
extra explicit refspec line for its branch.

**The architect's hypothesis was directionally right but imprecise, and the tech lead's fix could not
have worked.** The upstream *is* configured here: `branch.feature/1213-skills-checker.remote=origin`
and `.merge=refs/heads/feature/1213-skills-checker`. The remote-tracking ref also exists;
`git rev-parse --verify refs/remotes/origin/feature/1213-skills-checker` returns `f08b8241`. Git
still refuses:

```
fatal: upstream branch 'refs/heads/feature/1213-skills-checker' not stored as a remote-tracking branch
```

because git resolves `@{u}` by mapping `branch.<name>.merge` **through the remote's fetch refspec**,
not by testing whether a ref file exists. With `remote.origin.fetch =
+refs/heads/main:refs/remotes/origin/main`, `refs/heads/feature/1213-skills-checker` maps to nothing.
Pushing the branch and fetching its ref cannot fix that, which is exactly what was observed.

The remedy is one refspec line, the shape wg-2 already has. **It was not applied**: the instruction
was to diagnose, not to work around, and no git config, refspec or branch was changed.

There is also a latent second blocker behind this one. Even with `base_sha` populated, the gate
requires `base_sha == head_sha`. That holds today only because this branch has no commits of its own
(`git merge-base HEAD origin/main` returns `f08b8241`, which is HEAD). Once a real commit lands here,
whether the gate passes depends on whether Codebase Memory sets `base_sha` to HEAD or to the
merge-base. Worth knowing before anyone treats the refspec as a complete fix.

### 14.2 Method and what the gate outage costs

With the gate down, Section 12 and the fidelity review were settled by direct reads, which the Step 5
brief explicitly authorised. Sources read, all read-only:

- `src-tauri/src/config/session_context.rs`, ranges `:204-418`, `:446-490`, `:489-698`.
- `src-tauri/Cargo.toml:12` and `Cargo.lock:5482-5496`, `:6892-6896` for the pins.
- Vendored `serde_yaml-0.9.34+deprecated`: `src/de.rs:846-938`, `src/value/de.rs:100-107`,
  `src/mapping.rs:813-851`.

**Confidence cost, stated plainly.** The gate provides breadth, not depth: it certifies that the
graph is complete and fresh so that a "no other caller exists" claim is trustworthy. Everything in
Section 12 and Sections 14.3 and 14.4 is a *local* fact about code that was read line by line, and
those are unaffected. What is weaker is the negative claim inherited from the evidence artifact that
`session_context.rs` is the **only** module implementing these rules and that no second call site
enforces anything additional. That claim was verified under a clean gate during the original
investigation at this same HEAD `f08b8241`, and nothing has been committed to the branch since, so it
still holds by provenance rather than by re-verification. If a reviewer wants that re-established
independently, the refspec has to be fixed first.

### 14.3 Fidelity defects found and corrected

Seven. Two would have made the checker approve a file the indexer rejects, which is the failure mode
this tool exists to prevent.

| # | Defect | Section | Corrected to | Impact |
| --- | --- | --- | --- | --- |
| 1 | The mid-line frontmatter guard at `:346-351` was missed entirely, so a whitespace-padded closing `---` would close the block instead of failing | 6.4 step 3 | Guard A added, applied before the closing-delimiter test | **False pass** |
| 2 | `String.prototype.trim()` strips U+FEFF and Rust `str::trim` does not | 5.1.2, 6.6.1 | Explicit Unicode `White_Space` set, JS `trim()` banned for field values | **False pass** |
| 3 | Duplicate YAML keys specified as a warning; `serde_yaml` 0.9.34 rejects them | 6.5.4, 6.8, 12.1 | `E-YAML-DUPLICATE-KEY`, error, scoped to every top-level key | False negative |
| 4 | An unreadable skill directory in canonical position was routed to `W-DIR-UNREADABLE`; the indexer treats it as a hard error (`:396-397`) | 6.2 step 1 | Promoted to `E-SKILLS-DIR-UNREADABLE` | False negative |
| 5 | Duplicate-name participation limited to `E-NAME-*` failures; any earlier hard error also removes a candidate | 6.6.4 | Rewritten with a worked example | **False positive**: blames an innocent skill |
| 6 | `yes` / `no` / `on` / `off` listed as undecidable; 0.9.34 resolves them as plain strings | 6.5.1, 6.5.2, 12.5 | Removed from the undecidable list, added as strings | False positive on common spellings |
| 7 | The 1024-byte first-line rule stated as "find `\n` within the first 1024 bytes"; the terminator is counted, so the real ceiling is 1024 **including** it | 6.4 step 1, 12.2 | Restated as a running-length test, with boundary cases 35a and 35b | Off by one byte |

Also corrected without changing behaviour: a broken `skills` symlink is silent rather than
`E-SKILLS-NOT-DIR`, because `:520` gates on `Path::exists`, which follows links (6.2 step 4). And two
Rust-versus-JS comparison hazards are now spelled out in 5.1.2: `u8::is_ascii_whitespace` excludes
`\x0B`, and Rust `String` ordering is UTF-8 byte order rather than JS UTF-16 code-unit order.

### 14.4 Correction to the evidence artifact

`__agent_dev-rust/findings/skill-md-validation-rules.md` states 22 hard-error conditions. **The
correct number is 23.** Its hard-error table omits

``Skipped skill directory `{folder}`: could not inspect exact SKILL.md entrypoint: {err}``

emitted when `entry.file_type()` fails on the `SKILL.md` entry at `session_context.rs:405-407`,
wrapped by `:600-606`. The artifact documents the sibling failure at `:569-573` for skill directories
but not this one for the entrypoint. The error is mine and it propagated into this plan's
Section 6.8, which claimed to account for "all 22 hard errors". Section 6.8 now says 23 and lists the
condition in the unmappable bucket, where it belongs: Node resolves `Dirent` types inside
`readdirSync`, so there is no separate per-entry call that can fail.

Nothing else in the artifact was found to be wrong. Every other rule, message string and `file:line`
in it that this plan depends on was re-checked against `f08b8241` during this pass and holds. The
artifact's single self-declared unknown, duplicate-key behaviour, is now resolved in Section 12.1.

### 14.5 Implementability flags

Not defects in the rules, but places where an implementer would otherwise have to guess.

1. **`entrypointsFound` needs a definition.** With a shadowed casing variant (6.3 row 5) one
   directory yields two matches, one validated and one not. Fix the contract: `entrypointsFound`
   counts **every** discovered match including shadowed ones, `skillsIndexable` counts only
   fully validated entrypoints with zero `error` findings. Otherwise `summary` numbers will not
   reconcile with the findings array and self-test case 70 is untestable.
2. **JSON ordering needs a final tiebreak.** `path`, then `line`, then `code` can still tie, for
   example two `W-YAML-UNDECIDABLE` findings reported for the same line. Append emission order as
   the last key so the sort is total and case 71's byte-identical guarantee actually holds.
3. **`statSync` is required and must not be refactored away.** 6.2 step 4 needs one link-following
   `fs.statSync(p, { throwIfNoEntry: false })` to separate a broken symlink from a resolving one.
   Acceptance criterion 9 bans `existsSync` and this is not that, but the two look alike enough that
   the ban should be read as naming `existsSync` only.
4. **`Dirent.isSymbolicLink()` on Windows junctions should be proven, not assumed.** The plan leans
   on it in three places. libuv reports reparse points as `UV_DIRENT_LINK`, so it should hold, but
   nothing in this repo pins it. Self-test case 11 covers it only where the platform lets the test
   create a junction, and unprivileged Windows without Developer Mode will skip exactly the case that
   matters. Recommend the implementer run case 11 on a Windows box with Developer Mode on and record
   the result, rather than shipping on a skipped test.
5. **Case-variant `skills` in step 4 of 6.2 is platform-divergent** in the same way as 12.4. A file
   named `Skills` under an `_agent_*` directory is `E-SKILLS-NOT-DIR` on Windows and invisible on
   Linux. Emitting the error on both keeps the report stable across platforms and is the right call,
   but the message should say so rather than implying a universal rule.
6. **Node's `readdirSync` lossily converts unpaired surrogates in filenames**, and Rust's
   `to_string_lossy` at `:565` does the same, so the two agree. Worth a one-line comment at the call
   site so a future reader does not "fix" it toward a buffer-based listing and silently diverge.

**Step 6 note on this section.** Items 1 and 2 above were recorded here and **never written into the
sections that own them**. Section 6.10's field table still carried the superseded definition of
`skillsIndexable`, never defined `entrypointsFound` at all, and its ordering paragraph still stopped
at `code` with no emission-order tiebreak. Unlike Section 14.3, whose seven defects say "corrected in
place in the section that owns it", Section 14.5 is a record only — and an implementer working from
the normative sections would have followed 6.10 and shipped both bugs. Both are now applied in 6.10.
Item 3's `statSync` carve-out is restated in acceptance criterion 9's own wording, item 4 is answered
constructively in 9.2.1 (use a junction on win32 and stop skipping the case), and item 5 is answered
in 6.2 step 1. Item 6 stands as written.

## 15. Step 6 record (dev-rust-grinch)

### 15.1 Method, and the Codebase Memory gate

**The gate is green.** Run against `repo-AgentsCommander`, it returned
`{"operation":"gate","status":"ready", ... "nodes":20185,"edges":107234,`
`"git":{"is_git":true,"head_sha":"f08b8241…","base_sha":"f08b8241…"}}`. The tech lead's
`git remote set-branches --add` fix worked: `@{u}` now resolves and `base_sha` is populated. Section
14.1's diagnosis is confirmed correct in full, including the part about the refspec being the actual
cause rather than the missing remote-tracking ref.

Graph operations used: `name` over `(?i)(skill|frontmatter|yaml_field|is_valid_skill)`, and `trace`
on `discover_skill_index`. The trace settles the negative claim Section 14.2 could only carry by
provenance: `discover_skill_index` has exactly **one** production caller,
`ensure_session_context_with_config`; every other inbound edge is a test. Its callee set is closed
and is exactly the surface this plan mirrors — `find_exact_skill_entrypoint`,
`extract_skill_frontmatter`, `is_frontmatter_delimiter`, `append_frontmatter_line`,
`frontmatter_utf8`, `yaml_field_string`, `is_valid_skill_name`. No second module implements these
rules at `f08b8241`. That claim is now re-established independently rather than inherited.

Direct reads then went past the skill's one-fallback allowance, deliberately and with the Step 6
brief's explicit authorisation: `session_context.rs:204-423` and `:440-719` were read line by line,
because a fidelity pass over a pinned range is verification of exact text, not exploration. Every
finding below cites the line it came from.

### 15.2 The open question about the gate after a real commit lands

Section 14.1 flagged this and nobody had settled it. `cbm.ps1:2317` is

```powershell
if ($git['head_sha'] -cne $head -or $git['base_sha'] -cne $head) { throw 'Codebase Memory Git state is stale' }
```

so the gate requires `base_sha == HEAD`, not `base_sha == merge-base`. `base_sha` is derived from
upstream resolution. **Under either reading of that derivation the answer is the same:** if
`base_sha` is `rev-parse @{u}`, an unpushed local commit leaves it at the remote ref while `head_sha`
advances; if it is `merge-base(HEAD, @{u})`, an unpushed commit leaves it at the old head. Both
diverge from HEAD, and the gate throws `Codebase Memory Git state is stale`.

**Operational consequence for Step 8: commit, push, then re-index and re-gate.** Pushing advances the
remote ref, which repairs `base_sha` under both readings. A gate failure between the commit and the
push is expected and is not a regression of the refspec fix.

This is settled by the assertion plus the derivation, not by experiment: no clone on this machine is
currently ahead of its upstream (all 35 sibling repos surveyed read `ahead = 0`), so there was no way
to observe the failing state directly without changing branch state, which Step 6 was told not to do.

### 15.3 Defects found and corrected

Seventeen. Three are false passes, the class the brief named as the one that matters most, and six
are false positives that would fail a run over a healthy tree.

| # | Defect | Section | Corrected to | Impact |
| --- | --- | --- | --- | --- |
| G1 | Step 3 of the walk opened "For each entry that is a directory", so the nested `isSymbolicLink()` branch is **unreachable**: Node `Dirent`s use lstat semantics, and `isDirectory()` is false for every symlink | 6.2 step 3 | Rewritten to test `isSymbolicLink()` first, mirroring `:578-585` | **False pass** |
| G2 | `SKIP_DIRS` applied unconditionally, so `_agent_x/skills/target/` is never scanned; `target` is a legal skill name and the indexer applies no name filter | 6.2 step 3 | `SKIP_DIRS` never applies inside a `skills/` directory | **False pass** |
| G3 | `E-YAML-NOT-MAPPING` required "a **single** line with no `:`"; a two-line colon-free body is a multi-line plain scalar the indexer hard-rejects at `:633`, and fell through to a warning | 6.5.3 | Restated as "zero recognized entries plus significant content", with an exception for flow/tag/anchor lines | **False pass** |
| G4 | An `undecidable` `name` still ran the charset check at **error** severity, so `name: !!str ok-name` exited 1 on a skill the indexer indexes cleanly | 6.5.2, 6.6.1 | `W-NAME-UNDECIDABLE` (warning); an undecidable value may never yield an error | False positive |
| G5 | `E-SKILL-DIR-LINK` was not gated on canonical position, unlike its sibling `E-SKILLS-NOT-DIR` in the very next step | 6.2 step 3 | Gated; `W-SKILL-DIR-LINK` outside canonical position | False positive |
| G6 | "A tab in the indentation of any frontmatter line" fires on tab-indented content **inside** a `description: \|` block, which libyaml accepts | 6.5.3 | Restricted to lines treated as mapping entries | False positive |
| G7 | A file with no trailing newline whose last line is the closing `---` is **valid** (`:382-383`), and neither the prose nor any self-test said so | 6.4 step 3 | EOF branch given its own four-row table; cases 29a-29d | False positive |
| G8 | The self-test's skip list named only case 11. Case **20 hard-FAILS** on any case-insensitive filesystem, so `check:skills:self` exits 1 on Windows and macOS, breaking acceptance criterion 2 | 9.2.1 | Full skip table, probe-then-skip for 20 and 63c, and win32 junctions to stop 11/12/63e/63f skipping at all | Broken acceptance |
| G9 | `E-ENTRYPOINT-UNREADABLE` and `W-DIR-UNREADABLE` have no case, so acceptance criterion 12 was unsatisfiable | 9.2.2, 9.3 | Cases 76-80 added; one named exemption. **Step 7: only half closed.** None of cases 76-80 emits `W-DIR-UNREADABLE`, so that code still had no case and criterion 12 was still unsatisfiable with one exemption. Case 63g closes it (9.2.2) | Broken acceptance |
| G10 | The FAIL line counts entrypoints, but five error codes attach to no entrypoint, so a real failure prints `FAIL: 0 skills would not be indexed` | 6.9 | Explicit no-entrypoint wording | Incoherent output |
| G11 | Section 14.5 items 1 and 2 were recorded and never applied to Section 6.10, which an implementer would follow | 6.10, 14.5 | Both applied; 14.5 annotated | Two latent bugs |
| G12 | 6.10 claimed the JSON is byte-identical across platforms; `root`, `absolutePath`, `directoriesScanned` and `{err}`-bearing `indexerMessage` are all platform-dependent, and case 71 only tests idempotence on one machine | 6.10 | Guarantee scoped to `path` and `skillDirectory` | False guarantee |
| G13 | The `key: value` separator grammar was never defined: YAML allows a **tab** as separation space and a trailing ` # comment` after any scalar, and neither was in the subset | 6.5.1a | Grammar stated | False positive, via G4 |
| G14 | Block-scalar indentation detection was unspecified, and the explicit indicators `\|2`, `>2` were absent from the subset. A `description: \|` block containing `name: x` would emit a spurious `E-YAML-DUPLICATE-KEY` | 6.5.1, 6.5.1a | Indent rule stated; indicators added | False positive |
| G15 | 6.2 step 1 tests `skills` byte-exactly while 14.5.5 argues the opposite for step 4, with no statement of why | 6.2 step 1 | Both kept, and the severity rule that separates them made explicit | Internal contradiction |
| G16 | `E-SKILLS-DIR-UNREADABLE` covered two structurally different indexer errors, so `indexerMessage` is wrong for one of them despite 6.10 promising the exact log string | 6.8, 6.2 step 1 | Split into `E-SKILL-DIR-UNREADABLE` | Wrong `indexerMessage` |
| G17 | Section 7 claimed 200-character truncation stops a "line-broken" value scrambling the report; truncation does nothing about a `\n` at offset 10 | 7 | Collapse controls and whitespace first, then truncate, mirroring `:420-446` | Unmet claim |

### 15.4 What was attacked and held

Reported because a negative result is evidence too, and because re-attacking these in Step 7 is
wasted effort.

- **`isFrontmatterDelimiter` is a correct port.** Verified byte for byte against `:276-302`: the
  `\n`-then-`\r` strip order, the BOM strip **before** the trim, the `trim_ascii` set as exactly
  `\t \n \x0C \r space` with `\x0B` excluded, and the final `== b"---"`. All nine rows of the
  consequence table in 6.4.1 are right, including the subtle `  <BOM>---` = **false** and
  `<BOM>  ---` = true pair.
- **The 1024-byte first-line boundary.** `:339-345` pushes the byte then tests, and the test precedes
  the `\n` short-circuit at `:353`, so the terminator counts. 1020 spaces + `---\n` = 1024 is valid;
  1021 + `---\n` = 1025 fails. Cases 35a/35b are exact. The 1024-byte `read_buffer` at `:326` is
  chunking only and carries no semantics, so the plan's 64 KiB chunk is free to differ.
- **Guard A and Guard B.** `remaining.saturating_add(8)` at `:347-350`, the placement before the
  closing-delimiter test, and Guard B's exclusion of both delimiter lines (`:362` clears the opening,
  `:366` returns before the append) all match `:311-317` and `:346-351`. The `saturating_sub` cannot
  underflow in JS because the append guard keeps the buffer at or below 16384.
- **The duplicate-name sort.** `:588-591` is a two-element tuple compare of
  `(to_ascii_lowercase(), original)`, both as Rust `String`, that is UTF-8 byte order. The plan's
  `Buffer.compare` plus an explicit A-Z map is the correct port, the comparison is total because two
  entries in one directory cannot share a name, and both Rust `sort_by` and `Array.prototype.sort`
  are stable. Nothing to break here.
- **"Any hard error removes a candidate from the name race."** `:595-671` is one loop whose every
  failure path is a `continue`, and the `seen_skill_names.insert` at `:671` is strictly after the
  entrypoint, frontmatter, parse, mapping, name-type and charset checks. `description` (`:674`) and
  `when_to_use` (`:687`) run **after** the insert and can only ever produce warnings, so 6.6.4's
  "zero error findings at the end of field validation" is equivalent to the Rust. The worked example
  is correct.
- **`is_valid_skill_name`** (`:464-470`) is `chars().count()` in `1..=64` plus
  `is_ascii_lowercase() || is_ascii_digit() || '-'`. `[...name].length` with `/^[a-z0-9-]{1,64}$/` is
  faithful, and JS `$` without the `m` flag does **not** match before a trailing newline, so
  `"ok-name\n"` is correctly rejected. No gap.
- **A plain file directly inside `skills/` is silently ignored** by `:583`'s `else if is_dir()`, and
  the checker emits nothing for it either. The two agree.
- **The exit-code contract holds across all 31 codes.** Traced every one: no `error` code fails to
  force exit 1, no `warning` or `info` code can reach it, position findings never do, and there is no
  path producing exit 1 with zero findings. The only incoherence found was in the *reporting* of that
  verdict, which is G10, not in the verdict itself.
- **The `skills` broken-symlink silence** (6.2 step 4) is right: `:520` gates on `Path::exists`, which
  follows links, so `:524` is unreachable for a broken link and the indexer says nothing. Also worth
  recording: `Path::exists` returns false on **any** stat error, so an EACCES on `skills/` itself also
  returns silently at `:521` and `` `skills` could not be inspected `` (`:527-530`) is reachable only
  in the narrow window where `metadata` succeeds and `symlink_metadata` then fails. The plan's mapping
  is still correct; the message is simply rarer than it looks.

### 15.5 Verdict

The plan is implementable, and after these corrections I believe it is implementable **correctly**.
Before them it was not: an implementer following Sections 6.2, 6.5 and 6.10 to the letter would have
shipped three false passes and six false positives, and would have read a green
`npm run check:skills:self` on Windows as proof of behaviour that five codes' worth of cases never
actually exercised. None of the seventeen required new evidence — every one is decided by the pinned
source already cited in this plan — which is why they belong in Step 6 rather than in a rework.

Two things I would still not call closed, neither of them blocking:

1. **Drift remains uncontrolled**, as Section 10 item 4 already says. Seventeen fidelity defects
   surviving two review passes over a 500-line Rust surface is the measurement of how expensive a
   hand-maintained second implementation is. The shared-fixture CI job named there is worth opening as
   its own issue now, not later.
2. **`Dirent.isSymbolicLink()` on Windows junctions** is still asserted rather than proven. 9.2.1 now
   makes the self-test prove it on ordinary Windows instead of skipping, which is the cheapest
   available proof, but it becomes real evidence only once someone runs it. Step 8 should report that
   run's output.

## 16. Step 7 record (architect): certification

### 16.1 What Step 7 did and did not do

Step 7 is a Plan Contract audit, not a third fidelity pass. Section 15.4 lists what Step 6 attacked
and could not break, and re-attacking it here would have spent the round without improving the
answer, so it was not re-attacked. No Rust source was read in this pass and the Codebase Memory gate
was not run: nothing in Step 7 rests on a new fact about the codebase.

What was audited instead is the property the two enrichment passes could not audit for themselves:
**whether the normative sections of this file agree with each other and with the records that claim to
have corrected them.** That is the class Step 6 named as G11, where 14.5 items 1 and 2 were recorded
and never written into 6.10. Step 6 fixed its two instances; Step 7 was asked to check whether the
class had any others. It did. Eight, listed below.

Every section was read start to finish. Cross-references were checked in both directions: from each
record row to the section it claims to have corrected, and from each normative section back to the
codes and cases it names.

### 16.2 Gaps found and closed in Step 7

Numbered `A1`-`A8`, in the order they appear in the file.

| # | Gap | Section | Closed by | Class |
| --- | --- | --- | --- | --- |
| A1 | The traversal decision in Section 4's table still read "skipping `.git`, `node_modules`, `target`" with no carve-out, after G2 made `SKIP_DIRS` inapplicable inside a `skills/` directory. A summary table an implementer skims, contradicting the normative section | 4 row 4 | Carve-out added, with the G2 reference | Recorded-not-applied (G11 class) |
| A2 | The severity model did not cover its own instances. Rule 1 governs "a present **entrypoint file**" and rule 2 governs "something **absent or unreadable**"; `E-SKILLS-NOT-DIR` and `E-SKILL-DIR-LINK`/`W-SKILL-DIR-LINK` are neither, so no rule reached them, while 6.2 step 3 explicitly invoked "severity rule 2" for a case rule 2 did not describe | 4.1 | Rule 2 rewritten to cover every non-entrypoint finding, split into absence-type and present-entry-shape, with the complete code list for each | Normative gap |
| A3 | 6.2 step 1 said `E-SKILLS-NOT-DIR` is governed by "rule 1", while 6.2 step 3 four paragraphs below said the same gating "is severity rule 2 applied consistently with `E-SKILLS-NOT-DIR` in step 4". `E-SKILLS-NOT-DIR` is emitted only when the parent matches `/^_agent_.+$/`, which is a canonical gate, so rule 2 governs it and step 1 was wrong. The real difference between step 1 and steps 3 and 4 is byte-exact versus ASCII case-insensitive `skills` matching, a different axis entirely | 6.2 step 1, 4.1 | Step 1 corrected; both axes stated explicitly in rule 2 | Internal contradiction |
| A4 | The module table in 5.1 pointed `renderHuman` at 6.8 (the taxonomy), `renderJson` at 6.9 (the human report) and `main()` at 6.10 (the JSON format). Off by one throughout. Section 8 step 6 cites the correct sections, so the two disagreed | 5.1 | Corrected, plus a precedence rule and the four helper functions the 5.1.2 and Section 7 constraints imply | Wrong cross-reference |
| A5 | 6.7 closed with "the `canonical` flag is what promotes the two absence-type findings (`I-NO-ENTRYPOINT` and `W-DIR-UNREADABLE`)". Stale after G5 added `W-SKILL-DIR-LINK`, which is not absence-type, and after G16 split `E-SKILL-DIR-UNREADABLE` out. 6.7 also defined canonical position only for an entrypoint path, while 6.3, 6.2 step 1 and 6.2 step 3 all apply it to a bare directory | 6.7 | Directory form of the test stated once; promotion list replaced by a pointer to the complete list in 4.1 | Recorded-not-applied (G11 class) |
| A6 | **Self-test case 63c named `E-SKILLS-DIR-UNREADABLE`**, the code G16 split away from it. Case 79 already refers to "case 63c's `E-SKILL-DIR-UNREADABLE`", so the two rows contradicted each other. An implementer writing the self-test from case 63c would have asserted the wrong code and watched it fail against a correct implementation | 9.2, case 63c | Code corrected to `E-SKILL-DIR-UNREADABLE` | **Recorded-not-applied, behaviour-affecting** |
| A7 | **`W-DIR-UNREADABLE` still had no self-test case.** 9.2.2 said so and then claimed "cases 76-79 below close three of them"; none of 76-80 emits it. Acceptance criterion 12 therefore remained unsatisfiable with a single named exemption, which is the same defect G9 was raised to fix. Criterion 13's skip list was also short by three cases | 9.2.1, 9.2.2, 9.3 criteria 12 and 13, case 63g | Case 63g added; the accounting in 9.2.2 made explicit and checkable; criterion 13's skip list corrected | **Recorded-not-applied, breaks acceptance** |
| A8 | The self-test case count was reported as 94. The tables actually hold **109** rows after Step 6, and 110 after A7 added case 63g. Step 6's Section 15 record and the Step 6 report both carry 94, and Step 7's own summary inherited it before checking | header, 16.3 | Counted mechanically: 110 unique case ids, no duplicates, verified by extracting every row id between "Required cases, by section:" and Section 9.3. Header and 16.3 corrected | Wrong count in a record |

A6 and A7 are the two that would have cost the implementer real time: one makes a correct
implementation fail its own test, the other makes an acceptance criterion unsatisfiable. Both are the
same shape as G11 and both were invisible from inside the pass that created them, which is the
argument for the cross-reference audit being a distinct step rather than a habit.

A8 is the mildest of the eight and is recorded anyway, because a count nobody checks is exactly how
A6 and A7 survived: the number sounded settled, so it was carried forward instead of derived. The
count in Section 15.3 and in the Step 6 report is left as written; those are records of what that
pass believed, and correcting them retroactively would defeat the point of keeping records. The
header and 16.3, which an implementer reads, carry the verified number.

None of the eight needed dev-rust or dev-rust-grinch. Every one is either a correction of Step 4's own
text or a cross-reference an enrichment pass left behind, and in every case the correct answer was
already determined by a section of this plan or derivable by counting it. That is why Step 7 closed
them in place instead of opening a second round.

### 16.3 Plan Contract check

Each of the nine required elements, with where it lives.

| # | Required element | Where | Verdict |
| --- | --- | --- | --- |
| 1 | Issue and objective | 1 | Present. Issue, branch, baseline SHA, objective and non-objective |
| 2 | Evidence and current-state gap | 2, 12, 14, 15 | Present. Every rule traces to a `file:line`, and the one artifact error is corrected in 14.4 |
| 3 | In-scope and out-of-scope | 3 | Present. Three deliverables, eight exclusions |
| 4 | The decided solution | 4, 4.1 | Present. Nine decisions plus the severity model |
| 5 | Affected surfaces, exact files and symbols | 5.1, 5.2, 5.3 | Present. One new file with its module table, one two-line `package.json` diff, and an explicit do-not-touch list |
| 6 | Required behaviour, edge cases, failure behaviour | 6.1-6.11 | Present. 31 codes, the full CLI table, both frontmatter size guards, the EOF table, the YAML subset and its grammar |
| 7 | Compatibility and security | 7 | Present. Bounded reads, no `eval`, sanitize-then-truncate, rollback |
| 8 | Implementation order | 8, 8.1 | Present. Ten steps, one commit for 1-8, and the Codebase Memory sequence |
| 9 | Tests and objective acceptance criteria | 9.1-9.3 | Present. 110 cases, 14 criteria, every criterion mechanically checkable |

Cold-start test applied deliberately: an implementer with only this file needs no other document to
write the script. The one external dependency is the `git add -f` requirement for `plans/`, and
Section 8 step 0 states it with the `.gitignore` line number.

### 16.4 Two answers the coordinator asked for

1. **The shared-fixture CI job belongs outside #1213. Agreed.** Two independent reasons: the issue's
   own out-of-scope list excludes wiring anything into CI, and a job that runs the Rust and the `.mjs`
   over one fixture corpus needs a Rust-side harness, which means touching `src-tauri/`, which
   Section 5.3 forbids. It is the right follow-up and Section 15.5 is right that it should be opened
   now rather than later; it is not this issue.
2. **The remaining open items are correctly non-blocking.** `Dirent.isSymbolicLink()` on Windows
   junctions is asserted rather than proven, and 9.2.1 converts the assertion into a test that runs on
   ordinary Windows instead of skipping. That is the cheapest available proof and it is now on the
   critical path of acceptance criterion 13, so it cannot ship unexercised. Step 8 should report that
   run's output, as Section 15.5 asks.

### 16.5 Verdict

`READY_FOR_IMPLEMENTATION`.

The plan is a complete cold-start specification. Every rule traces to a pinned `file:line`; the five
Step 4 unknowns are resolved against source; the twenty-four defects the two enrichment passes found
are corrected in the sections that own them; and the eight cross-reference gaps those passes left
behind are closed above.

Certified against the file as it stands after the Step 7 edits. Any byte change invalidates this
certification, per the freeze gate.
