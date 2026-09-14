# #2014 Coding Agent modal v4: layout A, launch lines, left-only filter

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2014 (open; Spanish; authoritative acceptance criteria)
- Branch: `feature/2014-coding-agent-modal-v4`
- Planning base (frozen): `405b7afb02c18b3a6c37376a9a0a3c34badf8d60` (= `origin/main` = remote branch head, checked with `git ls-remote` at planning time)
- Band: Lite, score 27. One phase, no partition (`PARTITION: N/A`, not a Full plan).
- Owner: `ac-dev-webpage-ui-v4`. Reviewer: Grinch. Coordinator: `ac-tech-lead-v4`.
- Reference: `D:\0_repos\AgentsCommander_iac\.ac\project-shared\coding-agent-prototype\index-v4.html`, SHA-256 `6a78afcf576810b3e942295e7d4dc0f4cffb3ab85f505575dac27035caeeedee`. Its sample data, provenance note, capture aid and fixed CSS zoom (1.2) are NOT to be ported.

## 0. Task class and threat model

Routine frontend application change. No backend, IPC, persisted shape, dependency, workflow or release change. No enhanced controls apply (no signing, packaging provenance, untrusted host, security boundary or migration). Tool trust = repository `package-lock.json` + `npm ci` + exact-head GitHub CI.

## 1. Decisions (with evidence)

### D1. The launch line is computed on the frontend, no backend change

The backend spawn line and the frontend preview use the same three rules. Checked in current code:

| Rule | Backend (spawns) | Frontend (modal) | Same? |
|---|---|---|---|
| Fallback walk | `src-tauri/src/config/coding_agent_profiles.rs:1387-1400`: requested letter down to A; first ENABLED cell wins; A always wins (empty cell if A disabled/absent) | `src/shared/profile-utils.ts:153-164` same walk; `AgentPickerModal.tsx:276-279` `enabledLaunchCellFor` returns `EMPTY_DISPLAY_CELL` for a disabled/absent cell | yes |
| Enabled-only cells | `coding_agent_profiles.rs:1303` `.filter(|cell| cell.enabled)` | `profile-utils.ts:89` and `AgentPickerModal.tsx:278` | yes |
| Compose | `src-tauri/src/config/agent_command.rs:721-730` trim both, one space, empty side drops; used at `:830-831` | `profile-utils.ts:97-103` identical; mirrored tests `profile-utils.test.ts:157-162` vs `agent_command.rs:2596-2609` | yes |

The requested letter in the comparison is the modal's `selectedProfile()`. Assigning writes that letter as the replica's profile, and the backend then ranks it first (`coding_agent_profiles.rs:1375-1380`), so "this agent with this profile" is exactly what would launch.

`comparisonRows` (`AgentPickerModal.tsx:334-357`) already computes that string as `command` for the status. The change is to return it and show it. No new computation, no duplicated args.

Placeholder note (accepted limit, documented, not blocking). The backend expands `%AC_*%` per token after tokenizing, against the canonicalized launch root (`agent_command.rs:858-901`, `placeholders.rs:48-137`). The frontend shows `expandAcPlaceholdersPreview` against the modal's target path (`profile-utils.ts:131-142`). The texts are equal for commands without placeholders. With a placeholder they differ only in path spelling (canonical form), or when the backend would refuse the spawn (a `%AC_REPLICA_ROOT%` on a non-AC path). The line shown keeps the user's quotes as typed; the backend removes them while tokenizing. This is the same preview the modal already uses for env values (`:320`). Changing it would need a new backend command, out of scope for a Lite plan.

### D2. What the filter matches

Match text for an agent = `agent.label` + " " + that agent's launch line from D1 (same map as the right panel, keyed by `agent.id`). Lowercase both sides; trim the query; substring test. Because the base command is the first part of the line, the executable and every arg are covered.

Consequence (stated, not hidden): the line depends on the selected profile, so changing the profile can change which cards match. Filtering never changes the profile; only the user's own profile click does.

### D3. Filter hides cards without re-indexing

`highlightIndex` indexes `sortedAgents()` (`:134`, `:235`). Keep `<For each={sortedAgents()}>` and wrap each card in `<Show when={matchesFilter(agent)}>`. NEVER iterate a filtered array with its own index: that would silently move the selection. The right panel keeps reading `comparisonRows()` from `sortedAgents()` and never reads the filter signal.

### D4. Keys typed in the filter must not act on the modal

`handleKeyDown` (`:1036-1063`) is on the overlay, so keys typed in the input bubble up. ArrowLeft/Right would call `moveProfile` and change the profile while editing text; ArrowUp/Down would move the selection. Enter is already guarded by `isInteractiveTarget` (`:1028-1034`, matches `input`). Fix: at the top of `handleKeyDown`, after the Escape branch, return early when `e.target` is the filter input and the key is an Arrow key. Escape keeps closing the modal (unchanged behavior).

### D5. Layout A

DOM order inside `.agent-picker-modal`: header, `.agent-profile-assignment-body`, the `<Show when={showBroadScope()}>` block (lock bar, lock diagnostic, remove errors, kind list, Matrix default) moved as ONE unit from `:1096-1303` to right after the body, then `agent-picker-error`, then `.agent-picker-botonera`. `.agent-picker-bar` stays the last child of the botonera, and the botonera stays the last child of the modal. So the action bar is the last block, and Cancel and Assign exist once each. The conflict overlay stays a sibling of the modal (unchanged).

Zoom: in Tauri, zoom is native `setZoom` (`src/shared/zoom.ts:57-59`), so `100vh`/`100vw` follow the real window.

## 2. Exact scope

Only these tracked paths change (plus this plan, already tracked):

1. `src/sidebar/components/AgentPickerModal.tsx`
2. `src/sidebar/components/AgentPickerModal.test.tsx`
3. `src/sidebar/styles/sidebar.css`

No change to `src/shared/**`, `src-tauri/**`, `package*.json`, workflows, types. No new import in any file. Any other path in the diff blocks delivery.

## 3. Implementation steps

### 3.1 `AgentPickerModal.tsx`

1. `comparisonRows`: add `launchLine: command` to the returned object. Nothing else changes there.
2. Add `const launchLineByAgentId = createMemo(() => new Map(comparisonRows().map((row) => [row.agent.id, row.launchLine])));`
3. Add `const [agentFilter, setAgentFilter] = createSignal("");`, `const filterQuery = createMemo(() => agentFilter().trim().toLowerCase());`, `const matchesFilter = (agent: AgentConfig) => { const q = filterQuery(); return q === "" || `${agent.label} ${launchLineByAgentId().get(agent.id) ?? agent.command}`.toLowerCase().includes(q); };` and `const visibleAgentCount = createMemo(() => sortedAgents().filter(matchesFilter).length);`
4. Comparison row: replace `{row.active ? "selected coding agent" : "configured peer"}` with `{row.launchLine || "none"}` on the same `agent-comparison-agent-sub` span, and add `data-ac-testid={`agentPicker.comparison.row.${row.agent.id}.launchLine`}` to it. Keep `aria-pressed`-free row semantics; the active row is still marked by `classList.active` and `data-ac-state`.
5. Provider panel: between `.agent-profile-panel-head` and `.agent-profile-provider-list`, render (only when `sortedAgents().length > 0`):
   - wrapper `div.agent-profile-provider-filter`
   - `<label for="agentPickerAgentFilter">Filter by name or start line</label>`
   - `<input id="agentPickerAgentFilter" type="search" autocomplete="off" spellcheck={false} placeholder="name or command + args" aria-controls="agentPickerAgentList" value={agentFilter()} onInput={(e) => setAgentFilter(e.currentTarget.value)} data-ac-testid="agentPicker.agentFilter" />`
   - `<div class="agent-profile-provider-filter-status" role="status" aria-live="polite" data-ac-testid="agentPicker.agentFilterStatus">` with text: empty query `N agents`; matches `M of N agents match "<trimmed>".`; none `No coding agent matches "<trimmed>". Clear the filter to see all N.`
   - give the list `id="agentPickerAgentList"`.
6. Provider `<For>`: wrap the returned `<button>` in `<Show when={matchesFilter(agent)}>`. Keep `i()` from `sortedAgents()`.
7. `handleKeyDown`: after the Escape branch add `if (e.target instanceof HTMLElement && e.target.id === "agentPickerAgentFilter" && e.key.startsWith("Arrow")) return;`
8. Move the `<Show when={showBroadScope()}>` block (with its leading comment) from before the body to directly after the body's closing `</div>`, before `<Show when={error()}>`. No edits inside the block.

### 3.2 `sidebar.css`

All new rules are scoped under `.agent-picker-modal` so no other surface that shares `.selection-lock-*` classes can change. Port from `build/v4-framing.css` sections 1-6, `[V4-STARTUP-LINES]` and `[V4-FILTER]`, replacing `.pp-v4-frame` with `.agent-picker-modal` and dropping the `/ 1.2` zoom factor:

- `.agent-picker-modal { width: calc(100vw - 6px); max-width: none; height: calc(100vh - 6px); max-height: none; overflow-x: hidden; overflow-y: auto; scrollbar-gutter: stable; }` (+ 4 px scrollbar rules). `overflow-y: auto` is a safety valve for small windows only.
- header, botonera, `.selection-lock-bar`, `.selection-lock-future`: `flex-shrink: 0`. Body: `flex: 1 1 auto; min-height: 0` (already set at `:4355-4363`; keep).
- Compact lock bar and Matrix default grids, wrapping pair and hint (no nowrap/ellipsis), order-dependent borders (lock bar top border; botonera `border-top: 0`) exactly as v4 sections 3-6.
- `.agent-picker-modal .agent-comparison-table-head, .agent-picker-modal .agent-comparison-row { grid-template-columns: minmax(0, 1fr) auto; }` and the monospace wrapping `.agent-comparison-agent-sub` rule (`white-space: normal; overflow-wrap: break-word; text-overflow: clip`).
- `.agent-picker-modal .agent-profile-provider-panel { grid-template-rows: auto auto minmax(0, 1fr); }` and `.agent-profile-provider-filter`, `-filter label`, `-filter input` (+ `:focus`, `::placeholder`), `-filter-status` copied from `[V4-FILTER]` with renamed classes.
- Do not touch the `@media (max-width: 900px)` block except to verify it still stacks.

### 3.3 Tests (`AgentPickerModal.test.tsx`)

Deliberate updates to existing assertions (they encode the old behavior):

- `:619` and `:703` `not.toContain("claude --dangerously-skip-permissions")` become `expect(text("agentPicker.comparison.row.claude.launchLine")).toBe("claude claude --dangerously-skip-permissions")`. (The fixture cell repeats the binary; the backend composes it the same way.)
- `:2085` "places the lock bar above the three-panel body" becomes "places the lock bar below the three-panel body": `body.compareDocumentPosition(bar) & DOCUMENT_POSITION_FOLLOWING` truthy; the review-over-modal part is unchanged.

New tests (names are the contract Grinch checks):

- **L1 "shows each agent's full launch line instead of configured peer"**: profile A; codex row line `codex codex --model gpt-5`, claude row line `claude claude --dangerously-skip-permissions`; `text("agentPicker.comparison")` does not contain `configured peer` nor `selected coding agent`.
- **L2 "launch line follows the effective profile after fallback"**: click profile C; codex line `codex codex --profile fast` (C falls to B), claude line `claude claude --dangerously-skip-permissions` (C falls to A); rows keep `data-ac-profile-status="fallback"`.
- **L3 "launch line ignores a disabled cell and mirrors the backend fallback case"**: settings where codex has A `codex --a`, C `codex --c`, D disabled `codex --d`; select D (profileSlots include D); codex line `codex codex --c`. This is the TS twin of Rust `profile_content_hash_uses_effective_cell_after_fallback` (`agent_command.rs:2562-2593`).
- **F1 "filters only the left list by name, executable, argument and case"**: queries `CLAUDE` (only claude card), `codex` (only codex), `--model` (only codex, arg only in the line), `Dangerously` (only claude). Card presence via `maybe("agentPicker.provider.<id>")`.
- **F2 "right panel is byte-identical under every filter query, including zero matches"**: `const before = target("agentPicker.comparison").outerHTML;` then for each of `claude`, `--model`, `zzz-no-match`, `` (empty): set input value, dispatch `input`, `await settle()`, `expect(target("agentPicker.comparison").outerHTML).toBe(before)`. For `zzz-no-match`: both cards absent and status text `No coding agent matches "zzz-no-match". Clear the filter to see all 2.`; after empty: both cards present, status `2 agents`.
- **F3 "filtering never changes selection, profile, radios or buttons"**: WG scope context; click claude card and profile B; capture `data-ac-state` of every `[data-ac-testid^="agentPicker."]` element except `agentPicker.provider.*`, `agentPicker.agentFilter*` into a map, plus `apply.disabled`. Filter `codex` (claude card hidden), then clear. Map and `disabled` equal before/after; after clear claude card `data-ac-state="active"`; `applyCodingAgentProfileSelection`, `previewCodingAgentProfileSelection` call counts unchanged across the filter steps; `onSelect` not called.
- **F4 "keys typed in the filter do not move profile or selection"**: focus input; dispatch bubbling `keydown` ArrowRight, ArrowLeft, ArrowDown, Enter on the input; profile A and codex stay active; apply mock not called.
- **F5 "filter is labelled and placed right before the first card"**: `input.labels[0].textContent` is `Filter by name or start line`; filter wrapper's `nextElementSibling` is the provider list.
- **O1 "layout A order: body, lock bar, Matrix default, Apply to, action bar last"**: with `renderLockPicker()`: children order `header < body < lockBar < defaultSection < botonera` by `compareDocumentPosition`; `modal.lastElementChild` is `.agent-picker-botonera`; `botonera.lastElementChild` is `.agent-picker-bar`; exactly one `agentPicker.cancel` and one `agentPicker.apply` in the document.

## 4. Positive controls (mutants Grinch can run)

Materialise each mutant in the working tree, run `npx vitest run src/sidebar/components/AgentPickerModal.test.tsx`, confirm the named test FAILS, then restore the file with `git diff` checked empty for that hunk. Any mutant that stays green blocks approval.

| # | Mutant | Must fail |
|---|---|---|
| M1 | `comparisonRows` maps `sortedAgents().filter(matchesFilter)` | F2 |
| M2 | haystack uses `agent.label` only | F1 (`--model`) |
| M3 | drop `.toLowerCase()` on the haystack | F1 (`CLAUDE`) |
| M4 | provider `<For each>` over the filtered list, index from that list | F3 |
| M5 | launch line = `agent.command` only | L1 |
| M6 | line built from the requested (not effective) profile cell | L2 |
| M7 | remove the Arrow early return in `handleKeyDown` | F4 |
| M8 | move the lock `<Show>` block back above the body | O1 |

## 5. Geometry proof (jsdom has no layout, so this is done in real engines)

### G1. Real DOM + real CSS in Chrome (owner dev; before PR)

Recipe (memory-proven, `.visual-specs/` is gitignored, `.gitignore:9`):

1. Disposable probe `src/sidebar/components/__probe2014.test.tsx` mounts the real `AgentPickerModal` with the real mocks and a heavy fixture: 14 agents with launch lines up to 120 characters, WG scope context, `selectionState: "locked"` with a saved pair, a saved Matrix default. It writes `document.body.innerHTML` for state S1 (replica scope) and S2 (kind scope selected, 3-target preview) to `.visual-specs/2014/s1.html` and `s2.html`. Run by path, then DELETE the probe and prove `git status --porcelain` lists no probe.
2. One page per state with `<link href="/src/sidebar/styles/sidebar.css">`, served from the repo root on 127.0.0.1; measured inside iframes sized exactly 1280x800, 1400x1000, 1920x1080.
3. Measure with a script and save `.visual-specs/2014/geometry.json` with, per state and size:
   - modal top gap and bottom gap: each 3 ± 0.5 px
   - document `scrollHeight - clientHeight` = 0 and modal `scrollHeight - clientHeight` <= 0.5
   - every element under lock bar, Matrix default and botonera: rect inside modal and viewport; for text elements `scrollWidth <= clientWidth + 0.5` and computed `text-overflow` is not `ellipsis`
   - order by `top`: body < lock bar < Matrix default < scope stack < `.agent-picker-bar`; bar bottom is the largest bottom in the modal
   - comparison rows: `.agent-comparison-agent-sub` computed `white-space: normal`, no horizontal overflow
   - filter invariance in a real engine: comparison `outerHTML` identical before/after typing `claude` and `zzz-no-match`
   - variant C body height as % of modal (report only; no 75% floor)
4. Threshold sweep for the issue's "smaller sizes": width 1280, height from 800 down to 500 in 20 px steps; height 800, width from 1280 down to 900 in 20 px steps. Report the first size where any S1 check fails and confirm the modal scroll (safety valve) keeps every control reachable there.

Pass = all S1 and S2 checks at the three target sizes. If S2 fails at a target size, STOP and report to the coordinator (product decision; do not trade content for height silently).

### G2. App WebView (owner dev; before PR)

1. `npm run build:prod:no-bundle` (also runs `scripts/copy-testable-binary.mjs`, producing `target/release/agentscommander_testeable.exe`). Record path and SHA-256.
2. Launch it with the hidden `--window-width/--window-height` flags at the three sizes (native zoom 1.0), open the Coding Agent modal on a WG replica, and run `ui-query` for `agentPicker.modal`, `agentPicker.lockBar`, `agentPicker.defaultSection`, `agentPicker.scope`, `agentPicker.apply`, `agentPicker.cancel`, `agentPicker.agentFilter`, and one `agentPicker.comparison.row.*.launchLine`.
3. Pass: `viewport` from diagnostics equals the window content size; modal `rect.y` ≈ 3 and `rect.y + rect.height` ≈ `viewport.height - 3` (± 1); every other rect is non-null and its bottom <= modal bottom; the launch line text equals the configured `command + cell` for that agent in the local settings (write the expected string down before querying).
4. Repeat 1400x1000 once with native zoom 1.2 and report the result (the safety-valve scroll is acceptable there; record it).
5. Save the raw JSON outputs under `.visual-specs/2014/app/`. Do not use or modify the maintainer's running instance or its config dir.

If the bridge cannot open the modal or the build fails, report the blocker with its log; G1 does not replace G2.

## 6. Delivery gates

| Gate | Evidence | Owner / time | Failure |
|---|---|---|---|
| Git preconditions | `git -C <repo> status --porcelain` empty; `git rev-parse --abbrev-ref HEAD` = `feature/2014-coding-agent-modal-v4`; `git fetch origin main`; `git merge-base --is-ancestor 405b7afb origin/main`; record `git rev-parse HEAD` as phase base | dev, before first edit | stop, report |
| Drift | `git diff --name-only 405b7afb origin/main -- src/sidebar src/shared package.json package-lock.json .github/workflows` | dev, before first edit and before PR | any hit: refresh only affected evidence, tell coordinator |
| Scope | `git diff --name-only <phase-base>...HEAD` = exactly the 3 paths of section 2 (plus the plan, if updated by architect only); `git status --porcelain` empty | dev before PR; Grinch | extra path blocks |
| No new imports / cycles | `git diff <phase-base> -- src | grep -E '^[+-]import'` prints nothing; `npm run check:frontend-dependencies` passes | dev; Grinch | blocks |
| Typecheck | `npm run typecheck` exit 0 | dev | blocks |
| Targeted tests | `npx vitest run src/sidebar/components/AgentPickerModal.test.tsx src/shared/profile-utils.test.ts` all pass, new tests L1-L3, F1-F5, O1 present by name | dev; Grinch re-runs | blocks |
| Full frontend tests | `npm test`; only the known #480 signature tolerated as CI does | dev | unexpected failure blocks |
| Build | `npm run build` exit 0 | dev | blocks |
| Positive controls | section 4 table, each mutant red then restored | Grinch | green mutant blocks |
| Geometry | G1 `geometry.json` + G2 app JSON, attached to handoff | dev; Grinch checks numbers | blocks |
| PR | one PR into `main`, head `feature/2014-coding-agent-modal-v4`, body `Closes #2014` | dev | wrong base/head blocks |
| CI | every triggered and required check of `pr-regression-gates.yml` (including `frontend-regression`: `npm run typecheck`, `npm test` with #480 guard) green on the exact PR head SHA | dev reports; coordinator verifies | red or skipped-unexplained blocks merge |

Commands run with explicit cwd = repo root and runner timeouts; keep failing logs under `.visual-specs/2014/logs/` until reported.

Recovery: if a run fails mid-edit, restore only the 3 scoped paths and only if `git diff` shows the change is this run's own work (`git restore --source=<phase-base> -- <path>`). No `git reset --hard`, no repo-wide clean. Delete the disposable probe and serve scripts; they must not be committed.

## 7. Out of scope

Backend line preview command; ArrowUp/Down skipping hidden cards; persisting the filter; prototype provenance note, capture aid, sample agents, CSS zoom; any change to assignment, scopes, locks, conflicts or Matrix default logic.

## 8. Handoff content (dev to coordinator)

Phase base SHA, PR URL, tested head SHA, file list, test output summary (counts + new test names), mutant table results if run, `geometry.json` summary table (3 sizes x S1/S2 + threshold), G2 JSON summary with exe SHA-256, CI run URL for the head.
