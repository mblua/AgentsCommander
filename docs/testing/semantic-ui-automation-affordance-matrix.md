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
| Toggle selected room pin | `actionBar.pinSelectedWorkgroup` | `click` |
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
| Open an orchestrator's context menu | `replica.row.<context>.<replicaPath>` | `contextClick` |
| Wait for the Browse submenu to become available | `replica.<sessionId>.menu.repo.<index>.browse.arrow` | `wait` |
| Open the Browse submenu | `replica.<sessionId>.menu.repo.<index>` | `hover` |
| Detect the Browse submenu | `replica.<sessionId>.menu.repo.<index>.browse.flyout` | `query` |
| Open the repo root on GitHub | `replica.<sessionId>.menu.repo.<index>.browse.main` | `hover`, `click` |
| Open the current branch on GitHub (absent on main/master/HEAD) | `replica.<sessionId>.menu.repo.<index>.browse.branch` | `hover`, `click` |
| Open the Add to Group flyout | `replica.<wgPath>.groups.trigger` | `hover` |
| Detect the Add to Group flyout | `replica.<wgPath>.groups.flyout` | `query` |
| Park the pointer nowhere (closes hover flyouts, releases the sidebar order freeze) | (none) | `hover --leave` |

- The arrow wait must be re-run after **every** `contextClick`: opening a context menu clears the resolved-remote cache for all repos, so the previous menu's arrow disappears and the new one's has to resolve from scratch.
- Inactive (gray) orchestrators use the prefix `replica.inactive.<replicaPath>.menu.repo` instead of `replica.<sessionId>.menu.repo`; it is unique per replica.
- Bracket any hover-using script with `hover --leave` at both ends. The pointer is sticky **across CLI invocations**, and a script that starts with the pointer already on its target gets a same-element re-hover, which dispatches nothing (`diagnostics.hover.changed: false`).
- `hover` drives JS handlers (`onMouseEnter` / `onPointerEnter` and their leave twins). It cannot drive the CSS `:hover` pseudo-class, and it deliberately dispatches no `pointermove` / `mousemove`, so nothing that listens for pointer movement — a drag, a splitter, the screenshot crosshair — can see it.
- `hover` runs the same visibility and **obscured** gates as `click`: a covered element genuinely receives no pointer, so it is refused with `target_obscured`, and `diagnostics.topmost` names what is on top of it. The one place this bites in practice: the Browse flyout flips to the **left** of its anchor when it would overflow the viewport, so on a narrow window it can land on top of the menu itself, and the next `hover` on another repo entry is refused. Recovery: widen the window, or `hover --leave` (which closes the flyout) and retry.

## Sidebar Titlebar (#1274)

| Behavior | Selector | Action |
|---|---|---|
| Inspect the active screenshot-capture shortcut status | `[data-ac-testid="screenshot-hotkey-status"]` | `query` only; passive status with no semantic action |

## New Room (#2788)

P = automationIdPart(project.path), C = automationIdPart(rowContext), W = automationIdPart(workgroup.name). Confirm P with a direct query of the unique project.header.<P>. See [the isolated R2 fixture, five cases and separate gates](2788-new-room-black-box.md).

Approved source: option A in .ac/project-shared/i2788-visual-comparison/prototype-approved-option-B.html, SHA-256 fe72515f6145e6632d48fb05c5fbf2a9e01458b6b69d34e164740519477f2da5. User decision “D1-b y D2-b”: prototype-d1-d2-v1.html in the same directory, SHA-256 92708798c12d4527b2ed74cb7179260547f908859550bd9c37be89bae2056af9. D1-b keeps visible English confirmation “Selected team: <team>” / “No team selected.” D2-b preconfirms a sole team, suppresses opening only around synchronous initial mount focus, and first normal Enter creates once with taskTitle:"". Later focus, arrows, editing and Escape follow A; refresh never autoselects.

| Behavior | Selector | Action |
|---|---|---|
| Inspect/open project menu | `project.header.<P>` | `query`, `contextClick` |
| Open New Room from project menu | `project.action.newRoom.<P>.projectMenu` | `click` |
| Detect dialog | `newRoom.modal` | `query` |
| Inspect/filter combobox | `newRoom.teamSearch` | `query`, `click`, `setValue`, `typeText`, `key` (ArrowDown) |
| Inspect results/active index | `newRoom.team.list` | `query` when open |
| Inspect/confirm filtered row | `newRoom.team.option.<index>` | `query`, `click`; no `setValue` |
| Read visible confirmation | `newRoom.team.confirmed` | `query` |
| Set optional task title | `newRoom.taskTitle` | `setValue`, `query` |
| Read title help | `newRoom.taskTitle.hint` | `query` |
| Read conditional empty status | `newRoom.team.empty` | `query` |
| Inspect/create room | `newRoom.create` | `query`, `click` |
| Cancel dialog | `newRoom.cancel` | `click`, `query` |
| Read room title/Clean recognition | `workgroup.taskTitle.<P>.<C>.<W>` | `query` |

The native select and newRoom.team target are removed. Combobox role is combobox, detail “Search teams...”, state confirmed/unconfirmed, expanded from aria-expanded. Input set filters and invalidates confirmation even with the exact name; it never confirms. Without foreground Chromium may not dispatch focus: record input query and foreground/HWND before ui-key ArrowDown, then query expanded:true/list/options and active:0. This path does not establish focus-abre. Fixture remains three teams/five cases; sole-team mount/first Enter is a separate automated D2-b check, native Windows NOT-RUN.

List role listbox, detail JSON {options:[filtered names in order],active:index}. Row role option plus data-ac-role=text, detail/text exact name, state active/inactive, aria-selected for active row, not confirmation. Indices change: query list/row immediately before click. Confirmation data-ac-role=text, state confirmed/unconfirmed, detail JSON {selected:name}; identity/JSON format do not change by language.

Closed list/rows remain mounted under hidden: target_hidden expected. Empty results: {options:[],active:-1}, no rows (option.0 missing_selector expected), status “No teams match your search.” / “No teams available.” Closed modal: missing_selector. Inputs expose placeholder detail, not values. Title help is “Leave empty to start with Clean.” Title spans expose text/state clean/task.

Keep sanitization/120-character limit; parse complete fixture JSON, truncation means insufficient evidence. No bridge expansion or paths/agents/commands/tokens/task drafts projected. Query establishes snapshots/boxes, not hit-testing. Successful row click proves bridge dispatch/hit-test for that row, not OS mouse behavior; dispatched mousedown/focus/key/composition jsdom tests establish handlers, not Windows Tab/IME/WebView.

Separate R2 gates: new official binary receipt; five TASK cases (Clean 23/23 bytes, explicit 37 and UI state); deterministic D2-b; focus before ArrowDown; native Windows keyboard/mouse/Tab/IME NOT-RUN until authorized; new light/dark open/closed screenshots/bounds at normal/minimum geometry; isolation, unique receipts, manifest/cleanup. Normal placement x450/y250/1401x902; minimum 1200x900 physical at DPR1.5 requests 800 logical width, actual viewport may be 786. Record measured identity/DPR/viewport, no smaller geometry or OS fallback. Cropped captures cannot prove absent pixels/full comparison. Five PASS do not waive gates or inherit B acceptance.

## New Agent Dialog (#5172)

Open with `contextClick` on `project.header.<P>`, then `click` `project.action.newAgent.<P>.projectMenu`. The template list loads after the dialog mounts: `wait` for `newAgent.template.option.0` before searching.

| Behavior | Selector | Action |
|---|---|---|
| Detect dialog (role dialog) | `newAgent.modal` | `query`, `wait` |
| Filter templates | `newAgent.templateSearch` | `setValue`, `query` |
| Install Agency templates (state `installing`/`idle`) | `newAgent.installTemplates` | `query`, `click`, `wait` |
| Read install error | `newAgent.installTemplates.error` | `query` |
| Inspect template list | `newAgent.template.list` | `query` |
| Pick "No template" (state `selected`/`unselected`) | `newAgent.template.none` | `query`, `click` |
| Pick a template (detail = name, state `selected`/`unselected`) | `newAgent.template.option.<i>` | `query`, `click` |
| Set agent name | `newAgent.name` | `setValue` |
| Read create error | `newAgent.error` | `query` |
| Cancel dialog | `newAgent.cancel` | `click` |
| Create agent | `newAgent.create` | `query`, `click` |

## New Team Dialog (#5172)

Open with `contextClick` on `project.header.<P>`, then `click` `project.action.newTeam.<P>.projectMenu`. Wizard steps are exclusive: controls of hidden steps give `missing_selector`. Checkbox and radio inputs are toggled with `click`; the snapshot reports `checked`.

| Behavior | Selector | Action |
|---|---|---|
| Detect dialog (state `step-1`/`step-2`/`step-3`) | `newTeam.modal` | `query`, `wait` |
| Read step indicator (role status, state `step-<n>`) | `newTeam.step` | `query` |
| Set team name (step 1) | `newTeam.name` | `setValue` |
| Filter agents (step 2) | `newTeam.agentFilter` | `setValue` |
| Read loading / no-agents status | `newTeam.agents.loading`, `newTeam.agents.empty` | `query`, `wait` |
| Select agent (detail `name`, or `name@project` if not local) | `newTeam.agent.<a>` | `query`, `click` |
| Set coordinator (selected agents only) | `newTeam.agent.<a>.coord` | `click` |
| Enter repo URL (step 3) | `newTeam.repoInput` | `setValue` |
| Add repo | `newTeam.addRepo` | `click` |
| Inspect repo card (detail = repo display name) | `newTeam.repo.<r>` | `query` |
| Remove repo | `newTeam.repo.<r>.remove` | `click` |
| Toggle all agents of a repo | `newTeam.repo.<r>.allAgents` | `click` |
| Toggle one agent of a repo (detail as agent row) | `newTeam.repo.<r>.agent.<s>` | `click` |
| Read repo / create errors | `newTeam.repoError`, `newTeam.error` | `query` |
| Cancel (step 1) | `newTeam.cancel` | `click` |
| Back (steps 2 and 3) | `newTeam.back` | `click` |
| Next (steps 1 and 2) | `newTeam.next` | `click` |
| Create (step 3) | `newTeam.create` | `click` |
| Context alerts editor | `newTeam.contextAlerts.*` | see below |

Index rule: `<a>` is the 0-based position in the agent list as shown (after the filter), `<r>` the position in the repo list, `<s>` the position among selected agents, `<i>` the position in the filtered template list. Indices change when the list is filtered, added to or removed from: query the row `detail` just before `click`.

## Context Usage Alerts Editor (#5172)

Shared by New Team (`X` = `newTeam.contextAlerts`) and Edit Team (`X` = `editTeam.contextAlerts`). `<k>` is the 0-based row index.

| Behavior | Selector | Action |
|---|---|---|
| Add threshold | `X.add` | `click` |
| Read count / empty state | `X.count`, `X.empty` | `query` |
| Set percentage | `X.threshold.<k>` | `setValue` |
| Read row error | `X.threshold.<k>.error` | `query` |
| Remove threshold | `X.threshold.<k>.remove` | `click` |
| Read error summary | `X.errors` | `query` |

## Teams Section (#5172)

`<P>` = automationIdPart(project.path), `<T>` = automationIdPart(team.name), `<M>` = automationIdPart(member entry). The member entry is the `team.agents` value, written as `<projectFolder>/<agent>`; automationIdPart replaces each run of characters outside `[a-zA-Z0-9._-]` with `-` and trims `-`. Example: agent `tech-lead` of project folder `project` gives `team.member.<P>.dev-team.project-tech-lead`. The full entry is kept so same-named agents of two projects never collide.

| Behavior | Selector | Action |
|---|---|---|
| Expand/collapse team (state `collapsed`/`expanded`) | `team.header.<P>.<T>` | `query`, `click`, `contextClick` |
| Inspect member (detail = display label, e.g. `tech-lead@project`) | `team.member.<P>.<T>.<M>` | `query` |
| Team menu actions (after `contextClick` on the header) | `team.action.edit.<P>.<T>`, `team.action.delete.<P>.<T>` | `click` |

## Known Gaps For Follow-Up

| Surface | Missing action/selector family |
|---|---|
| Project panel rows | `project.row.*`, `workgroup.row.*`, `agent.row.*`, `replica.row.*` |
| Session rows | `session.row.<sessionId>` and row action selectors for close, detach, Telegram, explorer, mic |
| Context menus | Use `contextClick` on the owning row/header selector, then `query`/`click` the mounted action selector. Project Loops selectors include `project.loops.header.<projectId>`, `loop.row.<projectId>.<loopId>`, `loop.action.new.<projectId>`, `loop.action.runNow.<projectId>.<loopId>`, `loop.action.edit.<projectId>.<loopId>`, `loop.action.toggle.<projectId>.<loopId>`, and `loop.action.delete.<projectId>.<loopId>`. Loop delete uses an in-app confirmation with `loop.delete.confirm.<projectId>.<loopId>` and `loop.delete.cancel.<projectId>.<loopId>`. Disabled Loop rows use `data-ac-state="loop-disabled"` so their context menus remain actionable; reserve `data-ac-state="disabled"` for controls that automation should reject for non-query actions. |
| Open Agent modal | Only one hook today; list rows, form fields, launch actions |
| New Room modal | Creation progress/error state |
| Target-window evidence | HWND-surface screenshot support; for non-reserved monitors, foreground/unobscured assertion before screen-rectangle capture |
| Terminal internals | xterm buffer inspection is out of DOM-selector scope for #497 |
| Drag/hold gestures | Future pointer actions for splitters and hold-to-record. `hover` shipped in #944 and deliberately dispatches no `pointermove` |
