# Plan — Trim PTY overhead + raise PTY_SAFE_MAX

**Branch**: `feature/messages-always-by-files` (extend, do NOT branch). Stack on HEAD `24b160f`.
**Repo**: `repo-AgentsCommander`
**Date**: 2026-04-18
**Requirement source**: `__agent_tech-lead/_scratch/requirement-trim-pty-overhead.md`
**Predecessor plan**: `_plans/messages-always-by-files.md` (rounds 1-2 already shipped as commits `03141b9`, `07b4360`, `24b160f`).

---

## 1. Summary

Two complementary changes on top of the file-based messaging feature:

1. **Kill the inline reply-hint.** Interactive PTY injection and the post-command follow-up inject only `\n[Message from {from}] {body}\n\r` — no per-message CLI boilerplate. The reply flow (write file, then `send --send`) is already taught in `session_context.rs` and the Session Credentials block; duplicating it per message costs ~348 chars of PTY overhead for zero unique information. This eliminates the root cause of the `PTY_SAFE_MAX=500` fail-fast the user hit in production.
2. **Raise `PTY_SAFE_MAX` from 500 to 1024.** Defense in depth. After Change 1 the effective body budget already comfortably exceeds 1000, but a higher ceiling absorbs long workgroup paths and future template growth without re-opening the clamp issue.

Non-changes:
- The `use_markers=true` branch (non-interactive `--get-output`) at `mailbox.rs:855-863` stays untouched.
- Filename pattern, messaging-dir location, `--command` behaviour, mutex guard — all unchanged.
- No merge to main. No new branch.

---

## 2. New PTY injection format

Literal template (same for both call sites):

```
\n[Message from {from}] {body}\n\r
```

Rendered with empty placeholders → `"\n[Message from ] \n\r"`, which is **19 bytes** (`.len()` measured; contract test §6 pins the measurement).

Rendered with `from="wg7-tech-lead"` (13) + body=155 (`"Nuevo mensaje: <130-char-abs-path>. Lee este archivo."`):
- 19 (fixed) + 13 (from) + 155 (body) = **187 bytes** — well under new PTY_SAFE_MAX.

Upper bound on body under the new clamp:
- `PTY_SAFE_MAX - PTY_WRAP_FIXED - max_from.len() ≈ 1024 - 19 - 30 = 975 bytes of body`.
- Body shape = `34 + abs_path.len()` → max abs_path ≈ 941 bytes. Effectively unbounded for our deployments.

---

## 3. Files to MODIFY

### 3.1 `src-tauri/src/phone/messaging.rs`

**Line 13** — bump the clamp:
```rust
pub const PTY_SAFE_MAX: usize = 1024;
```

**Lines 18-39** — **delete** the `reply_hint!` macro and its doc-comment entirely. The `#[macro_export]` attribute goes with it.

**Lines 41-48** — **delete** the `pty_wrap_fixed()` OnceLock function. Replace the whole block with a single `pub const`:
```rust
/// Fixed-char portion of the interactive PTY wrap template
/// `\n[Message from {from}] {body}\n\r` with empty placeholders.
/// Kept as a public constant so the CLI clamp can size overhead precisely.
pub const PTY_WRAP_FIXED: usize = "\n[Message from ] \n\r".len();
```
(The expression is const-evaluable; `str::len` is `const fn`.)

Also remove the now-stale `use std::sync::OnceLock;` at line 10.

**Lines 282-291** — **delete** `estimate_wrap_overhead` entirely. Inline the computation at the single call site (see §3.2). Function signature is trivial enough that keeping it adds surface with no payoff.

### 3.2 `src-tauri/src/cli/send.rs`

**Lines 183-201** — replace the overhead-estimation call + clamp with the trimmed math. New block (drop-in, same behaviour minus the dropped inputs):

```rust
// PTY_SAFE_MAX clamp (trimmed overhead: the wrap no longer embeds
// wg_root or bin_path — only `from` and the fixed framing remain).
let overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender.len();
if body.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
    eprintln!(
        "Error: notification exceeds PTY-safe length (body {} + overhead {} > {}). \
         Shorten slug or move workgroup to a shallower path.",
        body.len(),
        overhead,
        crate::phone::messaging::PTY_SAFE_MAX
    );
    return 1;
}
```

Delete the `let bin_path = crate::resolve_bin_label();` and `let wg_root_str = wg_root.to_string_lossy();` lines (formerly L185-186) — unused after the trim.

The preceding `log::warn!` at L177-181 for `body.len() > 200` stays as-is.

### 3.3 `src-tauri/src/phone/mailbox.rs`

**Lines 864-876** — replace the `else` branch (interactive path inside `inject_into_pty`) with a minimal `format!`. Drop the `resolve_recipient_wg_root` call and `wg_root_display` binding; they are dead after the template trim.

```rust
} else {
    format!("\n[Message from {}] {}\n\r", msg.from, msg.body)
};
```

**Lines 972-983** — replace the payload construction in `inject_followup_after_idle_static` with the same minimal format. Drop the `bin_path` binding (L972) AND the `resolve_recipient_wg_root` call (L973-975). New body:

```rust
let payload = format!("\n[Message from {}] {}\n\r", msg.from, msg.body);
crate::pty::inject::inject_text_into_session(app, session_id, &payload, true).await
```

**Lines 987-…** (the `async fn resolve_recipient_wg_root` defined after `inject_followup_after_idle_static`) — **delete** the entire function. It has no remaining callers. Verify via `grep -n 'resolve_recipient_wg_root' src-tauri/src` after the edit returns only this definition site (which then goes away); no test references exist (the function is only used from inside mailbox.rs).

**Do NOT touch** the `use_markers=true` branch at L855-863 — explicitly out of scope.

**Do NOT touch** the token-refresh block at L1722 — the example `send` command there already uses `--send <filename>` (from round 1); unrelated to this change.

---

## 4. Files NOT modified (verified)

- `src-tauri/src/config/session_context.rs` — `default_context` (L412-434) already documents the two-step flow end-to-end. No change; this is now the single authoritative source for the reply flow.
- `README.md` — L223 says "The CLI injects a short notification into the recipient's PTY pointing at the file's absolute path". Still accurate; mentions nothing about a per-message replyable command. No edit.
- `CLAUDE.md` — L44-57 describes the two-step flow with `--send`. No mention of an inline reply hint. No edit.
- `ROLE_AC_BUILDER.md` — L415-418 describes the file-based flow. No mention of reply hints. No edit.

`grep -nE "To reply|reply hint|reply-hint|replyable"` across `*.md` files returns matches only inside `_plans/messages-always-by-files.md` (the predecessor plan, immutable review history per round-2 rules). Confirmed no stale agent-facing doc needs edits.

---

## 5. Test plan

### 5.1 Rewrites / deletes in `src-tauri/src/phone/messaging.rs`

- **Delete** `reply_hint_macro_is_single_source_of_truth` (L561-588) — macro is gone. Replace with a **new** contract test that pins the new minimal format:

```rust
/// The PTY wrap template used at both injection sites must agree with
/// PTY_WRAP_FIXED. Any edit to the template string in `inject_into_pty`
/// or `inject_followup_after_idle_static` must update PTY_WRAP_FIXED
/// (or this test fires).
#[test]
fn pty_wrap_fixed_matches_minimal_template() {
    let sample = format!("\n[Message from {}] {}\n\r", "", "");
    assert_eq!(PTY_WRAP_FIXED, sample.len());
    assert_eq!(PTY_WRAP_FIXED, 19);
}
```

- **Delete** `estimate_wrap_overhead_monotonic` (L552-559) — function under test is gone.

- **Rewrite** `pty_safe_max_clamp_rejects_long_path` (L590-611) — old math references `estimate_wrap_overhead` and the old 500 ceiling. Pathological case must now exceed **1024** including the trimmed overhead:

```rust
#[test]
fn pty_safe_max_clamp_rejects_long_path() {
    let from = "wg99-extremely-long-agent-name-segment";
    // Notification body shape: 34 fixed + abs path.
    // Construct an abs path deep enough to push body well past 1000.
    let body_len = 34 + 1000; // synthetic — 1000-char abs_path
    let overhead = PTY_WRAP_FIXED + from.len();
    assert!(
        body_len + overhead > PTY_SAFE_MAX,
        "expected clamp to fire for body={} overhead={} max={}",
        body_len,
        overhead,
        PTY_SAFE_MAX,
    );
}
```

- **Rewrite** `pty_safe_max_clamp_accepts_typical` (L613-629) — the typical deployment (body ~155, from ~13) must now fit comfortably under 1024:

```rust
#[test]
fn pty_safe_max_clamp_accepts_typical() {
    let from = "wg7-architect";
    // Realistic production body measured at the time of the fix: 155.
    let body_len = 155;
    let overhead = PTY_WRAP_FIXED + from.len();
    assert!(
        body_len + overhead <= PTY_SAFE_MAX,
        "expected clamp to accept body={} overhead={} max={}",
        body_len,
        overhead,
        PTY_SAFE_MAX,
    );
}
```

Both rewrites drop all references to `wg_root.len()` and `bin.len()` — those inputs no longer enter the overhead math.

### 5.2 Full suite

- `cargo test --lib` — must stay green. No test currently references `reply_hint!` outside messaging.rs (verified via grep); mailbox.rs changes affect only call-site strings, not any test.
- `cargo clippy -- -D warnings` — must not introduce new warnings relative to the HEAD `24b160f` baseline. Deleting `estimate_wrap_overhead` removes a `pub fn`; deleting `resolve_recipient_wg_root` removes a private `async fn`; both are valid clippy-clean removals.

### 5.3 Manual smoke (dev cycle)

- Build the standalone binary (the user's existing harness at `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone.exe`).
- In the current wg-7 workgroup, send a message from one agent to another using `--send`. Confirm the recipient's PTY shows `[Message from wg7-<from>] Nuevo mensaje: <abs>. Lee este archivo.` only, with no trailing reply-hint paragraph.
- Repeat with the predecessor's production-failure case (body 155 + typical `from`) — must NOT hit the clamp.
- Verify the recipient can still reply: they already know from startup context how to do the two-step.

---

## 6. Risks

1. **Out-of-tree consumers parsing the old reply-hint.** Anyone who scraped PTY output looking for the literal `"(To reply, run: …"` substring to extract the reply command will stop finding it. In-tree grep finds no such consumer. External telegram scrapers / custom dashboards are possible but unknown to us. Acceptable; the new format is simpler and the reply mechanics are unchanged — only the hint is gone.
2. **Agents that don't remember the two-step flow.** Mitigated by `session_context.rs` `default_context` being injected into every agent at session start. If an agent was already mid-session before the update is deployed, its context still has the flow documented (the round-1 `session_context.rs` edit shipped in `03141b9`, well before this change). No regression window.
3. **1024 empirically untested.** Requirement out-of-scopes empirical PTY limit testing. If any supported shell truncates under 1024, the clamp would not protect. Known safe shells (Claude Code, Codex, Gemini, bash, powershell, git-bash) all handle 1024 single PTY writes; no reports to the contrary. Accept.
4. **Clippy `dead_code` on `PTY_WRAP_FIXED` if CLI is ever refactored to lose the import.** Today `cli/send.rs` references it. If a future refactor drops the reference, clippy will flag. Non-issue in this PR; noted for future work.
5. **Monotonic property lost.** The deleted `estimate_wrap_overhead_monotonic` test asserted that adding chars to wg_root or bin_path grew the overhead. The new model has no such variables; the property is vacuously true. No replacement needed — clamp math is now trivially correct by inspection.
6. **Contract test drift protection weakens.** The old `reply_hint_macro_is_single_source_of_truth` test guaranteed the macro, the overhead const, and both call sites couldn't drift. Under the new design the three sites become: (a) two identical `format!` string literals in mailbox.rs, (b) a `.len()` on the same literal in messaging.rs, (c) the new constant and its contract test. The new `pty_wrap_fixed_matches_minimal_template` test pins (c); it cannot detect a mismatch if someone edits (a) without also updating (c). Acceptable since (a) is just `\n[Message from {}] {}\n\r`, identical in both call sites, and the fixed portion is only 19 chars — drift requires ignoring the constant's doc-comment. Flag in commit message; don't over-engineer a lint.
7. **`estimate_wrap_overhead` was `pub` — external bindings might reference it.** `pub` items are removable without backwards-compat concerns in a pre-1.0 crate. No external Rust consumer exists (binary-only crate). Delete safely.

---

## 7. Decision log (explicit)

| Question | Decision | Reason |
|---|---|---|
| Delete `reply_hint!` macro? | **Yes, delete.** | Zero remaining call sites after the trim. Requirement suggestion matches. |
| Keep `estimate_wrap_overhead`? | **No, inline and delete.** | New formula is `const + sender.len()` — a function adds surface for no payoff. |
| Replace OnceLock-backed `pty_wrap_fixed()` with `pub const PTY_WRAP_FIXED`? | **Yes.** | `str::len()` is `const fn`; OnceLock was only needed because the old template had interior placeholders we had to format. The new fixed string is trivially const-evaluable. |
| Delete `resolve_recipient_wg_root` in mailbox.rs? | **Yes.** | No remaining callers. `wg_root` is no longer in the template. |
| Touch `use_markers=true` branch (L855-863)? | **No.** | Explicitly out-of-scope per requirement §Out-of-scope. |
| Touch the token-refresh block (L1722)? | **No.** | Already uses `--send <filename>` (round-1 edit). Orthogonal to this change. |
| README / CLAUDE.md / ROLE_AC_BUILDER.md edits? | **None.** | Grep confirms no mention of a per-message reply-hint in docs. Wording about "short notification" remains accurate. |

---

## 8. Sequence for the implementing dev

Incremental commits on top of `24b160f`:

1. Edit `src-tauri/src/phone/messaging.rs` per §3.1: bump `PTY_SAFE_MAX` to 1024, delete macro + OnceLock fn + `estimate_wrap_overhead`, add `PTY_WRAP_FIXED` constant, drop `use std::sync::OnceLock;`.
2. Edit `src-tauri/src/cli/send.rs` per §3.2: inline overhead math, drop `bin_path`/`wg_root_str` locals.
3. Edit `src-tauri/src/phone/mailbox.rs` per §3.3: replace both reply-hint call sites with minimal `format!`, delete `resolve_recipient_wg_root`.
4. Rewrite tests in `src-tauri/src/phone/messaging.rs` `#[cfg(test)]` module per §5.1.
5. `cargo check`, `cargo clippy -- -D warnings`, `cargo test --lib` — all green.
6. Manual smoke per §5.3.
7. Version bump: `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src/sidebar/components/Titlebar.tsx` — patch bump `0.6.0` → `0.6.1` (trim is behaviour-preserving for correct callers; only the diagnostic/overhead profile changes).
8. Commit with a message referencing `_plans/trim-pty-overhead.md` and calling out:
   - Root cause (per-message reply-hint was redundant with startup context).
   - That the reply flow is now a single-source artifact owned by `session_context.rs`.
   - The `PTY_SAFE_MAX` bump to 1024 as defense in depth.
9. Do NOT push to main. Stack on `feature/messages-always-by-files`.

---

## 9. NOT-scope (explicit)

- No change to `use_markers` branch.
- No filename-pattern change.
- No messaging-dir relocation.
- No `--command` changes.
- No empirical PTY limit probing.
- No merge / push to main.
- No new agent-facing docs — existing `session_context.rs` is the single source of reply-flow truth.

---

## 10. dev-rust additions — 2026-04-18

Reviewer: dev-rust. Incremental against HEAD `24b160f`. Verdict: **ACK with deltas below.** No P0/P1. Two NITs — one has an OPEN decision; one is implementer-discretion.

### 10.1 Line-ref verification against HEAD

| Plan claim | File | Verified | Note |
|---|---|---|---|
| §3.1 L13 `PTY_SAFE_MAX` | `messaging.rs` | ✅ L13 exact | — |
| §3.1 L18-39 `reply_hint!` macro | `messaging.rs` | ✅ L18-39 (L17 blank, `#[macro_export]` at L26, `macro_rules!` at L27, closing `}` at L39) | — |
| §3.1 L41-48 `pty_wrap_fixed()` fn | `messaging.rs` | ✅ L41-48 exact | — |
| §3.1 L282-291 `estimate_wrap_overhead` | `messaging.rs` | ✅ doc-comment L282-288, fn sig L289-291 | — |
| §3.1 L10 `use std::sync::OnceLock;` | `messaging.rs` | ✅ L10 | OnceLock is also imported in 3 other files (`lib.rs`, `config/mod.rs`, `config/profile.rs`); those imports are independent — removing L10 is safe and isolated. |
| §3.2 L183-201 overhead clamp block | `send.rs` | ✅ exact | `sender` local is declared at L106 (`let sender = agent_name_from_root(&root);`); plan's `sender.len()` references it correctly. |
| §3.3 L864-876 interactive reply-hint | `mailbox.rs` | ✅ `else {` at L864, `};` at L876 | — |
| §3.3 L972-983 followup payload | `mailbox.rs` | ⚠️ actually L969-984 (`let bin_path` at L972, `let payload = crate::reply_hint!(` at L978, closing `)` at L983, `inject_text_into_session` call at L984). Plan's "L972-983" covers the core region. Not actionable. | — |
| §3.3 resolve_recipient_wg_root fn | `mailbox.rs` | ⚠️ doc-comment starts L987, `async fn` sig at L992, fn body L992-1012 | Plan's "Lines 987-…" is correct; clarified for implementer. |
| §5.1 L552-559 `estimate_wrap_overhead_monotonic` | `messaging.rs` | ✅ L552-559 exact | — |
| §5.1 L571-588 `reply_hint_macro_is_single_source_of_truth` | `messaging.rs` | ✅ test body L572-588 (plan's L571 is the `#[test]` attribute line L571) | Close enough. |
| §5.1 L590-611 / L613-629 clamp tests | `messaging.rs` | ✅ `pty_safe_max_clamp_rejects_long_path` L593-611, `pty_safe_max_clamp_accepts_typical` L614-629 | — |

### 10.2 Call-site audit

Grepped `estimate_wrap_overhead | resolve_recipient_wg_root | reply_hint` at HEAD:

- `estimate_wrap_overhead`: 4 callers — `send.rs:187` (production) + `messaging.rs:554,555,557` (monotonic test) + `messaging.rs:603,621` (two clamp tests). Plan §3.2 removes the production call; §5.1 deletes the monotonic test and rewrites both clamp tests to inline `PTY_WRAP_FIXED + from.len()`. **Zero callers after the trim.** ✓
- `resolve_recipient_wg_root`: 2 callers — `mailbox.rs:865,973` — both inside the branches plan §3.3 is replacing. No other callers, no test references. Safe to delete. ✓
- `reply_hint`: 4 expansions — `mailbox.rs:870,978` (production) + `messaging.rs:47` (inside `pty_wrap_fixed()` OnceLock seed) + `messaging.rs:573,586` (test). Plan §3.1 deletes the macro and the OnceLock seed, §3.3 replaces the mailbox sites, §5.1 deletes the test. **Zero expansions after the trim.** ✓

No dangling references. No orphan imports (apart from `OnceLock` which plan §3.1 already removes).

### 10.3 `PTY_WRAP_FIXED = "\n[Message from ] \n\r".len()` const-evaluability

`str::len()` has been `const fn` since Rust 1.39 (2019). Current toolchain is `edition = "2021"` (Cargo.toml L4) with no explicit MSRV pin below 1.39. The expression is valid at `const` position. ✓

Manual byte count of `"\n[Message from ] \n\r"`: `\n`(1) + `[Message from `(14) + `] `(2) + `\n\r`(2) = **19**. Matches plan §2 and §5.1 `assert_eq!(PTY_WRAP_FIXED, 19)`. ✓

### 10.4 NIT-1 (has OPEN decision) — drift detection re-weakens

Round-2 P1-A (grinch, `07b4360` → `24b160f`) specifically introduced the `reply_hint!` macro to prevent drift between the template literal in `mailbox.rs` and the overhead accounting in `messaging.rs`. The old test `reply_hint_macro_is_single_source_of_truth` proved single-source: any structural edit to the macro body failed the test.

Plan §5.1's new test `pty_wrap_fixed_matches_minimal_template` `format!()`s a **local** literal and compares its length to `PTY_WRAP_FIXED`. This is **exactly** the same fragility grinch flagged as P1-A: a future editor who changes the `format!("\n[Message from {}] {}\n\r", ...)` string in `mailbox.rs` (e.g. drops the trailing `\r`, adds a space, reorders) without touching `messaging.rs` will keep the test green, drift `PTY_WRAP_FIXED` from reality, and reopen the silent-truncation window.

Plan §6.6 acknowledges the regression and accepts it on the grounds that the fixed string is only 19 chars and "drift requires ignoring the constant's doc-comment". This is the same argument that was rejected in round 2.

**Counterproposal (non-blocking)**: keep single-source via a tiny helper fn — no macro, no OnceLock — just a regular `pub fn`:

```rust
// messaging.rs — one-line API:
pub fn format_pty_wrap(from: &str, body: &str) -> String {
    format!("\n[Message from {}] {}\n\r", from, body)
}
pub const PTY_WRAP_FIXED: usize = "\n[Message from ] \n\r".len();

// Contract test: DOES guard against drift in format_pty_wrap
#[test]
fn pty_wrap_fixed_matches_format_pty_wrap() {
    assert_eq!(format_pty_wrap("", "").len(), PTY_WRAP_FIXED);
    assert_eq!(PTY_WRAP_FIXED, 19);
}
```

Mailbox.rs replaces the two `format!()` call sites with `crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body)`. Cost: ~5 extra lines (one `pub fn`, two consumer swaps, no macro, no extra test). Benefit: single source of truth survives. If anyone edits `format_pty_wrap`, the contract test catches length drift immediately.

**OPEN-1**: Go with plan as-written (no drift detection, regresses round-2 P1-A) — **or** adopt `format_pty_wrap` helper (drift detection preserved, ~5 lines extra)?

I lean the helper. Round 2's effort was explicit defense against this exact regression class; trashing that investment 3 hours later without a matching gain reads as churn. Tech-lead's call.

### 10.5 NIT-2 (implementer discretion) — identical format at L862

After the trim, `mailbox.rs:862` (`use_markers=true && request_id.is_none()` branch) and the new `mailbox.rs:864-…` (`use_markers=false` branch) become **character-identical**:

```rust
format!("\n[Message from {}] {}\n\r", msg.from, msg.body)
```

The outer `if use_markers { … } else { … }` still discriminates the marker-wrapped case (L855-860), but the two non-marker branches could collapse into a single default. Example shape:

```rust
let payload = if use_markers {
    if let Some(ref rid) = msg.request_id {
        format!("\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... %%AC_RESPONSE::{}::END%%)\n\r",
            msg.from, msg.body, rid, rid)
    } else {
        format!("\n[Message from {}] {}\n\r", msg.from, msg.body)
    }
} else {
    format!("\n[Message from {}] {}\n\r", msg.from, msg.body)
};
```

could become:

```rust
let payload = match (use_markers, msg.request_id.as_ref()) {
    (true, Some(rid)) => format!("\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... %%AC_RESPONSE::{}::END%%)\n\r",
        msg.from, msg.body, rid, rid),
    _ => format!("\n[Message from {}] {}\n\r", msg.from, msg.body),
};
```

Saves 2-3 lines, eliminates the duplicated literal. **Implementer discretion**: if §10.4 OPEN-1 picks the helper fn, collapsing uses `format_pty_wrap` in the fallback arm (single source across 3 branches). If OPEN-1 picks plan-as-written, this collapse still reduces literal duplication from 3 → 2 sites. Either way, not in plan scope; flag for optional inclusion.

### 10.6 Version bump rationale

Plan §8 step 7 chooses `0.6.0 → 0.6.1` (patch). Checking what's actually breaking:

- `pub fn estimate_wrap_overhead` removed — public API deletion.
- `#[macro_export] reply_hint!` removed — crate-level macro deletion.
- `pub const PTY_SAFE_MAX` value changes 500 → 1024 — constant change.
- `pub const PTY_WRAP_FIXED` replaces internal `fn pty_wrap_fixed()` — private→public swap.

For a binary-only crate (no external `Cargo.toml` depends on `agentscommander-new`), these are all internal refactors. Patch bump is defensible. Same logic used in round 1 (0.5.4 → 0.6.0 as minor for CLI contract break); patch here because no CLI-visible change. **Acceptable as-is.** No objection.

### 10.7 Summary

- **ACK plan with §10 deltas.**
- **OPEN-1** (§10.4): adopt `format_pty_wrap` helper? Default no → keep plan; my preference yes → preserves round-2 guarantees.
- **NIT-2** (§10.5): optional branch collapse at mailbox.rs L862 + new L864. Implementer call.
- All line refs confirmed ✓. All call sites auditable and clean post-trim ✓. Const-evaluability confirmed ✓. No P0 or P1 surfaced.

— dev-rust (Step 3 review, 2026-04-18)

---

## 11. grinch adversarial review — Step 4 (2026-04-18)

Reviewer: dev-rust-grinch. Reviewed against HEAD `24b160f`. Premise: tech-lead has ratified **OPEN-1 option (b)** — `format_pty_wrap(from, body)` helper fn in `messaging.rs`, used by the contract test. Dev-rust §10 ACK'd with that deferred decision; my pass finds what breaks under that ratification.

### 11.1 Verdict

**CONDITIONAL APPROVED.** One P1 (plan omission that fails the clippy gate), two P2s (drift-protection gap + redundancy concern), vote on OPEN-1 = **CONFIRM (b)**, no new OPENs.

### 11.2 P1 finding

#### P1 — Plan §3.3 omits deleting the orphaned `bin_path` at `mailbox.rs:853`

**What.** After `inject_into_pty`'s reply_hint! call at L870-875 is replaced with the minimal `format!("\n[Message from {}] {}\n\r", ...)` (plan §3.3 target), the `let bin_path = crate::resolve_bin_label();` at **L853** becomes unused. Plan §3.3 explicitly removes the analogous `bin_path` binding at L972 (inside `inject_followup_after_idle_static`) but not L853.

**Why it matters.** Plan §5.2 requires `cargo clippy -- -D warnings` to pass. An unused variable surfaces as either `unused_variables` (rustc lint, warn-by-default) or escalates to deny under `-D warnings`. **The clippy gate fails post-trim** unless L853 is also removed.

**Reproduction.** Apply plan §3.3 as written to a test branch, run `cargo clippy -- -D warnings` in `src-tauri`, observe:
```
error: unused variable: `bin_path`
   --> src/phone/mailbox.rs:853:13
    |
853 |         let bin_path = crate::resolve_bin_label();
    |             ^^^^^^^^ help: if this is intentional, prefix it with an underscore: `_bin_path`
```

**Fix.** Add to plan §3.3 the explicit deletion of `mailbox.rs:853` (`let bin_path = crate::resolve_bin_label();`) alongside the existing removal at L972. One-liner plan edit. Or integrate with NIT-2 collapse, which removes the else-branch entirely and sidesteps the issue.

Dev-rust §10.1 didn't flag this either — both reviewers missed L853. The grep `bin_path | resolve_bin_label` in `mailbox.rs` returns: L853 (defn), L873 (consumer, deleted by §3.3), L972 (defn, deleted by §3.3), L981 (consumer, deleted by §3.3). Only L853 definition persists post-trim without its consumer.

### 11.3 P2 findings

#### P2-1 — With OPEN-1 (b) helper fn, `mailbox.rs:862` keeps its own literal

Under tech-lead's ratification (OPEN-1 = helper fn), the helper is used **only at the 2 trim-target sites** (interactive-path L864 and followup L978, per plan §3.3 + §10.4). The marker-path fallback at **L862** (`use_markers=true && rid.is_none()`) retains its own `format!("\n[Message from {}] {}\n\r", msg.from, msg.body)` literal — **character-identical** to what `format_pty_wrap` produces.

Consequence: if a future editor changes `format_pty_wrap` (e.g. trim the `\n\r` tail or reorder fields), the contract test `pty_wrap_fixed_matches_format_pty_wrap` catches drift AT the helper. But L862 continues emitting the OLD format. The two injection paths diverge silently. Not a clamp bug — `PTY_WRAP_FIXED` is still derived from the helper's empty expansion — just a **visible inconsistency** between interactive/marker-path payloads.

**Recommendation.** Use the helper at L862 as well, OR adopt dev-rust's §10.5 NIT-2 collapse with the helper in the fallback arm:
```rust
let payload = match (use_markers, msg.request_id.as_ref()) {
    (true, Some(rid)) => format!(
        "\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... %%AC_RESPONSE::{}::END%%)\n\r",
        msg.from, msg.body, rid, rid
    ),
    _ => crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body),
};
```
Three-site drift protection via one helper. Round 2's investment preserved.

Either explicit addition of helper at L862 OR NIT-2 collapse with helper closes the gap. Otherwise the P1-A regression is partial rather than full.

#### P2-2 — Loss of per-message redundancy for reply-flow knowledge

Pre-trim, every PTY-injected message embedded the reply flow in its own tail. Post-trim, the flow lives in exactly one place: `session_context.rs` → injected ONCE at session start into the agent's context.

Agents with long-running sessions have their conversation context periodically **compacted** or **summarized** (depends on the harness: Claude Code compacts via `/compact`, Codex has its own, etc.). After compaction, early context may be compressed to a summary that drops details like exact CLI flag wording. An agent mid-session whose startup block gets compacted loses the explicit `send --send <filename>` instruction, and the messages it now receives no longer re-teach it.

**Probability.** Medium — depends on session length and harness-specific compaction behaviour. For a session >100 messages or >N tokens, compaction is likely.
**Consequence.** Agent guesses at the reply CLI, asks the user, or falls back to old `--message` (which errors out).

**Mitigation options** (NOT blocking, but worth flagging to tech-lead):
- (a) Accept: compaction is the agent harness's responsibility; file the behaviour under "known user-facing limitation".
- (b) Tiny per-message reminder: `\n[Message from {}] {}\n(reply: --send <file>)\n\r` — adds ~18 chars. Body 155 + overhead 19+25(from)+18 = 217 → still comfortable under 1024.
- (c) Periodic reinjection of the full context block on N-message intervals — harness-specific, not trivial.

Recommendation: (b) if tech-lead values robustness; (a) if simplicity wins. Current plan picks (a) implicitly. My vote: (a) is defensible; just LOG the decision so future triage of "agent forgot how to reply" isn't a mystery.

#### P2-3 — `PTY_SAFE_MAX = 1024` is future-proof, but 512 would also fit

Math post-trim (realistic deployment):
- Body max ≈ 34 + abs_path ≈ 34 + 150 = 184.
- Overhead max ≈ 19 + sender.len() ≈ 19 + 30 = 49.
- Total max ≈ 233.

512 gives 279-char headroom; 1024 gives 791. Either works. Plan §2 justifies 1024 as "defense in depth" against future template growth. Accepted as a one-line constant change; reversible if empirical testing uncovers a shell that truncates below 1024.

**FYI observation, not a finding.** If tech-lead wants a tighter clamp that catches pathologies earlier (e.g. 256-char bodies from deep paths), consider 512. Otherwise 1024 is fine.

### 11.4 Other sensitive-area spot checks

All passed; listing so tech-lead knows they were checked:

- **Const-evaluability of `"\n[Message from ] \n\r".len()`**: `str::len` is `const fn` since Rust 1.39; repo uses edition 2021; valid. ✓
- **Windows line endings (`\n\r` vs `\r\n`)**: `\n\r` is pre-existing convention in `mailbox.rs:858, 862` — inherited, consistent. No terminal emulator in our support set (xterm.js included) treats `\n\r` differently from `\r\n` for cursor positioning. Intentional per existing code. Accept.
- **`resolve_recipient_wg_root` delete**: `git grep` confirms 3 hits total — 2 callers (both in trim-target branches) + 1 definition. Zero after trim. ✓
- **`estimate_wrap_overhead` delete**: `git grep` confirms 7 hits — 1 production (send.rs:187) + 3 monotonic test + 2 clamp test + 2 doc-comment refs (both in code blocks being removed by plan §3.1/§3.3). All disappear with the trim. `pub` → zero external consumers (binary-only crate). ✓
- **`use_markers` branch collision (tech-lead Q3)**: NIT-2 match-based collapse preserves semantics — `(true, Some) → marker`, `_ → minimal`. `_` covers `(true, None)` and `(false, _)`. Same behaviour as original nested if-else. Safe. ✓
- **Version bump 0.6.0 → 0.6.1 (patch)**: CLI contract unchanged (no flag add/remove/change). Rust `pub` API changes (`estimate_wrap_overhead` removed, macro removed) — but binary-only crate, no external consumers. PTY output format changes (visible to human observers); for agents, session_context already carries the flow. **Patch bump defensible.** Accept.
- **Docs edits**: grep `"To reply|reply hint|reply-hint|replyable"` + `"[Message from|Reply between markers"` on live `*.md` files returns zero actionable hits. CLAUDE.md:84 mentions `[Message from ...]` as a format descriptor — remains accurate under trim. No stale wording. ✓
- **Backward compat for in-session agents (tech-lead Q10)**: Each message is independently parsed by agents. Mixing old-format (reply-hint included) and new-format (minimal) messages in the same conversation context doesn't confuse parsing. session_context.rs is already consistent with trim (round 1 edit describes file-based flow without inline reply-hint). No regression window. ✓

### 11.5 OPEN-1 vote (tech-lead decision confirmation)

**CONFIRM tech-lead's ratification: OPEN-1 = (b) `format_pty_wrap` helper.**

Reasoning:
- Round 2's macro+OnceLock investment was defense-in-depth against *exactly* the drift class that a local-literal test would miss. Unwinding 3 hours later without matching gain is churn.
- Helper fn is ~5 extra lines, zero macro machinery, no OnceLock — simpler than round-2's macro/OnceLock design.
- Contract test asserts `format_pty_wrap("", "").len() == PTY_WRAP_FIXED` — catches drift at the helper (which is the single source all three logical sites can/should funnel through).
- The cost of *NOT* having the helper is the P1-A regression class: template drift in `mailbox.rs` makes the clamp math stale. Round 2 proved this is worth defending.

Ratify (b). See P2-1 above for the follow-on action: use the helper at L862 too (or collapse via NIT-2) so drift protection is total, not 2/3rds.

### 11.6 Summary table

| Item | Verdict | Action |
|---|---|---|
| P1: L853 `bin_path` orphan | blocks clippy gate | Plan §3.3 must add L853 delete |
| P2-1: L862 skips helper | drift protection partial | Helper at L862 OR NIT-2 collapse w/ helper |
| P2-2: per-message reply-flow redundancy lost | agent compaction risk | Accept, or add 18-char tail reminder |
| P2-3: PTY_SAFE_MAX=1024 | future-proof | Accept; 512 also valid |
| NIT: `\n\r` convention | inherited; correct | No action |
| NIT: marker-arm prefix factoring | over-engineering | No action |
| OPEN-1 vote | **CONFIRM (b)** | Tech-lead ratification stands |

### 11.7 Approval

**CONDITIONAL APPROVED.** Plan is sound; close P1 (L853 delete) + P2-1 (helper at L862 OR collapse) and it becomes APPROVED. Everything else is notes.

No round-2-style architectural surprises found. The trim design is clean.

— dev-rust-grinch (Step 4, 2026-04-18)
