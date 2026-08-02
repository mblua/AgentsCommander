# Implementation Plan: #1193 Watcher activity toolbar terminology

Status: READY_FOR_IMPLEMENTATION

Lite path. Certified by the architect in the Step 4 pass. No implementation decision is left open (Section 11).

## 1. Issue, baseline and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1193 (`Watcher activity toolbar terminology: SCOPE/AGENT collide with the product's own vocabulary`).
- Branch: `fix/1193-watcher-toolbar-terminology`, created from `main` at `d7285ceb7bda5259e370cc25433d1aa3293c8628`.
- **Baseline for every coordinate and every command in this plan: `d7285ceb`.** The branch has no commits on it yet (`git log main..fix/1193-watcher-toolbar-terminology` is empty), so every `file:line` below is valid at branch HEAD.
- Delivery classification: LITE. Confirmed, not reclassified. Three visible strings in one component. No API, schema, IPC, dependency, persistence or security-boundary change; no behavioural change; no new control; no new state.

**Objective.** Make the activity window's filter bar use the product's own vocabulary: the control that selects among the user's agents is labelled `AGENT`, and the chip group that filters by the CLI behind a session is labelled `Coding-Agent`.

**Non-objective.** No filter selects a different set of rows afterwards. This is not an opportunity to tidy adjacent copy; what was found while checking is reported in Section 10 and deliberately not changed.

## 2. Verified current state

### 2.1 The three strings, at exact coordinates

All three live in `src/watchers/App.tsx`, inside the filter bar (`:573-677`):

| Line | Current source | Rendered as |
| --- | --- | --- |
| `:575` | `<span class="watchers-filter-label">Scope</span>` | `SCOPE` |
| `:583` | `<option value="all">All sessions</option>` | `All sessions` |
| `:616` | `<span class="watchers-filter-label">Agent</span>` | `AGENT` |

Two of the three are uppercased by CSS, not by the source: `.watchers-filter-label` sets `text-transform: uppercase` (`src/watchers/styles/watchers.css:92-97`). The `<option>` is not: `.watchers-select` (`:99-108`) sets no `text-transform`, so option text renders exactly as written.

### 2.2 The collision, verified against the code rather than assumed

The issue's premise is correct, and it is worth stating how it was confirmed, because the whole change rests on it:

- **The chip group at `:614-633` filters by coding agent.** Its options come from `agentOptions()` (`:233-239`), keyed on `row.agentId`, and its label comes from `agentLabel()` (`:183-188`), which resolves that id through `settingsStore.current?.agents`. `AppSettings.agents` is typed `AgentConfig[]` (`src/shared/types.ts:566`), and that is the list the Settings modal presents under the tab labelled **"Coding Agents"** (`src/sidebar/components/SettingsModal.tsx:124`: `{ key: "agents", label: "Coding Agents" }`). So the control labelled `AGENT` today filters by Claude, Codex or a user-defined coding agent.
- **The dropdown at `:576-587` selects among the user's agent sessions.** Its options come from `agentSessions()` (`:170`), which is `sessions().filter((s) => !!s.agentId)`, labelled with `session.name`. The comment at `:168-170` states the rule: only agent sessions are ever registered with the watcher engine. So the control labelled `SCOPE` today is the one that is about the user's agents.

The two labels therefore name each other's concept, which is exactly what the issue reports.

### 2.3 What depends on these strings today

- **No test asserts any of the three.** `src/watchers/App.test.tsx` mentions "Agent" and "All sessions" only inside test names and comments (`:248`, `:553`, `:557`, `:643`); the session-list tests read `option.value`, never the option's text (`optionValues`, `:691-697`). Confirmed by `grep -rn "watchers-filter-label\|All sessions\|>Scope<" --include="*.test.tsx" --include="*.test.ts" src/`.
- **No documentation describes this toolbar.** `grep -rln "Watcher Activity\|watcher activity" docs/` returns nothing. The single "All sessions" hit under `docs/` (`docs/agents/teams-and-workgroups.md:161`) is the unrelated sentence "All sessions terminate cleanly".
- **No script or mockup renders it.** `grep -rn "All sessions\|watchers-filter-label\|watchers.scope" scripts/` returns nothing.
- **No stored key encodes this copy.** The scope is a runtime value pulled through `WindowAPI.getWatchersScope()` (`src/watchers/App.tsx:441`), not a persisted setting. The only persisted state this window owns is its geometry (`:494`).

The consequence is that the rename is unusually safe, and also that nothing would catch a regression of it. Section 9 closes that with one test.

## 3. Scope

### In scope

Exactly three string literals in `src/watchers/App.tsx`, and one new test.

| # | Line | From | To |
| --- | --- | --- | --- |
| 1 | `:575` | `Scope` | `Agent` |
| 2 | `:583` | `All sessions` | `All agents` |
| 3 | `:616` | `Agent` | `Coding-Agent` |

Renames 1 and 3 swap which concept each control names and **must land in the same commit**. Applying either alone leaves two controls labelled `AGENT`, which is strictly worse than today.

### Out of scope

- Every internal identifier: `scopeSessionId`, `scopeIds`, `isAllSessions`, `scopeLimit`, `changeScope`, `fetchScopeKey`, `paintedScopeKey`, `scopeEventGeneration`, `scopeSettled`, `agentFilter`, `agentOptions`, `agentLabel`, `agentChipText`, `agentSessions`, and the `watchers_scope_request` / `get_watchers_scope` IPC names.
- Every `data-ac-testid`: `watchers.scope`, `watchers.filter.agent`, `watchers.filter.agent.${id}`.
- The `<option>`'s `value="all"` and the `value === "all"` mapping in `changeScope` (`:551-555`). The value is a protocol between the `<select>` and the component, not copy.
- CSS. `text-transform: uppercase` stays; the longer label is handled in Section 6, edge case 2.
- Which rows any filter selects. Zero behavioural change.
- The clear button, column widths and maximize persistence (separate issue, per the issue's own Out of scope).
- The adjacent copy listed in Section 10. Reported, not changed.

## 4. The decided solution

Replace the three string literals. Nothing else, in any file.

Decisions taken, so that none is left to the implementer:

| Decision | Taken | Why |
| --- | --- | --- |
| Exact spelling of the chip group label | `Coding-Agent`, hyphenated, exactly as the issue's table specifies | It is a product-owner decision recorded in the issue. It diverges from the app-wide "Coding Agent" spelling; that divergence is reported in Section 10.4 and is **not** silently corrected here. |
| Case written in the source | Title case, as today (`Agent`, `Coding-Agent`) | The CSS uppercases labels at paint time (`:92-97`). Writing `AGENT` in the source would double-encode a presentation rule and would look wrong the day the CSS changes. |
| Option text | `All agents`, sentence case, matching the sibling options (session names) | The `<option>` is not uppercased by CSS, so its text is what the user reads verbatim. |
| Test ids and setting keys | Unchanged | The issue requires copy only, and nothing fails to compile without renaming them. |
| Order of the two swapped labels | One commit, both edits | See Section 3. |

## 5. Affected surfaces: exact files and symbols

### 5.1 `src/watchers/App.tsx` (the only production file)

Three single-line edits inside the `watchers-filter-bar` block (`:573-677`).

```diff
-          <span class="watchers-filter-label">Scope</span>
+          <span class="watchers-filter-label">Agent</span>
```

```diff
-            <option value="all">All sessions</option>
+            <option value="all">All agents</option>
```

```diff
-            <span class="watchers-filter-label">Agent</span>
+            <span class="watchers-filter-label">Coding-Agent</span>
```

The surrounding JSX is untouched: the `<select>` keeps `ref`, `onChange`, `data-ac-testid="watchers.scope"` and `data-ac-role="combobox"`; the chip group keeps its `Show` guard (`isAllSessions() && agentOptions().length > 0`), its `data-ac-testid="watchers.filter.agent"` and every per-chip attribute.

The comment at `:611-613` ("In single-session scope Agent and Workgroup have one possible value each and filter nothing...") describes the `Show` guard using the old label. Update the word `Agent` to `Coding-Agent` in that comment so it keeps naming the control it is attached to. This is a comment on the lines being changed, not adjacent tidying.

### 5.2 `src/watchers/App.test.tsx`

One new test, Section 9. No existing test is modified.

### 5.3 Files deliberately not touched

`src/watchers/styles/watchers.css`, `src/watchers/activity.ts`, `src/watchers/components/WatchersTitlebar.tsx`, `src/shared/**`, `src/sidebar/**`, `docs/**`, and everything under `src-tauri/`. If the implementation needs any of them, the change was misunderstood.

## 6. Required behaviour, edge cases and behaviour on failure

| # | Situation | Required behaviour |
| --- | --- | --- |
| 1 | "All sessions" scope | Toolbar reads `AGENT [All agents]  WATCHER (...)  CODING-AGENT (...)  WORKGROUP (...)`. Every chip, every row and every count is identical to before. |
| 2 | Long label wrapping | `Coding-Agent` renders as `CODING-AGENT`, 12 characters against the previous 5. `.watchers-filter-bar` (`src/watchers/styles/watchers.css:75-83`) and `.watchers-filter-group` (`:85-90`) are both `display: flex` with `flex-wrap: wrap`, so a narrow window wraps the group instead of overflowing. No CSS change, and no horizontal scrollbar can appear from this. |
| 3 | Single-session scope | The chip group is not rendered at all (`:614`), so only `AGENT [<session name>]` and `WATCHER` show. Unchanged behaviour, new label. |
| 4 | A session is created, destroyed or renamed | Unchanged. The option list is rebuilt from `agentSessions()`; only the fixed `all` option carries new text. |
| 5 | The window opens scoped to a session | Unchanged. The effect at `:205-212` still drives `scopeEl.value`, which is `"all"` or a session id, never the label. |
| 6 | A user's coding agent is renamed in Settings | Unchanged. Chip text still comes from `agentLabel`/`agentChipText` (`:249-252`), which the group's label never fed. |
| 7 | Failure behaviour | There is none to define. No branch, no async work, no new failure path is introduced; the change cannot fail at runtime, only at compile time, which the typecheck catches. |

## 7. Compatibility and security

- **IPC.** Unchanged: no command, event, payload or type. No Rust change.
- **Persistence.** Unchanged: no settings key, no TOML shape, no migration. Nothing stored anywhere contains these strings (Section 2.3).
- **Accessibility.** `data-ac-role="combobox"` and the chips' `aria-pressed` are unchanged. The labels are `<span>`s, not `<label for=...>`, before and after, so no association is created or lost.
- **Security.** No new surface. All three strings are static literals with no interpolation.
- **Rollback.** Reverting the commit restores the previous copy exactly; nothing else has to be undone.

## 8. Implementation order

Two commits: the plan, then the change.

0. Commit this plan file first, as its own commit, exactly as #1177's plan landed (`092d85cd`). **`plans/` is in `.gitignore` (`.gitignore:11`)**, so the file needs `git add -f plans/1193-watcher-toolbar-terminology.md`; a plain `git add` silently adds nothing and the plan never reaches the branch.
1. Apply the three edits of Section 5.1 plus the comment at `:611-613`.
2. Add the test of Section 9.
3. `npm run typecheck`, `npm test`, `npm run test:debt`.

Steps 1 and 2 are one commit. The two swapped labels must not be split across commits (Section 3).

## 9. Tests and acceptance criteria

### 9.1 The test

One test in `src/watchers/App.test.tsx`, added next to the existing scope tests. It exists because nothing today would catch a regression of this copy (Section 2.3), and because renaming only one of the two swapped labels is the specific mistake the issue warns about, which a single test can make impossible.

**T1: "the toolbar labels the dropdown Agent and the chip group Coding-Agent".**

Setup is the existing test at `:248` verbatim: `transportWith(snapshot({ matches: [match()] }))`, render with `initialSessionId="s1"`, wait for `watchers.table`, then switch the `<select>` to `"all"` and wait for `watchers.filter.agent` to appear, which is what makes the chip group visible (`:614`).

Assertions, all three in the one test so that a half-applied swap fails:

```ts
const scope = rendered.root.querySelector<HTMLSelectElement>(
  '[data-ac-testid="watchers.scope"]'
)!;
// The dropdown is the control that selects among the user's agents.
expect(
  scope.parentElement?.querySelector(".watchers-filter-label")?.textContent
).toBe("Agent");
expect(
  rendered.root.querySelector('[data-ac-testid="watchers.scope"] option[value="all"]')
    ?.textContent
).toBe("All agents");
// The chip group is the one that filters by the CLI behind the session.
expect(
  rendered.root.querySelector(
    '[data-ac-testid="watchers.filter.agent"] .watchers-filter-label'
  )?.textContent
).toBe("Coding-Agent");
```

Two notes that are part of the specification. The assertions read the **source** casing: `text-transform` is a paint-time rule and jsdom does not apply it to `textContent`, so asserting `"AGENT"` would fail against correct code. And the test id stays `watchers.filter.agent` on purpose (Section 3), so the selector above is stable across this change by design.

### 9.2 Acceptance criteria

Objective and individually checkable:

1. `npx tsc --noEmit` exits 0 with no diagnostics.
2. `npm test` is green. The count under `src/watchers/` goes from 71 (verified at `d7285ceb`: `npx vitest run src/watchers` reports `PASS (71) FAIL (0)`) to 72, and no existing test is modified, renamed or deleted.
3. `npm run test:debt` exits 0.
4. Five mechanical greps over `src/watchers/App.tsx`, which need no judgement about what is a comment. Each pattern anchors on the `>` that opens the text node, so `Coding-Agent</span>` cannot satisfy the `>Agent</span>` one:

   | Command | Expected |
   | --- | --- |
   | `grep -c '>Scope</span>' src/watchers/App.tsx` | `0` |
   | `grep -c '>All sessions</option>' src/watchers/App.tsx` | `0` |
   | `grep -c '>Agent</span>' src/watchers/App.tsx` | `1`, and it is `:575` |
   | `grep -c '>Coding-Agent</span>' src/watchers/App.tsx` | `1`, and it is `:616` |
   | `grep -c '>All agents</option>' src/watchers/App.tsx` | `1`, and it is `:583` |
5. `git diff --stat d7285ceb..HEAD` touches exactly three files: `plans/1193-watcher-toolbar-terminology.md` (force-added, see Section 8 step 0), `src/watchers/App.tsx` and `src/watchers/App.test.tsx`. If the plan file is missing from that diff, the force-add was skipped.
6. Every line changed in `src/watchers/App.tsx` falls inside the filter-bar block (`:573-677`): the three literals plus the comment at `:611-613`. A changed line outside that range means the scope was exceeded.
7. Manual check, once, in a real build: open the activity window in "All sessions" scope and confirm the bar reads `AGENT [All agents] ... CODING-AGENT (...)`, that selecting a session in the dropdown still narrows the table to that session, and that clicking a `CODING-AGENT` chip still filters by that CLI.

## 10. Adjacent surfaces: the survey the issue asked for

Checked at `d7285ceb` across the sidebar, the Settings modal, the guide and every tooltip. Reported, **not changed**.

### 10.1 One genuine instance of the same collision

`src/sidebar/components/AgentPickerModal.tsx:803` renders the panel title **"Same Profile In Other Agents"**, where "Agents" means coding agents. Two lines below, `:805` names the same set correctly: "compared across configured Coding Agents". This is the identical defect: a bare "Agents" used for the coding-agent list inside a product where an agent is the user's own. Recommendation: its own copy issue, for the product owner to decide the wording.

### 10.2 A stale reference in the guide

`src/guide/components/TutorialTab.tsx:21` tells the user to use "the 'Open Agent' button in the sidebar to launch a Coding Agent". No component renders that label at `d7285ceb`: `grep -rn "Open Agent" --include="*.tsx" src/` matches only that sentence, and the CSS class `toolbar-open-agent-btn` (`src/sidebar/styles/sidebar.css:882`, `:901`) has no consumer in TSX. So the guide both uses the bare "Agent" for a coding agent and appears to describe a control that no longer carries that name. Recommendation: hand to whoever owns the guide; it needs the current UI checked, not just a rename.

### 10.3 Residual mismatch left inside this very window

After the rename the dropdown says "All agents", while two nearby strings still speak of sessions:

- `src/watchers/App.tsx:792`: the table column header `Session`, which is what identifies which of those agents a row came from.
- `src/watchers/App.tsx:727`: the empty state "No configured watcher reaches this session's agent."

Both are outside the three renames the product owner asked for, so they stay. Reported because the mismatch is now visible on the same screen, and because "this session's agent" reads ambiguously once the toolbar has redefined "agent".

### 10.4 The hyphen

`Coding-Agent` is hyphenated, while every other surface in the app writes "Coding Agent" with a space: `SettingsModal.tsx:124` ("Coding Agents"), `SessionItem.tsx:542`, `RootAgentBanner.tsx:598`, `ProjectPanel.tsx:3387` and `:3457`, `AgentPickerModal.tsx:594` and `:609`, `CodingAgentQuickConfiguration.tsx:160` and `:247`, `OnboardingModal.tsx:12` and `:37`, `RestartPromptModal.tsx:28`. Implemented as specified, because the issue's table is explicit and it is a product-owner decision. Flagged so the inconsistency is chosen rather than accidental.

### 10.5 What is already correct, checked and clear

- Settings: the tab is "Coding Agents" (`SettingsModal.tsx:124`), and the container section says "Container Coding Agents" (`:1915`). No collision.
- Sidebar surfaces that mean the user's agents use the bare word correctly: "AC Agents" (`AcDiscoveryPanel.tsx:213`), "New Agent" (`NewEntityAgentModal.tsx:207`), "Delete Agent" (`ProjectPanel.tsx:2969`), "Agents" as a workgroup section (`ProjectPanel.tsx:2852`), "Root Agent" (`RootAgentBanner.tsx:62`, `:366-368`).
- Tooltips: no `title` attribute anywhere in `src/**/*.tsx` uses "agent" for a coding agent. `grep -rn 'title="[^"]*[Aa]gent' --include="*.tsx" src/` returns nothing outside tests.
- Before this change, `src/watchers/App.tsx:616` was the only bare `Agent` label in the whole frontend that named a coding agent.

### 10.6 A nuance about what "All agents" now names

`agentSessions()` is every session that has a coding agent assigned (`:170`), which in practice is the agent replica sessions but is not by construction limited to them. So "All agents" reads as "every agent session currently running", not "every agent defined in the matrix". Accurate for what the control does, and the wording is the product owner's; recorded only so nobody later reads the option as a promise about the matrix.

## 11. Open decisions

None. The spelling, the casing, the option text, the untouched identifiers and test ids, the single-commit constraint and the test are all fixed above.
