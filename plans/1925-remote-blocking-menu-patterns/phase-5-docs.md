# #1925 Phase 5: privacy entry and user docs for remote blocking-menu patterns

Status: READY_FOR_IMPLEMENTATION

- Parent issue: https://github.com/mblua/AgentsCommander/issues/1925. Child issue: created by `ac-tech-lead-v4`; its number arrives in your Step 8 handoff.
- Branch: `feature/<child-issue>-docs`, exactly as named in the Step 8 handoff, cut from `origin/main` at this phase's Step 8.
- Drift baseline: `main` = `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, where this plan was verified. Every line number below is pinned to it. It is not the branch point.
- Class: `patterned`. The privacy entry mirrors `### npm Update Check` in `PRIVACY.md:46-54`. Owner: `ac-technical-writer-v4`.
- Depends on: phase 2 (the `remoteBlockingMenusEnabled` key) and phase 4 (the checkbox label). Parallel with: phase 3. Phase 6 depends on this phase.
- Contract: none. Documentation only.
- Delivery: commit on the phase branch and report the head SHA to `ac-tech-lead-v4`. Push only as the Step 8 handoff says. `ac-tech-lead-v4` opens and lands the PR and collects the exact-head CI evidence.

## 1. Objective

Document the new automatic network call and the new pattern layer so that no statement contradicts the code or the #1924 privacy wording. You own the wording; every fact below is fixed.

This phase lands before the download (phase 6), so no state of `main` ever makes the request without its `PRIVACY.md` entry. Until phase 6 lands, the entry describes a request the app does not make yet; disclosure never trails the request. Write in the present tense, as for the finished feature, and do not mention phases.

## 2. Before any write (mandatory, from the repo root)

1. Branch and base:
   ```
   git fetch origin main
   git rev-parse --abbrev-ref HEAD
   git rev-parse HEAD origin/main
   git status --porcelain
   ```
   The branch equals the Step 8 name and matches `^feature/[1-9][0-9]*-docs$`; both SHAs are equal; status prints nothing.
2. Landed dependencies, each must print as stated:
   ```
   grep -c 'pub remote_blocking_menus_enabled: bool' src-tauri/src/config/settings.rs   # 1 (phase 2)
   grep -c 'Download blocking-menu pattern updates from GitHub' src/sidebar/components/SettingsModal.tsx   # 1 (phase 4)
   ```
   Otherwise stop.
3. Drift:
   ```
   git cat-file -e "b4f7c9de7f15b75f8f12c034c8356f73babd0a8d^{commit}" || git fetch --deepen=200 origin main
   git diff --name-only b4f7c9de7f15b75f8f12c034c8356f73babd0a8d "$(git merge-base HEAD origin/main)"
   ```
   Expected entries: files of phases 1 to 4 of #1925 that have landed; none of them is in section 3. If the list names any file in section 3, stop and report it to `ac-tech-lead-v4`: #1924 wording or an anchor may have moved.
4. State the environment risk in writing to `ac-tech-lead-v4` before touching code: Windows host; `core.autocrlf=true` checks the section 3 files out CRLF (`git ls-files --eol` shows `w/crlf`), but the section 7 greps are unanchored, so CRLF does not change their output; the clone is shallow; any other local risk you see.

## 3. Exact files

| File | Where |
|---|---|
| `PRIVACY.md` | new `###` entry after `### Home Panel Markdown` (`:56-64`); one bullet in `## What Is NOT Transmitted` (`:103-110`) |
| `docs/reference/settings.md` | `### Menu guard` (`:460-499`) and the related-links line `:548` |
| `docs/features/menu-guard.md` | sections named in 5.3 |
| `docs/reference/directory-layout.md` | files table rows at `:82-83` |

## 4. Fixed facts (quote these exactly where a name, value or string appears)

| Fact | Value |
|---|---|
| Endpoint | `https://raw.githubusercontent.com/mblua/AgentsCommander/main/remote-resources/blocking-menus/v1/settings-blocking-menus.json` |
| When | automatically at app startup, in a detached background task that never blocks or delays startup |
| Throttle | at most once per 24 hours. The last attempt time is stored in `blocking-menus-remote-check.json` in the config directory. Every attempt counts, a failed or offline one included, so the next try is 24 hours later. This differs from the npm update check, which does not cache a failed check: do not describe the two the same way. |
| Limits | 10-second timeout covering the request and the response body; response body capped at 64 KB |
| Validation | the whole file is rejected, never one entry, when any check fails: HTTP status other than 200, body over 64 KB, not valid JSON, `schemaVersion` other than 1, a non-empty `byAgent`, more than 200 entries, a pattern over 512 bytes, a notification over 200 bytes or containing a control character, a pattern that does not compile within its size limit, a pattern matching an empty line or a built-in sample of ordinary terminal lines |
| On rejection or failure | nothing is shown (no toast, no notice); the previously downloaded file stays and keeps applying |
| Stored copy | `settings-blocking-menus.remote.json` in the config directory; written only by the download, and only after the whole file passes; its `note` records the source URL, the download time and the source ref `main` |
| Re-validation | the stored copy is validated again at every start; if it fails, AC ignores it (one warning line in the log) and the shipped patterns apply |
| Takes effect | at the next start; the running app never reloads patterns |
| Data disclosed | your IP address, the request time, and a `User-Agent` header of `agentscommander/<version>`; no account, no identifier, no session content |
| Setting | `remoteBlockingMenusEnabled`, bool, default `true`, in `settings.json` |
| Settings label | **Download blocking-menu pattern updates from GitHub** (Settings, General) |
| Turning it off | stops the download only; a file already downloaded keeps applying until you delete `settings-blocking-menus.remote.json` |
| Precedence, first present wins and replaces every layer below it whole | (1) an array still on the agent; (2) `byAgent[id]` in `.local`; (3) `byCommand[stem]` in `.local`; (4) `byCommand[stem]` in `settings-blocking-menus.remote.json` (new); (5) `byCommand[stem]` in the shipped file; (6) nothing |
| Consequences | a remote array for a stem replaces the shipped array for that stem, and can be `[]`; to override a remote entry, put that stem in `.local` `byCommand`; `byAgent` is never read from the remote file; materialized defaults and the #1905 migration still use shipped content only |
| Not in scope, do not promise | a "refresh now" action, applying without restart, a Settings UI for `.local` patterns, a switch for `menuGuardEnabled` |

## 5. Required changes

### 5.1 `PRIVACY.md`

- Add `### Remote Blocking-Menu Patterns` after `### Home Panel Markdown`, shaped like `### npm Update Check`: a bold **Automatic.** lead sentence, then bullets for Endpoint, When, Limits, Data disclosed, Stored (the cache and the stamp), and Turn it off, each with the section 4 facts. Say plainly that a rejected or failed download shows nothing and keeps the previous copy.
- In `## What Is NOT Transmitted`, extend the bullet that lists requests carrying no session content (`:109`) with the blocking-menu pattern download.
- Do not contradict #1924, and leave these statements true and unedited: the intro's "a small number of automatic outbound requests" (`:3`); `### Home Panel Markdown`, including "There is no setting for this request." (`:64`); the `### npm Update Check` text (`:46-54`); the GitHub entry already under `## Third-Party Services` (`:123`).

### 5.2 `docs/reference/settings.md`

- `### Menu guard` intro paragraph (`:462`): three blocking-menus files, not two. The download has a Settings checkbox; hand-editing `.local` is still the only way to add your own patterns.
- Flag table (`:464-466`): add a row for `remoteBlockingMenusEnabled` (bool, `true`, the download, at most once per 24 h, applies at next start, off stops the download only).
- File table (`:468-471`): add a row for `settings-blocking-menus.remote.json` (writer: AC's startup download; when: only after a whole-file validation pass).
- `BlockingMenusFile` heading (`:473`): all three files share the shape; add that the remote file must also have an empty `byAgent`.
- Precedence paragraph (`:482`): the six-step order from section 4.
- After the paragraph about a `.local` file with the wrong shape (`:497`), add one paragraph: the remote file is validated whole at every start and ignored whole with one warning when any check fails.
- Related-links line (`:548`): the words "the two `settings-blocking-menus` files" become "the three `settings-blocking-menus` files". Nothing else on that line changes.

### 5.3 `docs/features/menu-guard.md`

- `## What ships by default` (`:25-41`): the published file can update patterns between releases for a stem it names, applied at the next start after a download.
- `## The two files and their precedence` (`:129-152`): retitle to three files and fix "The patterns live in two files" (`:131`) and "Both files share one shape" (`:138`). Add the remote row to the table and step (4) to the ordered list, and state the whole-file validation and the `byAgent` rule. Keep the "Replace-whole has a cost" paragraph and add that a remote array replaces the shipped one the same way.
- `## Turning the guard off` (`:154-169`): a row or sentence saying that overriding a downloaded pattern for a command means writing that stem in `.local` `byCommand`, and that the checkbox stops downloads only.
- `## Settings` (`:198-207`): add the `remoteBlockingMenusEnabled` row and name the third file in "Two files next to `settings.json`" (`:205`).
- `## Troubleshooting` "I added a pattern and it does nothing." (`:213-219`): no change needed. "My agent stalls on a dialog and AC says nothing." (`:211`): mention that a downloaded file may add patterns for more stems after a restart.
- Update any other "two files" wording on the page that now reads false. Line 3's promise of how to turn the whole thing off stays true through `menuGuardEnabled`.

### 5.4 `docs/reference/directory-layout.md`

After the `settings-blocking-menus.local.json` row (`:83`) add two rows in the same three-column shape. The Source column names the modules that read and write the file, as the existing rows do: the `.local` row names `config/settings.rs`, not `config/instance_artifacts.rs`, where its file-name constant lives.

- `settings-blocking-menus.remote.json`: blocking-menu patterns downloaded from GitHub; written only by the startup download after the whole file passes validation, and validated again at every start. Source: `config/settings.rs`, `update_check.rs`.
- `blocking-menus-remote-check.json`: time of the last remote blocking-menu download attempt; limits the download to once per 24 hours. Source: `config/settings.rs`, `update_check.rs`.

## 6. Tests

Documentation only. The checks are in section 7.

## 7. Verification (from the repo root, report output)

```
grep -c 'https://raw.githubusercontent.com/mblua/AgentsCommander/main/remote-resources/blocking-menus/v1/settings-blocking-menus.json' PRIVACY.md
grep -c 'remoteBlockingMenusEnabled' PRIVACY.md docs/reference/settings.md docs/features/menu-guard.md
grep -c 'Download blocking-menu pattern updates from GitHub' PRIVACY.md docs/features/menu-guard.md
grep -c 'There is no setting for this request.' PRIVACY.md
grep -c 'settings-blocking-menus.remote.json' PRIVACY.md docs/reference/settings.md docs/features/menu-guard.md docs/reference/directory-layout.md
grep -c 'blocking-menus-remote-check.json' PRIVACY.md docs/reference/directory-layout.md
grep -c 'the two `settings-blocking-menus` files' docs/reference/settings.md
grep -c 'the three `settings-blocking-menus` files' docs/reference/settings.md
grep -n -i -E '(two|both) (blocking-menus )?files' docs/features/menu-guard.md docs/reference/settings.md
git diff --stat "$(git merge-base HEAD origin/main)" HEAD
```

- Every `grep -c` on the first, second, third, fifth and sixth lines prints at least 1 for every file named. `There is no setting for this request.` prints exactly 1. `the two ...` prints 0 and `the three ...` prints 1. The case-insensitive `(two|both) ... files` line prints nothing, or you report each remaining line and why it is still true. At the drift baseline it prints 6 lines: `menu-guard.md:129`, `:131`, `:138`, `:205` and `settings.md:462`, `:473`.
- `git diff --stat` lists only the 4 files of section 3 (ignore `plans/`).
- Every in-page link you add or keep (`#menu-guard`, `../features/menu-guard.md`, `../reference/settings.md#menu-guard`) resolves to an existing heading or file.

## 8. Acceptance criteria

1. Everything in section 7 holds.
2. Acceptance 8 of the issue: `PRIVACY.md` names the new endpoint and contradicts none of the #1924 statements listed in 5.1. The reviewer checks each of those four anchors by reading them.
3. Every value in section 4 that the docs mention matches section 4 character for character: URL, file names, key, label, limits, precedence order.

## 9. Preserve

- `docs/home-en.md` is not edited (no HTML comment; the Home renderer would show it as text).
- `docs/reference/directory-layout.md:51` keeps its line number and bytes (`scripts/room-rename-allowlist.mjs:243` keys that line).
- Existing `.local` and shipped-file statements stay true; change them only where the new layer makes them false.

## 10. Recovery

Restore only paths this phase changed that still hold this run's output (`git restore --source=HEAD -- <path>`). No `git reset`, `git clean` or repository-wide restore. Report external changes to `ac-tech-lead-v4`.
