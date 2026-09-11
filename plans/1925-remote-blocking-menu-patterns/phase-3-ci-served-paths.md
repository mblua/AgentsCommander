# #1925 Phase 3: served-path inventory, its CI check, and CODEOWNERS

Status: READY_FOR_IMPLEMENTATION

- Parent issue: https://github.com/mblua/AgentsCommander/issues/1925. Child issue: created by `ac-tech-lead-v4`; its number arrives in your Step 8 handoff.
- Branch: `feature/<child-issue>-ci-served-paths`, exactly as named in the Step 8 handoff, cut from `origin/main` at this phase's Step 8.
- Drift baseline: `main` = `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, where this plan was verified. Every line number below is pinned to it. It is not the branch point.
- Class: `design-bearing` (a new check script; its self-test mirrors `scripts/check-test-debt.mjs --self-test`). Owner: `ac-dev-webpage-ui-v4`.
- Depends on: phase 1, which adds `remote-resources/blocking-menus/v1/settings-blocking-menus.json`. The inventory's second row names that path and the check fails on a row whose path is not tracked. Parallel with: phases 2, 4 and 5 (disjoint files, no dependency). Phase 6 depends on this phase.
- Contract: none at runtime. Repository metadata and CI only.
- Delivery: commit on the phase branch and report the head SHA to `ac-tech-lead-v4`. Push only as the Step 8 handoff says. `ac-tech-lead-v4` opens and lands the PR and collects the exact-head CI evidence.

## 1. Objective

Installed binaries fetch files from `main` by fixed path, forever. Record every such path in an append-only inventory, and add a CI check that fails in both directions: an inventory path missing from the tree, or a fetch literal in `src-tauri/` or `src/` with no inventory row. Add a `CODEOWNERS` file covering those paths.

Landing this phase alone adds one CI step and three repository files. The real tree then has 2 rows and 1 literal (the Home fetch). The second row is registered before any binary fetches its path; phase 6 adds that fetch later, and the inventory's rules allow a row to come first.

## 2. Before any write (mandatory, from the repo root)

1. State the environment risk in writing to `ac-tech-lead-v4` before touching code. At minimum: CI runs Node 22 in job `test-debt` with no `npm ci`, so the script may use Node built-ins only; your local Node version; the shallow clone; line endings (keep LF).
2. Branch and base:
   ```
   git fetch origin main
   git rev-parse --abbrev-ref HEAD
   git rev-parse HEAD origin/main
   git status --porcelain
   ```
   The branch equals the Step 8 name and matches `^feature/[1-9][0-9]*-ci-served-paths$`; both SHAs are equal; status prints nothing.
3. Landed dependency (phase 1): `git ls-files remote-resources/blocking-menus/v1/settings-blocking-menus.json` prints that path. Otherwise stop.
4. Drift:
   ```
   git cat-file -e "b4f7c9de7f15b75f8f12c034c8356f73babd0a8d^{commit}" || git fetch --deepen=200 origin main
   git diff --name-only b4f7c9de7f15b75f8f12c034c8356f73babd0a8d "$(git merge-base HEAD origin/main)"
   ```
   Expected entries: files of phases 1, 2, 4 and 5 of #1925 that have landed (all under `src-tauri/src/`, `src/`, `docs/`, `PRIVACY.md` or the phase 1 JSON). If the list names `.github/`, `package.json`, `package-lock.json`, `scripts/check-served-paths.mjs` or any other `remote-resources/` path, stop and report it.
5. Real-tree literals at the branch point: `git grep -n -i 'raw.githubusercontent.com/mblua/agentscommander/' -- src-tauri src` prints exactly one line, the `HOME_MARKDOWN_URL` literal in `src-tauri/src/commands/config.rs` (`:37` at the drift baseline). If it prints anything else, stop and report it.

## 3. Exact files

| File | Change |
|---|---|
| `remote-resources/SERVED-PATHS.md` | new |
| `.github/CODEOWNERS` | new |
| `scripts/check-served-paths.mjs` | new |
| `package.json` | two `scripts` entries |
| `.github/workflows/pr-regression-gates.yml` | one step in job `test-debt` |

Do not add the HTML comment to `docs/home-en.md` that the issue proposed; D6 says why.

## 4. Decisions (binding for this phase)

- D1. Inventory rows are `path`, `serving ref`, `first version that fetched it`. `docs/home-en.md` is row one even though it sits outside `remote-resources/`. Its version is `0.8.43`, verified with `gh api`: `v0.5.0` is the tag directly before `v0.8.43`; `src-tauri/src/commands/config.rs` has 0 `raw.githubusercontent.com` literals at `v0.5.0` and 1 at `v0.8.43`; the npm registry lists no version between 0.5.0 and 0.8.50. The blocking-menus row's version is `first release that ships the #1925 startup download`: true on every landed state of `main`, and it never needs editing.
- D2. Append-only by rule and by review, not by CI: the issue asks for exactly the two checks. `CODEOWNERS` routes changes to the owner.
- D3. The check runs as new commands in the existing required job `test-debt`. It is not a new job, so the repository ruleset's required-check list needs no change.
- D4. Both directions read the git index (`git ls-files -z`): a path counts as present only when tracked, and only tracked files under `src-tauri/` and `src/` are scanned, test code included.
- D5. A literal counts when it matches `https?://raw.githubusercontent.com/<owner>/<repo>/<ref>/<path>` with owner equal to `mblua` and repo equal to `agentscommander`, both compared case-insensitively (GitHub resolves them that way). Ref and path compare exactly. Strip trailing `.`, `,`, `;`, `:` from the path. Literals for other repositories are ignored. There is no exemption for templates: a counted literal whose ref or path holds `{`, `}` or other placeholder text is compared like any other and, having no row, fails. A fetch URL built from a template cannot be inventoried, so CI must reject it. Phase 6 pins its URL in a test without writing such a literal.
- D6. No HTML comment in `docs/home-en.md`. Verified at the drift baseline: `MarkdownIt({ html: false, linkify: true, typographer: false, breaks: false })`, the Home renderer options at `src/main/components/HomeView.tsx:6-11`, renders `<!-- x -->` as the visible paragraph `<p>&lt;!-- x --&gt;</p>`. Every installed binary fetches that file from `main`, so the comment would show as text on every Home panel. The issue made the comment conditional on the renderer dropping it, and it does not.
- D7. `CODEOWNERS` at `.github/CODEOWNERS`, owner `@mblua`. The current ruleset has `require_code_owner_review: false`, so the file requests the review but does not block a merge. Enforcing it is a repository-setting decision outside this diff.
- D8. Line endings: LF and CRLF inputs give the same result. `.gitattributes` has no `*.md` rule and Git for Windows sets `core.autocrlf=true` in its system gitconfig, so a Windows checkout holds this inventory in CRLF (`git ls-files --eol` shows `i/lf w/crlf` for `docs/home-en.md`, which has no attribute either), and phase 6 runs this check on Windows. The parser removes one trailing `\r` from each inventory line (section 5.3). The scan needs nothing more: its character classes exclude `\s`, so a `\r` never enters a ref or path, and lines count by `\n`. No `.gitattributes` pin: it would add a sixth file to this phase and still would not cover a CRLF `--inventory` copy.

## 5. Required content and behavior

### 5.1 `remote-resources/SERVED-PATHS.md` (verbatim, LF, trailing newline)

```markdown
# Served paths

AgentsCommander binaries fetch every path below from `https://raw.githubusercontent.com/mblua/AgentsCommander/<serving ref>/<path>`, starting with the version named in its row. Released binaries pin these paths forever: moving, renaming or deleting one breaks that fetch on every install still running such a binary, with no staging step.

Rules:

- Append only. Never delete or edit a row, even when no supported binary fetches that path any more.
- Add a row no later than the change that adds a fetch literal under `src-tauri/` or `src/`. A fetch URL must be one plain literal: a URL built from a template or placeholder cannot match a row and fails the check.
- `npm run check:served-paths` (CI job `test-debt`) fails when a row's path is not a tracked file, or when a fetch literal has no row with the same path and serving ref.

| Path | Serving ref | First version that fetched it |
|---|---|---|
| `docs/home-en.md` | `main` | 0.8.43 |
| `remote-resources/blocking-menus/v1/settings-blocking-menus.json` | `main` | first release that ships the #1925 startup download |
```

### 5.2 `.github/CODEOWNERS` (verbatim, LF, trailing newline)

```
# Installed AgentsCommander binaries fetch these paths from main.
# Read remote-resources/SERVED-PATHS.md before changing anything here.
/remote-resources/ @mblua
/docs/home-en.md @mblua
```

### 5.3 `scripts/check-served-paths.mjs`

- ESM, Node built-ins only (`node:fs`, `node:path`, `node:child_process`, `node:url`). Repo root = `path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')`.
- CLI: no argument runs the check. `--inventory <file>` overrides the default `remote-resources/SERVED-PATHS.md`; a relative path resolves against the current directory. `--self-test` runs the self-test. `--help` prints usage and exits 0. Any other argument exits 2.
- Exit codes: 0 consistent; 1 check failed; 2 usage error; 3 an input could not be read (`git ls-files` failed, inventory unreadable); 4 self-test failed.
- A pure core, `checkServedPaths({ inventoryText, inventoryName, trackedFiles, readSource })`, returns `{ rows, literals, errors }`. The CLI and the self-test both call it.
- Inventory parsing: split the text on `\n` and remove one trailing `\r` from each resulting line before any other test (D8); line numbers count those lines from 1. Every line starting with `|` is a table line. Skip exactly `| Path | Serving ref | First version that fetched it |` and `|---|---|---|`. Every other table line must match ``^\| `([^`|]+)` \| `([^`|]+)` \| ([^|]*[^|\s])\s*\|$``; otherwise the error is `malformed inventory row at <inventoryName>:<line>`. Zero rows gives `inventory has no rows`.
- Scan: every tracked path starting with `src-tauri/` or `src/`, read as UTF-8, global regex `https?:\/\/raw\.githubusercontent\.com\/([^\/\s"'`<>()\\]+)\/([^\/\s"'`<>()\\]+)\/([^\/\s"'`<>()\\]+)\/([^\s"'`<>()\\]+)`, filtered and trimmed per D5, each hit recorded with file and 1-based line.
- Errors, all collected before exiting:
  1. For each row whose path is not in `trackedFiles`: `inventory path <path> (<inventoryName>:<line>) is not a tracked file on this tree`.
  2. For each literal with no row of equal path and ref: `<file>:<line> fetches <ref>/<path>, which has no row in <inventoryName>`.
  3. Zero literals found: `no raw.githubusercontent.com/mblua/AgentsCommander literal found under src-tauri/ or src/; the scan is broken`.
- On success, print one line: `served-paths: <rows> rows, <literals> literals, consistent`. On failure, print each error on its own stderr line, prefixed `::error::` when `GITHUB_ACTIONS` is `true`.
- `--self-test`: in-memory fixtures only, no disk writes. Twelve cases, each asserting the exact error substring or no error:
  1. a good fixture (both rows tracked, one source with both literals, plus an unrelated `raw.githubusercontent.com/d3/d3-shape/master/img/x.png` literal): no error;
  2. a row path not tracked: `is not a tracked file`;
  3. a literal for `main/docs/new.md` with no row: `has no row`;
  4. the literal ref `dev` against a `main` row: `has no row`;
  5. owner and repo written `MBLUA/agentscommander` for a path with no row: `has no row`;
  6. positive control for D5 case-insensitivity: the only source literal is `https://raw.githubusercontent.com/MBLUA/agentscommander/main/docs/home-en.md`, and the only row is `docs/home-en.md` / `main`, tracked: no error and exactly 1 literal counted (a case-sensitive scan counts 0 and reports `the scan is broken`);
  7. a malformed row: `malformed inventory row`;
  8. zero rows: `inventory has no rows`;
  9. zero literals: `the scan is broken`;
  10. a trailing `.` after a literal path in a comment still matches its row: no error;
  11. a templated literal `https://raw.githubusercontent.com/mblua/AgentsCommander/{SOURCE_REF}/docs/home-en.md` beside a `docs/home-en.md` / `main` row: `has no row` (templates are never exempt);
  12. CRLF (D8): case 1's inventory text and source text with every `\n` replaced by `\r\n`: no error, and the same row and literal counts as case 1. A parser that keeps the `\r` reports `malformed inventory row` and fails this case.
  It prints `check-served-paths self-test passed (12 cases)` and exits 0, or names each failing case and exits 4.

### 5.4 `package.json`

After `"record:arcs:self"` (`:29`), add `"check:served-paths": "node scripts/check-served-paths.mjs",` and `"check:served-paths:self": "node scripts/check-served-paths.mjs --self-test",`. Nothing else changes (`package-lock.json` untouched).

### 5.5 `.github/workflows/pr-regression-gates.yml`

In job `test-debt`, directly after the step `"Check #480 guard controls"` (`:41-44`), add:

```yaml
      - name: "Check served-path inventory (#1925)"
        run: |
          npm run check:served-paths:self
          npm run check:served-paths
```

## 6. Tests

The self-test is the positive control for the broken-inventory side (acceptance 7). Section 7 adds real-tree negative controls.

## 7. Verification (from the repo root, report exit codes and output lines)

```
node scripts/check-served-paths.mjs --self-test
npm run check:served-paths
npm run test:debt
```

- Self-test: exit 0 and `check-served-paths self-test passed (12 cases)`.
- Real tree: exit 0 and exactly `served-paths: 2 rows, 1 literals, consistent`. Phase 6 adds the second literal and cannot have landed, because it depends on this phase.
- `npm run test:debt`: exit 0 (the existing step stays green).
- Negative control A, which never edits the tracked inventory: copy `remote-resources/SERVED-PATHS.md` to a scratch file outside the repo, delete the `docs/home-en.md` row, run `node scripts/check-served-paths.mjs --inventory <copy>`. Expect exit 1 and a line containing `src-tauri/src/commands/config.rs:<n> fetches main/docs/home-en.md, which has no row`, where `<n>` is the line section 2 step 5 printed.
- Negative control B: in a second scratch copy, change the blocking-menus row path to `remote-resources/blocking-menus/v1/missing.json`. Expect exit 1 and a line containing `is not a tracked file`.
- Positive control C (D8), which never edits the tracked inventory: write a CRLF copy outside the repo with `node -e "const f=require('fs');f.writeFileSync(process.argv[1],f.readFileSync('remote-resources/SERVED-PATHS.md','utf8').replace(/\r?\n/g,'\r\n'))" <copy>`, then run `node scripts/check-served-paths.mjs --inventory <copy>`. Expect exit 0 and exactly `served-paths: 2 rows, 1 literals, consistent`.
- Remote, after the PR exists, owner `ac-tech-lead-v4`: the `test-debt` log for the exact PR head SHA contains both the self-test line and the `served-paths:` line; `gh api "repos/mblua/AgentsCommander/codeowners/errors?ref=<this phase's branch>"` returns `{"errors":[]}`.

## 8. Acceptance criteria

1. Everything in section 7 holds.
2. `git diff --name-only "$(git merge-base HEAD origin/main)" HEAD` lists exactly the 5 paths of section 3 (ignore `plans/`). `docs/home-en.md` and `package-lock.json` are unchanged.
3. `git diff "$(git merge-base HEAD origin/main)" HEAD -- .github/workflows/pr-regression-gates.yml` adds exactly the 4 lines of section 5.5 and removes none.

## 9. Preserve

- Every existing job, step, name and trigger in `pr-regression-gates.yml`.
- `docs/home-en.md` bytes and path.
- `HOME_MARKDOWN_URL` in `src-tauri/src/commands/config.rs:36-37`.

## 10. Recovery

Restore only paths this phase changed that still hold this run's output: `git restore --source=HEAD -- <path>` for modified files, and deletion of the new files this run created. No `git reset`, `git clean` or repository-wide restore. Report external changes to `ac-tech-lead-v4`.
