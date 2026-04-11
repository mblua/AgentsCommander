# Plan: Replace coord-quick-item rendering with replica-item

**Branch:** `fix/coord-quick-use-replica-item`
**Status:** READY FOR IMPLEMENTATION

---

## Problem

1. **CSS specificity bug**: Status circles for `pending` and `waiting` states require `.session-item` or `.replica-item` as parent class (sidebar.css:461-465). The coord-quick section uses `.coord-quick-item`, so these status circles get no background color — they're invisible.

2. **Missing functionality**: Coord-quick items have no hover buttons (detach, telegram, close, explorer, voice) and no context menu, unlike the replica-items in the workgroups section.

## Solution

1. Extract the replica-item rendering (ProjectPanel.tsx:525-653) into a local helper function
2. Replace the coord-quick-item JSX (lines 441-474) with a call to this helper, adding a WG-name badge
3. Remove unused CSS classes: `coord-quick-item`, `coord-quick-info`, `coord-quick-name`, `coord-quick-wg`
4. Keep `.coord-quick-access` container and all its per-theme styles (they provide the visual card grouping)

---

## Changes

### 1. ProjectPanel.tsx — Extract `renderReplicaItem` helper function

Define a local function inside the component (inside the scope that has access to stores, `handleReplicaClick`, `handleReplicaContextMenu`, `voiceRecorder`, `bridgesStore`, `settingsStore`, etc.):

```tsx
const renderReplicaItem = (
  replica: AcAgentReplica,
  wg: AcWorkgroup,
  extraBadge?: string
) => {
  const dotClass = () => replicaDotClass(wg, replica);
  const isCoord = () => isReplicaCoordinator(replica, proj.folderName, proj.teams, wg.teamName);
  const rn = () => replicaRepoName(replica) || stripRepoPrefix(wg.repoPath?.replace(/\\/g, "/").split("/").pop() ?? "") || proj.folderName;
  const branchLabel = () => {
    const s = replicaSession(wg, replica);
    if (s?.gitBranch) {
      const name = rn();
      return name && !s.gitBranch.includes("/") ? `${name}/${s.gitBranch}` : s.gitBranch;
    }
    const name = rn();
    return name ? (replica.repoBranch ? `${name}/${replica.repoBranch}` : name) : null;
  };
  const session = () => replicaSession(wg, replica);
  const isLive = () => isSessionLive(session());
  const bridge = () => { const s = session(); return s ? bridgesStore.getBridge(s.id) : undefined; };
  const isRecording = () => { const s = session(); return s ? voiceRecorder.recordingSessionId() === s.id : false; };
  const isProcessing = () => { const s = session(); return s ? voiceRecorder.processingSessionId() === s.id : false; };
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);

  const handleMicClick = (e: MouseEvent) => {
    e.stopPropagation();
    const s = session();
    if (s) voiceRecorder.toggle(s.id);
  };
  const handleCancelRecording = (e: MouseEvent) => {
    e.stopPropagation();
    voiceRecorder.cancel();
  };
  const handleOpenExplorer = async (e: MouseEvent) => {
    e.stopPropagation();
    const s = session();
    try { await WindowAPI.openInExplorer(s ? s.workingDirectory : replica.path); } catch (err) { console.error("Failed to open explorer:", err); }
  };
  const handleDetach = (e: MouseEvent) => {
    e.stopPropagation();
    const s = session();
    if (s) WindowAPI.detach(s.id);
  };
  const handleTelegramClick = async (e: MouseEvent) => {
    e.stopPropagation();
    const s = session();
    if (!s) return;
    const b = bridge();
    if (b) {
      await TelegramAPI.detach(s.id);
    } else {
      const settings = await SettingsAPI.get();
      const bots = settings.telegramBots || [];
      if (bots.length === 1) {
        await TelegramAPI.attach(s.id, bots[0].id);
      } else if (bots.length > 1) {
        setAvailableBots(bots);
        setShowBotMenu(true);
      }
    }
  };
  const handleBotSelect = async (botId: string) => {
    setShowBotMenu(false);
    const s = session();
    if (s) await TelegramAPI.attach(s.id, botId);
  };
  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    const s = session();
    if (s) SessionAPI.destroy(s.id);
  };

  return (
    <div
      class="replica-item"
      onClick={() => handleReplicaClick(replica, wg)}
      onContextMenu={(e) => {
        const s = session();
        if (s && isLive()) handleReplicaContextMenu(e, s.id);
      }}
      title={replica.path}
    >
      <div class={`session-item-status ${dotClass()}`} />
      <div class="replica-item-info">
        <span class="replica-item-name">{replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name}</span>
        <div class="ac-discovery-badges">
          <Show when={branchLabel()}>
            <span class="ac-discovery-badge branch">{branchLabel()}</span>
          </Show>
          <Show when={isCoord()}>
            <span class="ac-discovery-badge coord">coordinator</span>
          </Show>
          <Show when={extraBadge}>
            <span class="ac-discovery-badge team">{extraBadge}</span>
          </Show>
        </div>
      </div>
      <Show when={isLive()}>
        <Show when={settingsStore.voiceEnabled}>
          <Show when={isRecording()}>
            <button class="session-item-mic-cancel" onClick={handleCancelRecording} title="Cancel recording">&#x2715;</button>
          </Show>
          <button
            class={`session-item-mic ${isRecording() ? "recording" : ""} ${isProcessing() ? "processing" : ""} ${voiceRecorder.micError() ? "error" : ""}`}
            onClick={handleMicClick}
            title={isRecording() ? "Stop recording" : isProcessing() ? "Transcribing..." : voiceRecorder.micError() ? voiceRecorder.micError()! : "Voice to text"}
          >&#x1F399;</button>
        </Show>
        <button class="session-item-explorer" onClick={handleOpenExplorer} title="Open folder in explorer">&#x1F4C2;</button>
        <button class="session-item-detach" onClick={handleDetach} title="Detach to own window">&#x29C9;</button>
        <Show when={bridge()}>
          <div class="session-item-bridge-dot" style={{ background: bridge()!.color }} title={`Telegram: ${bridge()!.botLabel}`} />
        </Show>
        <button
          class={`session-item-telegram ${bridge() ? "active" : ""}`}
          onClick={handleTelegramClick}
          title={bridge() ? "Detach Telegram" : "Attach Telegram"}
          style={bridge() ? { color: bridge()!.color } : {}}
        >T</button>
        <Show when={showBotMenu()}>
          <div class="session-item-bot-menu" onClick={(e) => e.stopPropagation()}>
            <For each={availableBots()}>
              {(bot) => (
                <button class="session-item-bot-option" onClick={() => handleBotSelect(bot.id)}>
                  <span class="settings-color-dot" style={{ background: bot.color }} />
                  {bot.label}
                </button>
              )}
            </For>
          </div>
        </Show>
        <button class="session-item-close" onClick={handleClose} title="Close session">&#x2715;</button>
      </Show>
    </div>
  );
};
```

**Placement (CRITICAL)**: Define this function inside the `{(proj) => {` callback of `<For each={projectStore.projects}>` (between lines ~222 and ~339, after `activeReplicas` and before the `return` at line 340). It MUST be inside this scope because:
- `proj` is only available inside this callback (needed for `proj.folderName`, `proj.teams`)
- `handleReplicaContextMenu` is defined at line 317, inside this same per-project scope
- `handleReplicaClick` is at component level (line 90) and accessible from here via closure
- Module-level imports (`voiceRecorder`, `bridgesStore`, `settingsStore`) are accessible everywhere

Placing it at the `ProjectPanel` component level (before the `return` at line 183) would FAIL because `proj` and `handleReplicaContextMenu` would be out of scope.

**Key difference from the workgroups version**: The `extraBadge` parameter. When rendered inside coord-quick-access, pass `wg.name` (e.g. "WG-1-DEV-TEAM"). When rendered inside workgroups, pass `undefined` (the WG context is already clear from the section header).

### 2. ProjectPanel.tsx — Replace coord-quick-item rendering (lines 441-474)

Replace the `<For>` inside coord-quick-access:

**Before (lines 441-474):**
```tsx
<For each={coordinators()}>
  {(item) => {
    const dotClass = () => replicaDotClass(item.wg, item.replica);
    const rn = () => ...;
    const branchLabel = () => ...;
    return (
      <div class="coord-quick-item" ...>
        ...
      </div>
    );
  }}
</For>
```

**After:**
```tsx
<For each={coordinators()}>
  {(item) => renderReplicaItem(item.replica, item.wg, item.wg.name)}
</For>
```

### 3. ProjectPanel.tsx — Replace workgroups replica rendering (lines 524-653)

Replace the entire `<For each={wg.agents}>` callback body:

**Before (lines 524-653):**
```tsx
<For each={wg.agents}>
  {(replica) => {
    const dotClass = () => ...;
    // ... 130 lines of handlers and JSX ...
    return (<div class="replica-item">...</div>);
  }}
</For>
```

**After:**
```tsx
<For each={wg.agents}>
  {(replica) => renderReplicaItem(replica, wg)}
</For>
```

### 4. sidebar.css — Remove unused coord-quick-item/info/name/wg classes

**DELETE the following CSS rule blocks** (keep `.coord-quick-access` and its per-theme variants):

| Lines | Selector | Action |
|-------|----------|--------|
| 3671-3678 | `.coord-quick-item` | DELETE |
| 3680-3682 | `.coord-quick-item:hover` | DELETE |
| 3684-3689 | `.coord-quick-info` | DELETE |
| 3691-3698 | `.coord-quick-name` | DELETE |
| 3700-3705 | `.coord-quick-wg` | DELETE |
| 3733-3737 | `[deep-space] .coord-quick-item` | DELETE |
| 3739-3741 | `[deep-space] .coord-quick-item:hover` | DELETE |
| 3743-3747 | `[deep-space] .coord-quick-name` | DELETE |
| 3749-3751 | `[deep-space] .coord-quick-wg` | DELETE |
| 3762-3764 | `html.light-theme[deep-space] .coord-quick-name` | DELETE |
| 3790-3794 | `[obsidian-mesh] .coord-quick-item` | DELETE |
| 3796-3799 | `[obsidian-mesh] .coord-quick-item:hover` | DELETE |
| 3801-3805 | `[obsidian-mesh] .coord-quick-name` | DELETE |
| 3807-3809 | `[obsidian-mesh] .coord-quick-wg` | DELETE |
| 3816-3818 | `html.light-theme[obsidian-mesh] .coord-quick-name` | DELETE |
| 3858-3862 | `[neon-circuit] .coord-quick-item` | DELETE |
| 3864-3867 | `[neon-circuit] .coord-quick-item:hover` | DELETE |
| 3869-3873 | `[neon-circuit] .coord-quick-name` | DELETE |
| 3875-3877 | `html.light-theme[neon-circuit] .coord-quick-name` | DELETE |
| 3879-3882 | `[neon-circuit] .coord-quick-wg` | DELETE |

**KEEP** all `.coord-quick-access` rules (base + per-theme + light-theme variants + `::before` + `::after`).

---

## Notes

- The `renderReplicaItem` function uses `createSignal` internally (for `showBotMenu`, `availableBots`). In SolidJS this is fine — signals created inside a function called within a reactive context are properly tracked. Each `<For>` iteration gets its own closure with independent signals.
- The `extraBadge` param renders as `<span class="ac-discovery-badge team">` — the `.team` badge class already exists and is styled. No new CSS needed.
- The coord-quick items will now inherit all `.replica-item` CSS (including hover button visibility rules at lines 2253-2257 and status circle specificity at lines 462/465). This fixes both bugs.
- The `.coord-quick-access` container provides `display: none` by default and `display: block` per theme. This gating is preserved — the container just wraps `replica-item` divs now instead of `coord-quick-item` divs.

---

## Frontend Review (dev-webpage-ui)

### Verification Summary

All file paths, line numbers, and code references verified against the current codebase on branch `fix/coord-quick-use-replica-item`. All CSS selector line references are accurate.

### SolidJS Reactivity — CONFIRMED CORRECT

- `createSignal` inside `renderReplicaItem` creates independent instances per `<For>` iteration — correct. Signals are just data; they don't need scope-based disposal. The reactive bindings in the returned JSX (compiled by SolidJS into effects/memos) are created within the `<For>` callback's tracking scope, so they're automatically disposed when items are removed.
- `extraBadge` as a plain `string` (not a getter) is acceptable: the `<For>` over `coordinators()` creates new `{ replica, wg }` objects every time the memo recalculates, so `<For>` treats every update as new items, re-invoking the callback with the fresh `wg.name` value. No stale-closure risk.

### Concern: CSS double-styling for coordinator items in coord-quick

**Impact: MEDIUM — visual only, not a blocker, but should be verified.**

In **deep-space** and **obsidian-mesh** themes, existing CSS rules style `.replica-item:has(.ac-discovery-badge.coord)` with special "coordinator beacon" visuals:

- **deep-space** (lines 2980-2990): adds `padding: 8px 12px`, gradient background, `border: 1px solid`, `border-radius: 6px`, `margin: 3px 4px`
- **obsidian-mesh** (lines 3396-3401): adds `padding: 7px 10px`, background tint, `border-left: 3px solid`

Since coord-quick items will now be `.replica-item` with `.ac-discovery-badge.coord`, these rules WILL match. The coord-quick-access container already provides its own visual framing (gradient backgrounds, borders via `::before`/`::after`). This could create:
1. Nested borders (container border + item border)
2. Stacking backgrounds (container gradient + item gradient)
3. Extra padding/margins that may look cramped

**Recommendation**: After implementation, visually verify coord-quick-access in deep-space and obsidian-mesh themes. If the double-styling looks off, add override rules:
```css
.coord-quick-access .replica-item:has(.ac-discovery-badge.coord) {
  background: transparent;
  border: none;
  margin: 0;
  padding: 6px 12px;  /* match old coord-quick-item */
}
```
Neon-circuit has no coordinator-specific `.replica-item` rules, so no issue there.

### Note: Visual behavior change — `originProject` suffix

The current coord-quick items display `{item.replica.name}` (line 462). The `renderReplicaItem` function displays `{replica.originProject ? \`${replica.name}@${replica.originProject}\` : replica.name}` (line 115 of the plan's code block). This adds an `@originProject` suffix for cross-project coordinators. This is more informative and correct — flagging it as a deliberate, intentional change.

### Note: `order: -1` rule is safe

The rule at line 2974 (`[deep-space] .ac-wg-subgroup > .replica-item:has(.ac-discovery-badge.coord)`) uses `.ac-wg-subgroup >` parent constraint, so it won't affect coord-quick items. This is correct since coord-quick only contains coordinators — no ordering needed.

---

## Grinch Review

### G1 — BUG: Bot menu dropdown clipped by `overflow: hidden` (HIGH)

**What:** The Telegram bot selection menu (`.session-item-bot-menu`) is `position: absolute` with `top: 100%` (sidebar.css:1407-1409) — it renders below the `.replica-item`. The `.coord-quick-access` container has `overflow: hidden` in deep-space (line 3720) and neon-circuit (line 3829). Since `.replica-item` (the bot menu's containing block via `position: relative`) is INSIDE the overflow container, the bot menu dropdown WILL be clipped. Users cannot see or interact with bot options that extend beyond the container boundary.

**Why it matters:** This is a functional bug, not cosmetic. When a user has 2+ Telegram bots configured and clicks the Telegram button on a coordinator in the coord-quick section, the dropdown is invisible or partially cut off. The feature is broken in these themes.

**Fix options (pick one):**
1. **Override `overflow` inside coord-quick-access** — add `.coord-quick-access { overflow: visible; }` per-theme, replacing the `overflow: hidden`. Verify that removing it doesn't break the container's visual clipping of border-radius corners or pseudo-elements.
2. **Change bot menu to `position: fixed`** — compute position from click coordinates instead of relying on CSS flow. More invasive but eliminates the clipping issue universally (would also fix it if `.ac-wg-subgroup` ever gains `overflow: hidden`).
3. **Accept the limitation** — if bot selection from coord-quick is considered unnecessary (users can always use the same button in the workgroups section below), document it and skip the fix.

Note: obsidian-mesh does NOT set `overflow: hidden` on `.coord-quick-access`, so the bug does not affect that theme.

### G2 — Placement precision (LOW)

**What:** The plan says to define `renderReplicaItem` "between lines ~222 and ~339". This is a 117-line range. `handleReplicaContextMenu` is defined at line 317 and referenced by `renderReplicaItem` in the returned JSX. While JavaScript closures handle forward references correctly (the function isn't called until JSX renders, by which point all `const` declarations have been evaluated), placing `renderReplicaItem` BEFORE `handleReplicaContextMenu` creates a confusing read order.

**Recommendation:** Place `renderReplicaItem` AFTER `handleReplicaContextMenu` (after line 338, immediately before the `return` at line 340). Same behavior, clearer maintainability.

### G3 — Agree with dev-webpage-ui's CSS double-styling finding (MEDIUM)

Confirmed: the coordinator beacon rules in deep-space (lines 2980-2990: gradient bg, border, border-radius, margin) and obsidian-mesh (lines 3396-3401: bg tint, border-left) match ANY `.replica-item:has(.ac-discovery-badge.coord)` — no parent constraint. These WILL apply to coord-quick items, stacking with the container's own styling. Dev-webpage-ui's proposed override fix is correct and should be part of the implementation.

### Summary

- **G1 (HIGH)** — must be addressed. Recommend option 1 (override overflow per-theme) or option 3 (accept limitation with documentation).
- **G2 (LOW)** — placement guidance, not a bug.
- **G3 (MEDIUM)** — confirmed, fix already proposed by dev-webpage-ui.

---

## Files Summary

| File | Action | Description |
|------|--------|-------------|
| `src/sidebar/components/ProjectPanel.tsx` | MODIFY | Extract `renderReplicaItem`, use it in both coord-quick and workgroups sections |
| `src/sidebar/styles/sidebar.css` | MODIFY | Delete ~20 unused CSS rule blocks for coord-quick-item/info/name/wg |
