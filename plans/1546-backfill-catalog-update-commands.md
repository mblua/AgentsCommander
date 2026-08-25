# Plan #1546: Backfill `updateCommands` for coding-agent catalogs seeded before the update-command era

Author: ac-architect-v3, workgroup wg-13-ac-dev-team-v3. Lite triage: no new abstractions, no new dependencies, no schema change, no API change, no version bump. Single-module Rust change plus one data file, the FE fallback mirror and its test (sole implementer ac-dev-rust-v3), and two docs.

Status: READY_FOR_IMPLEMENTATION

Revision: round 2 (2026-08-25) — combined verdicts applied: dev CHANGES_REQUIRED (FE drift-guard mirror gap → §4.5/§5.6/§6/§9/§12, sole implementer ac-dev-rust-v3) + grinch PLAN_APPROVED observations (1 mixed-duplicate winner → §5.3.7/§7/§10; 2 first-prompt surface → §5.5/§7/§10). Round-1 digest A1C065D3C380A47F4AAF3C9C42E013BD5FBF15E2F6ED8B78B03ECCA07D8AF0D3 superseded.

Issue: [mblua/AgentsCommander#1546](https://github.com/mblua/AgentsCommander/issues/1546), "feat: backfill `updateCommands` for coding-agent catalogs seeded before #1325" (OPEN).

Objective: catalogs seeded before #1325 (like this installation's `D:\0_repos\AgentsCommander_iac\.ac\coding-agents\agents.json`) carry no `updateCommands` for any agent, so the #1327 startup auto-update pass silently no-ops even when `agentAutoUpdateByCommand` says `true`. The fix backfills empty `updateCommands` IN MEMORY at catalog read time from the embedded default (never writing the user-owned file), and ships verified update commands for `hermes`, `opencode`, and `agy` in the embedded default. `cursor` remains without an update command by design (documented).

---

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `feature/1546-backfill-catalog-update-commands` (created from `main`; `main` == `origin/main`).

At authoring time (2026-08-25 UTC) the committed `HEAD` of the branch is `d64e625055906eecf2eb46ea513112181a34eaa2` and `git status --porcelain` is empty. Codebase Memory gate: `ready` (project `D-0_repos-AgentsCommander_iac-.ac-wg-13-ac-dev-team-v3-repo-AgentsCommander`, 24552 nodes / 132055 edges, index at the same SHA).

Root `.gitignore` line 11 ignores `/plans/`, so the implementation must force-add this exact plan file with `git add -f plans/1546-backfill-catalog-update-commands.md`. Do not remove or weaken the `plans/` ignore rule.

The implementers must repeat the authority ritual: fetch `origin/main`, and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

## 2. Issue and objective

The startup auto-update pass (#1327, `agent_update.rs`) builds its plan from `updateCommands` in the on-disk coding-agent catalog. Catalogs seeded BEFORE #1325 shipped the update-command field (this installation's catalog was seeded 2026-08-18 as a verbatim copy of the 2026-08-08 legacy catalog and has NO `updateCommands` for any of its six entries). `build_update_plan` skips every command whose sequence is empty, so the plan is empty: no prompt, no update, no `[agent-update]` log lines — although `agentAutoUpdateByCommand` says `true` for pi/claude/codex/opencode on this machine. The catalog is user-owned after the first seed (§14.1 seed-once); AC never rewrites it; the embedded default (`agents.default.json`) does carry commands for claude/pi/codex.

Required outcomes:

- **(A)** Every catalog read (UI `get_coding_agent_catalog`, startup update flow `run_startup_updates`, CLI `coding-agent catalog`) presents `updateCommands` for each entry whose `command` has a non-empty sequence in the embedded default — without writing a single byte to disk.
- **(B)** User-authored non-empty sequences always win; only EMPTY sequences are backfilled.
- **(C)** The embedded default ships verified update commands for `hermes`, `opencode`, and `agy` (antigravity); `cursor` (command `agent`) ships none, documented as intentional.
- **(D)** The update pass on this machine goes live: `[agent-update] running 'pi update' ...`-style info lines appear at next startup for the commands the user answered `true` to.
- **(E)** The FE fallback mirror `FALLBACK_CODING_AGENTS` (`src/shared/agent-presets.ts`) and its drift test stay truthful with the embedded default — the #769 guard halves cannot silently diverge.

## 3. Evidence (measured at d64e6250, not predicted)

- Disk catalog `D:\0_repos\AgentsCommander_iac\.ac\coding-agents\agents.json`: `schemaVersion 1`, entries claude/codex/hermes/cursor/pi/opencode, **zero** `updateCommands` keys (direct read).
- `src-tauri/src/agent_update.rs:252` `build_update_plan` binding rule 1: a command is prompted/updated only when `catalog.iter().find(|e| e.command == entry.command && !e.update_commands.is_empty())` yields a sequence (`:271-273`); otherwise `continue`. Empty plan → `log::debug!("[agent-update] nothing to prompt or update; skipping")` (`:621`) — hence the absent `[agent-update]` info lines despite `settings.rs:566` `agent_auto_update_by_command` being `{pi:true, claude:true, codex:true, opencode:true}` on this machine.
- `run_startup_updates` (`agent_update.rs:592`) loads the catalog via `load_catalog_for_settings(&settings)` (`:602`); `get_coding_agent_catalog` (`commands/config.rs:506-510`) and the CLI (`cli/coding_agent.rs:247,270`) use the same loader. Codebase-Memory call graph confirms `load_catalog` (`config/coding_agents_catalog.rs:239`) is the SINGLE parse of the on-disk manifest; the embedded-default fallbacks (`validated_embedded_default`, `:223`) already carry whatever the default ships.
- `src-tauri/resources/coding-agents/agents.default.json` (embedded via `include_str!` at `coding_agents_catalog.rs:57-58`): `updateCommands` present for claude (`["claude --update"]`, line 15), codex (`["codex update"]`, line 28), pi (`["pi update"]`, line 62); ABSENT for hermes (entry lines 31-40), cursor (command `agent`, lines 42-51), opencode (entry lines 65-75); antigravity ships explicit `[]` (line 86).
- Binary verification on this machine: opencode 1.18.13 exposes `opencode upgrade`; hermes v0.17.0 exposes `hermes update --yes` (`--yes` required because the update runner closes stdin and a prompt would hang the step); agy 1.1.19 exposes `agy update`. Cursor CLI has no documented update subcommand (self-updates with its desktop app).
- Seed path `ensure_seeded` (`coding_agents_catalog.rs:348`) copies a legacy catalog VERBATIM when present (seed-once, §14.1); AC never rewrites a present catalog (G3 corrupt-preserve / user-owned) — so the repair must be read-time and in-memory, never a disk migration.
- Existing tests that must remain green and double as coverage: `definition_defaults_update_commands_empty_auto_update_false_when_absent` (`:1734`, command `old` — no embedded match → stays empty), `load_catalog_for_settings_primary_wins_and_self_heals` (`:1591`, command `custom`), `embedded_default_parses_with_seven_agents_in_order` (`:1061`) and `embedded_default_matches_current_presets_exactly` (`:1078`, asserts preset fields but NOT `update_commands`).
- FE mirror state (verified at d64e6250): `src/shared/agent-presets.ts:14-15` comment "#1318/#1325 - mirror of the embedded default: claude, pi, and codex ship the update command"; hermes `updateCommands: []` (`:42`), opencode `[]` (`:81`), antigravity `[]` (`:96`) under a "#1482 — ... no verified upstream update command" comment (`:94-95`); `src/shared/agent-presets.test.ts:60` asserts the else-branch `toEqual([])` for hermes/opencode/antigravity/cursor. `EXPECTED_BUILTINS` (`agent-presets.test.ts:8-19`) pins only key/label/description/color/command/instructionsFilename and the Rust preset guards assert no `update_commands`, so neither drift guard breaks when the JSON gains commands — the mirror must keep matching the fields they DO assert. `definitionToSeed` (`agent-presets.ts:100-113`) drops `updateCommands`, so the mirror is functionally inert for seeding.

## 4. Scope

### In scope

1. **In-memory backfill in `load_catalog`** (the parsed on-disk path only), applied AFTER `validate_and_filter`: every entry whose `update_commands` is EMPTY gets the sequence of the FIRST embedded-default entry with the same `command` and a non-empty sequence. User non-empty sequences are never overwritten. NOTHING is written to disk (preserves seed-once / user-owned / G3 corrupt-preserve).
2. **`src-tauri/resources/coding-agents/agents.default.json`**: add `updateCommands` for hermes (`["hermes update --yes"]`), opencode (`["opencode upgrade"]`), antigravity (`["agy update"]`). Cursor stays without one.
3. **Tests** in `config/coding_agents_catalog.rs`: rename/extend the drift guard `embedded_default_ships_claude_pi_and_codex_update_commands` (`:1779`) to the new shipping set; add unit tests for the backfill (below, §5.3).
4. **Docs**: `docs/integrations/coding-agents.md` (the "Where `updateCommands` lives" paragraph, line 88) and a troubleshooting note in `docs/features/agent-auto-update.md`.
5. **FE mirror sync** (resolves the dev gap: the #769 drift-guard FE half must not assert stale facts): `src/shared/agent-presets.ts` (`FALLBACK_CODING_AGENTS` hermes/opencode/antigravity rows + the #1318/#1325 and #1482 comments) and `src/shared/agent-presets.test.ts` (the "#1318/#1325" test). Sole implementer: **ac-dev-rust-v3** (authorized; byte-exact mechanical edits; NOT a cross-owner change; §12 atomicity preserved).

### Out of scope (explicitly)

- NO change to `agent_update.rs`, `build_update_plan` semantics, `settings.rs`, `commands/config.rs`, IPC, the CLI surface, or any frontend behavior: NO other `src/` files, NO components, NO IPC payloads, NO types, NO `definitionToSeed` behavior, NO `agent-presets.ts` change beyond the mirror rows/comments, NO `agent-presets.test.ts` change beyond the "#1318/#1325" expectations.
- NO disk migration/repair of existing catalogs, NO re-seed, NO new write path, NO schemaVersion change, NO new dependencies, NO version bump, NO changelog entry requirement.
- NO `autoUpdate` field changes anywhere.
- NO update command for cursor (deliberate; documented in docs).

## 5. Decided solution (exact symbols)

### 5.1 New helper in `src-tauri/src/config/coding_agents_catalog.rs`

Place immediately after `validated_embedded_default` (`:223`):

```rust
/// #1546 - in-memory backfill: for every entry whose `update_commands` is
/// EMPTY, copy the sequence from the FIRST embedded-default entry with the
/// same `command` and a non-empty sequence. Matching by `command` (not key),
/// mirroring `build_update_plan`'s command-keyed binding rule. User-authored
/// non-empty sequences ALWAYS win (never overwritten). Entries with no
/// embedded match (custom commands, cursor's `agent`) stay empty. Never
/// writes to disk: the catalog is user-owned after the first seed (G3).
fn backfill_update_commands_from_embedded_default(
    agents: Vec<CodingAgentDefinition>,
) -> Vec<CodingAgentDefinition> {
    let defaults = validated_embedded_default();
    agents
        .into_iter()
        .map(|mut def| {
            if !def.update_commands.is_empty() {
                return def;
            }
            if let Some(src) = defaults
                .iter()
                .find(|d| d.command == def.command && !d.update_commands.is_empty())
            {
                def.update_commands = src.update_commands.clone();
            }
            def
        })
        .collect()
}
```

### 5.2 Wire into `load_catalog` (`:239`)

Change the parsed-manifest arm at `:266` from

```rust
Ok(catalog) => validate_and_filter(catalog.agents, &path.display().to_string()),
```

to

```rust
Ok(catalog) => backfill_update_commands_from_embedded_default(validate_and_filter(
    catalog.agents,
    &path.display().to_string(),
)),
```

The missing/unparseable arms keep returning `validated_embedded_default()` UNCHANGED: backfill there is a no-op by construction (the embedded default itself is the source, and `cursor`/`agent` finds no non-empty source), so wrapping them would be dead code. Extend the `load_catalog` doc comment (contract §14.2) with one sentence: on the parsed path, entries with empty `updateCommands` are backfilled in memory from the embedded default (first entry with the same `command` and a non-empty sequence); user sequences win; nothing is written to disk.

### 5.3 Tests in `src-tauri/src/config/coding_agents_catalog.rs` (module `tests`)

1. **Rename** `embedded_default_ships_claude_pi_and_codex_update_commands` (`:1779`) → `embedded_default_ships_update_commands_for_all_but_cursor` and update assertions to the full set: claude `["claude --update"]`, codex `["codex update"]`, pi `["pi update"]`, hermes `["hermes update --yes"]`, opencode `["opencode upgrade"]`, antigravity `["agy update"]`, cursor EMPTY; every entry still `auto_update == false`.
2. New `load_catalog_backfills_empty_update_commands_from_embedded_default`: write a manifest with 6 entries, all commands matching embedded defaults (e.g. claude, pi, opencode) but NO `updateCommands`; `load_catalog` returns each entry with the embedded sequence; the manifest file bytes are UNCHANGED after the load (no write).
3. New `load_catalog_never_overwrites_user_update_commands`: manifest entry claude with `["claude --custom"]` → stays `["claude --custom"]` after `load_catalog`; an entry with a one-element custom sequence is untouched.
4. New `load_catalog_backfill_no_embedded_match_leaves_entry_intact`: manifest entry with command `bob` (no embedded match) and empty/missing `updateCommands` → stays empty.
5. New `load_catalog_backfill_matches_by_command_for_duplicate_commands`: manifest with TWO entries whose command is `pi` under different keys, both empty → BOTH get `["pi update"]` (per-entry backfill, consistent with `build_update_plan`'s command keying).
6. Existing `definition_defaults_update_commands_empty_auto_update_false_when_absent` (`:1734`) and `load_catalog_for_settings_primary_wins_and_self_heals` (`:1591`) must stay green UNCHANGED — they already cover the no-match and embedded-fallback cases.
7. New `load_catalog_backfill_mixed_duplicates_first_empty_second_custom` (grinch obs 1): manifest with TWO entries whose command is `pi` — the FIRST with empty/missing `updateCommands`, the SECOND with a custom `["pi --custom"]`. After `load_catalog`: the first carries the backfilled `["pi update"]`, the second keeps `["pi --custom"]` UNTOUCHED (data-level never overwritten). Register the winner: `build_update_plan`'s first-non-empty `find` (`agent_update.rs:271-273`) selects the FIRST entry's sequence for command `pi` — the BACKFILLED `["pi update"]` wins over the custom one; before the backfill, the find skipped the empty first entry and used the custom sequence. This is a consequence of the pre-existing command-keyed first-wins rule, not a new mechanism.

### 5.4 `src-tauri/resources/coding-agents/agents.default.json`

Keep formatting identical to existing entries (2-space indent, single-line arrays). Add after the hermes entry's last field `"removable": true` (line 39, add the trailing comma): `"updateCommands": ["hermes update --yes"]`; after the opencode entry's `"removable": true` (line 74, add the trailing comma): `"updateCommands": ["opencode upgrade"]`; REPLACE line 86 `"updateCommands": []` of antigravity with `"updateCommands": ["agy update"]`. Do not reorder entries, do not touch claude/codex/pi/cursor.

### 5.5 Docs

- `docs/integrations/coding-agents.md`, "Where `updateCommands` lives." paragraph (line 88): add 2-3 sentences — catalogs seeded before the update-command era lack `updateCommands`; AC backfills them IN MEMORY at read time from the built-in default, matching by `command`, and never rewrites the user-owned file; commands you author yourself always win. State that cursor intentionally ships no update command (it self-updates with its app).
- `docs/features/agent-auto-update.md`, Troubleshooting section (starts line 45): add one entry — "My agents never update although I answered Yes." Cause: the catalog entry has no `updateCommands` (catalog seeded before they shipped; cursor by design). Current AC backfills from the built-in defaults at read time; to force a command, edit `agents.json` directly (the CLI only exposes the catalog read-only). The preference control remains `agentAutoUpdateByCommand`. Add one sentence right after it (grinch obs 2): with the new shipping commands, users who registered hermes/opencode/agy and were never asked may see ONE first prompt at the next startup (default No) — answering it is how the per-command preference is set.

### 5.6 FE mirror sync — `src/shared/agent-presets.ts` + `src/shared/agent-presets.test.ts`

Byte-exact edits, sole implementer ac-dev-rust-v3:

- `src/shared/agent-presets.ts`:
  - hermes row, line 42: `updateCommands: [],` → `updateCommands: ["hermes update --yes"],`
  - opencode row, line 81: `updateCommands: [],` → `updateCommands: ["opencode upgrade"],`
  - antigravity row: replace the comment at lines 94-95 (`// #1482 — mirror of the embedded default: no verified upstream update` / `// command, so Antigravity ships none (autoUpdate stays false).`) with the single line `// #1482/#1546 - mirror of the embedded default: Antigravity ships the verified 'agy update' command (autoUpdate stays false).`; line 96 `updateCommands: [],` → `updateCommands: ["agy update"],`
  - claude-row comment at lines 14-15 (`// #1318/#1325 - mirror of the embedded default: claude, pi, and codex` / `// ship the update command; every entry defaults autoUpdate to false.`) → `// #1318/#1325/#1546 - mirror of the embedded default: claude, pi, codex,` / `// hermes, opencode, and antigravity ship the update command; cursor ships` / `// none; every entry defaults autoUpdate to false.`
  - cursor row (line 55) stays `updateCommands: [],`.
- `src/shared/agent-presets.test.ts`, "#1318/#1325" test (line 60): rename to "#1318/#1325/#1546: claude, pi, codex, hermes, opencode, and antigravity ship update commands; cursor ships none; every entry defaults autoUpdate off" and extend the chain: hermes → `["hermes update --yes"]`, opencode → `["opencode upgrade"]`, antigravity → `["agy update"]`; the else branch then covers cursor alone (`[]`).

Verification (pre-checked, must stay green): `EXPECTED_BUILTINS` (`agent-presets.test.ts:8-19`) and the Rust `embedded_default_matches_current_presets_exactly` (`:1078`) assert no `update_commands`, so neither drift guard breaks; the mirror keeps matching every field they DO assert.

## 6. Affected surfaces, exhaustively

- `src-tauri/src/config/coding_agents_catalog.rs` — new private fn (§5.1), one-line wire (§5.2), doc-comment sentence, tests (§5.3).
- `src-tauri/resources/coding-agents/agents.default.json` — three additions/one replacement (§5.4).
- `src/shared/agent-presets.ts`, `src/shared/agent-presets.test.ts` — FE mirror sync (§5.6; sole implementer ac-dev-rust-v3).
- `docs/integrations/coding-agents.md`, `docs/features/agent-auto-update.md` (§5.5).
- `plans/1546-backfill-catalog-update-commands.md` — this file (force-add).

NOT touched (must remain byte-identical in the PR): `src-tauri/src/agent_update.rs`, `src-tauri/src/config/settings.rs`, `src-tauri/src/commands/config.rs`, `src-tauri/src/cli/coding_agent.rs`, `src/sidebar/*` and every `src/shared/*` file EXCEPT `agent-presets.ts`/`agent-presets.test.ts`, `src-tauri/module-arcs.txt`, Cargo.toml/package.json.

## 7. Required behavior, edge cases, failure behavior

- **Per-read, in-memory, idempotent**: every `load_catalog` call re-derives the same result from the same two inputs; no state, no writes, no caching.
- **User sequences always win**: any non-empty `updateCommands` (even one element) is never touched. Only EMPTY (absent or explicit `[]`) is backfilled.
- **Explicit `[]` vs absent is indistinguishable** (serde default) — both are backfilled. This is an accepted tradeoff: `updateCommands` is a CAPABILITY, not a preference; the user's preference surface is `agentAutoUpdateByCommand` (false = never update, never ask), which is untouched. Documented in §5.5 docs.
- **Matching by `command`, first embedded match wins** (`find` semantics; embedded JSON order). Duplicate commands in the USER catalog are each backfilled (per-entry processing), consistent with `build_update_plan`'s command-keyed first-wins rule.
- **Mixed duplicate commands (first empty, second custom)** (grinch obs 1): the first entry is backfilled, the second keeps its custom sequence; `build_update_plan`'s first-non-empty `find` (`agent_update.rs:271-273`) then selects the FIRST entry's sequence for that command — the backfilled one wins over the custom one (previously the custom one won by skipping the empty first entry). Data-level overwrite never happens; the winner is a consequence of the pre-existing command-keyed first-wins rule. Covered by §5.3.7.
- **Prompt-surface expansion** (grinch obs 2): users who registered hermes/opencode/agy and were never asked will see ONE first prompt at the next startup (default No); existing `agentAutoUpdateByCommand` answers are untouched. Documented in agent-auto-update.md (§5.5).
- **No embedded match** (custom commands, cursor's `agent`): entry stays empty → that command is never prompted nor updated (unchanged behavior).
- **Empty valid user catalog** (user removed all built-ins): honored verbatim; backfill over zero entries is a no-op.
- **Missing/corrupt manifest**: embedded default served unchanged; commands already present; backfill not needed (no-op if applied).
- **Failure behavior**: the helper is pure and cannot fail — `validated_embedded_default()` is already validated (its parse failure path is log-only and guarded by the drift test). No new error paths, no new log lines, no new panics. Update execution still goes exclusively through the existing `run_update_sequence` (per-step timeout, stdin closed, output tail-capture) with existing gate semantics.
- **Determinism**: identical files in → identical catalog out.

## 8. Compatibility and security

- No schema change (`schemaVersion` stays 1); on-disk format untouched; serde defaults unchanged; old binaries reading the new default JSON are unaffected (`updateCommands` has existed since #1323).
- Zero new write paths — seed-once, user-owned, and G3 corrupt-preserve are preserved by construction; the legacy verbatim-copy behavior is untouched.
- Zero new execution surface: the backfill only adds catalog DATA; commands still run only when (a) the command is registered in `settings.agents`, (b) a non-empty sequence exists, (c) the user's `agentAutoUpdateByCommand` answer or prompt consent allows it — all existing gates in `build_update_plan`.
- No new permissions, no frontend/IPC contract change (`get_coding_agent_catalog` returns the same shape), no role inversion (the helper is a pure config-layer function; it gains no `AppHandle`/tauri/transport dependency).

## 9. Tests and objective acceptance criteria

1. `cargo test` for the `coding_agents_catalog` module (and the full `src-tauri` test suite) is green on the branch head, including the renamed drift guard and the six backfill tests of §5.3 (items 2-7).
2. `embedded_default_parses_with_seven_agents_in_order` and `embedded_default_matches_current_presets_exactly` stay green (presets unchanged; JSON additions do not disturb key order).
3. `src-tauri/resources/coding-agents/agents.default.json` parses; drift guard asserts EXACTLY: claude/codex/pi/hermes/opencode/antigravity ship the §5.4 sequences, cursor ships none, all 7 `autoUpdate` false.
4. No-write proof: the backfill test asserts the manifest bytes are identical before/after `load_catalog` (write-path absence).
5. Manual verification on THIS machine (implementation environment): hash `D:\0_repos\AgentsCommander_iac\.ac\coding-agents\agents.json` before and after running the app — bytes unchanged; and the next startup log contains `[agent-update]` info lines (update steps for pi/claude/codex/opencode — the commands answered `true` — or at minimum a non-empty plan log).
6. Dependency-cycle gate (§11): zero new module arcs; regenerated `src-tauri/module-arcs.txt` byte-identical (empty `git status` on it); `cyclicSccs` unchanged; layering guards green.
7. Docs: the §5.5 paragraphs (including the first-prompt sentence) present in the PR diff.
8. No version bump and no dependency file changes (Cargo.toml / package.json / lockfiles byte-identical).
9. FE drift guard green: the project's FE test invocation for `src/shared/agent-presets.test.ts` passes — the renamed "#1318/#1325/#1546" test asserts the new shipping set with cursor `[]`; the `EXPECTED_BUILTINS` mirror test and the `definitionToSeed` tests stay green unchanged.

## 10. Explicit decisions and accepted residuals

- **Backfill-on-empty, including explicit `[]`** — accepted: capability vs preference separation (§7). Documented in both docs.
- **Backfill only on the parsed on-disk path** — the embedded-default returns need none (no-op by construction); keeps the change minimal.
- **Match by `command`, not key** — consistent with `build_update_plan` rule 1; covers user catalogs that key the same binary differently (e.g. two `pi` profiles).
- **Cursor stays un-updatable** — no verified vendor update subcommand; Cursor CLI self-updates with its app; documented in docs and enforced by the drift guard.
- **No new log lines** — keeps boot logs unchanged except for the newly live update activity itself.
- **No disk repair of old catalogs** — user-owned file (G3); in-memory backfill is the entire fix; re-seeding remains available to the user via the existing Settings button if they want the file rewritten.
- **FE mirror in scope, sole implementer ac-dev-rust-v3** — resolves the #769 drift-guard gap; byte-exact edits, no behavior change (`definitionToSeed` already drops `updateCommands`).
- **Mixed duplicates: backfilled first entry wins for plan-building** — accepted consequence of the pre-existing command-keyed first-wins rule; registered by test §5.3.7 and §7.
- **First prompt may appear for hermes/opencode/agy users** — accepted (one time per command, default No); documented in agent-auto-update.md.

## 11. Dependency-cycle and layering statement (planning rule 8)

The only Rust change is one private function and one call site inside `src-tauri/src/config/coding_agents_catalog.rs`. The helper calls `validated_embedded_default()` — module-local, ALREADY called by `load_catalog` on this exact path (missing/corrupt arms, `:244-253`). The JSON and docs changes add no code arcs. Therefore the plan adds **ZERO module-to-module arcs**; `cyclicSccs` and every SCC member set cannot change; no cross-boundary arc exists to classify. Role/layering hygiene holds: the helper is a pure config-layer function; no lower layer gains a UI-transport/`AppHandle`/tauri dependency.

Acceptance criterion for the implementer (base SHA `d64e6250` vs final branch head, clean tree for both):

```
node "<VAULT>/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "<VAULT>/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

Green iff: (1) `cyclicSccs` equal pre/post; (2) cyclic SCC member sets identical set-to-set; (3) zero new `from -> to` pairs cross a previously-clean SCC boundary; (4) regenerated `module-arcs.txt` byte-identical (empty `git status`); (5) structural layering guards stay green. Exit code 1 from the detector is the normal gating outcome; only exit 3 means no graph.

## 12. Implementation order

1. Commit A — data + BOTH drift guards in ONE commit: `agents.default.json` (§5.4) + Rust drift-guard rename/extension (§5.3.1) + FE mirror `agent-presets.ts` + FE test `agent-presets.test.ts` (§5.6). The two halves of the #769 guard stay truthful at every commit (dev gap resolution).
2. Commit B — add the helper + wire `load_catalog` + extend its doc comment (§5.1/§5.2).
3. Commit C — add the backfill unit tests incl. the mixed-duplicate case (§5.3 items 2-7).
4. Commit D — docs (§5.5).
5. Run the full `src-tauri` test suite, the FE `agent-presets.test.ts` suite, the §9.6 dependency-cycle gate, and the §9.5 manual verification on this machine. Force-add this plan file (`git add -f plans/1546-backfill-catalog-update-commands.md`) and push the branch.
