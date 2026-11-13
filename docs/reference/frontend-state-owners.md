# Frontend state owners

Each store module under `src/*/stores/` owns the data listed here. A component reads owned data
from its store. A local copy is allowed only for a draft, an optimistic override or a
read-modify-write before a save, with a comment saying which.

`src/shared/state-owners-map.test.ts` fails when a store module is missing from this map or a
mapped path no longer exists. Add one line here when you add a store module.

| Store module | Owner export | Data owned |
| --- | --- | --- |
| `src/shared/stores/settings.ts` | `settingsStore` | whole `AppSettings` (reloads on coding-agent and room-idle settings events, on reconnect and on explicit `refresh()`) |
| `src/shared/stores/toasts.ts` | `toastStore` | toast list and timers |
| `src/shared/stores/resourceMonitor.ts` | `resourceMonitorStore` | resource snapshot, loading, error, last update, stale |
| `src/sidebar/stores/sessions.ts` | `sessionsStore` | sessions, active id, selection, teams, team filter, show-inactive and categories, repos, activity and context % per session, comanaged flags, sidebar pointer and menu state |
| `src/sidebar/stores/sessions-helpers.ts` | (none) | pure helpers for `sessionsStore`, no state |
| `src/sidebar/stores/project.ts` | `projectStore` | loaded projects, archived paths, loading, last load error |
| `src/sidebar/stores/project-merge.ts` | (none) | pure merge helpers for `projectStore`, no state |
| `src/sidebar/stores/project-refresh.ts` | (none) | pure path-compare helpers for `projectStore`, no state |
| `src/sidebar/stores/project-collapse.ts` | `projectCollapseStore` | ProjectPanel section collapse map |
| `src/sidebar/stores/rail-collapse.ts` | `railCollapseStore` | rail project collapse map, favorites collapsed |
| `src/sidebar/stores/workgroup-groups.ts` | `workgroupGroupsStore` | workgroup group config per project, active project key |
| `src/sidebar/stores/coding-agents.ts` | `codingAgentsStore` | coding-agent catalog, reseedable commands, load state and error |
| `src/sidebar/stores/agent-update-overview.ts` | `agentUpdateOverviewStore` | agent update rows, install states |
| `src/sidebar/stores/replica-volatile.ts` | `replicaVolatileStore` | per-replica volatile git, branch, dirty and last-message data |
| `src/sidebar/stores/remote-activity.ts` | `remoteActivityStore` | remote activity by path |
| `src/sidebar/stores/room-activity.ts` | module functions | room activity entries (observer-counted) |
| `src/sidebar/stores/team-idle-watcher.ts` | module functions | OS focus, busy-to-idle grace state |
| `src/sidebar/stores/ci-activity-stamp.ts` | module functions | previous CI activity per key |
| `src/sidebar/stores/coordinator-close.ts` | `pendingCoordinatorClose` | pending coordinator-close request |
| `src/sidebar/stores/auto-unarchive.ts` | `autoUnarchiveStore` | auto-unarchive notices |
| `src/sidebar/stores/bridges.ts` | `bridgesStore` | Telegram bridges list |
| `src/sidebar/stores/clock.ts` | `clockStore` | shared `nowMs` tick |
| `src/sidebar/stores/project-archive-test-helpers.ts` | (none) | test fixture, no app state |
| `src/main/stores/centralView.ts` | `centralViewStore` | central view (terminal and others) |
| `src/main/stores/error-modal.ts` | `errorModalStore` | error log entries, index |
| `src/main/stores/home.ts` | `homeStore` | home panel visible, content, loading, error |
| `src/main/stores/webBrowser.ts` | `webBrowserStore` | web browser snapshot, error |
| `src/terminal/stores/terminal.ts` | `terminalStore` | active task snapshot, read state, task cache |
| `src/spec-board/stores/spec-board.ts` | `specBoardStore` | spec-board state |
