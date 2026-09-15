# Plan #2046: Agent Matrix row click explains the matrix, never launches a session

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2046 (OPEN)
- Repo: `repo-AgentsCommander`; branch `feature/2046-matrix-click-replica-notice`
- Base (frozen at authoring, 2026-09-15 UTC): branch HEAD = remote branch head =
  `d09beba5558f82782336c86c40101563e6575e22` (`git rev-parse HEAD`; `git ls-remote origin
  refs/heads/feature/2046-matrix-click-replica-notice`). Tracked tree clean. Every line number
  below refers to that SHA; if a quoted line no longer matches, re-anchor on the quoted text,
  never on the number.
- Class: Lite (band 1-25), one phase, no partition. Owner `ac-dev-webpage-ui-v4`; reviewer Grinch;
  coordinator `ac-tech-lead-v4`.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so commit with
  `git add -f plans/2046-matrix-click-replica-notice.md`.
- Frontend only. 1 new component, 1 modified component, 1 new test file. No backend, IPC,
  `src/shared/types.ts`, CSS, dependency, workflow, release, or migration change.

## 1. Problem and verified cause

Reported: in the sidebar Agents section, a left click on an Agent Matrix row starts a session.
It must instead explain the row: a matrix is the agent's canonical definition; what gets started
are replicas, instances of that agent inside a team, assigned to a room; the matrix structure is
reachable from the row itself.

Verified at the frozen base:

- The offline Agent Matrix row is `src/sidebar/components/ProjectPanel.tsx:3254-3266`; its
  `onClick={() => handleAgentClick(agent)}` is `:3256`.
- `handleAgentClick` (`:632-653`) has two branches: if a session named `agent.name` exists it
  switches to it (`:633-645`, including a `WindowAPI.ensureTerminal()` call under Tauri), else it
  runs `setPendingLaunch({ path: agent.path, sessionName: agent.name, gitRepos: [],
  currentAgentId: agent.preferredAgentId })` (`:647-652`).
- `pendingLaunch` (`:425`) renders the Coding Agent picker at the ProjectPanel root
  (`:4550-4582`): `AgentPickerModal` -> `SessionAPI.create(...)` (`:4560-4568`) ->
  `SessionAPI.switch(...)` (`:4569`). That picker is the launch flow the report is about.
- The row renders only while `sessionsStore.findSessionByName(agent.name)` is null
  (`:3249-3252`); a live matrix session renders `SessionItem` instead (`:3269-3281`).
- The row context menu stays the matrix-folder access path: "Open Matrix folder" (`:3299-3308`),
  "Delete" (`:3309-3320`), backed by `openMatrixFolder` (`:2041`); the menu block is `:3292-3323`,
  its opener `handleAgentContextMenu` (`:3172`).
- `src/sidebar/components/AcDiscoveryPanel.tsx:49-56` has its own `handleAgentClick`, but nothing
  mounts that component: the only references to `AcDiscoveryPanel` outside its own file are
  `docs/reference/architecture.md:845` and the room-rename allowlists. Dead code; no user-facing
  behavior change is reachable through it.

Cause: the offline matrix row is wired to the session-launch flow, although an Agent Matrix is not
a launchable unit. Only replicas are.

## 2. Decision

D1 - A left click on the offline Agent Matrix row opens a local informational modal. The handler
never calls `setPendingLaunch`, `SessionAPI`, or `sessionsStore`: no picker, no `create_session`,
no `switch_session`.

D2 - The notice is a new self-contained component `AgentMatrixNoticeModal`, rendered once at the
stable ProjectPanel root through `<Portal>` (same placement argument as the restart prompt,
`:4584-4588`), driven by the new root-level signal `agentMatrixNotice`.

D3 - Dismissal: the "Got it" button, a click on the overlay, and Escape. No confirm action, no
side effect, no new state beyond the signal.

D4 - `handleAgentClick` loses both the launch body and the existing-session switch branch: the row
it serves cannot exist while a session named `agent.name` exists (`:3249-3252`), so that branch was
unreachable from this row, and keeping it would preserve a second, now-misleading launch-adjacent
path. A live matrix session keeps working through `SessionItem`.

Alternatives closed:

- (a) Inline modal next to the delete-agent modal (`:3347`): rejected. It is opened from a
  ProjectPanel-root handler and a discovery refresh replaces the `<For>` rows, so the stable-root
  argument at `:4584-4588` applies; the delete modals' Escape effect (`:854-876`) is also scoped to
  one project section.
- (b) Keep `setPendingLaunch` and explain inside the picker: rejected. The picker is the launch
  surface; the requirement is that the row does not enter it at all.
- (c) Reuse `AutoUnarchiveModal` or `ContextTemplateUpdateModal` as-is: rejected. Both are
  store/payload-bound (`auto-unarchive` store; `ContextTemplateUpdate` payload) and carry
  acknowledgement or keep/overwrite semantics that do not fit a local notice.
- (d) Add "Open Matrix folder" as a button inside the modal: rejected as scope creep. The row
  context menu already owns the action and the copy points to it.
- (e) Show the notice for live matrix rows (`SessionItem`) too: rejected. That row is the running
  session; clicking it switches, which is existing, desired behavior.
- (f) Align `AcDiscoveryPanel.tsx`: rejected as out of scope. It is not mounted, so no user-facing
  path changes; editing dead code widens the diff with no testable behavior.

## 3. In scope / out of scope

In scope:

- New `src/sidebar/components/AgentMatrixNoticeModal.tsx`: the notice.
- `src/sidebar/components/ProjectPanel.tsx`: import, `agentMatrixNotice` signal, `handleAgentClick`
  body, root-level render.
- New `src/sidebar/components/ProjectPanel.agent-matrix-notice.test.tsx`: the tests in section 7.

Out of scope (binding):

- The row markup, classes and `title={agent.path}` tooltip (`:3254-3266`) other than the handler
  body: the row keeps its look, its `offline` dot and its name.
- The row context menu (`:3292-3323`) and `openMatrixFolder`; both stay byte-identical.
- `SessionItem` and the live matrix row (`:3269-3281`), `src/sidebar/components/SessionItem.tsx`.
- `src/sidebar/components/AcDiscoveryPanel.tsx` (unmounted dead code).
- `src-tauri/`, `src/shared/types.ts`, `src/shared/ipc.ts`, `src/sidebar/styles/sidebar.css`
  (every class reused already exists), dependencies, workflows, release scripts, docs.
- The row's keyboard focusability (a non-focusable `div` today): pre-existing gap, untouched.
- `preferredAgentId` for replicas (`handleReplicaClick` path): untouched; the field simply stops
  mattering to this row.

## 4. Exact changes

### 4.1 New `src/sidebar/components/AgentMatrixNoticeModal.tsx`

```tsx
import { Component, onCleanup } from "solid-js";
import { automationAttrs } from "../../shared/automation-hooks";

/**
 * #2046 - left click on an Agent Matrix row. The row is not launchable: this
 * notice says what the matrix is, what a replica is, and where the matrix
 * folder lives. Informational only; it closes on Got it, the overlay click, or
 * Escape, and never starts anything.
 */
const AgentMatrixNoticeModal: Component<{
  name: string;
  path: string;
  onClose: () => void;
}> = (props) => {
  // Registered while the modal is mounted (NewWorkgroupModal precedent,
  // NewWorkgroupModal.tsx:48-52) and removed on unmount.
  const handleDocumentKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };
  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  // Same display name the row shows (ProjectPanel.tsx:3262-3264).
  const displayName = () => props.name.slice(props.name.lastIndexOf("/") + 1);

  return (
    <div
      class="modal-overlay"
      onClick={props.onClose}
      {...automationAttrs("agentMatrixNotice.overlay", "overlay")}
    >
      <div
        class="agent-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="agentMatrixNoticeTitle"
        aria-describedby="agentMatrixNoticeDescription"
        style={{ "max-width": "380px" }}
        onClick={(e) => e.stopPropagation()}
        {...automationAttrs("agentMatrixNotice.modal", "dialog")}
      >
        <div class="agent-modal-header">
          <span id="agentMatrixNoticeTitle" class="agent-modal-title">
            Agent Matrix
          </span>
        </div>
        <div class="new-agent-form" id="agentMatrixNoticeDescription">
          <p style={{ margin: "0", "line-height": "1.5" }}>
            <strong>{displayName()}</strong> is an Agent Matrix: the canonical definition of
            this agent, with its Role, memory, plans and skills. It is not a session, so it is
            never launched.
          </p>
          <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
            What gets launched are replicas: instances of this agent, inside a team, assigned
            to a room. Start a room's replica from its row.
          </p>
          <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
            The matrix structure is still reachable from this row: right-click it and choose{" "}
            <strong>Open Matrix folder</strong>.
          </p>
          <div class="context-template-path" title={props.path}>
            {props.path}
          </div>
        </div>
        <div class="new-agent-footer">
          <button
            class="new-agent-create-btn"
            autofocus
            onClick={props.onClose}
            {...automationAttrs("agentMatrixNotice.close", "button")}
          >
            Got it
          </button>
        </div>
      </div>
    </div>
  );
};

export default AgentMatrixNoticeModal;
```

Every class is existing CSS: `.modal-overlay` (`sidebar.css:1433-1442`, `position: fixed; inset: 0;
display: flex; align-items: center; justify-content: center` - this is what centers the modal),
`.agent-modal`, `.agent-modal-header`, `.agent-modal-title`, `.new-agent-form`, `.new-agent-footer`,
`.new-agent-create-btn`, `.context-template-path`. No stylesheet edit.

### 4.2 `src/sidebar/components/ProjectPanel.tsx`

1. Import, after `:65` (`import RestartPromptModal from "./RestartPromptModal";`):

   ```ts
   import AgentMatrixNoticeModal from "./AgentMatrixNoticeModal";
   ```

2. Signal, after `:425`
   (`const [pendingLaunch, setPendingLaunch] = createSignal<PendingLaunch | null>(null);`):

   ```ts
   const [agentMatrixNotice, setAgentMatrixNotice] = createSignal<{ name: string; path: string } | null>(null);
   ```

3. Handler: replace `:632-653` with

   ```ts
   /** #2046 - an Agent Matrix row is not launchable: a left click only explains it. */
   const handleAgentClick = (agent: { name: string; path: string }) => {
     setAgentMatrixNotice({ name: agent.name, path: agent.path });
   };
   ```

   The call site (`:3256`) is unchanged. `sessionsStore`, `SessionAPI`, `isTauri`, `WindowAPI` keep
   every other use in the file.

4. Render: insert at `:4583`, between the pendingLaunch modal's closing `)}` and the `#537`
   restart-prompt comment (`:4584`):

   ```tsx
   {/* #2046: an Agent Matrix row is not launchable. Rendered at the stable
       ProjectPanel root (outside the projects <For>, like pendingLaunch and the
       restart prompt) so a discovery refresh that re-creates the row cannot
       unmount the notice mid-read. */}
   {agentMatrixNotice() && (
     <Portal>
       <AgentMatrixNoticeModal
         name={agentMatrixNotice()!.name}
         path={agentMatrixNotice()!.path}
         onClose={() => setAgentMatrixNotice(null)}
       />
     </Portal>
   )}
   ```

   `Portal` is already imported at `:2`.

Expected diff: 1 new component file, 3 small edits in `ProjectPanel.tsx`, 1 new test file. No other
module, no store, no IPC, no data shape.

## 5. Behavior and edge cases

| # | Case | Result |
|---|---|---|
| 1 | Offline Agent Matrix row, no session (the report) | Notice opens; picker never mounts; no `create_session`, no `switch_session` |
| 2 | Live session with the matrix's name | `SessionItem` renders as today (`:3269-3281`); clicking switches; notice never opens |
| 3 | Exited/inactive session with the matrix's name | `SessionItem` renders inactive; its click stays `undefined` (SessionItem.tsx:330); unchanged |
| 4 | Matrix with `preferredAgentId` set | The field is ignored by this row now; replica launches keep using it (unchanged) |
| 5 | Search filter hides the row (`filteredAgents`, `:1141-1144`, rendered `:3244`, `:3247`) | No row, no notice; unchanged behavior |
| 6 | Second left click while the notice is open | The overlay covers the row (z-index 1000), so the row is not clickable; a programmatic re-open just overwrites the same signal and the modal stays |
| 7 | Discovery refresh while the notice is open | The signal holds a `{ name, path }` copy and the modal lives at the ProjectPanel root, so it survives the row being re-created |
| 8 | Multiple projects | One notice instance; copy includes the full matrix path, so the identity is unambiguous |
| 9 | Long matrix name / path | Path line wraps (`.context-template-path` has `word-break: break-all`) |
| 10 | `AcDiscoveryPanel` mount path | Unreachable today (not imported anywhere); unchanged |
| 11 | Replica rows and the Agents header | Untouched: only the matrix fallback row's handler body changes |
| 12 | Row context menu (right click) | Untouched: "Open Matrix folder" and "Delete" still open (existing suite pins both) |

## 6. English UI copy (verbatim)

Title: `Agent Matrix`

Body paragraph 1: `{name} is an Agent Matrix: the canonical definition of this agent, with its
Role, memory, plans and skills. It is not a session, so it is never launched.` (`{name}` is the
row's display name and is bold.)

Body paragraph 2: `What gets launched are replicas: instances of this agent, inside a team,
assigned to a room. Start a room's replica from its row.`

Body paragraph 3: `The matrix structure is still reachable from this row: right-click it and
choose Open Matrix folder.` (`Open Matrix folder` is bold.)

Path line: the matrix `path`, monospace.

Button: `Got it`.

## 7. Tests

New file `src/sidebar/components/ProjectPanel.agent-matrix-notice.test.tsx` (`// @vitest-environment
jsdom`), modeled on the `ProjectPanel.offline-badges.test.tsx` mount shape.

Fixture: `projectPath = "C:\\Project"`, `originAgentPath = projectPath + "\\.ac\\_agent_dev-docs"`,
`agentName = "dev-docs"`, discovery `agents: [{ name: "dev-docs", path: originAgentPath,
roleExists: true }]`; `fake.resolve("new_project", ...)`, `get_settings` = `baseSettings()`,
`discover_project` = that discovery, `switch_session` = `null`; then `settingsStore.load()` and
`projectStore.createAndLoad(projectPath)`, `waitFor` the row text. Helpers: `q(testId)` over
`document.body.querySelector`, `agentRow(root)` = the `.replica-item` whose text contains `dev-docs`.

T1 - the required positive control: "left click on an Agent Matrix row shows the notice and never
mounts the Coding Agent picker".

```ts
const fake = await mount();
click(agentRow(rendered!.root));
await Promise.resolve();
expect(q("agentPicker.modal")).toBeNull();          // red on base: the picker is mounted here
await waitFor(() => expect(q("agentMatrixNotice.modal")).not.toBeNull());
expect(fake.callsFor("create_session")).toHaveLength(0);
expect(fake.callsFor("switch_session")).toHaveLength(0);
```

On the base this fails on the first assertion with the picker element printed, which is the raw
evidence that the base launched; after the fix both assertions pass.

T2 - copy and identity: after the same open, `q("agentMatrixNotice.modal")!.textContent` contains
`Agent Matrix`, `dev-docs`, `replica`, `assigned to a room`, `Open Matrix folder` and the full
`originAgentPath`; `q("agentPicker.modal")` is null.

T3 - dismissal and no side effect: three open/close cycles on the same row - Close button,
`click(q("agentMatrixNotice.overlay"))`, and `document.dispatchEvent(new
KeyboardEvent("keydown", { key: "Escape", bubbles: true }))` - each closes the notice
(`waitFor(... toBeNull())` and it reopens on the next click); also asserts
`q("agentMatrixNotice.overlay")!.classList.contains("modal-overlay")` (the class that centers
every project modal) and that an unclicked `agentPicker.modal` and zero
`create_session`/`switch_session` calls remain.

T4 - regression guard for the untouched path: mount with
`session({ id: "matrix-session", name: "dev-docs", workingDirectory: originAgentPath, status:
"running" })` in `sessionsStore`; the row is `[data-ac-testid="session.matrix-session"]`, clicking
it calls `switch_session` once with `{ id: "matrix-session" }`, and neither the notice nor the
picker appears. Green on base too; it pins the boundary.

Existing suites that must stay green (they are the authority for what this plan does not touch):

- `ProjectPanel.context-menu.test.tsx`: "opens the Matrix folder from an origin agent row"
  (`:696`), "shows a trash Delete action on an offline origin agent row" (`:756`) and the four
  delete-modal cases (`:787`, `:817`, `:850`, `:893`) - all right-click, none click.
- `ProjectPanel.reopen-resume.test.tsx` - clicks a room replica's `.replica-item` to open the
  picker; that path (`handleReplicaClick`) is untouched.
- No test on `main` left-clicks an origin agent row: `handleAgentClick` appears only at
  `ProjectPanel.tsx:632` and `:3256`, and no test file references it.

## 8. Acceptance criteria

1. Left click on an offline Agent Matrix row opens the notice; `agentPicker.modal` never enters the
   DOM; `create_session` and `switch_session` are never invoked.
2. The modal states the three required facts and shows the matrix path; the title reads
   `Agent Matrix`.
3. Centering comes from `.modal-overlay` (`sidebar.css:1433-1442`), the same surface every project
   modal uses; no CSS change.
4. The notice closes on Got it, overlay click and Escape, and reopens on the next click.
5. The live-session row, the row's context menu, replica rows, and the picker's replica launch flow
   are unchanged.
6. `npm run typecheck` is clean; the new file plus
   `ProjectPanel.context-menu.test.tsx` and `ProjectPanel.reopen-resume.test.tsx` are green; the
   full frontend suite is green (any pre-existing failure must be named and shown to be unrelated).
7. No backend, IPC, type, CSS, dependency, workflow or docs change.

## 9. Proof protocol (for the reviewer)

1. On the branch at `d09beba5`, add `ProjectPanel.agent-matrix-notice.test.tsx` only and run

   ```
   npx vitest run src/sidebar/components/ProjectPanel.agent-matrix-notice.test.tsx
   ```

   Capture the raw red: T1 fails on `agentPicker.modal` (expected `null`, received the mounted
   picker), and the notice assertion never gets to pass. T4 passes on base as the boundary control.
2. Implement section 4. Re-run the same command plus

   ```
   npm run typecheck
   npx vitest run src/sidebar/components/ProjectPanel.context-menu.test.tsx \
     src/sidebar/components/ProjectPanel.reopen-resume.test.tsx \
     src/sidebar/components/ProjectPanel.agent-matrix-notice.test.tsx
   ```

   -> green. Then `npm test` for the full suite.
3. Reply to the coordinator with both raw outputs (pre-fix red at base, post-fix green) and the
   commit SHAs.

## 10. Risks and compatibility

- Behavior surface: only "what a left click on an offline Agent Matrix row does". No persisted
  data, no IPC, no backend, no migration; revert is one `git revert` of the implementation commit.
- Removed code path: the existing-session switch branch in `handleAgentClick` was unreachable from
  this row by construction (`:3249-3252` versus `:633`); the reachable switch path for a live
  matrix row is `SessionItem` and is untouched. T4 pins it.
- Focus: the notice autofocuses its single button; it does not restore focus afterwards (the row
  is not focusable today), which matches the existing project modals.
- Rollout: none. Frontend-only, applies on reload.

## 11. Implementation order

1. New test file alone; capture the pre-fix red (section 9.1).
2. `AgentMatrixNoticeModal.tsx` + the three `ProjectPanel.tsx` edits (section 4).
3. Typecheck, the focused suites, then `npm test`; one implementation commit referencing #2046,
   plus this plan with `git add -f`.

## Plan Contract

No TBD, no open decision, no competing alternative (D1-D4 are decided; alternatives a-f are
closed). Every touched symbol is named with its file and line at the frozen base. The single entry
point for the new behavior is `handleAgentClick` in `src/sidebar/components/ProjectPanel.tsx`, and
the single surface is `AgentMatrixNoticeModal`.
