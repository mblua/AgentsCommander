# Plan #1171: Generic regex pattern watchers over PTY output, with activity window

Author: architect, wg-19. Revision 3 certified against `main` on 2026-07-31 UTC, after three rounds of adversarial and implementer review of the amendment (11.5 to 11.9).

Status: READY_FOR_IMPLEMENTATION

Revision 2's certification was invalidated on 2026-07-31, because two of its contracts could not carry the UI this same document specifies (11.5). The digest `3DF57D8E2F4B08BB6F3D81F4A2C827FBBE61189EBC5EEC95581A01BD589B9736` is void and must not be accepted by any verification step.

Issue: [mblua/AgentsCommander#1171](https://github.com/mblua/AgentsCommander/issues/1171), `feat(pty): generic regex pattern watchers over PTY output, with activity window`.

Branch: `feature/1171-pty-pattern-watchers`. Base: `main` at `33a4a7962c1a6bf710dc40ee9b3b4119dc18d944`.

Every `path:line` reference in this document was read against `33a4a79`. This is revision 3, produced after revision 2's implementation was reviewed and two of its IPC contracts were found unable to carry the UI this same plan specifies. Section 11 records both revisions, what changed in each and what was deliberately not taken.

**Revision 3 is confined to three surfaces**: the watcher reach preview (4.9, 4.12), the Settings editor's row lifecycle and validity predicate (4.12), and the activity window's scope handover and content guard (4.12). The engine, the read seam, the frame diff, the modes, the dedupe layers, the caps, the `watcher_matches` event, the history buffer and its purge helper, `get_watcher_activity`, `preview_watcher_pattern` and the settings schema are unchanged from revision 2. Nothing already implemented against those contracts is invalidated by this revision.

`plans/` is ignored by the repository `.gitignore` (`.gitignore:11`). The four existing plans are tracked, so this file must be added with `git add -f plans/1171-pty-pattern-watchers.md`. Do not weaken the ignore rule.

---

## 1. Issue and objective

Build a generic, user-configured engine that runs regular expressions over the plain de-ANSI'd rows of the vt100 screen mirror AgentsCommander already keeps for every session, and surface the matches in a detached activity window.

Today the mirror is scraped for exactly one hardcoded purpose: the per-agent `contextRegex` that produces `contextPercent`, sampled every 5 seconds by `ContextScraper` (`src-tauri/src/pty/context_scrape/mod.rs:26`). There is no mechanism to detect anything else on a coding agent's screen, and the configuration shape (`context_regex` as a scalar field on `AgentConfig`, `src-tauri/src/config/settings.rs:80-81`) cannot express a pattern that applies to every agent.

The objective is a second, independent engine that:

1. Evaluates any number of user-configured patterns, with applicability selected by the agent command's executable stem rather than by agent entry.
2. Distinguishes a `state` reading (idempotent, gated) from an `occurrence` (each match is an event).
3. Samples fast enough to catch rows in transit (200 ms rather than 5 s) and reports its own sampling loss instead of hiding it.
4. Feeds a per-session in-RAM history buffer and a singleton Tauri window that lists activations with filters.

The engine is a best-effort indicator and not an audit log. That limitation is a property of the PTY channel, is already established, and must be stated in the UI.

---

## 2. Evidence and current state

Every fact below was read from the tree at `33a4a79`.

### 2.1 The output pipeline and the shared mirror

- `SessionIoFanout::handle_output` (`src-tauri/src/pty/output.rs:118-179`) is the single point every PTY byte passes through. It feeds the vt100 mirror at `output.rs:157-167`, incrementing `ScreenReplayState::output_sequence` once per **chunk** (`output.rs:160`), including chunks that change no character (cursor show/hide, colour changes, repositioning).
- The mirror is created with **zero scrollback**: `vt100::Parser::new(rows, cols, 0)` (`output.rs:112`). Only the current view exists; rows that scroll off are gone for every backend consumer.
- `SessionIoFanout::get_screen_rows` (`output.rs:295-301`) clones the live grid as plain text rows, releasing the `screen_parsers` guard at the return.
- `SessionIoFanout::resize_screen_and_broadcast` (`output.rs:202-224`) calls `parser.set_size` and refuses a zero dimension (#973). It does **not** bump `output_sequence`.
- `SessionIoFanout::register_session` (`output.rs:109-116`) inserts a fresh parser with `output_sequence: 0`.
- `SessionIoFanout::remove_session` (`output.rs:226-236`) drops the parser.
- **There is more than one screen mirror map.** `screen_parsers` is created inside `SessionIoFanout::new` (`output.rs:105`), and two fanouts exist: one built by `LocalProcessBackend::new` (`local_backend.rs:678`) and one by `ContainerTransportBackend::new` (`container_backend.rs:1473`). Local and container sessions do not share that mutex.
- Relevant vt100 0.15.2 API, all public and all reachable under the same guard: `Screen::rows(start, width)` (`screen.rs:163-173`), `Screen::cursor_position() -> (u16, u16)` (`screen.rs:557`), `Screen::row_wrapped(row) -> bool` (`screen.rs:604-611`, documented as "whether the text in row `row` should wrap to the next line"), `Screen::size()`.

### 2.2 The existing scraper, which this plan does not modify

- `ContextScraper` (`src-tauri/src/pty/context_scrape/mod.rs:163-182`) holds five narrow trait objects and no `AppHandle` and no `PtyManager`. That is its documented capability boundary (`mod.rs:5-8`, `:158-162`).
- Registration chokepoint: `commands/session.rs:2268-2274`, after a successful spawn, only for sessions that have an `agent_id`.
- Retirement: `retire_session` (`mod.rs:248-251`), reached from `context_alerts.rs:1143-1147` and from the tick itself on `SessionOver` (`mod.rs:404-412`).
- Equality gate and coalesced persist: `mod.rs:414-432` and `mod.rs:435-442`. Empty-registry early exit: `mod.rs:322-330`, which takes the `registered` mutex to ask.
- Pattern resolution with a sticky compile failure: `mod.rs:296-319`.
- `pattern::compile` (`context_scrape/pattern.rs:36-54`) sets `size_limit(1 MiB)` and **rejects any pattern without capture group 1** (`pattern.rs:44-48`, test at `:68-75`).
- `rows::extract` scans bottom-up with `.rev()` (`context_scrape/rows.rs:19-26`), pinned by `the_lowest_matching_row_wins` (`rows.rs:125-130`).
- Adapters in `lib.rs`: `ScraperRows` (`lib.rs:531-572`), `ScraperPatterns` (`lib.rs:576-606`), `ScraperSink` (`lib.rs:610-618`), `ScraperSamples` (`lib.rs:622-668`), `ScraperPersist` (`lib.rs:675-699`). Construction and `app.manage` at `lib.rs:1051-1073`.
- **The existing rows read path is four locks deep and includes a child liveness syscall.** `ScraperRows::get_screen_rows` takes the `PtyManager` mutex (`lib.rs:540`); `PtyManager::get_screen_rows` (`manager.rs:575-580`) takes the `registry` mutex inside `kind_for_session` (`manager.rs:362-370`); the local backend then reaches `screen_rows_if_child_alive` (`local_backend.rs:1089-1109`), which probes the child under the `ptys` mutex; and finally `screen_parsers` is taken (`output.rs:296`). The container backend has no liveness gate and reads the mirror directly, mapping parser-absent to `SessionOver` with the comment "Parser-absent IS the container's liveness oracle" (`container_backend.rs:3171-3179`).
- `PtyManager::backend_for_kind` is `pub` and returns a cloned `Arc<dyn PtyBackend>` (`manager.rs:192-197`). The two backends are process singletons built once in `PtyManager::new`.
- The only measured cost number in the repository: "A full sample is ~200 us at AC's default 30x120, so fifty sessions at 5s is ~0.2% of one core, and the liveness gate adds ~0.9 us per configured session per tick" (`context_scrape/mod.rs:22-25`).

### 2.3 Configuration surface

- `AgentConfig` (`settings.rs:47-86`), `context_regex` at `:80-81`.
- `AppSettings` (`settings.rs:271-538`), `agents` at `:277`, `Default` impl at `settings.rs:685-765`.
- Root-level `BTreeMap` precedent with a global master: `auto_self_clear_enabled` (`settings.rs:525-526`) and `auto_self_clear_by_agent` (`settings.rs:530-531`).
- **Settings deserialization is all-or-nothing.** `parse_settings_json` (`settings.rs:870-871`) performs a single `serde_json::from_value::<AppSettings>`; any error is caught at `load_settings_from_path:1661-1664` and replaced with `AppSettings::default()`, leaving one `log::error!` line. The #1077 write gate then refuses to overwrite the file (`read_disk_object_for_write`, `settings.rs:2565-2591`, validating at `:2581`), so nothing is lost on disk, but the application runs with **no agents configured** and every subsequent save from the UI fails silently.
- Stem normalization already exists and is already composed: `command_executable_basename` (`config/coding_agents_catalog.rs:427-432`) chains `normalize_legacy_agent_command` (`config/agent_command.rs:87`) with `command_token_basename` (`settings.rs:768-774`, already `pub(crate)`). `command_executable_basename` itself is private.
- The catalog code explicitly rejects `starts_with` matching, naming `pi` and `agent` as the reason (`coding_agents_catalog.rs:494-497`). The TypeScript `suggestedContextRegex` rule uses `starts_with` and must not be reused.
- Window geometry: `main_geometry` is the live field, `#[serde(default)] Option<WindowGeometry>` **without** `skip_serializing_if` (`settings.rs:367-369`); `sidebar_geometry` and `terminal_geometry` are deprecated since 0.8.0 and do carry `skip_serializing_if` (`settings.rs:359-366`).
- `initWindowGeometry` (`src/shared/window-geometry.ts:26-47`) performs a debounced read-modify-write of the **whole** `AppSettings`. The repository documents the resulting race in writing (`commands/config.rs:653-655`) and defends against it with an explicit list of fields restored from live memory (`config.rs:611-624`, `:647-655`). `set_detached_geometry` (`commands/window.rs:528-538`) is the precedent for a dedicated single-field geometry command that never touches `AppSettings`.

### 2.4 Session lifetime chokepoints

- Creation of the scraper registration: `commands/session.rs:2268-2274`. Its comment at `:2266-2267` states "the first sample is 5s away", which the new engine invalidates.
- **There are three production sites that remove a session from `SessionManager`**, all funnelling into `mutations.remove(...)` and then into `apply_lifecycle_mutations` (`session/manager.rs:1737-1745`):

| site | function | post-commit cleanup loop |
|---|---|---|
| `commands/session.rs:3104` | `execute_destroy_transaction` | `session.rs:3181-3208` |
| `commands/session.rs:4026` | `execute_restart_transaction`, success path | `publish_restart_destroyed`, `session.rs:3758-3782`, called from `:4064` |
| `commands/session.rs:3792` | `finalize_failed_restart` | `publish_restart_destroyed`, called from `:3807` |

  (`session/manager.rs:3531` and `:4300` are test-only.)

- The two cleanup loops are **parallel copies**: both call `publish_destroyed`, both destroy the `terminal-*` window, and both reset `SubstantiveInputState` (`session.rs:3199-3207` and `session.rs:3773-3781`). The per-session side-state purge pattern is therefore already duplicated in the tree.
- Root-agent sessions with `force_destroy_root == false` are **not** removed: they go to `outcome.retained_exited_ids` (`session.rs:3096-3102`) and stay in the manager marked `Exited`. The destroy loop at `:3181-3186` iterates `destroyed_ids.chain(retained_exited_ids)`.
- `restart_session` is a normal flow: it is in the invoke handler (`lib.rs:2255`), the mailbox calls it (`phone/mailbox.rs:9679`), and `commands/config.rs:1134` and `:1180` can restart sessions in bulk through `request.restart_sessions`.
- `SessionManager::destroy_session` (`session/manager.rs:2040-2070`) is reachable only from `#[cfg(test)]` blocks and test-hook branches (`phone/mailbox.rs:7286`, `:7570`, `:10055`, `:14122`, `:17512`, `:17754`, `:17958`; `config/sessions_persistence.rs:3986`). The production branches of those same functions call `background_destroy_session_inner`, which routes to `execute_destroy_transaction`.
- `rollback_pending_create` (`session/manager.rs:1416-1434`) removes a session on spawn rollback. It runs before the registration site (`commands/session.rs:2254-2258` returns before `:2268`), so a rolled-back session never acquires per-session watcher state.

### 2.5 Frontend surface

- `SettingsModal.tsx:96` declares `type SettingsTab = "general" | "agents" | "resources" | "integrations"`, `TABS` at `:98-103`, and `resolveSettingsSection` at `:105-110`, which **falls back silently to `"general"`** for any unrecognised string and has no `"resources"` branch. The Resource Monitor's own Settings button (`src/resource-monitor/App.tsx:422`, `emitOpenSettings("resources")`) therefore lands on General today. That is a pre-existing bug, recorded in section 11.4 and out of scope here.
- `SettingsModal` is mounted in exactly one place, `ActionBar.tsx:388`, inside the sidebar. The chain `emitOpenSettings` (`src/shared/ipc.ts:1014-1019`) to `onOpenSettings` (`ActionBar.tsx:77-80`) to `<SettingsModal section={pendingSection()}>` (`:387-392`) is otherwise intact. `WindowAPI.focusMain()` exists (`src/shared/ipc.ts:474`).
- `formatTimestamp` (`src/resource-monitor/App.tsx:62-71`) is a module-local `const`, **not exported**, and returns `toLocaleTimeString(...)`, not `HH:MM:SS`.
- `.workgroup-task-text` (`src/terminal/styles/terminal.css:227-236`) uses `white-space: pre-wrap` with `word-break: break-word`, that is containment by wrapping, not by scrolling.
- `.status-bar-btn` already exists (`src/terminal/styles/terminal.css:631-660`).
- `src/browser/App.tsx:2` imports and renders `TerminalApp`, so the StatusBar exists in the web client. `src-tauri/src/web/commands.rs` has a window-command no-op list at `:625-639` and a catch-all `_ => Err(format!("Unknown command: {}", cmd))` at `:702`. `isTauri` is exported from `src/shared/platform.ts:3-4`.
- `src/shared/stores/settings.ts` does **not** autoload: `settingsStore.current` is `null` until someone calls `load()`.
- There is **no cross-window event for a generic settings change.** `emit_settings_draft_update_events` (`commands/config.rs:747-769`) emits only `coding_agent_profiles_updated` and `coding_agent_env_settings_updated`; `coding_agent_settings_updated` is emitted exclusively from `phone/mailbox.rs:10812`.
- Polling precedent with written cadence: `resourceMonitorStore.startPolling({ activeIntervalMs: 10_000, idleIntervalMs: 15_000, backoffIntervalMs: 20_000 })` (`ActionBar.tsx:81-87`).
- `emit_to` is already used in the tree (`testability/ui_automation.rs:631`).

### 2.6 The gap

There is no second pattern, no applicability selector, no occurrence semantics, no sub-second sampling, no history buffer, no activity window, and no way to configure a pattern that reaches every agent.

---

## 3. Scope

### 3.1 In scope

1. A new backend module `src-tauri/src/pty/watchers/` containing the engine, the pattern compiler, the frame diff, the dedupe layers and the history buffer type.
2. A new read seam on `SessionIoFanout` and `PtyBackend` that returns "unchanged" without cloning rows, and that carries wrap flags and the cursor row.
3. A root-level `watchers` map in `AppSettings` with the `commands` selector, deserialized entry by entry so one bad entry cannot take the file down.
4. The `watcher_matches` Tauri event, delivered to the activity window, and its TypeScript mirror and listener.
5. The `get_watcher_activity` command, the per-session ring buffer, and the loss signals.
6. `open_watchers_window`, `get_watchers_scope` and `set_watchers_geometry`, and the singleton `watchers` window with persisted geometry and a scope handover that survives being opened again while it is still loading.
7. The activity window itself: table, scope selector, filters by watcher, agent and workgroup, free text over captures, three empty states, snapshot polling, best-effort footer.
8. A StatusBar button that opens or focuses the window and sets its scope, hidden outside Tauri.
9. A Watchers section in the Settings modal that edits the root map, including the `resolveSettingsSection` branch that makes its entry point work.
10. `preview_watcher_pattern` and `preview_watcher_reach`, the two commands the Settings section needs to validate a pattern and to show the reach and the budget of the **whole draft** without porting either the stem rule or the budget rule to TypeScript.

### 3.2 Out of scope

1. **Actions triggered by a match.** The engine holds no `AppHandle` and no `PtyManager` for the same structural reason `ContextScraper` does not (`context_scrape/mod.rs:5-8`). Injection capability plus a loose pattern is a feedback loop: inject, printed to the PTY, matches its own injection, inject again. Any action layer must consume the events from outside the engine, with its own rate limiting, a dry-run mode, and a whitelist that excludes injecting into the session that produced the match.
2. **Migrating `context_regex` into the new map.** It keeps its field, its semantics, its tests and its path to `sessions.json` and `list-peers-lean`.
3. **Unifying `CodingAgentKind` (`session/profile.rs`) with the preset catalog.**
4. **A global cross-session history buffer.** `get_watcher_activity` is per session; the "All sessions" view is composed in the frontend with N calls.
5. **Filtering by workgroup replica ("Role").** `extractAgentName` (`src/shared/path-extractors.ts:28-36`) already exists and it would be cheap, but it was not requested.
6. **`RegexSet`.** Decided out; the arithmetic is in section 7.3.
7. **Persisting a per-watcher summary into `sessions.json`.** Decided out; the reasoning and the flag are in section 3.3.
8. **Seeding default watcher patterns from `agents.default.json`.** No pattern for any watcher use case is captured anywhere in the repository or in any log on the development machine. Shipping an unvalidated default would be shipping a guess as a product default. Every watcher in this issue is user-authored.
9. **Changing `waiting_for_input` to be driven by a state watcher** (`session/manager.rs:506`, `:554`).
10. **Fixing the missing `"resources"` branch in `resolveSettingsSection`.** Pre-existing, unrelated, recorded in 11.4.

### 3.3 One scope call that touches an issue-body bullet, stated explicitly

The issue body says, under "IPC contract": *"What is persisted to `sessions.json` stays a bounded per-watcher summary, never a list of occurrences."*

That sentence is a guardrail on the shape of persistence, not a commitment that persistence ships. **This plan ships no watcher persistence at all**, so the guardrail holds vacuously.

Two reasons. First, nothing in #1171 consumes it: the activity window reads the in-RAM ring buffer plus the live event, and neither path touches `sessions.json`. Adding the summary would mean a new field on `Session`, on `SessionInfo`, in `config/sessions_persistence.rs`, in the `Session` TypeScript type and in the CLI peer row.

Second, and stronger: `get_watcher_activity` is served from a per-session mutex, synchronously (section 4.10). A per-watcher summary on `Session` would put a write against the `SessionManager` async `RwLock` on the engine's hot path, once per match, on top of the coalesced persist `ScraperPersist` already performs (`lib.rs:679-699`). The cut does not merely lose nothing; it keeps the engine out of the session lock.

This is the one place where the plan narrows something an issue-body sentence could be read as requiring.

---

## 4. The decided solution

### 4.1 Shape: a sibling engine, not an extension of `ContextScraper`

`src-tauri/src/pty/watchers/` is a new module with its own thread, its own runtime and its own shutdown token, following the shape `ContextScraper::start` already uses (`context_scrape/mod.rs:207-227`).

`ContextScraper` is not modified. It keeps its 5 second interval, its single pattern per agent, its five sinks and its two shipped issues (#1032, #1088). The two engines share only the read seam on `SessionIoFanout`, which is already a public read boundary.

Reason: the new engine differs in interval (200 ms against 5 s), in modes, in dedupe, in the frame diff, in rate limiting and in owning a history buffer. Folding all of that into `ContextScraper` would put #1032 and #1088 at risk for no gain.

### 4.2 The read seam

Types live in `src-tauri/src/pty/watchers/mod.rs`, mirroring the existing arrangement in which `ScreenRowsRead` lives in its consumer module (`context_scrape/mod.rs:46-54`) and `backend.rs` imports it from there.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStamp {
    /// `ScreenReplayState::output_sequence` (`output.rs:160`).
    pub sequence: u64,
    pub rows: u16,
    pub cols: u16,
}

pub struct ScreenFrame {
    /// One entry per physical row, from `Screen::rows(0, cols)`.
    pub rows: Vec<String>,
    /// `Screen::row_wrapped(i)` for each i: does this physical row continue
    /// into the next one. Same length as `rows`.
    pub wrapped: Vec<bool>,
    /// `Screen::cursor_position().0`. The row currently being written.
    pub cursor_row: u16,
    /// `None` only from the default trait implementation, which has no
    /// sequence to report. `None` means "treat as changed".
    pub stamp: Option<FrameStamp>,
}

pub enum ScreenRowsSince {
    /// The stamp matched. NO rows were cloned and no allocation was made.
    Unchanged,
    Frame(ScreenFrame),
    /// No parser for this id. Says NOTHING about whether the session is over,
    /// exactly like `get_screen_rows` returning `None` (`output.rs:287-294`).
    Missing,
    /// This backend has no session behind this id. Retire it now.
    Gone,
}

impl SessionIoFanout {
    pub fn get_screen_rows_since(&self, id: Uuid, seen: Option<FrameStamp>) -> ScreenRowsSince;
}
```

Four decisions embedded here, each with its reason:

- **The size is part of the stamp** because `resize_screen_and_broadcast` reflows the grid without bumping `output_sequence` (`output.rs:202-212`). Comparing the sequence alone would miss a reflow. `output_sequence` itself is **not** modified, because it is also the replay ordering key `get_screen_snapshot` hands the frontend (`output.rs:274-285`, #955), and changing when it advances would change that contract. The seam only reads it.
- **`stamp` is `Option`** because the defaulted trait method has no sequence available: `get_screen_rows` returns `ScreenRowsRead` (`context_scrape/mod.rs:46-54`), which carries neither a sequence nor a size, and fabricating one would put an invented value into engine state. `None` reads as "changed", so the property "the default never reports `Unchanged`" falls out of the type.
- **`Gone` exists** so the container backend keeps the liveness oracle it documents in writing ("Parser-absent IS the container's liveness oracle", `container_backend.rs:3171-3179`). The local backend, whose parser-absence is not conclusive, returns `Missing`. The engine retires immediately on `Gone` and keeps sampling on `Missing`. The `Unavailable` and `SessionOver` distinction that `ScreenRowsRead` argues for at length (`context_scrape/mod.rs:39-45`) is preserved by exactly this split.
- **`wrapped` and `cursor_row` are read under the same guard** the row clone already takes, at O(1) each. They are what sections 4.3 and 4.4 need, and fetching them separately would mean a second lock acquisition on a possibly different frame.

`FrameStamp.sequence` is monotonic only within one parser instance: `SessionIoFanout::register_session` (`output.rs:109-116`) inserts a fresh parser at `output_sequence: 0`. That is safe here only because session ids are minted per spawn and never reused (`context_scrape/mod.rs:232-233`). This must be stated in the seam's doc comment, because #955's replay tolerates a reset and this engine does not: if a future "reattach in place" ever reuses an id, the stamp would move backwards and the engine could report `Unchanged` over a completely different screen.

**Routing.**

- `PtyBackend::screen_rows_since(&self, id, seen: Option<FrameStamp>) -> ScreenRowsSince` is added as a **defaulted** trait method on `pty/backend.rs` (the trait is at `:127`; `context_session_liveness` at `:148-154` is the precedent). The default delegates to `get_screen_rows`, mapping `Rows` to `Frame` with `stamp: None`, `wrapped` all false, `cursor_row: 0`, and everything else to `Missing`. The two `PtyBackend` test fakes (`pty/manager.rs:926`, `:1025`) keep compiling untouched.
- `LocalProcessBackend` and `ContainerBackend` override it with a direct `self.fanout.get_screen_rows_since(id, seen)`, **with no child liveness probe**. The container override returns `Gone` where its `get_screen_rows` returns `SessionOver`.
- `PtyManager::screen_rows_since(id, seen)` is added for completeness, mirroring `manager.rs:575-580`, and returns `Gone` when `kind_for_session` finds no route, preserving that function's documented reading ("A missing route is not 'we could not read'"). **The engine does not use it in the tick**; see 4.2.1.

#### 4.2.1 The engine holds the backend directly, so the tick takes one lock

At registration the engine resolves the session's `Arc<dyn PtyBackend>` once, through `PtyManager::backend_for_kind` (`manager.rs:192-197`, `pub`, returns a cloned `Arc`, backends are process singletons), and stores it with the session. Every tick then calls `backend.screen_rows_since(id, seen)` directly.

Consequence: the `PtyManager` mutex, "the one every terminal write, resize and kill locks on" (`local_backend.rs:1116-1117`), and the `registry` mutex inside `kind_for_session` are **out of the tick entirely**. What remains is one `screen_parsers` acquisition per session per tick, and that map is per backend (2.1), so local and container sessions do not even contend with each other.

If a read returns `Missing` or `Gone`, the engine re-resolves the `Arc` once through `PtyManager` before acting on it, which covers a session that changed route.

The liveness probe stays on `PtyManager::context_session_liveness` (`manager.rs:506-511`) and runs once every **25th tick**, that is once per 5 seconds per session, exactly today's rate. A session whose child exited is retired within 5 seconds, or immediately if a read returns `Gone`.

This makes the engine strictly better than the status quo on both axes rather than better at rest and worse under load. Section 7.3 has the arithmetic.

### 4.3 Frame diff by best-shift alignment

Per session the engine keeps:

- `prev_hashes: Vec<u64>`, one 64-bit hash per physical row of the previous frame.
- `dirty: Vec<bool>`, one flag per row position.
- `stamp: Option<FrameStamp>`, `evaluated_since_reseed: bool`, `possibly_missed_frames: u64`.

The previous frame's **text is never kept**: hashes suffice for alignment and change detection, at 8 bytes per row instead of up to 120 (240 bytes per session instead of about 3.6 KB). Hashing uses `std::collections::hash_map::DefaultHasher`, which needs no new dependency. A 64-bit collision costs one missed evaluation, which is the fail-closed direction the module already takes (`context_scrape/rows.rs:107-124`).

**Alignment is a best-shift search, not slice equality.** Revision 1 defined `k` as the smallest shift for which `prev_hashes[k..] == curr_hashes[..R-k]` and then branched, inside that same range, on rows that differ. Those two statements cannot both hold: exact equality of the overlap makes the differing branch unreachable, and with any statusline repainting no `k` ever satisfies it, so every tick would have fallen through to a full re-render. The alignment is therefore:

```
for k in 0..R:
    overlap(k) = R - k
    agree(k)   = |{ i in 0..overlap(k) : prev_hashes[i + k] == curr_hashes[i] }|

best_k = argmax agree(k), ties broken by the SMALLEST k
```

Cost: at most `R * (R + 1) / 2` u64 comparisons, 465 for R = 30, per changed session per tick. The smallest-k tie-break declares the fewest new rows, so ambiguity under-counts.

Per tick, given the frame and `curr_hashes`:

1. **First tick after registration** (`prev_hashes` empty): seed, clear `dirty`, set `evaluated_since_reseed = false`, evaluate **nothing**, and do **not** increment `possibly_missed_frames`. There was no sampling gap; reporting the whole starting screen as fresh activity would be wrong.
2. **Size change** (`stamp.rows` or `stamp.cols` differ, or the previous stamp was `None`): reseed as in step 1 and evaluate nothing, because a reflow re-lays every row at a new width. Increment `possibly_missed_frames` **only if `evaluated_since_reseed` is true**. The frontend fits xterm shortly after spawn, so the first resize is near universal; counting it would light the "Some screen output was not sampled" line on nearly every session from birth, which is exactly what separating that line from the `truncated` banner exists to avoid.
3. **Compute `best_k`.** Increment `possibly_missed_frames` when `2 * agree(best_k) < overlap(best_k)`, that is when less than half the overlap agreed. That is the honest meaning of "the sampler could not follow the screen": a `clear`, an alternate-screen switch, a full repaint, or a scroll burst larger than the screen.
4. **New by scroll**, positions `R - best_k .. R` when `best_k >= 1`, **excluding `cursor_row`**: evaluate immediately. These rows arrived complete from below and need no stabilization. Set their `dirty` to false. The cursor row is excluded because a terminal that prints a newline lands the cursor on an empty bottom row and only then writes it: at a 200 ms cadence, against 4096-byte read chunks (`local_backend.rs:946`), landing mid-write is routine, and evaluating it would emit one event with a truncated capture (`Read /path/to/fi`) followed by a second with the complete one. No `dedupe` setting can merge those two, because both the row and the captures differ. The cursor row is instead treated as in-place, so it is evaluated once, complete, one tick later.
5. **In place**, positions `0 .. R - best_k`, plus `cursor_row` whenever it falls in the new-by-scroll range, comparing `curr_hashes[i]` against the shifted previous hash:
   - different: set `dirty[i] = true`, do **not** evaluate.
   - equal and `dirty` carried over: the row has been stable for one full tick. **Evaluate** it and clear `dirty[i]`.
   - equal and not dirty: nothing.
6. **Shift the dirty flags** before step 5 uses them. This is a named operation, `shift_dirty(dirty, k)`, defined as `new[i] = old[i + k]` for `i` in `0 .. R - k` and `false` for `R - k .. R`. It is the step most easily implemented wrong and is tested on its own.
7. Rows that scrolled off the top while still dirty are lost. In practice this is rare (in-place writes happen at the cursor, which is near the bottom, and a scroll moves the cursor row up rather than off) and the engine cannot evaluate them because it never kept their text. It is a documented loss and it does **not** increment `possibly_missed_frames`, so that counter keeps exactly one meaning.
8. Store `curr_hashes`, the new stamp, and set `evaluated_since_reseed` if anything was evaluated.

There is **no separate "re-render, evaluate every row now" path**. A repaint produces a low-agreement alignment whose differing rows all land in `dirty`, and they are evaluated one tick later if the new screen is stable, which it is. That is one evaluation per row, in order, with no burst and with no reliance on the TTL dedupe layer. Removing that path is what makes layer 1 the layer that does the work, as the design always claimed.

**Known imprecision, stated rather than hidden.** When a TUI uses a scroll region, the rows at the bottom of the physical screen are the statusline and do not scroll with the transcript, yet they fall inside `R - best_k .. R` and are evaluated as new on each scrolling tick. A row that does not match costs one regex execution; a row that does match is suppressed by layer 2 while its text is unchanged. Watchers on statusline content belong in `state` mode, which does not use this path at all.

**A deliberate non-guard.** The engine does not skip a scrolled-in row whose hash equals the hash previously at that same position. Such a guard would remove the statusline imprecision above, but it would also drop the second of two consecutive identical rows, and edge case 39 requires identical rows to count twice. The requirement wins.

`state` mode does not use the diff: it evaluates the whole frame, which the engine already holds whenever the stamp changed. The diff state is maintained for every session regardless of configured modes, so switching a watcher's mode at runtime needs no reseed.

#### 4.3.1 Logical rows: wrapped rows are joined before evaluation

`Screen::rows` returns **physical** rows, so a line longer than the terminal width occupies two or more of them and no pattern can match across the break. At 120 columns that is precisely what an absolute file path does, which is the issue's primary use case.

The diff stays on physical rows, because physical arrival is what the alignment measures. Evaluation is on logical rows:

- A physical row `i` is a **continuation** when `wrapped[i - 1]` is true. A continuation is never evaluated on its own.
- When a non-continuation row `i` is selected for evaluation, the text handed to the patterns is `rows[i]` concatenated with `rows[i + 1]`, `rows[i + 2]` and so on while `wrapped` of the preceding row is true. There is no separator: `write_contents` emits no trailing padding (`vt100 row.rs:122-133`), so the concatenation reconstitutes the original line.
- A continuation whose start is above the top of the screen has lost its beginning and is **skipped**, never evaluated as a fragment. Fail-closed.
- The `row` field of the payload carries the logical row, truncated to 256 bytes on a char boundary with `rowTruncated` set, exactly as before.
- If a continuation row is selected for evaluation by step 4 or step 5 while its start row is not, the engine evaluates the **logical row starting at that start row**, once. Selection is therefore mapped from physical to logical before evaluation, and a logical row is evaluated at most once per tick.

`Row::resize` clears the wrap flag (`vt100 row.rs:73-76`), which is consistent: a resize reseeds anyway.

### 4.4 Modes

**`state`**: over the full frame, take the **lowest** matching logical row, mirroring `rows::extract` (`context_scrape/rows.rs:19-26`), because a statusline always sits below the transcript.

The gate is not `(captures, row)` alone. Revision 1 kept only that pair and cleared it when nothing matched, which means a second instance of a condition appearing while the first is still visible never emits: the lowest match still reads the same text, so the tuple never changes. That is the failure mode of the permission-prompt watcher, which is the strongest argument for the engine existing.

The gate is `(captures, row, generation)`, where `generation` is a per `(session, watcher)` counter incremented whenever the number of matching logical rows **rises** from the previous tick. Walking the cases:

| transition | count | generation | emits |
|---|---|---|---|
| condition appears | 0 to 1 | +1 | yes |
| screen scrolls, condition still visible | 1 to 1 | same | no |
| second instance appears while the first is visible | 1 to 2 | +1 | **yes** |
| first instance scrolls off | 2 to 1 | same | no |
| everything scrolls off | 1 to 0 | same, gate cleared | no |
| condition reappears later | 0 to 1 | +1 | yes |
| instance A scrolls off and B appears in the same tick, different text | 1 to 1 | same | yes, the text changed |

Incrementing only on a rise is what makes the gate scroll-stable: the count of a persistent condition does not change as the screen moves.

A transition to "no match" **clears the gate and emits nothing.** The only consumer in this issue is an activity log, and "the prompt disappeared" is not a log entry. Clearing is what lets an identical re-appearance emit again. This is deliberate and is why the payload needs no `present` or `cleared` field. The window compensates by marking state rows visually and by wording them as "first seen at", not "currently true" (4.12).

**`occurrence`**: every logical row the diff declares evaluable is matched; every match that survives the dedupe layer and the rate limit is an event. There is no equality gate, by definition.

### 4.5 Deduplication, three layers in this order

**Layer 1, best-shift alignment.** Structural, and it does the work: a logical row is evaluated on the one tick it becomes final, either by arriving from below or by holding still for a tick. The same text appearing twice in a transcript is two rows at two times and counts twice; a row shifted up by a scroll is not re-evaluated.

**Layer 2, key suppression with a TTL.** For the residual cases layer 1 cannot separate: differently truncated repeats of the same content, and the scroll-region statusline of 4.3. Per watcher:

- `dedupe: "row"` (default): the key is the matched logical row text.
- `dedupe: "capture"`: the key is the joined capture groups. Two rows truncated differently that capture the same path are one event.
- `dedupe: "none"`: every match counts.
- `dedupeWindowMs`, default `2000`, **clamped to `60_000`** on read, with one log line when a larger value is clamped.

Applies to `occurrence` only; `state` has its gate.

**The dedupe map is bounded.** Per `(watcher, session)` at most **256** keys, oldest-inserted evicted first. Expired keys are pruned once per tick for the sessions evaluated that tick. Without both bounds a large `dedupeWindowMs` and a `row` key would grow the key set to every distinct row seen in the window, which at 8 watchers and 20 sessions is hundreds of megabytes.

**Layer 3, `possiblyMissedFrames`.** Not deduplication. It declares uncertainty instead of hiding it: above zero means "something may have been missed", never "N things were missed". Per session, because sampling is per session.

### 4.6 Rate limiting, applied before both the event and the buffer

Two caps, both per tick:

- **8 matches per `(watcher, session)`.**
- **16 matches per session** across all watchers.

On exceeding either cap the engine counts the overflow, emits nothing further for that key this tick, logs once per `(watcher, session)` while degraded, and sets `degraded` on that watcher's counter in the snapshot. The recover-and-log-once shape follows `ScraperSamples` (`lib.rs:629-667`).

**The caps bound the rate but not the duration**, so a watcher that stays saturated would emit 40 events per second forever and turn the 500-entry ring over in 0.31 seconds, making `truncated` true and useless. Therefore: after **25 consecutive degraded ticks** (5 seconds) a `(watcher, session)` pair is **suspended for 5 seconds**, keeping `degraded` true and logging one line, then retried. This adds no new state, since the degradation counter already exists for the marker, and it does not touch the event contract.

The caps live **inside the engine loop**, immediately before the sink call, exactly where `ContextScraper`'s equality gate lives (`context_scrape/mod.rs:414-432`). The ring buffer lives in the concrete sink implementation. Consequence: everything that passes the caps reaches both the event and the buffer, and nothing else reaches either. The ordering requirement is a property of where each piece lives.

With the event coalesced and directed (4.9), the caps no longer carry the IPC load on their own: at 50 saturated sessions the delivered event rate is 250 per second when the window is open and zero when it is closed, regardless of match count. What the caps still bound is the per-tick batch size and the rate at which the ring turns over, which is what they should bound.

### 4.7 The pattern compiler

A new `pty/watchers/pattern.rs`, **not** a reuse of `context_scrape/pattern.rs`, because that compiler rejects any pattern without capture group 1 (`pattern.rs:44-48`, pinned by its own test at `:68-75`) and a watcher pattern legitimately has zero groups.

Everything else is identical:

- `RegexBuilder::new(source).size_limit(1024 * 1024).build()`, with the same rationale: `regex` 1.x has no backtracking and is linear time by construction, so a hostile pattern cannot burn CPU; the size limit is the bound that matters, and past it compilation fails rather than allocating (`context_scrape/pattern.rs:27-30`).
- The source string is kept beside the compiled regex so a recompile is decided by a changed string.
- Compile failure is **sticky** per watcher id, following `Cached::Failed` (`context_scrape/mod.rs:133-140`, `:296-319`): logged once per change, never once per tick.
- The user's pattern is handed over **verbatim**, never trimmed. Leading spaces are anchors, and the pattern is the only defence the feature has (`lib.rs:589-601`).

No matching timeout is added, because there is nothing to time out.

### 4.8 Configuration

In `AppSettings` (`settings.rs:271-538`), two siblings of `agents`:

```rust
/// #1171 - root-level watcher patterns, keyed by watcher id. `AgentConfig` gains no
/// field, so the 20+ struct-construction sites that already had to write
/// `context_regex: None` are untouched. Same shape as `auto_self_clear_by_agent`
/// (`settings.rs:530-531`). `BTreeMap` for stable on-disk order and clean diffs.
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub watchers: std::collections::BTreeMap<String, WatcherEntry>,

/// #1171 - geometry of the activity window. `skip_serializing_if` and NOT a literal
/// copy of `main_geometry` (`settings.rs:367-369`), which lacks it: without the skip,
/// `"watchersGeometry": null` would appear in every user's file on the next save,
/// contradicting section 7.1's promise that configuring nothing leaves no trace.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub watchers_geometry: Option<WindowGeometry>,
```

**A malformed watcher must not destroy the file.** `parse_settings_json` deserializes `AppSettings` in one shot and any failure yields `AppSettings::default()` (2.3). A hand-written `"mode": "State"`, `"commands": "claude"` or `"dedupeWindowMs": "2000"` would therefore start AgentsCommander with **no agents configured**, leaving one log line, and every later save would be refused by the #1077 gate. The map is consequently deserialized entry by entry:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WatcherEntry {
    Valid(WatcherConfig),
    /// Anything that did not deserialize as a `WatcherConfig`. Kept verbatim so a
    /// save round-trips the user's bytes instead of deleting what it could not read,
    /// skipped by resolution, and logged once per changed value.
    Invalid(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub mode: WatcherMode,
    pub pattern: String,
    /// Absent or null: reaches every configured agent. Present: only entries whose
    /// `command` executable stem matches EXACTLY. Present and empty: reaches none.
    /// `Option` and not `#[serde(default)] Vec`, because absent and `[]` are opposites
    /// and only `Option` lets serde tell them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    #[serde(default)]
    pub dedupe: WatcherDedupe,
    #[serde(default = "default_dedupe_window_ms")]
    pub dedupe_window_ms: u64,
    /// Free text, e.g. "claude 2.1.212". Never validated, never parsed. It exists
    /// because `context_scrape/rows.rs:183-186` documents that a TUI format already
    /// had to be re-captured, and that fact currently lives in a Rust comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_against: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatcherMode { State, Occurrence }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatcherDedupe { #[default] Row, Capture, None }
```

`mode` and `pattern` stay required: a watcher without either is not a watcher, and with the wrapper the consequence is one skipped entry rather than a lost configuration. `default_dedupe_window_ms()` returns `2000`. `default_true` already exists (`settings.rs:540-542`). `AppSettings::default()` (`settings.rs:685-765`) gains two lines.

**Applicability.**

```
reaches(w, agent) = w.enabled && match &w.commands {
    None       => true,
    Some(list) => list.iter().any(|s| stem(s) == stem(&agent.command)),
}
```

`stem` is `command_executable_basename` (`coding_agents_catalog.rs:427-432`), promoted from `fn` to `pub(crate) fn`. **A second stem function must not be written, in Rust or in TypeScript**, and the TypeScript `starts_with` rule in `suggestedContextRegex` must not be ported: the catalog code rejects it in writing, naming `pi` and `agent` as the false-match risk (`coding_agents_catalog.rs:494-497`). The Settings UI obtains reach through the `preview_watcher_reach` command (4.9) rather than reimplementing the rule.

| input | stem | effect |
|---|---|---|
| `claude` | `claude` | reached by `["claude"]` |
| `C:\...\claude-sandbox-runtime\claude.cmd` | `claude` | reached by `["claude"]` |
| `pi --provider claude` | `pi` | not reached by `["claude"]`, correctly: the TUI rendering is Pi's |
| `cmd /c claude` | `cmd` | not reached by `["claude"]`. The user adds `"cmd"` or drops the selector |
| `CLAUDE.EXE` | `claude` | case-insensitive on every platform |

- A `commands` entry that does not tokenize, or a list containing an empty string: the **whole watcher is skipped** and logged once. Never "reaches everything".
- A stem no agent has: not an error, reaches nobody. The Settings UI shows "reaches 0 agents" so a typo is visible.
- An agent whose own `command` does not tokenize: not reached by any watcher **with** a selector; watchers without one still reach it. `validate_agent_commands` (`settings.rs:1473-1475`) already rejects such a command on save.

**Scope of "every agent".** A watcher with `commands` absent reaches every entry in `settings.agents`, which means every **agent session**. Plain shell sessions are never registered with the engine, exactly as they are never registered with `ContextScraper` (`commands/session.rs:2268-2274`). "All agents" is not "all sessions".

**Precedence: none.** Two watchers reaching the same agent both run. They have distinct ids and write distinct slots. A "most specific wins" rule would silently discard a pattern the user configured. Two watchers with identical `pattern` and `mode` are two watchers and both fire.

**Budget: 8 watchers per agent.** Resolution iterates the `BTreeMap` in key order and takes the first 8 that reach the agent. Because the key order is alphabetical over user-chosen ids, adding a watcher named `aaa-test` can displace the eighth. The ones dropped are logged once per resolution change **and** reported per row by `preview_watcher_reach`, so the Settings UI shows "not running on <agent> (budget)" instead of leaving the user with a log line.

**The budget is a property of the whole set, and that decides the shape of its preview.** Whether a watcher is inside the first 8 cannot be answered from that watcher alone: it depends on every other watcher that reaches the same agent, on which of them are enabled, and on where their ids fall in key order. It also depends on the agents themselves, which the same modal edits in the same draft. `preview_watcher_reach` therefore receives the entire draft, watchers and agents, and not one row (4.9). Neither the key order nor the number 8 is reimplemented in TypeScript, for the same reason the stem rule is not.

**Watcher ids.** Written by the user, validated against `^[a-z0-9][a-z0-9-]{0,39}$`, unique by construction as the map key. Renaming is delete plus create: the new id starts with no history and rows already in the ring keep the old id, which is the same behavior edge case 17 defines for deletion. The Settings UI states this on the rename control.

**Resolution cadence.** Patterns are resolved fresh every tick, following `ScraperPatterns` (`lib.rs:576-606`), with one `SettingsState` read lock per tick for all sessions. Cost with 12 agents and 8 watchers: 96 stem comparisons and 12 command tokenizations per tick, that is 480 comparisons and 60 tokenizations per second. No cache is added.

**Zero-cost when unconfigured.** The tick reads settings first and returns immediately when no `WatcherEntry::Valid` is `enabled`, **before touching any session**. Revision 1 claimed an empty-registry early exit like `ContextScraper`'s; that was wrong, because registration is per session, not per watcher, so a running agent with no watchers configured would have kept the registry non-empty. The check on the resolved watcher set is what actually delivers the promise.

### 4.9 IPC contract

**Event `watcher_matches`**, one batch per `(session, tick)`.

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherMatchBatch {
    pub session_id: String,
    pub matches: Vec<WatcherMatchPayload>,
}

/// pty/watchers/mod.rs. Mould: `ContextUsagePayload` (`context_scrape/mod.rs:104-115`).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherMatchPayload {
    pub session_id: String,
    /// Monotonic per session, assigned by the engine as the match passes the caps.
    /// The SAME value the ring stores, so the window merges snapshot and stream on
    /// `(sessionId, seq)` instead of guessing. Without it two matches from one tick
    /// are indistinguishable, and edge case 39 requires that they be distinct.
    pub seq: u64,
    /// The key of the root `watchers` map. The same grouping key everywhere.
    pub watcher_id: String,
    pub mode: WatcherMode,
    /// The TICK's instant, not the match's: a match has no instant of its own.
    /// `chrono::DateTime<Utc>` serializes RFC3339, the repository convention
    /// (`resource_monitor/types.rs:158-160`).
    pub at: chrono::DateTime<chrono::Utc>,
    /// Groups 1..n IN ORDER, without group 0. `Option` per element because an
    /// optional group may not participate, and "" is not "did not capture".
    pub captures: Vec<Option<String>>,
    /// The logical row (4.3.1), truncated to 256 bytes on a char boundary.
    pub row: String,
    /// Whether `row` lost bytes to the cap. `row.length >= 256` cannot answer this
    /// in TypeScript, because the cap is on bytes and the row is multibyte.
    pub row_truncated: bool,
}
```

**No `skip_serializing_if` on any field**, for the reason `ContextUsagePayload` documents in writing (`context_scrape/mod.rs:110-114`): absent must never become a third state beside null and the value. `session_id` is repeated on each match so a frontend row is self-contained once inserted.

**Delivery is directed, not broadcast.** `app.emit` reaches every window, so at 50 saturated sessions an uncoalesced per-match event would deliver on the order of 16 000 payloads per second to four windows, and every detached terminal would pay to deserialize events it discards. The sink therefore:

1. Emits **one** `watcher_matches` per `(session, tick)` that produced at least one match.
2. Delivers it with `emit_to` (already used at `testability/ui_automation.rs:631`) targeting the `watchers` window label.
3. **Emits nothing at all when that window does not exist.** The ring buffer still records, so opening the window later shows the history. The window is closed most of the time, and this makes that case free.

The sink, not the engine, holds the `AppHandle` and performs the window check, exactly as `ScraperSink` does (`lib.rs:610-618`), so the engine's capability boundary is untouched. Adding a second consumer later means adding its label or falling back to broadcast, which is a one-line change in the sink.

TypeScript mirror in `src/shared/types.ts`, next to `SessionContextPayload` (`types.ts:122-125`):

```ts
export type WatcherMode = "state" | "occurrence";

export interface WatcherMatchPayload {
  sessionId: string;
  seq: number;
  watcherId: string;
  mode: WatcherMode;
  at: string;                    // RFC3339 UTC
  captures: (string | null)[];
  row: string;
  rowTruncated: boolean;
}

export interface WatcherMatchBatch {
  sessionId: string;
  matches: WatcherMatchPayload[];
}
```

Listeners in `src/shared/ipc.ts`, the mould of `onSessionContext` (`ipc.ts:668-672`):

```ts
export function onWatcherMatches(
  callback: (data: WatcherMatchBatch) => void
): Promise<UnlistenFn> {
  return transport.listen<WatcherMatchBatch>("watcher_matches", callback);
}

export function onWatchersScopeRequest(
  callback: (data: { sessionId: string }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string }>("watchers_scope_request", callback);
}
```

**Command `get_watcher_activity`**, the mould of `get_session_context` (`commands/pty.rs:526-533`):

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherActivitySnapshot {
    /// Oldest first. `limit` trims from the NEW end: `Some(n)` returns the n most
    /// recent, still ordered oldest to newest.
    pub matches: Vec<WatcherMatchPayload>,
    /// The highest `seq` ever inserted for this session. The merge fence for a
    /// window that subscribed before it fetched.
    pub last_seq: u64,
    /// The ring dropped at least one entry since the session started.
    pub truncated: bool,
    /// Monotonic since the session started. NOT a count of lost matches.
    pub possibly_missed_frames: u64,
    /// False until the engine has ticked this session at least once. Without it
    /// the window cannot tell "no watcher reaches this agent" from "the engine has
    /// not run yet", and shows the "Configure watchers" empty state for the first
    /// 200 ms of every session even when watchers are configured.
    pub warmed_up: bool,
    /// The watchers that reach this session's agent right now, with their count
    /// since the session started. Present even when the count is 0.
    pub active_watchers: Vec<WatcherActivityCounter>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherActivityCounter {
    pub watcher_id: String,
    pub mode: WatcherMode,
    pub count: u64,
    /// True while this watcher is hitting a per-tick cap or is suspended (4.6).
    pub degraded: bool,
}

#[tauri::command]
pub fn get_watcher_activity(
    app: AppHandle,
    session_id: String,
    limit: Option<usize>,
) -> Result<WatcherActivitySnapshot, String>
```

- A `session_id` that does not parse as a UUID returns `Err`, like `get_session_context` (`commands/pty.rs:528`).
- A session with no buffer returns an **empty snapshot**, not `None` and not an error: `{ "matches": [], "lastSeq": 0, "truncated": false, "possiblyMissedFrames": 0, "warmedUp": false, "activeWatchers": [] }`.
- An absent `limit` returns everything in the ring, and never more than the cap. The window always passes an explicit limit (4.12), so the unbounded form exists for the CLI-shaped caller and not for the UI.
- The command is **synchronous** and takes one per-session mutex. To make that possible the engine publishes `active_watchers`, `possibly_missed_frames` and `warmed_up` into the history structure at the end of each tick, rather than the command resolving settings and the session manager itself. `mode` rides on the counter so the empty state can say what it is waiting for without a second call.

Wrapper in `src/shared/ipc.ts`, the mould of `getSessionContext` (`ipc.ts:240-241`):

```ts
getWatcherActivity: (sessionId: string, limit?: number) =>
  transport.invoke<WatcherActivitySnapshot>("get_watcher_activity", { sessionId, limit }),
```

**Command `preview_watcher_pattern`**, for the Settings section:

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherPatternPreview {
    pub compiles: bool,
    pub error: Option<String>,
    /// False when no session was given, or the session had no readable frame.
    /// Distinguishes "matched nothing" from "could not look".
    pub sampled: bool,
    pub matched_rows: usize,
    pub total_rows: usize,
    /// Up to 3 matched logical rows, each truncated to 256 bytes.
    pub samples: Vec<String>,
    /// True when the captures of the lowest match differed between the two samples
    /// taken ~1s apart. A pattern that captures a clock or a token counter matches
    /// one row of thirty and still emits five events per second in `state` mode;
    /// `matchedRows` alone cannot say so.
    pub captures_volatile: bool,
}

#[tauri::command]
pub async fn preview_watcher_pattern(
    app: AppHandle,
    session_id: Option<String>,
    pattern: String,
) -> Result<WatcherPatternPreview, String>
```

- `session_id: None` compiles only and returns `sampled: false`, `matched_rows: 0`, `total_rows: 0`, `samples: []`, `captures_volatile: false`. This is the common case: a user opens Settings and writes a regex with no agent session running, and without it the only signal for a syntax error would be the absence of activations.
- With a session id: two frames are read about 1 second apart to compute `captures_volatile`. Any read that is not a frame (`Missing`, `Gone`, or an unparseable id that still parses as a UUID but has no PTY) yields `sampled: false` with the compile result intact. A `session_id` that is not a UUID returns `Err`.
- The command is `async` and performs the PTY reads inside `tokio::task::spawn_blocking`, because it takes the `PtyManager` and `ptys` mutexes and runs a child liveness probe (`local_backend.rs:1089-1109`) while a session may be producing heavy output. The Settings control debounces at 300 ms and never fires per keystroke.

**Command `preview_watcher_reach`**, so the Settings UI reimplements neither stem normalization nor the budget rule, and never has to guess what the budget will be after Save:

```rust
/// One watcher row of the draft the Settings modal holds in memory. Only the three fields
/// that `reaches` and the budget depend on (4.8); `pattern`, `mode`, `dedupe` and
/// `capturedAgainst` take no part in either and are not sent.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherDraftEntry {
    pub id: String,
    pub enabled: bool,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
}

/// One agent row of the same draft. The modal edits agents and watchers in ONE store
/// (`SettingsModal.tsx:989-1029`) and one Save writes both, so resolving against the saved
/// agent list would answer about a state the user has already left.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherAgentDraftEntry {
    pub id: String,
    pub label: String,
    pub command: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachEntry {
    pub agent_id: String,
    pub agent_label: String,
    pub command_stem: String,
    /// Whether this row is enabled in the draft AND holds one of this agent's 8 slots once
    /// every other ENABLED row of the draft is counted. It is the membership of the engine's
    /// own `running` list, and NOT a promise that the watcher will produce anything: a
    /// resolved watcher whose pattern does not compile is allocated a slot and is inert
    /// (edge case 13). Compilability is a separate dimension, answered per row by
    /// `preview_watcher_pattern`, and this field deliberately does not restate it. A
    /// disabled row is always false here, and the editor, which owns `enabled`, must say
    /// "disabled" rather than "budget" (4.12).
    pub allocated: bool,
}

/// The reach of one draft row. Exactly one per requested row, in request order, and it
/// carries `id` back, because the editor filters unrecognised rows out of the request and
/// its table positions therefore do not match the response positions.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachRow {
    pub id: String,
    /// Every agent this row's SELECTOR reaches, whether or not the row is enabled. Reach is
    /// a property of the selector alone; `allocated` is where enablement and budget land.
    pub entries: Vec<WatcherReachEntry>,
}

#[tauri::command]
pub async fn preview_watcher_reach(
    watchers: Vec<WatcherDraftEntry>,
    agents: Vec<WatcherAgentDraftEntry>,
) -> Result<Vec<WatcherReachRow>, String>
```

**Why the whole draft and not one row.** Running is a property of the set (4.8): resolution iterates the `BTreeMap` in key order and takes the first 8 that reach the agent. A command that receives a single row can only answer it by inventing what the rest of the set is, and the only set available to it is the saved one, which is not the one the user is editing. With an empty saved map, adding nine rows before pressing Save makes all nine previews resolve `{}` plus themselves and report that they run, and then only eight do. That is a positive claim that a watcher will run, about a watcher that will not, and it is the opposite of the fail-closed direction section 6 pins everywhere else.

Narrowing the claim to "as currently saved" does **not** rescue the row-level form. The command substitutes the edited row before resolving, so it already speaks about the state after Save; making it honest would mean suppressing the indicator whenever the draft differs from the saved map, which is exactly when it exists to be read. Computing the budget in TypeScript from per-row reach sets does not rescue it either: it duplicates the key order and the number 8 in a second language, `Array.prototype.sort` compares UTF-16 code units while `Ord for String` compares UTF-8 bytes (they agree for ids matching the id pattern and can disagree for a hand-written `settings.json`), and it needs the same lift of reach state out of the row component that the draft-shaped command needs, so it buys nothing.

**Why the agents travel too.** The same store holds both: `updateAgent` writes `settings.data.agents` and `mutateWatchers` writes `settings.data.watchers` (`SettingsModal.tsx:986-1030`), and one Save writes both. Resolving against the saved agent list therefore answers about a state the user has already left, and two of the three agent edits **over-report**: deleting an agent leaves it named in a reach list it will not be in, and changing an agent's `command` leaves a watcher reported as reaching it under the old stem. Only adding an agent under-reports. Carrying `(id, label, command)` per agent is a few short strings on a debounced call, it makes the preview a pure function of the draft, and it removes the last question the UI would otherwise have to answer about which unsaved edits the indicator reflects.

Seven points of fixed semantics, so nothing is left to decide at implementation time:

1. **Both halves come from the draft and nothing comes from disk.** The command reads no settings at all: it builds a `BTreeMap<String, WatcherEntry>` from `watchers`, a `Vec<WatcherAgent>` from `agents`, and hands them to the existing `resolve_watchers`. No second resolution rule is written, and **no settings lock is taken**, so a preview can never contend with a save.
2. **`WatcherEntry::Invalid` rows are not sent.** Resolution skips them before any budget is counted (6, edge case 14), so they consume no slot, and sending them would only produce notices. The editor keeps them verbatim for the save (4.12) and leaves them out of the request. This is only sound if the editor's validity predicate is the exact mirror of the Rust decoder; that requirement is fixed in 4.12 and is not optional, because a row the editor thinks is valid and Rust does not would be counted here and skipped by the engine, which is the same false-positive class this whole revision exists to remove.
3. **Reach and allocation are two different questions, answered by two passes, and neither is a counterfactual budget.**
   - **Pass A, every row forced enabled:** the reach relation. A row reaches an agent when it appears in that agent's `running` or `over_budget` list, which together hold everything whose selector matches. This pass supplies `entries` and nothing else. Reach does not depend on any other row, so forcing enablement here changes no row's answer but its own presence.
   - **Pass B, every row at its real draft `enabled`:** the engine's own resolution. `allocated` is true when the row is in that agent's `running` list.

   A disabled row therefore still shows the agents its selector reaches, which is the state where the control is needed most, and it reports `allocated: false` everywhere, which is true. **No counterfactual budget is computed and none is reported.** Revision 3's first form ran one forced-enabled pass per row and could report nine rows as running with only eight slots, because each row's pass silently displaced a different one; two passes over one draft cannot produce a set of answers that disagree with each other.

   **`allocated` is slot assignment, not a guarantee of output.** The pattern does not travel, because it takes no part in either pass, and a resolved watcher whose pattern fails to compile is inert while still holding its slot (edge case 13). The field is named for what the two passes can actually establish. Carrying the pattern so the command could also report compilability was rejected: `preview_watcher_pattern` already answers exactly that, per row and with the error text, so the second dimension is already on screen next to the first, and duplicating it here would inflate every debounced payload with pattern text for an answer the row already has. Enabling and saving a watcher whose regex does not compile stays possible and stays inert-plus-logged, which is what edge case 13 already fixes.
4. **What this deliberately does not forecast:** enabling a disabled row can push an enabled one out of budget, and no row's answer says so in advance. Nothing false is stated, and the moment the user enables the row the call re-fires and the displaced row shows its budget badge. A `displacedWatcherIds` field was considered and left out: it adds a concept to forecast a state that becomes visible and correct one click later.
5. **Cost, stated in the right variable.** `resolve_watchers` tokenizes every selector entry once and then, for every agent, scans the stems of every usable watcher (`mod.rs:197`, `:253`, `:267`). The cost of one pass is therefore **O(A x S)**, where A is the number of agents and **S is the total number of selector entries in the payload**, not the number of rows: one row carrying ten thousand selector entries costs what ten thousand rows carrying one each cost. The call runs two passes.

   The defense is not that S is small, because nothing bounds it. It is that **the engine already runs exactly this resolution over exactly this data every 200 ms** (4.8, resolution cadence), so a payload large enough to make the preview expensive is already costing five times as much per second inside the tick, and the bound that would matter belongs there and not here. Two debounced passes cannot be the worse offender. Revision 3's first form claimed "a few milliseconds, nothing to protect"; that priced only the case of one short selector per row and is withdrawn. The residual is debt item 12.

   No global cap on the number of watchers or selector entries is introduced, for the reason above. The computation is moved off the async worker into `tokio::task::spawn_blocking`, following `preview_watcher_pattern` (4.9), because it is synchronous CPU over an input the caller controls and an `async fn` would otherwise hold a Tokio worker for its duration. The command owns its inputs and takes no lock, so the move is a wrapper and nothing else.
6. **Duplicate ids in the draft:** the later row wins when the `BTreeMap` is built, and both response rows report that one resolution. **An empty id** is a legal key that sorts first and is not special-cased. Neither is reachable from the editor, whose map is keyed by id and whose renames are validated (4.12); the rule exists so the command has defined behavior, not because the UI can produce it.
7. **Order:** exactly one response row per requested row, in request order. Within a row, `entries` keeps the order revision 2 fixed, by `agentLabel` with `agentId` as tie-break, so the list does not reshuffle between keystrokes. A row whose selector does not tokenize, or whose `commands` is `[]`, is still present, with `entries: []`.

The command takes neither an `AppHandle` nor `State<SettingsState>`: revision 2's `app: AppHandle` asked for a capability it never used, and after point 1 there is nothing left to read. `WatcherDraftEntry`, `WatcherAgentDraftEntry`, `WatcherReachEntry` and `WatcherReachRow` mirror into `src/shared/types.ts`, and the `src/shared/ipc.ts` wrapper takes both halves:

```ts
previewWatcherReach: (
  watchers: WatcherDraftEntry[],
  agents: WatcherAgentDraftEntry[]
) => transport.invoke<WatcherReachRow[]>("preview_watcher_reach", { watchers, agents }),
```

**Commands `open_watchers_window(app, session_id: String)`** (mould: `open_resource_monitor_window`, `commands/window.rs:686-762`), **`get_watchers_scope(scope) -> Result<Option<String>, String>`** (4.12, the durable half of the scope handover) **and `set_watchers_geometry(settings, geometry)`** (mould: `set_detached_geometry`, `commands/window.rs:528-538`).

All commands are registered in the `tauri::generate_handler!` list at `lib.rs:2251`. `open_watchers_window` and `get_watchers_scope` are **not** added to the web no-op list; see 4.12.

### 4.10 The history buffer and its lifetime

```rust
/// pty/watchers/history.rs. Mould: `SessionWarningBuffer` (`session/warnings.rs:39-51`).
pub struct WatcherHistory {
    /// Outer lock guards the map only and is released before any read of a session's
    /// entry, so a snapshot never blocks the engine's writes to other sessions.
    sessions: Mutex<HashMap<Uuid, Arc<Mutex<SessionHistory>>>>,
}
pub type WatcherHistoryState = Arc<WatcherHistory>;
```

Three deliberate differences from the precedent, plus one from revision 1:

- **`VecDeque` with `pop_front`, not `Vec` with `remove(0)`.** `SessionWarningBuffer` uses the latter (`warnings.rs:48`), O(n) per insert. At its cap of 32 that is irrelevant; at 500 under a burst it is not.
- **`snapshot(limit)`, not `drain`.** `SessionWarningBuffer::drain` consumes (`warnings.rs:53-63`), which would mean closing and reopening the window shows nothing.
- **Cap: 500 entries per session**, hard, oldest dropped, setting `truncated` on the first drop. With `row` capped at 256 bytes the worst-case element is about 500 to 550 bytes, so 20 sessions is about 3.5 MB typical and about 5.5 MB ceiling.
- **Per-session locks, not one global mutex.** The engine takes this structure up to 250 times per second at 50 sessions; a single mutex would serialize every snapshot against every write.

Not persisted. The window never claims history across restarts.

**Lifetime.**

- **Created** on the session's first tick, and **only for a session the engine currently has registered**. Lazy creation without that condition would let a tick recreate the entry of a session that was purged moments earlier.
- **Purged** through a single new helper:

```rust
/// commands/session.rs. One helper, two call sites, because the per-session
/// side-state purge is already duplicated in this file.
pub(crate) fn purge_session_side_state<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    // 1. retire from the watcher engine FIRST, so no in-flight tick can republish
    //    `active_watchers` and recreate the entry that step 2 removes.
    // 2. purge WatcherHistory.
    // 3. reset SubstantiveInputState (moved here from its two existing copies).
}
```

  Called from **both** post-commit loops:

  - `execute_destroy_transaction`, in a **new loop over `outcome.destroyed_ids` only**, placed after the existing loop at `session.rs:3181-3208`. Not inside that loop with a `contains` check: it iterates `destroyed_ids.chain(retained_exited_ids)`, so a membership test would be O(n squared) and, worse, would express a business rule as a condition inside a loop that does something else. A separate loop makes "destroyed only" structural, which is the same argument section 4.6 uses for where the caps live.
  - `publish_restart_destroyed` (`session.rs:3758-3782`), replacing its inline `SubstantiveInputState` reset at `:3773-3781`. This covers both restart paths, `execute_restart_transaction` (`session.rs:4026`, published at `:4064`) and `finalize_failed_restart` (`:3792`, published at `:3807`).

  Revision 1 claimed `execute_destroy_transaction` was the only production exit from `SessionManager`. That was **false**: there are three (2.4), and the precedent it cited was one of two parallel copies. A session restart is a normal flow, reachable from the invoke handler, the mailbox and a bulk settings save, and without this helper every restart would orphan up to 500 entries, roughly 275 KB, unreachable from the UI because the old id is gone from the session list.

- **Purge only for `outcome.destroyed_ids`, never for `outcome.retained_exited_ids`.** The latter are root-agent sessions that stay in the manager marked `Exited` (`session.rs:3096-3102`); their row is still in the list, so their post-mortem view still has a place to be shown.
- **A session that exits on its own is not destroyed.** The engine retires it from sampling on the next liveness probe, or immediately on `Gone`, and the buffer stays. This is the case that matters: an API error or a CLI crash is exactly when the evidence is worth keeping.
- **Spawn rollback needs nothing.** `rollback_pre_created_session` returns before the registration site (`commands/session.rs:2254-2258` against `:2268`), and `rollback_pending_create` (`session/manager.rs:1416-1434`) only removes sessions that never reached that point.
- **`SessionManager::destroy_session` needs no purge call**, being test-only (2.4).

The registration comment at `commands/session.rs:2266-2267` ("the first sample is 5s away") must be updated when the engine registration is added beside the scraper's: the first watcher tick is 200 ms away. There is still no race, because `PtyManager::spawn` has already returned `Ok` at `:2252` and the parser is registered during the spawn, but the comment must not be left asserting something false next to new code.

### 4.11 `chrono::DateTime<Utc>`, resolved

`Cargo.toml:18` declares `chrono = { version = "0.4", features = ["serde"] }` for the single crate `agentscommander_lib`, of which `src/pty/` is part. `chrono::Utc::now()` is already called from `src-tauri/src/pty/output.rs:501`. `DateTime<Utc>` in a `#[derive(Serialize)]` struct works with the already-enabled `serde` feature and produces RFC3339, the convention `resource_monitor/types.rs:158-160` follows into `types.ts`. **No new dependency and no new feature flag.**

### 4.12 Frontend

**Button.** In the terminal StatusBar's `.status-bar-actions` (`src/terminal/components/StatusBar.tsx:100`), inside `<Show when={terminalStore.activeSessionId}>` (`:99`), next to the mic (`:110-125`) and clear-input (`:126-132`) buttons. Not in `.workgroup-task-actions`, because the TASK panel renders inside `<Show when={!terminalStore.activeIsRootAgent}>` (`src/terminal/App.tsx:401`) while `<StatusBar />` sits outside it at `:422`, so on root-agent sessions the TASK bar does not exist. Glyph `&#x1F4E1;`, title "Watcher activity", plus a `data-ac-testid`. It reuses the existing `.status-bar-btn` class (`terminal.css:631-660`) and adds **no new CSS**: it has no state of its own.

**The button renders only under `isTauri`** (`src/shared/platform.ts:3-4`). `src/browser/App.tsx:2` renders the same `TerminalApp` in the web client, where `open_watchers_window` would fall into the catch-all `_ => Err(...)` of `web/commands.rs:702`, and where `src/main.tsx` never routes `window=watchers` anyway. A button that returns a raw error is worse than an absent button, so `web/commands.rs` is left untouched.

**Window.** A singleton Tauri window, label `watchers`, URL `index.html?window=watchers&sessionId=<uuid>`, `decorations(false)`, `zoom_hotkeys_enabled(false)`, focus-if-exists. Routed in `src/main.tsx` next to the `resource-monitor` branch (`main.tsx:44-45`), rendering a new `src/watchers/App.tsx`. Its own HTML titlebar, mould `src/spec-board/components/SpecBoardTitlebar.tsx`.

**Re-open with a different scope, and why an event alone cannot carry it.** If the window already exists, `open_watchers_window` focuses it and emits `watchers_scope_request { sessionId }`; the window listens through `onWatchersScopeRequest` and switches scope, leaving its filters as they are.

That is not sufficient on its own. The window label exists the moment `WebviewWindowBuilder::build` returns, while the JavaScript listener exists only after the bundle loads, Solid mounts and `onWatchersScopeRequest` completes an IPC round trip. Tauri does not queue events for listeners that do not exist yet, and `emit_to` returns `Ok` either way, so the backend cannot even observe the loss. The lifecycle has **three** states, absent, created but not yet listening, and listening, and a contract that branches only on "the window exists" has no answer for the middle one: a second `open` during the load focuses a window that is not listening, emits, and the user's order is dropped in silence.

The fix is the shape this plan already uses for matches, subscribe first and then reconcile with a pull, applied to the scope:

1. A managed `WatchersScopeState(Mutex<Option<Uuid>>)` holds **the most recently requested scope**.
2. `open_watchers_window` writes that state **before** anything else, then creates the window, or focuses it and emits, as above. **The whole command from the state write through both branches is one critical section**; see point 6.
3. `get_watchers_scope()` (4.9) returns that value.
4. The window, in `onMount`, **after** `onWatchersScopeRequest` is registered, calls `get_watchers_scope()` and adopts what it returns **unless** a `watchers_scope_request` has been handled since the call was issued. That guard is the same generation idiom `lastSeq` and the two Settings previews already use.
5. **The query parameter stays and becomes the initial value**, so the first paint has a scope without waiting for a round trip. The pull is the authoritative one and overrides it. The parameter is still read only on first creation, since the window is a singleton and its URL does not change afterwards.
6. **One `tokio::Mutex`, acquired after the UUID is validated and held across the state write, the existence check, and whichever branch runs, including the emit.** Not two regions. Splitting it leaves this interleaving: A writes scope A and pauses; B writes scope B, takes the build guard and creates the window with B in its URL; A then takes the guard, finds a window, and emits A. The authoritative state says B while the last event says A, so a window that is already listening lands on A and disagrees with `get_watchers_scope`. Holding one guard across the whole body makes the last write and the last emit the same call by construction. The mutex is private to this command, so it cannot deadlock against anything else, and it is contended only by a second open.

Why this is durable, which is the only question that matters: the command is linearizable, the state is written before every emit and read after the subscribe, so no interleaving loses an order. An emit that raced the subscribe is recovered by the pull; an emit after the pull reaches a listener that exists; a pull whose response arrives after a newer event is discarded by the guard.

**The window needs a second guard, on content, and it is not the same one.** The pull guard decides *which scope is adopted*. It says nothing about *which rows are painted*: `refresh()` captures its session ids, awaits, and then writes snapshots and rows unconditionally, and its callers do not serialize against each other. A refresh for A still in flight when the window moves to B returns last and repaints A while the selector reads B. Three rules, and they are deliberately stated over `scopeIds()` and over requests rather than over a list of callers, because every enumeration of callers so far has been short by one:

- **One monotonic request counter. Every `refresh()` takes the next value on entry and commits only if it is still the highest one issued.** That covers the scope selector, the `watchers_scope_request` listener, the adoption of the pull, the mount's initial fetch and the poll, and it also covers two refreshes of the **same** scope racing each other, which a scope-keyed generation would let through: the older poll response would otherwise commit an older `lastSeq`, `warmedUp`, `degraded` and counters over the newer ones and leave the table and the counters describing two different instants.
- **Any change of `scopeIds()` invalidates and refetches**, not any change of the selected session. In "All sessions" the scope is a set derived from the session list, and the three session listeners rewrite that list without touching the selection: creating or destroying an agent session changes what the window should show. Without this, an in-flight refresh commits with the old ids and drops the new session's streamed rows, and the new session is not queried until the next poll, up to 15 seconds later with an incomplete table and nothing saying so. Stated this way the rule is also shorter than the enumeration it replaces, because the selector, the event and the pull all change `scopeIds()` by definition, while reloading the session list under a single-session scope does not and correctly changes nothing.
- **A scope change drops the content it invalidates, synchronously.** Snapshots are cleared and rows outside the current ids are dropped before the new selector is painted. Without this the selector changes immediately while the previous session's rows stay on screen for the whole round trip, and stay forever if the new fetch fails. "A correct selector over stale rows" is the failure this whole guard exists to prevent, and a commit-time check alone does not prevent it, because nothing has to commit for it to happen.

There is **no startup carve-out**. An earlier form had the listener set scope without fetching during mount, so that an event arriving before the first fetch would not start a competing round; that left a hole where an event arriving *during* the first fetch got the old fetch discarded and no fetch of its own, and the window sat correctly scoped and empty until the poll. With one counter the competing round is harmless, the older result is discarded on arrival, and one uniform rule replaces a rule plus an exception.

Two alternatives were considered and rejected, recorded so they are not rediscovered. `WebviewWindowBuilder::on_page_load(Finished)` needs no new IPC but fires before the listener registration completes, so it narrows the race rather than closing it, and a narrower race is not the standard this plan holds itself to. Treating the window's first `get_watcher_activity` call as an implicit readiness signal would also need no new IPC, but it would couple a command's meaning to a frontend ordering detail that nothing enforces.

**Geometry.** `watchers_geometry` on `AppSettings`, written through the dedicated `set_watchers_geometry` command and **not** through `initWindowGeometry`. Reason: `initWindowGeometry` (`src/shared/window-geometry.ts:26-47`) performs a debounced read-modify-write of the whole `AppSettings`, a race the repository already documents (`commands/config.rs:653-655`) and defends against with an explicit list of live-memory-restored fields (`config.rs:611-624`, `:647-655`). Adding `watchers` to that list would make it six fields and would leave the window as a whole-object writer; a dedicated command follows `set_detached_geometry` (`window.rs:528-538`) and touches one field. `src/shared/window-geometry.ts` is therefore **not** modified.

**Layout.** Titlebar, filter bar, scrolling table, fixed footer.

Columns, newest first:

1. **Time**, `at` formatted `HH:MM:SS`. `formatTimestamp` (`src/resource-monitor/App.tsx:62-71`) cannot be reused: it is a module-local `const`, not exported, and returns `toLocaleTimeString(...)`, which is `02:31:05 PM` under en-US. A new `src/shared/time-format.ts` exports `formatClockTime(value: string | null | undefined): string` producing zero-padded 24-hour `HH:MM:SS`. The Resource Monitor is **not** refactored onto it.
2. **Watcher**, a chip carrying the watcher id, colour derived deterministically from the id, **plus a mode marker**. Rows from the two modes are otherwise visually identical, and the word "state" invites reading the last such row as the current state. The window's help text states that a state row records when a condition was **first seen**, not that it still holds.
3. **Session**, two lines, visible only in "All sessions" scope: `session.name` on top, `agent label · workgroup` below in muted text.
4. **Captures**, `captures[0]` in a monospace cell with **left-side ellipsis**, so a path's tail stays visible. Remaining groups as secondary chips. A per-row copy button.
5. **Raw `row`** is **not** a column. Clicking a row expands it below, monospace, `white-space: pre-wrap` with `word-break: break-word`, following the real behavior of `.workgroup-task-text` (`terminal.css:227-236`). Revision 1 prescribed `pre` plus `overflow-x: auto` while citing that rule as precedent, which it is not; a nested horizontal scroller inside a vertically scrolling table is also worse for a 256-byte row.

Scrolling: its own `overflow-y: auto`, auto-scroll only while the user is pinned at the top.

Solid's `<For>` keys rows on `(sessionId, seq)`.

**Scope against filters**, two zones in one bar:

- **Scope**, deciding what is fetched: a session selector plus an "All sessions" toggle.
- **Filters**, deciding what is shown: chips for `Watcher`, `Agent` and `Workgroup`, plus free text over `captures`. `Set<string>` per dimension, AND between dimensions, OR within one, and a "Clear filters" control, reusing the Resource Monitor's shape (`src/resource-monitor/App.tsx:374-406`, `:606-744`) and its CSS classes.

With scope set to one session, `Agent` and `Workgroup` have one possible value each and are **not rendered**, the gesture `<Show when={workgroupOptions().length > 0}>` already uses (`App.tsx:669`). They appear on "All sessions".

The scope does **not** follow the active session. Filters are **not** persisted between openings, following the Resource Monitor's plain `createSignal`s (`App.tsx:364-369`).

**Fetching and staying correct.**

- On mount and on every scope change: subscribe to `watcher_matches` **first**, buffer arrivals, then call `get_watcher_activity` per session in scope, then merge on `(sessionId, seq)` discarding anything at or below the snapshot's `lastSeq` that is already present. This is only possible because `seq` exists; with `at` alone, two matches from one tick are indistinguishable and edge case 39 requires that they be distinct.
- **On mount the scope is settled before the first fetch.** The order is: subscribe to `watcher_matches`, subscribe to `watchers_scope_request`, `get_watchers_scope()`, adopt it unless the scope adoption guard says a newer event already arrived, and only then fetch. Fetching before the scope is settled would issue a round of calls for a session the window is about to leave.
- **Every fetch, including the mount's, takes the request counter and commits only if it is still the highest issued** (see the re-scope block above). Without it the window can show the selector on one session and the table on another, which is worse than showing neither.
- **The two guards are separate and must not be merged.** The **scope adoption guard** decides whether the pulled scope is adopted; the **request counter** decides whether a fetch may paint. They protect different things and each leaves the other's failure open.
- Explicit limits: **500** per session in single-session scope, **100** per session in "All sessions" scope. Fifty sessions at the full ring would be 25 000 payloads on first paint.
- **Snapshot polling**, because `truncated`, `possiblyMissedFrames`, `warmedUp`, `activeWatchers` and `degraded` exist only in the snapshot and there is no cross-window settings event to hang a refresh on (2.5). Cadence copies the Resource Monitor's written precedent (`ActionBar.tsx:81-87`): **10 s** while the window is focused, **15 s** while it is not. In "All sessions" that is N calls per period, which at 20 sessions is 2 calls per second against a per-session mutex. The number is written here rather than discovered later.
- The poll also refreshes `settingsStore` so a watcher saved from the modal turns empty state 1 into empty state 2 without the user reopening the window.
- The window calls `settingsStore.load()` on mount: `src/shared/stores/settings.ts` does not autoload, and the store is needed for the live agent-label fallback.
- The window subscribes to `onSessionCreated` (`src/shared/ipc.ts:355`), `onSessionDestroyed` (`:367`) and `onSessionRenamed` (`:404`), each released in `onCleanup`. Without them a session created while the window is open never enters the scope selector and its matches arrive unlabelled.

**Agent and workgroup resolution, entirely in the frontend:**

- Session list from `SessionAPI.list()` (`src/shared/ipc.ts:199`), maintained by the three listeners above.
- Agent from `session.agentId` and `session.agentLabel` (`src/shared/types.ts:36-37`), falling back to `agents.find(a => a.id === session.agentId)?.label`, the precedent being `liveAgentLabel` (`src/sidebar/components/ProjectPanel.tsx:903-908`). **Filter key is `agentId`, chip text is `label`**; on a duplicate label the chip appends a short id suffix.
- Workgroup from `extractWorkgroupName(session.workingDirectory)` (`src/shared/path-extractors.ts:20-26`), **not** from `session.name`: the backend derives the pair from the spawn cwd and says so in writing (`commands/session.rs:2093-2098`).
- Both are **frozen into the row when it is inserted**, so rows survive their session's death with the right label; the displayed label re-renders live from the settings store by `agentId`, with the frozen value as fallback.

The `Agent` chip is named `Agent` because it is a `settings.agents` entry. A future filter by workgroup replica must be called `Role`, which is what the Resource Monitor calls that concept (`App.tsx:377`, `:390`).

**Empty states, three of them, distinguished without nullability:**

1. `warmedUp && activeWatchers.length === 0`: no configured watcher reaches this session's agent. Message plus a **"Configure watchers"** button. This is what everybody sees on day one.
2. `activeWatchers` non-empty, all counts 0, `matches` empty: configured and waiting. The active watchers are listed with their `mode` and their zero counters.
3. `matches` non-empty: the table. If `truncated`, a one-line banner: *"Older activations were dropped (buffer limit)"*.

While `warmedUp` is false the window shows a neutral starting state, never state 1.

`possiblyMissedFrames` above zero gets a **separate and weaker** line: *"Some screen output was not sampled"*. It must not be merged with `truncated`: one is exact knowledge that something was lost, the other is uncertainty about whether anything was.

**Footer, always visible, never a tooltip:** *"Best-effort. Activations can be missed. This is not an audit log."*

**The "Configure watchers" button** calls `WindowAPI.focusMain()` (`src/shared/ipc.ts:474`) and then `emitOpenSettings("watchers")`. Focusing first is required: `SettingsModal` is mounted only in the sidebar (`ActionBar.tsx:388`), so with the watchers window in front the modal would open behind it and nothing visible would happen.

**Settings section.** A new "Watchers" tab in `src/sidebar/components/SettingsModal.tsx`, which needs three edits and not two:

1. `"watchers"` added to the `SettingsTab` union (`:96`).
2. An entry in `TABS` (`:98-103`).
3. **A branch in `resolveSettingsSection` (`:105-110`).** Without it the resolver falls back silently to `"general"`, and the day-one gesture of empty state 1 would open Settings on the wrong tab with no error and no log. The one existing precedent for an auxiliary window requesting its own section is broken for exactly this reason (2.5), so it must not be copied.

The section edits the root map as a list of rows: id, enabled, mode, pattern, commands, dedupe, dedupe window, captured-against. Specifics that are decisions, not implementation detail:

- **`commands` is edited through a two-state control, `All agents` or `Selected`.** A plain multiselect left empty produces `[]`, which is the exact opposite of absent. `All agents` writes `null`; `Selected` writes the list, and an empty list in `Selected` is a valid state meaning "nobody", shown as "reaches 0 agents".
- The options offered under `Selected` are the distinct **command stems**, not agent entries: five `claude` entries are one option. A free-text entry is allowed for a stem no agent currently has, which 4.8 defines as a non-error.
- Reach and budget come from `preview_watcher_reach`, **which receives the whole draft, watchers and agents**, so both the stem rule and the budget rule stay in Rust and the indicator answers what will happen when the user presses Save rather than what is true of the file on disk.
- **The two fields are worded differently, because they answer different questions.** `entries` is what the row's selector reaches; `allocated` is whether the row holds a slot on that agent after Save.
  - Enabled row: "Reaches N agents", and for each entry with `allocated: false`, "not running on <agent> (budget)". An enabled row that reaches an agent and holds no slot can only be out of budget, so the badge names the one real reason.
  - Disabled row **with a pattern**: "Would reach N agents when enabled", and **no budget badge at all**. A disabled row holds no slot because it is disabled, and presenting that as a budget outcome would name the wrong cause. Present-tense "reaches" on a disabled row is a false statement about a watcher that is doing nothing, which is the over-reporting direction section 6 forbids.
  - Disabled row **with an empty pattern**: "Would reach N agents when enabled. Add a pattern to enable it." This is the state every user sees the instant they press Add Watcher, and the previous sentence alone offers a condition the editor refuses to let them meet. Naming the missing condition first and keeping the reach after it is the choice, rather than hiding the reach until a pattern exists: configuring the selector before the pattern is a legitimate order of work, and it is the reason the reach of a disabled row is reported at all.
- **The reach state lives on the section, not on the row.** One debounced call at 300 ms carrying every valid row plus the draft agents; each row renders its own `entries`, indexed by `id`, because the request omits unrecognised rows and response positions therefore do not match table positions.
- **One rule keeps that call honest, and it is keyed on the request, not on the draft.** The section computes the **fingerprint of the request it would send**, that is the serialized watcher rows plus agent rows, and:
  - The displayed answer belongs to exactly one fingerprint. While the current fingerprint equals the displayed one, nothing is cleared and no call is made.
  - When the current fingerprint differs from the displayed one, the displayed answer is cleared **synchronously** and a call is issued, unless a call for that same fingerprint is already in flight, in which case its answer is awaited. An answer is rendered only if its fingerprint is still the current one when it resolves. A rejected call renders an error, not the previous answer.

  Keying on the request rather than on "any change to the draft" is what makes the rule consistent with itself. An earlier form paired an idempotence guard ("skip the call when the serialized request is unchanged") with a clear-on-any-change rule, and those two contradict: a `pattern` keystroke changes the draft but not the request, so the answer would be cleared and no call would ever replace it, leaving the row pending forever. Under the fingerprint rule a `pattern` keystroke changes nothing at all, which is the correct behavior, and a draft that returns to a shape it already had is re-requested rather than restored from a cache, because a cache would add an invalidation question to save one debounced call on an uncommon path.

  Clearing synchronously matters because a commit-time guard stops a stale answer from being **written**, not from being **read**: on its own it leaves the old answer on screen for the debounce plus the round trip, and a user who presses Save in that gap decides against an indicator belonging to a draft that no longer exists. Save is **not** blocked while pending: the requirement is that the indicator never states something false, not that the user waits for it.
- The **id** is user-written and validated against `^[a-z0-9][a-z0-9-]{0,39}$`. The rename control states that renaming is delete plus create and that existing history keeps the old id.
- **A new row is born `enabled: false`, and the editor refuses to enable a row whose `pattern` is empty.** An empty pattern is a valid regex that matches every row, so a row born enabled turns Add plus an accidental Save into a watcher that matches everything on every agent, fills the caps, turns the ring over, goes degraded and can displace a useful watcher out of an agent's budget, all without the user having written a pattern or looked at the preview. This is a decision, not a preference: the birth state of a row is the same altitude as the two-state selector control and the id rule, and leaving it to implementation leaves it with no requirement and no test. The serde default for a **hand-written** file stays `default_true` (4.8), because an omitted `enabled` in a file someone wrote deliberately means on; only the editor's new-row shape changes. The Rust pattern compiler is **not** changed to reject an empty pattern: it is a legal regex, a hand-written one is bounded by the caps and the suspension rule (4.6), and the editor is where the user is.
- **The editor's validity predicate accepts nothing the Rust decoder would reject, and the editor cannot produce a value the decoder would reject.** Today's predicate accepts any array as `commands` and any `number` as `dedupeWindowMs`, while Rust requires `Vec<String>` and `u64`, so `commands: [1]` or a typed `dedupeWindowMs: -1` is editable in the UI and `WatcherEntry::Invalid` in Rust. That gap is not cosmetic here: such a row would be sent as valid to `preview_watcher_reach`, counted against the budget, possibly push a real watcher out of it, and then be skipped by the engine after Save.

  The requirement is one-directional and that direction is the point. It is **not** "the exact mirror of the decoder": the decoder accepts several fields absent, through `#[serde(default)]` on `enabled`, `dedupe`, `dedupe_window_ms`, `commands` and `captured_against`, while the predicate requires the first three present. Being stricter than the decoder is safe, because the worst it does is leave a row out of the request, which under-reports budget; and it is unreachable in practice, because the frontend never sees the file's bytes but what Rust re-serialized, and those three fields carry no `skip_serializing_if` and are therefore always written. Relaxing the predicate to accept absences that cannot arrive would weaken it for nothing. **The predicate mirrors what the serializer emits, and rejects everything the decoder would reject.**

  Concretely: `commands` is absent, null, or an array **every element of which is a string**; `dedupeWindowMs` is a number that is a **non-negative safe integer**, that is `Number.isSafeInteger(n) && n >= 0`. `typeof n === "number" && n >= 0` is not sufficient and is the naive correction this rule exists to exclude: it still admits `1.5` and `1e30`, both of which serde rejects, so both would travel and consume a slot. Safe-integer is deliberately **narrower** than `u64`: JavaScript cannot represent every `u64` exactly, so a hand-written value above 2^53 is classified as unrecognised rather than silently rounded. Such a row still runs in the engine, clamped by 4.5, and the editor lists it as unrecognised instead of offering to edit a number it cannot hold.

  A row that fails the predicate is treated exactly like any other unrecognised entry: preserved verbatim, not offered for editing, not sent.
- **"Test against current session"** uses a session selector populated with sessions that have an `agentId`, preselecting `sessionsStore.activeId` (`src/sidebar/stores/sessions.ts:294-296`) when it qualifies, and allowing "no session" which exercises the compile-only path. Debounced at 300 ms.

Hand-editing `settings.json` stays valid because it is the same file.

---

## 5. Affected surfaces, exact files and symbols

### 5.1 New files

| path | contents |
|---|---|
| `src-tauri/src/pty/watchers/mod.rs` | `WatcherEngine`, `FrameStamp`, `ScreenFrame`, `ScreenRowsSince`, `WatcherMatchPayload`, `WatcherMatchBatch`, `WatcherMode`, narrow sink traits, `start`, `tick`, `register_session`, `retire_session`, caps and suspension |
| `src-tauri/src/pty/watchers/pattern.rs` | `WatcherPattern`, `compile` (size limit kept, group-1 requirement dropped) |
| `src-tauri/src/pty/watchers/frame.rs` | hashing, `best_shift`, `shift_dirty`, logical-row joining, dirty and stabilization rules, `possibly_missed_frames` |
| `src-tauri/src/pty/watchers/dedupe.rs` | key suppression with TTL, bounds and pruning |
| `src-tauri/src/pty/watchers/history.rs` | `WatcherHistory`, `SessionHistory`, `WatcherActivitySnapshot`, `WatcherActivityCounter` |
| `src/shared/time-format.ts` | `formatClockTime` |
| `src/watchers/App.tsx` | the window root |
| `src/watchers/components/*.tsx` | titlebar, filter bar, table, expandable raw row, empty states, footer |
| `src/watchers/styles/watchers.css` | window styles |

### 5.2 Modified backend files

| file | change |
|---|---|
| `src-tauri/src/pty/mod.rs` | `pub mod watchers;` next to `pub mod input_activity;` (`:14`) |
| `src-tauri/src/pty/output.rs` | add `SessionIoFanout::get_screen_rows_since`, next to `get_screen_rows` (`:295-301`). Nothing existing is modified |
| `src-tauri/src/pty/backend.rs` | add **defaulted** `PtyBackend::screen_rows_since`, following `context_session_liveness` (`:148-154`) |
| `src-tauri/src/pty/local_backend.rs` | override `screen_rows_since`, straight to the fanout, no child probe, `Missing` on absent parser |
| `src-tauri/src/pty/container_backend.rs` | same override next to `get_screen_rows` (`:3171-3179`), `Gone` on absent parser to preserve its documented oracle |
| `src-tauri/src/pty/manager.rs` | `PtyManager::screen_rows_since`, mirroring `get_screen_rows` (`:575-580`), `Gone` on a missing route |
| `src-tauri/src/config/settings.rs` | `AppSettings::watchers`, `AppSettings::watchers_geometry`, `WatcherEntry`, `WatcherConfig`, `WatcherMode`, `WatcherDedupe`, `default_dedupe_window_ms`, two lines in `Default` (`:685-765`) |
| `src-tauri/src/config/coding_agents_catalog.rs` | `command_executable_basename` from `fn` to `pub(crate) fn` (`:427`) |
| `src-tauri/src/lib.rs` | adapters `WatcherRows`, `WatcherPatterns`, `WatcherSink` next to the scraper adapters (`:524-699`); construction, `start` and `manage` for the engine, for `WatcherHistory` and for `WatchersScopeState` next to `:1051-1073`; **six** entries in `generate_handler!` (`:2251`), the five of revision 2 plus `get_watchers_scope`. Revision 2 said seven and revision 3 repeated it; the tree registers five (`lib.rs:2548-2550`, `:2587`, `:2593`) |
| `src-tauri/src/commands/session.rs` | register with the engine in the same block as the scraper and correct the comment (`:2266-2274`); new `purge_session_side_state` helper; a new `destroyed_ids`-only loop after `:3181-3208`; `publish_restart_destroyed` (`:3758-3782`) switched to the helper |
| `src-tauri/src/commands/pty.rs` | `get_watcher_activity`, `preview_watcher_pattern`, `preview_watcher_reach` with `WatcherDraftEntry`, `WatcherAgentDraftEntry` and `WatcherReachRow`, next to `get_session_context` (`:526-533`). The reach command holds no state, takes no lock, and runs its two passes inside `spawn_blocking` |
| `src-tauri/src/commands/window.rs` | `open_watchers_window` and `WATCHERS_WINDOW_LABEL`, following `open_resource_monitor_window` (`:686-762`), with **one** `tokio::Mutex` held from the scope-state write through both branches including the emit; `WatchersScopeState` and `get_watchers_scope`; `set_watchers_geometry`, following `set_detached_geometry` (`:528-538`) |

### 5.3 Modified frontend files

| file | change |
|---|---|
| `src/main.tsx` | a `windowType === "watchers"` branch next to `resource-monitor` (`:44-45`) |
| `src/shared/types.ts` | `WatcherMode`, `WatcherMatchPayload`, `WatcherMatchBatch`, `WatcherActivitySnapshot`, `WatcherActivityCounter`, `WatcherPatternPreview`, `WatcherDraftEntry`, `WatcherAgentDraftEntry`, `WatcherReachEntry`, `WatcherReachRow`, `WatcherConfig`; `watchers` and `watchersGeometry` on `AppSettings` (`:420`) |
| `src/sidebar/components/settings-watchers.ts` | `isWatcherConfig` tightened to the exact mirror of the Rust decoder, and `newWatcherConfig` born `enabled: false` (4.12) |
| `src/shared/ipc.ts` | `onWatcherMatches` and `onWatchersScopeRequest` next to `onSessionContext` (`:668-672`); `getWatcherActivity`, `previewWatcherPattern`, `previewWatcherReach` taking the draft array, next to `getSessionContext` (`:240-241`); `openWatchersWindow`, `getWatchersScope` and `setWatchersGeometry` on the window API |
| `src/terminal/components/StatusBar.tsx` | the button inside `.status-bar-actions` (`:100`), gated on `isTauri` |
| `src/sidebar/components/SettingsModal.tsx` | `SettingsTab` union (`:96`), `TABS` (`:98-103`), **`resolveSettingsSection` branch (`:105-110`)**, and the Watchers section, whose reach state is held by the section rather than by each row (4.12) |

### 5.4 Explicitly untouched

`src-tauri/src/pty/context_scrape/**`, `AgentConfig` (`settings.rs:47-86`), `context_regex` and its adapters and tests, `src-tauri/resources/coding-agents/agents.default.json`, `src/shared/profile-utils.ts`, `session/profile.rs`, `telegram/bridge.rs`, `output_sequence` semantics, `session/warnings.rs`, `src/shared/window-geometry.ts`, `src/resource-monitor/App.tsx`, `src-tauri/src/web/commands.rs`, `src/terminal/styles/terminal.css`.

---

## 6. Required behavior, edge cases and failure behavior

| # | situation | required behavior |
|---|---|---|
| 1 | No enabled watcher in `settings.json` | The tick reads settings and returns **before touching any session**: no `screen_parsers` lock, no allocation |
| 2 | Session with no `agent_id` | Never registered |
| 3 | First tick after registration | Seed the hashes, evaluate nothing, do not count a miss |
| 4 | Frame unchanged (stamp equal) | `Unchanged`: no row clone, no allocation, no evaluation, no event |
| 5 | Terminal resized | Reseed, evaluate nothing; count a miss **only if** something had been evaluated since the last reseed |
| 6 | Screen scrolled by `k` rows | Rows `R-k..R` except the cursor row are evaluated with no stabilization |
| 7 | Row written in place, or the cursor row | Evaluated on the next tick in which its hash is unchanged |
| 8 | Repaint or `clear`: agreement below half the overlap | Count a miss; the differing rows become dirty and are evaluated one tick later, once each. There is no evaluate-everything-now path |
| 9 | Dirty row scrolls off the top | Lost, documented, does **not** count a miss |
| 10 | Statusline inside a scroll region | May be re-evaluated as new on each scrolling tick; suppressed by layer 2 while its text is unchanged. Statusline watchers belong in `state` mode |
| 11 | A logical row spans several physical rows | Joined through `wrapped` before evaluation; the payload carries the joined row |
| 12 | A continuation whose start scrolled off | Skipped, never evaluated as a fragment, **for as long as the line is still wrapping across the top edge**. Once the head is gone entirely the survivor carries no wrap flag and is not distinguishable from a line that genuinely begins at row 0; that residual is debt item 10 and is not detectable with the `vt100` API over a mirror with no scrollback |
| 13 | Pattern does not compile | The watcher is inert for every session, logged **once** per source change through the sticky cache. Other watchers are unaffected |
| 14 | A `watchers` entry does not deserialize | That entry becomes `WatcherEntry::Invalid`, is skipped, is logged once, and is written back verbatim on save. **Every other setting and every other watcher is unaffected** |
| 15 | `commands` entry does not tokenize, or is empty | The whole watcher is skipped and logged once. Never "reaches everything" |
| 16 | `commands: []` | Reaches nobody |
| 17 | Stem that no agent has | Reaches nobody. Not an error. Settings shows "reaches 0 agents" |
| 18 | More than 8 watchers reach one agent | The first 8 in `BTreeMap` key order run; the rest are logged once per resolution change **and** shown per row in Settings as out of budget |
| 19 | `dedupeWindowMs` above 60000 | Clamped to 60000, logged once |
| 20 | Dedupe keys exceed 256 for a `(watcher, session)` | Oldest key evicted. Expired keys pruned once per tick |
| 21 | Per-tick cap exceeded | Count, stop emitting for that key this tick, log once while degraded, set `degraded`. Neither the event nor the buffer receives the overflow |
| 22 | 25 consecutive degraded ticks | That `(watcher, session)` is suspended for 5 s, `degraded` stays true, one log line, then retried |
| 23 | Pattern edited while running | Recompile on the changed source; clear that watcher's state gate, generation and dedupe entries for every session |
| 24 | Watcher deleted or renamed while running | It stops resolving and stops emitting. Its buffered matches stay under the old id and it leaves `activeWatchers`. Renaming behaves as delete plus create |
| 25 | `screen_parsers` mutex poisoned | `Missing`: no reading this tick, no claim about the session |
| 26 | Backend reports no session behind the id | `Gone`: retire immediately |
| 27 | Child exited | Retired on `Gone`, or on the next 5 s liveness probe. The buffer is **kept** |
| 28 | Session destroyed | `purge_session_side_state` retires from the engine, then purges, for `destroyed_ids` only |
| 29 | Session restarted, success or failed finalize | Same helper through `publish_restart_destroyed`. The old id's buffer is purged |
| 30 | Root-agent session retained as exited | The buffer is **kept**; the row is still in the list |
| 31 | Application restarted | Every buffer is empty. The window never promises history across restarts |
| 32 | Window opened with the session already running | It shows whatever the ring holds |
| 33 | Window not open | No event is emitted at all. The ring still records |
| 34 | Window opened during the session's first 200 ms | `warmedUp` is false, so a neutral starting state is shown, never "Configure watchers" |
| 35 | Session dies while the window is open | Rows keep their frozen agent and workgroup labels |
| 36 | Two watchers with identical `pattern` and `mode` | Both run and both emit |
| 37 | A pattern with zero capture groups | Valid. `captures` is `[]` and the UI falls back to the raw row |
| 38 | A logical row longer than 256 bytes | Truncated on a char boundary, `rowTruncated: true` |
| 39 | The same text appearing twice at two positions at two times | Counts twice. Distinguished in the payload by `seq` |
| 40 | State watcher stops matching | Gate cleared, **no event**. An identical re-appearance emits again |
| 41 | A second instance of a state condition appears while the first is visible | The match count rises, the generation advances, **it emits** |
| 42 | State watcher whose capture is volatile (a clock) | Emits every tick by design. `preview_watcher_pattern` reports `capturesVolatile` so this is visible before saving |
| 43 | Button pressed in browser mode | Not reachable: the button does not render outside Tauri |
| 44 | "Configure watchers" pressed with the watchers window in front | The main window is focused first, then the modal opens on the Watchers tab |

**Fail-closed direction throughout:** every ambiguous case under-reports rather than over-reports. The smallest-k tie-break, the hash collision, the missing parser, the untokenizable command, the skipped continuation and the dirty row that scrolls off all lose a detection rather than invent one, which is the direction `context_scrape/rows.rs:107-124` already pins with a test.

---

## 7. Compatibility, safety and cost

### 7.1 Compatibility

- **`settings.json`**: `watchers` and `watchers_geometry` are `#[serde(default)]` with `skip_serializing_if`, so an existing file loads unchanged and a user who configures nothing never sees either key appear. `AppSettings` has no `deny_unknown_fields`, so a file written by a newer build loads in an older one without failing.
- **Downgrade is lossy on the next save, and this is stated rather than promised away.** `AppSettings` has no `#[serde(flatten)]` catch-all and `save_settings_value` (`settings.rs:2646-2653`) serializes the struct, so a build without this feature reads a `watchers` key, ignores it, and **drops it the next time it saves settings**. Revision 1 claimed round-trip safety; that was wrong. The honest statement is: downgrading is safe until the older build writes settings, at which point watcher configuration is lost and must be re-entered.
- **`AgentConfig` gains no field**, so none of the 20-plus construction sites is touched.
- **`PtyBackend` gains a defaulted method**, so the two test fakes (`pty/manager.rs:926`, `:1025`) and any out-of-tree implementor keep compiling.
- **`ContextScraper`, `contextPercent`, `session_context` and `list-peers-lean` are untouched.**
- **`output_sequence` semantics are untouched**, so #955 replay ordering is unaffected.
- **`SubstantiveInputState` behavior is unchanged**; its two inline resets move into one helper called from the same two places.
- No new crate and no new cargo feature. `chrono`, `regex`, `vt100`, `uuid` and `serde` are already declared (`Cargo.toml:9-42`).

### 7.2 Safety

- **ReDoS is not the risk and needs no mitigation.** `regex` 1.x has no backtracking and is linear time by construction; the size limit is the bound that matters, and past it compilation fails rather than allocating (`context_scrape/pattern.rs:27-30`, pinned at `:83-92`).
- **The real risks are unbounded growth and unbounded emission**, and each has a structural bound: the per-tick caps plus suspension (4.6), the 500-entry ring (4.10), the 256-key dedupe bound with per-tick pruning and the 60 s window clamp (4.5), and directed coalesced delivery that emits nothing when the window is closed (4.9).
- **A malformed configuration cannot take the application down.** The per-entry wrapper (4.8) is what turns "the whole settings file is replaced by defaults" into "one watcher is skipped".
- **The engine cannot act.** It holds narrow sinks, no `AppHandle` and no `PtyManager` of its own. The worst a hostile or careless pattern can do is put a wrong row in a window and a wrong number in a counter.
- **`dfa_size_limit` is left at the crate default**, as it is today (`context_scrape/pattern.rs:37-40`). With many active patterns the DFA cache can thrash and degrade to the slower engine. That is a degradation, not a denial of service, and it is bounded by the 8-watcher budget.
- **The window shows raw PTY rows**, which can contain anything the agent printed. They are rendered as text, never as HTML, and truncated to 256 bytes.
- **Disk writes in the worst case: zero.** The engine persists nothing and its logging is gated to log-once. This is a property to keep.

### 7.3 Cost

**`RegexSet` is out of scope.** The reasoning, with the arithmetic:

1. This engine runs patterns over **newly evaluable logical rows**, not over the whole frame. Worst case per session per tick is 8 patterns against 30 rows, 240 regex executions. With a literal prefix `regex` resolves that as a memory scan, roughly 100 ns each, about 24 microseconds against a 200 ms tick: about 0.012 percent of a core per session, about 0.6 percent at 50 sessions, and that is a full screen of new rows every tick.
2. `RegexSet` cannot extract capture groups, so a hit still requires running the individual regex. It only helps the no-match case, which by point 1 is already cheap.
3. It would add a second compiled artifact per resolved watcher set whose invalidation is the cross product of the individual pattern caches.

**Re-entry condition:** revisit if the per-agent budget rises above 8, or the tick drops below 200 ms, or a measured profile shows regex execution above 2 percent of a core at 20 sessions.

**Lock cost, with the honest baseline.**

The `Unchanged` short circuit fires less often than revision 1 assumed: `output_sequence` advances on **every chunk** (`output.rs:160`), including chunks that change no character, so a CLI with a spinner or a seconds counter moves it several times per second. `Unchanged` therefore fires for a genuinely quiet PTY, not for a visually still one. The Phase 0 measurement is taken against a session with a live spinner, not a static screen.

What carries the cost argument instead is 4.2.1. Comparing sustained time held on the hot `PtyManager` mutex at 50 sessions:

| | acquisitions per second | path per acquisition | sustained |
|---|---|---|---|
| today, scraper at 5 s | 10 | registry, ptys, liveness syscall, 30-row clone | about 2.0 ms/s |
| revision 1 design, active sessions | 250 | same path minus the syscall | about 50 ms/s |
| **this design** | **2** (the 25th-tick liveness probe only) | liveness only | **well under 1 ms/s** |

The per-tick frame read no longer touches that mutex at all: it goes straight to the backend `Arc` and takes one `screen_parsers` lock, and that map is per backend (2.1). The engine is therefore strictly better than the status quo on both the acquisition count and the sustained hold of the mutex every terminal write contends for, instead of better at rest and worse under load.

**Measurement, in Phase 0, recorded in the code.** Before the engine is wired, a co-located `#[test]` measures at the default 30x120 grid: the wall time of `get_screen_rows`, of `get_screen_rows_since` on an unchanged frame, and of `get_screen_rows_since` on a changed frame. All three numbers go into the new module's doc comment in the form `context_scrape/mod.rs:22-25` already uses. The test is `#[ignore]`d so it never becomes a flaky gate and stays in the tree so the numbers can be re-taken.

**Acceptance criterion for contention:** the unchanged-frame read performs zero row allocations, enforced by the **type** (`ScreenRowsSince::Unchanged` carries no rows) rather than by a timing assertion, and the tick path takes neither the `PtyManager` mutex nor the `registry` mutex, enforced by a test that asserts the engine holds a backend `Arc` and by the absence of those calls in the tick.

**Memory.** Diff state is 8 bytes per row per session, about 240 bytes, about 12 KB at 50 sessions. Dedupe is at most 256 keys per `(watcher, session)`. History is at most 500 entries per session, about 5.5 MB ceiling at 20 sessions.

---

## 8. Implementation order

The ordering principle: **build and prove the engine on the mode that still has a gate, then remove the gate**, and put the tool that configures the engine in the hands of whoever is testing it before the hardest phase, not after.

### Phase 0: the read seam and the measurement

1. `FrameStamp`, `ScreenFrame`, `ScreenRowsSince` in `pty/watchers/mod.rs`; `SessionIoFanout::get_screen_rows_since` in `pty/output.rs`.
2. Defaulted `PtyBackend::screen_rows_since`, the two backend overrides with their `Missing` and `Gone` split, and `PtyManager::screen_rows_since`.
3. The ignored timing test and the recorded numbers.

### Phase 1: configuration, resolution and geometry

1. `WatcherEntry`, `WatcherConfig`, `WatcherMode`, `WatcherDedupe`, `AppSettings::watchers`, `AppSettings::watchers_geometry`, the two `Default` lines.
2. `set_watchers_geometry`.
3. `command_executable_basename` to `pub(crate)`.
4. Resolution: `settings.agents` crossed with the valid watchers through `reaches`, the 8-watcher budget, the log-once rules, the clamp on `dedupeWindowMs`.
5. `pty/watchers/pattern.rs` with the sticky compile cache.

`watchers_geometry` and its command sit here rather than with the window because they are `settings.rs` changes, and grouping them touches that file once.

### Phase 2: the engine on state mode only

1. `pty/watchers/mod.rs`: thread, runtime, shutdown token, `register_session` resolving the backend `Arc`, `retire_session`, the tick with the settings-first early exit.
2. The 200 ms tick reading through the Phase 0 seam, with the 25th-tick liveness probe.
3. `state` mode: full-frame evaluation with logical-row joining, lowest match, the `(captures, row, generation)` gate.
4. `WatcherMatchPayload`, `WatcherMatchBatch`, the directed `watcher_matches` emission, the `WatcherSink` adapter in `lib.rs`.
5. Registration at `commands/session.rs:2268-2274`, with the comment corrected.
6. The TypeScript types and `onWatcherMatches`.

At the end of Phase 2 the engine is provable end to end with a state watcher and a listener, and it cannot flood anything.

### Phase 3: the diff, occurrence mode, and the Settings editor

1. `pty/watchers/frame.rs`: hashing, `best_shift`, `shift_dirty`, the cursor-row rule, logical-row joining, `possibly_missed_frames`. Written as pure functions over hash slices and unit-tested on their own, which is why the riskiest piece is also the cheapest to test.
2. `occurrence` mode wired to the diff.
3. `pty/watchers/dedupe.rs` with its bounds and pruning.
4. The two per-tick caps, the degraded marker and the suspension rule.
5. `preview_watcher_reach` in its draft-shaped form; the tightened `isWatcherConfig` and the disabled-by-default `newWatcherConfig` in `settings-watchers.ts`; and the Watchers section in `SettingsModal.tsx` including the `resolveSettingsSection` branch, the section-level reach state with its two honesty rules, and the text that distinguishes reach from running.

The editor lands here and not at the end because it is the instrument used to exercise phases 2 and 3: without it the only way to configure a watcher is to hand-edit `settings.json` and restart. Occurrence mode enters here too, because this is the first point at which the gate is gone and the caps are the only thing bounding output.

### Phase 4: history and the snapshot command

1. `pty/watchers/history.rs`, per-session locks, the ring, `seq`, `lastSeq`, `truncated`, the counters, and the published `possibly_missed_frames`, `warmed_up` and `active_watchers`.
2. `get_watcher_activity` and its wrapper.
3. `purge_session_side_state` and its two call sites, with retire-before-purge.

### Phase 5: the window

1. `open_watchers_window` with its creation mutex, the label, the `main.tsx` route, `WatchersScopeState`, `get_watchers_scope`, `watchers_scope_request` and `onWatchersScopeRequest`. The scope state and its command land with the window rather than in Phase 1, because they exist only to make this window's handover durable and have no meaning without it.
2. `src/shared/time-format.ts`.
3. `src/watchers/App.tsx` and its components: table, expandable raw row, scope selector, filters, three empty states plus the warming state, the two loss banners, the footer, the subscribe-then-settle-scope-then-fetch-then-merge startup with its guard, the polling loop, `settingsStore.load()`, the three session listeners.
4. The StatusBar button, gated on `isTauri`.

### Phase 6: the pattern preview

1. `preview_watcher_pattern` with its optional session, its two samples and `spawn_blocking`.
2. The debounced "Test against current session" control with its session selector.

---

## 9. Tests and acceptance criteria

Rust tests are co-located in the module they cover.

### 9.1 Phase 0, the read seam

1. An unchanged frame returns `Unchanged`, and the returned variant carries no rows.
2. One `handle_output` chunk changes `sequence`, and the next call returns `Frame`.
3. A `resize_screen_and_broadcast` that does not change `sequence` still returns `Frame`, because the size is in the stamp. **This is the regression a sequence-only stamp would let through.**
4. An unknown id returns `Missing` from the local backend and `Gone` from the container backend.
5. A poisoned `screen_parsers` returns `Missing` and not `Unchanged`.
6. `get_screen_rows_since` on a changed frame returns the same rows, row for row, as `get_screen_rows`.
7. `wrapped` matches `Screen::row_wrapped` for every row, and `cursor_row` matches `Screen::cursor_position().0`.
8. The default `PtyBackend::screen_rows_since` never returns `Unchanged` and reports `stamp: None`.
9. `PtyManager::screen_rows_since` returns `Gone` for a session with no route.

### 9.2 Phase 1, configuration and resolution

10. `commands` absent reaches every configured agent; `commands: []` reaches none.
11. `commands: ["claude"]` reaches `claude`, `CLAUDE.EXE` and `C:\...\claude-sandbox-runtime\claude.cmd`, and does **not** reach `pi --provider claude`, `cmd /c claude` or `npx claude`.
12. `commands: ["claude"]` does not reach an agent whose command is `claude-phi`. Exact equality, never a prefix.
13. `enabled: false` reaches nobody while keeping its configuration.
14. A `commands` entry that does not tokenize skips the whole watcher and logs once.
15. An agent whose own command does not tokenize is reached by a selectorless watcher and by no watcher with a selector.
16. With 12 watchers reaching one agent, exactly 8 run, they are the first 8 in `BTreeMap` key order, and the other 4 are named in exactly one log line.
17. **A `watchers` map containing one entry with `"mode": "State"`, one with `"commands": "claude"` and one with `"dedupeWindowMs": "2000"` loads successfully: every other setting is intact, every other watcher resolves, and the three bad entries are skipped, logged once and written back verbatim on save.** This is the regression that a plain `BTreeMap<String, WatcherConfig>` would let through, and its failure mode is the whole settings file.
18. A `settings.json` with no `watchers` key round-trips without either new key appearing.
19. A `settings.json` with watchers round-trips byte-stable through save and load, including the absent-against-`[]` distinction for `commands`.
20. `dedupeWindowMs` above 60000 is clamped and logged once.
21. A pattern with zero capture groups **compiles**, which is the difference from `context_scrape::pattern::compile`.
22. A pattern over the size limit fails to compile because of the size limit.
23. A pattern is resolved verbatim, with leading and trailing whitespace intact.

### 9.3 Phase 2, the engine on state mode

24. A session with no `agent_id` is never registered.
25. **With sessions registered but no enabled watcher, the tick takes no `screen_parsers` lock and allocates no rows.**
26. The engine holds a backend `Arc` per session and the tick path calls neither `PtyManager::get_screen_rows` nor `kind_for_session`.
27. A state watcher whose pattern matches emits exactly one match for a value that does not change over 5 ticks.
28. The same watcher emits again when the matched row changes.
29. A state watcher that stops matching emits **nothing**, and emits again when an identical row reappears.
30. **A second identical match appearing while the first is still on screen emits**, because the match count rose and the generation advanced.
31. A screen scroll that leaves the match count unchanged emits nothing.
32. With several matching rows, the **lowest** one wins.
33. A compile failure logs once across 10 ticks.
34. A session that reports `Gone` is retired on that tick; one reporting `SessionOver` liveness is retired on the probe tick.
35. Liveness is probed on tick 25 and not on ticks 1 through 24.
36. No `watcher_matches` event is emitted when the `watchers` window does not exist, and the ring still records.
37. One event carries all of a tick's matches for a session.

### 9.4 Phase 3, the diff, occurrence mode and the editor

38. `best_shift` on two identical frames returns `k == 0` with full agreement.
39. `best_shift` on a frame scrolled by 3 returns `k == 3` and declares exactly the bottom 3 rows new.
40. **`best_shift` on a frame scrolled by 3 whose bottom row also changed in place returns `k == 3`**, and that row lands in the dirty set. This is the case revision 1's slice-equality predicate made unsatisfiable, and it is the normal case for any TUI with a statusline.
41. A frame in which only the statusline row changed returns `k == 0` with agreement `R - 1`, evaluates nothing, marks that row dirty, and does **not** increment `possiblyMissedFrames`.
42. A row that changes on every tick is never evaluated while it keeps changing.
43. `shift_dirty` moves flags by `k` and clears the tail.
44. A repaint with agreement below half increments `possiblyMissedFrames`, evaluates nothing that tick, and evaluates each changed row exactly once on the following stable tick.
45. The cursor row is not evaluated on the tick it arrives by scroll, and is evaluated once, complete, when it stabilizes. A watcher on `Read (.+)` against a row written in two chunks produces **one** event with the complete capture, not two.
46. A logical row spanning two physical rows is matched as one string, and a pattern that only matches across the break does match.
47. A continuation whose start is off the top of the screen is not evaluated.
48. The first tick after registration evaluates nothing and does not increment `possiblyMissedFrames`.
49. A resize before anything has been evaluated reseeds without incrementing `possiblyMissedFrames`; a resize after something has been evaluated does increment it.
50. The same text appearing twice at two positions at two times counts twice and the two events carry different `seq`.
51. The same row shifted up by a scroll is not re-evaluated.
52. `dedupe: "capture"` collapses two differently truncated rows capturing the same path within the window and lets them through outside it; `dedupe: "none"` lets every match through.
53. The dedupe map never exceeds 256 keys per `(watcher, session)`, and expired keys are pruned.
54. A pattern matching every row produces at most 8 events for that watcher and at most 16 for that session in one tick, and sets `degraded`.
55. The overflow reaches **neither** the event **nor** the buffer.
56. 25 consecutive degraded ticks suspend that pair for 5 seconds and then retry.
57. `possiblyMissedFrames` is monotonic.
58a. **A draft of 9 enabled selectorless rows against one agent returns `allocated: true` for the first 8 in id order and `false` for the ninth**, with nothing on disk contributing. The fixture sends the nine rows in an order **different from lexicographic**, so an implementation that honours request order instead of `BTreeMap` key order fails it. This is the regression the row-level signature let through, and its failure mode is the UI telling the user that a watcher holds a slot it does not.
58b. A draft that drops 5 of 9 rows returns `allocated: true` for all 4 that remain, and a row present on disk but absent from the draft contributes nothing to any agent's budget.
58c. **Displacement fixture: `a` disabled plus 8 enabled `b` to `i`, all selectorless.** `a` returns its full `entries` with `allocated: false` everywhere; `b` to `i` return `allocated: true`. **No draft ever yields more allocated rows on one agent than that agent has slots.** This is the case revision 3's first form got wrong, where `a`'s own pass displaced `i` and every row still reported itself in budget.
58d. Reach does not depend on enablement: the `entries` of a row are identical whether that row is enabled or disabled, all else equal.
58e. Still required from revision 2: `commands` absent reaches every agent, `commands: []` reaches none, a selector that does not tokenize skips the whole watcher, and the reported `commandStem` is the agent's. In the two reach-nobody cases **the response row is still present, with `entries: []`**.
58f. **Agents come from the draft:** a draft that changes an agent's `command` from `claude` to `codex` drops that agent from a `["claude"]` watcher's `entries` and adds it to a `["codex"]` one; a draft that removes an agent removes it from every `entries`; a draft that adds one includes it. Each of these is a case where resolving against the saved agent list would have answered about a state the user had already left, and two of the three would have over-reported.
58g. **The request excludes rows the Rust decoder would reject, across the whole numeric contract.** The fixture covers `commands: [1]`, and `dedupeWindowMs` of `-1`, `1.5`, `1e30` and `Number.MAX_SAFE_INTEGER + 2`, plus `Number.MAX_SAFE_INTEGER` itself as the accepted boundary. `typeof n === "number" && n >= 0` must fail this test; `-1` alone does not exercise it. Each rejected row is classified unrecognised, is not sent, consumes no budget slot, and survives a save round trip verbatim.
58h. Frontend, with at least three rows and fake timers, keyed on the **request fingerprint**: an edit to a row's selector fires exactly **one** call carrying every valid row plus the draft agents; adding, deleting and toggling `enabled` each fire exactly one; a `pattern` keystroke fires **none and clears nothing**, because the request is unchanged; changing a row's selector clears the displayed answer immediately and it does not reappear until the answer for that request arrives; **A to B and back to A re-requests A and settles on A, both when A had already answered and when A was still in flight**; a rejected call renders an error rather than the previous answer; and a stale response never overwrites a newer one. The permanent-pending case, cleared with no call ever issued, must fail this test.
58i. Frontend text, all three: an enabled row reads "Reaches N agents" and shows the budget badge for entries with `allocated: false`; a disabled row with a pattern reads "Would reach N agents when enabled" and shows **no** budget badge; a disabled row with an empty pattern reads "Would reach N agents when enabled. Add a pattern to enable it."
58l. A row with an **uncompilable pattern** that is enabled and within budget still reports `allocated: true`, and that is deliberate: allocation is slot assignment and the row's own `preview_watcher_pattern` reports `compiles: false` with its error. The two dimensions are asserted together so nobody later reads `allocated` as a promise of output.
58j. The fixed points of 4.9 points 6 and 7, which are contract and therefore tested even though the editor cannot produce the first two: two draft rows with the same id and different selectors resolve as later-wins and both response rows report that resolution; an empty id sorts first; the response is one row per request row in request order; `entries` is ordered by `agentLabel` with `agentId` as tie-break.
58k. `newWatcherConfig()` returns `enabled: false`, the editor refuses to enable a row whose pattern is empty, and Add followed immediately by Save produces a watcher that the engine does not run. The serde default for an omitted `enabled` in a hand-written file is still `true`.
59. `resolveSettingsSection("watchers")` returns `"watchers"` and not `"general"`.

### 9.5 Phase 4, history and the snapshot

60. `snapshot` does **not** consume: two consecutive calls return the same content.
61. The 501st match drops the oldest and sets `truncated`.
62. `seq` is monotonic per session and is identical in the event and in the ring entry.
63. `limit: Some(n)` returns the n most recent, still ordered oldest first; absent returns everything in the ring.
64. A session with no buffer returns the empty snapshot with `warmedUp: false`, not `None` and not `Err`.
65. A `session_id` that is not a UUID returns `Err`.
66. `activeWatchers` lists the reaching watchers with their `mode` and count 0 before any match.
67. `warmedUp` is false before the first tick and true after it.
68. Destroying a session purges its buffer.
69. **Restarting a session purges the old session's buffer**, through both the success path and the failed-finalize path.
70. **A tick landing between the purge and the engine's retirement does not recreate the entry**, because the helper retires before it purges and creation requires registration.
71. A root-agent session retained as exited **keeps** its buffer.
72. A session whose child exits on its own **keeps** its buffer and stops being sampled.
73. A snapshot of one session does not block the engine writing another.

### 9.6 Phases 5 and 6, frontend

74. The `WatcherMatchPayload` and `WatcherMatchBatch` TypeScript types match the Rust serialization field for field, asserted by a Rust test pinning the exact camelCase JSON.
75. Every payload field is present in the JSON, including empty `captures` and `rowTruncated: false`.
76. `at` parses with `new Date(value)` and `formatClockTime` returns zero-padded 24-hour `HH:MM:SS`.
77. A window that subscribes, buffers, fetches and merges on `(sessionId, seq)` neither duplicates nor loses a match that arrives during the fetch.
78. The StatusBar button renders on a root-agent session and does **not** render when `isTauri` is false.
79a. Opening the window twice does not create a second window and does change the scope through `watchers_scope_request`.
79b. **`open_watchers_window(A)` followed by `open_watchers_window(B)` leaves `get_watchers_scope()` at B, whether or not any listener exists.** This is the property that matters and the one a Rust-side listener cannot certify: `app.listen_any` registers in the Rust registry, which is always durable, so no reordering of a `listen_any` test can model a JavaScript listener that has not been registered yet. The revision 2 test is not wrong, it certifies that the emit is issued, and it stays alongside this one.
79c. **Two concurrent `open_watchers_window` calls, for two different sessions, produce exactly one window, two `Ok`s and no duplicate-label failure; and the final `get_watchers_scope()` agrees with the loser's emitted event.** Counting windows and `Ok`s is not enough: the defect this test exists for is the interleaving that leaves the authoritative state at one session and the last emitted event at the other, which a count-only assertion passes. Revision 3's separate "the state is written before the creation branch returns" test is folded in here, because with one critical section it is no longer an independent property.
79d. Frontend, two orderings of the same race, because they fail differently:
   - Pull blocked: mounted with `?sessionId=A`, **no `get_watcher_activity` is issued at all**; a `watchers_scope_request` for C arrives; the pull then resolves to B; **only C is fetched**.
   - Pull resolved, first fetch blocked: the pull settles on A and `get_watcher_activity(A)` is in flight when a `watchers_scope_request` for B arrives; **B is fetched and committed without waiting for the poll**, and A's result is discarded. This is the hole a startup carve-out left, and it is invisible to the first ordering.
79e. Frontend, content guard, three assertions that fail independently:
   - Two fetches in flight for different scopes resolving out of order leave the table and the snapshots showing the current scope only. Asserting the `<select>` value is not sufficient: the defect is precisely a correct selector over stale rows.
   - **Before** the new fetch resolves, and again when it **rejects**, the previous scope's rows are already gone from the DOM. A test that only inspects the end state after both promises settle passes without the synchronous drop.
   - Two refreshes of the **same** scope resolving out of order leave `lastSeq`, `warmedUp`, `degraded` and the counters from the newer one, never the older.
79f. In "All sessions", creating and destroying an agent session while a refresh is in flight invalidates that refresh, refetches, and does **not** drop rows belonging to the newly created session. A generation keyed on the selected session, rather than on `scopeIds()`, passes every other test here and fails this one.
80. `Agent` and `Workgroup` chips do not render in single-session scope and do render in "All sessions".
81. The four empty and warming states are distinguished from the snapshot alone, with no nullability.
82. The `truncated` banner and the `possiblyMissedFrames` line are separate elements with distinct text.
83. The footer is present in every state.
84. A row containing HTML-looking text renders as text.
85. State rows and occurrence rows are visually distinguishable.
86. `preview_watcher_pattern` with `session_id: None` returns the compile result with `sampled: false`; with a session it returns `matchedRows` against `totalRows`; with a session that cannot be read it returns `sampled: false` and keeps the compile result; a pattern that does not compile returns `compiles: false` with an error rather than failing.
87. A pattern capturing a volatile value reports `capturesVolatile: true`.

### 9.7 Objective acceptance criteria for the issue

- **A1.** A single watcher with no `commands` reaches all 12 agent entries of a real `settings.json`, which is the configuration the issue says cannot be expressed today.
- **A2.** With sessions running and no enabled watcher, `cargo test` passes and the tick takes no `screen_parsers` lock and allocates no rows.
- **A3.** Every existing `context_scrape` and #1088 test passes unmodified.
- **A4.** A state watcher on a persistent row emits once and does not re-emit while the row is unchanged over at least 25 ticks, and emits again when a second instance appears above it.
- **A5.** An occurrence watcher on a scrolling transcript of at least 60 rows delivered in one burst reports matches and, if agreement fell below half, a non-zero `possiblyMissedFrames`, never a silent zero.
- **A6.** With a watcher configured for `claude`, a real Claude session printing a file path longer than the terminal width produces exactly one activation carrying the **complete** path, proving both the wrapped-row join and the cursor-row rule.
- **A7.** The activity window opens from the StatusBar on both a normal and a root-agent session, shows the ring's content for a session that was already running, and shows the best-effort footer. Its "Configure watchers" button focuses the main window and opens Settings on the Watchers tab.
- **A8.** Destroying a session purges its buffer; restarting a session purges the old id's buffer; a session that crashes keeps it.
- **A9.** A `settings.json` whose `watchers` map contains one malformed entry starts the application with every agent and every other watcher intact.
- **A10.** Measured and recorded in the module doc comment: the unchanged-frame read, the changed-frame read, and the existing 200 microsecond figure, taken against a session with a live spinner.
- **A11.** End to end through the real editor, not through the command alone: starting from a `settings.json` with no `watchers` key, adding nine watchers, giving each a compilable pattern, enabling them, and reading the indicator **before** pressing Save shows the ninth as not running for budget; pressing Save then leaves the engine resolving exactly the eight the indicator named, associated with the same rows. The same holds when the draft also edits an agent's `command`, which is the case the saved-agent list would have got wrong. **The indicator never states that a watcher holds a slot it will not hold, and never shows more allocated rows on one agent than that agent has slots.** A tenth watcher with an uncompilable pattern is allocated and inert, which the row's own pattern preview reports and this criterion does not contradict.
- **A12.** Pressing the StatusBar button on session A and then, **while the activity window is still loading**, pressing it on session B leaves the window scoped to B **and showing B's rows**. Neither press produces an error, and only one window exists. The loading barrier is deterministic in the test rather than timing-dependent, and the assertion covers scope, content and window count together, because a window that shows the right session name over the wrong session's activations is the failure this criterion exists to catch.

---

## 10. Declared debt

These are known, bounded and deliberately not fixed in #1171. They are stated here so they are debt and not surprise.

1. **A dirty row that scrolls off the top is lost.** The engine never kept its text. Rare, because in-place writes happen near the cursor and a scroll moves that row up rather than off.
2. **A statusline inside a scroll region may be re-evaluated as new on each scrolling tick.** Bounded by layer 2 while its text is unchanged, and by the fact that statusline watchers belong in `state` mode. Fixing it would need the engine to infer the scroll region, which the mirror does not expose.
3. **Two consecutive identical rows arriving exactly one row apart are both counted**, which is correct, at the cost of item 2. The alternative guard was rejected because edge case 39 requires it.
4. **`possiblyMissedFrames` cannot say how much was missed**, only that agreement fell below half at least once. That is a property of sampling.
5. **A state watcher cannot report that a condition ended.** Correct for an activity log, which is the only consumer here. The day a state watcher drives a live indicator, the payload needs a field and section 4.4 must be reread.
6. **No watcher state is persisted**, so history does not survive an application restart and `list-peers-lean` shows nothing about watchers.
7. **Downgrading loses watcher configuration on the older build's next settings save** (7.1).
8. **The 8-watcher budget is resolved by alphabetical id order.** Surfaced per row in Settings rather than fixed, because any other order would need a user-visible priority field.
9. **8, 16, 256, 500, 25 ticks and 60 s are chosen numbers, not measured ones.** Each has its arithmetic written beside it so it can be redone rather than guessed at again.
10. **A wrapped line whose head has already scrolled off can be evaluated as if it began at row 0.** The survivor carries no wrap flag and is not distinguishable from a line that genuinely begins there. `vt100` exposes `row_wrapped(i)`, "does row i continue **into** row i+1", and nothing that says "row i continues **from** row i-1", and the mirror is created with zero scrollback (`output.rs:112`), so the case is not detectable with the API available. It is one row of thirty, only while a wrapped line is straddling the top edge, and it self-corrects on the next scroll. This is the **one known deviation from the fail-closed direction** of section 6, which is why it is stated here and reserved in edge case 12 rather than left in a module comment.
11. **Enabling a disabled watcher can push an enabled one out of an agent's budget, and nothing forecasts it.** Every answer the preview gives is true, and the displaced row shows its badge as soon as the toggle re-fires the call, but before the toggle no row names what it would displace. Reporting it would mean a `displacedWatcherIds` concept whose only job is to predict a state that becomes visible and correct one click later (4.9, point 4).
12. **The reach preview's cost is unbounded in the total size of the selectors, not in the number of rows.** One pass is O(agents x total selector entries), and nothing caps how many entries a hand-written `commands` list may hold. No cap is added here because the engine already runs the same resolution over the same data every 200 ms (4.8), so any payload large enough to matter costs five times as much per second inside the tick; the bound that would fix this belongs to the engine's resolution cadence, and adding one only to the preview would leave the real cost where it is. The computation runs in `spawn_blocking` so it cannot hold a Tokio worker (4.9, point 5).
13. **`allocated` does not know whether the pattern compiles.** A watcher that is enabled and inside the budget reports `allocated: true` even when its regex fails to compile, in which case it is resolved, inert and logged once (edge case 13). The compile result is on the same row, from `preview_watcher_pattern`, and the two are deliberately not merged (4.9, point 3).
14. **The editor's validity predicate is narrower than `u64`.** A hand-written `dedupeWindowMs` above 2^53 is classified unrecognised, so the row is listed but not offered for editing, even though the engine runs it clamped. JavaScript cannot hold that value exactly, and rounding it silently would be worse than declining to edit it (4.12).

---

## 11. Revision history: what changed and what was not taken

Sections 11.1 to 11.4 record revision 2, against revision 1. Sections 11.5 to 11.7 record revision 3, against revision 2.

### 11.1 The three convergent findings, all accepted as facts

1. **The restart paths orphan the ring buffer.** `execute_destroy_transaction` is not the only production exit from `SessionManager`; there are three (2.4), and the post-commit cleanup that revision 1 cited as precedent is one of two parallel copies. Fixed by `purge_session_side_state` called from both loops (4.10), with tests 69 and 70.
2. **The alignment predicate was unsatisfiable.** Slice equality of the overlap makes the in-place branch unreachable, and with any repainting statusline every tick would have fallen through to a full re-render, permanently lighting the sampling-loss line and leaving the TTL layer as the only contention. Fixed by best-shift alignment with an agreement threshold, and by removing the evaluate-everything-now path entirely (4.3), with tests 40, 41 and 44.
3. **The payload had no identity.** With `at` being the tick's instant and identical rows required to count twice, snapshot and stream could not be merged. Fixed by `seq` (4.9), with test 77.

### 11.2 Other findings accepted

Directed coalesced delivery and no emission with the window closed (4.9); tolerant per-entry deserialization of the `watchers` map (4.8); the state-mode generation counter (4.4); retire-before-purge and registration-gated creation (4.10); bounded and pruned dedupe with a clamped window (4.5); degraded suspension (4.6); the backend `Arc` resolved at registration (4.2.1); `stamp: Option<FrameStamp>` so the defaulted trait method is implementable (4.2); the `Gone` variant so the container backend keeps its documented oracle (4.2); the cursor-row rule (4.3); logical-row joining for wrapped lines (4.3.1); `warmedUp` (4.9); per-session history locks and explicit limits (4.10, 4.12); `preview_watcher_pattern` made async with an optional session, two samples and `capturesVolatile` (4.9); `preview_watcher_reach` so no stem rule is ported to TypeScript (4.9); the resize that no longer counts as a miss (4.3); the settings-first early exit replacing a mechanism that did not exist (4.8); `resolveSettingsSection` (4.12); the two-state `commands` control (4.12); watcher id lifecycle (4.8); `onWatchersScopeRequest` promoted from prose to contract (4.9); the button hidden outside Tauri (4.12); the dedicated geometry command (4.12); `formatClockTime` (4.12); `pre-wrap` and no new CSS (4.12); `settingsStore.load()` and the three session listeners (4.12); snapshot polling with a written cadence (4.12); the Settings editor moved to Phase 3 and `watchers_geometry` to Phase 1 (8).

Factual corrections: there are two screen-mirror maps, not one (2.1); the existing read path is four locks deep, not three (2.2); `chrono` is at `Cargo.toml:18` (4.11); `formatTimestamp` is neither exported nor `HH:MM:SS` (2.5); `.workgroup-task-text` wraps rather than scrolls (2.5); the registration comment must be updated (4.10); the downgrade claim was wrong (7.1).

### 11.3 Findings deliberately not taken

1. **Dropping the one-tick stabilization for in-place rows that are not the cursor row.** The cursor-row rule alone fixes the defect; also removing stabilization elsewhere would let a chunk boundary land mid-repaint and emit a half-drawn row. Keeping one uniform rule costs 200 ms of latency on repaint-driven matches and removes a class of failure.
2. **A guard that skips a scrolled-in row identical to what was previously at that position.** It would remove debt item 2 but violate edge case 39. The requirement wins.
3. **Adding `watchers` to the protected-field list in `commands/config.rs`.** The dedicated `set_watchers_geometry` command achieves the same protection without making the list six long and without leaving the new window as a whole-object writer.
4. **Making `mode` default rather than required.** With per-entry tolerance the failure is one skipped watcher, which is the right outcome; a defaulted mode would silently run a watcher the user did not describe.

### 11.4 Recorded, out of scope

`"resources"` is absent from `resolveSettingsSection` (`SettingsModal.tsx:105-110`), so the Resource Monitor's Settings button (`src/resource-monitor/App.tsx:422`) has landed on General since it shipped. Unrelated to #1171 and not fixed here.

### 11.5 Revision 3: why revision 2's certification was invalidated

Revision 2 was implemented and reviewed. Nine of the review's findings were code defects against a plan that was right. **Two were the plan being wrong**, and both are the same kind of wrong: a contract fixed in section 4.9 that cannot carry the user interface fixed in section 4.12.

1. **`preview_watcher_reach` could not compute the budget of the draft.** `in_budget` is a property of the whole watcher set, and revision 2 exposed it through a command that receives one row. A row-level command can answer it only by inventing what the rest of the set is, and the only set it has is the saved one, which is not the one the user is editing: with an empty saved map, nine rows added before Save all report `inBudget: true` and eight then run. The decisive argument is not the arithmetic but the direction: this **over-reports**, and section 6 pins under-reporting everywhere else, so revision 2 broke its own stated invariant on the one surface where the user decides to save. A second symptom of the same root cause was that the signature carried no `enabled` although `reaches` depends on it, leaving that choice to the implementation. Fixed by the draft-shaped command and its seven points of fixed semantics (4.9), with tests 58a to 58k. The field is now called `running`; 11.8 explains why the first replacement was itself replaced.
2. **The re-scope event was not durable during the window's creation.** The label exists when `build()` returns; the JavaScript listener exists only after the bundle loads and an IPC round trip completes; Tauri queues nothing for a listener that does not exist and `emit_to` returns `Ok` regardless. Revision 2's contract branched on "the window exists" and had no answer for the state between created and listening, so a second open during the load dropped the user's order in silence. Two concurrent opens could also both reach `build()`. Fixed by writing an authoritative scope before every emit and pulling it after the subscribe, plus a creation mutex (4.12), with tests 79a to 79e.

Absorbed in the same pass, because a certification is invalidated once: the `vt100` residual behind edge case 12 now has a reservation in section 6 and debt item 10, instead of living only in a module comment.

**What revision 3 does not touch**, stated so the cost of re-certification is not read as wider than it is: the engine, the read seam, the frame diff, both modes, the three dedupe layers, the caps and suspension, `watcher_matches`, `get_watcher_activity`, the history buffer and its purge helper, `preview_watcher_pattern`, the settings schema, and every test from 1 to 57 and 60 to 78. The amendment lives entirely in the configuration surface and the window-opening surface.

### 11.6 Considered in revision 3 and deliberately not taken

1. **Narrowing the budget indicator to "as currently saved" and keeping the row-level command.** The command already substitutes the edited row, so its answer already speaks about the state after Save; making the narrowed reading honest would mean suppressing the indicator whenever the draft differs from the saved map, which is exactly when it exists to be read.
2. **Computing the budget in TypeScript from per-row reach sets.** It duplicates the key order and the number 8 in a second language, it diverges from `Ord for String` for ids outside the id pattern that a hand-written `settings.json` can still contain, and it needs the same lift of reach state out of the row component that the draft-shaped command needs, so it buys nothing.
3. **`on_page_load(Finished)` as the readiness signal.** No new IPC, but it fires before listener registration completes, so it narrows the race instead of closing it.
4. **Treating the window's first `get_watcher_activity` call as an implicit readiness signal.** Also no new IPC, but it would couple a command's meaning to a frontend ordering detail that nothing enforces.
5. **Making the scope pull replace the query parameter.** The parameter stays as the initial value so the first paint has a scope without waiting for a round trip; the pull is authoritative, not exclusive.

Revision 3's first form also rejected **carrying the draft agent list**, on the grounds that its failure under-reports. That was wrong and is reversed in 11.8: two of the three agent edits over-report.

### 11.7 Sequencing noted for implementation

The reach rule and the birth state of a new row are one decision, not two, and 11.8 settles both: a new row is born disabled (4.12), and reach is reported for a disabled row while running is not (4.9, point 3). Revision 3's first form left the birth state conditional on a separate review finding, which gave the implementer no requirement and no test.

### 11.8 Revision 3, round 2: what the enrich changed

Two reviewers took the delta. Nine findings were integrated. Two of them replaced the delta's own core rule rather than patching it.

1. **The agent draft now travels, and debt item 11 is gone.** Round 1 of revision 3 claimed the agent half could stay on disk because its failure under-reports. That claim was false: the modal edits agents and watchers in one store and saves them together (`SettingsModal.tsx:986-1030`), and of the three agent edits only *adding* under-reports. Deleting an agent and changing an agent's `command` both leave the indicator naming an agent a watcher will not reach, which is the same false-positive class the whole revision exists to remove. The payload argument did not justify a positively false answer.
2. **The per-row counterfactual budget is replaced by two passes over one draft.** The delta ran one forced-enabled pass per row, which made each row's answer true in isolation and the *set* of answers incoherent: with one disabled row and eight enabled ones, all nine reported themselves in budget, because the disabled row's own pass silently displaced the eighth. Reach and slot assignment are now separate questions with separate passes, reach from an all-enabled pass and assignment from the real one, so no counterfactual budget is computed and no set of answers can disagree with itself. `in_budget` was renamed `running` here and then narrowed to `allocated` in round 3 (11.9).
3. **That redesign also removes the complexity objection.** N passes of an O(N x A) resolver was O(N squared x A) over an input with no bound, and it ran under the settings read guard. Two passes is linear, and after point 1 the command reads no settings at all, so it holds no lock. No cap on the number of watchers was added, because with linear cost and no lock there is nothing left for a cap to protect.
4. **The editor's validity predicate becomes contract.** "Unrecognised rows are not sent" was not implementable against a predicate that accepts `commands: [1]` and a typed `dedupeWindowMs: -1` while `serde` rejects both. Such a row would be counted in the budget and then skipped by the engine.
5. **A second guard, on content.** Both reviewers found it independently and one stated plainly that a frozen plan with a single guard would leave no room to invent the second. The pull guard decides which scope is adopted; it says nothing about which rows are painted, and a refresh for the old scope returning last repaints it under the new scope's selector. One scope generation, incremented on every transition, captured by every fetch. Round 3 replaced that with one monotonic request counter keyed on `scopeIds()`, for the reasons in 11.9 point 5; the current contract is the one in 4.12.
6. **One critical section in `open_watchers_window`.** The state write and the create-or-emit branches were two regions, which permits an interleaving that leaves the authoritative scope at one session and the last emitted event at the other. The window count and the two `Ok`s that the round-1 test asserted both pass through that interleaving.
7. **The birth state of a new row is decided, not conditional** (11.7).
8. **The reach call fires on a changed draft, and a changed draft clears the displayed answer.** Every edit replaces the whole row object, so the field-level dependency does not by itself prevent a `pattern` keystroke from firing the call; and a commit-time guard stops a stale answer from being written without stopping it from being read, which matters because this indicator gates a Save. Round 3 found that those two halves contradicted each other and replaced them with the fingerprint rule in 4.12; see 11.9 point 2.
9. **The handler count is six**, verified against `lib.rs:2548-2550`, `:2587`, `:2593`. Revision 2 said seven with five registered, and round 1 of revision 3 incremented the wrong number.

Not taken, with reasons: a `displacedWatcherIds` field (debt item 11), a contractual cap on the number of watchers (point 3), and rejecting an empty pattern in the Rust compiler rather than in the editor (4.12).

### 11.9 Revision 3, round 3: what the enrich changed, and what is certified

Both reviewers confirmed the core of round 2: the two passes are coherent, `B.allocated` implies `A.entries`, enablement does not change `entries`, the agent draft removes the fail-open in all three directions, one critical section closes the open-window interleaving, and the handler count is six. Seven further findings were taken.

1. **`running` is renamed `allocated` and its promise is narrowed.** The pattern does not travel, so the field cannot mean "will produce output": a watcher whose regex fails to compile is allocated a slot and is inert. The name now says what the two passes can establish, and compilability stays where it already was answered, on the row's own pattern preview. Carrying the pattern to merge the two dimensions was rejected: it would inflate every debounced payload to restate an answer already on screen.
2. **The idempotence guard and the synchronous invalidation contradicted each other and are replaced by one fingerprint rule.** As written, a `pattern` keystroke changed the draft but not the request, so the answer was cleared and no call was ever issued: pending forever. Keying both the render and the in-flight state on the request fingerprint removes the contradiction and also fixes A to B to A, which the previous pair got wrong in both the answered and the in-flight case. No answer cache is kept; a returning shape is re-requested.
3. **The startup carve-out is removed.** Letting the listener set scope without fetching during mount left an event arriving *during* the first fetch with its predecessor discarded and no fetch of its own, so the window sat correctly scoped and empty until the poll. One uniform rule replaces a rule plus an exception.
4. **Scope invalidation now drops painted content synchronously.** A commit-time check stops a stale result from being written; it does not remove rows that are already on screen, which stay for the whole round trip and forever if the new fetch fails.
5. **The scope generation becomes one monotonic request counter, and invalidation keys on `scopeIds()`.** Two independent findings converged here. A generation keyed on the selected session misses "All sessions", where the three session listeners rewrite the derived set without touching the selection, and it misses two refreshes of the same scope racing, where the older poll response commits older counters over newer ones. Stating the rule over `scopeIds()` and over requests is both wider and shorter than the enumeration of callers it replaces, which had been short by one in every version so far.
6. **The numeric contract is written out, and "exact mirror of the decoder" is corrected to "mirror of what the serializer emits".** The naive fix, `typeof n === "number" && n >= 0`, still admits `1.5` and `1e30`. The decoder also accepts absences the predicate requires present, and being stricter there is the safe direction and practically unreachable, so relaxing it would weaken the guard for nothing.
7. **The cost claim is withdrawn and restated in the right variable.** The work is O(agents x total selector entries), not O(rows x agents); `commands` is unbounded, so "a few milliseconds, nothing to protect" priced only the short-selector case. The defense is now that the engine already runs the same resolution over the same data five times a second, and the computation moves into `spawn_blocking`. The residual is debt item 12.

Also taken: the disabled row with an empty pattern gets its own sentence, because "when enabled" alone offers a condition the editor refuses to let the user meet, and it is the first state anyone sees after Add Watcher.

Two disagreements are recorded rather than resolved by agreement: no contractual cap on watchers or selector entries (debt item 12 explains where the real bound belongs), and no `displacedWatcherIds` (debt item 11). In both the finding was accepted and the proposed remedy was not.
