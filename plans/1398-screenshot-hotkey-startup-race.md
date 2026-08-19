# Plan #1398: Register the screenshot global hotkey at the top of setup instead of the post-restore tail

Author: architect, wg-14. Authored and certified in a single Lite pass on 2026-08-17 UTC.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1398](https://github.com/mblua/AgentsCommander/issues/1398), `fix: screenshot hotkey dead during startup restore and titlebar chip never appears (regression from a308271c / #1341)`.

This is a Lite regression fix. It relocates one existing statement block inside `src-tauri/src/lib.rs`, hardens one existing SolidJS component, and adds one test case. It introduces no new production abstraction, no new crate, no new npm dependency, no new module, no new schema, no new Tauri command, no new IPC surface, no new event, no new configuration key and no migration. It adds zero module-to-module dependency arcs.

## 1. Frozen authority and fail-closed entry gate

The implementation working tree is `repo-AgentsCommander`, branch `fix/1398-screenshot-hotkey-startup-race`, targeting `main`.

After `git fetch origin main` on 2026-08-17 UTC, all of the following resolved exactly to `8f272a76b53a15c3c442568ca169e1bb9b7d24cc`:

- committed `HEAD`;
- `origin/main`; and
- `git merge-base HEAD origin/main`.

The index and the non-ignored working tree were clean, verified by an empty `git status --porcelain=v1 --untracked-files=all`.

Root `.gitignore` line 11 ignores `/plans/`, so the implementation must force-add this exact plan file with `git add -f plans/1398-screenshot-hotkey-startup-race.md`. Do not remove or weaken the repository's `plans/` ignore rule.

Immediately before implementation, fetch `origin/main` again. Stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. Do not rebase, merge a moved base, or silently substitute a newer commit under this certification.

Branch-name validation was checked against `scripts/validate-branch-name.mjs` line 15. `fix/1398-screenshot-hotkey-startup-race` matches the allowed pattern with type `fix`, number `1398`, and slug `screenshot-hotkey-startup-race` (29 characters, under the 50-character cap), so the required `validate-branch-name` check will pass.

Every line number in this plan refers to the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

## 2. Objective and non-goals

Objective: make the configured screenshot global hotkey live from the very beginning of `.setup(...)` instead of after the whole startup restore plus every startup service, and make the titlebar chip stop depending on winning that race.

Non-goals, binding on the implementer:

- Do not reintroduce a main-thread `tauri::async_runtime::block_on` for the settings read. `src-tauri/src/lib.rs:1034-1040` declares the spawned-task launch as the only sanctioned way to start the startup restore precisely because a main-thread `block_on` starves WebView2 while a session open awaits the #1327 update gate. This plan never touches `spawn_restore_startup`'s launch shape.
- Do not change `register_configured_hotkey`, `parse_screenshot_hotkey`, `begin_capture_from_hotkey`, or anything else under `src-tauri/src/screenshot/`.
- Do not change `src/shared/ipc.ts`, `src/shared/types.ts`, or any Tauri command. See section 7.
- Do not add a backend event, a push notification, or a hotkey-registration broadcast. The frontend hardening is a bounded re-read of the existing `screenshot_get_hotkey_status` command.
- Do not reorder, rename, or restructure any other statement in `.setup(...)` or in the post-restore tail.
- Do not add tests beyond the single frontend case in section 6.

## 3. Verified current state

Verified by direct read at the frozen SHA:

- `src-tauri/src/lib.rs:1797-1807` holds the hotkey registration as the last statement of the post-restore tail inside `spawn_restore_startup`. The tail begins at `lib.rs:1749` and runs only after the restore body completes; `[restore] complete` is logged at `lib.rs:1700-1701`, before `completion.complete()` at `lib.rs:1744` and therefore before the tail. Today the hotkey is registered strictly after the `[restore] complete` line reaches the log.
- `.setup(move |app| {` begins at `lib.rs:2110`. Everything `register_configured_hotkey` needs is already in place before that line: the `tauri_plugin_global_shortcut` plugin at `lib.rs:2062-2073`, `.manage(settings)` (type `SettingsState`, built at `lib.rs:1991`) at `lib.rs:2084`, and `.manage(screenshot_hotkey_state)` at `lib.rs:2107`.
- `lib.rs:45` already has `use tauri::{Emitter, Manager};` and `lib.rs:32` already has `use config::settings::SettingsState;`, so the relocated block needs no new import.
- `lib.rs:2183-2190` is the existing precedent for this exact shape: a short braced block inside `.setup(...)` that clones the app handle and calls `tauri::async_runtime::spawn` for startup work that must not block the main thread (the #1327 agent-update task).
- `register_configured_hotkey` is declared at `src-tauri/src/screenshot/windows.rs:134` and at `src-tauri/src/screenshot/unsupported.rs:31`. Both take `(&AppHandle, &str)` and are synchronous. The only asynchronous part of the current call site is the `SettingsState` read at `lib.rs:1798-1802`.
- `ScreenshotHotkeyRuntime` derives `Default` at `windows.rs:95-99`, and `ScreenshotHotkeyStatus::default()` in `src-tauri/src/screenshot/mod.rs` yields `configured: "Ctrl+Q"`, `registered: false`, `error: None`. That triple is the pre-registration state the frontend currently reads and treats as final.
- `ScreenshotHotkeyStatusChip` at `src/sidebar/components/Titlebar.tsx:37-82` reads the status exactly once in `onMount` and never retries.
- `src/shared/ipc.ts:656-657` already exposes `ScreenshotAPI.getHotkeyStatus()` returning `ScreenshotHotkeyStatus`.

## 4. Change 1 (backend): relocate the registration

### 4.1 Delete the tail block

In `src-tauri/src/lib.rs`, delete the blank line 1796 and lines 1797-1807, that is exactly this text and nothing else:

```rust
            let screenshot_hotkey = app
                .state::<SettingsState>()
                .read()
                .await
                .screenshot_capture_hotkey
                .clone();
            if let Err(error) =
                crate::screenshot::register_configured_hotkey(app.app_handle(), &screenshot_hotkey)
            {
                log::warn!("[screenshot] global hotkey registration failed: {}", error);
            }
```

After the deletion the tail's last statement is `ui_automation_state.start(app.app_handle().clone(), shutdown.clone());`, immediately followed by the closing `});` of the `tail` async block.

Consequences the implementer must expect and must not "fix":

- The `tail` async block then contains no `.await` at all. That is legal Rust; an `async` block without an await point compiles without a warning, and `clippy::unused_async` applies to `async fn` items, not to blocks. Do not convert the block to a synchronous closure, and do not remove `AssertUnwindSafe` / `catch_unwind`, which are load bearing for the documented panic behaviour at `lib.rs:1746-1748`.
- No parameter of `spawn_restore_startup` becomes unused. `app` is still used at `lib.rs:1771-1795`, and `SettingsState` is still referenced at `lib.rs:1773`.

### 4.2 Insert the new block at the top of setup

In `src-tauri/src/lib.rs`, immediately after `let _ = app_handle_lock.set(app.handle().clone());` (`lib.rs:2115`) and before the `crate::logging::spawn_error_emit_task(app.handle().clone());` call, insert exactly:

```rust
            // #1398 - registered here, at the top of setup, and NOT in the
            // post-restore tail where a308271c parked it by adjacency: the
            // registration depends only on the global-shortcut plugin plus the
            // `SettingsState` and `ScreenshotHotkeyState` managed above, so
            // waiting for the restore left the hotkey dead for the whole
            // restore window, which grows with the number of sessions. Its own
            // short task keeps the async settings read off the main thread; a
            // `block_on` here would reintroduce the #1341 WebView2 starvation.
            {
                let app_for_hotkey = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let configured = app_for_hotkey
                        .state::<SettingsState>()
                        .read()
                        .await
                        .screenshot_capture_hotkey
                        .clone();
                    match crate::screenshot::register_configured_hotkey(&app_for_hotkey, &configured)
                    {
                        Ok(()) => log::info!(
                            "[screenshot] global hotkey registered '{}'",
                            configured
                        ),
                        Err(error) => {
                            log::warn!("[screenshot] global hotkey registration failed: {}", error)
                        }
                    }
                });
            }
```

Fixed decisions, not open to the implementer:

- **Placement.** The block goes after the `app_handle_lock.set(...)` line so the global app handle is already published before any hotkey press can be serviced, and before every other statement of `.setup(...)`. Do not move it later "to be safe"; the whole defect is late registration.
- **Shape.** A dedicated `tauri::async_runtime::spawn` task, mirroring `lib.rs:2183-2190`. This is the only sanctioned way to perform the asynchronous `SettingsState` read from `.setup(...)` without blocking the main thread. Do not use `block_on`, `try_read`, a synchronous `config::settings::load_settings()` re-read, or a rendezvous with the restore task.
- **Off-thread safety.** `register_configured_hotkey` already runs off the main thread today (inside the `spawn_restore_startup` task) and from async command handlers at `src-tauri/src/commands/config.rs:563`, `commands/config.rs:586` and `commands/screenshot.rs:67`. Moving the call to a different spawned task changes nothing about that.
- **Log lines.** The failure message keeps its exact current text, `"[screenshot] global hotkey registration failed: {}"`, so existing log scraping stays valid. The success `log::info!` is new and is required: it is the objective evidence for the primary acceptance criterion in section 9. Do not drop it, do not downgrade it to `debug`, and do not change the `[screenshot] global hotkey registered` prefix.
- **Non-Windows builds.** `crate::screenshot::register_configured_hotkey` resolves to the `unsupported` stub off Windows, which records `registered: false` with a non-null unsupported error and returns `Ok(())`. The new block therefore logs the success line on Linux and macOS too. That is the same behaviour the tail had; it is not a regression and must not be `cfg`-gated.

## 5. Change 2 (frontend): bounded re-read in the chip

In `src/sidebar/components/Titlebar.tsx`, add two module-level constants immediately above `const ScreenshotHotkeyStatusChip` (that is, after `formatScreenshotHotkeyForDisplay`, currently ending at line 35):

```tsx
// #1398 - the backend now registers the hotkey at the top of setup, so the
// first read almost always lands after registration. This bounded re-read is
// the hardening that keeps a slow boot from hiding the chip for the rest of
// the session; the ceiling exists so a permanently undecided runtime cannot
// poll forever.
const HOTKEY_STATUS_RETRY_DELAY_MS = 250;
const HOTKEY_STATUS_MAX_RETRIES = 20;
```

Then replace the whole `onMount` body of `ScreenshotHotkeyStatusChip` (`Titlebar.tsx:44-65`) with:

```tsx
  onMount(() => {
    let disposed = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;

    onCleanup(() => {
      disposed = true;
      if (retryTimer !== null) clearTimeout(retryTimer);
    });

    const attempt = (remaining: number) => {
      void ScreenshotAPI.getHotkeyStatus()
        .catch(() => null)
        .then((status) => {
          if (disposed) return;
          if (status !== null && status.registered === true) {
            // Registration is decided; the status never changes again this
            // session. The guard below is the current one-shot predicate,
            // unchanged except that `registered` is already known true here.
            if (
              status.error === null &&
              typeof status.configured === "string" &&
              formatScreenshotHotkeyForDisplay(status.configured) !== null
            ) {
              setCanonicalHotkey(status.configured);
            }
            return;
          }
          // `registered: false` with `error: null` is the pre-registration
          // default, the only genuinely transient state. A non-null error is a
          // decided refusal and must never be retried.
          if ((status === null || status.error === null) && remaining > 0) {
            retryTimer = setTimeout(
              () => attempt(remaining - 1),
              HOTKEY_STATUS_RETRY_DELAY_MS,
            );
          }
        });
    };

    attempt(HOTKEY_STATUS_MAX_RETRIES);
  });
```

Fixed decisions:

- The `.catch(() => null)` sits **before** `.then`, so a rejected request is retryable and still produces no `console.error`. The existing test `hides a rejected status request without a duplicate warning` (`Titlebar.screenshot-hotkey.test.tsx:187-194`) asserts exactly that absence.
- Retry only while the status is undecided. Termination conditions, all mandatory: `registered === true` (whatever the rest of the payload says), a non-null `error`, `disposed`, or the retry budget reaching zero.
- `HOTKEY_STATUS_MAX_RETRIES` times `HOTKEY_STATUS_RETRY_DELAY_MS` is a 5-second ceiling. Do not raise it, do not make it configurable, and do not replace the timer with an interval.
- `onCleanup` stays registered inside `onMount`, as today, and now also clears the pending timer. Losing that `clearTimeout` leaks a timer into every unmounted sidebar.
- Nothing else in `Titlebar.tsx` changes. The JSX, the `Show` gate, the class names and the `data-ac-testid` all stay byte-identical.

Behaviour against the five existing hidden-status cases (`Titlebar.screenshot-hotkey.test.tsx:175-185`), all of which must keep passing unchanged:

| Existing case | New path | Chip |
|---|---|---|
| `registered: false` | undecided, one retry scheduled, timers never advanced in that test | hidden |
| `error: "already registered"` with `registered: true` | terminal, error non-null | hidden |
| `configured: ""` with `registered: true` | terminal, formatting returns `null` | hidden |
| `configured: "Ctrl++1"` with `registered: true` | terminal, formatting returns `null` | hidden |
| `registered: "true"` (runtime-malformed) | not `=== true`, error null, one retry scheduled | hidden |

`uses the typed status route while preserving the complete native invoke allowlist` (`Titlebar.screenshot-hotkey.test.tsx:203-208`) also keeps passing: its status is `registered: true`, which terminates after exactly one call, so `toHaveBeenCalledTimes(1)` still holds.

## 6. Change 3 (test): the race case

In `src/sidebar/components/Titlebar.screenshot-hotkey.test.tsx`:

1. Add `vi.useRealTimers();` as the last statement of the existing `afterEach` (`Titlebar.screenshot-hotkey.test.tsx:123-127`), so a fake-timer test cannot leak into the rest of the file.
2. Add exactly one `it` block inside the existing `describe`, after the `uses the typed status route ...` case:

```tsx
  it("shows the chip when a later status reports the registration", async () => {
    vi.useFakeTimers();
    // The first read lands before the backend registration completes; the
    // mocked default supplies the registered status to every later read.
    mocks.getHotkeyStatus.mockResolvedValueOnce(screenshotStatus({ registered: false }));

    const { root } = await mountTitlebar();

    expect(getChip(root)).toBeNull();
    expect(mocks.getHotkeyStatus).toHaveBeenCalledTimes(1);

    // Comfortably past HOTKEY_STATUS_RETRY_DELAY_MS; the second read is
    // terminal, so no third read is ever scheduled.
    await vi.advanceTimersByTimeAsync(1_000);

    expect(mocks.getHotkeyStatus).toHaveBeenCalledTimes(2);
    expect(getChip(root)?.querySelector(".screenshot-hotkey-status-text")?.textContent)
      .toBe("Ctrl + 1");
  });
```

Notes that make this deterministic:

- `mockResolvedValueOnce` is queued before `mountTitlebar` runs. `mountTitlebar` calls `mockResolvedValue(status)` with the default registered status, and a `once` value always takes precedence over the standing value regardless of the call order, so read 1 is unregistered and read 2 onward is registered.
- Do not change `mountTitlebar`, `screenshotStatus`, `flushAsyncWork` or `MountOptions`. This case needs no helper change.
- Do not export `HOTKEY_STATUS_RETRY_DELAY_MS` from `Titlebar.tsx` just to use it here. The 1000 ms advance is intentionally slack.
- `vi.advanceTimersByTimeAsync` is already the repo's idiom for this (see `src/shared/stores/toasts.test.ts` and `src/shared/components/ToastHost.test.tsx`).

## 7. The two questions the dispatch left open, settled

**Does any Rust test pin the startup ordering?** No. `spawn_restore_startup` is referenced only at `src-tauri/src/lib.rs:1042`, `2795` and `2835`, all production code. No file under `src-tauri/tests/` mentions the screenshot hotkey (`tests/wake_consumption_measure.rs` matches only on the unrelated `/hotkeys` slash command injected into a coding-agent session). The `#[cfg(test)] mod tests` block at `lib.rs:3377` contains no startup-ordering test. **No Rust test file changes.**

**Does the frontend retry require touching `src/shared/ipc.ts`?** No. `ScreenshotAPI.getHotkeyStatus()` already exists at `src/shared/ipc.ts:656-657` with the right return type, and the retry only calls it more than once. `src/shared/types.ts` is likewise untouched: `ScreenshotHotkeyStatus` needs no new field. **No IPC change on either side of the boundary.**

**Corrected file set** (the dispatch's expectation was right and grows by nothing):

1. `src-tauri/src/lib.rs`
2. `src/sidebar/components/Titlebar.tsx`
3. `src/sidebar/components/Titlebar.screenshot-hotkey.test.tsx`
4. `plans/1398-screenshot-hotkey-startup-race.md` (this file, force-added)

Any other modified path is out of contract. If the implementer believes a fifth file is required, stop and report instead of widening the change.

## 8. Dependency-cycle gate

Applied per the `verify-no-dependency-cycles` criterion.

New module-to-module arcs introduced: **zero**. Removed arcs: **zero**.

- `agentscommander_lib -> agentscommander_lib::screenshot` already exists (`src-tauri/module-arcs.txt` line 29). The `register_configured_hotkey` call moves from `lib.rs:1804` to `lib.rs:2116`-ish; both sites are the same source module.
- `agentscommander_lib -> agentscommander_lib::config::settings` already exists (`src-tauri/module-arcs.txt` line 14). `SettingsState` is imported at `lib.rs:32` and is still used at `lib.rs:1773` after the deletion.
- The frontend adds no import at all. `Titlebar.tsx` already imports `ScreenshotAPI` from `../../shared/ipc`, and the test file already imports the same mock surface.

Therefore `cyclicSccs` is unchanged, every SCC member set is identical, there are zero arcs crossing a previously-clean SCC boundary, and the arc record must come out byte-identical. The objectively checkable form of that last claim is criterion C4 in section 9.

Role and layering hygiene: no lower-layer module gains an `AppHandle` or `tauri` dependency. The transport-taking call stays in `lib.rs`, the top transport layer, and `screenshot::register_configured_hotkey` already took `&AppHandle` before this change.

**Gate result: PASS.**

## 9. Acceptance criteria

Each criterion below is a pass/fail check with a stated procedure. C1 is the primary criterion from the issue.

**C1 (primary, ordering proof).** On a normal Windows boot of the built app, the application log contains a line matching `[screenshot] global hotkey registered` and that line appears **before** the line beginning `[restore] complete`.

This is discriminating, not incidental: at the frozen base the registration is the last statement of the post-restore tail, and `[restore] complete` is logged at `lib.rs:1700-1701`, before `completion.complete()` and therefore before the tail runs. So the base ordering is necessarily `[restore] complete` first, and the fixed ordering is necessarily the registration first. Verify with a single pass over the log, for example:

```powershell
Select-String -Path "$env:USERPROFILE\.agentscommander\app.log" `
  -Pattern '\[screenshot\] global hotkey registered|\[restore\] complete'
```

and read the two line numbers. Use the log path of the instance actually launched. Note that the running app holds the file open, so read it with `Select-String -Path` rather than loading it whole.

**C2 (primary, functional proof).** On an instance configured to restore at least three sessions, so the restore window is wide, launch the app and press the configured hotkey after the main window paints but before the `[restore] complete` line is written. The desktop freeze overlay appears (or, if the press cannot resolve a target session yet, a `screenshot_capture_failed` surface appears). Either outcome proves the hotkey is live. Silence proves it is not. Record the observed log timestamps of the press outcome and of `[restore] complete` to show the press fell inside the window.

The chip reappearing does **not** satisfy C1 or C2 and must not be offered in their place.

**C3 (secondary).** On a normal boot, the titlebar chip displays the configured shortcut, formatted with spaces around `+` (for example `Ctrl + 2` for a configured `Ctrl+2`).

**C4 (dependency-cycle gate).** `git diff --stat -- src-tauri/module-arcs.txt` is empty on the implementation branch. The arc record must not be regenerated, edited, or committed as part of this change; a non-empty diff means the change added or removed a module arc and contradicts section 8, so stop and report instead of committing the new record.

**C5 (regression suites).** All of the following pass from the repo root:

```
npm run typecheck
npm test
```

and from `src-tauri`:

```
cargo clippy --workspace --all-targets -- -D warnings
cargo test
```

`npm test` must include the new case from section 6 and all seven pre-existing cases in `Titlebar.screenshot-hotkey.test.tsx`, with no pre-existing assertion modified other than the `afterEach` addition. `cargo test` is expected to show no count change: this plan adds no Rust test.

Note for reading `cargo test` output on this platform: redirect to a file if panic detail or `--nocapture` output is needed, because the wrapper swallows stdout.

## 10. Implementation order

1. Apply section 4.1 (delete the tail block), then section 4.2 (insert the new block). Do these together; the intermediate state has no registration at all.
2. Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test` from `src-tauri`.
3. Apply section 5, then section 6.
4. Run `npm run typecheck` and `npm test`.
5. Check C4.
6. Build and run the app for C1, C2 and C3.
7. Commit the three source files plus this plan, force-adding the plan.

## 11. Risks and their bounds

- **A hotkey press lands before a session is selectable.** `begin_capture_from_hotkey` (`windows.rs:214-224`) already spawns its own task and routes every failure through `surface_hotkey_failure`, so an early press degrades to a visible error rather than a panic or a hang. This is the intended, in-scope consequence of making the hotkey live earlier, and C2 accepts it explicitly. Do not add a suppression window; that would rebuild the defect.
- **Two registrations racing.** The new task is the only startup registration path; the tail path is deleted, not duplicated. The settings-save paths in `commands/config.rs` cannot fire before the WebView loads. Even if they overlapped, `register_configured_hotkey` guards with `gs.is_registered(shortcut)` at `windows.rs:144-148`.
- **The settings read blocking.** `SettingsState` is a `tokio::sync::RwLock` built at `lib.rs:1991`; at the top of `.setup(...)` no writer holds it, and the read happens inside a spawned task regardless, so the main thread never waits.
- **Rollback.** Reverting the commit restores the frozen base exactly; there is no persisted state, no migration and no on-disk format involved.
