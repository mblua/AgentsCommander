# Plan #1536: Add "Edit TASK title" to the replica context menu in the sidebar

Author: ac-architect-v3, workgroup wg-21-ac-dev-team-v3. Full triage (cross-owner: `src-tauri/` + `src/` + tests + docs; new IPC command).

Status: READY_FOR_IMPLEMENTATION

Revision: round 3 (2026-08-24) — BLOCKER-2 invocation-token fix (§5.3.1/§5.3.2/§7.6/§10.14); §9.1 `USER:` readback correction; §9.2.1 coordinator-row order split; new §9.2.9 stale-continuation test; §5.1.1 full 256-cap comment retained verbatim. Round-2 digest BE13687C... superseded.

Issue: [mblua/AgentsCommander#1536](https://github.com/mblua/AgentsCommander/issues/1536), "Add 'Edit TASK title' to the replica context menu in the sidebar".

This is a Full cross-owner change: a new Tauri command plus IPC binding (ac-dev-rust-v3), the two sidebar context-menu items with an inline editor (ac-dev-webpage-ui-v3), tests on both sides, and a docs update. It introduces no new npm dependency, no new module, no new event, no new IPC payload shape, no configuration key, and no migration. The only new cross-boundary artifact is one Tauri command (`task_set_title_at`) that mirrors the existing `task_clean_at` (path-based, session-less) shape, and one new member on the existing `TaskAPI` object. It adds **zero module-to-module import arcs** (Rust or TypeScript); see section 11.

---

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `feature/1536-sidebar-edit-task-title`, targeting `main`.

At authoring time (2026-08-24 UTC) the committed `HEAD` of the branch is `e9365c277a8ff836068b05d56a6c3597f9d922de` (merge of PR #1534 from `main`), and `git status --porcelain` is empty. The Codebase Memory index used for the call-graph citations below reports the same `head_sha` (gate `ready`, project `D-0_repos-AgentsCommander_iac-.ac-wg-21-ac-dev-team-v3-repo-AgentsCommander`, 24441 nodes, 130784 edges).

Root `.gitignore` line 11 ignores `/plans/`, so the implementation must force-add this exact plan file with `git add -f plans/1536-sidebar-edit-task-title.md`. Do not remove or weaken the `plans/` ignore rule.

The implementers must repeat the authority ritual: fetch `origin/main`, and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

---

## 2. Issue and objective

Objective: let the user edit the `TASK` title of a workgroup replica from the sidebar, in **both** replica context menus — the active/live menu (right-click on a replica row with a session) and the inactive/no-session (gray) menu — positioned **between `Add to Group` and `Clear task title` (directly above the broom)** in both, using the **same pencil icon (`&#x270E;`) and tooltip (`Edit TASK title`) as the terminal's title-edit button**, with an inline editor inside the open context menu that mirrors the terminal editor (prefilled input, Save/Cancel, Enter saves / Escape cancels, busy and error states).

Required outcomes:

- **(A)** Both context menus show the `Edit TASK title` item between `Add to Group` and `Clear task title`.
- **(B)** Clicking it opens an inline editor inside the still-open context menu, input prefilled with the current title, focus + select on the input.
- **(C)** Save writes the title: via `TaskAPI.setTitle(sessionId, title)` when a **live** session exists for the workgroup (editor-specific resolution `resolveLiveWorkgroupSessionId` — the first live non-`inactive-` session, so dropped/exited sessions fall through to the path-based command), and via the new path-based `task_set_title_at(workgroupRoot, title)` Tauri command when there is no live session (cold/gray workgroup) — mirroring how the broom falls back to `task_clean_at` (#545 precedent).
- **(D)** After a successful save the menu closes (broom behavior), and the sidebar row refreshes through the existing `workgroup_task_updated` event (store `updateWorkgroupTask`).
- **(E)** Failure keeps the menu open, shows the error inline in the menu, and logs `Failed to edit task title:` to the console (consistent with the broom's `Failed to clear task title:`).
- **(F)** Backend behavior matches the existing `task_set_title` contract: same validations (non-empty trimmed, no control characters except tab, ≤ 256 chars), same `TaskUpdateResult` shape, same `workgroup_task_updated` emission (source `"manual"`), same file-creation semantics for a missing `TASK.md` (as `task_clean_at` has).
- **(G)** All existing behavior stays correct: broom position/disabled states, Add to Group flyout, menu dismissal (outside click, Escape, hover-leave), group-flyout pinning, repo flyouts.

Non-goals, binding on the implementers:

- No changes to the terminal `WorkgroupTask` editor, its CSS, or its logic.
- No new `task_get_title_at` command: cold-workgroup prefill comes from the already-present `menu().wg.taskTitle` (the same value the broom's disabled state uses today).
- No CLI changes (`src-tauri/src/cli/task_set_title.rs` and `src-tauri/src/cli/task_ops.rs` are untouched).
- No changes to the `workgroup_task_updated` event shape, to `TaskUpdateResult`, or to the poll path.
- No new npm dependencies, no new modules, no new configuration keys, no migration, no changes to `dependency-cruiser` config.

---

## 3. Evidence (measured at e9365c2, not predicted)

### 3.1 Terminal precedent (the icon/tooltip source)

`src/terminal/components/WorkgroupTask.tsx`:

- Line 239: `<button class="workgroup-task-action" onClick={startEditing} disabled={editDisabled()} title="Edit TASK title" type="button">&#x270E;</button>`.
- `startEditing` (96-129): guard, capture `sessionId`, set busy, prefill from `parseTaskTitle(terminalStore.activeWorkgroupTask) ?? ""`, refresh via `TaskAPI.getTitle(id)` (use only if non-null), abort on error or session change, then open editor (`titleDraft`, `editing`).
- `saveTitle` (138-166): guard captured session, reject empty trimmed title ("Title cannot be empty."), `TaskAPI.setTitle(id, title)` → `terminalStore.applyLocalTask(result.workgroupRoot, result.task)`, close editor; `catch` → `setError(String(err))`.
- `handleKeyDown`: Enter → `saveTitle()`, Escape → `cancelEditing()`.
- Editor JSX (~250-280): `div.workgroup-task-title-edit` > `input.workgroup-task-title-input` (ref focus+select, `value`, `onInput`, `onKeyDown`, `placeholder="Title"`, `disabled={busy()}`) + `button.workgroup-task-title-btn.save` (`disabled={busy() || !titleDraft().trim()}`) + `button.workgroup-task-title-btn.cancel` (`disabled={busy()}`). Error line: `div.workgroup-task-error`.

### 3.2 Sidebar broom (placement anchor and handler)

`src/sidebar/components/ProjectPanel.tsx`:

- Active menu branch (`<Show when={activeReplicaMenu()}>`, starts ~3390): ends with `{renderAddToGroupItem(menu().wg, menu().replica)}` → `<div class="context-separator" />` → broom `<button class="session-context-option" classList={{ "context-option-disabled": broomDisabled() }} disabled={broomDisabled()} title={broomTitle()} onClick={() => void clearReplicaTaskTitle(menu().wg)}>` with `&#x1F9F9;` icon (broom ends ~3456).
- Inactive menu branch (`<Show when={inactiveReplicaMenu()}>`, 3458-3508): same tail but **no** `context-separator` between `renderAddToGroupItem(...)` and the broom.
- `broomDisabled = () => isTaskClean(menu().wg.taskTitle)`; `broomTitle` = "Nothing to clear" / "Clear task title"; both defined per-branch.
- `renderAddToGroupItem` (1507-1534): separator + `Add to Group` submenu trigger (coordinator only); its click handlers call `e.preventDefault(); e.stopPropagation();` — the per-element guard pattern the editor repeats (see 3.3.2).
- `clearReplicaTaskTitle` (1674-1687): `setReplicaCtxMenu(null); cleanupCtx();` then `const sessionId = resolveWorkgroupSessionId(wg);` → `TaskAPI.clean(sessionId)` if present, else `TaskAPI.cleanAt(wg.path)`; `catch (e) { console.error("Failed to clear task title:", e); }`.
- `resolveWorkgroupSessionId` (1315-1321): first peer session whose id does **not** start with `inactive-`.
- `activeReplicaMenu` (1306-1309) / `inactiveReplicaMenu` (1310-1313): memos on `replicaCtxMenu().kind`; menu state is `{ kind: "active", sessionId, sessionName, wg, replica, exited, x, y }` or `{ kind: "inactive", wg, replica, x, y }`.
- `closeReplicaCtxMenu` (1081-1084); `positionReplicaCtxMenu` (1096-1118, clamps via `getBoundingClientRect()` so re-clamping after a size change works); `reclampReplicaCtxMenu` (1120-1129); `scheduleReplicaCtxMenuClose` (1086-1094, has a `groupErrorPinned()` suppression guard — the pattern to mirror); `cancelReplicaCtxMenuClose` (1063-1067).
- Menu container div: `class="session-context-menu"` at line 3372, `onMouseEnter={cancelReplicaCtxMenuClose}` (3376), `onMouseLeave` (3377-3380) → `scheduleReplicaCtxMenuClose(); // #977`.
- `isTaskClean(t)` (102-103): `!t?.trim() || t.trim() === "Clean"`.

### 3.3 The dismissal trap (must be engineered around)

Both menu-open handlers, `handleReplicaContextMenu` (1964-2002) and `handleReplicaInactiveContextMenu` (2004-2033), install the same `dismiss` listeners on `window` inside a `setTimeout`:

- `window` `click` → close the menu;
- `window` `contextmenu` → close the menu;
- `window` `keydown` → close the menu only when `ev.key === "Escape"`.

Measured guard that already exists: the menu container div (`class="session-context-menu"`, ~3372-3380) carries `onClick={(e) => e.stopPropagation()}` (~3374), so **clicks inside the menu never reach the window click listener** — an inside click does not, by itself, close the menu.

Consequences for this change (verified against the code):

1. **The input's `keydown` guard is strictly required.** `keydown` is not a click: the container's `onClick` does not cover it, and the window `keydown` dismiss fires on Escape. Without `stopPropagation` on the input's `onKeyDown`, Escape closes the whole menu instead of canceling the editor.
2. The per-element click guards (the `Edit TASK title` item, the editor container) are **correct belt-and-suspenders**: today the container's `onClick` already protects inside clicks, but the editor must not depend on that single container-level guard surviving a future refactor (the `Add to Group` trigger already follows the same per-element pattern). Keep all per-element guards; do not remove the container's `onClick`.
3. The menu also closes on hover-leave via the grace timer (`scheduleReplicaCtxMenuClose`, CONTEXT_MENU_CLOSE_GRACE_MS) — this must be suppressed while the editor is open, mirroring the existing `groupErrorPinned()` guard in `scheduleReplicaCtxMenuClose` (1086-1094).
4. **Replacement, not only dismissal**: both handlers replace `replicaCtxMenu()` **directly, never through `null`** (deliberate #943 behavior; `e.stopPropagation()` at 1966/2006 keeps the window `contextmenu` dismiss from firing). A stale editor must not survive a right-click on another replica — see the BLOCKER-1 remediation in §5.3.1/§5.3.4 and the regression test in §9.2.8.

### 3.4 TaskAPI and types (already in scope of both implementers)

`src/shared/ipc.ts` TaskAPI object:

- `getTitle` (667-668): `transport.invoke<string | null>("task_get_title", { sessionId })`.
- `setTitle` (670-671): `transport.invoke<TaskUpdateResult>("task_set_title", { sessionId, title })`.
- `clean` (673-674): `transport.invoke<TaskUpdateResult>("task_clean", { sessionId })`.
- `cleanAt` (676-677): `transport.invoke<TaskUpdateResult>("task_clean_at", { workgroupRoot })`.

`src/shared/types.ts`: `TaskUpdateResult { workgroupRoot: string; task: string | null }` (line 1388); `WorkgroupTaskUpdatedEvent` (1393-1405) with `source: "manual" | "poll"`, `workgroupRoot`, `task`, `taskTitle`.

`src/sidebar/components/ProjectPanel.tsx` line 4 already imports `TaskAPI` — no import changes needed. `normalizePath` at line 236 is a **local** helper with a different signature `(path, separator)` — do not reuse it; compare raw `wg.path` strings instead (both sides of the comparison originate from the same store objects; `wg.path` is never rewritten by store updates, unlike `wg` itself — see 7.6).

### 3.5 Sidebar store already refreshes on task events

`src/sidebar/stores/project.ts` `updateWorkgroupTask(workgroupPath, task, taskTitle)` (501-524) normalizes `taskTitle ?? undefined` and updates `wg.task`/`wg.taskTitle`. Wired in `src/sidebar/App.tsx:794` (`projectStore.updateWorkgroupTask(wgPath, taskFirstLine(data.task), data.taskTitle)`) for both `manual` and `poll` event sources. So after save, the sidebar refreshes without any manual store write — the broom relies on this today, and the editor will too.

### 3.6 Backend precedents

`src-tauri/src/commands/task.rs`:

- `task_set_title` (198-248): inline validations — `title.trim().is_empty()` → `Err("title cannot be empty")`; any `c.is_control() && c != '\t'` → `Err("title must be a single line of printable characters (control characters other than tab are not allowed)")`; `title.chars().count() > 256` → `Err("title is too long (max 256 characters)")`; then `resolve_wg_root(&session_mgr, &session_id)` → `task_ops::perform(&wg_root, TaskOp::SetUserTitle(title))` → maps `EditOutcome::{Wrote, NoOp, RejectedUserTitle}` to `(content, task_title)` → `task = None` if `content.trim().is_empty()` else `Some(...)` → `TaskUpdateResult { workgroup_root: strip_unc(&wg_root), task }` → `emit_task_updated(&app, &wg_root, &task, &task_title)`.
- `task_clean_at` (290-322): `app: AppHandle, settings: State<'_, SettingsState>, workgroup_root: String`; `let project_paths = { settings.read().await.project_paths.clone() };` (drop the read guard before blocking IO); `validate_wg_root(&workgroup_root, &project_paths)?`; `task_ops::perform(&wg_root, TaskOp::Clean)`; `log::info!("[task] clean_at for {} -> {:?}", workgroup_root, outcome);`; same outcome mapping and `TaskUpdateResult`/`emit_task_updated` construction.
- `emit_task_updated` (157-172): `app.emit("workgroup_task_updated", json!({ "workgroupRoot": strip_unc(wg_root), "source": "manual", "task": ..., "taskTitle": ... }))`.
- Existing unit tests in the same file use `make_real_wg`, `paths_of`, `read_task_fields_at`, `validate_wg_root`, `perform`, `TaskOp::SetTitle` for seeding: `task_clean_at_creates_task_when_missing` (620-633, asserts `EditOutcome::Wrote { backup: None, .. }` and that the title reads back as `"Clean"`), `task_clean_at_round_trip_returns_clean_title` (521-533).

Registration: `src-tauri/src/lib.rs` lines 2910 (`commands::task::task_set_title,`) and 2912 (`commands::task::task_clean_at,`) in the `invoke_handler(tauri::generate_handler![...])` list.

`task_set_title`'s three validations are **inline** today. To keep the two session-based and path-based set-title commands from drifting, this plan extracts them into one private helper (`validate_user_title`) used by both — pure code motion, identical messages and order.

### 3.7 Docs and tests to update

- `docs/features/sidebar-guide.md`: the broom is described at lines 58-62; the new item gets a sibling bullet right there.
- `src/sidebar/components/ProjectPanel.context-menu.test.tsx`: existing broom coverage incl. disabled states (~1040-1090); helpers `replicaMenu()` (121-124, queries `.session-context-menu` — menus render through a Portal into `document.body`) and `findBroom(menu)` (126-132, matches button textContent containing `"Clear task title"`); fixtures `setupPanel([coordSession(), memberSession()], projectDiscovery(taskTitle, task))`, `contextMenu(findRow(rendered!.root, rowTestId))`; assertions via `fake.lastCall(cmd)` (FakeTransport, `src/shared/testing/fake-transport.ts`). FakeTransport **throws** on unhandled commands (`Unhandled fake transport invoke: ...`) and exposes `resolve(cmd, value)` / `reject(cmd, reason)` — tests that trigger `task_get_title` / `task_set_title` / `task_set_title_at` must register handlers for them.

### 3.8 Dependency-cycle baseline (measured on the clean tree at e9365c2)

- `node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet` where `<VAULT>` = `D:\0_repos\AgentsCommander_iac\.ac\wg-21-ac-dev-team-v3\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust`. Exit code 1 is the **normal** gating outcome (cycles exist); only exit 3 means no graph. Baseline summary: `modulesResolved: 191`, `moduleEdges: 3683`, `moduleCycles: 1` (a pre-existing 85-module SCC — `commands::task` is **not** a member; membership re-verified round 3 via the graph at e9365c2), `functionsResolved: 4252`.
- `node scripts/02-module-arc-record.mjs --graph pre.json --out arcs-regen.txt` regenerates the committed `src-tauri/module-arcs.txt`: **byte-identical** at baseline (1034 arcs, 82019 bytes, `cmp` clean).

---

## 4. Scope

### In scope

1. New Tauri command `task_set_title_at` in `src-tauri/src/commands/task.rs` (path-based mirror of `task_set_title`, session-less like `task_clean_at`) + registration in `src-tauri/src/lib.rs`.
2. Extraction of the shared title validator `validate_user_title` in `src-tauri/src/commands/task.rs`, used by both `task_set_title` and `task_set_title_at`.
3. New `TaskAPI.setTitleAt(workgroupRoot, title)` member in `src/shared/ipc.ts`.
4. `Edit TASK title` menu item in both replica context-menu branches of `src/sidebar/components/ProjectPanel.tsx` (between `Add to Group` and the broom) with pencil icon `&#x270E;` and `title="Edit TASK title"`.
5. Inline editor inside the open context menu (input prefilled, Save/Cancel, Enter/Escape, busy/error), in both branches, with new `session-context-title-*` CSS in `src/sidebar/styles/sidebar.css`.
6. New signals + handlers in `ProjectPanel.tsx` (`titleEdit`, `titleDraft`, `titleBusy`, `titleError`; `startReplicaTitleEdit`, `saveReplicaTitle`, `cancelReplicaTitleEdit`, reset effect).
7. Tests: backend unit tests in `commands/task.rs`; sidebar tests in `ProjectPanel.context-menu.test.tsx`.
8. Docs: `docs/features/sidebar-guide.md`.

### Out of scope

- Terminal editor (`src/terminal/components/WorkgroupTask.tsx`), terminal CSS, terminal store.
- CLI (`src-tauri/src/cli/task_set_title.rs`, `task_ops.rs`) and any CLI contract.
- `task_get_title_at` / session-less title reads (prefill uses `wg.taskTitle`).
- Event shapes, poll path, `TaskUpdateResult`, config, migrations, npm dependencies, dependency-cruiser config.
- Any change to broom behavior, disabled states, flyouts, or dismissal behavior outside the editor's suppression guard.

---

## 5. Decided solution

### 5.1 Backend — `src-tauri/src/commands/task.rs` (owner: ac-dev-rust-v3)

**5.1.1** Extract the shared validator. Take the three checks from `task_set_title` (198-216) verbatim into a private helper:

```rust
fn validate_user_title(title: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("title cannot be empty".to_string());
    }
    if title.chars().any(|c| c.is_control() && c != '\t') {
        return Err(
            "title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
                .to_string(),
        );
    }
    // /* full existing comment retained verbatim (see note below) */
    if title.chars().count() > 256 {
        return Err("title is too long (max 256 characters)".to_string());
    }
    Ok(())
}
```

`task_set_title` calls `validate_user_title(&title)?;` first (replacing its inline block, same position, same messages, same order). Place the helper near `task_set_title`. Keep the existing comments with the checks. **The FULL original 256-cap comment (the 5-line block above the count check in `task_set_title` today — task.rs:205-209, including the "1 MB pasted blob / panel layout / file ergonomics" and "256-emoji" rationale) is retained VERBATIM, byte-for-byte, exactly as it reads in the current source.** The sketch above truncates it only to keep the sketch compact, marked with `/* full existing comment retained verbatim */` — the implementer must copy the comment from `task_set_title` unchanged, never the truncated sketch text.

**5.1.2** New command, placed directly after `task_clean_at` (mirror its signature shape and its comment about cloning `project_paths` before blocking IO):

```rust
#[tauri::command]
pub async fn task_set_title_at(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    workgroup_root: String,
    title: String,
) -> Result<TaskUpdateResult, String> {
    validate_user_title(&title)?;
    // Clone the small Vec and drop the read guard immediately; perform() does
    // blocking IO and must not be held across the settings lock.
    let project_paths = { settings.read().await.project_paths.clone() };
    let wg_root = validate_wg_root(&workgroup_root, &project_paths)?;

    let outcome = task_ops::perform(&wg_root, TaskOp::SetUserTitle(title)).map_err(|e| e.to_string())?;
    log::info!("[task] set_title_at for {} -> {:?}", workgroup_root, outcome);

    let (content, task_title) = match &outcome {
        task_ops::EditOutcome::Wrote { content, title, .. } => (content.clone(), title.clone()),
        task_ops::EditOutcome::NoOp { content, title } => (content.clone(), title.clone()),
        task_ops::EditOutcome::RejectedUserTitle { content, title } => {
            (content.clone(), title.clone())
        }
    };
    let trimmed = content.trim();
    let task = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task, &task_title);
    Ok(result)
}
```

Notes, binding:

- `#[tauri::command]` must match the attribute style used by the sibling commands in this file.
- Argument names in Rust are `workgroup_root` and `title`; Tauri's default camelCase conversion yields the IPC arg names `workgroupRoot` and `title` (same as `task_clean_at`'s `workgroupRoot`).
- Validation runs **before** the settings lock / `validate_wg_root` (mirrors `task_set_title`, where validation precedes session resolution).
- `log::info!` line mirrors `task_clean_at`'s wording with `set_title_at`.
- Zero new imports: `AppHandle`, `State`, `SettingsState`, `validate_wg_root`, `task_ops`, `TaskOp`, `EditOutcome`, `TaskUpdateResult`, `strip_unc`, `emit_task_updated`, `log` are all already used by sibling functions in this file. **Do not add imports.**

**5.1.3** Register in `src-tauri/src/lib.rs`: add `commands::task::task_set_title_at,` adjacent to `commands::task::task_clean_at,` (line 2912) inside the same `invoke_handler(tauri::generate_handler![...])` list. No other change to `lib.rs`.

### 5.2 IPC — `src/shared/ipc.ts` (owner: ac-dev-rust-v3 or ac-dev-webpage-ui-v3, single commit)

Add to the `TaskAPI` object immediately after `cleanAt` (line 677):

```ts
setTitleAt: (workgroupRoot: string, title: string) =>
  transport.invoke<TaskUpdateResult>("task_set_title_at", { workgroupRoot, title }),
```

### 5.3 Frontend — `src/sidebar/components/ProjectPanel.tsx` (owner: ac-dev-webpage-ui-v3)

**5.3.1 New component-level state and reset helper**, declared next to the existing `replicaCtxMenu` state (~lines 1063-1094):

```ts
const [titleEdit, setTitleEdit] = createSignal<{ wgPath: string; sessionId: string | null } | null>(null);
const [titleDraft, setTitleDraft] = createSignal("");
const [titleBusy, setTitleBusy] = createSignal(false);
const [titleError, setTitleError] = createSignal<string | null>(null);
// Monotonic invocation epoch (BLOCKER-2 token, round 3): every
// startReplicaTitleEdit captures the current value after setting state;
// resetTitleEditState() bumps it, so menu close, menu replacement, and
// cancel invalidate every in-flight getTitle continuation SYNCHRONOUSLY
// (no microtask window). A stale continuation must never touch shared
// editor state that a newer editor owns — see 5.3.2 and 7.6.
let titleEditEpoch = 0;

const resetTitleEditState = () => {
  titleEditEpoch += 1;
  setTitleEdit(null);
  setTitleDraft("");
  setTitleBusy(false);
  setTitleError(null);
};
```

- `titleEdit` is the edit target: raw `wg.path` (string equality — see 7.6) and the resolved live-session id at click time, or `null` when not editing.

Reset wiring (three nets, deliberate — this is the BLOCKER-1 remediation):

1. **Reset effect** (fires on menu CLOSE — the null transition):

```ts
createEffect(() => {
  if (!replicaCtxMenu()) resetTitleEditState();
});
```

2. **Reset in both menu-open handlers** (covers the A→B REPLACEMENT path, which never passes through `null` — #943): call `resetTitleEditState();` in `handleReplicaContextMenu` and `handleReplicaInactiveContextMenu`, right next to the existing `resetGroupMenuState();` calls (ProjectPanel.tsx:1977 active / 2017 inactive). This makes opening any new replica menu unconditionally close any in-flight editor — and, via the epoch bump, invalidates that editor's in-flight `getTitle` continuation in the same tick (BLOCKER-2).

3. **Render guard** (structural invariant, see §5.3.4): the editor only renders when `titleEdit() && titleEdit()!.wgPath === menu().wg.path` — an editor for workgroup X can never appear inside the menu of workgroup Y, even if stale state ever survived.

Coverage statement (corrected from round 1; BLOCKER-2 added round 3): the effect covers every **menu-close** path (outside click, Escape, hover-leave grace timer, `clearReplicaTaskTitle`, any action that closes the menu); nets 2+3 cover the **menu-replacement** path (right-click another replica while editing). `saveReplicaTitle`'s close-guard (5.3.2) is a net on the save side; the epoch token (this section + 5.3.2) is a net on the `getTitle`-continuation side (BLOCKER-2). There is no path by which an editor for workgroup A renders in — or saves through — workgroup B's menu, and no path by which a stale continuation writes to, resets, or un-busies a newer editor's state.

**5.3.2 Handlers** (place next to `clearReplicaTaskTitle`, ~line 1674):

```ts
// Liveness-aware session resolution for the EDITOR only. The broom keeps the
// existing resolveWorkgroupSessionId (first non-inactive id, live or not).
const resolveLiveWorkgroupSessionId = (wg: AcWorkgroup): string | null => {
  for (const peer of wg.agents) {
    const s = replicaSession(wg, peer);
    if (s && !s.id.startsWith("inactive-") && isSessionLive(s)) return s.id;
  }
  return null;
};

const startReplicaTitleEdit = async (wg: AcWorkgroup) => {
  const sessionId = resolveLiveWorkgroupSessionId(wg);
  setTitleError(null);
  setTitleDraft(wg.taskTitle ?? "");
  setTitleEdit({ wgPath: wg.path, sessionId });
  // Invocation token (BLOCKER-2, round 3). Each start bumps the epoch; any
  // later resetTitleEditState() (menu close, replacement, cancel) or newer
  // start bumps it again, so this invocation detects staleness with zero
  // microtask window — including same-wg-same-session double-clicks.
  const epoch = ++titleEditEpoch;
  const stillCurrent = () =>
    titleEditEpoch === epoch &&
    !!titleEdit() &&
    titleEdit()!.wgPath === wg.path &&
    titleEdit()!.sessionId === sessionId;
  if (sessionId) {
    setTitleBusy(true);
    try {
      const fromBackend = await TaskAPI.getTitle(sessionId);
      // Stale: bail BEFORE any shared-state mutation.
      if (!stillCurrent()) return;
      if (fromBackend !== null && fromBackend !== undefined) {
        setTitleDraft(fromBackend);
      }
    } catch (err) {
      // Stale: never reset a newer editor's state.
      if (!stillCurrent()) return;
      resetTitleEditState();
      setTitleError(String(err));
      return;
    } finally {
      // Busy belongs to the CURRENT invocation only; a stale continuation
      // must not clear a newer invocation's busy flag.
      if (stillCurrent()) setTitleBusy(false);
    }
  }
  if (!stillCurrent()) return;
  // The editor grows the menu: re-clamp so it stays inside the viewport.
  reclampReplicaCtxMenu();
};

const saveReplicaTitle = async () => {
  const target = titleEdit();
  if (!target) return;
  const title = titleDraft().trim();
  if (!title) {
    setTitleError("Title cannot be empty.");
    return;
  }
  setTitleBusy(true);
  setTitleError(null);
  try {
    if (target.sessionId) {
      await TaskAPI.setTitle(target.sessionId, title);
    } else {
      await TaskAPI.setTitleAt(target.wgPath, title);
    }
    // Close only if the same workgroup's menu is still open (raw path equality).
    if (replicaCtxMenu() && replicaCtxMenu()!.wg.path === target.wgPath) {
      closeReplicaCtxMenu();
    }
  } catch (e) {
    console.error("Failed to edit task title:", e);
    setTitleError(String(e));
  } finally {
    setTitleBusy(false);
  }
};

const cancelReplicaTitleEdit = () => {
  resetTitleEditState();
};
```

Notes, binding:

- `resolveLiveWorkgroupSessionId` is used **only** by the editor. Liveness routing (round-2 reviewer note 4): an exited session whose backend entry was dropped would make `task_get_title`/`task_set_title` reject with "session not found" — routing the editor to the path-based `task_set_title_at` in that case keeps the editor usable and targets the same workgroup `TASK.md`. The broom's `resolveWorkgroupSessionId` is **not** changed (shipped behavior, accepted precedent — see §10).
- `startReplicaTitleEdit` mirrors the terminal's `startEditing` (prefill locally, then refresh via `getTitle` when a live session exists). The token check (`stillCurrent()`) compares **`wg.path` and `sessionId`** (raw strings), never the menu object identity — `positionReplicaCtxMenu` writes a NEW menu object on every reclamp, so an object-identity guard could spuriously abort the `getTitle` continuation (reviewer note resolved by this choice).
- **Staleness (BLOCKER-2, round 3)**: the continuation bails BEFORE any shared-state mutation (`setTitleDraft`, `setTitleError`, `resetTitleEditState`, `setTitleBusy`) when `stillCurrent()` is false — a stale success never writes its draft over a newer editor's draft, a stale rejection never resets a newer editor's state, and a stale `finally` never clears a newer invocation's busy flag. Because `resetTitleEditState()` bumps the epoch, staleness is detected synchronously on menu close/replacement/cancel (no microtask window), and same-wg-same-session double-click is covered by the epoch itself. The round-2 `sameMenu()`-based "aborts and resets" analysis is superseded (see 7.6).
- On `getTitle` rejection (still current): the editor state is reset and the error is shown **outside** the editor block (5.3.4) — the editor briefly mounts (state is set before the await) and unmounts on rejection; the menu stays open.
- `saveReplicaTitle` mirrors `saveTitle` (empty-title guard, busy, captured target). The menu is closed **after** success only when the same workgroup's menu is still open (raw `wg.path` equality; see 7.6 for why reference equality is wrong). An in-flight save is never redirected: if the menu was replaced while saving, the save still completes for the captured target and the close-guard no-ops — the captured target is the workgroup the user clicked, so this cannot write to a different workgroup than the one whose editor was open.
- No manual store write: the `workgroup_task_updated` event (emitted by both `task_set_title` and `task_set_title_at`) refreshes the sidebar via `updateWorkgroupTask` (evidence 3.5) — same reliance as the broom.
- Error message and console line follow the broom: `console.error("Failed to edit task title:", e)`.

**5.3.3 Menu item** — in **both** branches, insert immediately above the broom:

- Active branch: after `{renderAddToGroupItem(menu().wg, menu().replica)}` **and after** the existing `<div class="context-separator" />`, immediately before the broom `<button>` (i.e. the existing separator keeps separating group actions from the task-title actions).
- Inactive branch: after `{renderAddToGroupItem(menu().wg, menu().replica)}`, immediately before the broom `<button>` (there is no separator in this branch — do not add one).

Item markup (same in both branches):

```tsx
<button
  class="session-context-option"
  title="Edit TASK title"
  onClick={(e) => {
    e.stopPropagation();
    void startReplicaTitleEdit(menu().wg);
  }}
>
  <span class="session-context-option-icon" aria-hidden="true">&#x270E;</span> Edit TASK title
</button>
```

- Always enabled: no `disabled`/`classList` logic (an empty/clean title is a valid edit target — the user types a new title; matches the terminal, whose edit button is enabled for empty titles).
- `e.stopPropagation()` is retained as belt-and-suspenders (evidence 3.3.2): the container's `onClick` already stops inside-clicks today, but the item's behavior must not depend on that single container-level guard surviving a refactor (the `Add to Group` trigger follows the same per-element pattern).

**5.3.4 Inline editor** — in **both** branches, directly after the new item (before the broom). The editor renders only when it matches the open menu's workgroup (render guard — BLOCKER-1 net 3); the error line is a **sibling** of the editor block (mirrors the terminal, where `workgroup-task-error` sits outside the editor `<Show>`), so a `getTitle` rejection can show the error without a mounted editor:

```tsx
<Show when={titleEdit() && titleEdit()!.wgPath === menu().wg.path}>
  <div
    class="session-context-title-edit"
    onClick={(e) => e.stopPropagation()}
  >
    <input
      ref={(el) => requestAnimationFrame(() => { el.focus(); el.select(); })}
      class="session-context-title-input"
      value={titleDraft()}
      onInput={(e) => setTitleDraft(e.currentTarget.value)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          if (!titleBusy()) void saveReplicaTitle();
        } else if (e.key === "Escape") {
          e.preventDefault();
          cancelReplicaTitleEdit();
        }
      }}
      placeholder="Title"
      disabled={titleBusy()}
    />
    <button
      class="session-context-title-btn save"
      onClick={(e) => { e.stopPropagation(); void saveReplicaTitle(); }}
      disabled={titleBusy() || !titleDraft().trim()}
      type="button"
    >
      Save
    </button>
    <button
      class="session-context-title-btn cancel"
      onClick={(e) => { e.stopPropagation(); cancelReplicaTitleEdit(); }}
      disabled={titleBusy()}
      type="button"
    >
      Cancel
    </button>
  </div>
</Show>
<Show when={titleError()}>
  <div class="session-context-title-error">{titleError()}</div>
</Show>
```

Notes, binding:

- **Render guard**: `titleEdit() && titleEdit()!.wgPath === menu().wg.path` in both branches. Same-workgroup replacement (another replica of the same wg) keeps the editor — the target is identical; cross-workgroup replacement hides it (and nets 1-2 in 5.3.1 clear the state anyway).
- The container-level `onClick` stopPropagation covers the input and both buttons (single guard point; belt-and-suspenders — see evidence 3.3.2).
- The input's `onKeyDown` stops propagation on **every** key and is **strictly required**: Escape must cancel the editor, not close the whole menu via the window `keydown` dismiss (evidence 3.3.1), and Enter must reach `saveReplicaTitle`. Enter/Escape are ignored while `titleBusy()`.
- Focus + select on open mirrors the terminal's `onInputRef`.
- The editor renders inside whichever branch is open; both branches share the same signals (only one menu is ever open). The error line renders whenever `titleError()` is set and the branch is mounted — a stale error cannot survive a menu replacement because nets 1-2 clear `titleError` on every menu open.

**5.3.5 Hover-close suppression** — in the menu container's `onMouseLeave` handler at lines 3377-3380, before `scheduleReplicaCtxMenuClose()`, add the guard (mirroring the `groupErrorPinned()` pattern already inside `scheduleReplicaCtxMenuClose`):

```ts
if (titleEdit()) {
  cancelReplicaCtxMenuClose();
  return;
}
```

This keeps the menu open while the user interacts with the editor (evidence 3.3.3). Do not touch `scheduleReplicaCtxMenuClose` itself, `groupErrorPinned`, or the flyout scheduling.

**5.3.6 CSS** — `src/sidebar/styles/sidebar.css`, new classes next to the existing `.session-context-option*` block (~3204-3300), mirroring the terminal editor's visual proportions (`.workgroup-task-title-edit/input/btn/error` in `src/terminal/styles/terminal.css:293-348` are the reference; do **not** reuse those classes):

- `.session-context-title-edit` — flex row, gap consistent with `session-context-option` padding, width 100% of the menu.
- `.session-context-title-input` — flex: 1, min-width for the menu's width (~180-220px), inherits menu font, `disabled` dimming.
- `.session-context-title-btn` + `.save` / `.cancel` — compact buttons, `:disabled` styles (mirror `.workgroup-task-title-btn` behavior: disabled when busy; Save also disabled when the trimmed draft is empty).
- `.session-context-title-error` — error line under the editor, styled like `.workgroup-task-error` (dimmed, small).
- Light-theme variants: mirror the terminal's `html.light-theme .workgroup-task-title-*` rules for the new classes (sidebar.css already carries light-theme overrides for other `session-context-*` rules — follow that pattern).
- If the gray (inactive) menu is dimmed via a container class, ensure the editor inside it remains readable/interactive (the input must accept focus and typed text regardless of the menu's dot-state styling; verify visually in both dot states).

### 5.4 Rejected alternatives

1. **Modal/prompt editor (e.g. window.prompt or a modal dialog)**: rejected — breaks the terminal precedent (inline editor), adds a new modal component, and the requirement pins the pencil icon/tooltip + terminal-mirroring behavior.
2. **Reuse terminal classes** (`workgroup-task-title-*`) in the sidebar menu: rejected — the sidebar keeps its own `session-context-*` namespace; reusing terminal classes would couple the two features' styling.
3. **Duplicate the three title validations inline in `task_set_title_at`**: rejected — extraction into `validate_user_title` is pure code motion inside one module and prevents message/order drift between the two commands; both call sites and messages are identical to today's `task_set_title`.
4. **Add `task_get_title_at` for cold-workgroup prefill**: rejected as speculative — `menu().wg.taskTitle` is already present in both menus (the broom's disabled state consumes it), and cold workgroups are refreshed by `poll` events; a path-based title read adds a command with no user-visible benefit.
5. **Close the menu before saving (broom style)**: rejected for the editor — the menu must stay open while editing; closing happens after a successful save (see 7.2).
6. **Keep the menu open after a successful save**: rejected — the broom closes the menu after acting; a stale open menu would overlay the very row whose title just changed. The event refresh is still observable on the row once the menu closes.

---

## 6. Affected surfaces, exhaustively

| File | Change | Owner |
|---|---|---|
| `src-tauri/src/commands/task.rs` | `validate_user_title` helper (extracted); `task_set_title` calls it; new `task_set_title_at` command; unit tests (9.1) | ac-dev-rust-v3 |
| `src-tauri/src/lib.rs` | register `commands::task::task_set_title_at` in `invoke_handler` (adjacent to line 2912) | ac-dev-rust-v3 |
| `src/shared/ipc.ts` | `TaskAPI.setTitleAt` after `cleanAt` (line 677) | ac-dev-rust-v3 (single commit with the two Rust files, or handed to ac-dev-webpage-ui-v3 in a follow-up commit — decide on merge; both are on the same branch) |
| `src/sidebar/components/ProjectPanel.tsx` | 4 signals + `resetTitleEditState` + reset effect (5.3.1); `resetTitleEditState()` calls in both menu-open handlers next to `resetGroupMenuState()` (~1993/~2005); `resolveLiveWorkgroupSessionId` / `startReplicaTitleEdit` / `saveReplicaTitle` / `cancelReplicaTitleEdit` (5.3.2); item in both branches (5.3.3); editor JSX with render guard + error line in both branches (5.3.4); hover-close guard (5.3.5) | ac-dev-webpage-ui-v3 |
| `src/sidebar/styles/sidebar.css` | `session-context-title-*` classes (5.3.6) | ac-dev-webpage-ui-v3 |
| `src/sidebar/components/ProjectPanel.context-menu.test.tsx` | new tests (9.2) | ac-dev-webpage-ui-v3 |
| `docs/features/sidebar-guide.md` | sibling bullet for `Edit TASK title` next to the broom description (lines 58-62) | ac-dev-webpage-ui-v3 |

Ordering constraint: ac-dev-webpage-ui-v3 can implement the entire `src/` side with the FakeTransport stubbing `task_set_title_at`/`task_get_title`/`task_set_title` before ac-dev-rust-v3 lands the backend; the backend and frontend do not block each other. The branch must not be merged until both sides and all tests are green.

---

## 7. Required behavior, edge cases, failure behavior

### 7.1 Item placement and presentation

- Both menus: `Edit TASK title` sits directly above the broom (`Clear task title`), below `Add to Group` (active menu: after the existing `context-separator`; inactive menu: directly after `renderAddToGroupItem(...)`).
- Icon `&#x270E;` inside `<span class="session-context-option-icon" aria-hidden="true">`; tooltip `title="Edit TASK title"`; class `session-context-option`. Never disabled.
- No `data-ac-testid` (consistent with the broom, which tests find by text).

### 7.2 Editor lifecycle

1. Click `Edit TASK title` → menu stays open, editor appears under the item, input focused + selected, prefilled (7.3), menu re-clamped (`reclampReplicaCtxMenu`).
2. Typing updates the draft; Save enabled only when the trimmed draft is non-empty and not busy; Cancel enabled unless busy.
3. Enter → save; Escape → cancel (menu stays open, editor closes, draft cleared); clicking outside the menu, right-clicking, or pressing Escape while the editor is **not** focused still dismisses the whole menu (existing behavior, unchanged) and the reset effect clears editor state.
4. Hover-leave of the menu while editing does **not** close it (5.3.5 guard).
5. **Right-clicking another replica while editing** replaces the menu directly (never through `null`, #943): the menu-open handler resets the edit state (5.3.1 net 2) and the render guard (5.3.4) prevents any stale editor from appearing in the new menu — a cross-workgroup title write is impossible (regression-tested in 9.2.8; the in-flight `getTitle` race in 9.2.9). Same-workgroup replacement (another replica of the same wg) also closes the editor via the handler reset; the target would have been identical, so no wrong-target write either way.
6. Save success → menu closes (guarded: only if the same workgroup's menu is still open); row refreshes via `workgroup_task_updated`.
7. Save failure → menu stays open, inline error shown, `console.error("Failed to edit task title:", e)`; draft kept so the user can retry.
8. Cancel → editor closes, menu stays open with the normal item list.

### 7.3 Prefill

- Cold/gray workgroup (no session): `menu().wg.taskTitle ?? ""`. If `undefined`/empty (never titled), the input is empty and Save is disabled until the user types — same as the terminal.
- Workgroup with a live session: same local prefill first, then refresh via `TaskAPI.getTitle(sessionId)` (terminal mirror). The editor mounts immediately with the local prefill (state is set before the await) and, if the fetch fails, unmounts again with the inline error shown (the error line is a sibling of the editor block — 5.3.4) while the menu stays open. If the menu closed or was replaced while awaiting (checked by `wg.path`, not object identity), the continuation aborts and resets.
- `taskTitle === "Clean"` (the sentinel): prefills the literal `Clean` — the user edits it as text, exactly as the terminal does (`getTitle` returns the header title).

### 7.4 Save semantics (frontend contract)

- `resolveLiveWorkgroupSessionId(wg)` non-null (live session) → `TaskAPI.setTitle(sessionId, title)`.
- `null` (gray workgroup, only `inactive-*` entries, or only exited sessions) → `TaskAPI.setTitleAt(wg.path, title)`.
- The menu is closed after success only when the currently open menu (if any) still belongs to the same workgroup (raw `wg.path` equality).

### 7.5 Backend contract

`task_set_title_at` returns `Err` (→ IPC rejection → inline error) for:

- empty/whitespace title (`title cannot be empty`);
- any control character other than tab (`title must be a single line of printable characters (control characters other than tab are not allowed)`);
- more than 256 Unicode scalars (`title is too long (max 256 characters)`);
- a `workgroup_root` that fails `validate_wg_root` (same failure as `task_clean_at`).

Success: creates `TASK.md` when missing (same `task_ops::perform` path as `task_clean_at` — proven by `task_clean_at_creates_task_when_missing`), writes the user title, emits `workgroup_task_updated` (`source: "manual"`), returns `TaskUpdateResult { workgroupRoot: strip_unc(&wg_root), task }`. `EditOutcome::RejectedUserTitle` maps to the same `TaskUpdateResult` + emission as `task_set_title` (no error) — identical mapping code.

### 7.6 Race and state-safety cases

- **Store replaces `wg` objects on events** (`updateWorkgroupTask` builds `{ ...wg, task, taskTitle }`): the success-close guard MUST compare `replicaCtxMenu()!.wg.path === target.wgPath` (raw strings), never the `wg` object reference (a reference comparison fails right after the save event lands — the very event the save triggers).
- **Menu replaced while editing (A→B, #943 replacement, not a dismissal)**: the menu-open handler resets the edit state (5.3.1 net 2); the render guard (5.3.4) hides any editor whose `wgPath` differs from the open menu's `wg.path`; `saveReplicaTitle`'s close-guard (raw path equality) never closes B's menu and never writes to B from A's captured target, because A's editor no longer exists after the reset. No stale editor can render or save.
- **Stale `getTitle` continuation vs a NEWER editor (BLOCKER-2, round 3 — corrected)**: edit on wg X (getTitle(X) in flight) → right-click wg Y → edit on wg Y (getTitle(Y) in flight) → getTitle(X) settles. The X continuation first checks its invocation token (epoch + `titleEdit` identity); it is stale, so it bails **before** `setTitleDraft`, `setTitleError`, `resetTitleEditState`, and `setTitleBusy` — Y's draft, error, busy, and mounted editor are untouched. This holds for both settle outcomes (success and rejection) and both settle orders (X-before-Y, where X bails and Y proceeds as current; Y-before-X, where Y's continuation is the current one and X bails). Requirement (B) holds in the race window; typed input is never lost or clobbered (regression-tested in 9.2.9). The WRITE invariant (nets 1-3, 5.3.1) was already safe; the round-2 plan's "aborts and resets" analysis is now superseded by the token check, which never resets or writes state it does not own.
- **Menu closed/replaced while `getTitle` is in flight (no newer editor)**: the epoch bump inside `resetTitleEditState()` (reset effect on close, handler reset on replacement) invalidates the continuation synchronously; it bails and leaves no state behind. If the menu is open and unchanged, the continuation proceeds normally.
- **Menu closed/replaced while the SAVE is in flight**: `saveReplicaTitle` completes the save for the captured target (the workgroup the user clicked — never a different one), then the close guard no-ops if the menu changed; on failure with no menu open, only the console log remains (no error element — correct). The save path owns its busy/error signals itself (user-driven interaction), so no epoch guard applies there.
- **Menu object rewrites during the `getTitle` await** (`positionReplicaCtxMenu` writes a NEW menu object on every reclamp): the `stillCurrent()`/close guards compare raw strings (`wg.path` + `sessionId`), never object identity, so a mid-await reclamp cannot spuriously abort the continuation (reviewer note resolved — decision recorded in §10).
- **Double-click on `Edit TASK title`**: second click re-runs `startReplicaTitleEdit` (refetch + reset draft). Harmless, mirrors the terminal's re-clickable edit button. Not special-cased.
- **Editor open while a group flyout is pinned/error-pinned**: unchanged — flyouts and the editor coexist; `groupErrorPinned()` logic untouched.
- **Menu at the viewport edge**: the editor grows the menu; `reclampReplicaCtxMenu()` re-clamps via `getBoundingClientRect` after the editor opens (evidence 3.2).
- **Window resize while editing**: existing `reclampReplicaCtxMenu` listeners (if any are wired today) continue to work; no new wiring.

### 7.7 Keyboard accessibility

- Item is a native `<button>` (focusable, Enter/Space activate — default button behavior; the item's `onClick` handles activation; no custom `onKeyDown` needed on the item).
- Input handles Enter (save) and Escape (cancel); both keys `stopPropagation` so the window dismiss listener never sees them.
- Save/Cancel are native buttons with `disabled` semantics.

---

## 8. Compatibility and security

- **IPC**: one new command name (`task_set_title_at`) with two string args; no existing command/event/payload shape changes; `TaskUpdateResult` unchanged. Old frontends calling only existing commands are unaffected.
- **Persistence**: writes go through the same `task_ops::perform(TaskOp::SetUserTitle)` path as the terminal save — same file format, backup semantics, and frontmatter rules. No migration.
- **Security boundaries**: `task_set_title_at` validates the workgroup root against `project_paths` (same as `task_clean_at`) before any file IO; title length/codepoint validations mirror `task_set_title` (256-scalar cap prevents oversized paste blobs). No new capabilities, no path traversal surface beyond the existing `validate_wg_root` gate.
- **Multi-window**: the `workgroup_task_updated` emission is app-wide (same as `task_set_title`/`task_clean_at`), so the sidebar refreshes regardless of which window saved.
- **Light theme**: new CSS classes get `html.light-theme` variants per 5.3.6.

---

## 9. Tests and objective acceptance criteria

### 9.1 Backend unit tests — `src-tauri/src/commands/task.rs` (owner: ac-dev-rust-v3)

Mirror the `task_clean_at` tests (same helpers: `tempfile::tempdir`, `make_real_wg`, `paths_of`, `validate_wg_root`, `perform`, `read_task_fields_at`):

1. `task_set_title_at_creates_task_when_missing` — mirror of `task_clean_at_creates_task_when_missing` (620-633): real wg, assert no `TASK.md`, `perform(&validated, TaskOp::SetUserTitle("My Title".into()))` → `matches!(outcome, EditOutcome::Wrote { backup: None, .. })`, file exists, `read_task_fields_at` returns title `Some("USER: My Title")` (round-3 correction — marker note below).
2. `task_set_title_at_round_trip_returns_title` — mirror of `task_clean_at_round_trip_returns_clean_title` (521-533): seed via `TaskOp::SetTitle("Real Brief".into())`, then `SetUserTitle("Edited Title".into())` → title reads back `Some("USER: Edited Title")`.

**Round-3 marker correction (rust-dev, verified at e9365c2)**: `TaskOp::SetUserTitle` writes `user_owned_title(title)` — `"USER: " + title` — into the file (task_ops.rs:290; pinned by `user_owned_title_prefixes_plain_title`, task_ops.rs:1449-1450), and neither reader strips the marker: `entity_creation::parse_task_title` (entity_creation.rs:294-327) and `task_ops::title_value_of` → `decode_title_scalar` (task_ops.rs:108-118, 409-413) both return the raw scalar. So the readback assertions MUST expect the `USER: ` prefix (`Some("USER: My Title")` / `Some("USER: Edited Title")`). This is the same scalar the live `task_set_title` path writes (no command-design change; §7.5 stays consistent), and it matches the existing shipped behavior that the sidebar/terminal display after a user-title save.

3. `validate_user_title_*` cases (the extracted helper is pure): `validate_user_title_rejects_empty_and_whitespace`; `validate_user_title_rejects_control_chars_except_tab` (nul, `\n`, `\r` rejected; a title containing a tab accepted); `validate_user_title_rejects_over_256_scalars` (257 chars rejected; 256 accepted); `validate_user_title_accepts_normal_title`.

Command-level behavior of `task_set_title_at` (arg plumbing, `emit_task_updated`, `strip_unc`) is exercised through the existing command-harness patterns already used for sibling commands in this file, if and only if such a harness exists for them (the `task_clean_at` tests above are perform-level, which is the established bar — do not invent a new harness).

### 9.2 Sidebar tests — `src/sidebar/components/ProjectPanel.context-menu.test.tsx` (owner: ac-dev-webpage-ui-v3)

Add a `findEditTitleButton(menu)` helper mirroring `findBroom` (matches button textContent containing `"Edit TASK title"`). Register FakeTransport handlers with `fake.resolve(...)` for every command a test triggers (`task_get_title`, `task_set_title`, `task_set_title_at`) — unhandled commands throw (evidence 3.7). New fixtures (mirror the existing `projectDiscovery`/`coordSession` shapes): `projectDiscoveryTwoWorkgroups()` adds a second workgroup `wg-3-other-team` at `${projectPath}\\.ac\\wg-3-other-team` (agents `dev-webpage-ui` coordinator + `dev-rust` member, `taskTitle: "Other team title"`), and `otherSession()` (id `"other-session"`, name `"wg-3-other-team/dev-rust"`, `workingDirectory: ${otherWorkgroupPath}\\__agent_dev-rust`, status running). Row testid: `replica.row.workgroups.wg-3-other-team.dev-rust` (follows `automationIdPart` slug rules like `memberRowTestId`).

1. **Both menus show the item, in order (round-3 corrected)** — `renderAddToGroupItem` renders only under `<Show when={replica.isCoordinator}>` (ProjectPanel.tsx:1508), so a member row has NO `Add to Group` button and an "between Add to Group and broom" assertion is unsatisfiable there. Split the assertion by row kind:
   - **Coordinator row** (`replica.row.workgroups.wg-2-dev-team.dev-webpage-ui`), active menu (`setupPanel([coordSession(), memberSession()])`) and gray menu (coordinator session omitted): assert `findEditTitleButton(menu)` exists and its index among the menu's buttons is between the `Add to Group` button's index and `findBroom(menu)`'s index.
   - **Member row** (`replica.row.workgroups.wg-2-dev-team.dev-rust`), both menus: assert the item exists and sits **directly above the broom** (item index === `findBroom(menu)` index − 1).
2. **Active session save** — `setupPanel([coordSession(), memberSession()])`, member row; open the menu, click the item; editor appears (`.session-context-title-edit` input in the menu); prefill from `fake.resolve("task_get_title", "Backend Title")` wins over `projectDiscovery("Local Title")`; type a new title; click Save → assert **explicitly** `fake.lastCall("task_set_title")` is defined with `args.sessionId === "coord-session"` and `args.title` matching (the coordinator is first in `wg.agents`, so `resolveLiveWorkgroupSessionId` returns `"coord-session"` — same pin as the existing broom test at :958); menu closes (`replicaMenu()` becomes null); store updated via the event (emit `workgroup_task_updated` through the fake transport listener if the harness supports it — otherwise assert the lastCall only).
3. **Cold (no-session) save** — fixture **must** be `setupPanel([])` (no sessions at all): any session in `wg.agents` would resolve via the coordinator and route to `task_set_title` — this is exactly the `task_clean_at` cold-route pattern at :1104. Open the gray menu on the member row; click the item; **do not register** `task_get_title` (an accidental call throws "Unhandled fake transport invoke" and fails the test — a built-in assertion that no session path is taken); prefill equals the discovery `taskTitle`; type; click Save → `fake.lastCall("task_set_title_at")` with `{ workgroupRoot: workgroupPath, title }`; **and** `expect(fake.lastCall("task_set_title")).toBeUndefined()` (mirror the cold `task_clean_at` test's no-fallback assertion at :1104-1123); menu closes.
4. **Save disabled on empty draft** — open the editor with an empty prefill (`setupPanel([])`, `projectDiscovery(null)`): Save disabled; type a space only → still disabled; type text → enabled.
5. **Enter saves, Escape cancels** — `setupPanel([coordSession(), memberSession()])`; dispatch `keydown` Enter on the input → `task_set_title` lastCall (`sessionId: "coord-session"`); reopen, Escape → editor closes, menu stays open (`.session-context-title-edit` gone, `.session-context-menu` still in the DOM), no IPC call.
6. **Failure path** — `fake.reject("task_set_title", "boom")`; click Save → inline `.session-context-title-error` visible with the error text; menu still open; draft preserved.
7. **getTitle failure** — `fake.reject("task_get_title", "boom")`; click the item → the editor mounts briefly (state is set synchronously before the await) and unmounts on rejection: after settling, assert no `.session-context-title-edit` in the menu, `.session-context-title-error` visible with the error text, menu still open.
8. **A→B replacement regression (BLOCKER 1)** — `setupPanel([coordSession(), otherSession()], projectDiscoveryTwoWorkgroups())`; open the wg-2 member row's menu (`contextMenu(findRow(rendered!.root, memberRowTestId))`); click its `Edit TASK title`; `fake.resolve("task_get_title", "A title")`; wait for `.session-context-title-edit`. Then right-click the wg-3 member row (`contextMenu(findRow(rendered!.root, otherRowTestId))`) — this replaces the menu directly, never through `null` (#943). Assert: the menu is replaced (it contains `Restart Session`, i.e. an active menu) but contains **no** `.session-context-title-edit` and no `Save` button; `fake.lastCall("task_set_title")` and `fake.lastCall("task_set_title_at")` are both undefined. Then click wg-3's `Edit TASK title` and Save → `fake.lastCall("task_set_title")` has `args.sessionId === "other-session"` (targets wg-3, never wg-2) — the end-to-end proof that no stale editor or captured target survived the replacement.
9. **Stale `getTitle` continuation vs a newer editor (BLOCKER 2, round-3 new)** — §9.2.8 settles wg-2's `getTitle` BEFORE the replacement, so it cannot exercise the race. Use a pending promise as the deferred (FakeTransport's `onInvoke` handler may return a promise; `fake-transport.ts:48`):
   ```ts
   let resolveCoordGetTitle!: (value: string) => void;
   const coordPending = new Promise<string>((res) => { resolveCoordGetTitle = res; });
   fake.onInvoke("task_get_title", (args) =>
     args.sessionId === "coord-session" ? coordPending : Promise.resolve("Other team title")
   );
   ```
   Script: `setupPanel([coordSession(), otherSession()], projectDiscoveryTwoWorkgroups())`; open the wg-2 member row's menu and click its `Edit TASK title` (getTitle("coord-session") stays PENDING); right-click the wg-3 member row (`contextMenu(findRow(rendered!.root, otherRowTestId))`) and click wg-3's `Edit TASK title` (getTitle("other-session") resolves immediately; editor mounted with draft "Other team title"); **then** `resolveCoordGetTitle("A title")` and flush microtasks (`waitFor`). Assert: wg-3's editor is still mounted with its draft intact (input value still "Other team title" — wg-2's stale continuation must not have overwritten the draft, reset the editor, or cleared busy); then click Save → `fake.lastCall("task_set_title").args.sessionId === "other-session"` and `args.title` matching the draft. Without the round-3 token fix this test fails: the stale continuation would `setTitleDraft("A title")` and the old identity check would then reset wg-3's editor entirely (or, on rejection, the unconditional reset would wipe it).
10. **Broom regression** — existing broom tests (order, disabled states at ~1040-1090) must stay green unchanged; the new item must not alter `findBroom` matches.

### 9.3 Docs

`docs/features/sidebar-guide.md`: next to the broom description (lines 58-62), add one bullet: right-click a replica → `Edit TASK title` (pencil) opens an inline editor in the menu; Save writes the title (session or path-based command), the menu closes, and the sidebar row updates; Cancel/Escape closes the editor.

### 9.4 Objective acceptance criteria (run on the final branch head, clean tree)

1. `cargo test -p agentscommander_lib --lib commands::task` (or the crate's equivalent targeted invocation) passes, including the new tests.
2. The sidebar vitest suite passes, including `ProjectPanel.context-menu.test.tsx` (full file).
3. `npm`/`cargo` lint and type checks (the repo's standard `check` targets for `src/` and `src-tauri/`) pass with no new warnings from the changed files.
4. **Dependency-cycle criterion** (planning rule 8; base SHA `e9365c2` vs final branch head, clean tree for both):

```
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet   # base SHA
node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet  # final head
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

Green iff: (a) `summary.moduleCycles` equal pre/post (must not increase); (b) `summary.modulesResolved` and `summary.moduleEdges` equal pre/post; (c) the regenerated `module-arcs.txt` is byte-identical to the committed one (empty `git status` on it); (d) `git status --porcelain` on the changed Rust files shows exactly the plan's files and nothing else; (e) the structural layering guards (e.g. `loops_layering`, `instance_gitignore_layering`, `project_settings_layering` tests) stay green. Exit code 1 of the detector is the normal gating outcome; only exit 3 means no graph.

5. Manual smoke (optional for the implementer, required for sign-off): right-click a running replica → Edit TASK title → editor opens prefilled → change title → Save → menu closes, row title updates; repeat on a gray replica (never launched) → save creates the title; terminal save and sidebar save both visible in both panels.

---

## 10. Explicit decisions and accepted residuals

1. **Editor placement**: inline inside the open context menu (terminal mirror) — decided, requirement-mandated shape.
2. **Menu stays open while editing**; hover-leave close suppressed only while `titleEdit()` is set — decided (5.3.5). Residual: while editing, the Add to Group flyout can still be triggered by hovering its item; the flyout and editor coexist (accepted — no interaction with each other).
3. **After successful save the menu closes** (broom behavior) with the same-workgroup guard — decided (7.6). Residual: the close is not instant-on-click but after the await resolves (sub-100ms; the editor shows busy meanwhile) — accepted, matches the terminal's own save-then-close.
4. **Prefill**: `wg.taskTitle` locally, refreshed via `TaskAPI.getTitle` when a session exists; no `task_get_title_at` — decided (5.4.4). Residual: for cold workgroups the prefill may be slightly stale relative to disk if the last `poll` event predates an external (CLI) edit — accepted; the poll cadence bounds this, and the user is about to overwrite the title anyway.
5. **Validation helper extraction** (`validate_user_title`) — decided (5.4.3); messages and order byte-identical to today's `task_set_title`; CLI validator untouched (different module, different contract).
6. **The new item is never disabled** (even for empty titles) — decided; matches the terminal edit button.
7. **No testid** for the new item — decided; tests match by text like the broom.
8. **`task_set_title_at` validation order** — title first, then `validate_wg_root` (mirrors `task_set_title`'s validate-then-resolve order).
9. Residual: the `Edit TASK title` item shows even when the workgroup has no `TASK.md` yet (cold); saving creates the file via `perform` — same as the broom (`task_clean_at_creates_task_when_missing`), so no menu-item hiding logic is needed.
10. **BLOCKER-1 mechanism (round 2)**: the stale-editor-on-replacement defect is fixed with **three nets together** — (a) `resetTitleEditState()` called in both menu-open handlers next to `resetGroupMenuState()` (ProjectPanel.tsx:1977/2017), (b) the render guard `titleEdit() && titleEdit()!.wgPath === menu().wg.path` in both branches, and (c) the existing null-transition reset effect. All three are retained (not alternatives): the handler reset makes replacement deterministic regardless of render logic, and the render guard makes the no-stale-editor property structural. Same-workgroup replacement also closes the editor (handler reset) — the target would have been identical, so this is behavior-only, not correctness.
11. **Exited-session routing (round 2 reviewer note 4)**: the editor resolves the first **live** non-`inactive-` session (`resolveLiveWorkgroupSessionId`, using the existing `isSessionLive` predicate at ProjectPanel.tsx:218) instead of `resolveWorkgroupSessionId`. A session whose backend entry was dropped would otherwise make `task_get_title`/`task_set_title` reject ("session not found") and block editing even though the path-based `task_set_title_at` works. The broom's `resolveWorkgroupSessionId` is **unchanged** (shipped behavior, accepted precedent). Residual: a live session that dies between click and save surfaces the rejection as the inline error (visible, retryable) — the same class as any in-flight failure; no automatic save-time fallback is attempted (string-matching backend errors is fragile).
12. **Guard mechanism (round 2 reviewer note 5)**: all menu-identity checks in the editor (`stillCurrent()` in `startReplicaTitleEdit` — `wg.path` + `sessionId` raw strings, round-3 evolution of `sameMenu()`; the close-guard in `saveReplicaTitle`) compare **raw strings**, never the menu object identity — `positionReplicaCtxMenu` writes a NEW menu object on every reclamp, so object identity would spuriously abort a `getTitle` continuation during a mid-await reclamp. Decision: string comparison; no residual.
13. **Dismissal-trap wording (round 2 reviewer note 3)**: the menu container's existing `onClick` stopPropagation (~3374) already protects inside-clicks from the window dismiss listener; the per-element guards (item, editor container, input keydown) are retained as belt-and-suspenders. The input `keydown` guard is strictly required (keydown is not a click; the window `keydown` dismiss fires on Escape). No container change.
14. **BLOCKER-2 mechanism (round 3, tech-lead/grinch verified)**: the stale-`getTitle`-continuation defect is fixed with an **invocation token** — a component-level monotonic epoch `titleEditEpoch` (5.3.1) that every `startReplicaTitleEdit` captures after setting state and that `resetTitleEditState()` bumps, plus a `stillCurrent()` identity check (`titleEditEpoch === epoch && titleEdit()!.wgPath === wg.path && titleEdit()!.sessionId === sessionId`). The continuation bails **before any shared-state mutation** (`setTitleDraft`, `setTitleError`, `resetTitleEditState`, `setTitleBusy`) when stale; the `finally { setTitleBusy(false) }` carries the same guard. Bumping the epoch inside the reset makes invalidation synchronous on menu close/replace/cancel (no microtask window) and covers same-wg-same-session double-clicks via the epoch itself. The round-2 plan's "aborts and resets" continuation analysis (§7.6) is superseded: a stale continuation never resets or writes state it does not own.

---

## 11. Dependency-cycle and layering statement (planning rule 8)

Enumerated arcs this plan adds or removes:

- **Zero new module-to-module import arcs (Rust)**: `task_set_title_at` is a new function inside the existing `commands::task` module and uses only identifiers already imported/used by sibling functions in that file (`AppHandle`, `State`, `SettingsState`, `validate_wg_root`, `task_ops::perform`, `TaskOp`, `EditOutcome`, `TaskUpdateResult`, `strip_unc`, `emit_task_updated`, `log`). The `lib.rs` registration adds one entry to the existing `generate_handler!` list — the `agentscommander_lib -> commands::task` arc already exists (lines 2910/2912). No new `use` statements anywhere.
- **Zero new module-to-module import arcs (TypeScript)**: `TaskAPI.setTitleAt` is a new member on the existing `TaskAPI` object in `src/shared/ipc.ts` (no import change); `ProjectPanel.tsx` already imports `TaskAPI` (line 4); new CSS classes are not module arcs; test additions live inside existing test modules.
- **Per-arc classification**: there are no new arcs to classify. The pre-existing single cyclic SCC (measured: `summary.moduleCycles: 1`, an 85-module SCC; `commands::task` is not a member) cannot change because no arc is added or removed, so its member sets are identical by construction.
- **Measurement**: baseline run at `e9365c2` on a clean tree produced `modulesResolved: 191`, `moduleEdges: 3683`, `moduleCycles: 1`, and a regenerated `module-arcs.txt` **byte-identical** to the committed record. Acceptance criterion 9.4.4 requires the implementer to repeat the measurement on the final tree and prove identity.
- **Role/layering hygiene**: `task_set_title_at` lives in `commands/task.rs`, the same transport-taking layer that already owns `task_set_title` and `task_clean_at` (both take `AppHandle` and `State<SettingsState>`). No lower layer (persistence/CLI) gains a UI-transport dependency; `task_ops::perform`/`TaskOp` are untouched. Frontend side adds no new consumer layering (ProjectPanel already consumes TaskAPI). No role inversion.

---

## 12. Implementation order

1. **ac-dev-rust-v3** (backend, independent of frontend): extract `validate_user_title`; add `task_set_title_at`; register in `lib.rs`; add unit tests (9.1); run `cargo test` targeted + the dependency-cycle criterion (9.4.4) on their commits.
2. **ac-dev-webpage-ui-v3** (frontend, independent of backend via FakeTransport): `TaskAPI.setTitleAt` in `src/shared/ipc.ts` (or await the backend commit and include it there — single-commit preference); ProjectPanel signals/handlers/JSX (5.3.1-5.3.5); sidebar.css classes (5.3.6); tests (9.2); docs (9.3).
3. **Integration**: both sides on `feature/1536-sidebar-edit-task-title`; full targeted suites + lint/type checks; repeat the cycle measurement; confirm acceptance criteria 9.4.1-9.4.5; force-add this plan (`git add -f plans/1536-sidebar-edit-task-title.md`) and commit.

---

*Authoring note: all line numbers and quoted code refer to `e9365c2`. Code sketches in section 5 are normative in behavior and in identifier/class/argument names; whitespace and comment placement may vary with local style.*
