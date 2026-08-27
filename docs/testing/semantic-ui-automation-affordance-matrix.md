# Semantic UI Automation Affordance Matrix

This matrix seeds issue #497 acceptance coverage. It tracks user-visible screen/mouse behaviors that currently have stable `data-ac-testid` hooks. Acceptance should report missing GUI automation as selector/action gaps.

## First-Run Onboarding

| Behavior | Selector | Action |
|---|---|---|
| Detect onboarding dialog | `onboarding.modal` | `query`, `wait` |
| Select Claude Code preset | `onboarding.agentPreset.claude` | `click` |
| Select Codex preset | `onboarding.agentPreset.codex` | `click` |
| Select Antigravity preset | `onboarding.agentPreset.antigravity` | `click` |
| Select custom preset | `onboarding.agentPreset.custom` | `click` |
| Enter custom label | `onboarding.custom.label` | `setValue` |
| Enter custom command | `onboarding.custom.command` | `setValue` |
| Cancel onboarding | `onboarding.cancel` | `click` |
| Confirm selected agent | `onboarding.confirm` | `query`, `click` |
| Detect done state | `onboarding.done` | `query`, `wait` |
| Close done dialog | `onboarding.done.close` | `click` |

## Already-Open GUI Seed

| Behavior | Selector | Action |
|---|---|---|
| Detect main WebView root | `main.root` | `query` |
| Detect sidebar root | `sidebar.root` | `query` |
| Detect terminal root | `terminal.root` | `query` |
| Open New/Open menu | `actionBar.newOpen` | `click` |
| Create project from menu | `actionBar.menu.newProject` | `click` |
| Open project from menu | `actionBar.menu.openProject` | `click` |
| Open settings | `actionBar.settings` | `click` |
| Toggle theme | `actionBar.theme` | `click` |
| Toggle home | `actionBar.home` | `click` |
| Toggle orchestrator sort | `actionBar.sortCoordinators` | `click` |
| Toggle sounds | `actionBar.sounds` | `click` |
| Toggle category visibility | `actionBar.categories` | `click` |
| Toggle selected workgroup pin | `actionBar.pinSelectedWorkgroup` | `click` |
| Open guide | `actionBar.guide` | `click` |
| Open spec board when enabled | `actionBar.specBoard` | `click` |
| Detect terminal empty state | `terminal.empty` | `query` |

## Settings Seed

| Behavior | Selector | Action |
|---|---|---|
| Detect settings dialog | `settings.modal` | `query`, `wait` |
| Switch settings tab | `settings.tab.<tabKey>` | `click` |
| Detect/add preset coding agent | `settings.agentPreset.<presetKey>` | `query`, `click` when `data-ac-state="available"` |
| Add custom coding agent row | `settings.agent.addCustom` | `click` |
| Detect coding agent row | `settings.agentRow.<index>` | `query` |
| Set coding agent label | `settings.agentRow.<index>.label` | `setValue` |
| Set coding agent command | `settings.agentRow.<index>.command` | `setValue` |
| Set coding agent color text value | `settings.agentRow.<index>.color` | `setValue` |
| Set coding agent color picker | `settings.agentRow.<index>.colorPicker` | `setValue` |
| Remove coding agent row | `settings.agentRow.<index>.remove` | `click` |
| Save settings | `settings.save` | `click` |
| Cancel settings | `settings.cancel` | `click` |

## Orchestrator Context Menu (#943 / #944)

`hover` is a sticky pointer transition. The bridge remembers the last hovered element and fires the leave chain (element + ancestors) before entering the next one. `click` and `contextClick` do NOT move the pointer. `hover --leave` takes no selector: it parks the pointer nowhere and cannot fail.

| Behavior | Selector | Action |
|---|---|---|
| Open an orchestrator's context menu | `replica.row.<context>.<wg>.<agent>` | `contextClick` |
| Wait for the Browse submenu to become available | `replica.<sessionId>.menu.repo.<index>.browse.arrow` | `wait` |
| Open the Browse submenu | `replica.<sessionId>.menu.repo.<index>` | `hover` |
| Detect the Browse submenu | `replica.<sessionId>.menu.repo.<index>.browse.flyout` | `query` |
| Open the repo root on GitHub | `replica.<sessionId>.menu.repo.<index>.browse.main` | `hover`, `click` |
| Open the current branch on GitHub (absent on main/master/HEAD) | `replica.<sessionId>.menu.repo.<index>.browse.branch` | `hover`, `click` |
| Open the Add to Group flyout | `replica.<wg>.groups.trigger` | `hover` |
| Detect the Add to Group flyout | `replica.<wg>.groups.flyout` | `query` |
| Park the pointer nowhere (closes hover flyouts, releases the sidebar order freeze) | (none) | `hover --leave` |

- The arrow wait must be re-run after **every** `contextClick`: opening a context menu clears the resolved-remote cache for all repos, so the previous menu's arrow disappears and the new one's has to resolve from scratch.
- Inactive (gray) orchestrators use the constant prefix `replica.inactive.menu.repo` instead of `replica.<sessionId>.menu.repo`, so two inactive orchestrators share a prefix.
- Bracket any hover-using script with `hover --leave` at both ends. The pointer is sticky **across CLI invocations**, and a script that starts with the pointer already on its target gets a same-element re-hover, which dispatches nothing (`diagnostics.hover.changed: false`).
- `hover` drives JS handlers (`onMouseEnter` / `onPointerEnter` and their leave twins). It cannot drive the CSS `:hover` pseudo-class, and it deliberately dispatches no `pointermove` / `mousemove`, so nothing that listens for pointer movement — a drag, a splitter, the screenshot crosshair — can see it.
- `hover` runs the same visibility and **obscured** gates as `click`: a covered element genuinely receives no pointer, so it is refused with `target_obscured`, and `diagnostics.topmost` names what is on top of it. The one place this bites in practice: the Browse flyout flips to the **left** of its anchor when it would overflow the viewport, so on a narrow window it can land on top of the menu itself, and the next `hover` on another repo entry is refused. Recovery: widen the window, or `hover --leave` (which closes the flyout) and retry.

## Sidebar Titlebar (#1274)

| Behavior | Selector | Action |
|---|---|---|
| Inspect the active screenshot-capture shortcut status | `[data-ac-testid="screenshot-hotkey-status"]` | `query` only; passive status with no semantic action |

## Known Gaps For Follow-Up

| Surface | Missing action/selector family |
|---|---|
| Project panel rows | `project.row.*`, `workgroup.row.*`, `team.row.*`, `agent.row.*`, `replica.row.*` |
| Session rows | `session.row.<sessionId>` and row action selectors for close, detach, Telegram, explorer, mic |
| Context menus | Use `contextClick` on the owning row/header selector, then `query`/`click` the mounted action selector. Project Loops selectors include `project.loops.header.<projectId>`, `loop.row.<projectId>.<loopId>`, `loop.action.new.<projectId>`, `loop.action.runNow.<projectId>.<loopId>`, `loop.action.edit.<projectId>.<loopId>`, `loop.action.toggle.<projectId>.<loopId>`, and `loop.action.delete.<projectId>.<loopId>`. Loop delete uses an in-app confirmation with `loop.delete.confirm.<projectId>.<loopId>` and `loop.delete.cancel.<projectId>.<loopId>`. Disabled Loop rows use `data-ac-state="loop-disabled"` so their context menus remain actionable; reserve `data-ac-state="disabled"` for controls that automation should reject for non-query actions. |
| Agent/open/new-agent modals | Dialog roots, list rows, template picker rows, form fields, launch actions |
| New Team modal | Dialog root, wizard step markers, team name input, agent filter, agent checkboxes, orchestrator radio buttons, repo input, create/back/next buttons |
| New Workgroup modal | Dialog root, team select, task-title input, create/cancel buttons, creation progress/error state |
| Target-window evidence | HWND-surface screenshot support; for non-reserved monitors, foreground/unobscured assertion before screen-rectangle capture |
| Terminal internals | xterm buffer inspection is out of DOM-selector scope for #497 |
| Drag/hold gestures | Future pointer actions for splitters and hold-to-record. `hover` shipped in #944 and deliberately dispatches no `pointermove` |
