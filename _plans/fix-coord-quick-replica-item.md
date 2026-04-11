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

**Placement**: Define this function inside the component scope, before the JSX return — it needs closure access to `proj`, stores, `voiceRecorder`, `handleReplicaClick`, `handleReplicaContextMenu`, etc.

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

## Files Summary

| File | Action | Description |
|------|--------|-------------|
| `src/sidebar/components/ProjectPanel.tsx` | MODIFY | Extract `renderReplicaItem`, use it in both coord-quick and workgroups sections |
| `src/sidebar/styles/sidebar.css` | MODIFY | Delete ~20 unused CSS rule blocks for coord-quick-item/info/name/wg |
