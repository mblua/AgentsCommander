# Plan: Running-Peers Badges on Coordinator Quick-Access Rows

**Branch:** `feature/coord-running-peers-badges` (already cut off `origin/main`)
**Scope:** pure frontend. Two files touched — one `.tsx` and one `.css`.
**Status:** Ready for implementation.
**Anchored against HEAD:** `fd44953 Auto stash before checking out "origin/main"` (the working replica is clean on that commit except for an untracked sibling plan file).

---

## 1. Overview

Add a new badge family to the **Coordinator Quick-Access** rows (and only those rows) that lists the *other* replicas of the same workgroup that are currently running (blue dot = `running` or `active`). One badge per running peer, each reading `<replica.name> RUNNING`. Empty state = no badges rendered.

The badges are purely derived from existing reactive state (`sessionsStore` + `projectStore`) — no new IPC, no new backend work, no new stores, no new events.

Data flow:

```
sessionsStore (reactive)
  → createMemo computing runningPeers for each coordinator item
  → passed as an Accessor<AcAgentReplica[]> into renderReplicaItem
  → rendered as <For> inside the badge row, gated by <Show>
```

When `renderReplicaItem` is called from the Workgroups list further down, the `runningPeers` parameter is **omitted** → the new block emits nothing → zero visual impact on that render site.

---

## 2. Files to touch

| File | Purpose |
|---|---|
| `src/sidebar/components/ProjectPanel.tsx` | Add optional 4th param to `renderReplicaItem`; compute memo in the coord-quick-access loop; render badges first in `.ac-discovery-badges` |
| `src/sidebar/styles/sidebar.css` | Add `.ac-discovery-badge.running-peer` base variant + one light-theme override |

No changes elsewhere. Specifically **do not** touch:
- `SessionItem.tsx`
- `src/shared/types.ts` (no new types — we reuse `AcAgentReplica`)
- `src/shared/ipc.ts`
- any Rust file
- any theme-specific badge block in `sidebar.css` beyond the new base variant + one light override

---

## 3. TypeScript edits — `src/sidebar/components/ProjectPanel.tsx`

### 3.1 Extend `renderReplicaItem` signature

**Anchor:** lines 393–397 (current signature).

```tsx
        const renderReplicaItem = (
          replica: AcAgentReplica,
          wg: AcWorkgroup,
          extraBadge?: string
        ) => {
```

**Change to:**

```tsx
        const renderReplicaItem = (
          replica: AcAgentReplica,
          wg: AcWorkgroup,
          extraBadge?: string,
          runningPeers?: () => AcAgentReplica[]
        ) => {
```

`runningPeers` is an **Accessor** (a `() => AcAgentReplica[]`), not a plain array, so that SolidJS reactively re-runs the inner `<For>` when the memo recomputes. Keep it optional — callers outside coord-quick-access do not pass it, and the badges block stays hidden for them.

### 3.2 Insert the running-peer badges block at the **top** of the badge row

**Anchor:** lines 477–509 (the `<div class="ac-discovery-badges">` block inside `renderReplicaItem`).

Current:

```tsx
                <div class="ac-discovery-badges">
                  <Show when={isCoord()}>
                    <Show
                      when={(() => { const s = session(); return s && s.gitRepos.length > 0 ? s : undefined; })()}
                      fallback={
                        <Show when={replica.repoPaths.length === 1 && replica.repoBranch}>
                          <span class="ac-discovery-badge branch">
                            {rn()}/{replica.repoBranch}
                          </span>
                        </Show>
                      }
                    >
                      {(s) => (
                        <For each={s().gitRepos}>
                          {(repo) => (
                            <span class="ac-discovery-badge branch">
                              {repo.label}{repo.branch ? `/${repo.branch}` : ""}
                            </span>
                          )}
                        </For>
                      )}
                    </Show>
                  </Show>
                  <Show when={liveAgentLabel()}>
                    <span class="ac-discovery-badge agent">{liveAgentLabel()}</span>
                  </Show>
                  <Show when={isCoord()}>
                    <span class="ac-discovery-badge coord">coordinator</span>
                  </Show>
                  <Show when={extraBadge}>
                    <span class="ac-discovery-badge team">{extraBadge}</span>
                  </Show>
                </div>
```

**Insert a new block as the very first child of `<div class="ac-discovery-badges">`, BEFORE the existing `<Show when={isCoord()}>` branch badge (i.e. before the current line 478):**

```tsx
                  <Show when={runningPeers && runningPeers()!.length > 0}>
                    <For each={runningPeers!()}>
                      {(peer) => (
                        <span
                          class="ac-discovery-badge running-peer"
                          title={`${wg.name}/${peer.name}`}
                        >
                          {peer.name} RUNNING
                        </span>
                      )}
                    </For>
                  </Show>
```

Tooltip format is `${wg.name}/${peer.name}` for all peers (no conditional on `originProject`, no `peer.path` fallback). This matches the `replicaSessionName` convention at `ProjectPanel.tsx:50`, which is the exact key by which the peer's session is registered in `sessionsStore`. See §11 Finding 4 for the rationale.

Final order of badges after the change:

```
[running-peer …] [branch …] [agent live] [coordinator] [WG-N-team]
```

Notes on the `<Show>` guard:
- `runningPeers && runningPeers()!.length > 0` short-circuits cleanly when the accessor is `undefined` (non-coord-quick-access callers).
- The `!` on `runningPeers!()` inside the `<For>` is safe because the outer `<Show>` has already proven it is defined.
- Do **not** inline the filter here — it is computed upstream in the coord-quick-access loop so each coordinator item has its own tracked memo.

### 3.3 Compute `runningPeers` in the Coordinator Quick-Access loop

**Anchor:** lines 649–657 (the `<For each={coordinators()}>` render inside the IIFE).

Current:

```tsx
                  return (
                    <Show when={coordinators().length > 0}>
                      <div class="coord-quick-access">
                        <For each={coordinators()}>
                          {(item) => renderReplicaItem(item.replica, item.wg, item.wg.name)}
                        </For>
                      </div>
                    </Show>
                  );
```

**Change the `<For>` callback to build a `runningPeers` memo per coordinator and pass it as the 4th argument. Use the single-lookup form (early-return on self-match, cache `replicaDotClass` once per peer):**

```tsx
                  return (
                    <Show when={coordinators().length > 0}>
                      <div class="coord-quick-access">
                        <For each={coordinators()}>
                          {(item) => {
                            const runningPeers = createMemo(() =>
                              item.wg.agents.filter((peer) => {
                                if (peer.name === item.replica.name) return false;
                                const dot = replicaDotClass(item.wg, peer);
                                return dot === "running" || dot === "active";
                              })
                            );
                            return renderReplicaItem(item.replica, item.wg, item.wg.name, runningPeers);
                          }}
                        </For>
                      </div>
                    </Show>
                  );
```

Notes:
- `createMemo` is already imported on line 1 — **do not** add a new import.
- The memo reads `item.wg.agents` (drives reactivity when workgroup membership changes) and reads `replicaDotClass(item.wg, peer)` for each peer. `replicaDotClass` calls `replicaSession`, which reads `sessionsStore.findSessionByName`, which is a reactive store. That read chain is what makes the badges update when a peer session transitions into or out of `running`/`active`.
- Exclusion of the coordinator itself is by `peer.name !== item.replica.name`. Within a single `wg.agents`, replica names are unique (invariant used elsewhere in this file via `replicaSessionName`), so no further disambiguation is needed.
- Do **not** change the `<For>` that renders replicas inside the Workgroups expandable section (lines 704–706 in the current file). It continues to call `renderReplicaItem(replica, wg)` with the third and fourth arguments omitted.

### 3.4 Do NOT change any other call site

The only other invocation of `renderReplicaItem` in this file is line 705 inside the Workgroups `<For>`. Leave it alone — the 4th arg is optional and omitted means no badges.

---

## 4. CSS edits — `src/sidebar/styles/sidebar.css`

### 4.1 Base variant (dark / default)

**Anchor:** lines 2335–2339 (the `.ac-discovery-badge.agent` block).

```css
.ac-discovery-badge.agent {
  background: rgba(16, 185, 129, 0.14);
  color: #34d399;
  text-transform: none;
}
```

**Insert immediately after, before the blank line that precedes `/* Workgroup styles */`:**

```css
.ac-discovery-badge.running-peer {
  background: rgba(58, 123, 255, 0.15);
  color: #6aa0ff;
  text-transform: none;
}
```

Design rationale:
- Background tint = 15% of `--status-running` (`#3a7bff`), matching the family convention (`.coord` uses 15%, `.branch` uses 15%, `.team` uses 12%, `.agent` uses 14%).
- Foreground `#6aa0ff` is a lighter shade of the blue to stay readable at 8px/uppercase on the dark backgrounds used by `deep-space`, `obsidian-mesh`, `noir-minimal`, `neon-circuit`.
- `text-transform: none` preserves the replica name's original casing (`dev-rust RUNNING` instead of `DEV-RUST RUNNING`). This mirrors `.branch` and `.agent`, which are the other variants that carry user-provided identifiers.
- No per-sidebar-style override is added — the base variant reads well across all five sidebar styles. If a future theme author wants a bespoke look, they can add a selector under `[data-sidebar-style="…"] .ac-discovery-badge.running-peer` without this plan being the blocker.
- **Intentional color unification across `running` and `active` peers.** `--status-active` is cyan (`#00d4ff`) while `--status-running` is blue (`#3a7bff`). Peers in either state produce a **blue** badge, even though an `active` peer's left-hand dot is cyan. This is deliberate: the user spec groups both statuses as a single "peer is live" family. Recording it here so a future reviewer does not chase this as a perceived dot/badge color mismatch (cf. §11 Finding 2).

### 4.2 Light-theme override

**Anchor:** directly after the rule added in 4.1.**

```css
html.light-theme .ac-discovery-badge.running-peer {
  background: rgba(37, 99, 235, 0.12);
  color: #2563eb;
}
```

Rationale: the light theme redefines `--status-running` to `#2563eb` (`variables.css:65`). The override mirrors that palette shift so the badge maintains contrast on light backgrounds, using the same tint/foreground split as the dark variant.

---

## 5. Data flow summary

```
sessionsStore.findSessionByName  ──▶  replicaSession(wg, peer)
                                         │
                                         ▼
                                  replicaDotClass(wg, peer)
                                         │
                                         ▼
             createMemo(() => wg.agents.filter(peer → dot ∈ {running, active} ∧ peer ≠ coord))
                                         │
                                         ▼
                       runningPeers Accessor<AcAgentReplica[]>
                                         │
                                         ▼
                   renderReplicaItem(replica, wg, wg.name, runningPeers)
                                         │
                                         ▼
             <Show when=...> <For each={runningPeers()}> <span class="ac-discovery-badge running-peer">
```

All reactivity is inherited from `sessionsStore`. No new subscription, no new event listener, no polling.

---

## 6. Edge cases considered

| Case | Behavior |
|---|---|
| Peer transitions from `idle` → `running` mid-render | `sessionsStore` emits; `replicaDotClass` re-reads; memo recomputes; `<For>` adds a badge. No flicker. |
| Peer has no session yet (never launched) | `replicaSession` returns `undefined` → `replicaDotClass` returns `"offline"` → filtered out. |
| Peer has a session but the PTY exited | `session.status` is `{exited: {...}}` → `replicaDotClass` returns `"exited"` → filtered out. |
| Peer has `waitingForInput === true` | `replicaDotClass` returns `"waiting"` (green dot) → filtered out. Not a blue-dot status. |
| Peer has `pendingReview === true` | `replicaDotClass` returns `"pending"` (amber dot) → filtered out. |
| Workgroup has no other agents | `wg.agents.filter(...).length === 0` → memo returns `[]` → outer `<Show>` hides the block. |
| Coordinator is also running | Coordinator's own blue dot is already visible on the left of its row; filter excludes it via `peer.name !== item.replica.name`. |
| Two workgroups each have their own coordinator + running peers | Each coordinator row gets its own `runningPeers` memo (one per `<For>` iteration). Memos are independent. |
| `renderReplicaItem` called from the Workgroups list | `runningPeers` is `undefined` → outer `<Show>` evaluates to false → no badges. Unchanged visual output in that section. |
| Replica `originProject` is set (agent was bundled into this workgroup from another project) | Tooltip shows `${wg.name}/${peer.name}` (session-key form) regardless of `originProject`; badge text still uses `replica.name`. |
| Replica name contains spaces | The `.ac-discovery-badge` font is 8px uppercase-transform is off for this variant, so the name renders verbatim. CSS `white-space` inherits `normal` and `flex-wrap: wrap` on `.ac-discovery-badges` handles overflow across lines. |
| Many running peers (e.g. 8) | Each renders as its own badge; `.ac-discovery-badges` already has `flex-wrap: wrap` + `gap: 4px` (lines 2299–2303), so they wrap to a second row. The row height grows naturally — that is acceptable for a coordinator row which already has more vertical space than regular rows. |
| Narrow sidebar (≈240 px) + 5+ running peers | Running-peer badges fill the first line; the subsequent `[branch] [agent] [coordinator] [WG-N]` badges wrap to a second (or later) line, pushing the `.coord` badge below the fold of its row. **Intentional trade-off** — the user spec locks both "first position" and "one badge per peer". Coordinator identity remains conveyed by the surrounding beacon styling on `deep-space` / `obsidian-mesh` / `neon-circuit` (enlarged row, tinted background, gradient border). If post-ship UX feedback justifies a cap+counter, that must go through a new intake round, not a silent patch here. |

---

## 7. Testing checklist (manual, in `npm run tauri dev`)

Run `npm run kill-dev` before starting. Then `npm run tauri dev`. Open the sidebar.

1. **Setup** — make a workgroup with a team that has ≥3 agents: 1 coordinator + 2 peers.
2. **Golden path — one peer running:**
   - Start the coordinator session. Its row shows in Coordinator Quick-Access.
   - Start one peer session. Confirm a `<peer-name> RUNNING` badge appears on the coordinator row, positioned **before** the branch badge.
   - Peer badge color = blue tint matching the status-running dot.
3. **Reactivity — peer goes from running to exited:**
   - With peer running, click the ✕ on the peer row to close it.
   - Badge for that peer disappears from the coordinator row without manual refresh.
4. **Reactivity — peer restarts:**
   - Restart the peer session via right-click → Restart Session.
   - Badge reappears.
5. **Multiple peers:**
   - Start a second peer. Both `<peer1> RUNNING` and `<peer2> RUNNING` badges appear, side by side (or wrapping to a second line if the sidebar is narrow).
6. **Empty state:**
   - Close all peers. The coordinator row shows zero running-peer badges — no placeholder, no empty container.
7. **Coordinator itself running:**
   - When the coordinator's dot is blue, confirm there is **no** `<coordinator-name> RUNNING` badge on its own row.
8. **Non-coord-quick-access rows:**
   - Expand the Workgroups section below. Confirm the same peer replicas listed there do **not** render running-peer badges next to the coordinator row under Workgroups. (That render site omits the 4th argument, so the block is hidden.)
9. **Status filter correctness:**
   - With a peer in `waitingForInput` state (dot green): confirm no badge.
   - With a peer in `pendingReview` state (dot amber): confirm no badge.
   - With a peer in `idle` state (dot grey): confirm no badge.
10. **Theme cycle:**
    - Open Settings → cycle sidebar style: `noir-minimal`, `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`.
    - For each of those five, confirm the badge is legible and visually consistent with `.branch` / `.coord` / `.team` siblings.
    - **`card-sections`**: by default rule `.coord-quick-access { display: none }` at `sidebar.css:3689`, and `card-sections` does not override it. Expected outcome: **no coordinator row visible at all** on this style, so running-peer badges are N/A. Confirm the whole coord-quick-access block stays hidden; do NOT flag absence of badges as a failure.
    - Toggle light theme (`html.light-theme`). Confirm the override kicks in — background lighter, foreground darker blue, still readable.
11. **Tooltip:**
    - Hover a `<peer-name> RUNNING` badge. Tooltip shows `<wg-name>/<peer-name>` (the session-key form) regardless of whether the peer has an `originProject`.
12. **No regressions elsewhere:**
    - The Agents section (bottom of each project) renders unchanged.
    - The Teams section renders unchanged.
    - `SessionItem` rows (in the Agents list for bound sessions) render unchanged.

All 12 steps must pass before reporting the feature complete.

---

## 8. Things the dev must NOT do

- Do **not** add running-peer logic to `SessionItem.tsx`. The spec is explicit: only coord-quick-access rows.
- Do **not** emit a badge on the Workgroups section's `renderReplicaItem` calls. Leave that call site at three arguments.
- Do **not** change the meaning, color, or shape of the status dot (`.session-item-status`).
- Do **not** introduce a new store, a new IPC command, a new Tauri event, or any Rust change. This is pure derived UI state.
- Do **not** sort or deduplicate the running-peers list beyond what `wg.agents.filter(...)` naturally produces — it already follows the workgroup's declared agent order.
- Do **not** collapse the rendering into `<peer1>, <peer2> RUNNING` or `3 running` counter — the spec requires **one badge per peer**.
- Do **not** add per-sidebar-style overrides for `.ac-discovery-badge.running-peer`. The base rule plus the single light-theme override is the agreed-upon scope.
- Do **not** add an empty-state placeholder (`"Nadie más activo"` or similar). Absence is the empty state.
- Do **not** bump the app version in this branch — the coordinator will handle versioning as part of the merge-to-main step if needed.

---

## 9. Dev enrichment (dev-webpage-ui)

Reviewed against HEAD `3e1f3c5 Merge fix/64-tech-lead-role-branch-naming` (12 commits ahead of the plan's `fd44953` anchor, but none of the drifted commits touched `ProjectPanel.tsx`, `sidebar.css`, `variables.css`, `types.ts`, or `stores/sessions.ts`). Every line anchor in §3 and §4 still points at exactly what the plan describes.

### 9.1 Verified anchors

- `ProjectPanel.tsx:393–397` — `renderReplicaItem` signature: **matches** the plan verbatim.
- `ProjectPanel.tsx:477–509` — `<div class="ac-discovery-badges">` block: **matches** verbatim.
- `ProjectPanel.tsx:649–657` — `<For each={coordinators()}>` inside the coord-quick-access IIFE: **matches** verbatim.
- `ProjectPanel.tsx:704–706` — Workgroups-list `renderReplicaItem(replica, wg)` call: **matches** verbatim.
- `sidebar.css:2299–2303` — `.ac-discovery-badges { display: flex; gap: 4px; flex-wrap: wrap; }`: **matches**.
- `sidebar.css:2335–2339` — `.ac-discovery-badge.agent` block: **matches**.
- `variables.css:14` / `variables.css:65` — `--status-running` dark (`#3a7bff`) / light (`#2563eb`): **matches**.
- `sessions.ts:341–343` — `findSessionByName` returns `state.sessions.find(...)` — reactive read chain confirmed.
- `types.ts:25` — `SessionStatus = "active" | "running" | "idle" | { exited: number }` — exactly the two string variants the filter targets (plus `idle` and exit object, correctly excluded).
- `ProjectPanel.tsx:54,68` — `replicaSession` / `replicaDotClass` helpers exist as module-level functions and read `sessionsStore.findSessionByName`, so the memo's reactivity is inherited end-to-end as the plan claims.
- `createMemo` is already imported on line 1 — no new import needed.

No anchor drift. The plan is safe to execute as written against current HEAD.

### 9.2 Correctness concern flagged in the intake spec (not a plan bug)

The user-confirmed spec says **"exactly `running` and `active` (both blue dots)"**. Looking at the theme:

- `variables.css:13` — `--status-active: #00d4ff` → **cyan**
- `variables.css:14` — `--status-running: #3a7bff` → **blue**
- `sidebar.css:471` — `.session-item-status.active` uses `--status-active` (cyan)
- `sidebar.css:472` — `.session-item-status.running` uses `--status-running` (blue)

So `active` dots are actually **cyan**, not blue. The filter is still correct per the user's spec (include both states), and unifying the badge under `--status-running` (blue) is a defensible family choice since `active` and `running` are semantically "peer is live / responsive". But anyone reading the intake doc later may be surprised that a cyan-dot peer produces a blue-toned badge. Worth recording this reality in the plan or the follow-up notes so a future reviewer does not chase a "bug" that was a deliberate unification.

**No change requested to implementation** — only a note that the colorimetric description in the intake is inexact.

### 9.3 Threading strategy — I endorse the 4th-param Accessor

I reviewed the two obvious alternatives the architect considered implicitly:

- **(a) Boolean flag + inline memo inside `renderReplicaItem`.** `renderReplicaItem` already has `wg` and `replica`, so it could compute the filter itself when a flag is true. Equivalent reactivity, similar LOC. **Rejected:** spreads the concern of "what rows get running-peer badges" across two files-worth of logic; a future contributor could flip the flag from the Workgroups caller and silently double the memo count.
- **(b) Dedicated `renderCoordReplicaItem`.** Duplicates ~100 lines of shared JSX (mic button, telegram, explorer, detach, context menu, close). **Rejected:** the only delta is the single `<Show>` block; duplication is strictly worse here.
- **(c) Optional 4th-param Accessor (architect's choice).** Keeps the memo at the loop site where each `<For>` iteration creates its own tracked scope; non-coord callers pay exactly zero cost by omitting the argument; `<Show when={runningPeers && ...}>` guards `undefined` cleanly. **Preferred** — this is the right pick.

### 9.4 SolidJS reactivity details the implementer must respect

1. **Memo owner is the `<For>` item scope, not the component.** `createMemo` inside the `<For>` callback is owned by the keyed iteration, so when a coordinator is removed from `coordinators()` the memo is auto-disposed. Do **not** hoist the memo to the component body — that would break per-row scoping.
2. **Do not destructure `runningPeers` inside `renderReplicaItem`.** `const peers = runningPeers!();` at the top of the function **would work** (the JSX still re-reads via the enclosing `<Show>`'s tracking of `runningPeers()`), but the preferred form is to invoke the accessor only inside JSX — `<Show when={runningPeers && runningPeers()!.length > 0}>` and `<For each={runningPeers!()}>`. This keeps the tracking scope tight.
3. **`<For each={runningPeers!()}>` is fine.** SolidJS's JSX compiler wraps attribute expressions in getters, so `runningPeers!()` is re-evaluated each reactive tick. This is idiomatic SolidJS — no need to pass the bare accessor.
4. **No `onCleanup` needed.** The plan introduces no event listeners, no `setInterval`, no `invoke()` subscriptions. All reactivity is derived from `sessionsStore`, which owns its own lifecycle.
5. **Keying by `peer.name` is implicit and correct.** `<For>` uses reference equality by default; since `wg.agents.filter(...)` returns the same `AcAgentReplica` objects from the workgroup model, identity is stable across memo recomputes. No `keyed` helper required.

### 9.5 Micro-optimization (optional)

In §3.3 the filter calls `replicaDotClass(item.wg, peer)` twice per peer for the `||` check:

```tsx
(replicaDotClass(item.wg, peer) === "running" ||
  replicaDotClass(item.wg, peer) === "active")
```

Each call re-runs `replicaSession` → `findSessionByName` → `.find()` over `state.sessions`. For a workgroup with N agents and M total sessions, the memo does `2 * N * M` finds per recompute. On realistic sizes (N ≤ 10, M ≤ 30) this is negligible, but it is trivially avoidable:

```tsx
const runningPeers = createMemo(() =>
  item.wg.agents.filter((peer) => {
    if (peer.name === item.replica.name) return false;
    const dot = replicaDotClass(item.wg, peer);
    return dot === "running" || dot === "active";
  })
);
```

Same reactivity, half the lookups. **Proposed** — not a blocker. Implementer can apply or skip at their discretion.

### 9.6 Tooltip content — suggestion

§3.2 sets the tooltip to `peer.originProject ? ${peer.originProject}/${peer.name} : peer.path`. `peer.path` is a full Windows absolute path (e.g. `C:\Users\maria\0_repos\…\wg-4-dev-team\__agent_dev-rust`) — noisy in a tooltip. A more consistent alternative is `${wg.name}/${peer.name}` for in-WG peers, which matches the `replicaSessionName` convention used elsewhere in this file (and matches what the bound session is actually named).

However, the current choice mirrors the row-level `title={replica.path}` on line 472, which is the established convention. **I defer to the architect** — either is acceptable. If keeping `peer.path`, no change needed; if changing, replace with `${wg.name}/${peer.name}`.

### 9.7 CSS family alignment — the new variant fits cleanly

Confirmed the existing badge family tints:

| Variant | Background alpha | Foreground |
|---|---|---|
| `.team` | 12% | `var(--sidebar-accent)` |
| `.coord` | 15% | `#eab308` |
| `.no-role` | 12% | `var(--status-exited)` |
| `.branch` | 15% | `#a78bfa` |
| `.agent` | 14% | `#34d399` |
| **`.running-peer` (new)** | **15%** | **`#6aa0ff`** |

Within tolerance of the family convention. Nothing to adjust.

### 9.8 Per-sidebar-style overrides — confirmed unnecessary

I checked `sidebar.css` for existing overrides in `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`, `card-sections`. Each of those styles overrides `.session-item-status.active` (line 2982, 3218, 3406, 3679) but **not** the `.ac-discovery-badge.*` family. The existing `.branch`, `.agent`, `.coord`, `.team` badges render with the base rule across all five styles without bespoke overrides. The new `.running-peer` will behave the same way. §4's single dark base + single light-theme override is exactly right.

### 9.9 Edge cases the plan already covers well

§6 of the plan is thorough. I validated each case against the code:

- "Peer transitions idle → running": `sessionsStore` emits on session `update_state_internal` → store.sessions array emits → `.find()` re-reads → `replicaSession` returns new value → `replicaDotClass` returns new string → memo filter includes peer. ✓
- "Peer has no session yet": `findSessionByName` returns `undefined` → `replicaDotClass` returns `"offline"` → filtered out. ✓
- "Peer `waitingForInput`": `replicaDotClass` returns `"waiting"` BEFORE reading `session.status`, so it correctly overrides even if the underlying PTY is `"running"`. ✓
- "Peer `pendingReview`": same early-return branch returns `"pending"`. ✓
- "`originProject` set": badge text uses `replica.name` (not fully-qualified). Consistent with the row's own display which uses `replica.name@originProject` at `ProjectPanel.tsx:476` — so the badge shows the short form while the tooltip disambiguates via `originProject/name`. ✓

### 9.10 Verdict

**Plan is complete, correct, and safe to implement as written.** One optional micro-optimization in §9.5, one optional tooltip refinement in §9.6, and one documentation note in §9.2. None are blockers. No plan edits beyond this section are needed.

Implementer: proceed with §3 and §4 verbatim. If you adopt §9.5, fold it into §3.3 at implementation time; it is a pure local simplification with no spec impact.

---

## 10. Grinch review (dev-rust-grinch)

Reviewed against HEAD `3e1f3c5`. All line anchors in §3, §4, and §9 verified against current files. Spec items in the intake (statuses, text, position, color, one-per-peer, coordinator-excluded) are NOT challenged, per the tech-lead's instructions. I exercised the surface; what follows is everything I could find that could break or confuse.

### 10.1 Things I checked and could NOT break

Listed up front so it is clear the review is not a skim:

- **Anchor drift.** All line numbers in §3 and §4 match the current file verbatim (re-verified at HEAD `3e1f3c5`).
- **Reactivity end-to-end.** `sessionsStore` (createStore at `sessions.ts:7`) → `findSessionByName` (`.find` over reactive `state.sessions`, `sessions.ts:341–343`) → `replicaSession` → `replicaDotClass` (reads `.pendingReview`, `.waitingForInput`, `.status` — all tracked) → memo → `<Show>`/`<For>` subscribes. Transitions idle↔running, running→exited, new session, destroyed session all propagate without manual invalidation.
- **Memo lifecycle.** `createMemo` inside the `<For>` child callback is owned by the keyed iteration scope (SolidJS semantics). When `coordinators()` recomputes and returns a fresh array of fresh `{replica,wg}` objects, the old iteration is torn down and the memo is disposed. No leak. Dev-UI §9.4.1 is correct.
- **`<Show>` guard against undefined accessor.** `runningPeers && runningPeers()!.length > 0` correctly short-circuits for the Workgroups caller that omits the 4th arg (line 705). Operator precedence parses as `runningPeers && (runningPeers().length > 0)` — verified.
- **`<For>` key stability.** `wg.agents.filter(...)` returns a subset with stable `AcAgentReplica` references (same objects from the workgroup model). SolidJS `<For>` reference-keying handles add/remove/reorder correctly without a `keyed` helper. No flicker between memo recomputes.
- **Status match correctness against `SessionStatus` union.** `SessionStatus = "active" | "running" | "idle" | { exited: number }` (`types.ts:25`). `replicaDotClass` returns `"exited"` (string) for the object variant at the last fallback, so the filter's string equality `=== "running" || === "active"` is sound against every possible state the function returns (`offline`, `pending`, `waiting`, `active`, `running`, `idle`, `exited`) — no accidental object-equality trap.
- **`pendingReview` / `waitingForInput` flags on top of running status.** `replicaDotClass` short-circuits to `"pending"` / `"waiting"` BEFORE reading `session.status`, so a running-but-awaiting peer is correctly excluded from the badge list. Matches the spec's explicit "`waiting`/`pending` excluded".
- **Coordinator self-exclusion.** `peer.name !== item.replica.name` is safe. `wg.agents` is built by iterating `__agent_<name>/` directories in the WG folder (`ac_discovery.rs:676–679`); the filesystem guarantees folder-name uniqueness within a workgroup, so `replica.name` is unique inside a single `wg.agents` array regardless of `originProject`. Two `dev-rust` replicas from different origin projects cannot coexist in the same workgroup folder.
- **CSS specificity.** Base `.ac-discovery-badge.running-peer` (0,2,0) beats base `.ac-discovery-badge` (0,1,0) for `text-transform: none`. Light override `html.light-theme .ac-discovery-badge.running-peer` (0,2,1) beats base (0,2,0). No `!important` needed; no collisions with existing `.branch`/`.coord`/`.agent`/`.team` rules (all sibling-class selectors at the same specificity tier).
- **Per-sidebar-style overrides.** Confirmed `[data-sidebar-style="deep-space"] .ac-discovery-badge` (`sidebar.css:2955`) only sets `opacity: 0.95` — affects every badge uniformly, does not clobber background/color of `.running-peer`. No other style has a generic `.ac-discovery-badge` rule that would override the new variant's color. Dev-UI §9.8 is correct for the variants it checked.
- **Event listener / onCleanup leaks.** Plan adds no `addEventListener`, no `invoke()` subscription, no `setInterval`. No cleanup path to audit.
- **Scope creep.** Two files, exactly the deltas described. No Rust, no stores, no types, no IPC. Confirmed.

### 10.2 Findings

#### Finding 1 — `card-sections` sidebar style never renders the badge (test step 10 is misleading) [LOW]

**What.** `.coord-quick-access { display: none }` is the default rule at `sidebar.css:3689`. Only `noir-minimal`, `arctic-ops`, `deep-space`, `obsidian-mesh`, and `neon-circuit` override it to visible (`sidebar.css:2663, 3222, 3731, 3770, 3795`). `card-sections` has no override, so its coordinator quick-access container stays hidden, and the new running-peer badges are not rendered for any user on that style.

**Why it matters.** Test step 7.10 reads: "cycle sidebar style: `noir-minimal`, `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`, `card-sections`. For each, confirm the badge is legible and visually consistent". On `card-sections` there is literally nothing to confirm — an implementer following this step will either falsely pass it ("I cycled it, didn't crash, done") or falsely fail it ("no badges appear, the feature is broken on card-sections!"). Both outcomes are wrong.

**Fix.** In §7 testing step 10, change the `card-sections` line to explicitly note: *"`card-sections` — coord-quick-access is hidden by this style (`sidebar.css:3689` default rule is not overridden). Confirm that NO coord row is visible here; running-peer badges are by definition not applicable."* Alternatively, drop `card-sections` from the style-cycle test and add a separate step verifying it stays hidden.

#### Finding 2 — `active` peers get a badge whose tint doesn't match their dot [LOW]

**What.** `variables.css:13` defines `--status-active: #00d4ff` (cyan), while `--status-running: #3a7bff` (blue) is what the new `.running-peer` badge uses. A peer with `session.status === "active"` passes the filter (per spec §1) but renders a BLUE badge next to its CYAN dot. The dev-UI already flagged this at §9.2 as a "colorimetric inexactness" but did not change the implementation because the user spec closed on "`--status-running` base".

**Why it matters.** The color/dot mismatch is only visible when a peer is `active` (not `running`). In practice `active` is the "user is focused on this session" status — it is usually the ONE peer the user is looking at, so the mismatch will stand out the single time the feature would most naturally draw attention. A future bug reporter will file this as "the badge color is wrong on active peers" and it will cost time to relitigate.

**Fix.** Not a spec change. Add an explicit line to §4.1's rationale bullet list: *"Unified under `--status-running` blue even when the peer is `active` (cyan dot). Intentional — the spec groups both statuses as one badge family. Documented here so a future reviewer does not chase a perceived bug."* If the tech-lead prefers to reopen the spec on this one point, that belongs in intake, not here.

#### Finding 3 — Double lookup in filter is O(N·M·2), worth the one-line simplification [LOW]

**What.** Dev-UI §9.5 already flagged this; I am endorsing and labeling it. The filter at §3.3 calls `replicaDotClass(item.wg, peer)` twice per peer for the `||` branch. Each call walks `state.sessions.find(s => s.name === name)` → linear scan. For N=10 agents and M=30 sessions, the memo does `2 × 10 × 30 = 600` comparisons per recompute; and the memo recomputes on ANY reactive read inside `replicaDotClass` (which is every tracked session property). With five coordinators across multiple projects, each with its own `runningPeers` memo, this is `5 × 600 = 3000` comparisons per relevant session update. Still small, but it is also a trivially avoidable waste.

**Why it matters.** On its own, nothing breaks. Combined with `.filter` also being invoked on every reactive tick from any session's status/flag transition, this is a predictable hot path as the workspace grows. Setting a precedent of "compute the dot class once" here also prevents the pattern from being copied elsewhere at 2× cost.

**Fix.** Adopt the dev-UI's §9.5 rewrite verbatim in §3.3 — single-lookup form with the early-return on self-match. This is pure local simplification, zero spec impact. I recommend making it part of the canonical implementation rather than an optional polish, to avoid two implementations diverging.

#### Finding 4 — Tooltip format disagrees with the row's own display format [NITPICK]

**What.** §3.2 sets the badge `title` to `peer.originProject ? ${peer.originProject}/${peer.name} : peer.path`. The row at `ProjectPanel.tsx:476` displays the replica name as `${replica.name}@${replica.originProject}` when `originProject` is set. Badge tooltip uses `/` as separator; row display uses `@`. Different orderings too: tooltip puts project first (`project/name`), row display puts name first (`name@project`).

**Why it matters.** Inconsistency between tooltip and visible text is subtle but real — a user hovering a badge will see `projectA/dev-rust` while the row they are pointing at says `dev-rust@projectA`. It is not a bug per se, but it is the kind of small inconsistency that wastes ten minutes of a future reader's time trying to figure out which one is the "real" identifier. Dev-UI §9.6 also raised this and deferred.

**Fix.** Unify on one format. Either change the tooltip to `${peer.name}@${peer.originProject}` to match the row display, OR use `${wg.name}/${peer.name}` to match the `replicaSessionName` convention at line 50 (which is what the actual session is named in sessionsStore, and arguably the most useful piece of info for the tooltip). The `peer.path` fallback for non-origin-project peers is unnecessarily noisy as already noted by dev-UI — prefer `${wg.name}/${peer.name}` in both branches.

#### Finding 5 — Plan's "running-peer first" makes the `.coord` badge scroll off on narrow sidebars with many peers [NITPICK]

**What.** With the new order `[running-peer … running-peer] [branch] [agent] [coordinator] [WG-N]`, once enough peers are running to wrap the badge row, the `.coord` badge (which is the single clearest visual identifier that "this is the team lead") gets pushed to the second (or later) wrapped line. On a 240px-wide sidebar with 5+ running peers, the coordinator badge disappears below the fold of its row.

**Why it matters.** The user spec explicitly says "Position: first in the badge row" for running-peer, so the ordering itself is not up for debate. But the visual consequence — the `.coord` badge moving out of easy scan — conflicts with the `card-sections`-less styles (deep-space, obsidian-mesh, neon-circuit) which already spend significant CSS establishing the coordinator as the visually-dominant row. Users on small-sidebar setups with active workgroups may complain.

**Fix.** Not a plan change. Note it in §6 edge cases so that if a user raises the issue post-ship, we know it was considered: *"On narrow sidebars with ≥5 running peers, the `.coord` badge may wrap to a second line. Per spec, running-peer is first; coordinator identity remains conveyed by the row's surrounding beacon styling in `deep-space`/`obsidian-mesh`/`neon-circuit`. Intentional trade-off."* If the tech-lead wants to hedge, add a max-badge CSS cap like `.ac-discovery-badge.running-peer:nth-of-type(n+6) { display: none; }` with a `+N more` counter — but that reopens the "one badge per peer" spec item, so it should only happen via intake reopening.

### 10.3 Summary

- **0 BLOCKERs**
- **0 HIGHs**
- **3 LOWs** (findings 1, 2, 3 — all have concrete fix text)
- **2 NITPICKs** (findings 4, 5)

Plan is safe to implement. Finding 3 is the only one I'd strongly lobby for folding in before the dev starts (it is dev-UI's own suggestion; making it canonical avoids two implementations diverging). Findings 1 and 2 are doc-only edits. Findings 4 and 5 are deferrable.

No concurrency bugs. No leak paths. No stale-reference TOCTOU. No status-string vs object-equality trap. No cross-workgroup name-collision vector (filesystem enforces uniqueness). No CSS specificity collisions across themes.

---

## 11. Architect round-2 resolution

Decisions on each finding raised in §9 (dev-webpage-ui) and §10 (dev-rust-grinch). All changes have been folded into §§3, 4, 6, and 7 above so the implementer works from a single coherent source — this section records the *why* so nothing is reopened.

- **Finding §10.1 (LOW #1) — `card-sections` test step is misleading.** **Accept.** §7 step 10 rewritten: the five styles that enable `.coord-quick-access` are listed as an explicit theme-cycle loop, and `card-sections` is called out separately with its expected "no coord row visible at all, running-peer test N/A" outcome. An implementer can no longer falsely pass or falsely fail the step.
- **Finding §10.2 (LOW #2) — `active`-status peers render a blue badge next to a cyan dot.** **Accept as documentation.** Added a rationale bullet to §4.1 explicitly marking the unification under `--status-running` as intentional. The user spec groups `active` and `running` as a single "peer is live" family; recording the dot/badge color reality in the plan prevents a future reviewer from treating it as a bug.
- **Finding §10.3 (LOW #3) + dev §9.5 — double lookup in the filter.** **Accept as canonical.** §3.3 now prescribes the single-lookup form with `if (peer.name === item.replica.name) return false; const dot = replicaDotClass(item.wg, peer); return dot === "running" || dot === "active";`. Both reviewers agreed the optimization should be the baseline, not a discretionary polish; making it canonical prevents two implementers from landing different code for the same memo.
- **Finding §10.4 (NITPICK #4) + dev §9.6 — tooltip format.** **Accept with modification.** Tooltip locked to `${wg.name}/${peer.name}` unconditionally. No `originProject` branch, no `peer.path` fallback. Rationale: this is the exact `replicaSessionName` key (see `ProjectPanel.tsx:50`) by which the peer's session is registered in `sessionsStore` — the most *useful* piece of information for a tooltip that is meant to disambiguate which peer is running. Grinch's proposal is adopted verbatim; dev-ui's deferral is resolved. §3.2 and §6 updated accordingly. Note: the row's own `name@originProject` display at `ProjectPanel.tsx:476` is intentionally left unchanged — the row shows a short display label, the tooltip shows the session key, and that split carries more information than forcing both to match.
- **Finding §10.5 (NITPICK #5) — `.coord` badge wraps below fold on narrow sidebars with ≥5 running peers.** **Accept as documentation only.** Added to §6 as an acknowledged trade-off with the mitigation note: coordinator identity is still carried by the surrounding beacon styling on the styles that enable coord-quick-access (`deep-space`, `obsidian-mesh`, `neon-circuit`). The user spec locks both "first position" and "one badge per peer", so no CSS cap or `+N more` counter is introduced here. If post-ship feedback justifies either, it must be reopened through intake.

### Items deliberately NOT reopened

The user-confirmed spec from round-1 intake remains locked:

- Statuses that trigger a badge: exactly `running` and `active` (nothing else).
- Badge text: `<replica.name> RUNNING`.
- One badge per peer (no counter, no comma list).
- Coordinator itself excluded.
- Position: **first**, before the branch badge.
- Color family: derived from `--status-running` (blue).

No round-3 needed. Consensus reached — hand to dev-webpage-ui for implementation.
