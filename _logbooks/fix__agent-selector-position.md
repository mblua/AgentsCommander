# fix/agent-selector-position

## Problem Statement

When double-clicking an inactive agent that is near the top of the session list, the agent selector modal (OpenAgentModal) options project behind/below the titlebar and team-filter header. The modal content is partially hidden, making it impossible to see or interact with the top options.

**Expected:** Modal overlay covers the full viewport, agent options are centered and fully visible.
**Observed:** Modal content clips behind the titlebar/team-filter area for items near the top of the list.

## Investigation

### 1. Reproduce
- Screenshot: `0_greenshot/2026-03-25 23_08_23-Agents Commander.png`
- Steps: Open sidebar with BIG-BOARD team filter. Double-click an inactive agent at the top of the list. The agent selector shows "Claude Code" and "Codex" options but they're partially obscured by the header.
- **Confirmed:** Bug is visible in the screenshot.

### 2. Hypothesize
The `OpenAgentModal` is rendered inside `SessionItem` (line 286-291). The modal uses `.modal-overlay` with `position: fixed; inset: 0; z-index: 1000`, which should cover the full viewport.

However, `.session-item:hover` (sidebar.css line 287) applies `transform: scale(1.01)`. In CSS, **any element with a `transform` other than `none` becomes the containing block for `position: fixed` descendants**. This means:

1. User hovers over session-item -> `transform: scale(1.01)` is applied
2. User double-clicks -> modal opens while transform is active
3. Modal's `position: fixed` is now relative to the session-item, NOT the viewport
4. The modal overlay is trapped inside the session-item's stacking context
5. The `z-index: 1000` is scoped to that context, so the titlebar/team-filter paint on top

### 3. Fix Applied
Wrapped `OpenAgentModal` in SolidJS `<Portal>` in `SessionItem.tsx`. Portal renders the modal directly into `document.body`, escaping the session-item's transform stacking context entirely.

**Files changed:**
- `src/sidebar/components/SessionItem.tsx` — added `import { Portal } from "solid-js/web"`, wrapped `<OpenAgentModal>` in `<Portal>`

### 4. Test
- [ ] Build succeeds (TypeScript check: PASS)
- [ ] App launches in dev mode
- [ ] Double-click inactive agent at top of list -> modal fully visible
- [ ] Double-click inactive agent at bottom of list -> still works
- [ ] Double-click active session name -> still opens agent selector
- [ ] Keyboard navigation (arrows + enter + esc) still works in modal
- [ ] "Open Agent" button in toolbar still works (not affected by this change)

### 5. Validate
Pending user testing.
