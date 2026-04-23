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
