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

- Toggle: `coordSortByActivity` — `src/sidebar/components/ActionBar.tsx:296-303` (button/tooltip), `src/sidebar/stores/sessions.ts:37` (state), `:563-565` (getter), `:614`, `:620-633` (toggle + persist), `src/shared/types.ts:646`, `:1215`, Rust `src-tauri/src/config/settings.rs` (no change needed), hydration `src/sidebar/App.tsx:647`.
- Re-sort: `src/sidebar/components/ProjectPanel.tsx` `naturalCoordinatorItems` (`:1880-1906`, sorts by `sessionsStore.lastActivityBySessionId[session.id]` desc when the toggle is on) and `coordinatorItems` (`:1907-1921`, consults `sessionsStore.coordinatorVisibleOrder(proj.path, keys)` when on, records via `recordCoordinatorVisibleOrder`).
- Existing partial freeze: `src/sidebar/stores/sessions.ts:13` `sidebarPointerInside` signal; `:14-15` `lastCoordinatorVisibleOrderByProject` / `frozenCoordinatorVisibleOrderByProject`; `:586-594` `setSidebarPointerInside` (snapshot on enter, clear on leave); `:596-606` `coordinatorVisibleOrder` (returns `nextKeys` unchanged when `!sidebarPointerInside()`, else reconciles against the frozen keys); `:608-611` `recordCoordinatorVisibleOrder`; `src/sidebar/stores/sessions-helpers.ts:118-137` `reconcileVisibleOrderKeys` (frozen order kept for surviving keys, disappeared keys dropped, new keys appended).
- Wiring: `src/sidebar/App.tsx:855-856` `onPointerEnter`/`onPointerLeave` on the `.sidebar-layout` div, `:843` reset in `onCleanup`.
- Root cause: every sidebar context menu is portalled to `document.body` (`Portal` imported at `ProjectPanel.tsx:2`; menu portals at `:2588`, `:2779`, `:2902`, `:3050`, `:3084`, `:3105`, `:3279`, `:3305`, `:3342`, `:3369`, `:3438`, `:3486` and in `SessionItem.tsx:498-603`, `WorkgroupGroupRail.tsx:847-879`, `RootAgentBanner.tsx:564-615`, `AcDiscoveryPanel.tsx:320-357`). The menu node is NOT a descendant of `.sidebar-layout`, so hovering it fires `pointerleave` → `setSidebarPointerInside(false)` → frozen map cleared → re-sort while the menu (positioned at right-click client coords, e.g. `ProjectPanel.tsx:624`, `:1853`) sits over a different row than the one it operates on.
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
- `src/shared/testing/ui-harness.tsx` — one added test-isolation reset line.
- Tests: `src/sidebar/stores/sessions-helpers.test.ts` (extend), new `src/sidebar/components/ProjectPanel.order-lock.test.tsx`, new `src/sidebar/App.order-lock.test.tsx`.

**In scope for the lock selector:** every element whose class is `session-context-menu` or `session-context-flyout` under `document.body` in the sidebar document. Flyouts (group flyout `ProjectPanel.tsx:1530` class `session-context-flyout`; repo browse flyout `:1596`) are included because they are context-menu UI with the same pointer-anchored misalignment risk; today they only exist while their parent menu is open, so this is defensive but harmless (presence-based boolean).

**Out of scope:**
- Modals (`modal-overlay`/`agent-modal` overlays: delete confirmations, entity-creation modals, `AgentPickerModal`, `OpenAgentModal`, `ArchivedProjectsModal`, `WorkgroupGroupsModal`, `OnboardingModal`): they are centered full-window overlays, not pointer-anchored to a row; a reorder behind an opaque overlay cannot misalign the user's intent, and the requirement names context menus. A modal opened from a menu action releases the lock when the menu closes — that is the intended behavior. (Test 4 in the ProjectPanel suite pins this decision.)
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

In `resetUiStoresForTests` (after `:281`) add: `sessionsStore.setSidebarMenuOpen(false);` (test isolation; the observer keeps the signal in sync with the DOM within one microtask).

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
2. `src/sidebar/App.tsx` — add the `onCleanup` reset line.
3. `src/shared/testing/ui-harness.tsx` — add the reset line in `resetUiStoresForTests`.
4. `src/sidebar/stores/sessions-helpers.test.ts` — add the store-level `"sidebar menu-open order lock"` describe block.
5. New `src/sidebar/components/ProjectPanel.order-lock.test.tsx` — integration suite (uses `renderWithFakeTransport`, `contextMenu`, `click`, `waitFor`, `resetUiStoresForTests`, fake timers for activity timestamps).
6. New `src/sidebar/App.order-lock.test.tsx` — end-to-end regression of the exact bug (full `<SidebarApp/>`, `pointerenter`/`pointerleave` on `.sidebar-layout`).
7. Verify: `npx vitest run src/sidebar/stores/sessions-helpers.test.ts src/sidebar/components/ProjectPanel.order-lock.test.tsx src/sidebar/App.order-lock.test.tsx`, then `npm test` (full), then `npm run typecheck`. Commit on the branch.

## Tests

### Store level — extend `src/sidebar/stores/sessions-helpers.test.ts` (new describe `"sidebar menu-open order lock"`)

Direct `setSidebarMenuOpen` calls (the observer is not exercised here; jsdom is active but no menus are mounted). Assertions:

1. "holds the frozen order while a menu is open with the pointer outside": record `[a,b,c]`; `setSidebarMenuOpen(true)`; `coordinatorVisibleOrder(path, ["c","a","b"])` → `["a","b","c"]`.
2. "releases when the menu closes": continue; `setSidebarMenuOpen(false)`; `coordinatorVisibleOrder(path, ["c","a","b"])` → `["c","a","b"]`.
3. "pointer-leave while a menu is open keeps the freeze": `setSidebarPointerInside(true)`; `setSidebarMenuOpen(true)`; `setSidebarPointerInside(false)` → still frozen; `setSidebarMenuOpen(false)` → released.
4. "drops disappeared and appends new coordinators while menu-locked": mirror of the existing hover structural-change test, with `setSidebarMenuOpen(true)` instead of pointer.
5. "re-snapshots the last visible order when the lock re-engages": record `[a,b,c]`; lock on; recompute `[c,a,b]` → `[a,b,c]`; lock off; recompute `[c,a,b]` (recorded as natural); lock on; recompute `[b,c,a]` → `[c,a,b]` (the re-engaged snapshot wins).

### Integration — new `src/sidebar/components/ProjectPanel.order-lock.test.tsx`

Setup (pattern of `ProjectPanel.context-menu-hover.test.tsx`): one project, two workgroups each with one coordinator (`coord-a`, `coord-b`) and live sessions; `sessionsStore.setCoordSortByActivity(true)`; `sessionsStore.setSidebarPointerInside(false)` before opening menus. Order is read from `[data-ac-testid^="replica.row.quick."]` document order. Timestamps: `vi.useFakeTimers()` + `vi.advanceTimersByTime(1000)` between `sessionsStore.markActivity(...)` calls; flush observer microtasks with `await Promise.resolve()`; all `waitFor` before enabling fake timers (pattern of the hover suite). Assertions:

1. "keeps the coordinator order frozen while a context menu is open even with the pointer outside": mark A; open the coordinator menu (right-click); wait menu node; flush; `setSidebarPointerInside(false)`; mark B (newer); flush → quick order still `[coord-a, coord-b]` and `sessionsStore.sidebarMenuOpen === true`.
2. "releases on Escape": from (1), `window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`; wait menu gone; mark B again (newer); flush → order `[coord-b, coord-a]`.
3. "releases on an outside click": open menu; click `document.body`; wait menu gone; mark B → order flips.
4. "releases when a menu action closes the menu and opens a modal (modals are out of scope)": open the coordinator menu; click "Coding Agent" (closes the menu, opens `AgentPickerModal` — a portal under body, NOT `.session-context-menu`); assert menu gone, modal present, `sessionsStore.sidebarMenuOpen === false`; mark B → order flips while the modal is open.
5. "keeps the lock across menu replacement": open the coordinator menu; right-click the project header row (project menu replaces it); assert the new menu's labels; order still frozen; Escape → released.
6. "releases when the project is removed while a menu is open": open the coordinator menu; `projectStore.removeProject(projectPath)`; wait panel gone; flush → `sessionsStore.sidebarMenuOpen === false`.

### End-to-end — new `src/sidebar/App.order-lock.test.tsx`

Setup (pattern of `src/sidebar/App.messaging.workflow.test.tsx`): render `<SidebarApp/>` with fake transport resolving `get_settings` (projectPaths), `open_project`, `discover_project` (two workgroups × one coordinator), `get_project_groups`, `search_repos`, `list_sessions` (both coordinators live), `get_active_session`, `list_detached_sessions`; `sessionsStore.setCoordSortByActivity(true)`. `const layout = target("sidebar.root")` (the `.sidebar-layout` div). jsdom 25 has no `PointerEvent` ctor — dispatch `layout.dispatchEvent(new Event("pointerenter"))` / `new Event("pointerleave")` (Solid's `onPointerEnter/Leave` are plain `pointerenter/pointerleave` listeners). Assertions:

1. "does not reorder when the pointer moves from a tile onto its open context menu" (the exact #1624 repro): mark A; pointerenter on layout; right-click the coord-a row; wait menu; pointerleave on layout; assert `sessionsStore.sidebarPointerInside === false` while the menu is open; fake timers; mark B; flush → quick order still `[coord-a, coord-b]`; Escape; wait menu gone; mark B again; flush → order `[coord-b, coord-a]`.
2. "does not reorder while a menu is open with the pointer outside, and releases on an outside click": right-click the coord-a row; pointerleave on layout; mark B → frozen; click `document.body`; wait menu gone; mark B → order flips.

Untouched on purpose: `App.context-menu.test.ts` (global `blockContextMenu` blocker), `ActionBar.test.ts`, `ProjectPanel.context-menu-hover.test.tsx`, `ProjectPanel.context-menu.test.tsx`, `ProjectPanel.repo-browse.test.tsx` — they must keep passing unchanged (regression gate).

## Objective acceptance criteria

1. New suites and extended store suite pass: `npx vitest run src/sidebar/stores/sessions-helpers.test.ts src/sidebar/components/ProjectPanel.order-lock.test.tsx src/sidebar/App.order-lock.test.tsx`.
2. Full `npm test` (vitest run) green, including the untouched sidebar suites named above.
3. `npm run typecheck` clean.
4. Manual verification: enable "Show recent orchestrators first"; (a) hover the sidebar — order frozen; (b) right-click any coordinator tile and move the pointer onto the open menu — rows do not reorder and the menu stays aligned with its row; (c) close via Escape, outside click, or a menu action — order is free again; (d) right-click a tile and move the pointer into the main window while the menu is open — order still frozen until the menu closes; (e) add/remove a coordinator while a menu is open — frozen rows keep their places, new one appends.
