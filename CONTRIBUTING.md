# Contributing to AgentsCommander

## Branch naming

All new branches **must** follow this pattern and reference an **open GitHub Issue**:

```
<type>/<issue-number>-<slug>
```

| Field | Rules |
|---|---|
| `<type>` | See **Branch type prefixes** table below |
| `<issue-number>` | An open issue in this repo (no leading zeros, e.g. `63` not `063`) |
| `<slug>` | Lowercase kebab-case, `[a-z0-9]+(-[a-z0-9]+)*`, at most 50 characters |

### Branch type prefixes

| Prefix | Use for |
|---|---|
| `feat/` (alias `feature/`) | New functionality |
| `fix/` | Bug fixes |
| `bug/` | Investigation / repro of a defect |
| `chore/` | Tooling, deps, non-functional |
| `docs/` | Documentation only |
| `refactor/` | Internal restructuring, no behaviour change |
| `test/` | Test-only changes |
| `ci/` | CI / workflow changes |
| `style/` | Formatting-only tweaks |

### Local dev requirements

- `npm` ≥ 11.0.0 (enforced via `package.json` `engines`). Older versions may produce a stale lockfile that fails the `lockfile-drift` CI check.

**Valid**:
- `feature/63-branch-name-enforcement`
- `fix/42-pty-resize-on-windows`
- `bug/101-missing-idle-callback`

**Invalid**:
- `wip/63-foo` — `wip` is not a recognized type (see Branch type prefixes table)
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

<!-- Status: as of issue #93, Phase 1 only. Phase 2 (UI dropdown) and Phase 3 (live reload via tracing-subscriber) are aspirational and may or may not ship. -->

## Log filter precedence

The runtime log filter is resolved at startup via this chain:

1. `RUST_LOG` environment variable (if set) — used as the filter expression. Backwards compatible; preferred for ad-hoc debugging from a terminal.
2. `settings.logLevel` field in `~/.agentscommander*/settings.json` (if `Some`) — used as the filter expression. Persistent across restarts, survives Windows GUI launches (shortcut/double-click).
3. Default: `agentscommander=info`.

Filter expressions follow standard `env_logger` syntax (e.g. `info,agentscommander_lib::config::teams=trace`).

⚠️ **Caveat — malformed filters silently suppress agentscommander logs.** If the value does not parse as a valid env_logger filter (e.g., typo, unrecognized level keyword, single `:` instead of `::`), no matching directives are produced for `agentscommander*` targets and all `agentscommander*` logs are suppressed at runtime. Verify your filter once with `RUST_LOG=<filter> agentscommander_mb.exe` from a terminal before persisting it in `settings.json`. This is the same behavior the binary had pre-#93 for malformed `RUST_LOG` values — Phase 1 of #93 does not change this.

Phase 2 of #93 (if shipped) will surface this in the sidebar UI; Phase 3 (if shipped) will move to live reload via `tracing-subscriber`.

## In-code comments

When you add or update source comments, follow the
[in-code comment convention](docs/in-code-comments.md).
