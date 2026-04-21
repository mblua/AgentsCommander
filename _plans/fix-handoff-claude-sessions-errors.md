# Plan — fix-handoff-claude-sessions-errors

**Branch:** `fix/handoff-claude-sessions-errors` (rebased on `origin/main`, empty vs. origin/main)
**Repo path:** `C:\Users\maria\0_repos\agentscommander\.ac-new\wg-3-dev-team\repo-AgentsCommander`
**Requirement source:** `.ac-new/wg-3-dev-team/messaging/20260419-180859-wg3-tech-lead-to-wg3-architect-revised-scope.md`
**Author:** Architect
**Date:** 2026-04-19
**Supersedes:** previous draft of this file (docs-only + out-of-repo CLI speculation). That version is obsolete.

---

## 1. Scope statement

Prevent Error #1 only — full path passed to `send --send`, rejected with `filename '...' contains path separators or traversal`. Three distinct agent sessions hit this.

Two edits, both in-repo:

- **Edit A (primary)** — Agent-context template at `src-tauri/src/config/session_context.rs::default_context`. Adds explicit basename-only rule + BAD/GOOD pair. Fixes all coding agents (Claude, Gemini, Codex) at once because they all read this materialized file on session start.
- **Edit B (secondary, complements A)** — CLI normalization in `src-tauri/src/phone/messaging.rs::resolve_existing_message`. Accept absolute/relative paths when they canonically resolve inside `<workgroup-root>/messaging/`; strip to basename and continue existing validation. Reject everything else (existing security posture preserved).
- **Edit C (optional)** — Sync `repo-AgentsCommander/CLAUDE.md` messaging subsection with the same emphasis. Low priority — primary audience is human contributors.

Errors #2(a) `git` rev-parse guard and #2(b) `gh --body-file` — **out of scope** per revised handoff (single occurrences, separate branches later).

---

## 2. Investigation log (post-rebase)

After `git fetch origin` on 2026-04-19, `origin/main` carries the landed file-based messaging feature:

| Commit | Subject |
|---|---|
| `03141b9` | feat: file-based inter-agent messaging (--send replaces --message) |
| `07b4360` | fix(messaging): tighten shape validator + audit PTY overhead constant |
| `24b160f` | fix(messaging): single-source reply-hint template via reply_hint! macro |
| `ea46672` | feat: trim PTY overhead + raise PTY_SAFE_MAX to 1024 |
| `4d0d215` | fix(cleanup): remove dead resolve_bin_label after reply-hint trim |
| `32b531c` | feat: delete active-only delivery mode |
| `81877b5` | feat: delete wake-and-sleep delivery mode + collapse dispatch |
| `b866923` | feat: remove busy-gate from wake + extract decision helper + bump 0.7.0 |
| `313b71e` | fix(context): unblock messaging dir writes + de-prioritize --help hint |

Confirmed in-repo:
- `--send` flag → `src-tauri/src/cli/send.rs:33-34` (`pub send: Option<String>`).
- Error definition → `src-tauri/src/phone/messaging.rs:36-37` (`InvalidFilename`).
- Validation call site → `src-tauri/src/cli/send.rs:161` (`resolve_existing_message(&msg_dir, filename)`).
- Validation logic → `src-tauri/src/phone/messaging.rs:228-260` (`resolve_existing_message`). Rejection on `/`, `\`, `..` at lines 232-233.
- Tests exist → `resolve_rejects_traversal` at `messaging.rs:477-500`, `create_and_resolve_round_trip` at `messaging.rs:442-474`.
- Agent-context template → `src-tauri/src/config/session_context.rs::default_context` at lines 346-446. Messaging subsection at 414-442.
- Repo CLAUDE.md → messaging subsection at `CLAUDE.md:41-60` already mentions "filename (not path)" as a bullet, without BAD/GOOD emphasis.

---

## 3. Affected files

| # | File | Change type | Anchor lines |
|---|---|---|---|
| A | `src-tauri/src/config/session_context.rs` | Insert enforcement paragraph + BAD/GOOD block inside `default_context()` raw string | ~430 (after the `send --send` command block, before "The recipient receives …") |
| B | `src-tauri/src/phone/messaging.rs` | Rewrite leading separator/traversal check in `resolve_existing_message` to normalize path args inside `messaging_dir` | 228-234 |
| B-tests | `src-tauri/src/phone/messaging.rs` | Extend `#[cfg(test)] mod tests` with 5 new cases | after line 500 (end of `resolve_rejects_traversal`) |
| C (opt) | `CLAUDE.md` (repo root) | Add BAD/GOOD emphasis to the existing `--send` bullet | 50-58 |

No Cargo.toml changes. No new imports. No TypeScript changes.

---

## 4. Concrete edits

### 4.A — `src-tauri/src/config/session_context.rs`

**Anchor — current content, lines 427-436 inclusive:**

```rust
2. Fire the send:

```
"<YOUR_BINARY_PATH>" send --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --to "<agent_name>" --send <filename> --mode wake
```

The recipient receives a short notification pointing to your file's absolute
path and reads the content via filesystem. Do NOT use `--get-output` — it
blocks and is only for non-interactive sessions. After sending, stay idle and
wait for the reply.
```

**Proposed — insert the emphasis block between the fenced command and the "The recipient …" paragraph:**

```rust
2. Fire the send:

```
"<YOUR_BINARY_PATH>" send --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --to "<agent_name>" --send <filename> --mode wake
```

**IMPORTANT: `--send` takes the filename ONLY — never a path.**

- BAD:  `--send "C:\...\messaging\20260419-143052-wg3-you-to-wg3-peer-hello.md"`
- GOOD: `--send "20260419-143052-wg3-you-to-wg3-peer-hello.md"`

The CLI resolves the filename against `<workgroup-root>/messaging/` automatically. Passing a path triggers `filename '...' contains path separators or traversal`.

The recipient receives a short notification pointing to your file's absolute
path and reads the content via filesystem. Do NOT use `--get-output` — it
blocks and is only for non-interactive sessions. After sending, stay idle and
wait for the reply.
```

**Dev notes:**
- The file is a Rust raw string `r#"..."#`. Backticks, `{`, and `}` in the inserted block are literal — no escaping needed inside `r#""#`. The only forbidden sequence is `"#`. None of the added text contains it.
- Do NOT alter the surrounding prose or the `### List available peers` section below.
- After Edit B lands, the line "Passing a path triggers …" becomes strictly correct for direct traversal (e.g. `../outside/foo.md`) but will NOT fire for an absolute path inside `messaging/`. That nuance is intentional: the enforcement in Edit A is the protocol rule; Edit B adds tolerance as a safety net. Keep the text as proposed — overspecifying the CLI's internal normalization in the agent-facing doc creates a footgun ("they said I can pass paths, why did it fail now?"). The rule "filename only" remains unambiguous.

### 4.B — `src-tauri/src/phone/messaging.rs`

**Anchor — current content, lines 228-234 inclusive:**

```rust
pub fn resolve_existing_message(
    messaging_dir: &Path,
    filename: &str,
) -> Result<PathBuf, MessagingError> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err(MessagingError::InvalidFilename(filename.to_string()));
    }
```

**Proposed — replace the guard (lines 232-234) with normalization + strict guard:**

```rust
pub fn resolve_existing_message(
    messaging_dir: &Path,
    filename: &str,
) -> Result<PathBuf, MessagingError> {
    // Reject traversal markers outright — defense in depth. `canonicalize`
    // would also resolve `..`, but an explicit reject is filesystem-independent
    // and keeps the error boundary crisp.
    if filename.contains("..") {
        return Err(MessagingError::InvalidFilename(filename.to_string()));
    }

    // Normalize path-shaped arguments to basename when the path canonically
    // resolves inside `messaging_dir`. Anything else is rejected with the
    // existing InvalidFilename error.
    let normalized_owned: String;
    let filename: &str = if filename.contains('/') || filename.contains('\\') {
        let as_path = Path::new(filename);
        let parent = as_path
            .parent()
            .ok_or_else(|| MessagingError::InvalidFilename(filename.to_string()))?;
        let canon_msg_dir = std::fs::canonicalize(messaging_dir)?;
        let canon_parent = std::fs::canonicalize(parent)
            .map_err(|_| MessagingError::InvalidFilename(filename.to_string()))?;
        if canon_parent != canon_msg_dir {
            return Err(MessagingError::InvalidFilename(filename.to_string()));
        }
        normalized_owned = as_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| MessagingError::InvalidFilename(filename.to_string()))?
            .to_string();
        &normalized_owned
    } else {
        filename
    };
```

**Dev notes:**
- Keep all existing downstream validation intact: `.md` suffix check at current line 235, `validate_filename_shape` at 238, `candidate.exists()` at 241, canonicalize/parent compare at 245-253, `is_file` at 255. They run unchanged against the (possibly rewritten) `filename`. The symlink-escape check at 248-253 continues to run on the basename-joined path — security posture preserved.
- `normalized_owned: String` is the lifetime trick: the shadowed `filename: &str` borrows from it when a rewrite happened; otherwise borrows from the original `&str` arg. Avoids allocating on the basename path (no separators) and avoids `Cow<str>` churn.
- `Path::parent()` returns `Some("")` for bare filenames — but we only take this branch when separators are present, so parent is always a real directory component.
- `std::fs::canonicalize` on Windows returns `\\?\`-prefixed paths on both sides, so `==` works. No manual strip needed (unlike the UNC-strip in `cli/send.rs:171` which is for display, not comparison).
- If `messaging_dir` itself cannot be canonicalized, the original `?` propagates as `MessagingError::Io` — same behavior as today at line 246. Keep it. Rejecting the whole send on a broken messaging dir is the right failure.
- No new `use` statements required (`Path` is already imported at line 9).

### 4.B-tests — extend `#[cfg(test)] mod tests`

**Anchor** — immediately after `resolve_rejects_traversal` at line 500. Add five named tests:

```rust
#[test]
fn resolve_accepts_abs_path_inside_messaging_dir() {
    let tmp = std::env::temp_dir().join(format!(
        "ac-msg-abs-ok-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();

    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
    let base = build_filename(ts, "wg7-a", "wg7-b", "abs-ok");
    let (written_abs, f) = create_message_file(&tmp, &base).unwrap();
    drop(f);

    // Pass the absolute path. Expect normalization + success.
    let abs_str = written_abs.to_string_lossy().to_string();
    let resolved = resolve_existing_message(&tmp, &abs_str).unwrap();
    assert_eq!(
        resolved.file_name().and_then(|n| n.to_str()).unwrap(),
        base
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn resolve_rejects_abs_path_outside_messaging_dir() {
    let tmp_msg = std::env::temp_dir().join(format!(
        "ac-msg-abs-out-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let tmp_other = std::env::temp_dir().join(format!(
        "ac-msg-abs-other-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp_msg).unwrap();
    std::fs::create_dir_all(&tmp_other).unwrap();

    // Real file, real .md name, but parent != messaging_dir.
    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
    let base = build_filename(ts, "wg7-a", "wg7-b", "other");
    let bad_path = tmp_other.join(&base);
    std::fs::write(&bad_path, b"x").unwrap();

    let bad_abs = bad_path.to_string_lossy().to_string();
    assert!(matches!(
        resolve_existing_message(&tmp_msg, &bad_abs),
        Err(MessagingError::InvalidFilename(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp_msg);
    let _ = std::fs::remove_dir_all(&tmp_other);
}

#[test]
fn resolve_rejects_abs_path_with_dotdot() {
    let tmp_msg = std::env::temp_dir().join(format!(
        "ac-msg-dd-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp_msg).unwrap();

    // Even if the `..` segment would canonically collapse back into messaging_dir,
    // we reject the input string form so the guard is filesystem-independent.
    let sneaky = format!("{}/../{}/foo.md",
        tmp_msg.display(),
        tmp_msg.file_name().and_then(|n| n.to_str()).unwrap());
    assert!(matches!(
        resolve_existing_message(&tmp_msg, &sneaky),
        Err(MessagingError::InvalidFilename(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp_msg);
}

#[test]
fn resolve_rejects_abs_path_with_missing_parent() {
    let tmp_msg = std::env::temp_dir().join(format!(
        "ac-msg-missing-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp_msg).unwrap();

    // Parent directory does not exist. canonicalize(parent) fails → reject.
    let bogus = std::env::temp_dir().join("no-such-dir-xyz").join(
        "20260419-143052-wg7-a-to-wg7-b-nope.md",
    );
    let bogus_abs = bogus.to_string_lossy().to_string();
    assert!(matches!(
        resolve_existing_message(&tmp_msg, &bogus_abs),
        Err(MessagingError::InvalidFilename(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp_msg);
}

#[test]
fn resolve_accepts_relative_path_inside_messaging_dir() {
    let tmp = std::env::temp_dir().join(format!(
        "ac-msg-rel-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();

    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
    let base = build_filename(ts, "wg7-a", "wg7-b", "rel-ok");
    let (abs_written, f) = create_message_file(&tmp, &base).unwrap();
    drop(f);

    // Construct a relative form like `./file.md` from the absolute path.
    // `canonicalize` of `"."` against messaging_dir's parent doesn't match,
    // so the relative form is accepted ONLY when parent canonicalizes to
    // messaging_dir. Here we pass `<tmp>/./<base>` to exercise the happy case.
    let rel = format!("{}/./{}", tmp.display(), base);
    let resolved = resolve_existing_message(&tmp, &rel).unwrap();
    assert_eq!(
        resolved.file_name().and_then(|n| n.to_str()).unwrap(),
        base
    );
    // Silence unused-variable warning if abs_written is not otherwise used.
    let _ = abs_written;

    let _ = std::fs::remove_dir_all(&tmp);
}
```

**Test design notes for dev-rust:**
- The existing `resolve_rejects_traversal` already covers `foo/bar.md`, `..`, forward- and back-slash inputs that do NOT resolve inside `messaging_dir`. Post-Edit B those inputs still reject (either via the early `..` guard or via `canonicalize(parent)` failing on the nonexistent parent). Keep that test untouched so we prove no regression.
- `create_and_resolve_round_trip` already covers basename-only happy path — no change needed.
- On Windows, temp dirs returned by `std::env::temp_dir()` canonicalize to `\\?\`-prefixed forms. Both sides of the `==` get canonicalized inside `resolve_existing_message`, so tests pass on Windows and Unix without special-casing.
- `resolve_rejects_abs_path_with_dotdot` uses a `..` that would otherwise canonically collapse to the messaging dir — the early-reject guard prevents that shortcut, which matches tech-lead's security-preservation intent.
- `resolve_accepts_relative_path_inside_messaging_dir`: dev may find that on some platforms `canonicalize` on `<tmp>/./` normalizes to `<tmp>`; if the equality fails on Windows due to `\\?\` edge cases, either (a) drop this test as over-specified, or (b) tighten the test to only assert success, not the exact returned filename. The primary coverage is abs-path; relative-path is a bonus.

### 4.C (optional) — `CLAUDE.md:50-58`

**Anchor — current:**

```markdown
```bash
agentscommander.exe send --token <TOKEN> --root "<CWD>" --to "<agent_name>" --send <filename> --mode wake
```

- `--token`: your session token (provided in the Session Init block injected into your console)
- `--root`: your working directory (must be under a `wg-<N>-*` ancestor)
- `--to`: target agent name — **must be verified first** via `list-peers` or `teams.json`
- `--send`: filename (not path) of a file you already wrote under `<workgroup-root>/messaging/`
- `--mode wake`: fire-and-forget, do NOT use `--get-output` (blocks interactive sessions)
```

**Proposed — tighten the `--send` bullet and add BAD/GOOD directly below the list:**

```markdown
```bash
agentscommander.exe send --token <TOKEN> --root "<CWD>" --to "<agent_name>" --send <filename> --mode wake
```

- `--token`: your session token (provided in the Session Init block injected into your console)
- `--root`: your working directory (must be under a `wg-<N>-*` ancestor)
- `--to`: target agent name — **must be verified first** via `list-peers` or `teams.json`
- `--send`: **filename ONLY, never a path**. The CLI resolves it against `<workgroup-root>/messaging/` automatically.
  - BAD:  `--send "C:\...\messaging\20260419-...-hello.md"`
  - GOOD: `--send "20260419-...-hello.md"`
- `--mode wake`: fire-and-forget, do NOT use `--get-output` (blocks interactive sessions)
```

Edit C is marked optional. Skip if time-pressured — Edit A covers agent-facing surface area.

---

## 5. Test considerations (per tech-lead §73)

Coverage matrix for `resolve_existing_message` after Edit B:

| Case | Input | Expected | Covered by |
|---|---|---|---|
| (a) basename only | `"<base>.md"` | Ok | `create_and_resolve_round_trip` (existing) |
| (b) abs path inside messaging | `"<tmp>/<base>.md"` | Ok, returned filename == base | `resolve_accepts_abs_path_inside_messaging_dir` (new) |
| (c) abs path outside | `"<other>/<base>.md"` | Err(InvalidFilename) | `resolve_rejects_abs_path_outside_messaging_dir` (new) |
| (d) `..` traversal | `"../etc/passwd"`, `"<tmp>/../<tmp>/foo.md"` | Err(InvalidFilename) | `resolve_rejects_traversal` (existing) + `resolve_rejects_abs_path_with_dotdot` (new) |
| (e) symlink escape | (fs-setup required) | Err(InvalidFilename) from existing parent-canon compare | Not added — existing check at 248-253 suffices and is orthogonal to Edit B |
| (f) missing parent dir | `"<bogus>/<base>.md"` | Err(InvalidFilename) | `resolve_rejects_abs_path_with_missing_parent` (new) |
| (g) relative path inside | `"<tmp>/./<base>.md"` | Ok | `resolve_accepts_relative_path_inside_messaging_dir` (new, bonus) |
| (h) dir (not file) arg | (existing test) | Err(NotAFile) | `resolve_rejects_directory_with_md_suffix` (existing) — unaffected by Edit B |

Run: `cd src-tauri && rtk cargo test -p <crate-name> phone::messaging` after Edit B + tests land. Full crate test suite should also pass.

No new integration / Tauri / frontend tests required — Edit A is doc-only inside a raw string (no behavior change), Edit C is doc-only in `CLAUDE.md`.

---

## 6. Risks / open questions

### Q1 — `canonicalize` on Windows junction points / symlinks
If `messaging_dir` or the parent of the passed path crosses a junction/symlink, canonicalization follows it. That's consistent with the existing symlink-escape check at `messaging.rs:248-253`, so behavior is uniform: a symlink pointing from inside `messaging_dir` to outside would fail Edit B's parent-compare, and the existing check catches it for basename inputs. No new risk.

### Q2 — Error code parity
The existing basename-only branch emits `InvalidFilename(filename)` with the raw arg string inside the error. Edit B preserves that — on rejection, the error carries the original (path-shaped) input, not the rewritten basename. Tech-lead's "existing error preserves current security posture" language covers this; explicitly calling out that debugging an outside-dir rejection shows the full path in the error message (which is fine for an error stream, not a security issue).

### Q3 — Agent-facing doc: do we mention the normalization tolerance?
Edit A proposes NOT mentioning that Edit B normalizes paths inside `messaging/`. The agent-facing rule stays "filename only." Tech-lead: confirm this is the intended framing. Alternative: append a one-liner like "Absolute paths that point inside this directory are also accepted, but the filename-only form is mandatory." Recommend NOT adding — creates ambiguity the protocol rule doesn't need. Decision requested.

### Q4 — Edit C inclusion
Edit C is marked optional per tech-lead §55. Default: include it for consistency (CLAUDE.md and the agent template drift otherwise, reviewers will notice in PR). Overriding default and skipping Edit C is fine if the reviewer considers it bikeshedding. Recommend include.

No other blocking issues. Dev-rust can proceed once Q3 is resolved (Q4 is cosmetic).

---

## 7. Dependencies

None. No new crates, no new imports, no Cargo.toml changes. `std::fs::canonicalize` and `std::path::Path` are already used in the same function.

---

## 8. Notes / constraints for the implementer (dev-rust)

- Branch `fix/handoff-claude-sessions-errors` is already created and rebased on `origin/main`. Do NOT branch again.
- Apply in order: Edit B logic → Edit B tests → `cargo test phone::messaging` passes → Edit A → Edit C (if included).
- Rebuild after Edit B before testing Edit A materialization: agent-context template is rendered at session start; to verify visually, spawn a replica agent after the build and inspect its injected `CLAUDE.md`.
- Do NOT touch `validate_filename_shape`, `create_message_file`, or any other function in `messaging.rs`. Scope is strictly `resolve_existing_message` + new tests.
- Do NOT add a bullet for the `--send` flag to `cli/send.rs`'s `after_help` — the agent-facing surface is `session_context.rs`, not clap's help text. Clap help is a FALLBACK per the agent context doc.
- Expected diff size: ~40 lines in `messaging.rs` (logic + 5 tests), ~8 lines in `session_context.rs`, ~4 lines in `CLAUDE.md`. If the diff balloons past ~100 LOC you have drifted off-plan — stop and re-read this plan.
- Required review passes per repo `CLAUDE.md ### Change Validation Protocol`: run `feature-dev:code-reviewer` pre- and post-implementation. Grinch will adversarially review after that.
- Commit message suggestion: `fix(messaging): normalize absolute paths inside messaging/ + emphasize filename-only in agent context`. Keep it one commit. Do not merge to main or push unless the user instructs.

---

## 9. Dev-rust enrichment (2026-04-19)

All additions verified against the live code on `fix/handoff-claude-sessions-errors` (repo SHA rebased on `origin/main`). Reasoning given for each.

### 9.1 — Pre-decisions locked (tech-lead §27-30)

- **Q3**: strict "filename ONLY, never a path" in Edit A stays. Edit B is silent safety net. Do NOT add "paths inside messaging/ are tolerated" to agent-facing docs. → §4.A text is final.
- **Q4**: Edit C INCLUDED. → §4.C is no longer optional; treat as mandatory.

**Why:** locks scope before implementation. Removes "recommend include/exclude" ambiguity in §6; the implementer must not revisit.

### 9.2 — Implementation ordering (answer to §34.1)

Confirmed order: Edit B logic → Edit B tests → `cargo test phone::messaging` passes → existing `resolve_rejects_traversal` passes unchanged (regression gate, see §9.6) → Edit A → Edit C → final `cargo test` + `cargo clippy`.

**Added gate between steps 3 and 4:** if `resolve_rejects_traversal` fails after Edit B, STOP and diagnose. That test is the contractual regression boundary — new tests passing while it fails means Edit B relaxed security, not just ergonomics. Do not "fix" it by editing the old test; fix Edit B.

**Why:** §34.1 asked about ordering. The subtle risk is the existing test accidentally masking a real regression if a dev reflexively "updates" it to match new behavior. Calling it out as an immovable gate prevents that slide.

### 9.3 — Lifetime trick verification (answer to §34.2)

The proposed form compiles cleanly:

```rust
let normalized_owned: String;
let filename: &str = if filename.contains('/') || filename.contains('\\') {
    // ... uses original `filename` (parameter) inside this block ...
    normalized_owned = as_path.file_name()... .to_string();
    &normalized_owned
} else {
    filename
};
```

Key observations:
- `normalized_owned` is declared in the outer scope → its lifetime covers the entire function body.
- The shadow `let filename: &str = ...` takes effect at the `let`'s semicolon. Inside the `if`-block's body (before the `;`), `filename` still refers to the parameter. That's exactly what the in-branch `MessagingError::InvalidFilename(filename.to_string())` calls want: they need the original path-shaped input, not the basename.
- On the else-branch the binding `filename` (new `&str`) reborrows from the parameter `filename: &str` — trivially valid.
- Borrow checker: `normalized_owned` outlives the shadow `filename: &str`, so `&normalized_owned` stored in the shadowed binding is valid for the rest of the function.

**Simpler alternatives evaluated and rejected:**
- `Cow<'_, str>`: forces `Cow<str>` on every downstream call site or an `.as_ref()` sprinkle. Worse than the current shadow.
- `String` with always-allocate: loses zero-alloc on the common basename path. The current code is called per send; allocating a fresh `String` on each hot-path call is wasteful when 100% of valid-protocol agent calls are basename-only.
- Returning early from the if with normalized owned string, then reassigning: requires a mut binding and an extra temporary — strictly uglier.

Keep the plan's form as-is. **No change.**

**Why:** §34.2 asked whether it compiles and whether something simpler exists. Answer: yes compiles, no simpler form worth the ergonomic loss.

### 9.4 — Windows `canonicalize` edge cases (answer to §34.3)

Verified behavior for each Windows concern:

| Concern | Behavior | Impact on Edit B |
|---|---|---|
| Junction points in messaging_dir's path | Followed; both sides canonicalize to target → `==` works | No change; matches existing 248-253 posture |
| UNC `\\?\` prefix | Applied uniformly on Windows by `std::fs::canonicalize` | `==` works without manual strip |
| Long paths (>260 chars) | `canonicalize` auto-prefixes with `\\?\` → supported | No config required (no relation to git's `core.longpaths`) |
| Network drives / UNC share | Returns `\\server\share\...` form | `==` works when both sides point at same share |
| Case sensitivity | Windows FS case-insensitive; `canonicalize` normalizes to on-disk case | `==` works regardless of user-supplied casing |
| Non-existent parent | Returns `Err(Io)` | Plan maps it to `InvalidFilename` via `.map_err(...)` at line 4.B. Correct. |
| Removed/locked messaging_dir mid-call | Propagates `Err(Io)` via `?` on line `canon_msg_dir = canonicalize(messaging_dir)?` | Matches existing behavior at current line 246. Acceptable. |

**Caveat to flag (non-blocking):** if `messaging_dir` ITSELF is a symlink/junction whose target is not the logical `messaging/` folder, Edit B will canonicalize both sides to the target and accept. That's the same policy as the existing 248-253 check — the function never distinguishes "logical" vs "physical" roots. Out of scope here.

**Why:** §34.3 asked for verification. The table is the verification. The non-blocking caveat documents a known limit already present pre-Edit B, so it is not a regression.

### 9.5 — Test isolation (answer to §34.4)

`Utc::now().timestamp_nanos_opt()` has nanosecond type width but the OS clock source on Windows typically ticks at ~100 ns (QueryPerformanceCounter). `cargo test` spawns threads in parallel by default. Two tests that hit `Utc::now()` in the same 100-ns tick and use the same static prefix would collide on `std::fs::create_dir_all` — not a hard error (idempotent), but the `remove_dir_all` at test end becomes a race.

**Recommended tightening:** append a thread-id hash to each temp dir name:

```rust
fn unique_tmp(prefix: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut h);
    std::env::temp_dir().join(format!(
        "{}-{}-{}",
        prefix,
        Utc::now().timestamp_nanos_opt().unwrap_or(0),
        h.finish()
    ))
}
```

Call it from each new test: `let tmp = unique_tmp("ac-msg-abs-ok");`.

Apply the same helper to the existing `create_and_resolve_round_trip`, `resolve_rejects_traversal`, and `resolve_rejects_directory_with_md_suffix` only if the diff stays in scope — otherwise leave them (they've been stable upstream). Safest: only wrap the five new tests in `unique_tmp`; keep existing tests untouched to honor "no regression" and keep diff small.

**Why:** §34.4 flagged collision risk. Nanos alone give ~100ns resolution; thread-id tiebreaker is 1 line of helper + 1 line per test and collapses the race window to zero. Per-test distinct prefix already reduces collision odds, but explicit thread suffix is cheap insurance on CI.

### 9.6 — Existing `resolve_rejects_traversal` post-Edit B (answer to §34.5)

Walked each input against the post-Edit B control flow:

| Input | Path taken | Rejection source | Still `is_err()`? |
|---|---|---|---|
| `"../etc/passwd"` | Early `..` guard | `InvalidFilename` | ✓ |
| `"foo/bar.md"` | `/` present → path branch. `parent = "foo"` (relative). `canonicalize("foo")` returns `Err` (no such dir under test cwd). `.map_err(...)` → `InvalidFilename` | `InvalidFilename` | ✓ |
| `r"foo\bar.md"` | Windows: `\` separator, same as above. Unix: `\` not a separator, `Path::new("foo\\bar.md").parent()` returns `Some("")`. `canonicalize("")` fails → `InvalidFilename` | `InvalidFilename` | ✓ |
| `"foo.txt"` | No separators, no `..`. Skips path branch. Falls to `.md` suffix check at line 235 | `InvalidFilename` | ✓ |
| `"bare.md"` | No separators. Skips path branch. Passes `.md`. `validate_filename_shape("bare.md")` fails → `InvalidShape` | `InvalidShape` | ✓ (test uses `is_err()`, not variant match) |
| `".."` | Early `..` guard | `InvalidFilename` | ✓ |

**All six inputs still reject.** Zero regression. Test untouched.

**Note for the implementer:** `foo.txt` currently rejects via `.md` suffix check, and `bare.md` via shape — both pre-existing behaviors that Edit B does not alter. Do NOT add an `.md` precheck inside the path branch — it's redundant and would duplicate the suffix check downstream.

**Why:** §34.5 asked for a walkthrough. The table is the walkthrough. The note prevents an easy over-engineering mistake.

### 9.7 — Symlink-escape preservation (answer to §34.6)

Plan claims lines 248-253 still run unchanged post-Edit B. Verified:

- When Edit B rewrites `filename` to basename, the downstream `candidate = messaging_dir.join(filename)` at line 240 produces exactly the same path shape as the pre-Edit basename happy path.
- `canonicalize(candidate)` at line 245 resolves `candidate` through any symlinks. If `messaging_dir/basename` is a symlink pointing outside, `abs` resolves outside, `abs_parent != canon_dir`, rejection fires at 252.
- Geometry identical to pre-Edit. Security posture preserved.

**Edge case worth a test (NOT in current plan, recommended add):**

```rust
#[test]
fn resolve_rejects_symlinked_abs_path_pointing_outside() {
    // Only meaningful on Unix — Windows symlink creation needs dev-mode or admin.
    #[cfg(unix)] {
        // Create messaging_dir with a symlink named <base>.md → /tmp/outside/x.md.
        // Pass the abs path "<messaging_dir>/<base>.md".
        // Expect Err(InvalidFilename) from the existing 248-253 check.
    }
}
```

**Recommendation:** skip adding this test. It gates on `#[cfg(unix)]` and Windows-only CI won't exercise it. The existing symlink-escape path is orthogonal to Edit B and has no test today; adding one now expands scope beyond "Edit B didn't regress anything".

**Why:** §34.6 asked to confirm geometry. Confirmed. Declining the bonus test is scope discipline.

### 9.8 — Raw-string safety in Edit A (answer to §34.7)

`default_context` uses `r#"..."#` (line 348, verified). Inserted block (plan §4.A) contains:
- Backticks — literal inside `r#""#`. Safe.
- No `"#` sequence — checked character-by-character. The only `"` characters appear inside backticks and are followed by backticks or alphanumerics, never by `#`.
- No literal `{` or `}` — the outer `format!` treats `{agent_root}` as interpolation; adding stray `{` or `}` would require `{{`/`}}`. The inserted block has neither.
- No `\` escapes needed — raw strings treat `\` literally.

**Safe to insert as proposed.**

**One stylistic nit:** the existing template uses fenced code blocks with plain ` ``` ` (no language tag). The proposed insert is consistent. Keep it. Do NOT switch to ` ```bash ` or ` ```rust ` — breaks the template's uniform style and produces a noisy diff.

**Why:** §34.7 asked for delimiter verification. Verified. Stylistic nit prevents a drive-by formatting change during implementation.

### 9.9 — Error-message clarity (answer to §34.8)

Post-Edit B, an abs path outside `messaging/` rejects with `filename '<full-path>' contains path separators or traversal`. Technically correct (the path does contain `/` or `\`), but confusing to a reader who thinks "I passed an absolute path, not a traversal."

**Recommendation:** ship Edit B as-is. Do NOT extend `InvalidFilename` with a `reason: &'static str` or add a new variant in this change. Why:
- Scope creep. The error is hit only when the agent-facing rule (filename-only) is already broken. Edit A fixes the root cause; Edit B catches stragglers.
- `MessagingError` changes ripple through `phone/*` callers that match on the variant — Grinch review would flag it as out-of-plan.
- Noisy diff. Better to track as a follow-up: open a tech-debt ticket after this branch merges. Rename the variant or add a reason field in a dedicated change.

**Action item for tech-lead (do NOT implement now):** after this branch lands, open a short issue titled "Enrich `InvalidFilename` with a reason field" referencing this plan's §9.9.

**Why:** §34.8 asked whether to address now. Answer: no, tracked as future work. Current change remains scoped.

### 9.10 — Commit boundary (answer to §34.9)

**Decision: one commit.** Rationale:
- Total diff ~40 LOC messaging (logic + 5 tests) + ~8 LOC session_context + ~4 LOC CLAUDE.md ≈ 52 LOC. Well inside the "atomic change" threshold.
- All three edits share one motivation: "passing a path to `--send` no longer fails loudly AND docs say 'don't'". Splitting obscures that motivation in git history.
- No edit depends on the others compiling — Edit A and Edit C are pure doc; Edit B is pure logic. But they must land together to avoid a window where the CLI normalizes paths silently while docs still say "forbidden" (confusing) or where docs emphasize "filename only" but the old rejection error still fires on path-shaped inputs (noisy).
- Test file churn is ~70 LOC of new tests in the same `mod tests` — trivial to review in a single commit.

Commit message (plan's suggestion is good, minor polish):

```
fix(messaging): accept path-shaped --send args that resolve inside messaging/

- resolve_existing_message: normalize abs/relative paths to basename when
  canonically inside messaging_dir; reject everything else.
- session_context default_context: add filename-only rule + BAD/GOOD pair.
- CLAUDE.md: sync filename-only emphasis for human contributors.
- Add 5 tests covering abs-inside, abs-outside, `..` guard, missing-parent,
  relative-inside.
```

**Why:** §34.9 asked for a call. The call is "one commit". Polished message shipped.

### 9.11 — Build & verification commands (answer to §34.10)

Exact command sequence, run from `repo-AgentsCommander/src-tauri/`:

```bash
# 1. Logic edit applied
rtk cargo check                                   # fast type check
rtk cargo test phone::messaging                   # targeted (fast)

# 2. After all edits (A, B, C) applied
rtk cargo test                                    # full suite
rtk cargo clippy -- -D warnings                   # lint, deny-on-warn

# Optional final sanity
rtk cargo build                                   # debug build
```

Single crate (`agentscommander-new`, confirmed via `cargo pkgid`). No `-p` flag needed. No workspace.

**Materialized `CLAUDE.md` eye-check (§34.10 "where to find"):** the agent context template is materialized via `default_context(agent_root)` when a replica is provisioned by the app. Existing replicas are NOT re-rendered on rebuild — only newly provisioned ones. Two options:

- **(a) deterministic, recommended):** add a tiny unit test in `session_context.rs` that asserts the BAD/GOOD substrings appear in the output:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_embeds_filename_only_warning() {
        let out = default_context("C:/tmp/fake-agent");
        assert!(out.contains("filename ONLY"));
        assert!(out.contains("BAD:"));
        assert!(out.contains("GOOD:"));
    }
}
```

- **(b) runtime eye-check:** rebuild → launch the app → spawn a new replica agent → open the newly-created `<replica>/CLAUDE.md` and inspect. Materialized path pattern: `<workgroup-root>/__agent_<name>/CLAUDE.md`.

**Recommendation: do (a), skip (b).** Reason: (a) catches regressions on every `cargo test`; (b) requires manual steps and depends on replica provisioning code that's orthogonal to this change.

**If `session_context.rs` has no existing `#[cfg(test)] mod tests`:** add one at EOF — ~6 lines, trivial diff. Check before writing: if a mod already exists elsewhere in the file, extend it instead.

**Why:** §34.10 asked for exact commands and eye-check location. Delivered both. (a) replaces runtime inspection with a fast deterministic gate, which is strictly better.

### 9.12 — Implementation summary for dev-rust (me) — self-handoff

When I execute:
1. `cd` into `repo-AgentsCommander` (never from replica dir).
2. Confirm branch: `rtk git branch --show-current` → must print `fix/handoff-claude-sessions-errors`.
3. Apply Edit B logic (plan §4.B). Run `rtk cargo check`. Green.
4. Add 5 tests + `unique_tmp` helper (§9.5). Run `rtk cargo test phone::messaging`. All 5 new + 3 existing pass.
5. Apply Edit A (plan §4.A).
6. Add `default_context_embeds_filename_only_warning` test (§9.11). Run `rtk cargo test config::session_context` (or similar). Green.
7. Apply Edit C (plan §4.C).
8. Full `rtk cargo test`. Green.
9. `rtk cargo clippy -- -D warnings`. Green or justify each warning.
10. One commit per §9.10 message.
11. Report back to tech-lead with branch + commit SHA. Do NOT merge, do NOT push unless instructed.

**Why:** self-contained checklist prevents me from skipping steps on a long session. Step 6 is NEW vs plan §8 (plan had no test for Edit A); §9.11 added it.

---

## 10. Open items for tech-lead (informational)

- **§9.11 step (a) adds a new test in `session_context.rs`.** Plan §3 didn't list this file as a test target. Scope delta: +~6 LOC in an existing `mod tests` (or a new one). Flagging for adversarial review (Grinch). Rationale: deterministic regression gate replaces manual runtime check. Tech-lead: confirm this is acceptable or instruct removal.
- **§9.5 `unique_tmp` helper.** Adds ~10 LOC test helper in `mod tests`. Not strictly required (nano + distinct prefix usually suffices) but low cost for CI stability. Tech-lead: keep or drop.
- **§9.9 future issue** to add a `reason` field to `MessagingError::InvalidFilename`. Not part of this branch. Tech-lead: file the ticket after this branch merges.

None blocks implementation. Defaults in §9 stand unless tech-lead overrides before green-light.

---

## 11. Grinch adversarial review

Adversarial read against live code on `fix/handoff-claude-sessions-errors` @ `313b71e`. Line anchors verified:
- `messaging.rs:228-260` matches plan §4.B anchor quote byte-for-byte.
- `session_context.rs:346-446` — `default_context` uses `r#"..."#` delimiter, line 348 opening, line 443 closing.
- `session_context.rs:414-436` — messaging subsection; `2. Fire the send:` at line 427, fenced command at 429-431, "The recipient…" paragraph at 433-436. Insertion anchor in §4.A is correct.
- `CLAUDE.md:41-60` — existing `--send` bullet "filename (not path)" at line 57.
- `cli/send.rs:161` calls `resolve_existing_message(&msg_dir, filename)`. `cli/send.rs:171` does UNC strip for display only. Plan §4.B dev-note is accurate.
- `session_context.rs` has no existing `#[cfg(test)] mod tests` (grepped). §9.11 option (a) creates a fresh one.
- Shape validator (`validate_filename_shape` at `messaging.rs:114-186`) splits on `-` and requires each segment to match `[a-z0-9]+` → a `..` substring can never appear in a canonically-valid filename. Early `..` reject never false-positives valid protocol input. This defuses tech-lead §34.3 concern.

Findings below. Severities conservative — none CRITICAL or HIGH. Plan is implementable as-is; LOW/NIT items are suggestions dev-rust may adopt during implementation without a replan.

### Finding 1 — §4.B-tests does not call the §9.5 `unique_tmp` helper

- **Severity** — LOW
- **Evidence** — plan §4.B-tests at lines 180-302 uses inline `std::env::temp_dir().join(format!("ac-msg-abs-ok-{}", Utc::now().timestamp_nanos_opt().unwrap_or(0)))` in each new test. Plan §9.5 at lines 478-492 defines `unique_tmp(prefix)` with a thread-id tiebreaker and instructs "Call it from each new test: `let tmp = unique_tmp(\"ac-msg-abs-ok\");`". Plan §9.12 step 4 says "Add 5 tests + `unique_tmp` helper". The three artefacts say three different things.
- **Why it matters** — dev-rust has to mentally merge §4.B-tests (nanos-only) with §9.5 (nanos + thread-id). Concrete failure scenarios: (a) dev applies §4.B-tests literally and forgets §9.5 → race under `cargo test -- --test-threads=N`; (b) dev applies §9.5 to new tests only, diff review flags "why are 5 tests different from the other 3 existing ones". Plan leaves the resolution to §9.5's last paragraph which is buried.
- **Proposed fix** — in §4.B-tests, replace every `std::env::temp_dir().join(format!("<prefix>-{}", ...))` call with `unique_tmp("<prefix>")`, and define the helper above the first new test. Keep the three existing tests untouched per §9.5's "safest" advice. One consistent code block prevents the ambiguity.

### Finding 2 — `unique_tmp` hash omits `std::process::id()` → unsafe under `cargo nextest`

- **Severity** — NIT
- **Evidence** — plan §9.5 lines 478-492 hashes `std::thread::current().id()` plus a nanos timestamp. `cargo nextest` runs each test in a separate sub-process. Thread ids restart at the main-thread id in every process (same type `ThreadId` but generated per-process). Two nextest workers hitting the same nano tick with the same main-thread id will produce the same temp-dir name and race on `create_dir_all`/`remove_dir_all`.
- **Why it matters** — project CI may not use nextest today, but the repo's test infra is not pinned to default `cargo test`. A future CI migration silently regresses these tests. Cost to preempt is 1 LOC.
- **Proposed fix** — add `std::process::id()` to the hash input in `unique_tmp`:
  ```rust
  std::process::id().hash(&mut h);
  std::thread::current().id().hash(&mut h);
  ```
  Or append to the format directly. Either kills the cross-process collision.

### Finding 3 — Edit B propagates `Io` (not `InvalidFilename`) when `messaging_dir` cannot be canonicalized, breaking error-code convention for path-shaped inputs

- **Severity** — LOW
- **Evidence** — plan §4.B line 149 uses `let canon_msg_dir = std::fs::canonicalize(messaging_dir)?;`. The `?` propagates `std::io::Error` through `#[from]` as `MessagingError::Io`. Pre-Edit behaviour for a broken messaging_dir: basename input reaches `candidate.exists()` at current line 241 → returns `false` → `FileNotFound`. Post-Edit for path input: Edit B canonicalizes messaging_dir FIRST, fails with `Io` before `FileNotFound` can fire. Two call styles produce two different error variants for the same underlying "WG structure broken" root cause.
- **Why it matters** — agent receives `Error: io: No such file or directory` for path input, `Error: message file not found: …` for basename input. Noisy, inconsistent, and the `Io` variant has no `filename` context for debugging. The existing `canonicalize(messaging_dir)?` at line 246 has the same flaw but is reached only after `candidate.exists()` filters most missing-dir cases; Edit B exposes it unconditionally for path inputs.
- **Proposed fix** — wrap the canonicalize with a map:
  ```rust
  let canon_msg_dir = std::fs::canonicalize(messaging_dir)
      .map_err(|_| MessagingError::InvalidFilename(filename.to_string()))?;
  ```
  Or, alternatively, leave as-is and document in §6 that path-shaped inputs can surface `MessagingError::Io` when the messaging dir is broken. Dev-rust's call; I'd pick the `map_err` because it keeps error boundary crisp per Edit B's own dev-note at plan line 133 ("explicit reject is filesystem-independent and keeps the error boundary crisp").

### Finding 4 — Post-normalization errors carry basename, not the original path-shaped input

- **Severity** — LOW
- **Evidence** — plan §4.B shadows `filename: &str` to the basename when normalization occurs. All downstream errors at current lines 236 (`.md` check), 238 (shape), 242 (FileNotFound), 250 (abs_parent None), 252 (parent mismatch), 256 (NotAFile) use the shadowed binding. Example: agent passes `C:\wg\messaging\20260419-….not-shape.md`, Edit B accepts normalization, shape validator fails, error reads `filename '20260419-….not-shape.md' does not match the required shape`. The agent's log does not preserve the fact that they passed a full path — making it harder to trace back to the agent template bug that caused the path to be passed in the first place.
- **Why it matters** — this is the EXACT failure mode Edit A is trying to prevent recurring. If docs drift and agents start passing paths again, the diagnostic signal that "this agent is still emitting a path" is lost because the path is silently stripped before the error fires. Makes post-merge regression detection harder.
- **Why not HIGH** — plan §9.9 explicitly defers error-message enrichment as out-of-scope, tech-lead §26 accepts. This finding agrees with the deferral BUT the deferred follow-up (plan §9.9 action item) should be broadened to also preserve the original input string in error context, not just add a `reason` field. Small scope delta for the follow-up ticket.
- **Proposed fix** — no plan change for this branch. Update §9.9's action-item description to read: "Enrich `InvalidFilename` with a reason field AND preserve the original caller-supplied input string (pre-normalization) in the error payload for diagnostic continuity." Tech-lead to file the broader ticket.

### Finding 5 — Double canonicalize of `messaging_dir` on path-shaped inputs

- **Severity** — NIT
- **Evidence** — plan §4.B calls `canonicalize(messaging_dir)` at the new block. Existing code at current line 246 calls `canonicalize(messaging_dir)` again. On path-shaped inputs, the syscall fires twice per send.
- **Why it matters** — `canonicalize` on Windows walks the path segment-by-segment and resolves reparse points; on long paths under a junction this is a few ms per call. Not measurable at typical send rates, but gratuitous.
- **Proposed fix** — skip. Not worth a plan edit or a diff delta. Flagged for completeness. If dev-rust wants to optimise, hoist one canonicalize into a local and pass it to the existing block, but that expands the scope of Edit B into the existing block's lines — a worse tradeoff than the wasted syscall.

### Finding 6 — Pre-existing file-ownership gap (not a regression, informational)

- **Severity** — INFORMATIONAL
- **Evidence** — `resolve_existing_message` verifies the filename is (a) shape-valid, (b) inside messaging_dir, (c) a regular file. It does not verify the sender actually authored the file. Agent Alice writes `20260419-143052-wg7-alice-to-wg7-carol-slug.md`; agent Bob invokes `send --to wg7-carol --send <Alice's basename>` — delivery succeeds with Bob's `from` field but Alice's file content. Edit B does not create this hole but makes it marginally easier to hit because abs paths now resolve instead of rejecting.
- **Why it matters** — requires agent template corruption or intentional misuse; not a practical attack vector (both agents already have filesystem write to messaging_dir). But plan §13.2 of the landed messaging feature talks about "append-only audit convention" — that convention depends on unique, sender-scoped filenames. Worth tech-lead awareness for post-merge tracking.
- **Proposed fix** — no action this branch. Tech-lead: consider filing a separate issue "Verify sender ownership of --send filename" tracked alongside §9.9's `InvalidFilename` enrichment. Out of scope here.

### Finding 7 — `resolve_rejects_traversal`'s `"foo/bar.md"` case relies on test CWD not containing a dir named `foo`

- **Severity** — NIT
- **Evidence** — plan §9.6 walks the existing test post-Edit B. For input `"foo/bar.md"`, Edit B takes the path branch, `Path::new("foo/bar.md").parent()` = `Some("foo")`, `canonicalize("foo")` resolves against the test CWD (`src-tauri/`). If `src-tauri/foo` exists and happens to be the messaging_dir (impossible), test would false-positive. If `src-tauri/foo` exists and is NOT messaging_dir, canonicalize succeeds → canon_parent != canon_msg_dir → reject → test still passes. If `src-tauri/foo` doesn't exist, canonicalize fails → `.map_err` → reject → test passes. All three branches reject, so the test is safe. But the test relies on an implicit invariant about the crate directory layout.
- **Why it matters** — in five years when someone adds `src-tauri/foo/` for unrelated reasons, the test could start exercising a code path it wasn't designed to test. Unlikely but possible.
- **Proposed fix** — skip. The existing test is upstream and stable (§9.6 walked it correctly). Flagging for awareness only. If dev-rust wants extra robustness, they could `std::env::set_current_dir(&tmp)` in the test, but that fights cargo test parallelism. Not worth it.

### Finding 8 — `Path::parent()` for `"foo\bar.md"` on Unix returns `Some("")`, not `None`

- **Severity** — NIT
- **Evidence** — plan §4.B dev-note at line 132 claims "Path::parent() returns Some("") for bare filenames — but we only take this branch when separators are present, so parent is always a real directory component." On Unix, `\` is NOT a path separator. `Path::new("foo\\bar.md").parent()` returns `Some("")` (treating the whole string as one component). The plan's proposed code calls `canonicalize("")` on that empty path, which returns `Err(NotFound)` on Unix → `.map_err` → InvalidFilename. Rejection fires correctly.
- **Why it matters** — the plan's dev-note is slightly misleading. Dev-rust might read it, conclude parent is always a real dir, and skip the `.map_err` wrapper. Result: the `?` would propagate `Io`, not `InvalidFilename`. Cross-platform test suite catches it, but the dev-note is a footgun.
- **Proposed fix** — amend §4.B dev-note to add: "On Unix, `\` is not a separator, so `foo\bar.md` takes the separator branch only on Windows; on Unix it skips the branch entirely (no `/`, no `\` detected by `.contains('\\')` as a separator-check). The `.contains('\\')` check detects the backslash character regardless of OS, so `foo\bar.md` DOES enter the path branch on Unix; `Path::new` then parses it as one component, `parent()` returns `Some("")`, canonicalize fails, InvalidFilename fires. The `.map_err` on canonicalize is load-bearing for cross-platform correctness." Same behavior, clearer contract.

### Finding 9 — Plan §5 test-matrix row (e) "symlink escape" explicitly not added — confirmed correct, but the gap is invisible to CI on Windows

- **Severity** — NIT
- **Evidence** — plan §5 table row (e) says "Not added — existing check at 248-253 suffices". Verified in §9.7: candidate-join → canonicalize follows symlinks → parent compare rejects. The logic is sound. But the project's CI runs on Windows; Windows symlink creation requires developer mode or admin, so even if someone added a `#[cfg(unix)]` symlink test, Windows CI wouldn't exercise it. The symlink-escape path has zero test coverage today and will continue to have zero post-merge.
- **Why it matters** — if a future refactor accidentally moves or removes the parent-compare at 248-253, no test catches it. Pre-existing hole, not caused by this branch.
- **Proposed fix** — no plan change for this branch. Tech-lead: consider adding a Unix CI leg (GitHub Actions ubuntu-latest) that runs `cargo test phone::messaging` — relatively cheap and gives symlink coverage for free. Out of scope here; flagging as a post-merge improvement separate from §9.9.

### Summary

| Severity | Count |
|---|---|
| CRITICAL | 0 |
| HIGH | 0 |
| MEDIUM | 0 |
| LOW | 3 (F1, F3, F4) |
| NIT | 4 (F2, F5, F7, F8) |
| INFORMATIONAL | 2 (F6, F9) |

None block implementation. F1 (unique_tmp wiring) and F8 (dev-note clarity) are pure documentation polish — dev-rust can fix during implementation. F3 (`map_err` on canonicalize) is a 1-line code adjustment that visibly tightens error semantics; recommended. F2 (PID in hash) is 1 line of test defensiveness, recommended. F4-F6-F9 are follow-up tickets for tech-lead to track, not blockers for this branch.

Plan is approved for implementation subject to dev-rust's discretion on whether to fold F1, F2, F3, F8 into the single commit. I did NOT find a way to break Edit B's security posture (symlink, traversal, drive-relative, UNC, ADS, NUL, non-UTF-8 all walked). Edit A's raw-string safety is verified — no `"#` sequence in the insert. Line anchors match live code. Shape validator architecturally precludes `..` in valid inputs, so the early `..` reject is pure defense-in-depth with no false-positive risk on valid protocol traffic.
