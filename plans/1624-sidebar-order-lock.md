# #1624 — Sidebar tile order lock: extend the hover freeze to open context menus

Status: READY_FOR_IMPLEMENTATION

Issue: `#1624` — "Sidebar: no tile reorder while pointer is over the sidebar or while any sidebar context menu is open" (Spanish original: no order change when mouse-over is over the sidebar OR when a sidebar context menu is open; the reorder comes from the "Show recent orchestrators first" toggle).
Repository: `repo-AgentsCommander`
Branch: `fix/1624-sidebar-order-lock-context-menu` (already created, checked out; base `main` = `df494bfa04f7e14fa9a42f3b0d89ccbc2ce76e80`, working tree clean)
Delivery: Full
Owning implementer: `ac-dev-webpage-ui-v3`

## Objective

While the pointer is inside the sidebar **or** while any sidebar context menu (or its submenu flyout) is open, the visible order of the coordinator tiles must not change, even if activity events arrive. Today the pointer half works; the open-menu half does not: every context menu renders through a Solid `<Portal>` mounted under `document.body`, so moving the pointer from a tile onto the open menu fires `pointerleave` on `.sidebar-layout`, the freeze is dropped, and the coordinator rows re-sort under the still-open, still-pointer-anchored menu — the reported bug.

The fix must be **leak-proof**: a menu closed by click-outside, by `Escape`, by selecting an action, by menu replacement, by the grace-timer close, or by component unmount must all release the lock; and the lock must never stay stuck with no menu on screen. No persistence, IPC, or Rust change.

## Cause (verified evidence)

- Toggle: `coordSortByActivity` — `src/sidebar/components/ActionBar.tsx:296-303` (button/tooltip/`aria-pressed`), `src/sidebar/stores/sessions.ts:37` (state), `:563-565` (getter), `:613-614` + `:620-633` (setter; toggle + persist), `src/shared/types.ts:646`, `:1215`, Rust `src-tauri/src/config/settings.rs` (no change needed), hydration `src/sidebar/App.tsx:647`.
- Re-sort: `src/sidebar/components/ProjectPanel.tsx` `naturalCoordinatorItems` (`:1880-1906`, sorts by `sessionsStore.lastActivityBySessionId[session.id]` desc when the toggle is on) and `coordinatorItems` (`:1908-1921`, consults `sessionsStore.coordinatorVisibleOrder(proj.path, keys)` when on, records via `recordCoordinatorVisibleOrder`).
- Existing partial freeze: `src/sidebar/stores/sessions.ts:13` `sidebarPointerInside` signal; `:14-15` `lastCoordinatorVisibleOrderByProject` / `frozenCoordinatorVisibleOrderByProject`; `:586-594` `setSidebarPointerInside` (snapshot on enter, clear on leave); `:596-606` `coordinatorVisibleOrder` (returns `nextKeys` unchanged when `!sidebarPointerInside()`, else reconciles against the frozen keys; note it never records — recording happens only in `ProjectPanel.coordinatorItems`); `:608-611` `recordCoordinatorVisibleOrder`; `src/sidebar/stores/sessions-helpers.ts:118-137` `reconcileVisibleOrderKeys` (frozen order kept for surviving keys, disappeared keys dropped, new keys appended).
- Wiring: `src/sidebar/App.tsx:854-856` — `class="sidebar-layout"` div with `onPointerEnter`/`onPointerLeave` (`:855-856`) and `data-ac-testid="sidebar.root"` (`:857`); `:843` reset in `onCleanup`.
- Root cause: every sidebar context menu is portalled to `document.body` (`Portal` imported at `ProjectPanel.tsx:2`). The `session-context-menu` nodes are the class two lines below each `<Portal>` at `ProjectPanel.tsx:2588`, `:2779`, `:2902`, `:3050`, `:3084`, `:3279`, `:3305`, `:3342`, `:3369`, `:3492`, and inside the portals at `SessionItem.tsx:523` (portals `:498-603`), `WorkgroupGroupRail.tsx:850` (portal `:847-879`), `RootAgentBanner.tsx:578` (portals `:564-615`), `AcDiscoveryPanel.tsx:322` (portal `:320-357`). Not part of that inventory: the portals at `ProjectPanel.tsx:3105` (Delete agent confirmation) and `:3443` (Delete Loop confirmation) render `modal-overlay`/`agent-modal` delete dialogs, not context menus. The menu node is NOT a descendant of `.sidebar-layout`, so hovering it fires `pointerleave` → `setSidebarPointerInside(false)` → frozen map cleared → re-sort while the menu (positioned at right-click client coords, e.g. `ProjectPanel.tsx:1854` for the project menu and `:2107` for the replica menu) sits over a different row than the one it operates on.
- Timer-driven close paths (pinned by the new grace-timer tests): `scheduleReplicaCtxMenuClose` at `ProjectPanel.tsx:1117-1125` with `CONTEXT_MENU_CLOSE_GRACE_MS = 250` at `:183`; `scheduleGroupFlyoutClose` at `:889-896` (180 ms). These are the only timer-driven menu/flyout close paths; the new tests arm both.
- Confirmed scope check: `coordSortByActivity` / `lastActivityBySessionId` / `sort(` appear only in `ProjectPanel.tsx` (plus `ActionBar.tsx`, `App.tsx`, stores, settings); `WorkgroupGroupRail.tsx`, `SessionItem.tsx`, `RootAgentBanner.tsx`, `AcDiscoveryPanel.tsx` have no activity-sort references. The reorder is confined to the coordinator quick-access tiles in `ProjectPanel`.

## Decision — one implementation

**Chosen: a DOM-derived open-menu lock.** Add a second lock source `sidebarMenuOpen` to the sessions store, fed by a module-level `MutationObserver` on `document.body` that recomputes `document.querySelector(".session-context-menu, .session-context-flyout") !== null` on every childList mutation. The combined interaction lock is `sidebarPointerInside() || sidebarMenuOpen()`; the frozen snapshot is captured when the combined lock turns on and dropped only when it turns fully off (pointer outside AND no menu/flyout node in the DOM).

Why this dominates the alternatives the lead raised:

- **(A) central open-menu counter (acquire/release):** rejected — every open/close path in 5 components and ~15 menus would need explicit acquire/release calls; a missed path leaks the lock permanently (order frozen forever) and a double-close desyncs the count. Exactly the fragility the task warns about.
- **(B) central registry keyed by menu id:** rejected — idempotent close fixes desync but still requires touching every open/close path in all 5 files (dismiss listeners, option clicks, sibling nulling, grace timers, unmount cleanups), and a new menu added later without registration silently re-introduces the bug.
- **(C) derived memo over the existing per-menu signals:** rejected — the menu signals are component-instance-scoped (11 in `ProjectPanel` per project instance, plus one each in `SessionItem`, `WorkgroupGroupRail`, `RootAgentBanner`, `AcDiscoveryPanel`); exposing them through the store means ~15 registration effects that must be kept in sync forever; a missed signal = silent recurrence.
- **(D) portal-aware pointer model (pointer over `.session-context-menu` counts as inside):** rejected — it only covers "pointer over menu". The requirement is absolute: *any* open sidebar context menu ⇒ no reorder, including when the pointer is outside both the sidebar and the menu (e.g. user right-clicks a tile, then moves to the main window to read; the open menu must keep the freeze).
- **(E, chosen) DOM-derived presence lock:** the lock derives from observable reality, so **every** close path releases it structurally (close = node removed = observer recomputes false) and no path can leave it stuck while a menu is visible. Zero per-menu wiring: automatically covers all five components, both flyout classes, menu replacement (in-place object rewrite keeps the node → lock continuity), the 180/250 ms grace timers, and any future menu component. There is no counter to desync and no registration to forget.

Synchronous-safety note: Solid mounts/unmounts the portal node synchronously inside the menu open/close task; the observer callback runs in the next microtask, before any activity event (IPC task) can interleave, so a recompute can never observe a menu present with a stale lock, nor a missing menu with a stale hold. A stale-`true` observer reading is harmless (it is recomputed on the very next mutation).

## Scope

**In scope (the lock sources and their consumers):**
- `src/sidebar/stores/sessions.ts` — lock core (see below).
- `src/sidebar/App.tsx` — one added cleanup line.
- `src/shared/testing/ui-harness.tsx` — two added test-isolation reset lines.
- Tests: `src/sidebar/stores/sessions-helpers.test.ts` (extend), new `src/sidebar/components/ProjectPanel.order-lock.test.tsx`, new `src/sidebar/App.order-lock.test.tsx`.

**In scope for the lock selector:** every element whose class is `session-context-menu` or `session-context-flyout` under `document.body` in the sidebar document. Flyouts (group flyout portal `ProjectPanel.tsx:1413`, node class at `:1416`; repo-browse flyout portal `:1586`, node class at `:1590`) are included because they are context-menu UI with the same pointer-anchored misalignment risk; today they only exist while their parent menu is open, so this is defensive but harmless (presence-based boolean).

**Out of scope:**
- Modals (`modal-overlay`/`agent-modal` overlays: the delete confirmations at `ProjectPanel.tsx:3105` and `:3443`, entity-creation modals, `AgentPickerModal` (root `class="modal-overlay"` at `AgentPickerModal.tsx:578`), `OpenAgentModal`, `ArchivedProjectsModal`, `WorkgroupGroupsModal`, `OnboardingModal`): they are centered full-window overlays, not pointer-anchored to a row; a reorder behind an opaque overlay cannot misalign the user's intent, and the requirement names context menus. A modal opened from a menu action releases the lock when the menu closes — that is the intended behavior. (Test 4 in the ProjectPanel suite pins this decision.)
- The pointer model (`sidebarPointerInside`, `.sidebar-layout` handlers) — unchanged.
- The `coordSortByActivity` toggle, settings persistence, Rust, IPC, CSS — unchanged.

## Affected files and exact symbols

### 1. `src/sidebar/stores/sessions.ts` (core change)

Module scope, next to the existing freeze signals (`:13-15`):

```ts
const [sidebarMenuOpen, setSidebarMenuOpen] = createSignal(false);
let sidebarOrderLockActive = false;
let sidebarMenuLockObserverInstalled = false;

function refreshSidebarOrderLock(): void {
  const active = sidebarPointerInside() || sidebarMenuOpen();
  if (active === sidebarOrderLockActive) return;
  sidebarOrderLockActive = active;
  if (active) {
    setFrozenCoordinatorVisibleOrderByProject(lastCoordinatorVisibleOrderByProject());
  } else {
    setFrozenCoordinatorVisibleOrderByProject({});
  }
}

function updateSidebarMenuOpen(value: boolean): void {
  if (value === sidebarMenuOpen()) return;
  setSidebarMenuOpen(value);
  refreshSidebarOrderLock();
}

function installSidebarMenuLockObserver(): void {
  if (sidebarMenuLockObserverInstalled) return;
  if (typeof document === "undefined" || !document.body) return; // node-env unit tests
  sidebarMenuLockObserverInstalled = true;
  new MutationObserver(() => {
    updateSidebarMenuOpen(
      document.querySelector(".session-context-menu, .session-context-flyout") !== null
    );
  }).observe(document.body, { childList: true, subtree: true });
}
installSidebarMenuLockObserver();
```

Store object changes:

- `setSidebarPointerInside` (`:586-594`): keep the early-return and the signal write, but replace the inline snapshot/clear with `refreshSidebarOrderLock();` — the clear now only happens when the **combined** lock releases, never while a menu is open.
- New store method (same shadowing pattern as the existing `setSidebarPointerInside`):

```ts
setSidebarMenuOpen(value: boolean) {
  updateSidebarMenuOpen(value);
},
```

- New getter next to `get sidebarPointerInside()` (`:578-579`):

```ts
get sidebarMenuOpen() {
  return sidebarMenuOpen();
},
```

- `coordinatorVisibleOrder` (`:596-606`): change the guard to `if (!sidebarPointerInside() && !sidebarMenuOpen()) return nextKeys;` — everything else (frozen-map fallback, `reconcileVisibleOrderKeys`, frozen-map refresh inside) unchanged.
- No change to `recordCoordinatorVisibleOrder`, `reconcileVisibleOrderKeys`, `setCoordSortByActivity`, `toggleCoordSortByActivity`.

### 2. `src/sidebar/App.tsx`

In `onCleanup` next to `:843` add: `sessionsStore.setSidebarMenuOpen(false);` (deterministic teardown; the observer self-heals on DOM teardown anyway). No other change.

### 3. `src/shared/testing/ui-harness.tsx`

`resetUiStoresForTests` (function declared at `:267`) currently resets `coordSortByActivity` and the other module-level signals but never resets `sidebarPointerInside`. The plan's new `setSidebarMenuOpen(false)` alone is not enough: a leaked `sidebarPointerInside === true` keeps the combined lock active, `setFrozenCoordinatorVisibleOrderByProject({})` never runs, and the frozen map survives into the next test in the same file. Add BOTH lines in `resetUiStoresForTests` next to the existing `sessionsStore.setCoordSortByActivity(false);`:

```ts
sessionsStore.setSidebarPointerInside(false);
sessionsStore.setSidebarMenuOpen(false);
```

(Order matters for intent, not for the final state: the pointer line alone cannot release while a menu flag leaked, and the menu line alone cannot release while a pointer flag leaked — both together guarantee the combined lock is off and the frozen map is cleared.)

### 4. Tests — see "Tests" below.

## Required behavior, edge cases, failure behavior

1. **The reported bug:** pointer leaves `.sidebar-layout` onto an open menu → `setSidebarPointerInside(false)` runs, but the menu node is in the DOM → `sidebarMenuOpen` is true → combined lock stays on → frozen map retained → `coordinatorVisibleOrder` keeps the frozen order.
2. **Release semantics:** the frozen snapshot is captured on the combined-lock off→on transition (either pointer enter or menu/flyout first appearing) and dropped only on the on→off transition (pointer outside **and** no menu/flyout node in the DOM). `setSidebarPointerInside(false)` with a menu open is a no-op on the freeze; `setSidebarMenuOpen(false)` with the pointer inside is a no-op on the freeze.
3. **Menu opened while pointer inside:** lock already on via pointer; no re-snapshot (the snapshot from pointer-enter is the order at interaction start — correct).
4. **Menu opened while pointer outside** (automation/keyboard): off→on transition snapshots `lastCoordinatorVisibleOrderByProject()` — the last rendered order; first-ever render falls back to `?? nextKeys` inside `coordinatorVisibleOrder` (unchanged).
5. **Menu replacement:** in-place object rewrite (`#943` replica-menu reclamp) keeps the same DOM node → no mutation → lock continuity. Sibling-nulling replacement (e.g. project menu over replica menu) removes one node and inserts another in the same task → observer recomputes presence → still locked. No gap.
6. **Coordinators added/removed while frozen:** unchanged `reconcileVisibleOrderKeys` semantics (surviving keys keep frozen positions, disappeared keys drop, new keys append) — the menu lock flows through the same `coordinatorVisibleOrder` path as the hover lock.
7. **Toggle off while locked:** the `coordSortByActivity` branch is skipped; natural order is recorded; no freeze interaction (unchanged).
8. **Two menus open at once** (different project instances): presence boolean, no counting — locked while ≥1 node exists.
9. **Failure behavior:** if `document`/`document.body` is unavailable at import (node-env unit tests), the observer is skipped and the signal stays `false` — behavior degrades to today's pointer-only freeze; no crash. If a menu were ever kept in the DOM without being visible, the lock stays on — that is the conservative, correct direction (never reorder under a possibly-visible menu).
10. **Performance:** the observer fires on childList mutations anywhere under `document.body` (session rows, portals). The callback is one `querySelector` + one boolean write; no counting, no per-node work. Accepted; no throttling needed for a sidebar of this size.
11. **Observer selector rationale:** `childList: true, subtree: true` only — all menus are conditionally mounted (insert/remove = childList mutations) and their `session-context-menu`/`session-context-flyout` classes are static, set at element creation before insertion. Omitting `attributes` avoids callback churn from unrelated class toggles (collapse states, badges) elsewhere in the body.

## Compatibility impact

None. No settings/persistence/Rust/IPC/CSS changes; no new dependencies. Existing behavior for the toggle and the hover freeze is preserved (verified: the store-level hover tests keep passing under the new transition-based snapshot logic, because `setSidebarPointerInside` still early-returns on unchanged values). The `test-debt.allowlist.json` is untouched.

## Ordered implementation

1. `src/sidebar/stores/sessions.ts` — add the signals/`refreshSidebarOrderLock`/`updateSidebarMenuOpen`/observer; rewire `setSidebarPointerInside`; add `setSidebarMenuOpen` + `get sidebarMenuOpen`; change the `coordinatorVisibleOrder` guard.
2. `src/sidebar/App.tsx` — add the `onCleanup` reset line next to `:843`.
3. `src/shared/testing/ui-harness.tsx` — add BOTH reset lines (pointer, then menu) in `resetUiStoresForTests` at `:267`.
4. `src/sidebar/stores/sessions-helpers.test.ts` — add the store-level `"sidebar menu-open order lock"` describe block after the hover block (`:289`).
5. New `src/sidebar/components/ProjectPanel.order-lock.test.tsx` — integration suite (uses `renderWithFakeTransport`, `contextMenu`, `click`, `waitFor`, `resetUiStoresForTests`, fake timers for activity timestamps).
6. New `src/sidebar/App.order-lock.test.tsx` — end-to-end regression of the exact bug (full `<SidebarApp/>`, `pointerenter`/`pointerleave` on `.sidebar-layout`).
7. Verify: `npx vitest run src/sidebar/stores/sessions-helpers.test.ts src/sidebar/components/ProjectPanel.order-lock.test.tsx src/sidebar/App.order-lock.test.tsx`, then `npm test` (full), then `npm run typecheck`. Commit on the branch.

## Tests

### Store level — extend `src/sidebar/stores/sessions-helpers.test.ts` (new describe `"sidebar menu-open order lock"`, after the existing `"sidebar coordinator hover freeze"` block at `:289`)

Same isolation contract as the hover block (this file has no `resetUiStoresForTests` in beforeEach — each test starts with the lock released and ends with the lock released). Direct `setSidebarMenuOpen` calls (the observer is not exercised here; jsdom is active but no menus are mounted). Assertions:

1. "holds the frozen order while a menu is open with the pointer outside": `sessionsStore.setSidebarMenuOpen(false)`; `recordCoordinatorVisibleOrder(path, ["coord-a","coord-b","coord-c"])`; `setSidebarMenuOpen(true)`; `coordinatorVisibleOrder(path, ["coord-c","coord-a","coord-b"])` → `["coord-a","coord-b","coord-c"]`.
2. "releases when the menu closes": continue; `setSidebarMenuOpen(false)`; `coordinatorVisibleOrder(path, ["coord-c","coord-a","coord-b"])` → `["coord-c","coord-a","coord-b"]`.
3. "pointer-leave while a menu is open keeps the freeze": `setSidebarPointerInside(true)`; `setSidebarMenuOpen(true)`; `setSidebarPointerInside(false)` → still frozen; `setSidebarMenuOpen(false)` → released.
4. "drops disappeared and appends new coordinators while menu-locked": mirror of the existing hover structural-change test (`:321-331`), with `setSidebarMenuOpen(true)` instead of the pointer; ends with `setSidebarMenuOpen(false)`.
5. "re-snapshots the last recorded visible order when the lock re-engages" — `coordinatorVisibleOrder` NEVER records (recording is `ProjectPanel.coordinatorItems`'s job), so the recompute between lock-off and lock-on must be recorded explicitly or the re-engaged snapshot would reuse the stale `[a,b,c]`:

```ts
sessionsStore.setSidebarMenuOpen(false);
sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-a", "coord-b", "coord-c"]);
sessionsStore.setSidebarMenuOpen(true);
expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"])).toEqual([
  "coord-a", "coord-b", "coord-c",
]);
sessionsStore.setSidebarMenuOpen(false);
// Explicit mirror of ProjectPanel.coordinatorItems recording the recomputed
// order once the lock is off; without it, "last" still holds [a,b,c].
sessionsStore.recordCoordinatorVisibleOrder(projectPath, ["coord-c", "coord-a", "coord-b"]);
sessionsStore.setSidebarMenuOpen(true);
expect(sessionsStore.coordinatorVisibleOrder(projectPath, ["coord-b", "coord-c", "coord-a"])).toEqual([
  "coord-c", "coord-a", "coord-b",
]);
sessionsStore.setSidebarMenuOpen(false); // end released — module state stays clean
```

### Integration — new `src/sidebar/components/ProjectPanel.order-lock.test.tsx`

**Environment (explicit):** the file starts with `// @vitest-environment jsdom` (repo default is node — `vitest.config.ts:12`). beforeEach: `cleanupDom = installBrowserDomStubs(); resetUiStoresForTests();`. afterEach: `rendered?.cleanup(); rendered = null; cleanupDom?.(); cleanupDom = null; resetUiStoresForTests(); document.body.replaceChildren();` (pattern of `ProjectPanel.context-menu-hover.test.tsx:196-210`).

**Fixture** (pattern of `ProjectPanel.context-menu-hover.test.tsx`): one project, two workgroups — `wg-1-dev-team` with coordinator `coord-a` and `wg-2-dev-team` with coordinator `coord-b`, each with a live session (`coord-a-session`, `coord-b-session`); `fake.resolve("new_project", …)`, `fake.resolve("discover_project", …)`, `fake.onInvoke("update_project_groups", …)`, `sessionsStore.setSessions([…])`, `await workgroupGroupsStore.save(projectPath, groups)`, `await projectStore.createAndLoad(projectPath)`, `await waitFor(...)` rendered content. After render: `sessionsStore.setCoordSortByActivity(true); sessionsStore.setSidebarPointerInside(false);`. Order is read from the document order of `rendered.root.querySelectorAll('[data-ac-testid^="replica.row.quick."]')` (quick-row testid pattern built at `ProjectPanel.tsx:2202`; see also `coordRowTestId` in the hover suite at `:47`).

**Local helpers** (pattern of the hover suite): `menu()` = `document.querySelector(".session-context-menu")`; `mouse(el, "mouseenter" | "mouseleave")` dispatching a NON-bubbling `MouseEvent` (Solid binds mouseenter/mouseleave directly — they do not bubble); `flush()` = `await new Promise((r) => setTimeout(r, 0))`; `openCoordinatorMenu()` = `contextMenu(row)`, `await waitFor(() => expect(menu()).not.toBeNull())`, `await flush()` — the flush also runs the menu's `setTimeout` that registers the window dismiss listeners and positions the menu (`ProjectPanel.tsx:2117-2122`), which the Escape/click releases rely on.

**Timer discipline (mandatory):**
- Every `waitFor` runs under REAL timers — fake timers freeze `Date.now()`, so `waitFor`'s polling never advances. All `waitFor` (menu/flyout/panel presence or absence) therefore happens before `vi.useFakeTimers()` or after `vi.useRealTimers()`; under fake timers, close/mount state is asserted synchronously (`expect(menu()).toBeNull()` etc.) after the relevant dispatch/advance.
- All `sessionsStore.markActivity(...)` calls run inside ONE `vi.useFakeTimers()` session per test (`try { ... } finally { vi.useRealTimers(); }`), with `vi.advanceTimersByTime(1000)` between them: `markActivity` stamps `performance.now()` (`sessions.ts:637`), the fake clock's epoch is not anchored to real time, and re-installing fake timers mid-test can make a later timestamp tie or precede an earlier one. Within one session, `advanceTimersByTime` makes timestamps strictly increasing (verified against vitest 4.1.5).
- Menu/observer state changes are synchronous or microtask-queued: after every open/close/timer-advance, `await Promise.resolve()` flushes the `MutationObserver` callback (Solid mounts/unmounts the portal node synchronously; the observer callback is a microtask). NEVER use the macrotask `flush()` while fake timers are active — its `setTimeout(0)` would be queued on the fake clock and never fire. `flush()` is only for real-timer phases (draining the dismiss-listener registration).
- Releases that need the window dismiss listeners (Escape, outside click) dispatch AFTER `openCoordinatorMenu()` already ran its real-timer flush; the dispatch itself works under fake timers — a registered listener runs on event dispatch, only its registration used a timer. A menu opened directly under fake timers never gets those listeners (the registration `setTimeout` is queued but never fires), so never rely on Escape/click for a menu that was opened under fake timers.

Assertions:

1. "keeps the coordinator order frozen while a context menu is open even with the pointer outside": `openCoordinatorMenu()` (real timers); `vi.useFakeTimers()`; `markActivity("coord-a-session")`; advance 1000; `sessionsStore.setSidebarPointerInside(false)`; `markActivity("coord-b-session")`; advance 1000; `await Promise.resolve()` → quick order still `[coord-a, coord-b]` and `sessionsStore.sidebarMenuOpen === true`. (finally `vi.useRealTimers()`.)

2. "releases on Escape": `openCoordinatorMenu()` (real); fake timers; mark A; advance; mark B; advance; `await Promise.resolve()` → frozen `[coord-a, coord-b]`; `window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`; `await Promise.resolve()` → `menu()` null, `sidebarMenuOpen === false`; mark B again; advance; `await Promise.resolve()` → order `[coord-b, coord-a]`.

3. "releases on an outside click": `openCoordinatorMenu()` (real); fake timers; mark A; advance; mark B; advance; `await Promise.resolve()` → frozen; `document.body.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }))`; `await Promise.resolve()` → `menu()` null, released; mark B; advance; `await Promise.resolve()` → order `[coord-b, coord-a]`.

4. "releases when a menu action closes the menu and opens a modal (modals are out of scope)": `openCoordinatorMenu()` (real); fake timers; mark A; advance; mark B; advance; `await Promise.resolve()` → frozen; `click()` the menu's "Coding Agent" button (the `.session-context-option` whose text is "Coding Agent") — the menu div's `onClick` stops propagation (`ProjectPanel.tsx:3497`), so the window dismiss listener does not fire; the option's own handler closes the menu and opens `AgentPickerModal` (root `class="modal-overlay"`, `AgentPickerModal.tsx:578`); `await Promise.resolve()` → `menu()` null, `document.querySelector(".modal-overlay")` non-null, `sessionsStore.sidebarMenuOpen === false`; mark B; advance; `await Promise.resolve()` → order `[coord-b, coord-a]` while the modal is open.

5. "keeps the lock across menu replacement": `openCoordinatorMenu()` (real); fake timers; mark A; advance 1000; `contextMenu(rendered.root.querySelector(".project-header")!)` (`handleProjectContextMenu`, bound at `ProjectPanel.tsx:2518`; the header has no testid — select by class exactly as `ProjectPanel.context-menu.test.tsx:348` does) — the replica menu node is removed and the project menu node inserted in the same task; `await Promise.resolve()` → a `session-context-menu` node is still present with the project menu's labels and `sidebarMenuOpen === true`; mark B; advance 1000; `await Promise.resolve()` → order still `[coord-a, coord-b]` (B is newest, so a dropped lock would show `[coord-b, coord-a]` — the lock survived the replacement). Release tail (the replacement menu registers its window dismiss listeners in its own `setTimeout` — `ProjectPanel.tsx:1862` — which was queued under fake timers and is lost on `useRealTimers`, so re-open it under real timers to re-register): `vi.useRealTimers()`; `contextMenu(rendered.root.querySelector(".project-header")!)` again; `await flush()`; `window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`; `await waitFor(() => expect(menu()).toBeNull())`; `await Promise.resolve()` → `sessionsStore.sidebarMenuOpen === false`. (No `markActivity` after `useRealTimers` — cross-epoch timestamps are not orderable; the re-sort after release is already covered by tests 2/3/7.)

6. "releases when the project is removed while a menu is open": `openCoordinatorMenu()` (real); fake timers; `await projectStore.removeProject(projectPath)` (`project.ts:612`); `await Promise.resolve()` → panel gone, `menu()` null, `sessionsStore.sidebarMenuOpen === false`.

7. "releases through the replica-menu grace timer (250 ms) with the pointer outside" — arms the only timer-driven menu close: `openCoordinatorMenu()` (real); fake timers; mark A; advance 1000; `sessionsStore.setSidebarPointerInside(false)` (lock held by the menu alone); `mouse(menu()!, "mouseleave")` → arms `scheduleReplicaCtxMenuClose` (250 ms; the co-armed 180 ms `scheduleRepoFlyoutClose` is a no-op with no repo flyout open — `ProjectPanel.tsx:1227-1233`); assert `menu()` still non-null (the close is only scheduled); `vi.advanceTimersByTime(300)`; `await Promise.resolve()` → `menu()` null and `sessionsStore.sidebarMenuOpen === false`; mark B; advance; `await Promise.resolve()` → order `[coord-b, coord-a]`.

8. "releases the group flyout on its 180 ms timer while the parent menu lock holds, then releases the parent on its own timer": `openCoordinatorMenu()` (real); fake timers; mark A; advance 1000; dispatch `mouseenter` on the "Add to Group" trigger `[data-ac-testid="replica.wg-1-dev-team.groups.trigger"]` (testid at `ProjectPanel.tsx:1557`; `onMouseEnter={openGroupFlyout}`) → the flyout mounts; `mouse(flyout, "mouseenter")` (cancels — no timers armed yet); `mouse(flyout, "mouseleave")` → arms `scheduleGroupFlyoutClose` (180 ms) AND `scheduleReplicaCtxMenuClose` (250 ms) (`ProjectPanel.tsx:1423-1426`); `vi.advanceTimersByTime(200)` (past 180, before 250); `await Promise.resolve()` → `[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]` null AND `sessionsStore.sidebarMenuOpen === true` (the parent menu node is still in the DOM); `mouse(menu()!, "mouseleave")` (re-arms the 250 ms parent close — `cancelReplicaCtxMenuClose` drops the pending t0+250); `vi.advanceTimersByTime(300)`; `await Promise.resolve()` → `menu()` null, `sidebarMenuOpen === false`; mark B; advance; `await Promise.resolve()` → order `[coord-b, coord-a]`.

### End-to-end — new `src/sidebar/App.order-lock.test.tsx`

**Environment (explicit):** `// @vitest-environment jsdom`; beforeEach: `cleanupDom = installBrowserDomStubs(); resetUiStoresForTests();`; afterEach: `cleanupDom?.(); cleanupDom = null; resetUiStoresForTests();` (pattern of `App.messaging.workflow.test.tsx`).

**Fixture** (pattern of `App.messaging.workflow.test.tsx`): render `<SidebarApp embedded />` with fake transport resolving `get_settings` (`baseSettings` with `projectPaths: [projectPath]`), `open_project`, `discover_project` (two workgroups × one coordinator each, live), `get_project_groups`, `search_repos`, `list_sessions` (both coordinators live), `get_active_session`, `list_detached_sessions`, and `telegram_list_bridges` (`[]` — `SidebarApp`'s onMount calls `TelegramAPI.listBridges()` unconditionally, `App.tsx:805`). After the content `waitFor`: `sessionsStore.setCoordSortByActivity(true);`.

**Layout node (concrete substitute — `target` does NOT exist):** `target("sidebar.root")` is not exported by `src/shared/testing/ui-harness.tsx` (it exists only as a local helper inside the ProjectPanel suites, e.g. `ProjectPanel.context-menu-hover.test.tsx:129`). Use `const layout = rendered.root.querySelector(".sidebar-layout")!;` — `rendered.root` is the mount div returned by `renderWithFakeTransport` (`ui-harness.tsx:27-40`) and `rendered.root.querySelector(...)` is the established pattern of the sibling App suites (e.g. `App.messaging.workflow.test.tsx:101-105`). The div itself carries `data-ac-testid="sidebar.root"` (`App.tsx:857`) if a document-level query is ever preferred.

**Pointer events:** jsdom 25 has no `PointerEvent` ctor — dispatch `layout.dispatchEvent(new Event("pointerenter"))` / `layout.dispatchEvent(new Event("pointerleave"))` (Solid's `onPointerEnter`/`onPointerLeave` are direct `pointerenter`/`pointerleave` listeners on the div, `App.tsx:855-856`).

**Timer discipline:** identical to the ProjectPanel suite (waitFor under real timers; one fake-timer session per test; open the menu under real timers and `await flush()` so the dismiss listeners are registered before fake timers take over).

Assertions:

1. "does not reorder when the pointer moves from a tile onto its open context menu" (the exact #1624 repro): render; waitFor the coord rows; `contextMenu(coord-a row)`; waitFor menu; `await flush()` (real timers — dismiss listeners registered); `vi.useFakeTimers()`; `markActivity("coord-a-session")`; advance 1000; `layout.dispatchEvent(new Event("pointerenter"))`; `layout.dispatchEvent(new Event("pointerleave"))`; `await Promise.resolve()` → `sessionsStore.sidebarPointerInside === false` while `sessionsStore.sidebarMenuOpen === true` (the #1624 repro state: pointer left the sidebar, menu still open — the lock must hold); `markActivity("coord-b-session")`; advance 1000; `await Promise.resolve()` → quick order still `[coord-a, coord-b]`; `window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`; `await Promise.resolve()` → menu gone, `sidebarMenuOpen === false`; `markActivity("coord-b-session")`; advance 1000; `await Promise.resolve()` → order `[coord-b, coord-a]`. (finally `vi.useRealTimers()`.)

2. "does not reorder while a menu is open with the pointer outside, and releases on an outside click": render; waitFor rows; `contextMenu(coord-a row)`; waitFor menu; `await flush()`; `vi.useFakeTimers()`; mark A; advance; mark B; advance; `await Promise.resolve()` → frozen `[coord-a, coord-b]` (pointer never entered — the lock comes from the menu alone); `document.body.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }))`; `await Promise.resolve()` → menu gone, released; mark B; advance; `await Promise.resolve()` → `[coord-b, coord-a]`. (finally `vi.useRealTimers()`.)

Untouched on purpose: `App.context-menu.test.ts` (global `blockContextMenu` blocker), `ActionBar.test.ts`, `ProjectPanel.context-menu-hover.test.tsx`, `ProjectPanel.context-menu.test.tsx`, `ProjectPanel.repo-browse.test.tsx` — they must keep passing unchanged (regression gate).

## Objective acceptance criteria

1. New suites and extended store suite pass: `npx vitest run src/sidebar/stores/sessions-helpers.test.ts src/sidebar/components/ProjectPanel.order-lock.test.tsx src/sidebar/App.order-lock.test.tsx`.
2. Full `npm test` (vitest run) green, including the untouched sidebar suites named above.
3. `npm run typecheck` clean.
4. Manual verification: enable "Show recent orchestrators first"; (a) hover the sidebar — order frozen; (b) right-click any coordinator tile and move the pointer onto the open menu — rows do not reorder and the menu stays aligned with its row; (c) close via Escape, outside click, or a menu action — order is free again; (d) right-click a tile and move the pointer into the main window while the menu is open — order still frozen until the menu closes; (e) add/remove a coordinator while a menu is open — frozen rows keep their places, new one appends.
