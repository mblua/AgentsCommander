# Semantic UI Automation Affordance Matrix

This matrix seeds issue #497 acceptance coverage. It tracks user-visible screen/mouse behaviors that currently have stable `data-ac-testid` hooks. Acceptance should report missing GUI automation as selector/action gaps.

## First-Run Onboarding

| Behavior | Selector | Action |
|---|---|---|
| Detect onboarding dialog | `onboarding.modal` | `query`, `wait` |
| Select Claude Code preset | `onboarding.agentPreset.claude` | `click` |
| Select Codex preset | `onboarding.agentPreset.codex` | `click` |
| Select Gemini preset | `onboarding.agentPreset.gemini` | `click` |
| Select custom preset | `onboarding.agentPreset.custom` | `click` |
| Enter custom label | `onboarding.custom.label` | `setValue` |
| Enter custom command | `onboarding.custom.command` | `setValue` |
| Skip onboarding | `onboarding.skip` | `click` |
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
| Toggle coordinator sort | `actionBar.sortCoordinators` | `click` |
| Toggle sounds | `actionBar.sounds` | `click` |
| Toggle category visibility | `actionBar.categories` | `click` |
| Toggle selected workgroup pin | `actionBar.pinSelectedWorkgroup` | `click` |
| Open guide | `actionBar.guide` | `click` |
| Open spec board when enabled | `actionBar.specBoard` | `click` |
| Detect terminal empty state | `terminal.empty` | `query` |
| Start a new empty terminal session | `terminal.empty.newSession` | `click` |

## Settings Seed

| Behavior | Selector | Action |
|---|---|---|
| Detect settings dialog | `settings.modal` | `query`, `wait` |
| Switch settings tab | `settings.tab.<tabKey>` | `click` |
| Add preset coding agent | `settings.agentPreset.<presetKey>` | `click` |
| Add custom coding agent row | `settings.agent.addCustom` | `click` |
| Save settings | `settings.save` | `click` |
| Cancel settings | `settings.cancel` | `click` |

## Known Gaps For Follow-Up

| Surface | Missing action/selector family |
|---|---|
| Project panel rows | `project.row.*`, `workgroup.row.*`, `team.row.*`, `agent.row.*`, `replica.row.*` |
| Session rows | `session.row.<sessionId>` and row action selectors for close, detach, Telegram, explorer, mic |
| Context menus | `contextMenu` action plus `menu.<surface>.<action>` selectors |
| Agent/open/new-agent modals | Dialog roots, list rows, form fields, launch actions |
| Terminal internals | xterm buffer inspection is out of DOM-selector scope for #497 |
| Drag/hold gestures | Future pointer actions for splitters and hold-to-record |
