# Plan #1439: Reconcile the screen parser with the ConPTY grid across detach/re-attach, and stop skipping its resize silently

Author: architect, wg-8. Draft authored 2026-08-19 UTC as Step 4 of the full `code-implementation-workflow` path.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1439](https://github.com/mblua/AgentsCommander/issues/1439), `Embedded terminal re-attach paints garbled screen-parser state after #1432`.

This is a minimal regression fix. It adds one field and two log-bearing branches inside `src-tauri/src/pty/output.rs`, one injected parameter and one INFO log inside `src-tauri/src/commands/pty.rs`, one guarded call plus a one-line dedup-key invalidation inside `src/terminal/components/TerminalView.tsx`, one test-only seam, and exactly two regression tests. Round 2 additionally re-pins four existing #973 test cases in `src/terminal/components/TerminalView.spawn-size.test.tsx` to the post-R1 resize contract (expectation and comment edits inside existing cases; zero test cases added or removed). It introduces no new crate, no new npm dependency, no new module, no new Tauri command, no new event, no new IPC payload shape, no new configuration key, and no migration. It adds zero module-to-module dependency arcs.

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `fix/1439-embedded-attach-parser-resync`, targeting `main`.

At authoring time (2026-08-19 UTC) the committed `HEAD` of the branch is `b19ee1858cd6bf929abb6ae59f01239da20de498`, equal to the base `main` given by the dispatch, and `git status --porcelain` over tracked files is empty. The Codebase Memory index used for every symbol citation below reports the same `head_sha`.

Root `.gitignore` ignores `/plans/`, so the implementation must force-add this exact plan file with `git add -f plans/1439-embedded-attach-parser-resync.md`. Do not remove or weaken the `plans/` ignore rule.

Step 7 (certification) must re-run the authority ritual: fetch `origin/main`, and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. Every line number in this plan refers to the frozen SHA. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

Round-2 anchor note (2026-08-19 UTC, post-blocker): steps 1-3 landed as `8d09e835`, `98ae55c5`, `71cd694f` on this branch, and the round-2 amendment is authored at `71cd694f` with a clean tracked tree. `git diff --name-only b19ee185 71cd694f` touches only `plans/` and `src-tauri/` files, so `src/terminal/components/TerminalView.tsx` and `src/terminal/components/TerminalView.spawn-size.test.tsx` are byte-identical at the frozen SHA and at `71cd694f`; line anchors for those two files are valid at either SHA (section 9.2a cites them at `71cd694f`). Everything else keeps the frozen-SHA rule above.

## 2. Objective and non-goals

Objective: after any detach/re-attach cycle, an embedded (or any) window that re-attaches to a session must never be seeded with a snapshot whose grid disagrees with the grid the ConPTY is actually using, and any path that skips or fails the parser's grid follow must say so in `app.log`. When no trustworthy seed exists, the attach must fall back to the already-supported no-snapshot path AND still re-impose the local viewport grid on the PTY so live output stops garbling without user intervention.

Non-goals, binding on the implementer:

- Do not re-engineer the #1432 attach protocol: `applySnapshot`'s reset-then-reseed contract, the sequence/watermark reconcile, `resizeTerminalForSnapshot`'s deliberate PTY-resize suppression, and the batch-cut-at-snapshot-boundary behavior all stay exactly as they are.
- Do not move `parser.set_size` into `handle_output` or any deferred/pending-resize scheme. Section 5.1 records why that design is rejected; the rejection is a fixed decision.
- Do not change `resize_instance`, `hand_over_held_size`, `record_resize`, the startup gate flow, or anything in `src-tauri/src/pty/local_backend.rs`.
- Do not change `PtyScreenSnapshotPayload`, `src/shared/types.ts`, or `src/shared/ipc.ts`. The IPC surface is untouched.
- Do not force a ConPTY repaint after a reconcile miss. The issue marks it optional; it would require a resize jiggle against `resize_instance`'s dedup, and the TUI's own repaint cadence (sub-second for the reported Claude Code sessions) converges the content. Known residual: after a reconcile miss, the parser's cell content stays incomplete until the child app next repaints; the seed for that interval is `None`, never garbage.
- Do not touch the `applySnapshot` discard branch (`live output outran the reconcile budget`). It is dev-rust's kept-alive secondary suspect (#1439 evidence, gap 4) and out of this issue's minimal scope.
- Do not add tests beyond the two named in section 9. The round-2 re-pin (9.2a) amends the EXPECTATIONS of four existing #973 cases and adds or removes none; the spawn-size file's case count stays 9.
- Do not silence, rename, or reuse existing log stages (`stage=parser_fault` stays exactly as-is on both existing paths).

## 3. Evidence and identified cause

Confirmed at the frozen SHA by direct read of every cited body (Codebase Memory, 20 graph operations, 1 bounded fallback):

- `SessionIoFanout::activate_terminal_output` (`src-tauri/src/pty/output.rs:1149-1213`): on attach, the snapshot grid is the PARSER's own grid: `let (rows, cols) = state.parser.screen().size()`. The seed bytes are the raw history-ring replay (`UI_HISTORY_REPLAY_PROLOGUE` + `state.history` slices) whenever `include_history && !history.is_empty()`, which is the production path (the command's sole caller passes `true`); `screen.contents_formatted()` is used only when history is empty. Snapshot construction is `PtyScreenSnapshot { data, rows, cols, sequence: state.output_sequence }`.
- Closing of #1439 gap 1: the "snapshot renders from the screen parser" premise holds for the GRID always and for the CELLS only on the empty-history path. The poison vector in the incident class is therefore the parser's stale `rows/cols`: the re-attaching xterm adopts them (`applySnapshot` -> `resizeTerminalForSnapshot` when they differ) and then replays raw bytes that were rendered for a different grid, which wraps/clamps/stacks exactly as the incident screenshots show. This refines the issue's premise without contradicting it: divergence of the parser grid from the ConPTY grid remains the defect.
- `SessionIoFanout::resize_screen_and_broadcast` (`output.rs:1296-1339`): the only parser grid follow. Confirmed three SILENT early-returns: poisoned `screen_parsers` lock, missing entry, `parser_availability != Available`. The degenerate `0x0` refuse WARNs (#973). `set_size` runs under `catch_payload_unwind`; a panic flips the parser `Unavailable`, logs `stage=parser_fault`, and flushes unsequenced.
- Closing of #1439 gap 2: `pty_resize` command (`src-tauri/src/commands/pty.rs:452-464`) -> `PtyManager::resize` (`src-tauri/src/pty/manager.rs:573-576`) -> `LocalProcessBackend::resize` (`src-tauri/src/pty/local_backend.rs:1986-2018`), which resizes the ConPTY FIRST (`resize_instance` under the `ptys` lock, returning `sent: bool`) and only `if sent` calls `resize_screen_and_broadcast`, from the command thread. The startup path is `open_startup_gate` (`local_backend.rs:1694-1711`): `hand_over_held_size` then the same follow call. The in-code #973 comment documents the invariant this plan builds on: the parser may only follow a size the ConPTY actually took, and after registration "the two only ever move together": an assumption that any single silent skip falsifies forever, because the dedup in `resize_instance` then blocks the healing resize.
- `SessionIoFanout::handle_output` (`output.rs:982-1122`): the reader-side ingest. `parser.process(&data)`, the sequence advance, and the history append all run under the SAME `screen_parsers` mutex that `resize_screen_and_broadcast` takes; the in-code comment states "Appending under the parser lock is what keeps the batch boundary atomic with the sequence assignment". Consequence: `set_size` can never land mid-chunk today; it lands between chunks. What is NOT guaranteed is which side of still-unread pipe backlog it lands on, and whether it lands at all (the three silent skips).
- `applySnapshot` (`src/terminal/components/TerminalView.tsx:570-613`): the no-snapshot branch (`!snapshot || snapshot.data.length === 0`) sets `SNAPSHOT_UNAVAILABLE_MESSAGE` when nothing rendered yet and RETURNS EARLY, skipping the `scheduleViewportSync(sessionId)` call that the seeded path runs for the visible session. Consequence: an attach that resolves without a seed leaves the PTY at the other window's grid until something else resizes it; live bytes garble the embedded xterm for that whole interval. This is the frontend half of the incident and of the required fix.
- Incident (app.log session `4e3ca876`, 2026-08-19): spawn 11:29:00 embedded 81x27; detach 12:11:54; re-attach 12:12:49; corrupted embedded frame 12:13:05; re-detach 12:13:24; clean detached frame 12:14:07. Stacked transient spinner frames from inside the detached interval require replay of wide-grid bytes into a narrower grid, i.e. a stale parser grid at attach or a missed viewport re-imposition, both of which this plan fixes; zero pty-resize logging exists today to adjudicate which skip fired, which the diagnostics below fix.

Identified cause, in one sentence: the parser's grid follow is a separate, silently skippable step that nothing reconciles afterwards, so a single miss makes every later re-attach adopt a stale grid and seed garbled content, while the no-seed attach path never re-imposes the local grid on the PTY.

## 4. In scope / out of scope

In scope:

1. Record, inside the fanout state, the grid the ConPTY last took, unconditionally on every follow call (record-first).
2. WARN on every skipped parser follow (the three silent early-returns).
3. Reconcile parser grid against the recorded ConPTY grid inside `activate_terminal_output`; on mismatch WARN, converge the parser grid, and return `None` (no seed) instead of a garbled seed.
4. INFO log for every window-requested PTY resize (session, cols, rows, requesting window label).
5. Frontend: schedule the viewport sync on the no-snapshot attach path for the visible session.
6. Exactly two regression tests (one Rust in `output.rs`, one case in `TerminalView.attachment.test.tsx`) plus the one test-only seam the Rust test needs.
7. Round 2: re-pin the four #973 invoke-count cases in `TerminalView.spawn-size.test.tsx` to the post-R1 contract (section 9.2a); expectation and comment edits inside existing cases only.

Out of scope (per issue and section 2 non-goals): forced ConPTY repaint, pending/deferred resize application, the discard branch, `local_backend.rs` internals, container backend specifics (`ContainerTransportBackend` inherits whatever it already gets from the shared fanout code paths), any IPC/type change, any refactor.

## 5. Decided solution

### 5.1 Fixed interpretation of issue item 1 ("atomic with the PTY resize relative to the output stream")

The issue's literal suggestion is "apply `parser.set_size` from the fanout/reader side at the batch boundary". Two facts from section 3 fix the design:

- `set_size` ALREADY applies only at chunk boundaries, because it serializes with `parser.process` on the `screen_parsers` mutex. The reader-side relocation would not add boundary atomicity.
- A deferred "apply on next chunk" scheme is actively harmful: an idle session's pending resize would never apply, so the parser grid would sit stale until the next output byte, and the new reconcile would then refuse to seed attaches of perfectly healthy idle sessions. A true in-stream marker is unattainable: the OS pipe between ConPTY and the reader cannot be injected into.

Decision (fixed, not open to the implementer or enrichers except by returning the plan): item 1 is realized as record-first plus WARN-on-skip plus attach-time reconcile. The application point of `set_size` does not move. The residual mis-parse window (backlog bytes rendered for the old grid parsed under the new size) keeps its correct rows/cols, is bounded by pipe backlog, and self-heals at the ConPTY's own post-resize repaint; the reconcile guarantees it can never poison a seed's grid.

### 5.2 Backend: record-first + WARN in `resize_screen_and_broadcast` (`src-tauri/src/pty/output.rs`)

Add one field to `ScreenReplayState` (same file; locate the struct by name):

```rust
    /// The last grid the ConPTY actually took (rows, cols): recorded by every
    /// follow call BEFORE the skippable steps, so a skipped or failed
    /// `set_size` leaves a visible divergence for the attach reconcile (#1439).
    /// On transport backends (container) the follow runs after merely queuing
    /// the resize frame, so there this records the size last REQUESTED of the
    /// remote, not necessarily taken; the local backend's `if sent` gate is
    /// what keeps the record honest where #1439 lives.
    conpty_size: (u16, u16),
```

Initialize it at every `ScreenReplayState` construction site from the same rows/cols the `vt100` parser is constructed with (the registration seed; the compiler's missing-field error enumerates the sites). No constructor gains a parameter unless the rows/cols are not already in scope there; they are, because the parser is built from them.

Rewrite `resize_screen_and_broadcast` (`output.rs:1296-1339`) keeping its signature, its `0x0` refuse, its panic path, its flush semantics, and its broadcast condition byte-identical in behavior, changing only the marked lines:

```rust
    pub fn resize_screen_and_broadcast(&self, id: Uuid, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            log::warn!("[pty] refusing to resize the screen of {id} to {cols}x{rows} (#973)");
            return;
        }

        let parser_fault = {
            let Ok(mut parsers) = self.screen_parsers.lock() else {
                log::warn!("[terminal-snapshot] stage=resize_skipped reason=parsers_lock_poisoned session={id} cols={cols} rows={rows} (#1439)");
                return;
            };
            let Some(state) = parsers.get_mut(&id) else {
                log::warn!("[terminal-snapshot] stage=resize_skipped reason=no_parser_entry session={id} cols={cols} rows={rows} (#1439)");
                return;
            };
            // #1439 record-first: the ConPTY took this size whether or not the
            // parser can follow it; the attach reconcile compares against this.
            state.conpty_size = (rows, cols);
            if state.parser_availability != ParserAvailability::Available {
                log::warn!("[terminal-snapshot] stage=resize_skipped reason=parser_unavailable session={id} cols={cols} rows={rows} (#1439)");
                return;
            }
            let resized =
                crate::logging::catch_payload_unwind(|| state.parser.set_size(rows, cols));
            if resized.is_err() {
                state.parser_availability = ParserAvailability::Unavailable;
                true
            } else {
                false
            }
        };
        if parser_fault {
            log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            // Flush at the transition: from here every batch carries no sequence, and one
            // batch cannot describe both with a single scalar.
            self.attachments.flush(id, false);
            return;
        }

        if let Some(ref bc) = self.ws_broadcaster {
            bc.broadcast_event(
                "pty_resized",
                &serde_json::json!({
                    "sessionId": id.to_string(),
                    "cols": cols,
                    "rows": rows,
                }),
            );
        }
    }
```

Behavioral deltas, exhaustively: the three early-returns now WARN with distinct `reason=` values, and `conpty_size` is recorded whenever the entry is reachable. Everything else, including which paths broadcast `pty_resized`, is unchanged.

### 5.3 Backend: reconcile in `activate_terminal_output` (`output.rs:1149-1213`)

Inside the `ParserAvailability::Available` arm, before the existing `catch_payload_unwind` render, insert the reconcile. Exact logic, anchored on the existing body:

```rust
        let mut reconcile_fault = false;
        let snapshot = if state.parser_availability == ParserAvailability::Available {
            let (parser_rows, parser_cols) = state.parser.screen().size();
            if (parser_rows, parser_cols) != state.conpty_size {
                // #1439: the parser grid diverged from the grid the ConPTY took
                // (a skipped follow, or any path that resized one without the
                // other). A seed rendered off this parser adopts the stale grid
                // in the attaching window and replays other-grid bytes into it:
                // the garbled re-attach. Converge the grid now, seed nothing;
                // the frontend attaches live (the no-snapshot path), and the
                // next attach after the child's next repaint seeds cleanly.
                let (conpty_rows, conpty_cols) = state.conpty_size;
                log::warn!(
                    "[terminal-snapshot] stage=attach_grid_mismatch session={id} parser={parser_cols}x{parser_rows} conpty={conpty_cols}x{conpty_rows} (#1439)"
                );
                let resized = crate::logging::catch_payload_unwind(|| {
                    state.parser.set_size(conpty_rows, conpty_cols)
                });
                if resized.is_err() {
                    state.parser_availability = ParserAvailability::Unavailable;
                    reconcile_fault = true;
                }
                None
            } else {
                // ... existing catch_payload_unwind render block, byte-identical ...
            }
        } else {
            None
        };

        self.attachments.attach(id, label); // existing line, unchanged
        drop(parsers);                      // existing line, unchanged
        if reconcile_fault {
            log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            // #1439 R2: flush at the transition, OUTSIDE the parser lock. An
            // emit under that lock stalls the PTY reader on its next chunk;
            // both existing fault sites flush only after releasing the lock.
            self.attachments.flush(id, false);
        }
        Ok(snapshot)
```

Fixed decisions: the mismatch arm always returns `None` for this attach, even when the healing `set_size` succeeds (the parser CELLS are not trustworthy for the divergence interval; only the grid converged). The attach itself still registers (`self.attachments.attach(id, label)` at the tail is unchanged), so live output flows to the window immediately, unsequenced or sequenced exactly as the existing contract dictates. The panic arm mirrors the existing fault transition (`Unavailable` + `stage=parser_fault` + `flush(id, false)`) so sequence semantics stay single-sourced. (Step 7, R2+G4) The fault pair is hoisted OUT of the `screen_parsers` lock scope through the `reconcile_fault` bool and runs after the existing `attach`/`drop(parsers)` tail, before `Ok(snapshot)`: `flush` emits to webviews, an emit under the parser lock stalls the PTY reader on its next chunk, and both existing fault sites (`resize_screen_and_broadcast`, `handle_output`) flush only after releasing the lock; the post-lock flush race (a chunk accumulating between the drop and the flush) is the same one those sites already accept. The two `None` exits stay distinct: heal-succeeded logs nothing and flushes nothing; heal-panicked runs exactly the hoisted pair.

### 5.4 Backend: INFO per window-requested PTY resize (`src-tauri/src/commands/pty.rs:452-464`)

Replace the `pty_resize` body with (signature gains the Tauri-injected webview, making the function generic exactly like `activate_terminal_output` in the same file; no frontend change, no `ipc.ts` change, the invoke payload is identical):

```rust
pub fn pty_resize<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    // #1439: the only record of who drove a grid change; the incident was
    // diagnosable only by inference because no pty resize was ever logged.
    log::info!(
        "[pty] resize session={uuid} cols={cols} rows={rows} from={}",
        webview.label()
    );
    pty_mgr
        .lock()
        .unwrap()
        .resize(uuid, cols, rows)
        .map_err(|e| e.to_string())
}
```

If the command registration macro requires a turbofish or generic listing change where `pty_resize` is registered, mirror exactly what `activate_terminal_output` already does there; add nothing else. Backend-internal resizes (spawn, startup gate handover) are intentionally NOT INFO-logged: they are not window requests, and their follow outcomes are covered by the section 5.2 WARNs.

Two recorded acceptances (Step 7, G7 and N4): the body retains the pre-existing `pty_mgr.lock().unwrap()` pattern of `commands/pty.rs`; a poisoned PtyManager mutex panics the command thread today and after this change alike, and un-poisoning it is out of scope and must not be attempted here. And the INFO line covers the desktop command path only: any non-command resize driver reaches `PtyManager::resize` without it, so during triage the absence of an INFO line is not evidence that no resize happened.

### 5.5 Frontend: dedup-key invalidation and viewport sync on the no-snapshot attach path (`src/terminal/components/TerminalView.tsx:570-613`)

Two edits in `applySnapshot`, changing nothing else in the file.

First (Step 7, ratifying R1 with G3's placement): invalidate the viewport dedup key as the FIRST statement of `applySnapshot`, before every arm:

```tsx
    // #1439: a viewport sent before a detach must never dedup the
    // re-imposition after the re-attach; the other window may have driven the
    // PTY elsewhere while this key sat stale. Cleared once per attach settle
    // (this function's only caller); within an attached interval the dedup
    // operates unchanged.
    entry.lastSentViewport = null;
```

Why this line is required: `sendPtyResize` (`TerminalView.tsx:319-347`) early-returns when `entry.lastSentViewport` equals the requested cols/rows, and the key is written in exactly three places: entry creation (`lastSentViewport: spawnViewport`, line 257), the send itself (line 333), and the failure rollback (lines 340-341). Nothing clears it across detach/attach. In the incident geometry the embedded box does not change across the detach, so the sync scheduled by the branch below refits the same cols/rows, hits the dedup, and never invokes `pty_resize`: without this line the branch below is a no-op exactly when the incident happens, and the healthy SEEDED cross-grid re-attach is deduped the same way, so field acceptance 9.3 fails even on a fully healthy repro.

Why the placement is fixed at the top of the function (G3; the narrower inside-the-branch variant is REJECTED): the in-branch variant leaves the seeded cross-grid re-attach deduped and leaves the section 7 reconcile race residual unhealed, and either alone fails 9.3. `applySnapshot` has exactly one caller (the attach-settle path) and attach settles are discrete UI events, never per-resize-tick, so the key clears once per settle and the dedup operates unchanged within an attached interval. Worst added cost is one same-size `pty_resize` per attach settle, which `resize_instance` answers with `sent = false` (no follow, no broadcast), plus at most one `reportSpawnSizeDrift` console line. The failure rollback in `sendPtyResize` restores `null` and the retry path is unaffected. The discard branch's own body stays untouched (section 2 non-goal); this line merely runs before it, where letting the next natural resize through is the desired behavior.

Round-2 ratification (Step 7 round 2, W1): the #973 child-protection guarantee transfers its enforcement point. R1's settle send is frontend-visible, and the #973 suite (`TerminalView.spawn-size.test.tsx`) pins frontend `pty_resize` INVOKE COUNTS as its acceptance, so R1 shifts those pins by exactly one send per attach settle. The guarantee "no resize reaches the child while it is starting up" is hereby ratified as enforced like this: WITHIN an attached interval the frontend dedup suppresses identical sends exactly as before (R1 clears the key only at the discrete attach settle); the settle send itself is same-size and is answered by the backend `resize_instance` dedup with `sent = false` (no ConPTY motion, no parser follow, no broadcast; the G3/E6 body reads at `local_backend.rs:1991-1997`, 2013-2015); and during the startup hold window a held size never moves the ConPTY while a deduped size is already the parser's size (G8), so on every path nothing reaches the starting child. The four suite cases that pinned the OLD frontend counts are re-pinned in 9.2a; the suite keeps guarding what the frontend can actually observe (exact send counts, exact dims, drift reporting, retry boundedness). Field evidence reinforcing the every-attach placement: the second #1439 incident (issue #1439 comment 5346328441, 2026-08-19) reproduced the corruption through a plain deselect -> re-select (`source=userSwitch`, `targetDetached=false`, no detach at all), so the healed path must fire on every attach settle, which is exactly what the top-of-function placement provides and what any narrower variant misses.

Second: extend the early no-snapshot branch:

```tsx
    if (!snapshot || snapshot.data.length === 0) {
      if (!entry.hasRenderedOutput) {
        setReplayStatus(entry, SNAPSHOT_UNAVAILABLE_MESSAGE);
      }
      // #1439: a seedless attach must still re-impose this window's grid on
      // the PTY; otherwise live bytes keep arriving for the other window's
      // grid and garble this xterm until the user resizes something.
      if (sessionId === visibleSessionId) {
        scheduleViewportSync(sessionId);
      }
      return;
    }
```

Fixed decision: the guard is the same `sessionId === visibleSessionId` the seeded path uses; hidden sessions keep deferring their sync to the existing visibility flow.

Doc amendment in the same diff (Step 5 N2, ratified, zero behavior): the retention comment above this branch (lines 575-581) closes with a clause claiming that an attach resolved without a snapshot is exactly the unavailable-parser case where every event is unsequenced. #1439 makes that clause false. Rewrite only that clause so the comment states: an attach can resolve without a snapshot either because the parser is unavailable (every event unsequenced) or because the #1439 grid reconcile refused the seed (parser still Available, live events still sequenced); replaying retention without a reset stays a no-op for sequenced events (the watermark drops them) and a duplicate write for unsequenced ones, which is why this branch replays nothing.

## 6. Affected surfaces, exhaustively

| File | Symbol | Change |
|---|---|---|
| `src-tauri/src/pty/output.rs` | `ScreenReplayState` | new field `conpty_size: (u16, u16)` + initialization at construction site(s) |
| `src-tauri/src/pty/output.rs` | `SessionIoFanout::resize_screen_and_broadcast` (1296-1339) | record-first + three WARNs (section 5.2) |
| `src-tauri/src/pty/output.rs` | `SessionIoFanout::activate_terminal_output` (1149-1213) | grid reconcile arm (section 5.3) |
| `src-tauri/src/pty/output.rs` | new `SessionIoFanout::desync_screen_size_for_test` | test-only seam (section 9.1), gated exactly like `poison_screen_parsers_for_test` |
| `src-tauri/src/pty/output.rs` | tests mod | one new test (section 9.1) |
| `src-tauri/src/commands/pty.rs` | `pty_resize` (452-464) | generic `R`, injected `webview`, INFO log (section 5.4) |
| `src/terminal/components/TerminalView.tsx` | `applySnapshot` (570-613) | dedup-key invalidation at function top + viewport sync in the no-snapshot branch + one comment-clause amendment (section 5.5) |
| `src/terminal/components/TerminalView.attachment.test.tsx` | `describe("TerminalView attachment (#1363)")` | one new test case (section 9.2) |
| `src/terminal/components/TerminalView.spawn-size.test.tsx` | the four cases at lines 313, 364, 481, 528 (anchors at `71cd694f`) | round 2: expectations re-pinned to the post-R1 contract (section 9.2a); zero cases added or removed |

No other file changes. `pty/manager.rs`, `pty/local_backend.rs`, `pty/backend.rs`, `pty/container_backend.rs`, `shared/types.ts`, `shared/ipc.ts` are read-only context for this fix.

## 7. Required behavior, edge cases, failure behavior

- Healthy re-attach (grids agree): byte-identical behavior to today, including snapshot content, sequence, batch cut, and viewport sync. The reconcile compares two u16 pairs under a lock already held; no measurable cost.
- Re-attach after divergence (any cause): snapshot is `None`; WARN `stage=attach_grid_mismatch` names both grids; parser grid converges to the ConPTY grid; window attaches live; visible sessions schedule viewport sync from the frontend branch (5.5), whose `pty_resize` is guaranteed to fire because the dedup key was invalidated at settle (5.5, R1); the next attach after the child repaints seeds cleanly. No reset is issued on the attaching xterm (existing `None` contract), so no content is destroyed.
- Idle session attach: unaffected. `set_size` still applies immediately on the follow call, so an idle session's grids agree and it seeds exactly as today. (This is the reason the deferred-application design is rejected; see 5.1.)
- Skipped follow paths: `parsers_lock_poisoned` -> WARN; the same poisoned lock makes `activate_terminal_output` return `Ok(None)` (existing behavior), so no stale seed can follow this skip. `no_parser_entry` -> WARN; attach on a missing entry returns `Err(SessionUnavailable)` (existing). `parser_unavailable` -> WARN; `conpty_size` is still recorded; attach returns `None` via the existing `Unavailable` arm. In every skip case the user-visible outcome is live-attach-without-seed, never garbage.
- `set_size` panic (either call site): existing fault transition, unchanged semantics: `Unavailable`, `stage=parser_fault`, `flush(id, false)`, batches unsequenced from the transition on.
- `0x0` resize: refused and WARNed before any record, exactly as today (#973); `conpty_size` keeps the last size the ConPTY actually took.
- MAX_ROWS / MAX_COLUMNS / MAX_CELLS caps in the render block: unchanged, still applied after the reconcile passes.
- Sequence/watermark: a reconcile `None` neither advances nor consumes `output_sequence`; retained-event admission on the frontend behaves exactly as any other no-snapshot attach.
- `pty_resized` websocket broadcast: emitted on exactly the same conditions as today (successful non-faulting follow).
- Known residual (accepted, documented): the divergence-interval parser cells stay unreliable until the child's next full repaint; the plan trades a garbled seed for a `None` seed during that interval. A second residual: a healthy cross-grid re-attach still reflows previously seeded wide content in the local scrollback (cosmetic, scrollback-only, inherent to the #1432 seed-then-refit design); out of scope, noted for the record.
- Reconcile false-negative window (Step 6 G6, documented residual): `LocalProcessBackend::resize` computes `sent` under the `ptys` guard and calls the follow only after dropping it, so an attach landing between the ConPTY resize and the follow sees parser == record == old grid and seeds the old grid. That seed is internally consistent; live bytes garble only until the 5.5 sync re-imposes the attacher's grid, so the residual is transient and self-heals BECAUSE of the R1 invalidation. Closing it would require holding `ptys` across the follow, which the in-code comment rejects for lock-queueing reasons; out of scope. Triage rule for 9.3: a garble report WITHOUT a `stage=attach_grid_mismatch` WARN is attributed to this race, not read as the fix failing.
- Transport backends (Step 6 G5, documented residual): `ContainerTransportBackend::resize` calls the follow unconditionally after merely queuing the resize frame to the bridge; no applied-ack exists, so for container sessions `conpty_size` records the size last requested of the remote and the reconcile is structurally blind to remote-apply divergence. Identical blindness ships today, parser and record always move together there (no new false positives), and the local path's `if sent` gate keeps the record honest where the incident lives.
- Retry-budget accounting in an all-fail world (round 2, W1, accepted residual): when every `pty_resize` invoke fails, the R1 settle burst consumes the whole retry budget first. `resizeRetryAttempts` resets only on success (`sendPtyResize`, `TerminalView.tsx:319-347`; `scheduleResizeRetry`, 292-317; `PTY_RESIZE_MAX_RETRIES = 3` and `PTY_RESIZE_RETRY_DELAY_MS = 120`, lines 44-45), so a later user resize still sends its first attempt (first attempts are never budget-gated) but arms no retry, and the loud "giving up" line fires at the settle burst and again after that attempt. The outcome stays bounded and loud, only the burst that spends the budget changes, and the state is reachable only while the resize IPC is persistently dead, a world in which no frontend policy can correct the PTY grid anyway. Pinned by the re-pinned budget test (9.2a, T4).

## 8. Compatibility and security

- IPC: no shape change. `PtyScreenSnapshotPayload` unchanged; `pty_resize` invoke arguments unchanged (the webview is Tauri-injected); no TypeScript type or wrapper changes.
- Cross-backend: the fix lives in `SessionIoFanout`, shared state and methods; backends inherit the record and reconcile mechanics uniformly, with the G5 caveat that only the local backend's `if sent` gate makes the record a taken-size fact (transport backends record the requested size; section 7 residual). No backend trait signature changes.
- Logging hygiene: new log lines carry session UUID, dimensions, window label, and stage/reason tokens only; no terminal payload bytes are ever logged (consistent with the existing `stage=` lines).
- Dependency-cycle gate (executed at Step 7, result PASS; full statement in the Step 7 consensus section): this plan adds ZERO new module-to-module references. `commands/pty.rs` already imports tauri (line 4) and already hosts two generic-with-webview command signatures (`activate_terminal_output` at 526-528 and its neighbor at 570), verified by direct read; `output.rs` additions reference only symbols already used in `output.rs` (`log`, `crate::logging::catch_payload_unwind`, `ParserAvailability`, `attachments`); the TSX change calls a function already in the same component scope. No lower layer gains an `AppHandle`/`tauri` dependency. Because no implementation tree exists at plan stage, a pre/post detector run is vacuous; certification used the explicit per-arc manual analysis, and the implementation review MUST run `rust-levelization-run` pre/post (base SHA vs final branch head, clean trees for both) and require: `cyclicSccs` unchanged, every cyclic SCC member set identical set-to-set, zero new cross-boundary `from -> to` pairs, regenerated arc record byte-identical, structural layering guards green.

## 9. Tests and objective acceptance criteria

Exactly two tests. No others. Round 2 re-pins four existing #973 cases (9.2a) without adding any.

### 9.1 Rust regression test (`src-tauri/src/pty/output.rs`, existing tests mod)

Seam, following the existing `poison_screen_parsers_for_test` / `exhaust_output_sequence_for_test` precedent and their exact cfg gating:

```rust
    /// #1439 test-only: force the parser grid WITHOUT recording a ConPTY grid,
    /// simulating any historical silent-skip divergence.
    pub(crate) fn desync_screen_size_for_test(&self, id: Uuid, rows: u16, cols: u16) { ... }
```

(sets `state.parser.set_size(rows, cols)` directly and touches nothing else.)

Test `a_grid_divergence_yields_no_seed_and_the_next_attach_seeds_clean`, using the harness of `attach_cuts_the_batch_exactly_at_the_snapshot_boundary` (`output.rs:2477-2501`): `fanout()`, `new_sink()`, `session_with_sink`, `registration_token_for_test`, `attach`, `handle_output`, `flush`, `events`, `WINDOW`/`SECOND_WINDOW`:

1. Install a session; drive one follow to a known grid A via `resize_screen_and_broadcast` (grids agree: parser = recorded = A).
2. `handle_output` one frame of bytes (history non-empty, sequence advances).
3. `desync_screen_size_for_test` to grid B (parser B, recorded A: the divergence class).
4. `attach(WINDOW)` must return `None` (assert). This is the regression assertion: pre-fix logic returns a `Some` snapshot carrying the stale grid.
5. `handle_output` another frame; `attach(SECOND_WINDOW)` must return `Some`, whose `rows/cols` equal grid A (converged) and whose `sequence` is the current one (assert all three). Existing invariants must hold: the first window keeps receiving its bytes; `pending_output_bytes_for_test` behaves as in the neighboring tests.

Grid constraints (Step 6 G1, binding): the registration grid R0 (the grid the harness installs the session at), the follow grid A, and the desync grid B must be pairwise distinct, each asymmetric (rows != cols), and no pair may be a transposition of another. Rationale: with A == R0, an implementation that never writes `conpty_size` still passes every step above (the field's init value already equals A), and that missing write would make every post-resize attach converge the parser to the WRONG grid in production; with A != R0 the step-5 assertion turns red against the missing record, because convergence would land on R0 instead of A. Asymmetric, non-transposed grids also make a rows/cols argument swap fail loudly (the #973 bug class; the codebase mixes `(cols, rows)` and `(rows, cols)` orders across these calls). Implementer note (Step 5 E5): match the harness `attach` helper's actual return shape when asserting `None` (the neighboring test consumes it with `.expect(...)`).

Objective criteria: the new test passes; every existing test in `output.rs` (notably `attach_cuts_the_batch_exactly_at_the_snapshot_boundary`, `a_parser_fault_flushes_at_the_transition_and_unsequences_everything_after`, `an_unavailable_parser_still_attaches_and_emits_unsequenced`, `a_degenerate_resize_never_reaches_the_vt100_parser`, `resize_and_capture_race_returns_one_complete_dimension_set`, `a_resize_that_does_not_move_the_sequence_still_returns_a_frame`) passes unchanged. Run per repo convention with output redirected to a file (Windows note: `cargo test` stdout must be redirected to be readable).

### 9.2 Frontend regression test (`src/terminal/components/TerminalView.attachment.test.tsx`)

New case inside the existing `describe("TerminalView attachment (#1363)")`, using the file's mocks (`vi.mock` of `@xterm/xterm` FakeTerminal, fit/webgl addons, platform) and its IPC fake layer (the `fake.resolve("pty_resize", ...)` pattern at line 148):

Name: `a re-attach that resolves without a snapshot resyncs the viewport before live writes land`.

1. Mount with a visible session and complete a normal first attach per the existing helpers.
2. Re-attach with the `activate_terminal_output` fake resolving `null` (the reconcile-miss outcome).
3. Assert `pty_resize` is invoked with the local terminal's cols/rows as part of the attach settle, and that the invocation is ordered BEFORE the next live `pty_output` write reaches the FakeTerminal (use the fake call log / write spy ordering the harness already exposes).
4. Assert the no-snapshot contract held: no `terminal.reset()` on the FakeTerminal during this re-attach, and the pre-detach buffer content was not cleared.

Harness mechanics (Step 5 E5 + Step 6 G2, binding): (i) pin the harness so the FakeTerminal fit dims differ from the entry's `spawnViewport`, otherwise the priming assert in (ii) dedups at creation time and the case loses its red-first property; (ii) `scheduleViewportSync` defers through two nested `requestAnimationFrame` callbacks (`TerminalView.tsx:381-395`), so flush BOTH frames for the FIRST attach too, and assert that first attach produced exactly one `pty_resize` carrying the fit dims: this primes `lastSentViewport` and proves the priming; (iii) clear or segment the fake invoke log before the re-attach so step 3 counts only re-attach calls; (iv) flush both rAF frames on the re-attach settle before delivering the live `pty_output` write. With (i)-(iv) the case is deterministically red against 5.5 without the R1 invalidation line and green with it. Reading note (Step 5 E5): the ordered-before-live-write assertion documents harness-controlled ordering, not a production invariant; in production a live byte can land during the rAF gap, and the objective stays convergence without user intervention (section 2).

Objective criteria: the new case passes; every existing case in the file passes unchanged; the full frontend suite stays green.

### 9.2a Round-2 re-pin: the four #973 invoke-count cases (`src/terminal/components/TerminalView.spawn-size.test.tsx`, anchors at `71cd694f`)

Why: R1 admits exactly one settle-driven `pty_resize` per attach settle, and this suite pins frontend invoke counts as #973's acceptance; the collision is test-contract level only (5.5 round-2 ratification). The new pins below are the binding contract. They were derived from the implementer's measured post-R1 run (blocker report 20260819-190311: T1 observed 1, T2 observed 2, T3 premise observed 2, T4 observed 6) plus a direct read of the suite and of the resize mechanics at `71cd694f`. If any pin misses under a scheduling difference during implementation, STOP and report the observed sequence; do not adjust counts unilaterally.

Suite mechanics every re-pin builds on (verified by direct read):

- Every attach in this suite settles through the no-snapshot branch: the suite's `activate_terminal_output` fake returns `data: []`, so `applySnapshot` takes the 5.5 early branch and the settle sync is that branch's `scheduleViewportSync`.
- The file installs `installDeterministicAnimationFrames()`: rAF callbacks run only under `frames.flush()` / `frames.flushFrame()`, and `waitFor` sleeps real time without advancing frames. A settle sync whose rAFs are queued after a flush has drained would never run under flush-then-assert, so every NEW settle-send assertion must use the file's own `driveFramesUntil(frames, ...)` helper (drive, re-check, stop), then may flush the remainder and assert stability.
- Resize mechanics (`TerminalView.tsx`): `sendPtyResize` (319-347) dedups on `lastSentViewport`, sets the key optimistically, resets `resizeRetryAttempts` to 0 on success, and on failure rolls the key back, warns, and calls `scheduleResizeRetry` (292-317), which coalesces onto a pending timer, gives up loudly at `resizeRetryAttempts >= PTY_RESIZE_MAX_RETRIES` (3), and otherwise arms one real timer (120ms x attempt) that re-sends the terminal's current dims. `reportSpawnSizeDrift` (271-290) fires at most once per entry (`spawnDriftReported` latch) and only for dims that differ from `spawnViewport`.
- Consequence per settle: with sends succeeding, exactly ONE extra invoke (the first post-clear sync sends; the second rAF frame and every later sync dedup against the freshly written key). With every send failing, the rollback re-nulls the cleared key between frame passes, so the settle burst is one rAF-frame send, at most one repeat from the second frame, then exactly 3 timer retries, then the loud give-up: 4 or 5 sends, bounded either way.

The four re-pins. In all of them `resizesFor(fake, SPAWNED)` payload asserts include `sessionId: SPAWNED`; comment rewrites are free in wording but must name #1439/R1 and, where the old comment claimed "no resize reaches the child", the `resize_instance` dedup as the new enforcement point.

**T1 (line 313, was `issues no resize at all when the fitted size already equals the spawn size`; expected 0 sends).** New title: `sends exactly one same-size resize at the attach settle, and the backend dedup keeps it from the starting child`. Replace the final flush-then-assert-0 sequence with: `await driveFramesUntil(frames, "the settle sync sent", () => resizesFor(fake, SPAWNED).length > 0)`; assert the payload list equals exactly `[{ sessionId: SPAWNED, cols: 74, rows: 23 }]`; then `await frames.flush()` and assert the list is STILL exactly that one element (the second rAF frame and any other queued sync dedup: one send per settle, no more). Keep the ON_SCREEN `length > 0` assert unchanged. Red/green: without R1 the settle sync dedups against the creation-primed key and the count stays 0, so the case is red (a live R1-regression detector); wrong dims or a more-than-once-per-settle key clear (the storm class G3 excluded) also fail the exact-payload pin.

**T2 (line 364, was `resizes exactly once, and says so, when the fit drifts from the spawn size`; expected exactly 1 send of 74x24).** New title: `resizes once for the drift and once more at the settle, and reports the drift exactly once`. After the attach: `await driveFramesUntil(frames, "drift send and settle re-send landed", () => resizesFor(fake, SPAWNED).length >= 2)`; assert the payload list equals exactly `[{ sessionId: SPAWNED, cols: 74, rows: 24 }, { sessionId: SPAWNED, cols: 74, rows: 24 }]` (the pre-settle drift send, then the settle re-imposition of the identical size); then `await frames.flush()` and assert still exactly 2. Strengthen the drift assert to EXACTLY one warn call containing `spawn-size drift` (deterministic: the `spawnDriftReported` latch, and the settle re-send carries no drift anyway). Red/green: without R1 the settle re-send dedups and the count stays 1, red against the new pin; the exact-two pin still kills the original #973 defect class (any identical-resize burst beyond the two named senders).

**T3 (line 481, `re-sends a failed resize that is the only one of its burst`; title and object KEPT).** Problem to fix: post-R1 the settle send (74x23) is the first SPAWNED send, so the fake's positional `spawnedAttempts === 1` failure is consumed by the settle send (premise observed: failed settle send plus its timer retry = 2), and the user drag then succeeds first try, so the case stops exercising a failed USER resize. Re-aim the failure by dims, not position: replace the counter with a one-shot latch that throws for the FIRST SPAWNED invoke whose `rows === 24` (the drag size; the settle sends rows 23, which must succeed). New premise (replacing the assert-0): `await driveFramesUntil(frames, "the settle send landed", () => resizesFor(fake, SPAWNED).length > 0)`; assert the payload list equals exactly `[{ sessionId: SPAWNED, cols: 74, rows: 23 }]`; `await frames.flush()`; assert still exactly 1 (settle send succeeded, so nothing is armed and nothing else will send until the drag). Then keep `spawned.emitResize(74, 24)` and re-pin the tail: `waitFor(() => expect(resizesFor(fake, SPAWNED).length).toBeGreaterThanOrEqual(3), 2000)`, then assert entries [1] and [2] both equal `{ sessionId: SPAWNED, cols: 74, rows: 24 }` (the failed drag attempt and its rollback-driven timer re-send). Update the leading comment block: the settle send now exists and succeeds; after it settles, nothing else in the system sends for this session until the drag, so the drag's failure still has nothing to hide behind. Red/green: the case keeps its original object (a rollback that is not a real retry leaves the drag at one call) verbatim on the drag segment, and the premise now also pins the settle send's exact payload.

**T4 (line 528, `gives up loudly, and boundedly, when the resize can never land`; title and object KEPT).** The all-fail fake is unchanged. Force the order so the pins are deterministic: after the existing `frames.flush()`, move the give-up `waitFor` to BEFORE the drag; the settle burst alone exhausts the budget (one rAF-frame send, at most one second-frame repeat, exactly 3 timer retries at 120/240/360ms, then the loud line; the existing 3000ms budget covers it). Then record `const settleSends = resizesFor(fake, SPAWNED).length` and assert `settleSends <= 5`. Then `spawned.emitResize(74, 24)`; `await waitFor(() => expect(resizesFor(fake, SPAWNED).length).toBe(settleSends + 1))`; assert the LAST payload equals `{ sessionId: SPAWNED, cols: 74, rows: 24 }` and the total is `<= 6`. Keep the `giving up` console.error assert as-is (already satisfied by the settle burst; it fires again after the drag attempt). Replace the budget comment: the budget is one frame-driven send plus at most one frame repeat plus `PTY_RESIZE_MAX_RETRIES` timer re-sends, all consumed by the settle burst in an all-fail world, after which the user's own attempt still goes out (first attempts are never budget-gated) but arms no retry until a success resets the counter (section 7 residual, round 2). Red/green: boundedness stays the object and is tightened per segment: the drag adds EXACTLY one send, pinning "no retry without a prior success"; an unbounded or key-corrupting implementation reddens on the <= 6 bound.

The OTHER FIVE cases in the file must pass UNCHANGED, and the file's case count stays 9. Per case, why they hold: `opens the PTY at the size...` (295) asserts `create_session` args only; `starts xterm at the size...` (345) asserts the xterm start size and xterm reflows (`spawned.resizes`), never invoke counts, and the settle sync's `fit()` is a no-op on an already-fitted terminal; `still fits and resizes a session opened without a spawn size` (397) asserts `length > 0` and the FIRST payload, both stable under one extra deduped-or-first settle send; `sends no size for a collapsed tile...` (434) same shape (`> 0` plus first payload); `never measures a hidden session...` (587) asserts exact ON_SCREEN payload lists whose settle syncs are either blocked by the visibility guards while hidden or dedup to the same single send while visible. Any of the five turning red during implementation is a STOP-and-report signal, not a case to amend.

Objective criteria (9.2a): `npx vitest run src/terminal/components/TerminalView.spawn-size.test.tsx` reports 9 passed (9) with the full step-4 change applied; red-first spot check: with ONLY the R1 line removed and everything else in place, T1 and T2 go red on their new counts (0 and 1 observed instead of 1 and 2); the full frontend suite stays green (9.2 criteria unchanged).

### 9.3 Field acceptance (post-merge verification, evidence over intuition)

Reproduce the incident choreography (embedded at a small grid, detach, resize detached window, keep output active ~1 min, re-attach): the embedded view must show either a clean seed or a live view that converges without manual resizing, and `app.log` must now contain the INFO resize lines for both windows plus, if any skip occurred, exactly one WARN naming its `reason=`. Any `stage=attach_grid_mismatch` WARN in the field identifies the residual divergence source with evidence, which is the diagnosability the issue demands.

Second, cheaper repro (round 2, from the field): per issue #1439 comment 5346328441, deselect a working embedded session, let its agent keep printing for a while, then re-select it; same acceptance criteria as above. This choreography involves no detached window at all (`source=userSwitch`, `targetDetached=false` in the incident log), so it exercises the reconcile and the R1 re-imposition through the plain selection-switch attach path; run both choreographies.

9.3 spans BOTH halves and runs only after the Rust half (steps 1-3) and the TypeScript half (step 4) are both landed on this branch: the backend half supplies trustworthy-or-None seeds and the log evidence, the frontend half supplies the guaranteed grid re-imposition, and neither half alone satisfies the criteria above (see also the section 7 triage rule for a garble without a mismatch WARN).

## 10. Implementation order

1. `output.rs`: `ScreenReplayState.conpty_size` + record-first + WARNs (5.2). Compile + existing tests green. Owner: dev-rust.
2. `output.rs`: reconcile arm in `activate_terminal_output` with the hoisted fault pair (5.3) + seam + Rust test (9.1). Green. Owner: dev-rust.
3. `commands/pty.rs`: `pty_resize` generic + INFO (5.4). Green. Owner: dev-rust.
4. `TerminalView.tsx`: dedup-key invalidation + no-snapshot viewport sync + comment amendment (5.5) + frontend test (9.2) + the four #973 re-pins (9.2a). Green, including the full frontend suite. Owner: dev-webpage-ui.

Each step is independently compilable and testable; no step depends on a later one. Ownership and landing order (Step 7, ratifying dev E7): the Rust half (steps 1-3, dev-rust) lands first, the TypeScript half (step 4, dev-webpage-ui) lands second, both on this branch. The 9.2 harness fakes the backend, so step 4 has no build-time dependency on steps 1-3; the ordering is a certification ordering, not a technical one. Field acceptance 9.3 runs only after both halves are landed (section 9.3).

## Dev enrichment (Step 5)

Author: dev-rust, wg-8, 2026-08-19 UTC, as implementer-to-be. Independently re-verified at the frozen SHA `b19ee185` (fresh CBM gate, ready first attempt, same head; 20 graph operations; 1 bounded fallback = one union rg over `TerminalView.tsx` + `TerminalView.attachment.test.tsx` for the `lastSentViewport` lifecycle and the vitest harness anchors, both unindexed needs). Every file path, symbol, line range, and quoted body in sections 3-9 that I re-read matched the plan exactly: `resize_screen_and_broadcast` (1296-1339), `activate_terminal_output` fanout (1149-1213) and command (526-559), `handle_output` (982-1122), `LocalProcessBackend::resize` (1986-2018), `register_session` (885-925), `poison_screen_parsers_for_test` (1645-1652), the boundary test (2477-2501), `applySnapshot` (570-613), `scheduleViewportSync` (381-395), `syncViewport` (349-359), `sendPtyResize` (319-347). No incorrect plan item found. Two findings below (R1 required addition, R2 required adjustment) need Step 7 ratification; neither reopens a fixed decision.

### E1. Position on the section 5.1 deviation: ACCEPT

I accept record-first + WARN-on-skip + attach-time reconcile, with the `set_size` application point unchanged, on re-verified evidence:

- `handle_output` runs `parser.process`, the sequence advance, and the history append under the same `screen_parsers` mutex the follow takes, so `set_size` already lands only between chunks. Relocating the call to the reader adds no atomicity that the mutex does not already provide.
- A deferred apply-on-next-chunk fires only inside `handle_output`, which runs only when bytes arrive. An idle session would hold a stale parser grid indefinitely, and the new reconcile would then refuse to seed perfectly healthy idle attaches. The deferred design and the reconcile are mutually exclusive; the reconcile is the one that heals unknown divergence sources.
- The wedge is real and verified: `LocalProcessBackend::resize` resizes the ConPTY first (`resize_instance` returning `sent: bool`) and follows only `if sent`. A deduped size never follows, so one silent skip today is permanent, exactly as section 3 argues. Only an attach-time reconcile recovers from that class.
- No injectable in-stream marker exists between ConPTY and the reader, so "atomic relative to the output stream" is unattainable in the literal sense; the residual backlog mis-parse window is bounded and cannot poison a seed's grid once the reconcile exists.

I also confirm the correction to my own Step 2 brief: "a healthy parser snapshot contains only the final screen" is FALSE on the non-empty-history path. Verified in the body: seed bytes are `UI_HISTORY_REPLAY_PROLOGUE` + both history slices whenever `include_history && !state.history.is_empty()`; `contents_formatted()` only on empty history; the grid alone always comes from `parser.screen().size()`. Neither planned test asserts the old phrasing.

### E2. R1 (REQUIRED addition): section 5.5 is defeated by `sendPtyResize`'s dedup in the exact incident scenario

`sendPtyResize` (`TerminalView.tsx:319-347`) early-returns when `entry.lastSentViewport` already equals the requested cols/rows. `lastSentViewport` is written in exactly two places: entry creation (`lastSentViewport: spawnViewport`, line 257) and `sendPtyResize` itself (set at 333, rollback on IPC failure at 340-341). Nothing clears it on detach or attach; it survives the whole detached interval.

Consequence in the incident geometry: embedded window last sent 81x27; session detaches; the detached window drives the PTY to its own grid; re-attach resolves with no seed (the new reconcile miss); plan 5.5 schedules the viewport sync; `syncViewport` runs `fit()`, which recomputes 81x27 because the embedded box never changed; `sendPtyResize(81, 27)` compares equal to `lastSentViewport` and returns WITHOUT invoking `pty_resize`. The PTY stays at the other window's grid and the live garble persists indefinitely. Plan 5.5 as written is a no-op precisely when the local grid did not change across the detach, which is the reported incident.

The same stale key also breaks the SEEDED cross-grid re-attach (grids healthy and agreeing, seed carries the detached grid): `resizeTerminalForSnapshot` adopts the seed grid locally without a PTY resize (by design), the viewport sync then fits back to the local box dims, and the re-imposition is deduped identically. Live bytes keep arriving for the detached grid and the field acceptance in 9.3 FAILS on a fully healthy repro, with or without the backend reconcile.

Required minimal fix, one line, for Step 7 to ratify into 5.5: invalidate the dedup key at attach settle before scheduling the sync:

```tsx
      entry.lastSentViewport = null;
```

Recommended placement: first line of `applySnapshot` (covers the no-snapshot arm, the discard arm, and the seeded arm with one line; the discard arm schedules no sync, so there the reset only lets the next natural resize through, which is desirable for the same reason). Minimum viable placement if Step 7 prefers the narrowest diff: inside the no-snapshot branch next to the plan's `scheduleViewportSync` call, accepting that the healthy seeded cross-grid case keeps the pre-existing garble. I recommend the top-of-function placement: it is the same single line, it is the only variant that makes 9.3 pass on a healthy repro, and its worst cost is one redundant same-size `pty_resize` per attach, which `resize_instance` dedups backend-side (`sent = false`, no follow, no broadcast). The failure rollback in `sendPtyResize` stays correct (it restores `previous`, i.e. null, and the retry path is unaffected).

Note for 9.2: the test as specified in the plan is exactly the red test for this hole. In the harness the local dims do not change across the re-attach, so against plan-5.5-as-written the `pty_resize` assertion fails on the dedup; it passes once the invalidation line exists. Keep the test exactly as specified.

### E3. R2 (REQUIRED adjustment): hoist the 5.3 flush out of the `screen_parsers` lock scope

The 5.3 snippet calls `self.attachments.flush(id, false)` inside the mismatch-panic arm, while the `parsers` guard is still held (it lives until `drop(parsers)` at the function tail). Both existing fault sites deliberately flush AFTER releasing that lock: `resize_screen_and_broadcast` captures a `parser_fault` bool and flushes outside the block, and `handle_output` returns `Accumulated::FlushNow` to run the flush after the lock, with the in-code comment "The emit it may ask for is run below, after the lock is dropped". `attach` and `accumulate` under the lock are established practice (lock order `screen_parsers` then attachments); `flush` EMITS to webviews, and an emit under the parser lock stalls the PTY reader on its next chunk.

Adjustment, semantics unchanged: in the mismatch arm set a local fault flag (and keep returning `None`), then after `drop(parsers)` run the existing pair `log::error!("[terminal-snapshot] stage=parser_fault session={id}")` + `self.attachments.flush(id, false)`, mirroring `resize_screen_and_broadcast` line for line. The post-lock flush race (a chunk accumulating between drop and flush) is the same one the two existing sites already accept.

### E4. Hidden coupling and flags (no plan change)

- N1: the existing render-panic arm in `activate_terminal_output` (`Err(_) => { Unavailable; None }`) neither logs `stage=parser_fault` nor flushes; after 5.3 the function will contain two adjacent panic arms with different logging behavior. The plan's new arm matches the two canonical fault sites and is the better behavior; leave the old arm alone (out of scope), but Step 6/7 should be aware the asymmetry is pre-existing, not introduced.
- N2: the comment block above the no-snapshot branch in `applySnapshot` states that "attach resolved without a snapshot" is "exactly the unavailable-parser case, where every event is unsequenced". After #1439 that sentence is false: a reconcile miss returns no seed while the parser stays Available and live events stay SEQUENCED. The 5.5 edit lands inside this exact branch; amend that one sentence in the same diff (doc-only, zero behavior).
- N3: the no-watermark admission path must accept sequenced live events. Expected fine: the existing caps-exceeded arm (`rows > MAX_ROWS` etc. returning `None` with the parser Available) already produces "no seed + sequenced live events" today. Implementer must confirm `writeLivePtyOutput` (reached via `handlePtyOutput`, `TerminalView.tsx:762-771`) admits them on the new path; that read is free at edit time.
- N4: the 5.4 INFO line covers the desktop command path only. Any non-command resize driver (e.g. a websocket/phone client, if one drives resizes) reaches `PtyManager::resize` without an INFO line. Acceptable for 9.3, which choreographs desktop windows; recorded so nobody reads absence of the INFO line as absence of a resize.
- Registration macro: `generate_handler!` content is not indexed, but the generic `activate_terminal_output` command is registered and shipping today, so the 5.4 genericization has a working precedent; check the macro site visually when editing (expected: no change needed).

### E5. Test-harness fit

9.1 (Rust): fits as specified. Every named harness piece exists in the boundary test at 2477-2501 (`fanout()`, `new_sink()`, `session_with_sink`, `registration_token_for_test`, `attach`, `handle_output`, `flush`, `events`, `pending_output_bytes_for_test`, `WINDOW`/`SECOND_WINDOW`), the seam precedent `poison_screen_parsers_for_test` is `pub(crate)` on `SessionIoFanout`, and `PtyScreenSnapshot` exposes `rows/cols/sequence` for step 5's asserts. Two implementer notes: (a) use ASYMMETRIC grids for A and B (e.g. A=24 rows x 80 cols, B=30x100) so a rows/cols transposition cannot pass; the codebase mixes orders (`resize_screen_and_broadcast(id, cols, rows)` vs `set_size(rows, cols)` vs the seam's proposed `(id, rows, cols)`), which is the #973 bug class; (b) the test helper `attach` is consumed with `.expect(...)` in the neighboring test, so match its actual unwrap shape when asserting `None` (free read at edit time). Run with output redirected to a file per repo convention.

9.2 (frontend): fits with one mechanical addition. All anchors verified: the `describe` at line 176, `fake.resolve("pty_resize", ...)` at 148, the `fake.onInvoke("activate_terminal_output", ...)` override pattern (lines 241/400/430) which is how the re-attach resolves `null`, `fake.callsFor(name)` for the invocation assert, and the `FakeTerminalInstance` log for write/reset ordering. The addition: `scheduleViewportSync` defers through TWO nested `requestAnimationFrame` callbacks (381-395), so the test must flush both frames before delivering the live `pty_output` event (whatever rAF driver the file's setup provides, or a minimal shim if none exists; that is the "minimal adjustment" the dispatch anticipated). One reading note: the "ordered BEFORE the next live write" assertion documents harness-controlled ordering, not a production invariant; in production a live byte can land during the rAF gap, and the objective remains convergence without user intervention, per section 2.

### E6. Edge cases re-checked and clean

- Startup held-size window: `conpty_size` initializes to the PTY-open size at `register_session` (single production construction site, `rows/cols` in scope as claimed; `vt100::Parser::new(rows, cols, 0)` on the same values). During a held resize the parser also sits at the open size, so an attach during the hold agrees and seeds; `open_startup_gate` then moves both together. No idle/startup wedge.
- Identity-mismatch arm (`OutputTargetUnavailable`) precedes the reconcile and is untouched.
- Poisoned-lock attach arm returns `Ok(None)` after attaching, as section 7 claims (verified in the body); no record, no seed, no garbage.
- The `0x0` refuse stays ahead of the lock and the record, as the rewrite shows; `conpty_size` keeps the last taken size.
- `resize_instance` backend dedup makes R1's redundant resize a no-op (`sent=false`, no follow, no `pty_resized` broadcast): no log spam beyond one INFO per attach settle.

### E7. Implementation ownership and order

Plan section 10 order stands, with R2 folded into step 2 and R1 into step 4. Ownership flag for the tech-lead: steps 1-3 are Rust and mine; step 4 (5.5 + R1 + 9.2) is TypeScript, which my role boundaries assign to the frontend owner. Either dispatch step 4 to dev-webpage-ui with this plan, or explicitly re-assign it; the split is clean because no step depends on a later one, but 9.3's field acceptance needs BOTH halves landed.

### E8. Scope discipline audit

Still exactly two tests, zero new dependencies, zero new module arcs, zero IPC shape changes. R1 adds one frontend line inside a function the plan already edits; R2 moves two lines the plan already adds across a lock boundary; N2 amends one comment sentence in the same branch the plan already edits. Nothing else grew.

## Grinch enrichment (Step 6)

Author: dev-rust-grinch, wg-8, 2026-08-19 UTC, adversarial pass. Independently verified at the frozen SHA `b19ee185`: fresh CBM gate ready first attempt (same head, project `D-0_repos-AgentsCommander_iac-.ac-wg-8-dev-v5-team-repo-AgentsCommander`), 20 graph operations (the ceiling; two spent on failed qualified-name guesses, one on an arg-parse retry), 1 bounded fallback (one 18-line rg over `TerminalView.tsx` for the `snapshotReplayPending` / `lastSentViewport` / `lastAppliedSequence` lifecycles, all unindexed). Bodies I re-read verbatim matched the plan and Step 5 exactly: `resize_screen_and_broadcast` (output.rs:1296-1339), fanout `activate_terminal_output` (1149-1213), `register_session` (885-925), `LocalProcessBackend::resize` (local_backend.rs:1986-2018), `open_startup_gate` (1694-1711), `applySnapshot` (TerminalView.tsx:570-613), `sendPtyResize` (319-347), `scheduleViewportSync` (381-395), `writeLivePtyOutput` (478-491), `shouldDropAlreadyAppliedEvent` (441-447), plus `ContainerTransportBackend::resize` (container_backend.rs:3182-3203), which no prior step had read.

Verdict up front: no BLOCKER. Four REQUIRED (G1-G4: two test-hardening demands without which BOTH regression tests can go green over a live defect, plus the ratification of dev's R1 and R2 with one sharpening each) and four NOTEs (G5-G8). Step 7 can proceed if it ratifies G1-G4 together with R1/R2.

### G1 (REQUIRED): test 9.1 as specified goes green over an implementation that never records — pin three pairwise-distinct grids

- **What:** 9.1 step 1 says "drive one follow to a known grid A" without constraining A against the registration grid. `register_session` seeds the parser (and, per 5.2, `conpty_size`) from the same rows/cols (output.rs:904). If the test picks A equal to the registration grid R0 — the natural reuse in that harness — an implementation that OMITS the `state.conpty_size = (rows, cols)` write entirely still passes all five steps: the record stays at its init value == A, step 3's desync to B mismatches, step 4 returns `None`, convergence lands on A, step 5 seeds at A. Green.
- **Why:** with the record write missing, the first real resize to any grid X moves ConPTY and parser to X while `conpty_size` stays frozen at R0; every later attach then reconciles parser X against record R0, WARNs, returns `None`, and converges the parser to R0 — the WRONG grid — after which the next attach seeds at stale R0 while the ConPTY sits at X. The shipped "fix" would permanently manufacture the exact garbled-seed class it exists to kill, for every session that ever resized, with both tests green.
- **Fix:** one constraint sentence in 9.1: registration grid R0, follow grid A, and desync grid B MUST be pairwise distinct, each asymmetric (rows != cols), and no pair a transposition of another (extends dev E5(a) to R0). With A != R0, the existing step-5 assertion (`rows/cols == A`) turns red against a missing record, because convergence would land on R0.

### G2 (REQUIRED): test 9.2's red-first property is conditional — pin the dedup-key priming

- **What:** dev E2's claim "the test as specified is exactly the red test for this hole" holds only if `entry.lastSentViewport` EQUALS the local fit dims when the re-attach sync runs. At entry creation the key is `spawnViewport` (TerminalView.tsx:257); it becomes the fit dims only if the FIRST attach's viewport sync actually completed (both `requestAnimationFrame` frames of `scheduleViewportSync`, 381-395, flushed, so `sendPtyResize` wrote the key at 333). If the harness never flushes rAF during the first attach AND its spawnViewport differs from the FakeTerminal fit dims, the re-attach `sendPtyResize` misses the dedup (325-328) and invokes `pty_resize` even WITHOUT R1: the case passes while the incident-critical line is absent.
- **Fix:** ratify into 9.2's mechanics: (i) pin the harness so the FakeTerminal fit dims differ from `spawnViewport`; (ii) flush the double rAF for the FIRST attach too and assert it produced exactly one `pty_resize` with the fit dims (priming proof — note that if fit dims equaled spawnViewport this assert would itself fail on the creation-time dedup, which is why (i) is part of the demand); (iii) clear or segment the fake invoke log before the re-attach so step 3's assertion counts only re-attach calls; (iv) keep dev E5's rAF flush before delivering the live write. With (i)-(iv) the case is deterministically red against 5.5-without-R1 and green with R1.

### G3 (REQUIRED): R1 confirmed at the code — and only the top-of-`applySnapshot` variant is ratifiable

Confirmed: `lastSentViewport` is written at exactly three places — creation (257), send (333), failure rollback (340-341) — and cleared by nothing across detach/attach, so the dedup (325-328) blocks the healing resize precisely when the local box did not change, which is the incident geometry. I could not construct a resize-storm regression for the one-liner: `applySnapshot` has exactly one caller (the attach-settle path; CBM in-degree 1), attach settles are discrete UI events, never per-resize-tick, so the key clears once per settle and the dedup operates unchanged within an attached interval. Worst added cost is one same-size `pty_resize` per attach settle, which `resize_instance` answers with `sent=false` (local_backend.rs:1991-1997) — no follow (2013-2015), no broadcast, no WARN — plus one `reportSpawnSizeDrift` call on that send. The failure rollback restores `null`, and the retry path is unaffected. Ratify specifically the TOP-OF-FUNCTION placement: the narrow in-branch variant leaves the healthy seeded cross-grid re-attach deduped (dev E2) and leaves the G6 race unhealed, and either alone makes 9.3 fail on a healthy repro. R1 is also what turns the G6 residual self-healing.

### G4 (REQUIRED): R2 confirmed — hoist, but keep the two None exits distinct

Confirmed at the current bodies: `resize_screen_and_broadcast` computes a `parser_fault` bool inside the lock block and runs `stage=parser_fault` + `attachments.flush(id, false)` only after it (1296-1339), and `activate_terminal_output` holds the `parsers` guard until `drop(parsers)` at 1211, so plan-5.3-as-written would emit to webviews under the parser lock, which every existing fault site avoids. Ratify R2 with one precision the E3 text leaves implicit: the mismatch arm has TWO `None` exits — heal-succeeded (no fault: no log, no flush) and heal-panicked (fault: `stage=parser_fault` + flush) — and the hoisted bool must distinguish them, mirroring 5.2's shape; the hoisted pair runs after the existing `attach` (1210) / `drop(parsers)` (1211) tail and before `Ok(snapshot)`.

### G5 (NOTE): on the container path `conpty_size` records a promise, not a fact — fix the 5.2 doc comment

`ContainerTransportBackend::resize` (container_backend.rs:3182-3203) calls `resize_screen_and_broadcast` unconditionally at 3201 after merely QUEUING the resize frame to the bridge (`send_text_frame(...)?`, 3183-3190); there is no `sent`-equivalent ack that the remote PTY applied it. For container sessions the new field therefore records "the size last requested of the remote", so 5.2's doc sentence ("the last grid the ConPTY actually took") and section 8's "inherit it uniformly" overstate: parser and record always move together there, and the reconcile is structurally blind to remote-apply divergence (bridge death mid-frame, paused container). Not a regression — the identical blindness ships today, and the local path's `if sent` gate keeps the record honest where the incident lives. Remedy, doc-only: one clause in the 5.2 field comment naming the transport-backend semantics, and one residual sentence in section 7. No behavior change, no rename.

### G6 (NOTE): the reconcile has a false-negative window between `resize_instance` and the follow — document it as a residual

`LocalProcessBackend::resize` computes `sent` under the `ptys` guard and calls the follow only after dropping it (1991-1997 vs 2013-2015; `open_startup_gate` documents the outside-the-guard rule, 1694-1711). An attach landing in that window sees parser == record == old grid while the ConPTY already took the new one: the reconcile passes and seeds the old grid. The seed is internally consistent at the old grid; live bytes then garble until the R1-enabled viewport sync re-imposes the attacher's grid, so the residual is transient and self-healing — but only with R1 landed. Closing it would require holding `ptys` across the follow, which the in-code comment rejects for lock-queueing reasons; out of scope. Remedy: one sentence in section 7 residuals, so a garble report WITHOUT a `stage=attach_grid_mismatch` WARN during 9.3 triage is attributed to this race instead of read as the fix failing.

### G7 (NOTE): the 5.4 replacement body re-ships a production `.lock().unwrap()`

The quoted `pty_resize` replacement keeps `pty_mgr.lock().unwrap()` on the PtyManager mutex. It is the pre-existing pattern in `commands/pty.rs`, and un-poisoning that mutex is out of scope; Step 7 should record it as retained-pre-existing (a poisoned PtyManager mutex panics the command thread today and after this plan alike) rather than let the rewrite silently re-introduce it as new code. No change demanded.

### G8 (NOTE): attack log — surfaces probed and found clean, settling dev's confirm-at-edit items

- Dev N3 is SETTLED now, not at edit time: `shouldDropAlreadyAppliedEvent` (441-447) admits every event when `entry.lastAppliedSequence` is null and admits monotonic-newer sequenced events over a stale watermark; the watermark is written only by live writes (454-457) and the seeded rebuild (564). A `None` attach can neither freeze the terminal nor duplicate content: retained events were already written live on arrival and are re-applied only after a reset, which the `None` path never performs.
- No retention leak on the `None` path: `snapshotReplayPending` is set at attach initiation (618), consumed into `settle.reconcilable` (525) and cleared unconditionally (529) BEFORE `applySnapshot` runs, with a watchdog guard at 625; the early return at 582-587 cannot leave it set.
- "Record-first lies" has no live local path: a pre-registration resize dies at `ptys.get(&id).ok_or(SessionNotFound)` before touching ConPTY or follow; a HELD size never moves the ConPTY and `open_startup_gate` follows only an APPLIED size; a DEDUPED size is already the parser's size (#973 comment); `no_parser_entry` is realistically reachable only during teardown. Each of the three skip classes also independently degrades the attach itself (poisoned lock: attach returns `Ok(None)`, output.rs:1155-1161; no entry: `Err(SessionUnavailable)`, 1162-1164; unavailable: the existing `None` arm), so none of them can yield a garbled seed even before the reconcile exists.
- "Reconcile refuses forever" has no path: the mismatch arm converges the grid, so the next attach seeds unless a fresh divergence lands in between; the only permanent-`None` states are the pre-existing `Unavailable`/poisoned semantics, which this plan does not change (no path restores `Available` after a fault; both fault writes and the registration write are the only `parser_availability` writes in the bodies read).
- Resize storms: every follow records last-writer-wins under the same `screen_parsers` mutex the reconcile reads, and attach serializes on it too; no interleaving yields a stale record with a fresh parser or vice versa.
- The 5.4 genericization risk has a shipping precedent: the `activate_terminal_output` command is already generic-with-webview and macro-registered (in-degree 0: macro-invoked only), exactly as 5.4 claims.
- One more parser consumer exists outside the reconcile's protection: `get_screen_snapshot` (#955; named by the #973 comment block inside `LocalProcessBackend::resize`) reads `contents_formatted()` off the parser grid. The plan neither fixes nor regresses it, and attach-time convergence incidentally repairs its grid at the next re-attach; recorded so nobody expects the reconcile to guard watcher-style reads.
- The pre-existing silent render-panic arm dev flagged as N1 is confirmed at output.rs:1202-1205 (`Err(_)` sets `Unavailable`, returns `None`, no `stage=parser_fault`, no flush-at-transition). It is the one silent failure path this plan knowingly leaves silent; it predates the plan and deserves a follow-up issue, not in-scope growth.

### Scope audit (Step 6)

The plan plus R1 (one line), R2 (a hoist of lines the plan already adds), N2 (one comment sentence), G1 (one constraint sentence inside test 9.1), G2 (test-internal mechanics inside 9.2), and G5/G6 (one documentation sentence each) still totals exactly two tests, zero new dependencies, zero new module arcs, zero IPC shape changes. Nothing else may grow.

## Step 7 consensus resolution (round 1)

Author: architect, wg-8, 2026-08-19 UTC, design authority for this plan. Every resolution below is incorporated into the plan sections named in the table; the two enrichment sections above stay untouched as the historical record. The certified plan is sections 1-10 as amended; the enrichment sections bind only through the resolutions here.

### Authority ritual, re-run at certification

`git fetch origin main` (verified against `FETCH_HEAD`): `origin/main` advanced from the frozen `b19ee1858cd6bf929abb6ae59f01239da20de498` to `b1eefa7c0e076d79d7ea38d76f998d1c05fd5055` (3 commits). Local committed branch head: `b19ee185`, unchanged; tracked tree clean; merge-base(branch, origin/main) = `b19ee185`, and the branch head is an ancestor of the new tip. Current-base review mandated by section 1: `git diff --name-status b19ee185 b1eefa7c` touches exactly one file, `docs/glossary.md`, which is none of this plan's surfaces (section 6) and no file cited in section 3. Every line anchor and behavior claim therefore remains valid at the frozen SHA, and the branch merges onto the new tip with zero overlap. Certification proceeds on the frozen authority; re-run the ritual again at delivery per section 1.

### Resolution table

| Item | Verdict | Landed in |
|---|---|---|
| R1 (Step 5, REQUIRED): invalidate `entry.lastSentViewport` at attach settle | RATIFIED-AMENDED: top-of-`applySnapshot` placement fixed per G3; the in-branch variant is rejected because it leaves the healthy seeded cross-grid re-attach deduped and the G6 race unhealed, failing 9.3 on a healthy repro | 5.5, 6, 7, 10, intro |
| R2 (Step 5, REQUIRED): hoist the 5.3 fault log+flush out of the `screen_parsers` lock | RATIFIED-AMENDED per G4: the `reconcile_fault` bool keeps heal-succeeded (no log, no flush) and heal-panicked (hoisted pair) distinct; the pair runs after `attach`/`drop(parsers)`, before `Ok(snapshot)` | 5.3 |
| G1 (Step 6, REQUIRED): 9.1 grid constraints | RATIFIED-AS-IS: R0/A/B pairwise distinct, each asymmetric, no pair a transposition | 9.1 |
| G2 (Step 6, REQUIRED): 9.2 dedup-key priming mechanics | RATIFIED-AS-IS: fit dims differ from `spawnViewport`; first-attach double-rAF flush with an exactly-one-`pty_resize` priming assert; invoke-log segmentation; re-attach rAF flush before the live write | 9.2 |
| G3 (Step 6, REQUIRED): R1 placement | RATIFIED-AS-IS, folded into R1's resolution | 5.5 |
| G4 (Step 6, REQUIRED): distinct None exits in R2 | RATIFIED-AS-IS, folded into R2's resolution | 5.3 |
| G5 (Step 6, NOTE): container path records request-not-taken | RATIFIED-AS-IS, doc-only: field-comment clause plus section 7 and 8 residual sentences | 5.2, 7, 8 |
| G6 (Step 6, NOTE): reconcile false-negative TOCTOU window | RATIFIED-AS-IS, doc-only: section 7 residual plus the 9.3 triage rule | 7, 9.3 |
| G7 (Step 6, NOTE): retained `pty_mgr.lock().unwrap()` in 5.4 | RATIFIED-AS-IS: recorded as retained-pre-existing and explicitly accepted | 5.4 |
| G8 (Step 6, NOTE) + N1 (Step 5): pre-existing silent render-panic arm (`output.rs:1202-1205`) | RATIFIED-AS-IS: out-of-scope follow-up material; see the record below | below |
| N2 (Step 5): false comment clause in `applySnapshot` | RATIFIED: amended in the same diff, doc-only | 5.5 |
| N3 (Step 5): sequenced-event admission on a watermark-less `None` attach | RESOLVED by Step 6's attack log: `shouldDropAlreadyAppliedEvent` admits null-watermark and monotonic-newer events; no confirm-at-edit item remains | none needed |
| N4 (Step 5): INFO covers the desktop command path only | RATIFIED: recorded as an accepted limitation | 5.4 |
| E7 (Step 5): ownership flag for the TypeScript half | RATIFIED: steps 1-3 dev-rust, step 4 dev-webpage-ui, Rust half first, 9.3 after both | 10, 9.3 |

Fixed-decision integrity: no enrichment item reopens a fixed decision. The 5.1 rejection of relocated or deferred `set_size` was ACCEPTED by Step 5 on re-verified evidence; the mismatch arm still returns `None` even when the heal succeeds (R2/G4 move where the fault pair runs, not the None contract); the IPC surface stays untouched (R1 clears a frontend-local key, and the extra same-size `pty_resize` is an existing command with an unchanged payload, answered by `resize_instance` with `sent = false`).

### G8 follow-up record (out of scope, for the tech-lead at delivery)

`activate_terminal_output`'s render-panic arm (`output.rs:1202-1205`) sets `Unavailable` and returns `None` without `stage=parser_fault` and without the flush-at-transition, unlike both canonical fault sites; after this plan lands it remains the one silent failure path, adjacent to the new logged reconcile arm. Pre-existing, confirmed independently by Step 5 (N1) and Step 6 (G8). Follow-up-issue material at delivery time, not in-scope growth here.

### Dependency-cycle gate (verify-no-dependency-cycles, executed)

Arcs enumerated against the actual bodies at the frozen SHA, not the plan text: this plan adds ZERO new module-to-module references and removes none. `output.rs` additions reference only `log` macros, `crate::logging::catch_payload_unwind` (already invoked in the same function today), `ParserAvailability`, `self.attachments`, and one new primitive field; the seam and the Rust test live in the same file. `commands/pty.rs` gains a `tauri::Webview<R>` parameter in the file that already imports tauri (line 4) and already ships two generic-with-webview commands (526-528, 570). `TerminalView.tsx` writes `entry.lastSentViewport` and calls `scheduleViewportSync`, both already in the same component scope. Per-arc classification: no new arcs exist to classify, so nothing can create an SCC, grow an SCC, or cross an SCC boundary. Measurement: a pre/post detector run is vacuous at plan stage because no implementation tree exists (pre == post == `b19ee185`); this certification therefore uses the skill's explicit per-arc manual analysis with that limitation stated, and section 8 binds the implementation review to run `rust-levelization-run` pre/post with the full five-point criterion. Role/layering hygiene: no lower layer gains an `AppHandle`/`tauri` dependency; the only tauri-touching edit is in `commands/`, the transport layer that already owns it; `pty/output.rs` gains no external dependency. GATE RESULT: PASS.

### Verdict

READY_FOR_IMPLEMENTATION (round 1). All four REQUIRED items and all four NOTEs are resolved and incorporated above; the Plan Contract holds: no TBD, no open decision, no competing alternative (the R1 placement alternative is closed in 5.5). The Plan-SHA256 of the certified file bytes is recorded in the Step 7 reply message to the tech-lead, never inside the file it hashes.

## Step 7 consensus resolution (round 2): the #973 suite collision

Author: architect, wg-8, 2026-08-19 UTC, on the tech-lead's amendment dispatch (20260819-190800) after the Step-8 stop. The round-1 certification (digest `43054DC7...DFA43`) was INVALIDATED by the Step-4 blocker; this round amends and re-certifies. The round-1 sections above stay untouched as the historical record.

### W1 (Step 8 blocker, dev-webpage-ui 20260819-190311): verified and accepted as a plan insufficiency

The finding: R1 alone, by isolation proof, reddens the four invoke-count cases of `TerminalView.spawn-size.test.tsx` (313, 364, 481, 528), while the certified plan simultaneously mandated R1 at top-of-`applySnapshot`, section 6 "No other file changes", and 9.2 "the full frontend suite stays green": three clauses unsatisfiable together. No enrichment step had read or run that suite; the production-layer analysis (the settle send is same-size and `resize_instance` answers `sent = false`) was and remains unchallenged. Round-2 verification, independent of the blocker report: fresh CBM gate ready first attempt at `71cd694f` (same project), 11 graph operations, 1 bounded fallback (the two retry constants, unindexed), plus a full direct read of the suite file and of the preserved step-4 patch; every mechanism claim in the blocker reproduced against the actual bodies (`sendPtyResize` dedup and rollback, `scheduleResizeRetry` coalescing and give-up at 3 attempts, `reportSpawnSizeDrift` once-per-entry latch, the suite's `data: []` fake and deterministic-rAF harness), and the observed counts (1, 2, 2, 6) are exactly what those mechanics predict. W1 also surfaced the retry-budget accounting consequence, resolved below.

### Decision: (a). R1 stands; the four #973 cases are re-pinned

Alternative (b), re-deciding R1, has no winning variant: dropping R1 reopens the incident itself (the healing resize dedups away and 9.3 fails on a healthy repro, per E2/G3); the narrow in-branch variant was already rejected in round 1 for failing 9.3 on the healthy seeded cross-grid re-attach and leaving the G6 race unhealed, and it would not even dodge this collision, because the suite's attaches all settle through the no-snapshot branch (the fake's snapshots carry `data: []`), where the in-branch line fires too; and the new field evidence (issue comment 5346328441: corruption reproduced by a plain deselect -> re-select with no detach) demands the heal on every attach settle, which only the top-of-function placement provides. What actually broke was the test contract: #973's acceptance was pinned as frontend send-suppression, and R1 deliberately moves the settle-send suppression to the backend dedup. The transfer is ratified in 5.5 (round-2 paragraph), the four cases are re-pinned with exact expectations in 9.2a, and no production behavior changes relative to the round-1 certification.

### Resolution table (round 2)

| Item | Verdict | Landed in |
|---|---|---|
| W1 main: R1 vs the #973 invoke-count pins | RATIFIED as re-pin: guarantee-transfer paragraph + four exact case specs; R1 and its placement unchanged; "full suite green" kept binding | 5.5, 6, 9, 9.2a, 10, intro, 1, 2, 4 |
| W1 retry-budget accounting note | ACCEPTED as documented residual, pinned by T4's new bound (settle burst spends the budget; first attempts never gated; reset only on success) | 7, 9.2a |
| Field evidence: second incident, no detach involved | INCORPORATED: reinforces every-attach placement; adds the cheaper 9.3 repro choreography | 5.5, 9.3 |

### Three-clause consistency restored

(i) R1 stays at top-of-`applySnapshot` (5.5, unchanged). (ii) Section 6 now lists `TerminalView.spawn-size.test.tsx` as an amended surface, so "No other file changes" holds again over the widened, still-exhaustive list. (iii) 9.2's "the full frontend suite stays green" stays binding and is again satisfiable because 9.2a re-pins the four cases to the true post-R1 contract. No TBD, no open decision, no competing alternative.

### Dependency-cycle gate (verify-no-dependency-cycles, round 2)

The round-2 amendment consists of expectation and comment edits inside one existing frontend test file plus plan text: it adds and removes ZERO module-to-module references, so no SCC can appear, grow, or merge and no cross-boundary arc can be created; role/layering hygiene is untouched (no lower layer gains an `AppHandle`/`tauri` dependency; the edited file is a test file that already imports its own harness). The round-1 per-arc manual analysis and section 8's binding on the implementation review (`rust-levelization-run` pre/post with the five-point criterion) stand unchanged. GATE RESULT: PASS.

### Verdict (round 2)

READY_FOR_IMPLEMENTATION (round 2). W1 and its accounting note are resolved and incorporated; the Plan Contract holds across the amended sections. The round-2 Plan-SHA256 of the certified file bytes is recorded in the round-2 reply message to the tech-lead, never inside the file it hashes; the round-1 digest is superseded.
