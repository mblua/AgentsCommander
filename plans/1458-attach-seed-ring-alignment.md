# Plan #1458: Guarantee the attach seed starts at a line boundary, and fall back to the parser mirror when the ring has none

Author: architect, wg-17. Draft authored 2026-08-20 UTC as Step 4 of the full `code-implementation-workflow` path. Sections 1 to 10 amended at Step 7 round 1 (2026-08-20 UTC) to resolve the Step 5 and Step 6 findings; see `## Step 7 consensus resolution (round 1)` for the per-finding disposition.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1458](https://github.com/mblua/AgentsCommander/issues/1458), `Terminal attach seed replays the history ring from a mid-escape-sequence offset (broken screens after 64 KiB)`.

This is a minimal defect fix confined to one backend file. It adds one `bool` field to a private struct, one parameter and one three-line guard to one private function, one private free function of about fifteen lines, one branch at the attach seed site, one WARN emitted after the parser lock is released, and exactly five regression tests. It changes no ring bytes, no trim arithmetic, no IPC surface, no frontend file, no event, no command signature, no configuration key. It introduces no new crate, no new module, and adds zero module-to-module dependency arcs.

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `fix/1458-attach-seed-ring-alignment`, targeting `main`.

At authoring time (2026-08-20 04:21 UTC) the committed `HEAD` of the branch is `1376c2b84a23125624e919c9af7e65813d624241`, equal to the base `main` given by the dispatch, and `git status --porcelain` is empty (clean tracked and untracked tree). The single file this plan modifies, `src-tauri/src/pty/output.rs`, is blob `4f47604810cc17b399b51663fa7a17bc1c3da830` at that SHA. Re-verified independently at Step 5 (E.1), at Step 6, and again at Step 7 round 1: all three match.

Every line number below was read from that blob. Two anchors in the Step 4 draft were off by one and are corrected here (E.1): `ScreenReplayState` is `output.rs:58-75`, and its construction is `output.rs:911-922`.

Root `.gitignore` ignores `/plans/` (line 11), so the implementer must force-add this exact plan file with `git add -f plans/1458-attach-seed-ring-alignment.md`. Do not remove or weaken the `plans/` ignore rule.

Certification note: this plan is certified READY at the exact byte content of this file. Any byte change after certification invalidates it and requires a new certification round.

Step 7 (certification) must re-run the authority ritual: fetch `origin/main`, and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

## 2. Objective and non-goals

Objective: the bytes an attach seeds into a freshly reset xterm must always begin at a point a parser in ground state can start reading from. A ring front that sits at an arbitrary byte offset (which is what the capped hot-path realignment leaves behind whenever it fails) must never reach the frontend, and when the ring holds no point that qualifies, the attach must seed the parser mirror instead of garbage.

Non-goals, binding on the implementer:

- Do not change what `append_history` does to the RING. Its byte-space trim, its `UI_HISTORY_LINE_SCAN_BYTES` cap, its drain order, and the exact bytes it retains all stay as they are. The two edits to it are that it now RECORDS the outcome of the scan it already runs, and that it records the one arithmetic case where no scan runs at all (section 5.6's `history.is_empty() && tail.len() < data.len()` guard). Neither changes a single ring byte. Section 5.2 records why the scan cap must survive.
- **Do not weaken the guard's `<` to `<=`.** With `<=` the condition is always true, so every first chunk into an empty ring is flagged unaligned, every young session takes the cold path, and a session that has not yet emitted a `\n` gets the mirror instead of its complete ring: the plan would recreate #1458 for exactly the window section 3 identifies as the only healthy one. Section 9.6 records which existing test catches this mutation.
- Do not raise, lower, or delete `UI_HISTORY_LINE_SCAN_BYTES`, `UI_HISTORY_LIMIT_BYTES`, or `UI_HISTORY_REPLAY_PROLOGUE`.
- Do not add per-chunk logging to `append_history`, and do not thread the session id into it. Section 5.5 records why the diagnostic goes on the cold path instead, and outside the parser lock. This rejection is a fixed decision.
- Do not make `append_history` inspect the last drained byte in order to recover the accidentally-aligned case. Section 5.7 records why that refinement is rejected.
- Do not touch any frontend file. `src/terminal/components/TerminalView.tsx`, `src/shared/ipc.ts`, and `src/shared/types.ts` are byte-identical before and after this change.
- Do not touch `PtyScreenSnapshot`, `PtyScreenSnapshotPayload`, `activate_terminal_output`'s signature, or `src-tauri/src/commands/pty.rs`.
- Do not touch the #1439 grid reconcile branch (`stage=attach_grid_mismatch`), `resize_screen_and_broadcast`, `conpty_size`, or anything about parser sizing. This plan runs strictly inside the branch #1439 already validated.
- Do not change the `include_history == false` path, and do not change the empty-ring path. Both already resolve to `screen.contents_formatted()` and must keep producing byte-identical output; two existing tests pin them.
- Do not emit `\x1b[?1049h` (or any mode sequence) before the mirror. Section 5.4 records the alt-screen consequence as known and accepted, and the mirror's exact bytes are pinned today by two existing tests.
- Do not attempt to make the ring's TAIL safe. Section 7.4 records why the tail needs no work.
- Do not fix #1459 (replay wrapping when the ring's bytes were laid out at a different width). It is a real and separate defect, explicitly out of scope; see section 4.
- **Exactly five tests, and no existing test may be modified.** The Step 4 draft said three; Step 6 (G.1) demonstrated that all three land on the same arm of the new function and that a stub returning only `None` passes every one of them and every acceptance criterion. The binding rule is one test per decided branch, which is five: sections 9.1, 9.2, 9.3, 9.4, 9.5. Do not add a sixth, and specifically do not add one for the `history.is_empty() && tail.len() < data.len()` guard; section 9.6 records why.
- Do not add a crate. The alignment scan is a `[u8]::iter().position()` over at most 64 KiB on a cold path; `memchr` is not warranted.

## 3. Evidence and identified cause

Confirmed at the frozen SHA by direct read of every cited body.

- `const UI_HISTORY_LIMIT_BYTES: usize = 65_536;` (`src-tauri/src/pty/output.rs:233`), `const UI_HISTORY_LINE_SCAN_BYTES: usize = 4_096;` (`output.rs:237`), `const UI_HISTORY_REPLAY_PROLOGUE: &[u8] = b"\x1b[?1049l\x1b[r\x1b[?7h\x1b(B\x1b[0m";` (`output.rs:241`). The prologue deliberately carries no erase sequence.
- `fn append_history(history: &mut std::collections::VecDeque<u8>, data: &[u8])` (`output.rs:250-267`). It trims for space by BYTES (`history.drain(..over.min(history.len()))`), then attempts a realignment that is conditional and capped: `history.iter().take(UI_HISTORY_LINE_SCAN_BYTES).position(|byte| *byte == b'\n')`, draining through the newline only `if let Some(newline)`. When the scan finds nothing, the front stays wherever the byte-space trim put it, and NOTHING records that. Its own doc comment asserts the stronger claim, "so the ring stays line aligned", which is exactly the assumption the rest of the file then relies on.
- Call site: `append_history(&mut state.history, &data);` (`output.rs:1082`), inside `handle_output`, under the `screen_parsers` mutex the whole backend shares, once per chunk, immediately after the `output_sequence` advance. The in-code comment above it pins the ordering contract between the sequence and the ring; this plan does not disturb it.
- The ring is the seed. `activate_terminal_output` (`output.rs:1158-1253`) builds `data` as `UI_HISTORY_REPLAY_PROLOGUE` followed by both `state.history.as_slices()` halves verbatim, whenever `include_history && !state.history.is_empty()` (`output.rs:1210-1222`). `screen.contents_formatted()` is used only when history is empty or history was not requested (`output.rs:1223`).
- That condition is the production path unconditionally. `attachOutput` hardcodes `includeHistory: true` (`src/shared/ipc.ts:470`, with the comment explaining why), and the command defaults it to `true` anyway (`include_history.unwrap_or(true)`, `src-tauri/src/commands/pty.rs:557`). The parser mirror is therefore reached in production only for an empty ring.
- The frontend writes the blob in one call after a full reset: `entry.terminal.reset(); ... writeTerminalBytes(entry, new Uint8Array(snapshot.data));` (`src/terminal/components/TerminalView.tsx:562-566`). There is no chunking and no partial decode, so the first byte of the seed is read by an xterm.js parser in ground state.
- Only sessions past 64 KiB are exposed. While `over == 0` the front never moves and is the first byte the session ever emitted, so every attach seeds cleanly. This is the "it worked for the first minutes" window the report describes. One arithmetic case escapes that reasoning and is fixed by the guard in section 5.6; see 7.2.
- Byte-level signature from the incident (dev-rust, Step 1): session `9d6d5678` printed `8;2;153;153;153m` at row 1 col 1 (three bytes of `\x1b[38;2;153;153;153m` eaten) and session `c8c1088c` printed `38;2;157;157;157m` (two bytes eaten). Two different offsets inside the same CSI family is a random byte cut, not a parser or grid defect.
- Density of the real stream, measured at Step 5 over 41 samples of Claude Code v2.1.237 PTY bytes taken from `spawn_diagnostics` `head=` fields in the incident `app.log`: one visual line averages **234 bytes**, and **64% of all bytes sit inside an escape sequence**. A front at a random offset is therefore inside a sequence about two times in three.
- Not a #1439 regression. `append_history` and the three ring constants are byte-identical across `243c6c70..1376c2b8`; the only diff on those lines is indentation. The #1439 guards protect the parser GRID, and the seed's CONTENT does not come from the parser, so those guards are correct and simultaneously inert here. Corroborated in the 35 MB `app.log`: zero `terminal-snapshot` lines of any stage, zero `[ERROR]` in the incident run.
- Trigger is volume, not an event, which is why no global trigger exists in the timeline: each session crosses 64 KiB independently within minutes, and a Claude Code spinner rewrites one line per ~100 ms with `\r` and no `\n`, so a session that has been thinking for minutes has a ring that is entirely newline-free. The capped scan then fails on every chunk, and the ring simultaneously holds no conversation content. That single fact explains all three reported symptoms at once: literal SGR tail at 1,1, near-empty screen, live output still painting.
- No other mutation site exists. Grepping `history` across `output.rs` yields exactly two writes: construction with `VecDeque::with_capacity(UI_HISTORY_LIMIT_BYTES)` (`output.rs:920`) and `append_history` (`output.rs:1082`). Nothing clears, replaces, or rotates the ring elsewhere, which is what makes a single alignment flag sufficient and non-drifting. Step 6 re-derived this independently and additionally confirmed that `register_session` refuses a second registration for a live id (`output.rs:927-929`), so no ring can outlive its flag or be adopted by a fresh one.

Identified cause, in one sentence: `append_history`'s line-boundary realignment is best effort and capped at 4 KiB, its failure is silent and sticky for as long as the ring's front sits inside a newline-free region, and the attach seed consumes that front verbatim as the first byte a freshly reset parser reads.

## 4. In scope / out of scope

In scope:

1. Record, on the per-session replay state, whether the ring's front is known to sit at a line start. Written only by `append_history`, and only when the front actually moved or was newly installed.
2. At the attach seed site only, when that flag says the front is NOT known-aligned, realign by scanning the whole ring for its first `\n` and seed from the byte after it.
3. When the ring holds no `\n`, or holds nothing after its first `\n`, seed `screen.contents_formatted()` instead, which is the already-supported and already-tested mirror path.
4. One WARN whenever the seed had to deal with an unaligned ring, recording whether content survived, emitted after the parser lock is released.
5. Correct `append_history`'s doc comment so it no longer claims a guarantee it does not provide.
6. Five regression tests in `output.rs`'s `mod tests`.

Out of scope, explicitly:

- **#1459 (width-mismatch wrapping) is out of scope.** `applySnapshot` resizes the xterm to `snapshot.rows`/`snapshot.cols`, which come from the parser, and then writes ring bytes that may have been laid out at a different width (`src/terminal/components/TerminalView.tsx:605-617`). Verified in the same log: session `356dadc6` ran at 222 columns at 23:12:52 and returned to 81 columns at 23:21:16 with a ring full of 222-column layout. That defect corrupts WRAPPING, never prints a literal sequence tail, is a frontend-side concern, and must not be touched by this change.
- The mirror's alternate-screen mode inconsistency (section 5.4). Accepted as a known consequence here; a follow-up issue is requested of the coordinator and must not be filed or fixed inside this change.
- Raising the hot-path scan cap, or making `append_history` guarantee alignment. Section 5.2.
- Recovering the accidentally-aligned front that the flag conservatively reports as unaligned. Section 5.7.
- Per-chunk diagnostics in `append_history`. Section 5.5.
- Any change to the ring's size, retention policy, or the prologue.
- Any repro instrumentation build. The cause is established by code plus the byte-level screenshot signature; no further evidence is required before the fix.

## 5. Decided solution

### 5.1 Shape

Move the alignment GUARANTEE to the cold path, and keep the hot path's cheap opportunistic realignment as the fast case. The hot path keeps doing exactly what it does today to the ring's bytes, and additionally leaves behind the one bit of information the cold path needs in order to know whether it must do anything at all.

Four states result at seed time, and each has exactly one decided outcome:

| Ring state at attach | Seed |
|---|---|
| Empty, or history not requested | `screen.contents_formatted()` (unchanged, already tested) |
| Front known line-aligned | `UI_HISTORY_REPLAY_PROLOGUE` + both ring slices verbatim (unchanged, byte for byte) |
| Front not known line-aligned, ring has a `\n` with content after it | `UI_HISTORY_REPLAY_PROLOGUE` + the ring from the byte after its first `\n`, plus a WARN with `kept > 0` |
| Front not known line-aligned, no such `\n` | `screen.contents_formatted()`, plus a WARN with `kept=0` |

### 5.2 Why the flag, and why not an unconditional cold-path scan

The obvious minimal fix, and the one sketched in the issue, is to drop the flag and always start the seed at the ring's first `\n`. That is rejected. **The judgment call the Step 4 draft left open is now closed on measured evidence and is not reopened.**

When the hot-path trim DOES find a newline it drains through it, so the ring's front is already the first byte of a line. An unconditional cold scan then skips to the NEXT `\n`, discarding that whole first line on every attach of every session past 64 KiB, which is the overwhelmingly common healthy case. The discarded amount is not bounded by "one short line": it is the distance from the front to the next newline.

The draft said this rejection would be revisited if an enricher showed that a newline-free block at the ring front cannot exceed one line in real output. Step 5 measured the opposite, on real Claude Code v2.1.237 bytes from the incident log: a visual line averages 234 bytes, 64% of bytes sit inside an escape sequence, and the observed incident had rings that were **entirely** newline-free across all 65 536 bytes. The newline-free block at the front is bounded by the ring, not by a line. An unconditional cold scan on a healthy attach of a session that has just left a thinking phase would discard a multi-kilobyte block that is still in the ring and would otherwise have replayed. Step 5's author, who proposed the unconditional scan, withdrew it on this evidence; Step 6 attacked the flag design across thirteen scenarios (G.8) and did not break it.

One argument that is NOT available in defence of the unconditional scan, recorded so nobody reaches for it: "the hot trim already drops a whole line per trim, so one more is free". Measured on section 9.2's own fixture, each overflow drains 50 bytes for space and 52 more to realign, total 102, to admit 102. That is exactly the space needed, not a bonus loss.

The flag costs one `bool` on a struct that is one-per-session, and one branch assignment inside a block that already runs only when the ring overflowed. It cannot drift: the ring has exactly two write sites in the whole file (section 3), construction and `append_history`, so the flag has exactly two too.

Raising `UI_HISTORY_LINE_SCAN_BYTES` to cover the ring is separately rejected: that scan runs per chunk inside the parser mutex shared by every session of the backend, and the constant's own doc comment states that bounding it is the point. The attach is cold and per user action, so the full scan is free there and unaffordable in `append_history`.

### 5.3 Why `\n` is the resync point, and why nothing else works

A seed that starts mid-sequence cannot be repaired in band. The receiving parser is in ground state at byte 0 of the seed, so `\x18` (CAN), `\x1a` (SUB), or any other abort control is a no-op: there is no sequence in progress to abort, and `;153;153m` is simply printable text to it. The only way to avoid printing the tail is to not send it, which means skipping forward to a byte offset that is provably a ground-state boundary. `\n` is the one such offset recoverable from the ring alone, and it is already the boundary `append_history` uses, so this plan introduces no new assumption about the byte stream.

Recorded so this section is not later read as a proof of ground-state safety (E.5, confirmed at G.8): a `\n` inside a string-terminated sequence (OSC, DCS, APC, PM, SOS) is not a ground-state boundary, and draining through it would put the front inside the string. The guarantee therefore holds for streams whose newlines all sit outside string-terminated sequences, which is every stream this app has carried: the only OSC here is the title, `\x1b]0;...\x07`, with no newline, visible verbatim in the incident `head=` samples. The hot path has always made this assumption and the cold path does not widen it, because a `\n` deeper in the ring is no more likely to sit inside a string sequence than one near the front.

### 5.4 Why the parser mirror is the right fallback

- It is grid-consistent by construction at this point in the function: the #1439 branch above already returned `None` for any attach where the parser grid disagreed with `conpty_size`, so control only reaches the seed assembly when the two agree.
- It is not a new path. It is the same expression the empty-ring and `include_history == false` paths already produce, pinned today by `activation_payload_falls_back_to_screen_when_history_empty` and `activation_payload_ignores_history_when_not_requested`.
- Nothing of value is lost in the case that triggers it. A ring with no usable line start in 64 KiB is, in the observed incident, 64 KiB of spinner frames rewriting a single line: it contains no conversation history to preserve, and the mirror renders that same single line correctly.

Two properties of the mirror are known, accepted, and stated here so nobody misreads what a fallback attach should look like:

- **It is exactly one screen, with no scrollback.** The parser is built as `vt100::Parser::new(rows, cols, 0)` (`output.rs:912`), so `contents_formatted()` returns the visible grid and nothing else. After a `kept=0` attach the user sees one correctly rendered screen and no scrollback. That is the intended result, not a defect.
- **It is grid-consistent but not MODE-consistent.** `contents_formatted()` emits the alternate grid's content when the session is in the alternate screen (`vt100` 0.15.2, `screen.rs:742-748`) but emits no `\x1b[?1049h`, while the frontend's `terminal.reset()` immediately before the write leaves xterm.js in the normal buffer. A mirror seed of an alt-screen session therefore renders alternate content in the normal buffer. The desync class is pre-existing (the ring path's own prologue starts with `\x1b[?1049l`), but this plan gives it new reach: until now the mirror was production-reachable only for an EMPTY ring, and a session with an empty ring cannot yet be in the alternate screen. Accepted as a cosmetic consequence of a fallback that is otherwise strictly better than a garbled seed. A follow-up issue is requested of the coordinator; emitting a mode sequence here is a binding non-goal (section 2), because two existing tests pin the mirror's bytes and the alt-screen story is not #1458's.

### 5.5 Why the diagnostic goes on the cold path, and outside the lock

Step 1's analysis offered an optional WARN inside `append_history`'s failed-scan arm, which would require threading the session id into it. Rejected: that arm fires once per chunk for the entire duration of a newline-free region, on the hot reader path, inside the shared parser mutex, and it would log most loudly exactly when the session is producing the most output.

One WARN at the attach instead, which is per user action, and which closes the real observability gap this incident exposed: `activate_terminal_output` logs nothing on the happy path, which is why the first occurrence of this defect could not be pinned anywhere in 35 MB of `app.log`. It also distinguishes the new fallback from the pre-existing empty-ring fallback, which is otherwise indistinguishable in the payload.

**It is emitted after `drop(parsers)`, not inside the parser lock.** The Step 4 draft put it inside the `catch_payload_unwind` closure because that is where its values are in scope; Step 6 (G.4) showed that is an argument about convenience, not about the lock, and that this function already refuses it: `reconcile_fault` is set inside the lock and its `log::error!` runs after `drop(parsers)`, carrying the in-code rule "#1439 R2: flush at the transition, OUTSIDE the parser lock. An emit under that lock stalls the PTY reader on its next chunk" (`output.rs:1243-1251`). The `log` implementation writes synchronously under a file mutex and can trigger a rotation that renames a multi-megabyte file (`logging.rs:259`, `logging.rs:275`); the incident's `app.log` was 35 MB, so that is a live cost, and every PTY reader thread in the process would block on `screen_parsers` for its duration. Frequency makes it worse rather than better: on the container backend a single frame can be the whole 64 KiB ring, so any newline-free frame above the 4 KiB scan window flips the flag and makes this WARN the common case. `reconcile_fault` proves the values can be carried out; section 5.6 carries them out the same way.

`attach_grid_mismatch` (`output.rs:1191`) stays where it is: it fires on a divergence, not on every attach, and this plan does not touch it.

### 5.6 Code shape (normative)

The implementer must produce this shape. Comment wording may be edited for accuracy; the branch structure, the names, the guard's `<`, and the emptiness handling may not.

New field on `ScreenReplayState` (`output.rs:58-75`), placed **between `history` and `conpty_size`**. `history` is not the last field of the struct: `conpty_size` follows it (`output.rs:76`, added by #1439), and a careless read of "after `history`" that puts the field at the end of the struct also compiles:

```rust
    /// Whether the ring's front byte is known to sit at the start of a line, that is, at a
    /// point a parser in ground state can start reading from. Starts true, which is correct
    /// for a ring that is still growing from the first byte the session emitted, and is
    /// corrected by `append_history` both when a trim moves the front and when an oversized
    /// chunk installs a truncated one. Conservative in one direction only: `false` never
    /// means the front is definitely unsafe, it means nothing proved it safe (#1458).
    history_aligned: bool,
```

Construction (`output.rs:911-922`), immediately after the `history` field:

```rust
            history_aligned: true,
```

`append_history` (`output.rs:250-267`) gains the flag and records both cases in which the front changes. Nothing else in it changes:

```rust
fn append_history(
    history: &mut std::collections::VecDeque<u8>,
    aligned: &mut bool,
    data: &[u8],
) {
    let tail = &data[data.len().saturating_sub(UI_HISTORY_LIMIT_BYTES)..];
    let over = (history.len() + tail.len()).saturating_sub(UI_HISTORY_LIMIT_BYTES);
    if over > 0 {
        history.drain(..over.min(history.len()));
        // The scan stays capped: this runs per chunk inside the parser mutex the whole
        // backend shares. What #1458 changes is only that its failure is now RECORDED
        // instead of assumed away, so the cold attach path knows it has work to do.
        match history
            .iter()
            .take(UI_HISTORY_LINE_SCAN_BYTES)
            .position(|byte| *byte == b'\n')
        {
            Some(newline) => {
                history.drain(..=newline);
                *aligned = true;
            }
            None => *aligned = false,
        }
    }
    // #1458: the one path on which the front changes without `over` ever being positive.
    // When `tail` becomes the WHOLE ring the front is `tail[0]`, and a chunk larger than the
    // ring was truncated at an arbitrary byte, so that front is not a line start and nothing
    // above recorded it. The `<` is load bearing: `tail.len() == data.len()` means the chunk
    // was NOT truncated, so `tail[0]` is a real stream boundary and `true` is correct.
    if history.is_empty() && tail.len() < data.len() {
        *aligned = false;
    }
    history.extend(tail);
}
```

Its doc comment must lose the unconditional claim. Replace "so the ring stays line aligned and its length never exceeds the limit" with wording that says the length bound is guaranteed and the line alignment is best effort, recorded in `aligned` and enforced at the attach seed (#1458).

New private free function, immediately after `append_history`:

```rust
/// The ring sliced from the byte after its first `\n`, or `None` when the ring holds no `\n`
/// at all or holds nothing after it.
///
/// Only the cold attach path calls this, so the scan is unbounded on purpose. `\n` is the one
/// resync point a replay can trust: a parser reading the ring from an arbitrary byte offset
/// renders the tail of whatever escape sequence that offset falls inside as literal text
/// (#1458), and no in-band cancel undoes it, because that parser is already in ground state.
fn history_from_first_line<'a>(front: &'a [u8], back: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
    if let Some(newline) = front.iter().position(|byte| *byte == b'\n') {
        let aligned_front = &front[newline + 1..];
        if aligned_front.is_empty() && back.is_empty() {
            return None;
        }
        return Some((aligned_front, back));
    }
    let newline = back.iter().position(|byte| *byte == b'\n')?;
    let aligned_back = &back[newline + 1..];
    if aligned_back.is_empty() {
        return None;
    }
    Some((&[], aligned_back))
}
```

The explicit `'a` is required: with two reference parameters, lifetime elision cannot pick an output lifetime. The emptiness check on the first branch is on BOTH halves on purpose; section 7.2 row 8 is the case it decides, and section 9.5 pins it.

Call site (`output.rs:1082`):

```rust
                        append_history(&mut state.history, &mut state.history_aligned, &data);
```

The two `&mut` borrows are of disjoint fields of `state` and are accepted by the borrow checker.

Seed assembly (`output.rs:1179-1251`). The replay decision moves OUT of the unwind-guarded closure so the WARN can be emitted after the lock is released (section 5.5). Slicing the ring cannot panic (`newline < len`, so `newline + 1` is at worst `len`), so nothing is lost by deciding it outside. Declare the carrier next to the existing `reconcile_fault`:

```rust
        let mut reconcile_fault = false;
        // #1458: carried out of the parser lock exactly like `reconcile_fault`. `Some` only
        // when the ring was flagged unaligned; the pair is (ring length, bytes kept).
        let mut history_unaligned: Option<(usize, usize)> = None;
```

and replace the current `let copied = crate::logging::catch_payload_unwind(|| { ... });` with:

```rust
                let copied = {
                    let uses_history = include_history && !state.history.is_empty();
                    // `as_slices` keeps this a read: `make_contiguous` needs `&mut` and
                    // rotates the buffer during what is otherwise a copy out.
                    let replay = if uses_history {
                        let (front, back) = state.history.as_slices();
                        // #1458: the hot-path realignment is capped at 4 KiB and stays failed
                        // for as long as the front sits in a newline-free region, so it can
                        // leave the front inside an escape sequence. Attach is cold: pay the
                        // full scan here rather than seed a literal sequence tail.
                        if state.history_aligned {
                            Some((front, back))
                        } else {
                            history_from_first_line(front, back)
                        }
                    } else {
                        None
                    };
                    if uses_history && !state.history_aligned {
                        history_unaligned = Some((
                            state.history.len(),
                            replay.map_or(0, |(front, back)| front.len() + back.len()),
                        ));
                    }
                    crate::logging::catch_payload_unwind(|| {
                        let screen = state.parser.screen();
                        let (rows, cols) = screen.size();
                        let cells = usize::from(rows).checked_mul(usize::from(cols)).ok_or(())?;
                        if rows > MAX_ROWS || cols > MAX_COLUMNS || cells > MAX_CELLS {
                            return Err(());
                        }
                        let data = match replay {
                            Some((front, back)) => {
                                let mut bytes = Vec::with_capacity(
                                    UI_HISTORY_REPLAY_PROLOGUE.len() + front.len() + back.len(),
                                );
                                bytes.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
                                bytes.extend_from_slice(front);
                                bytes.extend_from_slice(back);
                                bytes
                            }
                            // No line start with content behind it. The ring cannot be
                            // replayed from any offset, and in the observed incident it holds
                            // nothing but spinner frames anyway. The mirror is a consistent
                            // full repaint on a grid the #1439 branch above already validated.
                            None => screen.contents_formatted(),
                        };
                        Ok::<PtyScreenSnapshot, ()>(PtyScreenSnapshot {
                            data,
                            rows,
                            cols,
                            sequence: state.output_sequence,
                        })
                    })
                };
```

The enclosing block is what makes `replay`'s borrow of `state.history` end before `match copied`'s `Err(_)` arm takes `&mut state`. `match copied { ... }` itself is unchanged. `Option<(&[u8], &[u8])>` is `Copy`, so reading `replay` for `history_unaligned` and then matching on it needs no clone.

Finally, after `drop(parsers)` and before the existing `if reconcile_fault` block:

```rust
        if let Some((ring, kept)) = history_unaligned {
            log::warn!(
                "[terminal-snapshot] stage=attach_history_unaligned session={id} ring={ring} kept={kept} (#1458)"
            );
        }
```

Inline format captures throughout, matching the neighbouring log lines in this file.

### 5.7 The flag is a conservative under-approximation, and must not be sharpened

Recorded so it is not later mistaken for something stronger, and so the sharpening is not attempted (G.9). When the byte-space drain happens to land exactly after a `\n` and the next `\n` is beyond the 4 KiB scan window, the front IS a line start but the flag says `false`. The cold path then skips to the next `\n` and discards a legitimately replayable block, or takes the mirror if there is none. With a measured 234-byte average line that is roughly a 1-in-234 chance per attach, the loss is bounded by the distance to the next newline, and the outcome is always safe rather than garbled.

Making `append_history` inspect the last drained byte to recover that case is rejected: it extends the hot path beyond "record the outcome of the scan you already run", which section 2 forbids, and it buys a fraction of a percent.

## 6. Affected surfaces, exhaustively

One file: `src-tauri/src/pty/output.rs`.

| Symbol | Location at frozen SHA | Change |
|---|---|---|
| `ScreenReplayState` | `output.rs:58-75` | one added field, `history_aligned: bool`, between `history` and `conpty_size` |
| `SessionIoFanout::register_session` construction of `ScreenReplayState` | `output.rs:911-922` | one added initializer, `history_aligned: true` |
| `append_history` | `output.rs:250-267` | doc comment corrected; one added `&mut bool` parameter; the `if let Some` realignment becomes a `match` whose two arms set the flag; one three-line guard before `history.extend(tail)`. Ring bytes unchanged. |
| `history_from_first_line` | new, immediately after `append_history` | new private free function |
| `SessionIoFanout::handle_output` | `output.rs:1082` | one argument added at the `append_history` call |
| `SessionIoFanout::activate_terminal_output` | `output.rs:1179-1251` | one added local next to `reconcile_fault`; the replay decision moves out of the unwind closure and gains the alignment branch and the mirror fallback; one WARN after `drop(parsers)` |
| `mod tests` | after `history_ring_is_bounded_and_line_aligned`, `output.rs:3207-3232` | five added tests |

Not touched, and expected byte-identical after the change: every other Rust file, every TypeScript file, `Cargo.toml`, `package.json`, every config and schema.

Log surface: one new stage string, `[terminal-snapshot] stage=attach_history_unaligned`. No existing stage is renamed, reused, or silenced. One consequence worth recording (E.7): `[terminal-snapshot]` currently carries three stages in this file (`attach_grid_mismatch` at `output.rs:1191`, `parser_fault` at 1099/1246/1368/1581, `resize_skipped` at 1344/1348/1355), and "zero `terminal-snapshot` lines in the log" was the single fact that proved the #1439 guards were inert. After this ships, that grep no longer means what it meant; anyone checking whether the #1439 guards fired must filter by stage.

### 6.1 Dependency-cycle gate

New module-to-module arcs added by this plan: **zero**. New arcs removed: zero. Enumerated:

- `history_from_first_line` is a private free function in `agentscommander_lib::pty::output`, called only from `activate_terminal_output` in the same module. Intra-module, not an arc.
- `history_aligned` is a field on a private struct in the same module, read and written only within it.
- The new `log::warn!` targets the `log` facade, which `pty::output` already depends on and already calls at `output.rs:1099`, `1191`, `1246`, `1338`, `1344`, `1348`, `1355`. No new dependency edge.
- No `use` statement is added, removed, or moved. No module is created, renamed, split, or relocated. No file is added to or removed from any module tree.

Consequence for the `rust-levelization-run` criterion: because the change adds no `use` and no module, the arc record is expected byte-identical, `cyclicSccs` unchanged, and every SCC member set identical. Role/layering hygiene: no lower-layer module gains an `AppHandle`/`tauri` dependency; the change is entirely inside a module that already sits at this layer, and both new units (`history_from_first_line`, the flag) are pure, transport-free, and below the transport-taking function that calls them. `src-tauri/tests/claude_watcher_layering.rs` targets `telegram::claude_watcher`, not `pty::output`, and pins no dependency set that this change touches (verified by read at the frozen SHA).

This gate was satisfied on enumeration at Step 4, re-derived independently at Step 5 (E.6) and Step 6 (G.8), and re-affirmed at Step 7 round 1: none of the amendments in this round adds a `use` or a `mod`. Criterion 9 mechanizes it for the implementation.

## 7. Required behavior, edge cases, failure behavior

### 7.1 Required behavior

1. For every attach with a non-empty ring whose front is known line-aligned, the emitted `data` must be exactly `UI_HISTORY_REPLAY_PROLOGUE` followed by the ring's bytes in order, with nothing dropped from either end. This is today's behavior and must remain byte-identical.
2. For every attach with a non-empty ring whose front is not known line-aligned AND whose ring holds a `\n` with at least one byte after it, the emitted `data` must be `UI_HISTORY_REPLAY_PROLOGUE` followed by the ring's bytes starting immediately after that first `\n`, with nothing further dropped. Falling back to the mirror in this case is a defect, not a safe alternative: it silently downgrades a full 64 KiB replay to a single viewport with no scrollback.
3. For every attach with a non-empty ring whose front is not known line-aligned and which holds no `\n` with content after it, the emitted `data` must be exactly `screen.contents_formatted()`.
4. The emitted `data` must never begin with ring bytes that precede the ring's first `\n` when the front is not known aligned.
5. `state.history`'s bytes, length, and capacity after any sequence of `handle_output` calls must be identical to what the pre-change code produces for the same sequence.
6. `history_aligned` must be `true` for every session that has not yet overflowed the ring and has never received a chunk larger than `UI_HISTORY_LIMIT_BYTES`.

### 7.2 Edge cases, each with its decided outcome

| Case | Outcome |
|---|---|
| Empty ring | Mirror, no prologue. Unchanged; the `!state.history.is_empty()` guard runs first and the flag is never consulted. |
| `include_history == false` | Mirror, no prologue. Unchanged; the flag is never consulted. |
| Ring still growing, no chunk ever exceeded the limit (`over == 0` throughout) | Flag stays `true` from construction, seed is the full ring. Unchanged. The qualification matters: the unconditional form of this row in the Step 4 draft was false for the row below. |
| First chunk into an empty ring, `data.len() <= UI_HISTORY_LIMIT_BYTES` | `tail == data`, `over == 0`, no scan, flag stays `true`, and that is CORRECT: the front is the chunk's byte 0, a real stream boundary. |
| First chunk into an empty ring, `data.len() > UI_HISTORY_LIMIT_BYTES` | `tail` is truncated to the ring's size, `over` is still 0, so no scan runs and the front is an arbitrary byte of that chunk. The `history.is_empty() && tail.len() < data.len()` guard records `false`, and the cold path realigns or falls back. Unreachable through either production backend today (see 7.5); the guard exists so the flag's meaning survives a future one. |
| `\r`-only spinner content, newline-free stretch SHORTER than `UI_HISTORY_LINE_SCAN_BYTES` | The hot-path scan finds a `\n` within 4 KiB, drains through it, sets the flag `true`. Cold path does nothing. Seed is the full ring, byte-identical to today. |
| `\r`-only spinner content, newline-free stretch LONGER than the scan window but shorter than the ring | Flag `false`. Cold scan finds the first `\n` further in and seeds from after it, dropping only the bytes that cannot be replayed from any offset. WARN with `kept > 0`. Pinned by 9.4. |
| Entire ring newline-free (the reported incident) | Flag `false`, `history_from_first_line` returns `None`, seed is the mirror, no prologue. WARN with `kept=0`. Pinned by 9.1. |
| Ring whose only `\n` is its last byte | `history_from_first_line` finds the newline but the remainder is empty in both halves, so it returns `None`: seed is the mirror. Seeding prologue-plus-zero-bytes would blank the attaching terminal, which is strictly worse than the mirror. WARN with `kept=0`. Pinned by 9.3 (which Step 6 verified is layout-independent) and by 9.5. |
| Ring whose only `\n` is the last byte of `front`, with `back` non-empty | `aligned_front` is empty but `back` is not, so this is `Some((&[], back))`: a normal aligned seed. The emptiness check is deliberately on BOTH halves. Not reachable at will through the fanout, so pinned by 9.5's direct call. |
| Ring split so that `front` has no `\n` and `back` does | Handled by the second scan; the seed is `back` from after its newline, `front` is dropped entirely. Correct: everything before that newline is unreplayable. Pinned by 9.5's direct calls, on both its `Some` and its `None` outcome. |
| Front is accidentally a line start but the flag says `false` | Cold path realigns to the next `\n` (or takes the mirror). Safe, never garbled, and deliberately not sharpened; section 5.7. |
| `UI_HISTORY_REPLAY_PROLOGUE` | Emitted on the aligned and the realigned seed paths only, exactly as today for the aligned one. Never emitted on the mirror path, on either the pre-existing or the new route into it. |
| Session in the alternate screen when the mirror fires | Alternate content rendered in the normal buffer. Known, accepted, out of scope; section 5.4. |
| Parser grid disagrees with `conpty_size` (#1439) | Returns `None` before reaching this code. Unchanged; this plan runs only inside the already-validated branch. |
| Poisoned parser lock, missing session, identity mismatch, `parser_availability != Available` | All return before the seed assembly. Unchanged. |
| Parser flipped to `Unavailable` by a panic in `append_history` | The ring stops being appended to and the attach never reaches the seed assembly (`output.rs:1180` gate), so a frozen ring is never seeded and the flag cannot go stale against a moving front. |
| Zero-length chunk (the container bridge admits one) | `over == 0`, the guard's `tail.len() < data.len()` is false, nothing is appended and the flag is untouched. `output_sequence` still advances, as today. |
| Oversized chunk into a NON-empty ring | `over == history.len()`, the ring is fully drained, the capped scan runs on an empty ring, finds nothing and records `false`. Correct before and after the guard. `history_ring_is_bounded_and_line_aligned` continues to pin the arithmetic. |

### 7.3 Failure behavior

The new code cannot fail in a way the surrounding code does not already handle. `history_from_first_line` performs no arithmetic that can overflow (`newline + 1` where `newline < len`, so the slice index is at most `len`), takes no lock, and allocates nothing; the `Vec::with_capacity` on the `Some` arm is computed from the SLICED lengths, so it is exact and never smaller than what is pushed. The ring cannot be made to reallocate: `over <= history.len()` always, because `tail.len() <= UI_HISTORY_LIMIT_BYTES`.

The replay decision now sits outside `catch_payload_unwind`, which is sound because it is pure slice arithmetic with no panic path. Everything that can panic (`screen.size()`, `contents_formatted()`, the allocation) stays inside, so an unforeseen panic still degrades exactly as today: the parser flips to `Unavailable`, `stage=parser_fault` is logged, the attach proceeds with no snapshot and the window writes live.

There is no partial-failure state: the flag is written only on paths that also mutate the ring, under the same mutex, in the same critical section. Two windows attaching the same session serialize on `screen_parsers`, and an attach mutates neither the ring nor the flag, so it is idempotent and both windows get identical seeds. There is no `.await` anywhere on this path.

### 7.4 Why the ring's TAIL needs no work

The ring's last bytes are the last bytes of the most recent chunk, which is a PTY read boundary and can therefore also fall mid-sequence. That is harmless and must not be "fixed":

- xterm.js's parser is stateful across `write()` calls, so a sequence split across the seed boundary is completed by the next live chunk.
- That next chunk is guaranteed to be the immediate successor of the seed. `append_history` and the `output_sequence` advance both run under the `screen_parsers` mutex (`output.rs:1068-1082`), and `activate_terminal_output` copies the ring and calls `self.attachments.attach(id, label)` while still holding the same lock, dropping it only afterwards (`output.rs:1243-1244`). The ring cannot advance between the copy and the attach.
- The frontend reconciles by watermark, skipping only events at or below `snapshot.sequence` (`entry.lastAppliedSequence = snapshot.sequence`, `TerminalView.tsx:564`), so the first live chunk it applies is exactly the one after the seed.

The head is different in kind precisely because a `reset()` precedes it: the reset is what destroys the parser state that would otherwise have completed the leading sequence.

### 7.5 Reachability of the oversized-chunk guard, for the record

The guard added by section 5.6 covers a case no production caller can reach today. Verified at Step 6 by reading both entry points:

- Local backend: `let mut buf = [0u8; 4096];` then `fanout.handle_output(..., buf[..n].to_vec())` (`local_backend.rs:1653-1662`). Hard ceiling 4 096.
- Container backend: `handle_bridge_output` rejects with `AppError::PtyError` when `data.len() > MAX_TRANSPORT_FRAME_BYTES` (`container_backend.rs:1682-1686`), and `MAX_TRANSPORT_FRAME_BYTES = crate::pty::backend::PTY_INPUT_MAX_BYTES = 65_536` (`container_backend.rs:35`, `backend.rs:21`), capped again twice in `session_transport.rs:54-55`. The ceiling is 65 536 INCLUSIVE, and 65 536 is exactly the value that does not trigger the hole; the hole needs 65 537.

`handle_output` has no other non-test caller. The guard is therefore an invariant repair, not a reachability fix: it exists so the flag keeps its meaning under a future backend, a raised frame ceiling, or a coalescing reader. Section 9.6 records why it is deliberately left without a dedicated test and which existing test catches the dangerous mutation of it.

## 8. Compatibility and security

- IPC: unchanged. No command signature, payload field, event name, or serde attribute is touched. `PtyScreenSnapshot` and `PtyScreenSnapshotPayload` keep their exact shapes, so `src/shared/types.ts` needs no edit.
- Frontend: unchanged, and required to stay unchanged. A frontend built before this change interoperates with a backend built after it, in both directions: the only observable difference is which bytes the existing `data: Vec<u8>` field carries.
- Persistence and configuration: unchanged. No TOML key, no migration, no on-disk format.
- Platform: no platform-specific code. Nothing here touches paths, ConPTY, or shell wrapping.
- Security: none of the change crosses a trust boundary. The seed is already produced from bytes the session itself emitted and already delivered to the same window; this change can only make it a strict sub-slice of what it is today, or the parser mirror. The new WARN logs a session id and two byte counts, never output content, matching the existing `stage=` lines in this file.
- Performance: the hot path gains one branch assignment inside a block that already only runs on ring overflow, plus one `is_empty()` test per chunk, and no additional scan. The cold path gains at most one 64 KiB linear scan per attach, which is negligible beside the `vt100::Parser::process` this same mutex already serializes, and the WARN it may emit now runs outside the lock.

## 9. Tests and objective acceptance criteria

Five tests, all in `mod tests` of `src-tauri/src/pty/output.rs`, placed immediately after `history_ring_is_bounded_and_line_aligned` (`output.rs:3207-3232`). All existing helpers are reused as-is: `fanout()` (`output.rs:1945`), `session()` (`output.rs:1971`, registers 30x120), `feed()` (`output.rs:1964`), `activation_data()` (`output.rs:3175`), `mirrored_screen()` (`output.rs:3183`), `WINDOW` (`output.rs:2161`). No new helper is required.

Numbering note: sections 9.4 and 9.5 are the two tests added at Step 7 round 1. The Step 4 draft's "9.4 existing tests" and "9.5 acceptance criteria" are now 9.6 and 9.7; the Step 5 and Step 6 sections below refer to them by their draft numbers.

All spinner fixtures use a **33-byte** frame, corrected at Step 5 (E.4) and re-verified at Step 7. `\x1b[38;2;153;153;153m* Drizzling..\r` is 19 + 13 + 1 = 33 bytes, `65536 mod 33 = 31`, so the steady-state front sits at offset `33 - 31 = 2` inside a frame and the ring's first bytes are `38;2;153;153;153m...`: the wg-18 `c8c1088c` incident signature verbatim. The Step 4 draft's frame was 31 bytes, not 30, and put the cut at offset 29 (the `g` of `Drizzling`), outside any escape sequence. The tests passed anyway because the `None` arm decides them, but a fixture that demonstrably does not do what its comment claims is the kind of thing a later maintainer "corrects" in the wrong direction.

### 9.1 Test 1, red today, green after: a newline-free ring must not seed a partial sequence

```rust
/// #1458. A ring saturated by a newline-free stream (a coding agent's spinner rewriting one
/// line with `\r`) leaves the ring's front at an arbitrary byte offset, which lands inside an
/// escape sequence most of the time. The seed must never emit that sequence's literal tail;
/// with no `\n` anywhere in the ring there is nothing to realign to, so the attach takes the
/// parser mirror.
#[test]
fn a_newline_free_ring_seeds_the_mirror_instead_of_a_partial_sequence() {
    let fanout = fanout();
    let id = session(&fanout);
    // A realistic spinner frame: truecolor SGR, label, carriage return. No `\n`. 33 bytes, so
    // the steady-state front lands 2 bytes into the SGR, exactly where the incident cut.
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r";
    for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
        feed(&fanout, id, &[frame]);
    }
    {
        // The precondition of the defect, asserted rather than assumed.
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert_eq!(state.history.len(), UI_HISTORY_LIMIT_BYTES);
        assert!(!state.history_aligned);
    }

    let expected = mirrored_screen(&fanout, id);
    let data = activation_data(&fanout, id, true);

    assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(data, expected);
}
```

Today this fails: the seed is the prologue followed by ring bytes whose first byte is the `3` of `38;2;153;153;153m`, so the attaching terminal prints `38;2;153;153;153m* Drizzling..` as literal text at row 1 col 1.

### 9.2 Test 2, green before and after: an aligned ring still seeds the whole ring

```rust
/// #1458. The healthy path must stay byte identical: when the capped trim did realign the
/// ring, the seed is the ring verbatim, from its very first byte. Asserting the whole body
/// against the ring is the point. An alignment scan applied unconditionally would silently
/// drop the ring's first line here, and a `starts_with` on the line's prefix would still pass,
/// because every line of such a replay begins with the same SGR bytes.
#[test]
fn a_line_aligned_ring_still_seeds_the_whole_ring() {
    let fanout = fanout();
    let id = session(&fanout);
    // 102 bytes per line, not a divisor of the 65 536 byte ring, so the space trim lands off a
    // line boundary and the realignment has to do real work: 50 bytes drained for space and 52
    // more to realign, on every overflow.
    for index in 0..2_000 {
        feed(
            &fanout,
            id,
            &[format!("\x1b[38;2;153;153;153m>{index:081}\n").as_bytes()],
        );
    }
    let expected = {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert!(state.history_aligned);
        let (front, back) = state.history.as_slices();
        [front, back].concat()
    };

    let data = activation_data(&fanout, id, true);

    assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(&data[UI_HISTORY_REPLAY_PROLOGUE.len()..], expected.as_slice());
}
```

### 9.3 Test 3, red today, green after: a ring whose only newline is its last byte

```rust
/// #1458 edge case. A ring whose only `\n` is its last byte does have a line start, but has
/// nothing after it: aligning to it would seed the prologue and zero bytes of content, which
/// blanks the attaching terminal. That case must take the mirror, exactly like a ring with no
/// `\n` at all.
#[test]
fn a_ring_whose_only_newline_is_its_last_byte_seeds_the_mirror() {
    let fanout = fanout();
    let id = session(&fanout);
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r";
    for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
        feed(&fanout, id, &[frame]);
    }
    // One frame that ends in the ring's only newline. The capped trim scan still sees no `\n`
    // in the first 4 KiB, so the ring stays flagged unaligned.
    feed(&fanout, id, &[b"\x1b[38;2;153;153;153m* Drizzling..\n"]);
    {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert!(!state.history_aligned);
        assert_eq!(state.history.back().copied(), Some(b'\n'));
        assert_eq!(
            state.history.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
    }

    let expected = mirrored_screen(&fanout, id);
    let data = activation_data(&fanout, id, true);

    assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(data, expected);
}
```

Step 6 verified this test's outcome is independent of where `VecDeque`'s head sits: with the computed layout the single `\n` is the last byte of `back` and the `back` scan lands on its empty-remainder `None`; in the only other possible layout the `\n` is the last byte of `front` with `back` empty, and the both-halves emptiness check returns `None` as well.

### 9.4 Test 4, red today, green after: an unaligned ring that still holds lines must recover them

This is the only test that drives `history_from_first_line` to its `Some` arm, and it is the reason the Step 4 draft's "exactly three tests" was wrong. Without it, this implementation passes every other test and every acceptance criterion:

```rust
fn history_from_first_line<'a>(front: &'a [u8], back: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
    let _ = (front, back);
    None
}
```

and the user-visible result is that every attach to a session whose ring is unaligned but still holds replayable lines is silently downgraded from a full 64 KiB replay to a single 30-row viewport with no scrollback: the same unbounded silent content loss section 5.2 engineers the flag to prevent, reintroduced on the other path. Two weaker wrong implementations are closed by the same test: `&front[newline..]` instead of `&front[newline + 1..]` (seeds a leading `\n`), and dropping the second scan entirely.

Fixture verified in a byte-for-byte simulator of `append_history` at Step 6: after the spinner the ring is exactly full with 0 newlines and the flag is `false`; after the 40 lines the ring is still exactly full, still flagged `false` (the 4 KiB hot scan sees only spinner at the front), holds 40 newlines with the first at offset 64 053, and `kept` is 1 482, which is also the only `kept > 0` WARN the suite exercises.

```rust
/// #1458. The recovering case, and the only one that exercises `history_from_first_line`'s
/// `Some` arm: an unaligned ring that still holds lines must seed from the byte after its
/// first `\n`, not fall back to the mirror. Asserting the whole body is the point; a stub
/// helper that always returns `None` passes every other test in this file.
#[test]
fn an_unaligned_ring_with_a_later_newline_seeds_from_that_line() {
    let fanout = fanout();
    let id = session(&fanout);
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r"; // 33 bytes, no `\n`
    for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
        feed(&fanout, id, &[frame]);
    }
    for index in 0..40 {
        feed(
            &fanout,
            id,
            &[format!("\x1b[38;2;153;153;153mrecovered line {index:03}\n").as_bytes()],
        );
    }
    let expected = {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        // The 4 KiB hot scan still sees only spinner at the front, so the flag stays false
        // even though the ring now holds 40 newlines further in.
        assert!(!state.history_aligned);
        let (front, back) = state.history.as_slices();
        let ring = [front, back].concat();
        let newline = ring
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("a newline");
        ring[newline + 1..].to_vec()
    };

    let data = activation_data(&fanout, id, true);

    assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(&data[UI_HISTORY_REPLAY_PROLOGUE.len()..], expected.as_slice());
}
```

### 9.5 Test 5, red today, green after: the helper's four branches, called directly

Three decided rows of 7.2 depend on which half of the ring holds the first `\n` and on the both-halves emptiness check, and none of them is reachable at will through the fanout: `VecDeque`'s head position decides it. A direct call is the only deterministic instrument, and it is also what closes the "dropped the second scan" and off-by-one variants independently of any layout. `history_from_first_line` is private to this module and `mod tests` is a child of it, so no visibility change is needed.

```rust
/// #1458. The helper's four decided branches, pinned without a fixture because which half of
/// the ring holds the first `\n` is a `VecDeque` layout detail no fanout test can choose.
/// Covers 7.2 rows 8, 9 and 10 and the both-halves emptiness check.
#[test]
fn history_from_first_line_decides_every_branch() {
    // A newline in `front`, content behind it: seed from the byte after it, keep all of `back`.
    assert_eq!(
        history_from_first_line(b"ab\ncd", b""),
        Some((&b"cd"[..], &b""[..]))
    );
    // The newline is `front`'s last byte but `back` is not empty: still a normal seed. The
    // emptiness check is on BOTH halves for exactly this row.
    assert_eq!(
        history_from_first_line(b"ab\n", b"cd"),
        Some((&b""[..], &b"cd"[..]))
    );
    // No newline in `front`, one in `back` with content behind it: the whole of `front` is
    // unreplayable and is dropped. Deleting the second scan makes this row dead.
    assert_eq!(
        history_from_first_line(b"ab", b"cd\nef"),
        Some((&b""[..], &b"ef"[..]))
    );
    // The ring's only newline is its last byte: nothing survives it, so the caller must take
    // the mirror rather than seed a prologue and zero bytes.
    assert_eq!(history_from_first_line(b"ab", b"cd\n"), None);
    // No newline anywhere: the reported incident.
    assert_eq!(history_from_first_line(b"ab", b"cd"), None);
}
```

### 9.6 Existing tests that must keep passing, unmodified

- `history_ring_is_bounded_and_line_aligned` (`output.rs:3207`). Pins the ring's length, capacity, front byte, and the oversized-chunk arithmetic. It must not be edited, and specifically must not gain a `!state.history_aligned` assertion: its oversized feed goes into a NON-empty ring, where `over == history.len()`, the ring is fully drained and the capped scan already records `false` with or without section 5.6's guard, so such an assertion would measure nothing. This test is the proof that `append_history`'s ring behavior is unchanged.
- `activation_payload_replays_history_for_background_session` (`output.rs:3237`). Newline-rich, aligned; asserts the prologue and that the oldest line survives. **This is also the detector for the guard's dangerous mutation.** With `<=` instead of `<`, its very first 16-byte chunk into an empty ring sets `history_aligned = false`, the ring never overflows so nothing ever resets it, the attach takes the cold path and seeds from after the first `\n`, and the test's `replayed.contains("history line 0\r\n")` fails. That is why section 5.6's guard is deliberately shipped without a dedicated test of its own: the case it repairs is unreachable in production (7.5), and the way it can be broken is already pinned.
- `activation_payload_falls_back_to_screen_when_history_empty` (`output.rs:3263`).
- `activation_payload_ignores_history_when_not_requested` (`output.rs:3277`).

### 9.7 Objective acceptance criteria

The change is accepted when ALL of the following hold. Every one is a command with a binary outcome; none is a judgment call.

1. `cargo test -p agentscommander-new --lib pty::output::tests` passes with **44 passed**, and the five tests of 9.1 to 9.5 are present and green. The frozen SHA reports 39 passed, 3 500 filtered out, exit 0 (measured at Step 6), so the delta is exactly the five added tests. `agentscommander_lib` is the LIB target name, not the package name; `-p agentscommander_lib` fails with "did not match any packages".
2. Reverting only the `activate_terminal_output` hunk (leaving the flag, the guard, the helper and the tests in place) makes tests 9.1, 9.3 and 9.4 fail, and leaves 9.2, 9.5 and all four tests of 9.6 green. This is the red-before-green proof and must be demonstrated, not asserted.
3. `git diff --name-only <frozen SHA>..HEAD` lists exactly `plans/1458-attach-seed-ring-alignment.md` and `src-tauri/src/pty/output.rs`, and nothing else. No PR-time gate requires a CHANGELOG entry (`CHANGELOG.md` is referenced only by `release.yml`).
4. `git diff <frozen SHA>..HEAD -- src-tauri/src/pty/output.rs` contains no change to the three `UI_HISTORY_*` constants, no change to `history.drain(..over.min(history.len()))`, no change to `history.drain(..=newline)`, and no change to `history.extend(tail)`. It contains exactly one occurrence of `tail.len() < data.len()` and zero of `tail.len() <= data.len()`.
5. `cargo clippy --workspace --all-targets -- -D warnings` is clean. Do not run this before implementation step 4: between steps 2 and 4 `history_from_first_line` has no caller and `dead_code` is denied here.
6. `rustfmt --check --edition 2021 src-tauri/src/pty/output.rs` reports exactly **7** `Diff in` regions and **99** total output lines, the same as the frozen SHA, and none of the reported line numbers falls inside a hunk of `git diff <frozen SHA>..HEAD -- src-tauri/src/pty/output.rs`. The frozen-SHA regions are at lines 786, 2432, 2546, 2591, 2662, 2727, 2734, all pre-existing drift and all outside every hunk this plan touches; the numbers themselves shift when code is inserted above them, which is why the criterion counts regions rather than pinning line numbers. There is no `rustfmt.toml` in the repo and the edition is 2021 (`src-tauri/Cargo.toml:4`). No CI job runs any fmt check. Do NOT run bare `cargo fmt`: it rewrites unrelated files carrying that pre-existing drift.
7. The suite passes in the form CI runs, `cargo test --lib --bins --tests`, and also as `cargo test --workspace`. No test count changes other than the five added. A workspace total that is +2 against expectation is a `dist/` build artefact, not a regression.
8. `git diff --name-only <frozen SHA>..HEAD | rg "TerminalView|shared/ipc|shared/types"` returns nothing, that is, no frontend file appears in the diff. Note the pipe: `rg PATTERN FILE...` would search file CONTENTS and match this plan's own prose, which cites all three paths deliberately.
9. Dependency-cycle gate: `git diff <frozen SHA>..HEAD -- src-tauri/` adds zero lines matching `^\+\s*use ` and zero lines matching `^\+\s*mod `, so the module arc record is byte-identical and `cyclicSccs`, SCC membership, and cross-boundary arc count are all unchanged by construction. If a later change makes either count non-zero, run the `rust-levelization-run` arc criterion before certification.
10. Manual confirmation, recorded in the Step 7 report: with the log level admitting WARN for `agentscommander*` targets (the level is runtime-settable through an `AtomicU8` gate, `logging.rs:169-176`), build the app, let a session emit well past 64 KiB with a Claude Code agent left thinking for two to three minutes, then select it. The screen must not show a literal SGR tail at row 1 col 1. If the ring was fully newline-free, `app.log` must contain exactly one `stage=attach_history_unaligned ... kept=0` line for that session and that attach, and the expected on-screen result is one correctly rendered screen with no scrollback (section 5.4). A `kept=0` line is also the direct measurement of the one precondition Step 1 inferred rather than instrumented: that the CLI emits more than 64 KiB without a `\n` while spinning.

## 10. Implementation order

Single phase; this is an MVP-only fix with no follow-on phases.

1. Add `history_aligned: bool` to `ScreenReplayState`, between `history` and `conpty_size`, and initialize it to `true` at construction. Add the parameter to `append_history`, convert its `if let Some` into the two-arm `match` that sets the flag, add the `history.is_empty() && tail.len() < data.len()` guard before `history.extend(tail)`, correct its doc comment, and update the single call site. Build. Nothing observable changes yet; all existing tests stay green.
2. Add `history_from_first_line` with its doc comment. Build. Still nothing observable; the function has no caller yet, so `dead_code` will fire from here until step 4. Do not run criterion 5 in this window.
3. Add the five tests of section 9. Run them. Tests 9.1, 9.3 and 9.4 must be RED and tests 9.2 and 9.5 GREEN. Record the failure output; this is criterion 2's evidence and it must be captured before step 4.
4. Change the seed assembly in `activate_terminal_output` per section 5.6: the new local next to `reconcile_fault`, the replay decision hoisted out of the unwind closure, the mirror fallback, and the WARN after `drop(parsers)`. Run the tests. All five green, all four of section 9.6 still green.
5. Run acceptance criteria 1 and 3 through 9. Force-add the plan (`git add -f plans/1458-attach-seed-ring-alignment.md`) with the source change.
6. Perform acceptance criterion 10 and record the result.

## Dev enrichment (Step 5)

Author: dev-rust, wg-17. Appended 2026-08-20 UTC. Nothing above this heading was modified.

Method note: every claim below was checked by running something, not by reading. Where I contradict the plan I say what I ran. I re-implemented `append_history` byte for byte as a simulator and drove the plan's own fixtures through it, because three of the plan's fixture claims are arithmetic that no test asserts.

### E.1 Frozen authority re-verified, with two off-by-one corrections

`HEAD` is `1376c2b84a23125624e919c9af7e65813d624241` and `git ls-tree HEAD src-tauri/src/pty/output.rs` is blob `4f47604810cc17b399b51663fa7a17bc1c3da830`. Both match section 1. I checked all 19 line anchors the plan cites by reading the line and matching the quoted text. Seventeen are exact. Two are off by one:

| Plan says | Actual | Note |
|---|---|---|
| `ScreenReplayState` at `output.rs:57-74` | `output.rs:58-75` | line 57 is the blank line after `PtyScreenSnapshot`'s closing brace |
| construction at `output.rs:911-921` | `output.rs:911-922` | 921 is `conpty_size: (rows, cols),`, 922 is the closing `};` |

Neither changes any instruction. Section 1 already binds the implementer to re-anchor on quoted text rather than numbers, so this is a note, not a blocker.

Two structural facts the implementer needs and the plan does not state:

- `history` is NOT the last field of `ScreenReplayState`; `conpty_size` follows it (`output.rs:76`, added by #1439). "Immediately after `history`" therefore means between `history` and `conpty_size`, which is what section 5.6 draws. Worth saying out loud because a careless read of "after `history`" plus a glance at the struct's end puts the field in the wrong place, and both compile.
- `catch_payload_unwind` is `F: FnOnce() -> T` with `AssertUnwindSafe` applied internally (`src-tauri/src/logging.rs:61-73`). There is no `UnwindSafe` bound to satisfy, so the closure may capture `&state` freely. Section 5.6's shape compiles on the borrow side: `state.history.as_slices()`, `state.history.len()` and `state.history_aligned` are three immutable borrows of disjoint or identical immutable places, alongside the existing immutable `state.parser.screen()`.

### E.2 Verdict on `history_aligned` versus my original unconditional scan

**I agree with the rejection, and my own measurements are the reason. Section 5.2's open judgment call should be closed as "not reopened", on this evidence.**

Section 5.2 says the unconditional scan will be reconsidered only if an enricher shows that a newline-free block at the ring front cannot exceed one line in real output. It can, and by a lot. From the incident log (`app.log`, `spawn_diagnostics` `head=` fields, 41 samples of real Claude Code v2.1.237 PTY bytes):

- One visual line averages **234 bytes**, because every space is `\x1b[1C` and every colour change is a 19-byte truecolour SGR.
- **64% of all bytes sit inside an escape sequence** (measured by walking CSI and OSC sequences over the largest 512-byte sample).
- The spinner rewrites a single line with `\r` and no `\n`. The observed incident had rings that were **entirely** newline-free across all 65 536 bytes, which is why `history_from_first_line` needs a `None` arm at all.

So the newline-free block at the front is not bounded by one line, it is bounded by the ring. An unconditional cold scan on a healthy attach would discard everything from the front to the next `\n`, and in a session that just left a thinking phase that is a multi-kilobyte block that is still in the ring and would otherwise have replayed. That is exactly the unbounded silent loss section 5.2 names. My original sketch was wrong on this point and the flag is the right correction.

One thing section 5.2 slightly overstates, for the record: on the healthy path the hot trim already drops a whole line per trim. Simulated on the plan's own 9.2 fixture (102-byte lines), each overflow drains 50 bytes for space and then 52 more to realign, total 102, to admit 102. That is exactly the space needed, not a bonus loss, so it does not weaken the argument. It only means "the code already drops a line" is not available as a counter-argument to anyone who tries it.

I am not re-implementing the unconditional scan and I am not asking for it.

### E.3 Correctness hole: the flag can be `true` over an unaligned front

This is the one substantive defect I found, and it survives the plan as written.

`append_history` only touches `aligned` inside `if over > 0`. When the ring is **empty** and the incoming chunk is at least `UI_HISTORY_LIMIT_BYTES`, `over` is exactly zero:

```
over = history.len() + min(data.len(), LIMIT) - LIMIT = 0 + LIMIT - LIMIT = 0
```

so no scan runs, the flag keeps its construction value `true`, and `history.extend(tail)` then seeds the ring with `data[data.len() - LIMIT..]`, whose first byte is an arbitrary offset inside that chunk. The cold path trusts the flag, skips the scan, and seeds a literal sequence tail. This is the exact defect the plan exists to remove, reachable through the one arithmetic path the flag does not cover.

Simulated against my byte-for-byte reimplementation, first chunk of `LIMIT + 1003` bytes into an empty ring:

```
len=65536  aligned=True  front=b'8;2;153;153;153mX\x1b[38;2;'
```

That front is the wg-18 ac-healer screenshot signature verbatim, with the flag asserting the front is safe.

Reachability: the plan and `append_history`'s own comment both call an oversized chunk unreachable in production, and I agree for the local backend (4 KiB reads). The container backend accepts frames up to 64 KiB, so a first frame of exactly 65 536 bytes hits it. More importantly, section 7.2's table states the invariant unconditionally ("Ring never trimmed ... Flag stays `true` from construction, seed is the full ring. Unchanged."), and section 5.6's doc comment states it as a fact ("Starts true: an untrimmed ring begins at the first byte the session ever emitted"). Both are false in this case, and a future reader will rely on them.

Minimal remedy, three lines, entirely inside `append_history`, no new parameter and no behaviour change to the ring's bytes. The flag describes the FRONT, so it only needs correcting when the incoming tail BECOMES the front, which is exactly when the ring is empty at that moment:

```rust
    // #1458: a chunk larger than the ring is truncated at an arbitrary byte, so when it
    // becomes the whole ring the front is not a line start and `over` was zero, meaning
    // nothing above recorded that. Unreachable on the local backend's 4 KiB reads; the
    // invariant is what matters, not the reachability.
    if history.is_empty() && tail.len() < data.len() {
        *aligned = false;
    }
    history.extend(tail);
```

The existing `history_ring_is_bounded_and_line_aligned` already feeds `LIMIT + 4_096` bytes but into a **non-empty** ring, where `over` equals `history.len()`, the ring is fully drained, the scan runs on an empty ring, finds nothing and sets `false` correctly. So today's suite passes either way and this hole has no coverage. If the architect accepts the remedy, the cheapest coverage is two extra assertions inside the existing test rather than a fourth test (section 2 forbids adding one): after the oversized feed it already holds the lock, so `assert!(!state.history_aligned);` costs one line. That does not pin the empty-ring variant, which is the one that is actually broken. My recommendation is therefore: take the three-line remedy and accept it uncovered, or let section 2's "no fourth test" be relaxed for exactly this case. I do not have authority over either and am not choosing.

If the architect prefers to leave the code alone, then sections 5.6 and 7.2 must be reworded to state the invariant conditionally, because as written they document a guarantee the code does not provide, which is precisely the failure mode that produced #1458 in the first place (`append_history`'s old doc comment claimed "so the ring stays line aligned").

### E.4 Test fixture realism, with three corrected numbers

I ran the plan's three fixtures through the simulator. Test 9.2 is correct as written. Tests 9.1 and 9.3 pass for the right reason but the prose explaining them is wrong, and the fixture misses the symptom it claims to reproduce.

**The frame in 9.1 and 9.3 is 31 bytes, not 30.** `\x1b[38;2;153;153;153m` is 19 bytes (ESC plus 18), `* Drizzling` is 11, `\r` is 1.

**Consequence: the front does not land inside an escape sequence.** The steady-state front offset within a frame is `n - (65536 mod n)`. For `n = 31`, `65536 mod 31 = 2`, so the offset is 29, which is the `g` of `Drizzling`. Simulated:

```
plan test 9.1 (31 byte)   frame=31 len=65536 aligned=False front=b'g\r\x1b[38;2;153;153;153m* Drizzling\r\x1b'
```

So section 9.1's "Today this fails ... the front at offset 14 of a frame ... the body reads `;153;153m* Drizzling\r...`" is wrong on the frame length, the offset and the resulting body. The test itself still goes red today and green after, because with no `\n` anywhere the decision is driven entirely by the `None` arm, not by where the cut landed. But a fixture whose stated purpose is "lands inside an escape sequence most of the time" and which demonstrably does not is the kind of thing that gets "corrected" by the next maintainer in the wrong direction.

**Recommended fixture, verified: a 33-byte frame puts the cut two bytes into the SGR and reproduces the incident signature byte for byte.**

```rust
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r"; // 19 + 13 + 1 = 33 bytes
```

`65536 mod 33 = 31`, so the steady-state offset is `33 - 31 = 2`. Simulated, for both 9.1 and 9.3:

```
proposed 9.1 (33 byte)  frame=33 len=65536 aligned=False front=b'38;2;153;153;153m* Drizzling..\r\x1b[3'
proposed 9.3 (33 byte)  len=65536 aligned=False newlines=1 last=b'\n' front=b'38;2;153;153;153m* Drizz'
```

`38;2;153;153;153m` is exactly what session `c8c1088c` printed at row 1 col 1 in the incident. The rest of both tests is unchanged, including `UI_HISTORY_LIMIT_BYTES / frame.len() + 64` (1985 + 64 = 2049 iterations, and 1987 are needed) and `assert_eq!(state.history.len(), UI_HISTORY_LIMIT_BYTES)`, which I confirmed still holds exactly at 65 536 for `n = 33`. In 9.3 the terminating chunk becomes `b"\x1b[38;2;153;153;153m* Drizzling..\n"`, also 33 bytes, and the assertions on `back() == Some(b'\n')` and a single `\n` in the ring both hold.

**Escape density of the fixture against reality:** the 33-byte frame is 19/33 = 58% escape bytes, against 64% measured on real CLI output. The 31-byte frame is 61%. Both are realistic; the 33-byte one is the one that also lands the cut where the incident landed it, so it costs nothing to prefer it.

**Test 9.2's fixture is correct and its comment is accurate.** Verified: 102 bytes per line, `65536 mod 102 = 52`, the space trim drains 50 and the realignment then drains 52 more, so the realignment does real work on every overflow rather than passing by arithmetic accident. Steady ring length 65 484, flag `true`, front `\x1b[38;2;153;153;153m>0000`. The reason the `assert_eq!` on the whole body matters is the one section 9.2 gives, and I confirm it: an unconditional scan would drop 102 bytes here and every line begins with the same 20 bytes, so no prefix assertion could see it.

**One realism gap I am flagging without a fix:** 9.2's line is 19/102 = 19% escape bytes, well under the measured 64%. That is fine for what 9.2 pins (byte-exact preservation on the healthy path) and I am not asking for a change; I note it so nobody later cites 9.2 as evidence about real-world byte patterns.

### E.5 The `\n` resync point is an assumption, not a proof

Section 5.3 argues `\n` is "the one such offset recoverable from the ring alone". That is right in practice and I have no better candidate, but the claim is stronger than what holds: a `\n` inside a string-terminated sequence (OSC, DCS, APC, PM, SOS) is not a ground-state boundary, and draining through it puts the front inside the string. For this CLI it does not happen; the only OSC in the stream is the title, `\x1b]0;...\x07`, with no newline, visible verbatim in the `head=` samples.

No action required, because the hot path has always made this assumption and this plan does not widen it. I record it so that section 5.3 is not later read as a proof that the seed is guaranteed ground-state safe. It is guaranteed only for streams whose newlines are all outside string-terminated sequences, which is every stream this app has ever carried.

### E.6 Defects in the acceptance criteria

Three of the ten criteria cannot pass as written, or do not measure what they say. All three are commands, so all three are cheap to fix.

**Criterion 6 (`cargo fmt --check` clean for `output.rs` only) is unsatisfiable at the frozen SHA.** `rustfmt --check --edition 2021 src-tauri/src/pty/output.rs` already exits 1 with 99 lines of diff across 7 pre-existing regions, at lines **786, 2432, 2546, 2591, 2662, 2727, 2734**. There is no `rustfmt.toml` anywhere in the repo and the edition is 2021 (`src-tauri/Cargo.toml:4`), so that is the correct invocation. None of the 7 regions is inside any hunk this plan touches. Also worth pinning: **no CI job runs any fmt check at all** (`.github/workflows/` contains bundle-validation, cache-warm, lockfile-check, pr-regression-gates, release, validate-branch-name, version-sync-check; only `pr-regression-gates` runs Rust, and it runs `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests`). Recommended replacement: "`rustfmt --check --edition 2021 src-tauri/src/pty/output.rs` reports diffs at the same 7 line numbers as the frozen SHA and no others." Do not run bare `cargo fmt`: it rewrites unrelated files carrying pre-existing drift.

**Criterion 8 (`rg ... $(git diff --name-only ...)` returns nothing) can never pass.** `rg PATTERN FILE...` searches file **contents**, and the diff will list this plan file, which contains 7 matches for `TerminalView|shared/ipc|shared/types` (sections 3, 4 and 7.4 all cite those paths deliberately). The criterion as written fails on the plan's own prose. It should filter names, not contents:

```
git diff --name-only <frozen SHA>..HEAD | rg "TerminalView|shared/ipc|shared/types"
```

**Criterion 7 (`cargo test --workspace`) does not match what CI runs**, which is `cargo test --lib --bins --tests`. Not wrong, just broader; a green here does not prove a green there and vice versa. Recommend running the CI form as well, and note that the workspace total is +2 when a `dist/` directory exists, so an unexplained count delta of 2 is a build artefact, not a regression.

Criterion 3 (exactly two files in the diff) is safe: I checked that no PR-time gate requires a CHANGELOG entry. `CHANGELOG.md` is referenced only by `release.yml`.

Criterion 9's two `rg` counts are sound, and I confirm the premise: the change adds no `use` and no `mod`, so the arc record is byte-identical by construction. I did not re-run the levelization instrument, because section 6.1's enumeration is complete and its precondition holds.

Criterion 2's red-before-green step leaves `history_from_first_line` uncalled, producing a `dead_code` warning. That is fine for `cargo test`, and section 10 step 2 already anticipates it, but note it will fail `cargo clippy -- -D warnings` in that intermediate state. Do not run criterion 5 until step 4 is complete.

### E.7 Implementation risks worth stating

- **The mirror fallback is one screen, with no scrollback.** `vt100::Parser::new(rows, cols, 0)` (`output.rs:912`) is constructed with scrollback 0, so `contents_formatted()` returns the visible grid and nothing more. For the case that triggers the fallback this is the right outcome and section 5.4 argues it correctly, but the plan never says the number is zero. An implementer who assumes the mirror carries some history will misjudge what criterion 10 should look like on screen: after a `kept=0` attach the user sees exactly one screen, correctly rendered, and no scrollback. That is the intended, accepted result.
- **The WARN fires once per attach for as long as the ring stays unaligned**, not once per incident. In the observed run the user made about 130 selection commits in three hours, so the ceiling is user-driven and harmless. It is also the point: `activate_terminal_output` logs nothing on the happy path today, which is why the first occurrence of #1458 could not be located anywhere in 35 MB of log.
- **`stage=attach_history_unaligned` is a new stage on an existing prefix.** `[terminal-snapshot]` currently has three stages in this file (`attach_grid_mismatch` at `output.rs:1191`, `parser_fault` at 1099/1246/1368/1581, `resize_skipped` at 1344/1348/1355). Anyone grepping `terminal-snapshot` to check whether the #1439 guards fired will now also catch #1458 lines. Worth knowing, because "zero `terminal-snapshot` lines in the log" was the single fact that proved #1439's guards were inert. After this ships that grep no longer means what it meant.
- **Shadowing in the WARN closure is legal and intended.** `aligned.map_or(0, |(front, back)| front.len() + back.len())` shadows the outer `front`/`back`; `Option<(&[u8], &[u8])>` is `Copy`, so reading it for the WARN and then matching on it needs no clone, as section 5.6 says. Confirmed.
- **Mixed format-argument styles in the WARN** (`ring={}` positional alongside `kept={kept}` inline) are accepted by default clippy; `uninlined_format_args` is pedantic and not enabled here. Neighbouring lines in this file use inline captures throughout, so inlining `state.history.len()` into a local first would read more like its surroundings. Style only.
- **`screen` is in scope at the fallback.** The seed assembly sits inside the closure that opens with `let screen = state.parser.screen();`, so `None => screen.contents_formatted()` resolves, and it coexists with the `state.history` borrows because both are immutable. Confirmed by reading `output.rs:1198-1223`.
- **The frozen ring is safe.** When the parser flips to `Unavailable`, `append_history` stops being called (it lives inside the `Available` arm at `output.rs:1069-1082`) and `activate_terminal_output` returns no snapshot at all for an unavailable parser, so a frozen ring is never seeded and the flag cannot go stale against a moving front. No hole here; I checked because a stale flag is the obvious failure mode of this design.

### E.8 What I did not verify

- I did not compile anything. Every borrow-checker claim above is read from `logging.rs:61-73` and the existing code shape, not from `cargo check`. Step 6 or the implementer should be the first to compile.
- I did not run the existing test suite. Section 9.4's four tests are named and located correctly; whether they are green at the frozen SHA I take from CI, not from a local run.
- The simulator reproduces `append_history`'s arithmetic, not `vt100`'s rendering. Every "front=" figure above is exact; no claim about what `contents_formatted()` returns for these fixtures is.
- I still have not directly measured how many consecutive bytes the CLI emits without a `\n` while spinning. The incident's fully newline-free 64 KiB rings are the evidence that it exceeds the ring, and the fix does not depend on the number, but it remains inferred rather than instrumented. Criterion 10 will settle it: a `kept=0` line proves the ring was newline-free end to end.

## Grinch enrichment (Step 6)

Author: dev-rust-grinch, wg-17. Appended 2026-08-20 UTC. Nothing above this heading was modified.

Method note: I re-read the whole plan including the Step 5 section, then read every body it cites at the frozen SHA, plus both backends' entry points, `logging.rs`, the frontend seed site, and the `vt100` 0.15.2 source in the cargo registry. Where I claim arithmetic I ran a byte-for-byte simulator of `append_history`; where I claim a command fails I ran the command. Section G.8 lists what I tried to break and could not, because a Grinch review that reports only findings hides how much of the plan actually survived.

Frozen authority re-checked independently: `HEAD` is `1376c2b84a23125624e919c9af7e65813d624241`, `git ls-tree HEAD src-tauri/src/pty/output.rs` is blob `4f47604810cc17b399b51663fa7a17bc1c3da830`, `git status --porcelain` is empty. Matches section 1 and E.1.

### G.1 BLOCKING: the new function's `Some` arm has zero coverage, and a stub that never returns `Some` passes every listed test

**What.** `history_from_first_line` is the entire content-preserving half of this plan (section 5.1 row 3, required behavior 7.1 item 2). No test in section 9, and no existing test in the repository, ever drives it to return `Some`. Every listed test reaches it only on its `None` arm, or does not reach it at all.

Enumerated over all eleven call sites of `activate_terminal_output` in the repository (`output.rs:2184`, `2432`, `2446`, `3177`, plus `container_backend.rs:5542`, plus the four `activation_data` callers at `3241`, `3253`, `3268`, `3286`), the function is reached only when `include_history && !history.is_empty() && !history_aligned`:

| Test | Reaches it? | Arm |
|---|---|---|
| 9.1 newline-free ring | yes | `None` |
| 9.3 only `\n` is the last byte | yes | `None` |
| 9.2 line-aligned ring | no, flag is `true` | - |
| `activation_payload_replays_history_for_background_session` | no: 200 lines of ~18 bytes never reach `UI_HISTORY_LIMIT_BYTES`, so `over` is always 0 and the flag stays `true` | - |
| `activation_payload_falls_back_to_screen_when_history_empty` | no, empty ring | - |
| `activation_payload_ignores_history_when_not_requested` | no, `include_history == false` | - |
| `history_ring_is_bounded_and_line_aligned` | no, it never attaches | - |
| `attach()` helper users (`output.rs:2184`), `output.rs:2432`/`2446`, `container_backend.rs:5542` | no: tiny feeds, a faulted parser, an absent session, and an attach before any output respectively | - |

**Why it matters, concretely.** This implementation ships and passes all ten acceptance criteria:

```rust
fn history_from_first_line<'a>(front: &'a [u8], back: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
    let _ = (front, back);
    None
}
```

It passes 9.1 and 9.3 (both expect the mirror), 9.2 and all four of 9.4 (they never call it), and it satisfies criterion 2's red-before-green proof exactly as written, because reverting the `activate_terminal_output` hunk is what makes 9.1 and 9.3 red regardless of what the helper returns. The user-visible result is that **every attach to a session whose ring is unaligned but still holds replayable lines is silently downgraded from a full 64 KiB replay to a single 30-row viewport with no scrollback.** That is precisely the unbounded silent content loss section 5.2 spends a paragraph engineering the flag to prevent on the healthy path, reintroduced on the unhealthy path with no detector anywhere in the suite.

Weaker wrong implementations pass too, and each is a plausible thing to write:

- `&front[newline..]` instead of `&front[newline + 1..]`: seeds a leading `\n`. Uncovered.
- Dropping the second scan (`return None` when `front` holds no `\n`): the whole `back`-only branch of 7.2 row 9 becomes dead. Passes 9.1 and 9.3 because both expect the mirror anyway. Uncovered.
- `if aligned_front.is_empty() { return None }` without `&& back.is_empty()`: 7.2 row 8 inverts, and a ring whose first `\n` is the last byte of `front` takes the mirror instead of seeding `back`. Uncovered.

**Fix required.** One test, and it must assert the whole body rather than a prefix, for the same reason 9.2 gives. Fixture verified in the simulator, exact numbers below:

```rust
/// #1458. The recovering case, and the only one that exercises `history_from_first_line`'s
/// `Some` arm: an unaligned ring that still holds lines must seed from the byte after its
/// first `\n`, not fall back to the mirror. Asserting the whole body is the point; a stub
/// helper that always returns `None` passes every other test in this file.
#[test]
fn an_unaligned_ring_with_a_later_newline_seeds_from_that_line() {
    let fanout = fanout();
    let id = session(&fanout);
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r"; // 33 bytes, no `\n`
    for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
        feed(&fanout, id, &[frame]);
    }
    for index in 0..40 {
        feed(
            &fanout,
            id,
            &[format!("\x1b[38;2;153;153;153mrecovered line {index:03}\n").as_bytes()],
        );
    }
    let expected = {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        // The 4 KiB hot scan still sees only spinner at the front, so the flag stays false
        // even though the ring now holds 40 newlines further in.
        assert!(!state.history_aligned);
        let (front, back) = state.history.as_slices();
        let ring = [front, back].concat();
        let newline = ring.iter().position(|byte| *byte == b'\n').expect("a newline");
        ring[newline + 1..].to_vec()
    };

    let data = activation_data(&fanout, id, true);

    assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(&data[UI_HISTORY_REPLAY_PROLOGUE.len()..], expected.as_slice());
}
```

Simulated against the byte-for-byte reimplementation of `append_history`:

```
after spinner:  len=65536 aligned=False newlines=0
after 40 lines: len=65536 aligned=False newlines=40 first_nl_at=64053 kept=1482
seed body starts: b'\x1b[38;2;153;153;153mrecovered '
```

so the fixture is real: the ring is exactly full, the flag is false, `kept` is 1482 (39 whole lines), and the WARN fires with `kept > 0`, which is the only `kept > 0` case the plan will have exercised anywhere.

This test alone closes the stub, the off-by-one and the missing-`back`-scan variants at once. It does NOT close 7.2 row 8 (first `\n` is `front`'s last byte, `back` non-empty), which is unreachable through the fanout at will because it depends on where `VecDeque`'s head happens to sit. If the architect wants that row pinned too, the cheapest instrument is three direct calls to `history_from_first_line` inside one `#[test]`, with no fanout, no session and no helper:

```rust
assert_eq!(history_from_first_line(b"ab\ncd", b""), Some((&b"cd"[..], &b""[..])));
assert_eq!(history_from_first_line(b"ab\n", b"cd"), Some((&b""[..], &b"cd"[..])));
assert_eq!(history_from_first_line(b"ab", b"cd\n"), None);
```

**Conflict the architect must resolve.** Section 2 states, as a binding non-goal, "Do not add a fourth test". That non-goal is the direct cause of this hole. The plan cannot simultaneously forbid the test and claim required behavior 7.1 item 2 is verified. My position: 7.1 item 2 is a required behavior, so it needs a test, and the "exactly three tests" number was chosen before anyone noticed that all three land on the same arm. I do not have authority over section 2 and am not editing it.

### G.2 BLOCKING: acceptance criterion 1 cannot be run as written

**What.** Criterion 1 is `cargo test -p agentscommander_lib pty::output`. `agentscommander_lib` is the **lib target** name (`src-tauri/Cargo.toml:82`), not the package name. The package is `agentscommander-new` (`src-tauri/Cargo.toml:2`).

**Why.** Ran it at the frozen SHA:

```
$ cargo test -p agentscommander_lib pty::output --no-run
error: package ID specification `agentscommander_lib` did not match any packages
```

Criterion 1 is the criterion every other one leans on, and it fails before compiling anything. An implementer who hits this will either invent a substitute command (losing the criterion's binary character) or, worse, read the error as a broken workspace.

**Fix.** `cargo test -p agentscommander-new --lib pty::output::tests`. Ran that form at the frozen SHA: **39 passed, 3500 filtered out, exit 0.** That also closes E.8's second gap: the four tests of section 9.4 and every other `pty::output` test are green at the frozen SHA, measured rather than taken from CI.

### G.3 BLOCKING: E.3's hole is real, but its reachability claim is wrong in both directions, and the plan text that follows from it would produce either a dead test or a serious regression

E.3 is right that `append_history` can leave `aligned == true` over a front that is an arbitrary byte offset. I reproduced it. Everything downstream of that in E.3 needs correcting before it goes into the plan.

**Correction 1: the hole needs `data.len() >= UI_HISTORY_LIMIT_BYTES + 1`, not `>= UI_HISTORY_LIMIT_BYTES`.** With an empty ring, `over` is zero for any chunk, so the front is `tail[0]`, and `tail` is only truncated when `data.len() > LIMIT`. At exactly 65 536 the tail IS the whole chunk, so the ring's front is the chunk's byte 0, which for a first chunk is the session's first emitted byte: `aligned == true` is CORRECT there. Simulated:

```
chunk= 65535  aligned=True  ring front = chunk byte 0     <- correct
chunk= 65536  aligned=True  ring front = chunk byte 0     <- correct
chunk= 65537  aligned=True  ring front = chunk byte 1     <- THE HOLE
chunk= 66539  aligned=True  ring front = chunk byte 1003  <- THE HOLE (E.3's own figure)
```

**Correction 2: `data.len() >= 65_537` is unreachable through both production backends.** Verified by reading both entry points:

- Local: `let mut buf = [0u8; 4096];` then `fanout.handle_output(..., buf[..n].to_vec())` (`local_backend.rs:1653-1662`). Hard ceiling 4 096.
- Container: `handle_bridge_output` rejects with `AppError::PtyError` when `data.len() > MAX_TRANSPORT_FRAME_BYTES` (`container_backend.rs:1682-1686`), and `MAX_TRANSPORT_FRAME_BYTES = crate::pty::backend::PTY_INPUT_MAX_BYTES = 65_536` (`container_backend.rs:35`, `backend.rs:21`). The websocket layer caps the same value twice more (`session_transport.rs:54-55`). So the container ceiling is 65 536 inclusive, which is exactly the value that does NOT trigger the hole.

So E.3's sentence "The container backend accepts frames up to 64 KiB, so a first frame of exactly 65 536 bytes hits it" is wrong twice over: 65 536 does not hit it, and 65 537 cannot arrive. `handle_output` has no other caller: the five files that reference it are the two backends, `output.rs`, `watchers/mod.rs` and `tests/`, and every non-test caller is one of the two guarded ones above.

**Why this matters, with two concrete break scenarios.**

*Break A, the dead test.* E.3 closes with "the cheapest coverage is two extra assertions inside the existing test". If the architect writes that assertion from E.3's stated reachability, the implementer feeds a 65 536-byte first frame into an empty ring and asserts `!state.history_aligned`. It fails, because that case is genuinely aligned. The implementer then has a red test, correct code, and a plan that says the code is wrong.

*Break B, the regression, and this is the dangerous one.* The obvious way to "fix" Break A is to relax the guard from `tail.len() < data.len()` to `tail.len() <= data.len()`, which is always true. Then **every** first chunk into an empty ring sets `aligned = false`, so every young session is flagged unaligned. A session 3 KiB into its life, mid-spinner with no `\n` yet, now takes the cold path, `history_from_first_line` returns `None`, and the attach seeds the parser mirror instead of the session's complete ring. The plan would have converted the "it worked for the first minutes" window (section 3) from the only healthy window into a second instance of the very defect it is fixing. Simulated confirmation of the correct guard:

```
chunk= 65536 -> aligned=True   (correct: front is chunk byte 0)
chunk= 65537 -> aligned=False  (correct: front is chunk byte 1)
chunk= 66539 -> aligned=False  (correct: front is chunk byte 1003)
```

**What I am asking for.** E.3's three-line remedy is arithmetically correct as written and I support taking it. What must change is the justification and the coverage sentence around it:

1. Keep `if history.is_empty() && tail.len() < data.len() { *aligned = false; }` exactly as E.3 wrote it. Do not weaken the `<`.
2. Restate the reason as an INVARIANT, not a reachability claim: no production caller can reach it today (local reads 4 KiB, container rejects above 65 536), and the guard exists so the flag's meaning survives a future backend, a raised frame ceiling, or a coalescing reader. E.3's own closing sentence, "the invariant is what matters, not the reachability", is the right framing; the container sentence above it contradicts it and must go.
3. Do NOT add the assertion E.3 proposes to `history_ring_is_bounded_and_line_aligned`. Its oversized feed goes into a NON-empty ring, where `over == history.len()`, the ring is fully drained, the capped scan runs on an empty ring and correctly records `false`. Asserting `!aligned` there passes with or without the remedy, so it is coverage of nothing. If the remedy is to be covered at all it needs an empty-ring feed of at least `UI_HISTORY_LIMIT_BYTES + 1`, which is a fifth test and which section 2 forbids. Given the case is unreachable in production, I recommend taking the guard uncovered and saying so in the plan, rather than buying a test that measures the wrong arm.
4. Fix the two places that state the invariant unconditionally, exactly as E.3 asks: section 5.6's doc comment ("Starts true: an untrimmed ring begins at the first byte the session ever emitted") and section 7.2's "Ring never trimmed" row. Both are false for a first chunk above the limit, and documenting a guarantee the code does not provide is the failure mode that produced #1458.

### G.4 NON-BLOCKING, strongly recommended: the WARN goes inside the parser mutex, against this same function's own documented rule

**What.** Section 5.6 places `log::warn!` inside the `catch_payload_unwind` closure, which runs while `screen_parsers` is held.

**Why it matters.** The `log` implementation writes synchronously on the calling thread: `struct AppLogFile { file: Mutex<std::fs::File>, ... }` (`logging.rs:259`), and the same call path can trigger `rotate()` (`logging.rs:275`), which performs up to `KEEP` `std::fs::rename` calls on a multi-megabyte log file under that same file mutex. The incident's `app.log` was 35 MB, so rotation is a live event, not a theoretical one. Every PTY reader thread in the process blocks on `screen_parsers` for the duration.

The file already knows this. `activate_terminal_output` deliberately defers its other diagnostic: `reconcile_fault` is set inside the lock and `log::error!("[terminal-snapshot] stage=parser_fault session={id}")` runs after `drop(parsers)` (`output.rs:1243-1251`), with the in-code comment "#1439 R2: flush at the transition, OUTSIDE the parser lock. An emit under that lock stalls the PTY reader on its next chunk". The new WARN is the only per-attach log this function would take under the lock. `attach_grid_mismatch` (`output.rs:1191`) is in the lock, but it fires on a divergence, not on every attach.

Frequency is not incidental here. On the container backend a single frame can be the whole 64 KiB ring, so any newline-free frame larger than the 4 KiB scan window flips the flag, and the cold path plus this WARN become the common case rather than the exception. E.7 already notes about 130 attaches in three hours on the local backend.

I checked for a deadlock and there is none: `LevelGateLogger`, `PrivacyFilterLogger` and `AppLogFile` never touch `screen_parsers`, and the `error_log_event` emit runs in a separate task and only for ERROR (`logging.rs:686-700`). This is a latency finding, not a correctness one.

Note also that `activate_terminal_output` is a **synchronous** `#[tauri::command]` (`commands/pty.rs:532-533`), so under Tauri 2's dispatch rule for non-async commands this runs on the main thread. I did not verify Tauri 2's current dispatch empirically, so treat that half as unconfirmed; the reader-blocking half needs no such assumption.

**Fix, four lines, using the shape already in this function.** Compute inside, log outside, next to the existing `reconcile_fault` log:

```rust
// inside the closure, replacing the `if !state.history_aligned { ... warn ... }` block:
let unaligned = (!state.history_aligned).then(|| {
    (state.history.len(), aligned.map_or(0, |(front, back)| front.len() + back.len()))
});
// ... carried out of the closure alongside the snapshot, then after `drop(parsers)`:
if let Some((ring, kept)) = unaligned {
    log::warn!("[terminal-snapshot] stage=attach_history_unaligned session={id} ring={ring} kept={kept} (#1458)");
}
```

Section 5.5's reasoning ("it is emitted inside the existing `catch_payload_unwind` closure ... which is where the values it reports are in scope") is true but is an argument about convenience, not about the lock. `reconcile_fault` proves the values can be carried out. If the architect keeps the WARN inside the lock, the plan should say so deliberately and say why the #1439 R2 rule does not apply, rather than leaving the divergence unremarked.

### G.5 NON-BLOCKING: section 5.4 overstates the mirror. It is grid-consistent, not MODE-consistent, and this plan makes it reachable for an alternate-screen session for the first time in production

**What.** Section 5.4 says the mirror "is grid-consistent by construction" and 7.2 calls it "a consistent full repaint". Read against `vt100` 0.15.2:

- `Screen::contents_formatted` is `HideCursor` + `Grid::write_contents_formatted` + an attribute diff (`screen.rs:266-276`).
- `Grid::write_contents_formatted` emits `ClearAttrs`, then `ClearScreen`, then the **visible rows only** (`grid.rs:202-227`). With `vt100::Parser::new(rows, cols, 0)` (`output.rs:912`) there is no scrollback, which E.7 already flags.
- `Screen::grid()` returns `alternate_grid` when `MODE_ALTERNATE_SCREEN` is set (`screen.rs:742-748`), so the CONTENT is correct for an alt-screen session.
- Nothing in that path emits `\x1b[?1049h`. The frontend calls `entry.terminal.reset()` immediately before writing the seed (`TerminalView.tsx:562`), which puts xterm.js in the NORMAL buffer.

**Why it matters.** After a mirror seed of a session that is in the alternate screen, the attached xterm renders the alternate buffer's content while sitting in the normal buffer. Subsequent live repaints land in the normal buffer and accumulate in the user's scrollback, and the TUI's eventual `\x1b[?1049l` on exit does not restore what the user expects. This is cosmetic, but it is new reach: today the mirror is production-reachable only for an EMPTY ring, and a session with an empty ring cannot yet be in the alternate screen. This plan makes the mirror the outcome for a session that has emitted 64 KiB, which is exactly when it is likely to be in a TUI. This repository's own test constant models that: `TUI_PROLOGUE` (`output.rs:1961-1962`) is described as "A coding agent's TUI on the way up" and contains `\x1b[?1049h`.

I checked whether this is already broken on the ring path and it partly is: `UI_HISTORY_REPLAY_PROLOGUE` starts with `\x1b[?1049l`, so any replay forces the normal buffer, and a long-running TUI's original `?1049h` has usually scrolled out of the 64 KiB ring. So the desync class is pre-existing. What is new is that it now also happens on a path with no ring bytes to accidentally re-enter alt screen.

**What I am asking for.** Nothing in the code. One sentence in section 5.4 recording it as a known and accepted consequence, and a follow-up issue if the architect thinks it deserves one. I am explicitly NOT asking for `\x1b[?1049h` to be emitted before the mirror: that is a behavior change to a path two existing tests pin byte for byte, and it belongs to whoever owns the alt-screen story, not to #1458.

### G.6 NON-BLOCKING: two decided rows of 7.2 have no test, beyond the one in G.1

Recorded so the architect can decide, not as separate asks:

- **7.2 row 8** (first `\n` is `front`'s last byte, `back` non-empty, expected `Some((&[], back))`). Not reachable at will through the fanout: it depends on where `VecDeque`'s head sits. Closed only by the three direct calls sketched in G.1.
- **7.2 row 9** (`front` has no `\n`, `back` does, expected a `back`-only seed) on its `Some` outcome. The `None` outcome of that branch IS exercised by 9.3, which is worth recording: I computed the layout, and with 2 049 spinner frames of 33 bytes plus the terminating frame the head sits at byte 2 114, so `front` is `buf[2114..]` with no `\n` and the single `\n` is the last byte of `back`. 9.3 therefore runs the `back` scan and lands on its empty-remainder `None`.

I also tried to break 9.3 through the layout and could not: in the other possible layout (head at 0, `back` empty) the `\n` is `front`'s last byte and the both-halves emptiness check returns `None` as well. 9.3's outcome is layout-independent. That check is worth keeping in the record, because "this assertion depends on a `VecDeque` internal" is exactly the kind of thing that gets discovered by a flaky CI run two months later.

### G.7 NON-BLOCKING: E.6's replacement for criterion 6 cannot pass either

E.6 is right that criterion 6 is unsatisfiable, and I reproduced its measurement exactly:

```
$ rustfmt --check --edition 2021 src-tauri/src/pty/output.rs
exit=1, 99 lines, 7 regions at 786, 2432, 2546, 2591, 2662, 2727, 2734
```

But E.6's proposed replacement, "reports diffs at the same 7 line numbers as the frozen SHA and no others", fails 100% of the time after the change. The plan inserts a struct field near line 76, roughly ten lines in `append_history` near line 250, and about fifteen lines of `history_from_first_line` after it, all of which sit ABOVE line 786. Every one of the seven reported line numbers shifts down by the insertion count. The replacement criterion is as unrunnable as the criterion it replaces.

**Fix.** Make it count-based, which is stable under insertion and still binary:

> `rustfmt --check --edition 2021 src-tauri/src/pty/output.rs` reports exactly 7 `Diff in` regions and 99 total lines, the same as the frozen SHA, and none of the reported line numbers falls inside a hunk of `git diff <frozen SHA>..HEAD -- src-tauri/src/pty/output.rs`.

E.6's corrections to criteria 7 and 8 are both right and I have nothing to add to them. Criterion 10 has one soft spot worth a clause: the app's log level is runtime-settable through an `AtomicU8` gate (`logging.rs:169-176`), so the criterion should say the level must admit WARN for `agentscommander*` targets when the manual run is performed.

### G.8 What I tried to break and could not

Each of these was a real attempt with a specific failure in mind, not a checklist tick.

- **Flag drift.** `ScreenReplayState` is constructed in exactly one place (`output.rs:911`), and `register_session` refuses a second registration for a live id (`contains_key` then `SessionAlreadyRegisteredOrClosing`, `output.rs:927-929`), so no ring can outlive its flag or be adopted by a fresh one. Section 3's "exactly two write sites" holds.
- **The other way the flag can lie.** I hunted for a second path where the flag ends `true` over an arbitrary front, specifically `history.drain(..=newline)` emptying the ring and `extend(tail)` then installing an arbitrary first byte. It cannot: the ring only empties there when the `\n` was its last byte, so `tail[0]` is the stream byte immediately after that `\n`, and `true` is correct. For `data.len() > LIMIT` the space drain has already emptied the ring, the capped scan runs on an empty ring, and the `None` arm records `false`. Simulated both. E.3's empty-ring case is the only hole.
- **Sticky false.** Once the ring is at the limit, every chunk has `over > 0`, so the scan reruns and the flag recovers to `true` on the first `\n` that lands in the first 4 KiB. There is no state in which the flag stays `false` while the front is repeatedly realigned.
- **`as_slices` returning an empty `front`.** Impossible for a non-empty `VecDeque`: `head < capacity` is a standing invariant, so the first slice is non-empty whenever `len > 0`, and the `!state.history.is_empty()` guard runs first. The second branch of `history_from_first_line` is therefore never reached with a degenerate `front`.
- **Slice arithmetic.** `newline < len` always, so `&front[newline + 1..]` is at worst an empty slice at `len`. No panic path, no overflow, no allocation, and the `Vec::with_capacity` on the `Some` arm is computed from the SLICED lengths, so it is exact and never smaller than what is pushed.
- **Reallocation of the ring.** `over <= history.len()` always, because `tail.len() <= UI_HISTORY_LIMIT_BYTES`, so post-drain length plus `tail` never exceeds the reserved capacity. The change cannot make the ring realloc, and `history_ring_is_bounded_and_line_aligned`'s capacity assertion continues to pin it.
- **Concurrency and deadlock.** The cold scan takes no lock the caller does not already hold, allocates nothing before the `Vec::with_capacity` that already existed, and calls nothing re-entrant. Two windows attaching the same session serialize on `screen_parsers`, and the attach mutates neither the ring nor the flag, so it is idempotent and both windows get identical seeds. There is no `.await` anywhere on this path and no guard held across one.
- **Attach latency.** The worst case is one `slice::iter().position()` over 65 536 bytes, on a path that already runs `vt100::Parser::process` for every chunk under the same mutex. Not measurable against the existing cost, and the only thing on this path that can actually stall is the WARN (G.4).
- **Torn state mid-attach.** A panic inside `append_history` flips the parser to `Unavailable` via the `Ok(Err(())) | Err(_)` arm (`output.rs:1091-1094`), after which `activate_terminal_output` never reaches the seed assembly at all (`state.parser_availability == Available` gate, `output.rs:1180`). A torn ring is therefore never seeded, and a stale flag cannot be read against a moving front. E.7's conclusion holds and I reached it independently.
- **A second unfixed seed path.** Grepped every consumer of the ring and of `contents_formatted`. The ring has exactly one reader (`output.rs:1210-1221`); `get_screen_snapshot` (`output.rs:1496`) builds from `contents_formatted()` and never touches the ring. There is no second attach path that would keep seeding a mid-sequence front after this fix.
- **Empty chunks from the container bridge.** `handle_bridge_output` admits a zero-length frame. `append_history` then computes `over == 0`, changes nothing and appends nothing, while `output_sequence` still advances. Harmless, and the flag is untouched, which is correct.
- **E.5's string-terminated-sequence concern.** I tried to construct a real stream for this app that puts a `\n` inside an OSC, DCS, APC, PM or SOS payload and could not: OSC 0 (title) carries no newline and appears verbatim in the incident `head=` samples, OSC 52 is base64, and nothing here emits sixel or Kitty graphics. I also checked whether the cold path WIDENS the assumption relative to the hot path, since it scans 64 KiB instead of 4 KiB. It does not: the hot path already realigns to whatever `\n` it finds, and a `\n` deeper in the ring is no more likely to sit inside a string sequence than one near the front. E.5's "no action required" is right, and its "do not later read 5.3 as a proof" caveat is worth keeping.
- **Byte order across the two halves.** `as_slices().0` is the older half. Both arms of the plan's assembly push `front` then `back`, and the `back`-only arm returns `(&[], aligned_back)`, so replay order is preserved in every case.
- **The plan's own dependency-cycle gate (6.1).** Confirmed by reading: no `use`, no `mod`, one private free function and one private field, all intra-module. I did not re-run the levelization instrument, for the same reason E.6 gives.

### G.9 NON-BLOCKING: `history_aligned == false` is a conservative under-approximation, and nobody should read it as a proof

Recorded so it is not later mistaken for something stronger. When the byte-space drain lands by accident exactly after a `\n` and the next `\n` is beyond the 4 KiB scan window, the front IS a line start but the flag says `false`. The cold path then skips to the next `\n` and discards a legitimately replayable block, or takes the mirror if there is none. With E.2's measured 234-byte average line that is roughly a 1-in-234 chance per attach, the loss is bounded by the distance to the next newline, and the outcome is always safe rather than garbled. I considered asking for `append_history` to inspect the last drained byte and set the flag `true` in that case, and I am NOT asking for it: it changes `append_history` beyond "record the outcome of the scan you already run", which section 2 forbids, and it buys a fraction of a percent.

### G.10 Verdict

Not approved as it stands. Three blocking items: **G.1** (the plan's central new function has no `Some`-arm coverage, and a stub passes every test and every criterion), **G.2** (criterion 1 does not run), and **G.3** (E.3's reachability claim is wrong, and the two obvious ways to act on it produce either a permanently red test or a regression that re-creates #1458 for young sessions). G.1 and G.3 both need an architect decision, because both collide with binding non-goals in section 2.

Everything else survived. The flag design is right and E.2's evidence for it is the strongest argument in the file; the ring arithmetic, the `None`-arm reasoning, the tail argument in 7.4, the failure behavior in 7.3 and the dependency gate in 6.1 all hold under attack. I found no way to deadlock, no way to leak, no way to lose the ring's bytes other than through the untested `Some` arm, and no Windows-specific or backend-specific behavior that changes the answer, beyond the container backend's 64 KiB frames making the cold path the common case rather than the exception.

## Step 7 consensus resolution (round 1)

Author: architect, wg-17. 2026-08-20 UTC. Verdict: **READY_FOR_IMPLEMENTATION**.

Sections 1 to 10 above were amended in this round. The Step 5 and Step 6 sections are historical record and were not edited; where they cite "section 9.4" or "9.5" they mean the Step 4 draft's numbering, now 9.6 and 9.7 (section 9 carries the same note).

What I re-verified myself before ruling, rather than taking on report: the package name is `agentscommander-new` and the lib target is `agentscommander_lib` (`src-tauri/Cargo.toml:2`, `:82`, edition 2021 at `:4`); `catch_payload_unwind` is `F: FnOnce() -> T` with `AssertUnwindSafe` applied internally (`logging.rs:61-73`), so hoisting work out of the closure is a free choice and not a bound to satisfy; `vt100::Parser::new(rows, cols, 0)` at `output.rs:912` really does construct with zero scrollback; `let mut reconcile_fault = false;` is at `output.rs:1179`, which is where the new carrier goes; `rustfmt --check --edition 2021 src-tauri/src/pty/output.rs` at the frozen SHA exits 1 with exactly 99 lines and 7 `Diff in` regions at 786, 2432, 2546, 2591, 2662, 2727, 2734; and the corrected spinner frame is 33 bytes with `65536 mod 33 = 31`, giving a steady-state front offset of 2. The working tree is still clean and `HEAD` still `1376c2b8`.

### Blocking findings

**G.1, the `Some` arm has no coverage. ACCEPTED in full, and the binding non-goal it collides with is repealed.** The demonstration is decisive on its own terms: a helper that returns only `None` passes all three original tests, all four existing tests, and criterion 2's red-before-green proof, while silently downgrading every recoverable unaligned attach to a single viewport. That is the same unbounded content loss section 5.2 spends its argument preventing on the healthy path, reintroduced on the unhealthy one. The "exactly three tests" number was mine and it was an estimate made before the branch structure was fully enumerated; it was never a scope principle. Section 2 now binds "exactly five tests" and states the real rule, one test per decided branch.

Grinch's fixture is adopted verbatim as section 9.4, including its simulator-verified numbers (ring exactly full, flag false, first newline at 64 053, `kept = 1 482`).

I also took the optional direct-call instrument, as section 9.5, and extended it from three assertions to five. Grinch offered it only for 7.2 row 8; I am taking it because it is the sole deterministic detector for three separate things the fixture-based tests cannot reach at will (which half of the ring holds the first `\n` is a `VecDeque` layout detail), and because it closes the off-by-one and dropped-second-scan variants independently of any layout. The two assertions I added beyond grinch's three cover 7.2 row 9's `Some` outcome (G.6's second bullet, otherwise uncovered) and the no-newline-anywhere case. Cost: one `#[test]`, no fixture, no fanout, no session.

**G.2, criterion 1 does not run. ACCEPTED verbatim.** Criterion 1 is now `cargo test -p agentscommander-new --lib pty::output::tests`, and I have hardened it with the count grinch measured: 39 at the frozen SHA, so 44 after, which makes the criterion prove the five tests exist rather than merely that something passed. The package-versus-lib-target trap is named in the criterion so the next reader does not repeat it.

**G.3, E.3's hole is real but its reachability is wrong. ACCEPTED in full, all four asks.** I re-derived the arithmetic rather than taking it: with an empty ring `over` is zero for any chunk, `tail` is truncated only when `data.len() > LIMIT`, so at exactly 65 536 the front is the chunk's byte 0 and `true` is CORRECT; the hole needs 65 537. The `<` therefore stays, the container sentence is gone, the justification is restated as an invariant repair (new section 7.5, with both backends' ceilings), the E.3 assertion is NOT added to `history_ring_is_bounded_and_line_aligned`, and both places that stated the invariant unconditionally are fixed (the `history_aligned` doc comment in 5.6, and the 7.2 row, which is now split into three rows that each say which case they are).

One thing I am adding that neither enrichment reached, and it is what lets me ship the guard uncovered with a clear conscience. Grinch's Break B, weakening `<` to `<=`, is **already detected by an existing test**. I traced it: with `<=` the condition is unconditionally true, so `activation_payload_replays_history_for_background_session`'s first 16-byte chunk into an empty ring sets the flag `false`, its 200 short lines never overflow the ring so nothing ever resets it, the attach takes the cold path and seeds from after the first `\n`, and its `replayed.contains("history line 0\r\n")` assertion fails. So the dangerous mutation is pinned by a test that already exists and that section 2 forbids modifying. That is recorded in 9.6 as the reason the guard needs no test of its own, which is a stronger position than "unreachable, accept it uncovered".

### Non-blocking findings

**G.4, the WARN inside the parser mutex. ACCEPTED, and this is the amendment I am least ambivalent about.** Grinch is right that section 5.5's justification was about convenience and never about the lock, and that this function already refuses exactly this: `reconcile_fault` exists to carry a diagnostic past `drop(parsers)` and carries the #1439 R2 comment saying why. Synchronous file-mutex writes with a rotation that renames a multi-megabyte file, against a 35 MB incident log, blocking every PTY reader in the process, on a path the container backend can make the common case, is not a cost worth a convenient variable scope.

I did not take grinch's `.then(|| ...)` sketch literally. Instead the whole replay decision moves out of the unwind closure, which is sound because it is pure slice arithmetic with no panic path, and which leaves the closure holding only the things that can actually panic. That gives one assignment to a plain local declared next to `reconcile_fault`, in the file's own idiom, instead of a value threaded through the closure's return type. The enclosing block around `let copied = { ... }` is deliberate: it ends `replay`'s borrow of `state.history` before `match copied`'s `Err(_)` arm takes `&mut state`, so the shape does not depend on a reader trusting NLL's liveness analysis at a glance.

**G.5, the mirror is grid-consistent but not mode-consistent. ACCEPTED as written.** Section 5.4 now records it as a known and accepted consequence, with the new-reach point stated (until now the mirror was production-reachable only for an empty ring, and an empty-ring session cannot be in the alternate screen). I agree with grinch's own refusal to fix it here: emitting `\x1b[?1049h` would change bytes on a path two existing tests pin, and the alt-screen story is not #1458's. Section 2 now binds that as a non-goal so nobody helpfully adds it during implementation. **I am asking the coordinator to file the follow-up issue**; per the dispatch I am not filing it myself.

**G.6, two decided rows with no test.** Resolved by 9.5, as described above. Grinch's layout analysis of 9.3 is worth more than the finding it accompanies and is now recorded under 9.3: that test's outcome is layout-independent, which is exactly the kind of thing that otherwise surfaces as a flaky CI run months later.

**G.7, E.6's replacement for criterion 6 cannot pass either. ACCEPTED.** Grinch is right that pinning the seven line numbers fails 100% of the time, because every insertion this plan makes sits above line 786. Criterion 6 is now count-based (7 regions, 99 lines) plus the requirement that no reported region falls inside a hunk of the diff, which is stable under insertion and still binary. I reproduced the baseline measurement independently.

**G.9, the flag is a conservative under-approximation. ACCEPTED**, recorded as new section 5.7, together with grinch's own refusal of the sharpening, which is now a binding non-goal.

**E.1, two off-by-one anchors and the struct-field placement. ACCEPTED.** Sections 1, 5.6 and 6 carry the corrected anchors, and 5.6 now says in bold where the field goes and why the wrong placement also compiles.

**E.2, the measured case for the flag. ACCEPTED**, and section 5.2's open judgment call is closed. I wrote that call and I am the one closing it: the invitation to reopen is removed, the measurements are in section 3 and 5.2, and E.2's correction about the trim's line drop ("exactly the space needed, not a bonus loss") is recorded so it is not used as a counter-argument later.

**E.4, the fixture is 33 bytes, not 31 or 30. ACCEPTED**, and I verified the arithmetic myself. My draft got the frame length wrong twice over and the resulting offset wrong as well. Both spinner fixtures and all the prose around them are corrected, and section 9 carries the reason the correction matters even though the tests passed either way.

**E.5, `\n` is an assumption and not a proof. ACCEPTED**, recorded in 5.3 with grinch's confirmation that the cold path does not widen it.

**E.6, criteria 7 and 8. ACCEPTED.** Criterion 8's pipe is the fix (`rg PATTERN FILE...` searches contents and matched this plan's own prose, so the criterion could never pass). Criterion 7 now names both the CI form and the workspace form, and records the `dist/` +2. E.6's note about `dead_code` between implementation steps 2 and 4 is now attached to criterion 5 and to step 2 itself.

**E.7, implementation risks. ACCEPTED into the plan** rather than left in the enrichment: the mirror's zero scrollback is in 5.4 and in criterion 10's expected outcome, and the `terminal-snapshot` grep-meaning change is in section 6's log-surface note. The format-argument style point resolves itself, because the relocated WARN is a single inline-capture line.

### Nothing rejected outright

Every finding in both enrichments is accepted. Two are accepted with a different remedy than the one proposed: G.4 (whole decision hoisted, rather than a value threaded out of the closure) and G.3's coverage question (guard shipped uncovered because an existing test already catches its dangerous mutation, rather than because the case is merely unreachable). Two optional items offered without a recommendation are taken: G.1's direct-call instrument, extended by two assertions, and G.5's follow-up issue, which the coordinator will file.

### Verdict

**READY_FOR_IMPLEMENTATION.**

The dependency-cycle gate is re-affirmed at this round: none of the amendments adds a `use` or a `mod`, so the enumeration in section 6.1 still holds and criterion 9 mechanizes it. The three blocking findings are resolved in the decided sections, not deferred. The test count moves from three to five and the acceptance criteria from partly unrunnable to all ten runnable as written, four of them measured at the frozen SHA rather than assumed.

Certification is against the exact bytes of this file. Any byte change after this point invalidates it.
