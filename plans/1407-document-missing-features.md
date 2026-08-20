# Plan #1407: Document every shipped AgentsCommander feature that has no user-facing documentation

Author: architect, wg-14. Authored 2026-08-17 UTC.

Status: READY_FOR_IMPLEMENTATION

Certified by the architect on 2026-08-17 UTC after consensus round 1. Section 11 is the `dev-rust` enrichment, section 12 the `dev-rust-grinch` adversarial review, and section 13 the architect's disposition of all twelve findings, all of which were accepted and folded into sections 1 to 10.

Issue: [mblua/AgentsCommander#1407](https://github.com/mblua/AgentsCommander/issues/1407).

This is a **documentation-only** plan. It creates 16 new files under `docs/` (14 feature pages, the features index, and one reference page), edits 10 existing files under `docs/` (`reference/architecture.md`, `reference/settings.md`, `reference/directory-layout.md`, `features/session-auto-close.md`, `agent-matrix-conventions.md`, `concepts.md`, `glossary.md`, `quickstart.md`, `home-en.md`, `security.md`), and changes **zero** lines of application code. It adds no crate, no npm dependency, no Rust module, no TypeScript module, no Tauri command, no event, no settings key and no migration. It adds zero module-to-module dependency arcs.

---

## 1. Frozen authority and fail-closed entry gate

The implementation working tree is `repo-AgentsCommander`, branch `docs/1407-document-missing-features`, targeting `main`.

Frozen base SHA: `51e70e47f442109d6b618299b26d95a12801f156`.

Verified on 2026-08-17 UTC: committed `HEAD` equals the frozen SHA, the branch is `docs/1407-document-missing-features`, and `git status --porcelain=v1 --untracked-files=all` was empty.

Branch-name validation was checked against `scripts/validate-branch-name.mjs:15`. The pattern is `^(bug|chore|ci|docs|feat|feature|fix|refactor|style|test)\/([1-9][0-9]*)-([a-z0-9]+(?:-[a-z0-9]+)*)$` with `MAX_SLUG = 50`. `docs/1407-document-missing-features` matches with type `docs`, number `1407`, slug `document-missing-features` (25 characters). The required `validate-branch-name` check will pass.

Root `.gitignore:11` ignores `/plans/`, so this plan file must be force-added: `git add -f plans/1407-document-missing-features.md`. Do not remove or weaken that ignore rule.

Every line number in this plan refers to the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

**Entry gate for each batch.** Before starting any batch, the implementer runs, from the repo root:

```bash
git rev-parse HEAD
git branch --show-current
git status --porcelain=v1 --untracked-files=all
```

Proceed only if the branch is `docs/1407-document-missing-features` and the only dirty entries are files this plan authorizes. Never rebase onto a moved `main` under this plan without re-certification.

---

## 2. Objective and non-goals

**Objective.** Every shipped feature listed in issue #1407 ends up with product documentation a user can read: a page under `docs/features/`, a section inside an existing doc, or a glossary/concept entry. `docs/features/` gains an index. The four inaccurate or incomplete statements in `docs/reference/architecture.md` are corrected. The one genuinely missing settings key is added to `docs/reference/settings.md`.

**Non-goals, binding on the implementer:**

- Do **not** modify any file under `src/`, `src-tauri/`, `scripts/`, `.github/`, `package.json`, `Cargo.toml`, or `tauri.conf.json`. This plan changes documentation only.
- Do **not** modify the two inventory message files in `.ac/wg-14-dev-v5-team/messaging/`.
- Do **not** modify anything under `docs/testing/`. Those are QA scripts, not product documentation, and are explicitly out of scope (confirmed by the tech-lead).
- Do **not** rename, move, or delete any existing file under `docs/`. Every change to an existing doc is an addition or an in-place correction of a named line.
- Do **not** renumber the existing H2 sections of `docs/agent-matrix-conventions.md`. `docs/features/coding-agent-profiles.md:169` links to the anchor `#5-profile-path-placeholders`; renumbering would break it.
- Do **not** create `mkdocs.yml` or any site-generator config. The index is a plain `README.md`.
- Do **not** document the intentionally hidden internal verbs (`role-experiment`, `test-reset`, `window-info`, `ui-*`). `docs/reference/cli.md:42` declares their exclusion by design.
- Do **not** add a page for a feature already covered. See section 3.2.

---

## 3. Verified current state

### 3.1 Inputs

Two read-only inventories produced at the frozen SHA, with `path:line` evidence, are the scope input. Do not redo the inventory.

- Backend/CLI: `.ac/wg-14-dev-v5-team/messaging/20260817-184003-wg14-dev-rust-to-wg14-tech-lead-features-doc-gap-inventory.md` (items `C1`-`C6`, `B1`-`B10`).
- Frontend `src/`: `.ac/wg-14-dev-v5-team/messaging/20260817-183737-wg14-dev-webpage-ui-to-wg14-tech-lead-ui-features-doc-gap-inventory.md` (items `UI-C1`-`UI-C15`, `UI-B1`-`UI-B12`, plus three "docs describe UI wrongly" findings).

Throughout this plan, backend items are cited as `C<n>` / `B<n>` and frontend items as `UI-C<n>` / `UI-B<n>`.

### 3.2 Three corrections to the inventories, verified at the frozen SHA

These three items are **removed from scope**. They are false positives. Each was verified directly against source.

1. **`legacyStartOnlyCoordinators` is not a missing settings key.** `src-tauri/src/config/settings.rs:304-309` carries `#[serde(default, skip_serializing_if = "Option::is_none", rename = "startOnlyCoordinators")]` on `pub legacy_start_only_coordinators: Option<bool>`. The JSON key is `startOnlyCoordinators`, and `docs/reference/settings.md:468` already documents it in the "Migration carriers" table. Nothing to add. This also corrects "established fact" 2 in the Step 4 dispatch: `docs/reference/settings.md` is missing exactly **one** key, not two.

2. **`codex_resolver.rs`, `gemini_resolver.rs` and `wg_delete_diagnostic.rs` do not belong in the IPC contract table.** All three contain zero occurrences of `tauri::command` (verified by counting `tauri::command` per file in `src-tauri/src/commands/`). They are internal helper modules with no IPC surface, so their absence from `docs/reference/architecture.md:305-324` is correct, not a gap. Only the agent-update pair is a genuine omission: `get_agent_update_status` (`src-tauri/src/commands/config.rs:2240-2245`) and `agent_update_answer` (`src-tauri/src/commands/config.rs:2255-2262`), both `#[tauri::command]` and both registered in `src-tauri/src/lib.rs:2930-2931`.

3. **`UI-C9` (profile-outdated badge) is already documented.** `docs/features/coding-agent-profiles.md:124-135` is a section titled `## Drift: the "outdated" badge` that describes the badge text (`⟳ outdated`), its tooltip, the click-to-relaunch behavior, and its persistence rules. The inventory's grep for the literal string `profile outdated` missed it. No new documentation is required.

### 3.3 The documentation tree at the frozen SHA

`docs/features/` holds 11 pages and **no** `README.md`: `coding-agent-profiles.md`, `config-seed.md`, `container-coding-agents.md`, `portable-instances.md`, `screenshot-capture.md`, `seed-manifest.md`, `session-auto-close.md`, `telegram-bridge.md`, `terminal-snapshots.md`, `voice-to-text.md`, `window-capture.md`. There is no `mkdocs.yml` in the repo.

`docs/style-guide.md` is the binding authority for prose. Its six numbered rules, its vocabulary table (`:56-66`), its banned-word list (`:73`, plus "simply", "just", "easily", "easy to use" at `:77`), and its markdown conventions (`:88-92`) apply to every file this plan creates or edits.

`docs/style-guide.md:96-101` sets the new-page-versus-extend rule: a new page is justified when the topic has its own audience, is reusable from multiple other docs, or exceeds roughly 200 lines. Section 4.1 applies that rule item by item.

### 3.4 Facts verified for the corrections

- `src/sidebar/components/SettingsModal.tsx:134-142` declares exactly five tabs: `general` -> "General", `agents` -> "Coding Agents", `resources` -> "Resources", `watchers` -> "Watchers", `integrations` -> "Integrations". There is no "API clients" tab.
- `src/shared/shortcuts.ts:44-67` registers via `document.addEventListener("keydown", handler)`. The two entries are Ctrl+Shift+W (`requestCoordinatorCloseById` on the current selection) and Ctrl+Shift+R (`voiceRecorder.toggle`, live selections only). Neither is an OS-global hotkey. The only OS-global hotkey is the screenshot one, documented at `docs/features/screenshot-capture.md:112`.
- `docs/reference/architecture.md:809-854` (table "Frontend (`src/`)") lists `main.tsx` but no file from `src/main/`. `src/main/` contains `App.tsx`, `components/{HomeView,ErrorModal,QuitConfirmModal}.tsx`, `stores/{home,centralView,error-modal}.ts`, `listeners-central-view.ts`, `listeners-home.ts`.
- `grep -ic cascade docs/features/session-auto-close.md` returns `0`.
- `grep -rn "agentAutoUpdateByCommand" docs/` returns nothing. The field is `src-tauri/src/config/settings.rs:565` (`agent_auto_update_by_command: BTreeMap<String, bool>`), default empty (`:917`), camelCase round-trip asserted at `:5098-5124`.
- `src-tauri/src/api/README.md` exists (11.9K, headings: Enabling, Auth, Endpoints (`/api/v1`), Terminal snapshots, Container runtime contract, Versioning, Audit) and is not published under `docs/`. Handler modules: `src-tauri/src/api/handlers/{list_peers,pty_input,send,session_transport,terminal_snapshot,window_screenshot}.rs`.
- `docs/concepts.md:3` states "Nine terms." and the file has exactly nine H2 sections. Any concept added must update that count.

---

## 4. Decisions

These are the six open questions from the dispatch, resolved. Nothing here is left to the implementer.

### 4.1 Granularity

Rule applied: one page per **feature as a user experiences it**, merging the backend half and the UI half. Anything that is a single paragraph inside an existing feature's story becomes a section in that existing page. Anything that is a one-line definition becomes a glossary or concepts entry.

**14 new pages under `docs/features/`:**

| Page | Merges |
|---|---|
| `non-stop-mode.md` | `C1` + `UI-C5` |
| `spec-board.md` | `C2` + `UI-B1` |
| `context-tracking.md` | `C3` + `UI-C8` + `UI-C12` |
| `agent-auto-update.md` | `C4` + `UI-C4` |
| `project-archiving.md` | `C6` + `UI-B11` |
| `project-loops.md` | `B1` + `UI-B6` |
| `resource-monitor.md` | `B2` + `UI-B5` |
| `remote-web-ui.md` | `B3` + `UI-B8` |
| `control-plane-api.md` | `B4` |
| `watchers.md` | `B5` + `UI-B4` |
| `activity-log.md` | `B7` |
| `sidebar-guide.md` | `UI-B2` + `UI-B7` + `UI-C11` + `UI-C13` + `UI-C14` + `UI-C15` + `B6` |
| `app-windows.md` | `UI-C1` + `UI-B3` + `UI-B12` |
| `notifications-and-dialogs.md` | `UI-C2` + `UI-C3` + `UI-C7` + `UI-C10` + `UI-B9` + `UI-B10` + `B10` |

**1 new page under `docs/reference/`:** `keyboard-shortcuts.md` (`UI-C6`).

**1 new index:** `docs/features/README.md`.

**Sections added to existing pages (no new page):**

| Item | Host page | Why not its own page |
|---|---|---|
| `B8` cascade close | `docs/features/session-auto-close.md` | It is a close behavior of the feature that page already owns. |
| `B9` idle badge thresholds | `docs/features/session-auto-close.md` | That page already has `## The idle badge` (`:34`); it only lacks the two threshold keys. |
| `C5` memory rotation at spawn | `docs/agent-matrix-conventions.md` | `docs/reference/directory-layout.md:57` already routes readers there for `memory_YYYYMMDD_hhmmss/`. |

**Concepts and glossary only:** the four terms in section 4.4.

**Explicitly dropped:** `legacyStartOnlyCoordinators`, the three non-IPC command modules, and `UI-C9`. See section 3.2.

### 4.2 Page template

Derived from **`docs/features/session-auto-close.md`**, the closest existing match for "a background behavior with settings, badges and failure modes". Its shape is: H1, an audience-and-promise line, a one-paragraph plain summary, behavior H2s, `## Settings`, `## Troubleshooting`, `## See also`.

Every new page under `docs/features/` MUST have, in this order:

1. `# <Feature name>` - sentence case, matching the index row exactly.
2. **Audience line** - one sentence, "For developers who ...", per `docs/style-guide.md:5`.
3. **Promise line** - one sentence, "After this page you ..." or equivalent concrete outcome, per `docs/style-guide.md:7-12`. May be merged into the same paragraph as the audience line, as `session-auto-close.md:3` does.
4. **Summary paragraph** - what AC actually does, in plain prose, before the first H2.
5. **An opening H2 that answers what the feature does**, named exactly as section 5 lists it for that page. The mechanism in 3 to 8 sentences or a short list.
6. **An enablement H2**, where section 5 lists one: `## Turning it on` (the exact UI path or settings key that enables it, with expected result) or `## Availability` (for an always-on feature, stated plainly). Where section 5 lists neither, the page has neither.
7. One or more behavior H2s, one concept per H2 (`docs/style-guide.md:24-26`). Per-page requirements are in section 5.
8. `## Settings`, **only where section 5 lists it** - a table with columns `Key | What it controls`, one row per related key, and a link to the matching `../reference/settings.md` anchor. Every key in that table must be a real JSON key of `settings.json`. Pages whose configuration does not live in `settings.json` get `## Where the configuration lives` instead, with no `settings.md` link and no key table: D6, D9 (see section 5). Pages with no configuration of their own get neither: D18, D20.
9. `## Troubleshooting` - at least two entries. Each names a literal symptom (a badge text, an error string, an empty list, a missing window) and the concrete check or fix, per `docs/style-guide.md:41-46`. Omitted only where section 5 omits it: D20.
10. `## See also` - at least two relative links to other docs.

**Section 5's per-page H2 list is authoritative.** This subsection describes the intent behind the skeleton and the shape each named H2 must take; where the two appear to disagree, section 5 governs the heading text, the count and the order, and criterion 7.2.4 enforces section 5.

Prose rules that apply everywhere: second person, present tense, active voice; no word from the banned list at `docs/style-guide.md:73,77`; every code block carries a language tag; every shown command is followed by what the reader should see (`docs/style-guide.md:39`).

### 4.3 Index: `docs/features/README.md`

Shape:

```markdown
# Features

For developers looking for the page that covers one AgentsCommander feature. Every feature page lives in this directory; this index is the map.

## Agents and sessions
| Page | What it covers |
|---|---|
| [...](...) | ... |

## Automation
...
## Monitoring
...
## Remote access
...
## Configuration and packaging
...
```

Five H2 groups, fixed, in this order, with these members:

- **Agents and sessions** (8): `coding-agent-profiles.md`, `container-coding-agents.md`, `session-auto-close.md`, `agent-auto-update.md`, `sidebar-guide.md`, `app-windows.md`, `notifications-and-dialogs.md`, `voice-to-text.md`.
- **Automation** (4): `non-stop-mode.md`, `project-loops.md`, `spec-board.md`, `watchers.md`.
- **Monitoring** (6): `resource-monitor.md`, `context-tracking.md`, `activity-log.md`, `terminal-snapshots.md`, `window-capture.md`, `screenshot-capture.md`.
- **Remote access** (3): `remote-web-ui.md`, `control-plane-api.md`, `telegram-bridge.md`.
- **Configuration and packaging** (4): `config-seed.md`, `seed-manifest.md`, `portable-instances.md`, `project-archiving.md`.

Group sizes are 8 + 4 + 6 + 3 + 4 = 25. `voice-to-text.md` sits under "Agents and sessions", not "Remote access": it is a local microphone feature and a reader looking for it will not think of it as remote.

Every row is `| [Title](file.md) | one-sentence description |`. Title matches the page's H1 exactly. Final state: 25 rows (11 existing pages plus 14 new ones). The index itself is never listed as a row.

**Self-creating index.** If `docs/features/README.md` does not exist when a batch runs, the batch creates it with the H1, the audience line and the five H2 group headings above, and then adds only its own rows. A batch never adds a row for a page it did not write. This is what makes Batches 2 to 7 executable in any order from a cold start.

### 4.4 Cross-links

**`docs/concepts.md`** gains four H2 entries, inserted in this order after `## Workgroup` (`:48-52`) and before `## Brief` (`:54`): `## Non-stop mode`, `## Project Loop`, `## Watcher`, `## Spec Board`. Each is 2 to 4 sentences ending in a `See [<page>](features/<page>.md).` link. The lead line at `docs/concepts.md:3` changes "Nine terms." to "Thirteen terms."

**`docs/glossary.md`** gains these H2 entries, each 1 to 3 sentences, inserted so the file stays alphabetically ordered by heading: `## Activity log`, `## Archived project`, `## Context alert`, `## Context badge`, `## Control-plane API`, `## Loop (Project Loop)`, `## Non-stop mode`, `## Raise hand`, `## Resource watchdog`, `## Spec Board`, `## Watcher`. Each entry links to its feature page.

**Where a term lands in both files** - `Non-stop mode`, `Watcher`, `Spec Board`, and `Loop (Project Loop)` against `Project Loop` - the concepts entry is the authoritative definition and the glossary entry is one sentence plus a `See [Concepts](concepts.md#<anchor>).` link. Do not write two independent definitions of the same term. The counts in section 7.3 are unaffected.

**`docs/quickstart.md`** - `## Next steps` (`:83`) gains one bullet: a link to `features/README.md` described as the full feature index.

**`docs/home-en.md`** - `## Useful next steps` (`:22`) gains one bullet linking to `features/README.md`.

**`docs/reference/settings.md`** - each of these `###` groups gains a one-line "See [<page>](../features/<page>.md)." pointer immediately after its table: `Projects` (`:178`) -> `project-archiving.md`; `Resource monitor` (`:216`) -> `resource-monitor.md`; `Git status sweeper` (`:229`) -> `sidebar-guide.md`; `Web server (opt-in)` (`:311`) -> `remote-web-ui.md`; `Control-plane API server (opt-in)` (`:386`) -> `control-plane-api.md`; `Watchers` (`:417`) -> `watchers.md`; `Coding agents` (`:75`) -> `context-tracking.md`; `Logging` (`:456`) -> `activity-log.md`. The `See [Log filtering](log-filtering.md).` line at `:463` stays. That is eight new pointers across seven groups.

The `context-tracking.md` pointer goes on the **`Coding agents`** group, not on `Watchers`. `contextRegex` (`docs/reference/settings.md:94`) is the only `settings.json` key that feeds the context reading and it lives in that group; the `Watchers` group (`:417-437`) holds only `watchers` and `watchersGeometry` plus the `WatcherConfig` shape and carries no context-alert key at all. Pointing `Watchers` at `context-tracking.md` would promise a reader that context-tracking settings live there, and they do not.

**`docs/reference/directory-layout.md`** - the `activity.jsonl` row (`:74`) gains `see [Activity log](../features/activity-log.md)` in its "What it is" cell; the `_agent_<name>/` row (`:57`) gains `see [Agent Matrix conventions §11](../agent-matrix-conventions.md#11-agent-memory-rotation-at-spawn)` after the existing link.

**`docs/security.md`** - `## What an agent can reach` (`:22`) gains one sentence pointing to `features/control-plane-api.md` and `features/remote-web-ui.md` as the surfaces described there. No other change to that file.

### 4.5 The `architecture.md` corrections

Four edits to `docs/reference/architecture.md`. Each is stated as exact old text -> exact new text.

**A1 - `:301` heading plus one missing row.**

Old heading: `## 4. IPC Contract: All Commands`
New heading: `## 4. IPC Contract: Command Modules`

Old lead sentence (`:303`): `Rust handlers live in \`src-tauri/src/commands/\`; the frontend invokes them through \`shared/ipc.ts\`, which routes over a Tauri or WebSocket transport.`
New lead sentence: same text, plus a second sentence: `The table maps each frontend API area to its Rust module and names representative commands; it is not an exhaustive command list.`

Add one row, immediately after the `SettingsAPI` row (`:309`):

`| AgentUpdateAPI | \`commands/config.rs\` | \`get_agent_update_status\`, \`agent_update_answer\` |`

Do **not** add rows for `codex_resolver.rs`, `gemini_resolver.rs`, or `wg_delete_diagnostic.rs`. See section 3.2 item 2.

**A2 - `:819` shortcut scope.**

Old cell: `Global keyboard shortcuts (Ctrl+Shift+W/R)`
New cell: `Document-level keyboard shortcuts: Ctrl+Shift+W closes the selected session, Ctrl+Shift+R toggles voice capture. Active only while an AC window has focus.`

**A3 - Settings tabs. Two edits, both required.**

The false tab list appears twice in this file, and fixing only one ships a page that contradicts itself: a mermaid node claiming an "API clients" tab that does not exist, next to a table listing the five real tabs.

**A3a - the table cell at `:835`.**

Old cell: `Settings tabs: General, Agents, Integrations, Watchers, API clients, ...`
New cell: `Settings tabs: General, Coding Agents, Resources, Watchers, Integrations`

**A3b - the mermaid node at `:221`.**

Old text: `tabs: General, Agents, Integrations,<br/>Watchers, API clients, ...`
New text: `tabs: General, Coding Agents,<br/>Resources, Watchers, Integrations`

Change nothing else on line `:221`. **Do not touch `:54`** (`APICLIENT["Control-plane API clients<br/>(containers, scripts)"]`): that text is correct - it names the API's callers, not a Settings tab.

**A4 - `:809-854` frontend table completeness.**

All three insertion points are **inside** the single existing table that starts at `:811`. The header row `| File | Purpose |` and the separator row `|------|---------|` shown in each block below are for readability only: **insert the data rows and nothing else**. Copying a header or separator row into the middle of a live table renders it as an ordinary data row and visibly breaks the table.

Insert these rows, in this order, immediately after the `main.tsx` row (`:813`):

| File | Purpose |
|------|---------|
| `main/App.tsx` | Main window root: central view routing |
| `main/components/HomeView.tsx` | Home markdown view (fetch, markdown-it, DOMPurify) |
| `main/components/ErrorModal.tsx` | Global error modal |
| `main/components/QuitConfirmModal.tsx` | Quit confirmation |
| `main/stores/centralView.ts`, `main/stores/home.ts`, `main/stores/error-modal.ts` | Main-window state |
| `main/listeners-central-view.ts`, `main/listeners-home.ts` | Main-window event listeners |

Insert these rows immediately after the `shared/stores/resourceMonitor.ts` row (`:825`):

| File | Purpose |
|------|---------|
| `shared/components/ToastHost.tsx` | Toast host rendering |
| `shared/components/ExternalLinkConfirm.tsx` | External-link confirmation dialog |
| `shared/external-links.ts`, `shared/github-url.ts` | External-link resolution |

Insert these rows immediately after the `sidebar/components/SessionItem.tsx` row (`:833`):

| File | Purpose |
|------|---------|
| `sidebar/components/AcDiscoveryPanel.tsx` | Branch and repo discovery panel |
| `sidebar/components/AgentPickerModal.tsx` | Agent picker for launching a session |
| `sidebar/components/AgentUpdateOverlay.tsx` | Startup coding-agent update overlay and prompt |
| `sidebar/components/CodingAgentQuickConfiguration.tsx` | Inline coding-agent configuration |
| `sidebar/components/ContextBadge.tsx` | Per-session context-usage badge |
| `sidebar/components/ContextTemplateUpdateModal.tsx` | Context-template update flow |
| `sidebar/components/ProfileOutdatedBadge.tsx` | Profile-drift badge |
| `sidebar/components/RaiseHandIcon.tsx` | Raise-hand indicator |
| `sidebar/components/TeamContextAlertsEditor.tsx` | Per-team context-alert thresholds |
| `sidebar/components/WorkgroupGroupsModal.tsx` | Workgroup group administration |
| `sidebar/components/ZoomStepper.tsx` | Sidebar zoom control |
| `sidebar/components/Titlebar.tsx` | Sidebar titlebar: zoom, web-server menu, screenshot chip |

No row is removed from the table. Nothing else in `architecture.md` changes **except** the mermaid node at `:221` required by A3b.

---

## 5. Document catalog

For each document: target path, feature covered, source evidence, the required H2 headings verbatim, the source files the writer reads to get the facts, and the done criterion.

The writer reads the named source files. When source and inventory disagree, source wins; record the discrepancy in the batch report rather than guessing.

---

### D1. `docs/features/README.md` (new)

- **Covers:** the missing features index (backend inventory §3 item 2).
- **Evidence:** `20260817-184003-...:84-86`.
- **Shape:** exactly as specified in section 4.3.
- **Sources:** the H1 of every page in `docs/features/`.
- **Done:** file exists; contains the five H2 group headings from section 4.3; at the end of Batch 1 it holds the 11 rows for the existing pages; at the end of Batch 7 it holds 25 rows.

---

### D2. `docs/reference/architecture.md` (edit)

- **Covers:** the four inaccurate or incomplete entries.
- **Evidence:** `20260817-184003-...:77-82`; `20260817-183737-...:60-62`.
- **Required edits:** A1, A2, A3a, A3b, A4 from section 4.5. Apply the stated old-text-to-new-text replacements; for A4, insert the data rows only, per the note in 4.5.
- **Sources:** `src/sidebar/components/SettingsModal.tsx:134-142`; `src/shared/shortcuts.ts:1-70`; `src/main/` tree; `src-tauri/src/commands/config.rs:2240-2262`; `src-tauri/src/lib.rs:2930-2931`.
- **Done:** `grep -n "IPC Contract: Command Modules" docs/reference/architecture.md` matches; `grep -c "Watchers, API clients" docs/reference/architecture.md` returns 0; `grep -c "Control-plane API clients" docs/reference/architecture.md` returns 1 (line `:54` untouched); `grep -n "AgentUpdateAPI" docs/reference/architecture.md` matches; `grep -n "main/components/HomeView.tsx" docs/reference/architecture.md` matches; `grep -c "Global keyboard shortcuts" docs/reference/architecture.md` returns 0; `grep -c "^|------|---------|" docs/reference/architecture.md` returns 3, the baseline verified at the frozen SHA (the A4 blocks must add no separator row); and `grep -c "^| File | Purpose |" docs/reference/architecture.md` returns 3, likewise the frozen-SHA baseline (no duplicated header row).

---

### D3. `docs/reference/settings.md` (edit, Batch 1 part)

- **Covers:** `C4`, the one genuinely missing key.
- **Evidence:** `20260817-184003-...:38,47`.
- **Required edit:** add one row to the `### Coding agents` table (`:75-122`):
  `| \`agentAutoUpdateByCommand\` | object | \`{}\` | Per-coding-agent-command answer to the startup update prompt. Keys are coding-agent commands (for example \`claude\`, \`codex\`); \`true\` means AC updates that command at startup without asking again, \`false\` means it never asks again and never updates. An absent key means AC asks on the next startup. See [Coding agent auto-update](../features/agent-auto-update.md). |`
- **Sources:** `src-tauri/src/config/settings.rs:560-566`, `:917`, `:5098-5124`.
- **Done:** `grep -n "agentAutoUpdateByCommand" docs/reference/settings.md` matches exactly once.
- **Note:** `legacyStartOnlyCoordinators` is explicitly **not** added. See section 3.2 item 1.

---

### D4. `docs/features/session-auto-close.md` (edit, Batch 1 part)

- **Covers:** `B8` (cascade close) and `B9` (idle badge thresholds).
- **Evidence:** `20260817-184003-...:69,70`.
- **Required edits:**
  1. Insert a new H2 `## Coordinator cascade close` immediately before `## Settings` (`:58`). It states what `coordinatorCascadeCloseEnabled` does: whether closing a coordinator also closes its team's member sessions, what happens when it is off, and its default. Minimum 4 sentences.
  2. Extend the existing `## The idle badge` section (`:34-51`) with the two configurable thresholds `coordinatorIdleBadgeYellowMinutes` and `coordinatorIdleBadgeRedMinutes`: what each controls, their defaults, and that they change only the badge color, not the auto-close timeout.
  3. Add both new keys plus `coordinatorCascadeCloseEnabled` to the existing `## Settings` table.
- **Sources:** `docs/reference/settings.md:267-279`; `src-tauri/src/config/settings.rs` (search `coordinator_cascade_close_enabled`, `coordinator_idle_badge_yellow_minutes`, `coordinator_idle_badge_red_minutes`); the cascade-close implementation reachable from those symbols.
- **Done:** `grep -ic cascade docs/features/session-auto-close.md` returns a value greater than 0; `grep -c "coordinatorIdleBadgeYellowMinutes" docs/features/session-auto-close.md` returns a value greater than 0.

---

### D5. `docs/agent-matrix-conventions.md` (edit, Batch 1 part)

- **Covers:** `C5` (automatic `memory/` rotation at spawn).
- **Evidence:** `20260817-184003-...:39`.
- **Required edit:** append a new H2 **at the end of the file, after line 551** (the last line at the frozen SHA), titled exactly `## 11. Agent memory rotation at spawn`. `:525` is the heading of section 10, not its end; do not insert there. Do not renumber any existing section, and do not "restore numbering order" among the nine unnumbered H2s that sit inside template content (`## Core Concepts`, `## Source of Truth`, `## Agent Memory Rule`, `## What You Must NEVER Do`, and others) - they are quoted material, not document structure. Required content:
  - what rotates: the origin Agent Matrix's `memory/` directory becomes `memory_YYYYMMDD_hhmmss/`;
  - when: at agent spawn;
  - the no-op case: an empty `memory/` is not rotated;
  - the repair case: an absent `memory/` is recreated, not rotated;
  - the refusal cases: a junction at `memory/`, and a non-directory or otherwise non-rotatable matrix root;
  - what the agent should do about it: archives are read-only history, never edited or deleted by the agent.
- **Sources:** `src-tauri/src/config/agent_memory.rs` - `rotate_origin_memory_at_spawn`, `rotate_memory_dir`, and the tests `rotate_memory_dir_refuses_a_junction_at_memory`, `rotate_memory_dir_absent_memory_is_repaired_not_rotated`, `resolve_rotatable_matrix_root_rejects_plain_directory`.
- **Done:** `grep -n "^## 11. Agent memory rotation at spawn" docs/agent-matrix-conventions.md` matches; `grep -c "memory_YYYYMMDD_hhmmss" docs/agent-matrix-conventions.md` returns a value greater than 0; `grep -c "^## 5. Profile Path Placeholders" docs/agent-matrix-conventions.md` still returns 1.

---

### D6. `docs/features/non-stop-mode.md` (new)

- **Covers:** `C1` + `UI-C5`.
- **Evidence:** `20260817-184003-...:35`; `20260817-183737-...:25`.
- **Required H2s:** `## What it does`, `## Turning it on`, `## What the watchdog does`, `## Scope: per workgroup`, `## Where the configuration lives`, `## Troubleshooting`, `## See also`.
- **Content requirements:**
  - `## Turning it on` must give both UI paths - the rail button and the project-panel checkbox - naming the visible control.
  - `## Scope: per workgroup` must state that non-stop is set per workgroup, not globally, and how the display name is derived.
  - `## Where the configuration lives` replaces a `## Settings` section. **There is no non-stop key in `settings.json`**: `grep -c nonStop docs/reference/settings.md` returns 0 at the frozen SHA, and the configuration is per project in `src-tauri/src/config/project_settings.rs`. This section says that, names the file, and links to no `settings.md` anchor. Do not invent `nonStopEnabled`, `nonStopToleranceSeconds`, `nonStopWatchdogIntervalSeconds`, or any other key.
  - `## What the watchdog does` is governed by section 12.14, which supersedes the blanket prohibition in section 11.5. It **may** state: AC checks once a second; an episode fires once, after the disparity has lasted the tolerance configured for that workgroup; and a workgroup whose frontend stops reporting for more than three minutes is disarmed rather than fired. It **must not** state a tolerance value, must not state the frontend keepalive interval, and must not describe what firing does downstream - stop at "fires" and link out. Do not cite `fireable` (`non_stop_watchdog.rs:430-433`): it is inside a test module.
- **Sources:** `src-tauri/src/commands/non_stop.rs`; `src-tauri/src/loops/non_stop_watchdog.rs`; `src-tauri/src/config/project_settings.rs` (`default_non_stop_name`, `non_stop_defaults_none_for_legacy_json`, `legacy_non_stop_json_defaults_favorite_false`); `src/sidebar/watchdog/non-stop-watchdog-client.ts`; `src/sidebar/components/WorkgroupGroupRail.tsx` (`nonStopButtonFor`); `src/sidebar/components/ProjectPanel.tsx` (`nonStopChecked`); `src/sidebar/stores/workgroup-groups.ts` (`addWorkgroupToNonStop`, `defaultNonStop`, `nonStopDisplayName`).
- **See also (minimum):** `project-loops.md`, `../concepts.md`.
- **Backend enrichment:** section 11.5. The cadence question is **not answered**; do not state one.

---

### D7. `docs/features/spec-board.md` (new)

- **Covers:** `C2` + `UI-B1`.
- **Evidence:** `20260817-184003-...:36`; `20260817-183737-...:43`.
- **Required H2s:** `## What a Spec Board is`, `## Turning it on`, `## The editing window`, `## Snapshots`, `## Conflicts and unsaved work`, `## Asking an agent about the spec`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## Turning it on` names the `specBoardEnabled` gate and the action-bar entry point. `## The editing window` covers the toolbar and the Mermaid preview. `## Conflicts and unsaved work` covers the conflict banner and the save-before-close modal, including what each says.
- **Sources:** `src-tauri/src/commands/spec_board.rs` (`spec_board_new`, `spec_board_open`, `spec_board_save`, `spec_board_close`, `spec_board_pick_open`, `spec_board_pick_save`, `spec_board_update_content`, `spec_board_list_snapshots`); `src-tauri/src/config/settings.rs:897` area for `specBoardEnabled`; `src/spec-board/components/{SpecBoardEditor,SpecBoardToolbar,MermaidPreview,AskAgentPanel,ConflictBanner,SaveBeforeCloseModal}.tsx`; events `spec_board_changed / conflict / file_missing` (`docs/reference/architecture.md:346`).
- **See also (minimum):** `app-windows.md`, `../reference/settings.md`.

---

### D8. `docs/features/agent-auto-update.md` (new)

- **Covers:** `C4` + `UI-C4`.
- **Evidence:** `20260817-184003-...:38`; `20260817-183737-...:24`.
- **Required H2s:** `## What it does`, `## The startup prompt`, `## How your answer is remembered`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## The startup prompt` must quote the visible overlay and prompt behavior, including that Enter and Esc both answer No. `## How your answer is remembered` must explain the per-command map, that the answer is persisted before the update is attempted, and what an absent key means. Troubleshooting must cover the late-answer case where the prompt already closed.
- **Sources:** `src-tauri/src/agent_update.rs` (`AgentUpdateGate::{register_prompt, resolve_answer, was_prompted, mark_started, drop_pending, snapshot}`); `src-tauri/src/commands/config.rs:2238-2262`; `src-tauri/src/config/settings.rs:560-566`, `:917`; `src/sidebar/components/AgentUpdateOverlay.tsx:20-108`; `src/sidebar/agent-update.ts`; `src/sidebar/update-toast.ts`.
- **See also (minimum):** `../reference/settings.md`, `../integrations/coding-agents.md`.

---

### D9. `docs/features/project-loops.md` (new)

- **Covers:** `B1` + `UI-B6`.
- **Evidence:** `20260817-184003-...:62`; `20260817-183737-...:48`.
- **Required H2s:** `## What a Loop is`, `## Creating a Loop`, `## Scheduling`, `## Delivery: what happens when a Loop fires`, `## Busy sessions and respawn`, `## Where the configuration lives`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## Creating a Loop` covers both the UI modals and a pointer to the CLI section that already exists at `docs/reference/cli.md:838`. `## Busy sessions and respawn` must state the liveness and respawn rules in user terms. Do not duplicate the CLI flag reference; link to it. `## Where the configuration lives` replaces a `## Settings` section: **there is no loop key in `settings.json`** (`grep -c loops docs/reference/settings.md` returns 0 at the frozen SHA). Loops are created and stored through the CLI and the UI, not through `settings.json`. Say that, and link to `../reference/cli.md#loop` rather than to `settings.md`.
- **Sources:** `src-tauri/src/loops/{scheduler,delivery,events}.rs` (`loop_candidate_is_live`, `loop_candidate_should_respawn`, `loop_delivery_config_matches`); `src-tauri/src/commands/loops.rs`; `src/sidebar/components/{NewLoopModal,EditLoopModal}.tsx`; `src/sidebar/components/loop-modal-helpers.ts`; `src/sidebar/loop-event-toast.ts`; `docs/reference/cli.md:838-864`.
- **See also (minimum):** `../reference/cli.md`, `non-stop-mode.md`.

---

### D10. `docs/features/resource-monitor.md` (new)

- **Covers:** `B2` + `UI-B5`.
- **Evidence:** `20260817-184003-...:63`; `20260817-183737-...:47`.
- **Required H2s:** `## What it measures`, `## Turning it on`, `## The Resource Monitor window`, `## Attaching it to the main window`, `## The watchdog: thresholds and actions`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## The watchdog: thresholds and actions` must enumerate both accepted values of `resourceWatchdogAction` and state exactly what each does. **It must not say "which threshold triggers which action"**: per section 11.1 the three thresholds do not map onto the two action values. Write it as: the thresholds decide *whether* the group is over the line; `resourceWatchdogAction` decides *what AC does about it*. The section must also carry the two non-obvious facts from 11.1 - there is no per-process kill (crossing the per-process threshold kills the whole session group), and `Warn` still reclaims quarantined groups. Read the JSON spellings of the two values from `src-tauri/src/config/settings.rs:249-256` and `src/shared/types.ts`; do not copy spellings from this plan. `## The Resource Monitor window` must name the columns the window shows.
- **Sources:** `src-tauri/src/resource_monitor/{registry,types,watchdog,windows}.rs`; `src-tauri/src/commands/resource_monitor.rs`; `src/resource-monitor/App.tsx`; `src/shared/stores/resourceMonitor.ts`; `docs/reference/settings.md:216-228`, `:256`.
- **See also (minimum):** `app-windows.md`, `../reference/settings.md`.
- **Backend enrichment:** section 11.1. It corrects the "which threshold triggers which action" clause above, which is misleading as written.

---

### D11. `docs/features/watchers.md` (new)

- **Covers:** `B5` (the watcher engine) + `UI-B4` (the Watchers window).
- **Evidence:** `20260817-184003-...:66`; `20260817-183737-...:46`.
- **Required H2s:** `## What a watcher is`, `## The two modes: state and occurrence`, `## Deduplication`, `## Commands a watcher can run`, `## The watcher budget`, `## The Watchers window`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## The watcher budget` must state that the budget of 8 is **per agent, not global** (per section 11.2): a user can configure any number of watchers, and each individual agent runs at most eight of the ones that reach it. It must state that nothing is rejected, dropped or evicted at the limit - overflow watchers stay configured and simply do not run on that agent - that the winners are the first eight in ascending watcher-id order (so renaming a watcher can change which ones run), and that disabled watchers never consume budget. Quote the three observable surfaces from 11.2: the single per-agent log line, the Settings notice ` Not running on <agent labels> (budget).`, and the absence of any terminal toast. `## Deduplication` must state that an oversized `dedupeWindowMs` is clamped to 60000, not rejected. `## Troubleshooting` uses the two skip cases from 11.2 (an unreadable `commands` selector entry, and an invalid watcher entry), quoting their literal log lines. `## The Watchers window` must explain the activity window that `watchersGeometry` persists. Do not restate the settings schema line by line; `docs/reference/settings.md:417-437` already does that well - link to it and explain the behavior instead.
- **Sources:** `src-tauri/src/pty/watchers/{dedupe,frame,history,pattern,mod}.rs`; `src-tauri/src/pty/context_scrape/{mod,pattern,rows}.rs`; `src/watchers/App.tsx`; `src/watchers/activity.ts`; `src/watchers/components/WatchersTitlebar.tsx`; `docs/reference/settings.md:417-437`.
- **See also (minimum):** `context-tracking.md`, `../reference/settings.md`.
- **Backend enrichment:** section 11.2. The budget is **per agent**, not global; the plan text above must be read with that correction.

---

### D12. `docs/features/context-tracking.md` (new)

- **Covers:** `C3` + `UI-C8` + `UI-C12`.
- **Evidence:** `20260817-184003-...:37`; `20260817-183737-...:28,32`.
- **Required H2s:** `## What it tracks`, `## The context badge`, `## Context alerts`, `## Setting thresholds for a team`, `## The injected alert message`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:**
  - `## Context alerts` must state the accepted range (1 to 100), that values are normalized and sorted, and that alerts are disabled by default. Per section 12.13 it **must** also state the firing semantics: the injection fires **once per crossing** and does not repeat while the session stays above the threshold; a session that drops below and climbs back alerts again. Do not write "once per session" or "once per threshold, ever". Name the four re-arm paths from 12.13.
  - `## The injected alert message` must name the `context-alert` template id and point at the `injected-messages reseed` documentation at `docs/reference/cli.md:631,637`. It **must not** state a polling interval: the 30-second `CONTEXT_ALERT_MAINTENANCE_INTERVAL` is maintenance bookkeeping, not the sampling cadence, and the retry delays at `context_alerts.rs:26-27` are internal delivery mechanics that do not belong on the page.
  - `## Setting thresholds for a team` must describe the per-team editor in the team modals, and must state that the thresholds are stored in the team configuration, not in `settings.json`.
  - `## Settings` carries exactly **one** row: `contextRegex` (`docs/reference/settings.md:94`, in the `Coding agents` group), the per-agent pattern that produces the reading. There is no `contextAlerts` key in `settings.json` (`grep -c contextAlert docs/reference/settings.md` returns 0 at the frozen SHA); do not add one, and do not link this page to the `Watchers` group.
  - `## Troubleshooting` must include the case from 12.13: a reading above 100 percent is rejected with a `log::error!` and produces no alert, so the log is where the reason is.
- **Sources:** `src-tauri/src/session/context_alerts.rs`; `src-tauri/src/commands/entity_creation.rs:5346-5360` (`normalize_context_alert_percentages`); `src-tauri/src/cli/workgroup.rs` (`cli_team_builder_defaults_context_alerts_to_disabled`); `src-tauri/src/config/injected_messages.rs` (`default_context_alert_template_is_pinned`); `src-tauri/src/cli/team.rs`; `src/sidebar/components/ContextBadge.tsx`; `src/sidebar/components/session-context.ts`; `src/sidebar/components/TeamContextAlertsEditor.tsx`; `src/sidebar/components/team-context-alerts.ts`.
- **See also (minimum):** `watchers.md`, `../reference/cli.md`.
- **Backend enrichment:** section 11.6. Whether the alert fires once per crossing or repeats is **not answered**; do not state either.

---

### D13. `docs/features/remote-web-ui.md` (new)

- **Covers:** `B3` + `UI-B8`.
- **Evidence:** `20260817-184003-...:64`; `20260817-183737-...:50`.
- **Required H2s:** `## What it does`, `## Turning it on`, `## Connecting from another device`, `## What the browser UI can do`, `## Authentication`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## What the browser UI can do` must state plainly that the served page is the full AgentsCommander UI (sidebar plus terminal) running over a WebSocket transport, and name what is not available in the browser build. `## Authentication` must reference the web token and point to `docs/security.md`. Do not duplicate the firewall and LAN guidance at `docs/reference/settings.md:315-374`; link to it.
- **Sources:** `src-tauri/src/web/{auth,broadcast,commands,embedded,event_broadcast,mod}.rs`; `src/browser/App.tsx`; `src/shared/transport-ws.ts`; `src/sidebar/components/WebServerMenu.tsx`; `docs/reference/settings.md:311-385`; `docs/security.md`.
- **See also (minimum):** `../security.md`, `control-plane-api.md`.

---

### D14. `docs/features/control-plane-api.md` (new)

- **Covers:** `B4`.
- **Evidence:** `20260817-184003-...:65`.
- **Required H2s:** `## What it is`, `## Turning it on`, `## Authentication`, `## Endpoints`, `## Authorization and audit`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## Endpoints` must have one subsection or table row per handler: `pty_input`, `session_transport`, `send`, `list_peers`, `terminal_snapshot`, `window_screenshot`, each with its purpose and a link to the deeper page where one exists (`terminal-snapshots.md`, `window-capture.md`). The page is a **rewrite for users**, not a copy of `src-tauri/src/api/README.md`: the README stays where it is and this page is written from it plus the handler modules, in the house voice and template. Do not restate the settings schema at `docs/reference/settings.md:386-397`; link to it.
- **Sources:** `src-tauri/src/api/README.md`; `src-tauri/src/api/handlers/{list_peers,pty_input,send,session_transport,terminal_snapshot,window_screenshot,mod}.rs`; `docs/reference/cli.md:646` (`api-client` mint/revoke/list); `docs/security.md:52-116`.
- **See also (minimum):** `terminal-snapshots.md`, `window-capture.md`, `../security.md`.

---

### D15. `docs/features/activity-log.md` (new)

- **Covers:** `B7`.
- **Evidence:** `20260817-184003-...:68`.
- **Required H2s:** `## What it records`, `## Turning it on`, `## Where the file lives`, `## Record format`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## Record format` must show one real JSONL line in a fenced `json` block and name every field. `## Where the file lives` must point at `docs/reference/directory-layout.md` for the per-instance directory rule rather than restating it.
- **Sources:** `src-tauri/src/config/activity_log.rs`; `docs/reference/directory-layout.md:74`; `docs/reference/settings.md:461`.
- **See also (minimum):** `../reference/directory-layout.md`, `../reference/log-filtering.md`.

---

### D16. `docs/features/project-archiving.md` (new)

- **Covers:** `C6` + `UI-B11`.
- **Evidence:** `20260817-184003-...:40`; `20260817-183737-...:53`.
- **Required H2s:** `## What archiving does`, `## Archiving a project`, `## What blocks an archive`, `## Unarchiving`, `## Auto-unarchive`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## What blocks an archive` must present blockers as **one deduplicated list**, not two features (correction from section 11.3): pending spawns whose cwd is inside the project and live sessions whose working directory is inside the project both feed the same "open session(s)" count, and names are deduplicated across the two. It must state the two exclusions - Root Agent sessions never block, and a running session with no PTY does not block - because otherwise the count will not match what the user sees in the sidebar. `## Auto-unarchive` must explain why the modal appears on its own and what confirming it does. `## Troubleshooting` must quote the literal blocker text from 11.3 verbatim, in both its shapes (three or fewer named blockers, and the "and `<m>` more" shape), and must carry the post-write rollback case as a separate entry, including the appended suffix `The project could not be restored automatically (<error>). Open Archived Projects to restore it.`
- **Sources:** `src-tauri/src/config/archive_gate.rs` (`archived_root_for_cwd`, `probe_spawn_refusal_for_archived_root`, `auto_unarchive_registration_validates_the_archived_root`, `auto_unarchive_registration_coalesces_when_project_is_no_longer_archived`, `raw_project_path_containing_returns_the_matching_archived_root`); `src-tauri/src/commands/ac_discovery.rs` (`archive_blockers_names_live_pty_sessions_under_project`, `archive_project_inner_pending_spawn_mark_blocks_precheck`); `src/sidebar/components/{ArchivedProjectsModal,AutoUnarchiveModal}.tsx`; `src/sidebar/stores/auto-unarchive.ts`; `docs/reference/settings.md:188-212`.
- **See also (minimum):** `../reference/settings.md`, `sidebar-guide.md`.
- **Backend enrichment:** section 11.3. It carries the literal blocker text and corrects the pending-spawn claim above.

---

### D17. `docs/features/sidebar-guide.md` (new)

- **Covers:** `UI-B2`, `UI-B7`, `UI-C11`, `UI-C13`, `UI-C14`, `UI-C15`, and `B6`.
- **Evidence:** `20260817-183737-...:44,49,31,33,34,35`; `20260817-184003-...:67`.
- **Required H2s:** `## The workgroup rail`, `## Favorites and groups`, `## Raise hand`, `## The project panel`, `## What a session row shows`, `## The git branch badge`, `## The agent picker`, `## Quick coding-agent configuration`, `## Branch and repo discovery`, `## Zoom`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements.** Every behavior H2 has one, so no section is left as a heading plus a filename:
  - `## The workgroup rail` - what the rail is, what one rail entry represents, and how collapsing works. Quote the visible label or `title`/`aria-label` of the rail entry from `WorkgroupGroupRail.tsx`.
  - `## Favorites and groups` - how a user marks a workgroup as a favorite and what changes when they do; what a group is and how membership is edited through `WorkgroupGroupsModal.tsx`. Quote the visible control text for both.
  - `## Raise hand` - what a raised hand signals about a session, who raises it and how it is lowered. Quote the icon's tooltip or `aria-label` from `RaiseHandIcon.tsx`. If the source does not settle who raises it, say only what the source settles and report the gap.
  - `## The project panel` - the tree the panel renders (project, workgroup, replica, session) and what the row-level context menu offers. Name the entries by their visible text.
  - `## What a session row shows` - enumerates the row indicators and links out rather than duplicating: status dot -> `../concepts.md#session`; profile drift -> `coding-agent-profiles.md#drift-the-outdated-badge`; context badge -> `context-tracking.md`; idle and AUTO-CLOSED badges -> `session-auto-close.md`; mic -> `voice-to-text.md`; Telegram -> `telegram-bridge.md`.
  - `## The git branch badge` - AC polls each session's git branch and dirty state on an interval; names `gitSweepConcurrency` and `gitSweepMinIntervalSecs` and states what the dirty indicator means.
  - `## The agent picker` - when the picker opens, what it lists, and what selecting an entry does. Quote the modal title from `AgentPickerModal.tsx`.
  - `## Quick coding-agent configuration` - what this inline panel lets a user change without opening Settings, and how it relates to the Settings `Coding Agents` tab. Quote the visible heading or button text from `CodingAgentQuickConfiguration.tsx`, and link to `../integrations/coding-agents.md`.
  - `## Branch and repo discovery` - what the panel shows about a replica's repos and branch, and that it updates from the `ac_discovery_branch_updated` event. Quote the badge text from `replica-repo-badges.ts`.
  - `## Zoom` - the control's range and that the value persists.
  - `## Settings` - `gitSweepConcurrency` and `gitSweepMinIntervalSecs` only, linked to `../reference/settings.md#git-status-sweeper`. Do not add a key for the rail, groups, favorites or zoom without first confirming it exists in `docs/reference/settings.md`.
- **Sources:** `src/sidebar/components/WorkgroupGroupRail.tsx`; `src/sidebar/components/RaiseHandIcon.tsx`; `src/sidebar/stores/rail-collapse.ts`; `src/sidebar/components/ProjectPanel.tsx`; `src/sidebar/components/SessionItem.tsx`; `src/sidebar/components/WorkgroupGroupsModal.tsx`; `src/sidebar/components/AgentPickerModal.tsx`; `src/sidebar/components/CodingAgentQuickConfiguration.tsx`; `src/sidebar/components/AcDiscoveryPanel.tsx`; `src/sidebar/components/replica-repo-badges.ts`; `src/sidebar/components/ZoomStepper.tsx`; `src/shared/zoom.ts`; `src-tauri/src/pty/git_watcher.rs`; `docs/reference/settings.md:229-237`.
- **See also (minimum):** `app-windows.md`, `../concepts.md`.

---

### D18. `docs/features/app-windows.md` (new)

- **Covers:** `UI-C1`, `UI-B3`, `UI-B12`.
- **Evidence:** `20260817-183737-...:21,45,54`.
- **Required H2s:** `## The window map`, `## Sidebar`, `## Main window and Home`, `## Terminal`, `## Guide`, `## Other windows`, `## Troubleshooting`, `## See also`.
- **Content requirements:** `## The window map` is a table with columns `Window | What it is for | How to open it`, one row per window: Sidebar, Main, Terminal (including detached), Guide, Watchers, Resource Monitor, Spec Board, Screenshot overlay. `## Main window and Home` describes the Home markdown view, its refresh control, and its loading, error and retry states. `## Terminal` describes the titlebar, the status bar, the workgroup task title and status from `TASK.md`, the task-clean confirmation, and the last-prompt display, linking to `voice-to-text.md` for the mic. `## Guide` describes the Tutorial and Hints tabs and states that the tutorial covers the same ground as `../quickstart.md`. `## Other windows` is one short paragraph per remaining window, each linking to its own feature page. This page has no settings keys of its own, so it has **no** `## Settings` section.
- **Sources:** `src/main/App.tsx`; `src/main/components/HomeView.tsx`; `src/main/stores/home.ts`; `src/guide/App.tsx`; `src/guide/components/{TutorialTab,HintsTab}.tsx`; `src/terminal/components/{Titlebar,StatusBar,WorkgroupTask,TaskCleanConfirmModal,LastPrompt}.tsx`; `src/terminal/prompt-input-capture.ts`; `docs/reference/architecture.md:809-854`.
- **See also (minimum):** `sidebar-guide.md`, `../reference/keyboard-shortcuts.md`.

---

### D19. `docs/features/notifications-and-dialogs.md` (new)

- **Covers:** `UI-C2`, `UI-C3`, `UI-C7`, `UI-C10`, `UI-B9`, `UI-B10`, and `B10`.
- **Evidence:** `20260817-183737-...:22,23,27,30,51,52`; `20260817-184003-...:71`.
- **Required H2s:** `## Toasts`, `## The error modal`, `## Quitting the app`, `## Opening an external link`, `## Onboarding`, `## Restart prompt`, `## Root Agent banner`, `## Context-template updates`, `## Sounds`, `## Settings`, `## Troubleshooting`, `## See also`.
- **Content requirements.** Every section names the visible title or button text, so a reader who sees the dialog can find its section by matching what is on screen:
  - `## Toasts` - severities, whether a toast is sticky or auto-dismisses, and how to dismiss one.
  - `## The error modal` - what raises it, what it shows, and how it is dismissed. Quote the modal's visible title from `ErrorModal.tsx`.
  - `## Quitting the app` - what the confirmation asks and what each button does. Quote both button labels from `QuitConfirmModal.tsx`.
  - `## Opening an external link` - which links trigger the confirmation, what it says, and what happens on each choice. Quote the dialog text from `ExternalLinkConfirm.tsx`.
  - `## Onboarding` - when the modal appears, what it asks for, and what dismissing it persists. Quote the visible heading from `OnboardingModal.tsx`. Note that `docs/testing/02-onboarding-and-coding-agents.md` records a known `onboardingDismissed` issue (#505); do not restate it as current behavior without confirming it in source.
  - `## Restart prompt` - what change asks for a restart and what the prompt offers. Quote its text from `RestartPromptModal.tsx`.
  - `## Root Agent banner` - what the banner announces, when it is visible, and what its action does. Quote the banner text from `RootAgentBanner.tsx`, and link to the Root Agent entry in `../glossary.md`.
  - `## Context-template updates` - what triggers the modal, what it proposes to update, and what accepting it writes. Quote its title from `ContextTemplateUpdateModal.tsx` and link to `../agent-matrix-conventions.md`.
  - `## Sounds` - covers `soundsEnabled` and `teamIdleBeepEnabled` and states that both default to `true` (pinned by four tests in `src-tauri/src/config/settings.rs`, per section 11.4). The trigger and the gating are **already published** in the repo's own settings reference at `docs/reference/settings.md:252-253`: `soundsEnabled` is the master switch for all app-emitted sounds, and `teamIdleBeepEnabled` beeps when a team transitions from busy to all-idle and is gated by `soundsEnabled`. The page restates exactly that and no more. **Do not state a debounce**, a repeat interval, or any per-session behavior: section 11.4 established that no Rust symbol implements the beep and none of that is settled. Before writing, confirm the busy-to-all-idle transition in `src/` (the frontend computes it from `waitingForInput`); if source contradicts `settings.md:252-253`, write nothing on the trigger and report the contradiction as a defect in the settings reference.
  - `## Settings` - `soundsEnabled` (`docs/reference/settings.md:252`) and `teamIdleBeepEnabled` (`:253`), linked to `../reference/settings.md`. Both were verified present at the frozen SHA.
- **Sources:** `src/shared/components/ToastHost.tsx`; `src/shared/stores/toasts.ts`; `src/main/components/{ErrorModal,QuitConfirmModal}.tsx`; `src/main/stores/error-modal.ts`; `src/main/listeners-central-view.ts`; `src/shared/components/ExternalLinkConfirm.tsx`; `src/shared/external-links.ts`; `src/shared/github-url.ts`; `src/sidebar/components/{OnboardingModal,RestartPromptModal,RootAgentBanner,ContextTemplateUpdateModal}.tsx`; `src-tauri/src/config/settings.rs` (search `sounds_enabled`, `team_idle_beep_enabled`); `docs/agent-matrix-conventions.md:24-70`.
- **See also (minimum):** `app-windows.md`, `../reference/settings.md`.
- **Backend enrichment:** section 11.4. The two sound keys and their defaults are settled; the beep's trigger and debounce are **not** decided in Rust.

---

### D20. `docs/reference/keyboard-shortcuts.md` (new)

- **Covers:** `UI-C6`, plus the correction context behind A2.
- **Evidence:** `20260817-183737-...:26,61`.
- **Required H2s:** `## Window shortcuts`, `## The global screenshot hotkey`, `## Scope`, `## See also`.
- **Content requirements:** `## Window shortcuts` is a table `Shortcut | What it does | Where it works` with exactly two rows: `Ctrl+Shift+W` closes the currently selected session, `Ctrl+Shift+R` toggles voice capture on the selected live session. `## Scope` states that these are document-level listeners, active only while an AC window has focus, and that they are not OS-global. `## The global screenshot hotkey` is one paragraph that states it is the only OS-global hotkey and links to `../features/screenshot-capture.md#configure-the-hotkey`. This page has no settings table.
- **Sources:** `src/shared/shortcuts.ts:1-70`; `docs/features/screenshot-capture.md:53-109`; `docs/integrations/voice.md:41`.
- **See also (minimum):** `../features/screenshot-capture.md`, `../integrations/voice.md`.

---

### D21. Cross-link edits (Batch 8)

Exactly the edits specified in section 4.4, applied to `docs/concepts.md`, `docs/glossary.md`, `docs/quickstart.md`, `docs/home-en.md`, `docs/reference/settings.md`, `docs/reference/directory-layout.md`, and `docs/security.md`.

- **Done:** each grep in section 7.3 matches.

---

## 6. Implementation order: eight batches

Each batch is independently executable from a cold start, in any order. A batch's only prerequisites are this plan file, the two inventory messages, and the repo at the frozen SHA on the working branch.

Batches 2 through 7 each add their own rows to `docs/features/README.md`. That file does not exist at the frozen SHA and is created by D1 in Batch 1, so the self-creating rule in section 4.3 is what makes the independence claim true: a batch that finds no index creates it with the H1, the audience line and the five H2 group headings, and adds only its own rows. Without that rule these batches would depend on Batch 1 having run first. Nothing else in any batch depends on another batch.

**Batch 1 - Corrections and the index.** Documents: D1 (index seeded with the 11 existing pages), D2, D3, D4, D5. Files touched: `docs/features/README.md` (new), `docs/reference/architecture.md`, `docs/reference/settings.md`, `docs/features/session-auto-close.md`, `docs/agent-matrix-conventions.md`.

**Batch 2 - Non-stop, Spec Board, auto-update.** Documents: D6, D7, D8. Plus three rows in D1.

**Batch 3 - Loops and resources.** Documents: D9, D10. Plus two rows in D1.

**Batch 4 - Watchers and context.** Documents: D11, D12. Plus two rows in D1.

**Batch 5 - Remote surfaces and the activity log.** Documents: D13, D14, D15. Plus three rows in D1.

**Batch 6 - Archiving and the sidebar.** Documents: D16, D17. Plus two rows in D1.

**Batch 7 - Windows, dialogs, shortcuts.** Documents: D18, D19, D20. Plus two rows in D1 (D20 is a reference page and is **not** listed in the features index).

**Batch 8 - Cross-links and completeness.** Document: D21. Plus the completeness checks in section 7.4.

---

## 7. Acceptance criteria

All commands are run from the repo root on branch `docs/1407-document-missing-features`.

**What these criteria do and do not guarantee.** Criteria 7.1 to 7.4 are mechanical: they test existence, heading text, heading order, counts, vocabulary, link resolution and identifier provenance. **They cannot tell a true sentence from a plausible invented one.** A page with every required heading present, each holding one confident and fabricated paragraph, passes all of them. That is why section 5 names, per page, the source files and the literal strings the writer must read, why criteria 7.2.7 and 7.2.9 exist, and why 7.5 makes the writer produce a provenance list as an artifact rather than an assumption. Treat 7.5 as the criterion that carries the truth burden; the rest guard shape.

### 7.1 Global, checked at the end of every batch

1. `git status --porcelain=v1 --untracked-files=all` lists only files this plan authorizes for that batch. This is the primary guard and it works whether or not the batch has committed.
2. `git diff --name-only 51e70e47f442109d6b618299b26d95a12801f156 -- src src-tauri scripts .github package.json Cargo.toml src-tauri/tauri.conf.json` returns empty. No application code, build config or CI changed. Compare against the literal frozen SHA, **not** `$(git merge-base HEAD origin/main)`: on this branch the merge base is `HEAD` itself, which makes the diff empty by construction, and the clone is shallow (`git rev-parse --is-shallow-repository` returns `true`), so a moved `origin/main` can make `merge-base` fail outright. Omitting `HEAD` from the command includes the working tree, so the check holds before the batch commits.
3. `git diff --name-only 51e70e47f442109d6b618299b26d95a12801f156 -- docs/testing` returns empty.
4. For every file created or edited in the batch: `grep -inE "revolutionary|unleash|supercharge|next-gen|AI-powered|game-changing|blazing-fast|seamless|magical|agentic" <file>` returns no matches.
5. For every file created or edited in the batch: `grep -inE "\bsimply\b|\beasily\b|easy to use" <file>` returns no matches.
6. Every relative link **the batch added** resolves. For each added `](target)`, strip everything from the first `#` onward, then `test -e` the remaining path. Scope the check to added links only: a broken link that already exists in a file the batch touched is not this batch's defect. Do **not** run `test -e` on the full target - the plan mandates anchored links (`../concepts.md#session`, `coding-agent-profiles.md#drift-the-outdated-badge`, `../features/screenshot-capture.md#configure-the-hotkey`, `../agent-matrix-conventions.md#11-agent-memory-rotation-at-spawn`) and `test -e` fails on every one of them.
7. Every anchor the batch added resolves. For each added `](target#anchor)`, slugify the target file's `^#{1,6} ` headings (lowercase, drop punctuation, spaces to hyphens) and require the anchor to match one. The four anchors named in 7.1.6 were verified to resolve at the frozen SHA; this check keeps them resolving.

### 7.2 Per new page (D6 to D20)

For each page at path `P`:

1. `test -f P` succeeds.
2. `head -1 P` is a single `# ` heading.
3. Line 3 of `P` is a non-empty paragraph (the audience and promise line).
4. `grep -c "^## " P` returns the exact count of required H2s listed for that document in section 5, and `grep "^## " P` returns them in the listed order with the listed text.
5. `grep -c "^## See also" P` returns 1, and the `## See also` block contains at least two `](` links.
6. For every page except D20: `grep -c "^## Troubleshooting" P` returns 1, and that section contains at least two entries. D18 is **not** exempt: its required-H2 list in section 5 includes `## Troubleshooting`.
7. For every page with a `## Settings` H2: that section contains a markdown table and at least one link into `docs/reference/settings.md`. Pages with `## Where the configuration lives` instead (D6, D9) must contain **no** link into `docs/reference/settings.md` from that section.
8. `grep -c "$(basename P)" docs/features/README.md` returns 1 for every page under `docs/features/`. D20 is exempt: it is a reference page.
9. **Settings-key provenance.** For every backtick-quoted identifier matching `^[a-z][A-Za-z0-9]*$` inside a page's `## Settings` table, `grep -c "<identifier>" docs/reference/settings.md` returns at least 1. An identifier that returns 0 is an invented key and fails the batch. This is the check that would have caught `nonStopEnabled`, `nonStopToleranceSeconds` and `nonStopWatchdogIntervalSeconds`, none of which exist.

### 7.3 Per edit

- D2: the five greps in section 5, D2 "Done".
- D3: `grep -c "agentAutoUpdateByCommand" docs/reference/settings.md` returns 1. `grep -c "legacyStartOnlyCoordinators" docs/reference/settings.md` returns 0.
- D4: `grep -ic cascade docs/features/session-auto-close.md` is greater than 0; `grep -c "coordinatorCascadeCloseEnabled" docs/features/session-auto-close.md` is greater than 0; `grep -c "coordinatorIdleBadgeYellowMinutes" docs/features/session-auto-close.md` is greater than 0.
- D5: `grep -c "^## 11. Agent memory rotation at spawn" docs/agent-matrix-conventions.md` returns 1; `grep -c "^## 5. Profile Path Placeholders" docs/agent-matrix-conventions.md` returns 1.
- D21 concepts: `grep -c "^## Non-stop mode" docs/concepts.md` returns 1; same for `^## Project Loop`, `^## Watcher`, `^## Spec Board`. `grep -c "Nine terms" docs/concepts.md` returns 0; `grep -c "Thirteen terms" docs/concepts.md` returns 1.
- D21 glossary: `grep -c "^## " docs/glossary.md` returns 42 (31 existing plus 11 new).
- D21 quickstart and home: `grep -c "features/README.md" docs/quickstart.md` returns 1; `grep -c "features/README.md" docs/home-en.md` returns 1.
- D21 settings pointers: `grep -c "\.\./features/" docs/reference/settings.md` returns 20. The baseline at the frozen SHA is 12 (verified); section 4.4 adds 8 pointers across seven groups. If the baseline has moved, the implementer recomputes it at the start of Batch 8 and asserts baseline plus 8, reporting both numbers. Also: `grep -c "context-tracking.md" docs/reference/settings.md` returns 1, and that occurrence is in the `Coding agents` group, not in `Watchers`.
- D21 directory-layout: `grep -c "features/activity-log.md" docs/reference/directory-layout.md` returns 1; `grep -c "#11-agent-memory-rotation-at-spawn" docs/reference/directory-layout.md` returns 1.
- D21 security: `grep -c "features/control-plane-api.md" docs/security.md` returns 1.

### 7.4 Completeness, checked once in Batch 8

1. `ls docs/features/*.md | wc -l` returns 26 (25 feature pages plus `README.md`).
2. `grep -c "^| \[" docs/features/README.md` returns 25.
3. For every `docs/features/*.md` except `README.md`: its basename appears exactly once in `docs/features/README.md`.
4. Every item in the scope checklist below has at least one covering document. The implementer verifies this list item by item and reports it in full:

   `C1` D6 - `C2` D7 - `C3` D12 - `C4` D8 + D3 - `C5` D5 - `C6` D16
   `B1` D9 - `B2` D10 - `B3` D13 - `B4` D14 - `B5` D11 - `B6` D17 - `B7` D15 - `B8` D4 - `B9` D4 - `B10` D19
   `UI-C1` D18 - `UI-C2` D19 - `UI-C3` D19 - `UI-C4` D8 - `UI-C5` D6 - `UI-C6` D20 - `UI-C7` D19 - `UI-C8` D12 - `UI-C9` already documented, out of scope - `UI-C10` D19 - `UI-C11` D17 - `UI-C12` D12 - `UI-C13` D17 - `UI-C14` D17 - `UI-C15` D17
   `UI-B1` D7 - `UI-B2` D17 - `UI-B3` D18 - `UI-B4` D11 - `UI-B5` D10 - `UI-B6` D9 - `UI-B7` D17 - `UI-B8` D13 - `UI-B9` D19 - `UI-B10` D19 - `UI-B11` D16 - `UI-B12` D18
   architecture corrections: D2 (A1, A2, A3a, A3b, A4)

### 7.5 Literal provenance, reported per batch

The criteria above test shape. This one tests whether the writer read the source, and it is the only criterion that bears on truth.

For every page the batch wrote, the batch report lists **every literal the page quotes** - settings key, UI control label, badge text, tooltip, error string, log line, file path, threshold value - with the `path:line` in `src/`, `src-tauri/`, `docs/reference/settings.md` or an existing `docs/` page where it was read. A literal with no `path:line` is not published: remove it from the page and report the gap instead.

The batch is not done until that list is complete. Two consequences the writer must accept:

- **An omitted sentence is correctable; a published wrong string is not.** Where section 5 or section 11 says a fact is unconfirmed, leave the sentence out and report it. Never supply a plausible value to fill a required section.
- **Section 5's "source wins over inventory" is operationalised here.** If source contradicts an inventory item or a content requirement in this plan, write what source says and record the contradiction in the batch report.

---

## 8. Dependency-cycle gate

Applied per the `verify-no-dependency-cycles` skill.

**Module arcs added by this plan: zero.** This plan changes no file under `src/` or `src-tauri/`. It adds no import, no `use`, no `mod`, no function call, and no type reference in any compiled source. It therefore adds no module-to-module reference, cannot grow or join an SCC, and cannot add an arc that crosses a previously-clean SCC boundary.

**Criterion state:** `cyclicSccs` unchanged (no arcs added or removed), SCC member sets identical, zero cross-boundary arcs, arc record byte-identical. The detector is not required to run because the change set contains no source file; criterion 7.1.2 (`git diff --name-only 51e70e47f442109d6b618299b26d95a12801f156 -- src src-tauri scripts .github package.json Cargo.toml src-tauri/tauri.conf.json` returns empty) is the objective check that this precondition held.

**Role and layering hygiene:** no module gains an `AppHandle` or `tauri` dependency; no transport-taking function is introduced or moved; no pure predicate changes layer. Not applicable, because no module changes.

**Verdict on the gate: PASS.**

---

## 9. Risks and their bounds

1. **A writer restates a settings schema instead of explaining the feature.** Bound: sections 5 D11, D13, D14, D16 explicitly forbid duplicating the schema and require a link instead. The `## Settings` table is capped at `Key | What it controls`, which cannot carry a full schema.

2. **A writer documents a behavior that does not exist.** Bound: every document names the exact source files to read, and section 5 states "source wins over inventory; record the discrepancy". Nothing in section 5 asks the writer to trust the inventory's prose over code.

3. **Page count grows the docs tree faster than the index.** Bound: each of Batches 2 through 7 adds its own index rows, and criterion 7.4.3 fails if any page is missing from the index.

4. **`docs/agent-matrix-conventions.md` anchor breakage.** Bound: section 2 forbids renumbering and criterion 7.3 D5 asserts `## 5. Profile Path Placeholders` still exists.

5. **The glossary count in criterion 7.3 is brittle.** If a future commit lands a glossary entry before Batch 8, the expected 42 changes. Bound: the implementer recomputes the baseline with `grep -c "^## " docs/glossary.md` at the start of Batch 8 and asserts baseline plus 11, reporting both numbers.

6. **Scope pressure to also fix `docs/testing/*` or publish `src-tauri/src/api/README.md` verbatim.** Bound: section 2 forbids both. D14 requires a user-facing rewrite, not a copy, and leaves the in-code README in place.

---

## 10. Open questions for the enrichment pass - all closed

Posed for `dev-rust` and `dev-rust-grinch`. Status after the Step 5 and Step 6 passes. All six are closed for certification purposes; none blocks a batch.

| # | Question | Status |
|---|---|---|
| 1 | D10: is the value set of `resourceWatchdogAction` closed and enumerable, and does each value have a distinct observable effect? | **Answered** by 11.1. Exactly two values. The three thresholds do **not** map onto them; section 5 D10 was corrected. |
| 2 | D11: is the 8-watcher budget a hard constant, and what happens at the limit? | **Answered** by 11.2. Hard constant, **per agent**; nothing is rejected or evicted; the first eight in id order run. Section 5 D11 was corrected. |
| 3 | D12: does `context-alert` fire once per crossing or repeat while above? | **Answered** by 12.13. Once per crossing, latched per threshold, four re-arm paths. Section 11.6's prohibition is lifted; section 5 D12 now carries the semantics. |
| 4 | D6: does the non-stop watchdog have a user-visible cadence or timeout, and what stops it? | **Answered** by 12.14. Backend Tokio loop, 1s tick, stopped only by application shutdown, one shot per episode against a per-episode tolerance, self-disarming after 180s. `dev-rust`'s frontend-polling hypothesis is refuted and must not be carried forward. Section 11.5's prohibition is lifted, bounded by the four limits in 12.14. |
| 5 | D16: what is the literal blocker text? | **Answered** by 11.3, in both shapes, plus the rollback suffix. Section 5 D16 was corrected: the blockers are one deduplicated list, not two features. |
| 6 | D19: is the team idle beep debounced, and does `soundsEnabled` gate it? | **Closed, not by new source evidence.** 11.4 established that no Rust symbol implements the beep. The gating and the trigger are already published in the repo's own settings reference (`docs/reference/settings.md:252-253`), which section 5 D19 now restates and no more. The debounce stays unstated. If the writer's `src/` check contradicts `settings.md:252-253`, that is a defect in the settings reference, to be reported rather than propagated. |

**No question remains that would change a decision in sections 1 to 9.**

---

## 11. Backend enrichment (dev-rust)

Added by `dev-rust` on 2026-08-17 UTC as the Step 5 enrichment pass. This section is **additive**. It changes no batch, no document list, no acceptance criterion, and no decision in sections 1 to 10. It does not certify the plan.

All evidence below was read at the frozen SHA `51e70e47f442109d6b618299b26d95a12801f156` through the Codebase Memory graph (gate `ready`, `head_sha` matched the frozen SHA) plus one text search. Every claim carries a `path:line` anchor. Where a fact could not be established, this section says so instead of supplying a plausible one.

**Reading rule for the writer:** where section 11 contradicts a content requirement in section 5, section 11 wins, because it is anchored to source. Where section 11 says a fact is unconfirmed, the writer must not state that fact at all; leave the sentence out and report it, rather than inventing a value.

---

### 11.1 D10: the resource watchdog (answers open question 1)

**The value set of `resourceWatchdogAction` is closed and has exactly two members.**

`src-tauri/src/config/settings.rs:253-256`:

```rust
pub enum ResourceWatchdogAction {
    Warn,
    KillGroup,
}
```

The default is `Warn`, from `default_resource_watchdog_action` at `src-tauri/src/config/settings.rs:811-813`.

**Not confirmed: the two JSON string spellings.** The `serde` attributes sit above `:253` and were not read. Do **not** write `"warn"` and `"killGroup"` from this plan. Read `src-tauri/src/config/settings.rs:249-256` and the matching `ResourceWatchdogAction` type in `src/shared/types.ts` before writing the values into the page, and use exactly what is there.

**Yes, each value has a distinct observable effect, but the effect is narrower than section 5 implies.**

`src-tauri/src/resource_monitor/watchdog.rs:118-169` (`run_tick`) is the whole decision path:

- `:123-124` the tick returns immediately unless `watchdog_eligible` holds. That is `supports_process_tree_enforcement()` (`:105-107`), so on a platform or backend without process-tree enforcement the watchdog does nothing at all, whatever the action is.
- `:125-127` the tick also returns immediately when `resourceMonitorEnabled` is false. The watchdog is not independently switchable: it rides on the resource monitor being on.
- `:142-149` the kill dispatch is the **only** thing gated on the action. It runs only when `cfg.resource_watchdog_action == ResourceWatchdogAction::KillGroup`.
- `:159-168` quarantine retry cleanup runs on **every** action, `Warn` included. The in-code comment at `:136-140` states this explicitly and warns that turning the action gate into an early return would break it.

So: `Warn` computes and surfaces the threshold state and never terminates anything. `KillGroup` does the same and additionally terminates the offending session's process group. Both keep reclaiming quarantined groups.

**Correction to section 5, D10.** The content requirement says the page must state "which threshold triggers which action". That framing is wrong and would produce a false sentence. The three thresholds do not map onto the two action values. `evaluate_watchdog_groups` (`src-tauri/src/resource_monitor/watchdog.rs:387-431`) computes all three flags on every tick regardless of the configured action:

| Flag | Threshold compared | Scope |
|---|---|---|
| `group_warn` | `limits.group_warn_private_bytes` vs the group's private bytes (`:398-400`) | the whole agent group |
| `group_kill` | `limits.group_kill_private_bytes` vs the group's private bytes (`:401-403`) | the whole agent group |
| `process_kill` | `limits.process_kill_private_bytes` vs each process's private bytes (`:404-414`) | any single process in the group |

`kill_required` is `group_kill || process_kill` (`:415`) and `warn_required` is `group_warn || kill_required` (`:416`). Only groups in state `Running` are evaluated at all (`:394-396`).

Write it as: the thresholds decide *whether* the group is over the line; `resourceWatchdogAction` decides *what AC does about it*.

**Two further facts the page should carry, both non-obvious and both easy to get wrong:**

1. **There is no per-process kill.** Crossing the per-process threshold kills the whole session group. `run_tick` passes only `decision.session_id` to `submit_watchdog_kill` (`src-tauri/src/resource_monitor/watchdog.rs:146`); the offending pids are collected (`:404-414`) but are not what gets terminated.
2. **`Warn` is not "do nothing".** Quarantined groups are still reclaimed under `Warn` (`:159-168`). A user who sets `Warn` to stop AC from touching processes will still see quarantine cleanup happen.

---

### 11.2 D11: the watcher budget (answers open question 2)

**Yes, 8 is a hard constant, and it is per agent, not global.**

`src-tauri/src/pty/watchers/mod.rs:94`:

```rust
pub const WATCHERS_PER_AGENT_BUDGET: usize = 8;
```

It is a compile-time constant with no settings key behind it. The budget is applied once per agent inside the per-agent loop of `resolve_watchers` (`src-tauri/src/pty/watchers/mod.rs:160-304`), at the `running.len() < WATCHERS_PER_AGENT_BUDGET` check (`:270-274`).

**Correction to section 5, D11.** The content requirement says "`## The watcher budget` must state the fixed budget of 8". Left as-is a writer will read that as a global cap on how many watchers exist. It is not. A user can configure any number of watchers; each individual agent runs at most eight of the ones that reach it.

**At the limit, nothing is rejected, dropped or evicted.** The overflow watchers stay configured, stay valid, and simply do not run on that agent. They are collected into `over_budget` (`:273`) and returned in the `AgentResolution` (`:296-301`). The same watcher can be over budget on one agent and running on another.

**What the user actually observes at the limit, in three places:**

1. **One log line per agent**, naming every dropped id in a single line (`:276-287`):
   `[watchers] agent '<agent id>' is over the 8-watcher budget; these are configured but not running on it: <id>,<id>,...`
   The test at `:2734` pins that the dropped ids are one line, not one line each.
2. **A Settings notice**, appended to the watcher's reach description: ` Not running on <agent labels> (budget).`, from `watcherBudgetNotice` at `src/sidebar/components/settings-watchers.ts:286-295`. It is empty when the watcher is disabled or nothing was displaced.
3. **Nothing on the terminal.** There is no toast and no modal on this path.

**Which eight win: watcher id order, ascending.** `resolve_watchers` iterates the `BTreeMap<String, WatcherEntry>` of watchers (`:167`), so candidates are considered in ascending watcher-id order and the first eight that reach the agent are the ones that run. Test `only_the_first_eight_watchers_in_key_order_run_on_one_agent` (`src-tauri/src/pty/watchers/mod.rs:2716-2739`) builds `w01` to `w12` and asserts `w01` to `w08` run and `w09` to `w12` are over budget. State this plainly: renaming a watcher can change which ones run.

**Disabled watchers never consume budget.** `:185-188` skips a watcher whose `enabled` is false before the candidate list is built, silently and by design (the in-code comment at `:185-186` calls it a state the user chose, not a problem).

**Two more facts for `## Deduplication` and `## Troubleshooting`, both from `resolve_watchers`:**

- The dedupe window is clamped, not rejected. `MAX_DEDUPE_WINDOW_MS: u64 = 60_000` at `src-tauri/src/pty/watchers/mod.rs:101`; a larger configured value is clamped with the log line `[watchers] watcher '<id>' asks for a <n> ms dedupe window; clamping to 60000 ms` (`:221-233`).
- A watcher whose `commands` selector contains an entry that is not a command is **skipped entirely**, and never widened to every agent. Log line: `[watchers] watcher '<id>' is being skipped: its commands selector entry '<token>' is not a command. A watcher with an unreadable selector reaches nobody, never everybody` (`:206-215`). An invalid watcher entry is likewise skipped alone, with `[watchers] watcher '<id>' is not a valid watcher and is being skipped; every other watcher and every other setting is unaffected: <detail>` (`:171-181`). These are the best `## Troubleshooting` entries available for this page: they are literal symptoms with a literal cause.

---

### 11.3 D16: what blocks an archive, and the literal text (answers open question 5)

**Yes, the blocker text originates in Rust.** `archive_blocked_message` at `src-tauri/src/commands/ac_discovery.rs:2865-2888`. It has exactly two shapes, and it names at most three blockers (`const MAX_NAMED: usize = 3` at `:2866`):

- Three or fewer blockers:
  `Cannot archive: <n> open session(s) in this project (<name>, <name>, <name>). Close them first.`
- More than three blockers:
  `Cannot archive: <n> open session(s) in this project (<name>, <name>, <name>, and <m> more). Close them first.`

`<n>` is the total blocker count, `<m>` is `<n>` minus 3. Quote these verbatim in `## Troubleshooting`; do not paraphrase the count parentheses.

**Correction to section 5, D16.** The content requirement splits the blockers into "live PTY sessions under the project block it" and "a pending spawn under an archived root is refused", as if those were two different features. They are one list. `archive_blockers` (`src-tauri/src/commands/ac_discovery.rs:2834-2863`) builds a single `Vec<String>` from two passes over the same snapshot:

1. `:2838-2848` pending spawns whose cwd is inside the project, contributing `pending.label`.
2. `:2850-2861` sessions that are live (`session_is_live(&s.status, has_pty)`) and whose working directory is inside the project, contributing `session.name`.

Both feed the same "open session(s)" count in the same message. Names are deduplicated across the two passes (`seen.insert`, `:2845` and `:2858`), which the test `archive_blockers_dedupes_pending_mark_and_live_record_by_name` pins.

**Two exclusions the page should state, because a user will otherwise see a count that does not match what the sidebar shows:**

- **Root Agent sessions never block an archive**, neither by their `is_root_agent` flag nor by their directory name (`:2841-2843` and `:2852-2854`, via `is_root_agent_cwd`). Tests: `archive_blockers_ignores_root_agent_session`, `archive_blockers_ignores_root_agent_by_dir_name_when_flag_is_false`, `archive_blockers_ignores_pending_root_agent_by_dir_name`.
- **A running session with no PTY does not block** (`session_is_live` takes `has_pty`, `:2855`). Test: `archive_blockers_ignores_running_session_with_no_pty`. An exited session under the project does not block either: `archive_blockers_ignores_exited_session_under_project`.

**There is a second check after the write, and it can roll the archive back.** `archive_project_inner_with_settings_path` (`src-tauri/src/commands/ac_discovery.rs:2900-2986`) checks blockers, writes the archive, then re-snapshots and re-checks (`:2924-2926`). If a session became live in between, AC unarchives the project, emits the unarchive event, reseeds the project catalog, and still returns the same `archive_blocked_message` (`:2930-2971`). If that rollback itself fails, the returned string is the blocked message with a suffix appended (`:2978-2983`):

`<the blocked message> The project could not be restored automatically (<error>). Open Archived Projects to restore it.`

That suffix is a distinct `## Troubleshooting` entry: the project is archived, the user did not want it archived, and the fix is the Archived Projects modal.

---

### 11.4 D19: the sound keys (partial answer to open question 6)

**Confirmed in Rust: both keys exist and both default to `true`.** `src-tauri/src/config/settings.rs` carries `sounds_enabled` and `team_idle_beep_enabled`, pinned by four tests in the same file: `sounds_enabled_defaults_true_when_missing_from_json`, `sounds_enabled_round_trips_through_serde`, `team_idle_beep_enabled_defaults_true_when_missing_from_json`, `team_idle_beep_enabled_round_trips_through_serde`. There is a setter command `set_sounds_enabled` in `src-tauri/src/commands/config.rs`.

**Not confirmed, and not confirmable in Rust: the debounce and the master-switch relationship.** No Rust symbol implements the beep. The only backend surface is storing and serving the two booleans. Whether `teamIdleBeepEnabled` is additionally gated on `soundsEnabled`, and whether the beep is debounced across repeated busy-to-idle transitions, is decided in the frontend.

**Consequence for section 5, D19.** The content requirement says `## Sounds` must state "the exact trigger for the team idle beep: the transition from busy to all-idle across a team". That sentence is **unverified from the backend** and must not be written on this plan's authority. Route it to `dev-webpage-ui` before Batch 7, or drop the precision and describe only the two settings keys and their defaults, which are settled.

---

### 11.5 D6: the non-stop watchdog (open question 4 NOT answered)

**Confirmed:** `src-tauri/src/loops/non_stop_watchdog.rs` exists at the frozen SHA, so section 5's source list for D6 is valid. `src-tauri/src/commands/non_stop.rs` exists and exposes `non_stop_report`. The per-project configuration lives in `src-tauri/src/config/project_settings.rs` (`default_non_stop_name`, `populated_non_stop`, plus the tests section 5 already names), which supports the `## Scope: per workgroup` requirement as written.

**Not answered: the cadence, the timeout, and what stops the watchdog.** I could not establish any of the three within the evidence budget. `src-tauri/src/loops/non_stop_watchdog.rs` contributes no symbol matching `non_stop` to the call graph, so its entry point could not be located without a further pass.

One observation that is **not** an answer but bounds the next pass: the only symbol in the tree whose name binds "non-stop" to "watchdog" is the frontend `startNonStopWatchdogClient` at `src/sidebar/watchdog/non-stop-watchdog-client.ts`. If the user-visible cadence is a frontend polling interval rather than a backend loop, this question belongs to `dev-webpage-ui`, not to `dev-rust`. That is a hypothesis, not a finding. Do not write it into a page.

**Binding instruction for the writer:** `## What the watchdog does` must be written without any interval, timeout, or retry number until this is resolved. An omitted sentence is correctable; a published wrong interval is not.

---

### 11.6 D12: context-alert firing semantics (open question 3 NOT answered)

**Confirmed:** `src-tauri/src/session/context_alerts.rs` exists at the frozen SHA, so section 5's source list for D12 is valid. `normalize_context_alert_percentages` is in `src-tauri/src/commands/entity_creation.rs`, and `cli_team_builder_defaults_context_alerts_to_disabled` is in `src-tauri/src/cli/workgroup.rs`, both as section 5 states.

**Not answered: whether the `context-alert` injection fires once per threshold crossing or repeats while above the threshold.** `src-tauri/src/session/context_alerts.rs` yielded no matching symbol in the call graph, so the firing path could not be reached within the evidence budget.

**Binding instruction for the writer:** `## Context alerts` and `## The injected alert message` must describe the thresholds, the accepted range, the normalization and the template id, and must say nothing about repetition. Do not write "once", do not write "repeatedly", and do not write "each time it crosses". Leave the behavior out and flag it in the batch report.

---

### 11.7 Position on the two reversals in section 3.2

**Item 1, `legacyStartOnlyCoordinators`: architect is right, and the reason is stronger than "already documented".**

Confirmed at `src-tauri/src/config/settings.rs`: the `rename = "startOnlyCoordinators"` attribute is at `:307` and the field `pub legacy_start_only_coordinators: Option<bool>` is at `:309`. So the JSON key is `startOnlyCoordinators`, exactly as section 3.2 states.

The stronger reason: this is not a settings key a user can set at all. It is a one-way migration carrier for issue #248. `:2068` takes the value out of the struct, `:2072` translates it to `restoreCoordinatorWakeState` and logs `[settings-migration] #248 - translated legacy startOnlyCoordinators=<v> -> restoreCoordinatorWakeState=<v>`, and when both are present on disk the new value wins and the legacy one is dropped (`:2077`). The round-trip test at `:6223` asserts the serialized output does **not** contain `startOnlyCoordinators`: write it into `settings.json` and it disappears on the next save.

Documenting it as a settable key would have been an error, not a redundancy. `docs/reference/settings.md:468` placing it under "Migration carriers" is the correct treatment. My inventory was wrong on this item and I withdraw it.

**Item 2, the three non-IPC modules: architect is right, confirmed by count.**

`codex_resolver.rs`, `gemini_resolver.rs` and `wg_delete_diagnostic.rs` in `src-tauri/src/commands/` contain zero occurrences of `tauri::command` at the frozen SHA. They have no IPC surface and do not belong in the table at `docs/reference/architecture.md:305-324`. My inventory was wrong on this item and I withdraw it.

**One supporting fact for edit A1.** `src-tauri/src/commands/` holds 23 `.rs` files at the frozen SHA, while the table lists a small subset. That is exactly why A1's new lead sentence ("it is not an exhaustive command list") is the right fix and why adding rows for non-IPC modules would have made the table worse. A1 as written is correct; keep it.

---

### 11.8 Evidence budget and what a further pass would cost

Codebase Memory gate: `ready`, project `D-0_repos-...-repo-AgentsCommander`, `head_sha` `51e70e47f442109d6b618299b26d95a12801f156`, matching the frozen SHA. Twenty graph operations plus both reserve operations were spent, and the single permitted text fallback was spent on exact-text items that the graph does not index (a `serde` rename attribute, two `const` values, and an attribute-macro count). Both reserve operations returned empty, which is why 11.5 and 11.6 are unanswered rather than guessed.

Resolving 11.5 and 11.6 needs one further evidence pass with a fresh budget, scoped to two files: `src-tauri/src/loops/non_stop_watchdog.rs` and `src-tauri/src/session/context_alerts.rs`. Neither blocks Batch 1 through Batch 3. D12 lands in Batch 4 and D6 in Batch 2, so both need an answer before those batches are written, or the affected sentences must be omitted per the binding instructions above.

---

## 12. Grinch Review (dev-rust-grinch)

Added by `dev-rust-grinch` on 2026-08-17 UTC as the Step 6 adversarial pass. This section is **additive**. It changes no batch, no
document list, no acceptance criterion and no decision in sections 1 to 11, and it does **not** certify the plan. The architect is the
only certifier.

Read at the frozen SHA `51e70e47f442109d6b618299b26d95a12801f156` (verified: committed `HEAD` equals it, branch is
`docs/1407-document-missing-features`, `git status --porcelain=v1 --untracked-files=all` empty). Rust evidence came through the
Codebase Memory graph (gate `ready`, `head_sha` matched); markdown under `docs/` is not indexed by the graph and was read directly.

**Precedence.** Where section 12 contradicts sections 1 to 11, section 12 states the contradiction and proposes a fix; it does not
overrule the architect. Where section 12 reports a verified fact with a `path:line`, that fact is authority for the writer, on the same
footing as section 11.

### 12.0 Sections 1 to 11 that I re-verified and found correct

Stated so the architect can see what the review actually covered, not only what it broke.

- The four `architecture.md` anchors A1 to A4 all exist verbatim at the frozen SHA: `## 4. IPC Contract: All Commands` (`:301`), the
  lead sentence (`:303`), the `SettingsAPI` row (`:309`), the `main.tsx` row (`:813`), `Global keyboard shortcuts (Ctrl+Shift+W/R)`
  (`:819`, exactly one occurrence), `Settings tabs: General, Agents, Integrations, Watchers, API clients, ...` (`:835`), the
  `shared/stores/resourceMonitor.ts` row (`:825`) and the `sidebar/components/SessionItem.tsx` row (`:833`). **A1 to A4 are
  applyable as written.**
- Every line anchor in section 4.4 is correct: `settings.md:178/216/229/311/386/417/456/463`, `session-auto-close.md:34/58`,
  `agent-matrix-conventions.md:525`, `quickstart.md:83`, `home-en.md:22`, `security.md:22`, `directory-layout.md:57/74`,
  `cli.md:42/631/637/646/838`, `coding-agent-profiles.md:124/169`, `screenshot-capture.md:112`.
- Every baseline count in section 7 is correct at the frozen SHA: `docs/features/*.md` = 11, `grep -c "^## " docs/glossary.md` = 31
  (so 42 = 31 + 11 holds), `grep -c "^## " docs/concepts.md` = 9 with `docs/concepts.md:3` reading "Nine terms.",
  `grep -c "\.\./features/" docs/reference/settings.md` = 12 (so 20 = 12 + 8 holds).
- The index arithmetic in sections 4.3 and 6 closes: 11 + 3 + 2 + 2 + 3 + 2 + 2 = 25 rows, and the five groups in 4.3 have
  7 + 4 + 6 + 4 + 4 = 25 members.
- The three anchors the plan asks writers to link to all exist: `## Drift: the "outdated" badge` in `coding-agent-profiles.md`,
  `## Session` in `concepts.md`, `## Configure the hotkey` in `screenshot-capture.md`. `docs/integrations/voice.md:41` says exactly
  what D20 relies on. The `docs/concepts.md` H2 order puts `## Workgroup` immediately before `## Brief`, so 4.4's insertion point is real.
- Section 3.2's three reversals and section 11.7's confirmation of them hold.

### 12.1 `grep -c "API clients"` cannot return 0, and A3 leaves the page contradicting itself

- **What:** criterion 7.3/D2 requires `grep -c "API clients" docs/reference/architecture.md` to return 0. The string appears **three**
  times at the frozen SHA, and A3 corrects only one of them.
  - `:54` - `APICLIENT["Control-plane API clients<br/>(containers, scripts)"]`. Correct text. Must not be touched.
  - `:221` - `AB --> SM["SettingsModal.tsx<br/>tabs: General, Agents, Integrations,<br/>Watchers, API clients, ..."]`. This is the
    **same false tab list A3 exists to correct**, inside a mermaid node.
  - `:835` - the table cell A3 fixes.
- **Why:** two concrete failures. First, the criterion is unsatisfiable: the writer either fails Batch 1 or edits `:54` and breaks a
  correct diagram node to make a grep go quiet. Second, and worse if the writer stops at A3: `architecture.md` ships a diagram at
  `:221` claiming a non-existent "API clients" tab and a table at `:835` listing the five real tabs, on the same page. A reader who
  trusts the diagram goes looking for a tab that does not exist. That is exactly the plausible-and-false defect this plan is meant to
  remove, published under the plan's own authority.
- **Fix:** extend A3 to a second edit at `:221`, replacing `tabs: General, Agents, Integrations,<br/>Watchers, API clients, ...` with
  `tabs: General, Coding Agents,<br/>Resources, Watchers, Integrations`; and change the criterion to
  `grep -c "Watchers, API clients" docs/reference/architecture.md` returns 0, which is specific to the defect and leaves `:54` alone.
  Section 4.5's closing sentence "Nothing else in `architecture.md` changes" must be amended to allow this second edit, or it forbids
  the fix its own criterion demands.

### 12.2 Three pages are required to have a `## Settings` section for which no settings key exists

- **What:** criterion 7.2.7 requires every page with a `## Settings` H2 to contain a markdown table **and at least one link into
  `docs/reference/settings.md`**. Section 5 gives `## Settings` to D6, D9 and D12. At the frozen SHA, `docs/reference/settings.md`
  contains, case-sensitively: `nonStop` 0 times, `loops` 0 times, `contextAlert` 0 times, `contextScrape` 0 times.
- **Why:** a writer working cold from this plan reaches `## Settings` on `non-stop-mode.md`, finds the section mandatory and the table
  mandatory, finds nothing in `settings.md` to point at, and writes a plausible key. `nonStopEnabled`, `nonStopToleranceSeconds` and
  `nonStopWatchdogIntervalSeconds` are all names a competent writer would produce and all three are false. Non-stop configuration is
  per project, in `src-tauri/src/config/project_settings.rs`, not in `settings.json` (section 11.5 already says the config lives there;
  it does not draw the consequence). Loop configuration is likewise not a `settings.json` key. This is the single most expensive defect
  in the plan: it does not merely permit an invented key, it makes inventing one the cheapest way to pass the criteria.
- **Aggravating factor for D12.** Section 4.4 adds a pointer from the `### Watchers` group of `settings.md` (`:417`) to **both**
  `watchers.md` and `context-tracking.md`. That group contains only `watchers` and `watchersGeometry` plus the `WatcherConfig` shape.
  It carries no context-alert key at all. The pointer promises a reader that context-tracking settings live there. They do not.
- **Fix:** three changes.
  1. Add D6, D9 and D12 to the section 4.2 item 8 list of pages that legitimately have no `## Settings` H2, and remove `## Settings`
     from their required-H2 lists in section 5 - or replace it with `## Where the configuration lives`, pointing at project settings
     and at the relevant UI, with no `settings.md` link.
  2. Drop `context-tracking.md` from the `### Watchers` pointer in section 4.4, and recompute the 7.3 expectation from 20 to 19.
  3. Add a criterion that would have caught this class by itself, cheap and objective: for every new page, every backtick-quoted
     identifier that matches `^[a-z][A-Za-z0-9]*$` inside its `## Settings` table must return at least one hit in
     `docs/reference/settings.md`. This is runnable and falsifiable, and it is the only proposed check in this review that tests
     whether a page says something true.

### 12.3 Section 7 cannot fail a page that is entirely fabricated

- **What:** section 7.1 states "Every criterion is a command with a stated expected result. There is no subjective quality bar." Both
  halves are wrong, and the second is dangerous.
- **Why:** every criterion in 7.1 to 7.4 tests existence, heading text, heading order, counts, banned vocabulary and link resolution.
  Not one tests whether a sentence is true. A page with all required H2s present, each holding one confident and invented paragraph,
  passes 7.1, 7.2, 7.3 and 7.4 in full. Combine that with 12.2 and the incentive points the wrong way. Published documentation is read
  as authoritative; a wrong settings key or a wrong badge string costs more than a missing page, because a missing page is visibly
  missing and a wrong one is not.
  The claim of full objectivity is also false on its own terms: 7.2.6 "contains at least two entries", 7.2.7 "contains a markdown
  table", and D4's "Minimum 4 sentences" are not commands with stated expected results.
- **Fix:** add to section 7.2 a per-page criterion that the batch report lists, for every literal the page quotes - settings key, UI
  control label, badge text, error string, file path - the `path:line` in `src/`, `src-tauri/` or `docs/reference/settings.md` it was
  read from, and that the batch is not done until that list is complete. Combined with 12.2's identifier check this converts "did the
  writer read the source" from an assumption into an artifact. Also drop or reword the "no subjective quality bar" sentence: a plan
  that overstates its own guarantees is read as covering more than it does.

### 12.4 Batches 2 to 7 are not independently executable from a cold start

- **What:** section 6 states "Each batch is independently executable from a cold start. A batch's only prerequisites are this plan file,
  the two inventory messages, and the repo at the frozen SHA on the working branch." Batches 2 to 7 each append rows to
  `docs/features/README.md`, which does not exist at the frozen SHA and is created by D1 in Batch 1.
- **Why:** run Batch 4 first, on a fresh checkout, with only the plan. There is no `docs/features/README.md` to append to. The writer
  either creates a partial index holding two rows and the five H2 group headings - which then collides with Batch 1 - or skips the
  index rows and fails criterion 7.2.8 (`grep -c "$(basename P)" docs/features/README.md` returns 1) for both pages. Section 6's own
  supporting sentence, "Batches 2 through 7 each append their rows to `docs/features/README.md`, so no batch depends on a later one",
  answers a different question than the claim it is defending: forward independence is not cold-start independence.
- **Fix:** either state plainly that Batch 1 is a prerequisite of Batches 2 to 7 and only Batch 1 and Batch 8 are order-free relative
  to each other, or add one sentence to section 4.3: "If `docs/features/README.md` does not exist, the batch creates it with the H1,
  the audience line and the five H2 group headings from this section, and adds only its own rows." The second option costs one
  sentence and makes the section 6 claim true.

### 12.5 A4's insertion blocks corrupt the table they are inserted into

- **What:** section 4.5, edit A4, gives three insertion blocks. Each is written as a complete markdown table, header row
  `| File | Purpose |` and separator `|------|---------|` included. All three insertion points (`:813`, `:825`, `:833`) are inside the
  single existing table that starts at `:811`.
- **Why:** a writer copying A4 verbatim - which is what "verbatim" in D2's Required edits instructs - drops a second header row and a
  second separator row into the middle of a live table, three times. Markdown renders both as ordinary data rows. The published table
  gains three rows reading `File | Purpose` and three rows of dashes. The edit is applyable, passes every criterion in 7.3/D2
  (`grep -n "main/components/HomeView.tsx"` still matches), and produces a visibly broken table.
- **Fix:** add one sentence to A4: "The header and separator rows shown below are for readability only. Insert the data rows, and
  nothing else, into the existing table."

### 12.6 Criterion 7.1.6 fails on the links the plan itself mandates

- **What:** 7.1.6 checks every relative link by "extracting `](...)` targets and testing each with `test -e`". Section 5 D17 mandates
  `coding-agent-profiles.md#drift-the-outdated-badge` and `../concepts.md#session`; D20 mandates
  `../features/screenshot-capture.md#configure-the-hotkey`; section 4.4 mandates
  `../agent-matrix-conventions.md#11-agent-memory-rotation-at-spawn`; `docs/features/coding-agent-profiles.md:169` already carries
  `../agent-matrix-conventions.md#5-profile-path-placeholders`.
- **Why:** `test -e 'coding-agent-profiles.md#drift-the-outdated-badge'` fails. Run literally, the criterion fails every batch that
  obeys its own content requirements. An implementer who notices will "fix" it by deleting the anchors, which silently destroys D17's
  entire design - D17's `## What a session row shows` is specified as link-outs precisely so it does not duplicate six other pages.
  A second, opposite failure mode: 7.1.6 also runs over pre-existing links in the existing files a batch touches, so a broken link
  already on `settings.md` fails a batch the writer did not cause.
- **Fix:** strip everything from `#` onward before `test -e`, and scope the check to links the batch added. Optionally add an anchor
  check, since the anchors are load-bearing: slugify the target file's `^#{1,6} ` headings (lowercase, drop punctuation, spaces to
  hyphens) and require a match. I verified all four anchors above resolve correctly today, so the design is sound; only the check is wrong.

### 12.7 Criteria 7.1.2 and 7.1.3 do not currently guard anything

- **What:** both criteria are `git diff --name-only $(git merge-base HEAD origin/main) HEAD -- <paths>`. At the frozen SHA on this
  branch, `git merge-base HEAD origin/main` returns `51e70e47f442109d6b618299b26d95a12801f156`, which is `HEAD` itself, so the diff is
  empty by construction. The clone is also shallow (`git rev-parse --is-shallow-repository` returns `true`).
- **Why:** three failures. First, until a batch commits, the criterion returns empty whether or not `src-tauri/` was modified - and
  nothing in section 6 or 7 tells the implementer to commit per batch, so "checked at the end of every batch" can be a no-op every
  time. The real guard is 7.1.1 (`git status --porcelain`), which is already there; 7.1.2 adds the appearance of a second guard and
  none of the substance. Second, on a shallow clone a moved `origin/main` can make `merge-base` fail outright; a failing command in a
  criterion list reads as "returned empty" to a hurried implementer. Third, the pathspec is `src src-tauri scripts package.json`, but
  section 2 also forbids modifying `.github/`, `Cargo.toml` and `tauri.conf.json`, which no criterion covers.
- **Fix:** compare against the literal frozen SHA rather than a computed merge-base - `git diff --name-only
  51e70e47f442109d6b618299b26d95a12801f156 -- <paths>`, which needs no `origin/main` and works on a shallow clone - drop `HEAD` so the
  working tree is included, and extend the pathspec to `src src-tauri scripts .github package.json Cargo.toml
  src-tauri/tauri.conf.json`.

### 12.8 Section 4.2's mandatory skeleton contradicts section 5 for twelve of the fifteen new pages

- **What:** 4.2 says every new page under `docs/features/` MUST have, in order, item 5 `## What it does` and item 6 exactly one of
  `## Turning it on` or `## Availability`. Section 5 gives a differently named opening H2 to D7, D9, D10, D11, D12, D14, D15, D16,
  D17, D18, D19 and D20, and gives **neither** `## Turning it on` nor `## Availability` to D11, D17, D18, D19 and D20.
- **Why:** criterion 7.2.4 enforces section 5's exact count, text and order, so section 5 wins mechanically. But a writer reads
  section 4.2 first - it is the page template, and it says MUST - adds `## What it does` to `spec-board.md`, and fails 7.2.4 with a
  count mismatch and no diagnostic pointing at the cause. The writer's most likely repair is to distrust section 5, which is the
  section anchored to the inventory.
- **Fix:** reword 4.2 item 5 as "an opening H2 that answers what the feature does, named exactly as section 5 lists it for this page",
  and 4.2 item 6 as "where section 5 lists `## Turning it on` or `## Availability`, exactly one of the two is present". State once, in
  4.2, that section 5's H2 list is authoritative and 4.2 describes intent.

### 12.9 The 7.2.6 Troubleshooting exemption names the wrong page

- **What:** 7.2.6 exempts D18 and D20 from the `## Troubleshooting` check. D18's required-H2 list in section 5 **includes**
  `## Troubleshooting`. Only D20 lacks it.
- **Why:** three parts of the plan now disagree about one page: 4.2 item 9 makes Troubleshooting mandatory, section 5 D18 lists it,
  7.2.6 exempts it. 7.2.4 will still catch its absence, so this is not a correctness hole - it is a signal to the writer that D18 may
  skip a section it is required to have, on a page that already has the loosest content requirements in the plan (see 12.10).
- **Fix:** exempt D20 only.

### 12.10 D17 and D19 are the two pages mapped in name only, and they carry fourteen inventory items between them

The tech lead asked for items mapped in name but not in substance. These are they.

- **What:** D17 `sidebar-guide.md` covers seven inventory items (`UI-B2`, `UI-B7`, `UI-C11`, `UI-C13`, `UI-C14`, `UI-C15`, `B6`)
  across ten behavior H2s, and section 5 gives content requirements for exactly three of them: `## What a session row shows`,
  `## The git branch badge`, `## Zoom`. `## The workgroup rail`, `## Favorites and groups`, `## Raise hand`, `## The project panel`,
  `## The agent picker`, `## Quick coding-agent configuration` and `## Branch and repo discovery` get a source file in the Sources
  list and nothing else. D19 `notifications-and-dialogs.md` covers seven items across eleven H2s with content requirements for two
  (`## Toasts`, `## Sounds`) - and section 11.4 has already voided half of the `## Sounds` requirement.
- **Why:** every other document in section 5 states what a section must say, which is what makes "source wins over inventory"
  actionable. For these seventeen H2s the writer has a heading and a filename. Compare D16, where the plan names the literal blocker
  text, or D8, where it names the Enter/Esc behavior. A writer who is confident and cold will fill `## Raise hand` and
  `## Root Agent banner` from the heading plus a skim, and section 7 will pass it. These two pages are where the plan's
  plausible-and-false risk actually concentrates, and they are also the two largest pages, which is the worst combination.
- **Fix:** either add one content-requirement sentence per unspecified H2 - naming the visible control or string the section must
  quote - or split D17 and D19 so no page carries more than three or four inventory items without per-section requirements. This is
  the finding I would not waive. It does not block Batch 1 through Batch 5; D17 lands in Batch 6 and D19 in Batch 7.

### 12.11 Three terms are defined twice with no stated relationship

- **What:** section 4.4 adds `## Non-stop mode`, `## Watcher` and `## Spec Board` to **both** `docs/concepts.md` and
  `docs/glossary.md`. The plan does not say how the two definitions relate.
- **Why:** two independently written definitions of one term drift on the first edit that touches only one of them, and a reader who
  finds both cannot tell which is authoritative. The plan is otherwise careful about this - 4.1's whole granularity rule exists to
  avoid duplication, and D11, D13, D14 and D16 are explicitly forbidden from restating the settings schema.
- **Fix:** one sentence in 4.4: the glossary entry is one sentence plus a link to the concepts entry, and the concepts entry is the
  authoritative definition. Nothing else changes; the counts in 7.3 stay as they are.

### 12.12 Two smaller items

1. **`agent-matrix-conventions.md` append point.** Section 5 D5 says "append a new H2 at the end of the file, after section 10
   (`:525`)". `:525` is the *heading* `## 10. CLI Reference for Agent Management`; the file is 551 lines. "After section 10 (`:525`)"
   reads two ways and one of them puts the new H2 at line 526, in the middle of section 10. The file also contains nine unnumbered
   H2s (`## Core Concepts`, `## Source of Truth`, `## Agent Memory Rule`, `## What You Must NEVER Do`, and others) sitting inside
   template content, so a writer who tries to "restore numbering order" has plenty of room to do damage. **Fix:** say "append at the
   end of the file, after line 551 at the frozen SHA". Criterion 7.3/D5 already protects the `## 5. Profile Path Placeholders` anchor.
2. **Index grouping.** Section 4.3 files `voice-to-text.md` under **Remote access**, alongside the web UI, the control-plane API and
   the Telegram bridge. Voice-to-text is a local microphone feature. A reader browsing the index for it will look under "Agents and
   sessions". Cosmetic, but the index is a published navigation surface and it costs one line to move. **Fix:** move it to "Agents and
   sessions" (making the groups 8 / 4 / 6 / 3 / 4), or leave it and accept the mismatch knowingly.

### 12.13 Open question 3 ANSWERED: `context-alert` fires once per crossing

This closes the gap section 11.6 left open. **The binding instruction in section 11.6 is lifted**: `docs/features/context-tracking.md`
may now state the firing semantics below, and only these.

`src-tauri/src/session/context_alerts.rs:803-839` (`evaluate_numeric_sample`) is the whole decision. Each configured threshold carries
its own latch, `LatchState::Armed` or `LatchState::Latched`:

- when the reading is **below** the threshold, that threshold's latch is set back to `Armed` (`:815-816`);
- when the reading is **at or above** the threshold and the latch is `Armed`, the latch becomes `Latched` and the threshold is queued
  as newly crossed, unless a delivery for that same threshold is still outstanding (`:817-821`);
- a `Latched` threshold is skipped on every later sample, so nothing is queued while the session stays above it.

**So: the injection fires once per crossing. It does not repeat while the session stays above the threshold.** Thresholds crossed in
the same reading are queued together, sorted ascending, and delivered as one batch (`:826-836`).

**What re-arms it, all four paths:**

1. the reading drops back below that threshold (`:815-816`) - the normal case, and the only one a user controls directly;
2. the session ends (`ContextSample::SessionOver`, `src-tauri/src/session/context_alerts.rs:485-487`);
3. the session is reported unavailable and the runtime confirms it is no longer live (`:488-497`), or the member policy resolves to
   `Disabled` or `PermanentIneligible` (`:571-581`) - all of which drop the session's state, latches included;
4. the session's identity fingerprint changes (`:549-553`), which also drops all latches for that session.

**Writing rules for D12, to keep the page from over-promising:**

- Do **not** write "once per session" or "once per threshold, ever". It is once per crossing, and a session that drops below and
  climbs back alerts again. Say that explicitly - it is the behavior a user will observe and be surprised by.
- Do **not** state a polling interval for context readings. The actor's 30-second `CONTEXT_ALERT_MAINTENANCE_INTERVAL`
  (`src-tauri/src/session/context_alerts.rs:24`) is maintenance and retry bookkeeping, not the sampling cadence; the samples arrive on
  a channel from a producer this review did not read. Writing "AC checks every 30 seconds" would be plausible and false.
- Retry delays exist (`FIRST_RETRY_DELAY` 5s at `:26`, `MAX_RETRY_DELAY` 60s at `:27`) and are internal delivery mechanics. They are
  not user-facing and should not appear on the page.
- A reading above 100 percent is rejected with a `log::error!` and no alert (`:502-509`). Worth one `## Troubleshooting` line: if no
  alert arrives, the log is where the reason is.

### 12.14 Open question 4 ANSWERED: the non-stop watchdog is a backend loop, and `dev-rust`'s hypothesis is refuted

This closes the gap section 11.5 left open. **The binding instruction in section 11.5 is lifted, with the limits stated below.**

Section 11.5's hypothesis - that the user-visible cadence might be a frontend polling interval in
`src/sidebar/watchdog/non-stop-watchdog-client.ts`, making this a `dev-webpage-ui` question - is **wrong, and should not be carried
forward**. `src-tauri/src/loops/non_stop_watchdog.rs` does contain the loop. The earlier search missed it because the functions are
named `start`, `tick`, `fire`, `report` and `bot`; none of them contains the string `non_stop`, so a name search for `non_stop` returns
nothing from that file. A text search for `non_stop_watchdog` returns all six.

**The backend loop** - `src-tauri/src/loops/non_stop_watchdog.rs:202-212` (`start`):

```rust
pub fn start(app: AppHandle, state: NonStopWatchdogState, shutdown: ShutdownSignal) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => break,
                _ = interval.tick() => tick(&app, &state).await,
            }
        }
    });
}
```

- **Cadence:** `TICK_INTERVAL` is `Duration::from_secs(1)` (`src-tauri/src/loops/non_stop_watchdog.rs:37`). One tick per second.
- **What stops it:** cancellation of the application `ShutdownSignal`, and nothing else. There is no timeout on the loop, no retry
  count, and no user-facing off switch on the loop itself - turning non-stop off for a workgroup empties what the tick finds, it does
  not stop the tick.

**The timeout the user actually sets is not a constant** - `collect_fireable`, `src-tauri/src/loops/non_stop_watchdog.rs:145-176`:

- an episode fires when `now - disparity_since >= Duration::from_secs(ep.report.tolerance_seconds)` (`:170-174`).
  `tolerance_seconds` arrives on the report, so it is per episode and configurable, **not** a backend constant.
- firing sets `ep.fired = true` (`:171`), and `:165-167` skips any episode already fired. **One shot per episode**, like the context
  alerts, not a repeat.
- **Self-heal:** an armed episode whose last report is older than `REPORT_STALENESS_CEILING` -
  `Duration::from_secs(180)` at `:42` - is disarmed (`disparity_since = None`, `fired = false`, `:148-159`) with the log line
  `[non-stop] '<project path>' disarmed: no frontend report for >180s (frontend gone)`. The in-code comment at `:146-147` states the
  design intent: "Never trips for a live minimized episode (keepalive <= ~60s << 180s)."

**Writing rules for D6, which replace section 11.5's blanket prohibition:**

- `## What the watchdog does` **may** now state: AC checks once a second; an episode fires once, after the disparity has lasted the
  episode's tolerance; and a workgroup whose frontend stops reporting for more than three minutes is disarmed rather than fired.
- It **must not** state a tolerance value. `tolerance_seconds` comes from the frontend report and this review did not establish its
  default or its UI control. Describe it as "the tolerance configured for the workgroup" and stop there.
- It **must not** state the frontend keepalive interval as fact. The `~60s` figure is from an in-code comment, not from the frontend
  source, and comments are not evidence. If D6 needs it, route the question to `dev-webpage-ui` for
  `src/sidebar/watchdog/non-stop-watchdog-client.ts`.
- It **must not** describe what firing does downstream. `fire`, `report` and `bot` in the same file were not read. `## What the
  watchdog does` should stop at "fires" and link out rather than guess at a notification, a Telegram message or a session wake.
- `fireable` at `:430-433` is inside a test module. Do not cite it as production behavior.

### 12.15 Evidence budget

Codebase Memory gate `ready`, project `D-0_repos-AgentsCommander_iac-.ac-wg-14-dev-v5-team-repo-AgentsCommander`, `head_sha`
`51e70e47f442109d6b618299b26d95a12801f156`, matching the frozen SHA. Twelve of twenty graph operations spent. **No reserve graph
operation was spent.** The single permitted fallback was spent on one `rg` for `const ... : Duration` across
`non_stop_watchdog.rs` and `context_alerts.rs`, because the graph indexes callables and not constants. Markdown under `docs/` is not
in the graph and was read directly; every count and anchor in section 12.0 comes from that reading.

Both open questions this pass inherited are now closed. Open questions 1, 2, 5 and 6 in section 10 were answered by section 11, except
the frontend half of question 6, which section 11.4 correctly routes to `dev-webpage-ui` and which this pass did not touch.

**This review does not certify the plan.** Findings 12.1, 12.2, 12.4, 12.5, 12.6 and 12.10 are defects I would expect resolved before
the plan is certified; 12.3, 12.7, 12.8, 12.9, 12.11 and 12.12 are weaknesses the architect may knowingly accept.

---

## 13. Architect resolution and certification (consensus round 1)

Recorded by the architect on 2026-08-17 UTC, as the design authority. Sections 11 and 12 are enrichment and review; this section is the disposition, and sections 1 to 10 above have already been amended to match it.

**All twelve findings are accepted. None is rejected, none is knowingly waived.** Six were argued as blocking and six as optional; the six optional ones were cheap to fix and each removed a way for a cold writer to go wrong, so there was no reason to carry them.

Before resolving, the architect independently re-verified the four load-bearing facts at the frozen SHA: `API clients` occurs at `docs/reference/architecture.md:54`, `:221` and `:835`; `nonStop`, `loops`, `contextAlert` and `contextScrape` each return 0 in `docs/reference/settings.md`; `git merge-base HEAD origin/main` returns the frozen SHA itself and `git rev-parse --is-shallow-repository` returns `true`; `docs/agent-matrix-conventions.md` is 551 lines with `## 10. CLI Reference for Agent Management` at `:525`. All four hold as reported.

### 13.1 Disposition

| Finding | Verdict | Where it landed |
|---|---|---|
| 12.1 `architecture.md` self-contradiction | **Accepted** | 4.5 A3 split into A3a (`:835`) and A3b (`:221`), with `:54` explicitly protected; 4.5's closing sentence amended; D2 criterion changed to `grep -c "Watchers, API clients"` returns 0 plus `grep -c "Control-plane API clients"` returns 1. |
| 12.2 `## Settings` for keys that do not exist | **Accepted, with one variation** | D6 and D9 get `## Where the configuration lives` and no `settings.md` link. D12 **keeps** `## Settings` with exactly one row, `contextRegex` (`docs/reference/settings.md:94`), which is a real key the review did not probe for; its per-team thresholds are routed to `## Setting thresholds for a team`. New criterion 7.2.9 enforces identifier provenance. |
| 12.3 section 7 cannot fail a fabricated page | **Accepted** | The "no subjective quality bar" claim is deleted and replaced by an explicit statement of what the criteria do and do not guarantee. New criterion 7.5 makes literal provenance a reported artifact. |
| 12.4 batches not cold-start independent | **Accepted** | 4.3 gains the self-creating index rule; section 6's independence claim now names it as the reason the claim is true. |
| 12.5 A4 corrupts the table | **Accepted** | A4 gains the insert-data-rows-only note; D2 gains two baseline criteria (`^\|------\|---------\|` returns 3, `^\| File \| Purpose \|` returns 3), both verified at the frozen SHA. |
| 12.6 criterion 7.1.6 fails on mandated anchors | **Accepted** | 7.1.6 rewritten to strip from `#` onward and to scope to links the batch added; new 7.1.7 adds the slugified anchor check, since the anchors are load-bearing. |
| 12.7 7.1.2 and 7.1.3 guard nothing | **Accepted** | Both now diff against the literal frozen SHA with no `HEAD`, and the pathspec is extended to `.github`, `Cargo.toml` and `src-tauri/tauri.conf.json`. Section 8's citation of 7.1.2 updated to match. |
| 12.8 4.2 contradicts section 5 | **Accepted** | 4.2 items 5, 6, 8 and 9 reworded to defer to section 5, with an explicit "section 5 is authoritative" statement. |
| 12.9 Troubleshooting exemption names the wrong page | **Accepted** | 7.2.6 exempts D20 only, and says so. |
| 12.10 D17 and D19 mapped in name only | **Accepted** | Both entries now carry one content requirement per behavior H2, each naming the visible control, label or string the section must quote and the file to read it from. No page was split: the requirements, not the page size, were the defect. |
| 12.11 three terms defined twice | **Accepted** | 4.4 states the concepts entry is authoritative and the glossary entry is one sentence plus a link. Counts unchanged. |
| 12.12 append point and index grouping | **Accepted, both** | D5 now says "after line 551" and warns against renumbering the nine unnumbered template H2s. `voice-to-text.md` moved to "Agents and sessions"; groups are 8 / 4 / 6 / 3 / 4 = 25. |

### 13.2 Corrections carried from section 11 into section 5

Section 11 declared itself additive and left three wrong content requirements standing in section 5, protected only by a precedence rule. Finding 12.8 showed that a cold writer reads the earlier section first, so the wrong requirements were corrected in place rather than left to precedence: D10 (thresholds do not map onto actions), D11 (the budget is per agent), D16 (blockers are one deduplicated list), D19 (`## Sounds`), plus D6 and D12, which now carry the semantics closed by 12.13 and 12.14.

### 13.3 What this certification does not cover

- **Truth of the prose is not certifiable in advance.** Criterion 7.5 shifts it to a reported artifact per batch; it does not eliminate it. The two pages where the residual risk concentrates are D17 and D19, now bounded by per-H2 requirements.
- **The frontend half of open question 6** (whether the busy-to-all-idle trigger published at `docs/reference/settings.md:252-253` matches `src/`) is a verification the writer performs in Batch 7, not a fact this plan asserts.
- **Baselines drift.** Four criteria carry frozen-SHA baselines (glossary 31, settings `../features/` 12, architecture separator rows 3, header rows 3). Each says how to recompute if the base moves.

### 13.4 Verdict

The dependency-cycle gate is unchanged and remains **PASS**: the change set contains no source file, so zero module arcs are added, `cyclicSccs` is unchanged, SCC member sets are identical, and the arc record is byte-identical.

Status: READY_FOR_IMPLEMENTATION

This certification freezes the bytes of this file. Any later edit, however small, invalidates it and requires recertification with a new digest.
