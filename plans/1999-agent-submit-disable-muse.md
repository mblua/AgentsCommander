# #1999 — Hermes/OpenCode/Grok submit; disable Muse

Status: READY_FOR_IMPLEMENTATION CANDIDATE — round-2 amendment 2026-09-14: corrects Grinch's round-1 plan-text CHANGES_REQUIRED (1: removes the obsolete global nonterminal-executions/idle gate; 2: step 7, Acceptance, Status and Delivery now separate the pre-implementation recipe review from the post-implementation review of the real eight-row bytes, required before commit/push). Round-1 verdict: the Sonar `.map` recipe D1-D3 and scope were accepted; that review covered the recipe only, never product bytes, which do not exist yet (the step-7 delta is unapplied). Steps 1-6 and 8, and the E1-E4 semantics of step 7, are already implemented and committed (`5ec18a5`, `05a700b`). Still pending: Grinch review of this round-2 text and user approval of the amended recipe; this is not permission to implement. No implementation, build, test run, commit or push performed by the amender. Decision complete; frontend note incorporated (ac-dev-webpage-ui, 2026-09-14).
Canonical: this file, `plans/1999-agent-submit-disable-muse.md` in `repo-AgentsCommander` (branch `fix/1999-agent-submit-disable-muse`). Earlier copies in the project-shared `.ac/plans/issue-1999/` and in the author's Agent Matrix are historical.
PARTITION: 1 phase; owner ac-dev-rust-v4 for the 4 backend files, ac-dev-webpage-ui for the 2 TS mirror files; reviewer ac-dev-rust-grinch-v4; no new interface, dependency, schema, IPC or module arc.

## Base and evidence (2026-09-14)

- Repo `D:/0_repos/AgentsCommander_iac/.ac/room-3-ac-dev-team-v4/repo-AgentsCommander`; branch `fix/1999-agent-submit-disable-muse`; base at plan creation: HEAD = `main` = `origin/main` = `51fff3d1946ea71ddf32cbdc583d6446548398e7` (round-1 context: HEAD `b2e3f9e67abb6fa00ac73303503de20428140c3d`, implementation commits `5ec18a5`, `05a700b`, PR #2003). Tree clean; only untracked `.codebase-memory/` (discovery artifact — preserve, never commit).
- Issue #1999 OPEN (gh). `node scripts/validate-branch-name.mjs fix/1999-agent-submit-disable-muse` → OK. Graph at that root: ready, 24151 nodes / 162887 edges. All cited files read directly (coverage `metadata_changed`; `agent-presets.test.ts` excluded from the index by design, read directly).
- Prior evidence: `room-shared/expanded-agent-submit-proposal.md`, `room-shared/hermes-opencode-submit-proposal.md`, `room-shared/informe-hermes-enter-room3-20260914.md`. Grok identity confirmed by the user (`grok` command, npm `@xai-official/grok`, label "Grok Build") and primary docs (`docs.x.ai/build/cli/reference`, `.../keyboard-shortcuts`, `.../features/project-rules`).

## Round-1 amendment (2026-09-14) — Sonar new-code duplication

Trigger: PR #2003 (head `b2e3f9e67abb6fa00ac73303503de20428140c3d`, base `main`) has all GitHub Actions jobs green; the only red check is SonarCloud Code Analysis — **30.8% duplication on new code (required ≤3%)**: `src/shared/agent-presets.ts` `new_lines=14`, `new_duplicated_lines=8` (57.14% file density; total file duplication 76.1%, pre-existing); `src/shared/agent-presets.test.ts` 0% (12 new lines). Duplicated groups are all inside `agent-presets.ts`: 11-65 ↔ 66-118 and 25-62 / 64-101 / 77-115; new-and-duplicated lines are 22, 23 (claude comment) and 107-112 (grok `key`..`instructionsFilename`). No Rust/JSON or test counterpart; the detector matches rows whose string values differ (it normalizes literals).

Sources: amender `ac-dev-webpage-ui-v4` (band 1-25) per the `ac-tech-lead-v4` round-1 request; architect proposal `messaging/20260914-082000-room3-ac-architect-v4-to-room3-ac-tech-lead-v4-1999-sonar-analysis.md`; FE feasibility verdict `messaging/20260914-081523-room3-ac-dev-webpage-ui-v4-to-room3-ac-tech-lead-v4-1999-sonar-feasibility-verdict.md` (implementation feasibility APPROVED; Sonar sufficiency **not demonstrable statically**).

Amended scope: step 7's prescribed bytes are replaced by the `.map` normalization (D1-D3). Scope is otherwise unchanged (muse out, grok in after `antigravity`, comments corrected, tests untouched). No exclusion, no threshold relaxation, no blind reruns. The amended plan needs Grinch recipe review and user approval of the new recipe before implementation; the eight-row equivalence is a post-implementation byte gate (step 7), not a claim made at recipe time. I2001 stays separate.

## Requirement and confirmed design

Support submit for Hermes, OpenCode, Grok Build and Cursor CLI (Cursor already supported: key `cursor`, command `agent`), and disable Muse with the existing boolean.

1. `src-tauri/src/pty/inject.rs`: add private `PtyInjectionProfile::ExplicitSubmit` for exact stems `hermes|opencode|grok` (exact match, like `agy`/`pi`/`agent`; only `claude*`/`codex*` keep prefix matching). `needs_explicit_enter` becomes true through the existing `!Unsupported`. The canonical injector sequence is untouched: text, `\r` at +1500 ms (fatal), `\r` at +500 ms (non-fatal). Capabilities stay negative — not `Established`: `resolve_logical_command_text` → `None` (no clear/compact), `supports_auto_self_maintenance` → false, `supports_self_handoff_switch` → false. No helper, parameter or interface changes.
2. In scope and pinned: `validate_supported_agent_session` (`inject.rs:276`) now admits the three stems, so internal notices, context alerts (`mailbox.rs:8202,8223`) and restart-resume (`session.rs:5251`) become available for them; root/exited/agentless rejection unchanged.
3. Catalog: insert the Grok row after `antigravity` and before `muse`; `BUILTIN_AGENT_SUPPORT` gets `("grok", true)` in that position and `("muse", false)`. The muse row and embedded JSON stay (key correspondence, reversibility). The flag is data-only: no session or file cleanup, and no edit to the local layer, migration backups, corrupt files or already-seeded masters. It does change the managed revision, so a verified managed base refreshes on the next initialization (dropping muse); a brand-new seed writes the embedded bytes with the false row omitted.
4. Cursor, Claude, Codex, Pi, Antigravity behavior unchanged (regression-only).
5. Frontend mirror (authorized; no runtime behavior, test-effect only): the #1912 rule makes `FALLBACK_CODING_AGENTS` equal the enabled rows — muse out, grok in. Since #1965 (`3b7cc6e`, 2026-09-11) it has no production consumer: on an IPC catalog failure the store disables registrations and serves no embedded bytes (`src/sidebar/stores/coding-agents.ts:10-15`; `CodingAgentQuickConfiguration.test.ts:588`), so this is a data/parity change pinned by the drift test, not a runtime behavior change. The obsolete "Served only when the IPC catalog call rejects" header comment is corrected in the same edit.

## Inventory (6 existing files + this new plan)

| # | File | Change |
|---|---|---|
| 1 | `src-tauri/src/pty/inject.rs` | ExplicitSubmit profile + classification + docstrings + tests |
| 2 | `src-tauri/src/config/coding_agents_catalog.rs` | `grok=true`, `muse=false`, test adaptations |
| 3 | `src-tauri/resources/coding-agents/agents.default.json` | Grok row |
| 4 | `src-tauri/tests/cli_project_registration.rs` | seeded last-row expectation |
| 5 | `src/shared/agent-presets.ts` (owner ac-dev-webpage-ui) | mirror (muse out, grok in, header comment corrected) committed in `05a700b`; **round-1 pending delta**: remove the four repeated defaults from the eight rows and close with the final `.map` (Sonar ≤3%) |
| 6 | `src/shared/agent-presets.test.ts` (owner ac-dev-webpage-ui) | mirror test: set, order, titles, de-supported negative |
| 7 | `plans/1999-agent-submit-disable-muse.md` | this plan (new doc) |

## Exact edits, in order

1. `src-tauri/resources/coding-agents/agents.default.json` — insert between the `antigravity` and `muse` objects, byte-exact:

```json
{
  "key": "grok",
  "label": "Grok Build",
  "description": "Coding agent Grok Build",
  "color": "#64748b",
  "command": "grok",
  "instructionsFilename": "AGENTS.md",
  "envs": [],
  "isolatedHome": false,
  "removable": true
}
```

No `configSeed`, no `updateCommands`, no `autoUpdate` (reads default to `[]`/false, as with `cursor`).

2. `src-tauri/src/config/coding_agents_catalog.rs` — `BUILTIN_AGENT_SUPPORT`: `("grok", true)` before `("muse", false)`; all other rows unchanged.
3. `src-tauri/src/pty/inject.rs` — `PtyInjectionProfile`, `pty_injection_profile`, and the `needs_explicit_enter`/`inject_text_into_session` docstrings naming the three exact stems.
4. Catalog/module tests (same file):
   - `EXPECTED_PRESETS` → 9 entries (grok after antigravity, `Some("AGENTS.md")`, no seed dest); `embedded_default_parses_with_eight_agents_in_order` → nine, grok before muse, muse stays last (update the inline "Muse is last, immediately after Antigravity" comment: muse still last, now preceded by grok); `embedded_default_matches_current_presets_exactly` → configSeed 3 / no-seed 6 (comment "the other five ship none" → six).
   - update-commands test → 9 rows, grok in the empty branch; `builtin_agent_support_ships_every_row_enabled` → only `muse` false.
   - Add `TABLE_ALL_ENABLED` (9 rows) for the four tests needing a state differing from shipped: `support_override_scopes_to_closure_and_restores_shipped_table`, `managed_catalog_false_row_is_part_of_the_revision_and_refreshes_only_the_base`, `managed_catalog_support_gate_change_refreshes_the_base_both_ways`, `managed_catalog_interrupt_across_a_revision_change_completes_then_refreshes` (inside counts 9, muse present).
   - `desupported_row_dropped_from_parsed_manifest_and_dedup_cannot_resurrect_it`: run the `["muse","mine"]` control half under `TABLE_ALL_ENABLED`; keep the drop half under the shipped table.
   - Two `shipped_def_json(&["…","muse"])` users switch key to `grok`: `managed_catalog_local_removability_is_evaluated_against_the_base`, `managed_catalog_stale_revision_warning_uses_the_exact_context_reason`.
   - `managed_catalog_local_new_row_order_and_tombstone`: tombstone key muse → grok (keep `keys.len() == 8`).
   - Extend `TABLE_MUSE_OFF` and `TABLE_CLAUDE_OFF` to the 9 rows; in `TABLE_CLAUDE_OFF` keep `muse=true` (only claude false) and update the shared comment to "all 9 rows ... exactly one false each" (`TABLE_MUSE_OFF` mirrors shipped).
   - Mechanical count rule: embedded 8→9; enabled/effective stays 8; `TABLE_MUSE_OFF` blocks that asserted 7 become 8 (`desupported_row_dropped_from_seeded_manifest_and_corrupt_is_unavailable`, `managed_catalog_fresh_base_bytes_carry_only_enabled_rows`, `..._with_nonregular_legacy`).
   - Unchanged by design: dedup, local-layer, migration-backup byte-exactness, corrupt-file preservation, tombstone counts (6 and 8), `keys[8] == "pi-max"`, publication/interrupt/symlink/lock tests.
5. `src-tauri/tests/cli_project_registration.rs` — `new_project_seeds_catalog_into_ac`: seeded count stays 8, last row becomes the grok object above (serialized with `updateCommands: []`, `autoUpdate: false`), no muse key.
6. `src-tauri/src/pty/inject.rs` tests:
   - `agent_clis_require_explicit_enter`: add positives for the three stems — bare, extension (`.exe`/`.cmd`/`.ps1`), uppercase, padded whitespace, Unix-style absolute path (`/usr/local/bin/<stem>`), all cross-platform.
   - Windows-only positives live in a separate `#[cfg(windows)]` test (`explicit_submit_windows_native_paths`): `C:\...\<stem>.exe`, `\\server\share\<stem>.cmd`, `\\?\C:\Tools\<stem>.exe`. `shell_file_stem` uses `std::path::Path::file_stem`, which does not treat `\` as a separator on Unix, so these must not be asserted unconditionally (the older ungated pi block is pre-existing and unchanged).
   - `direct_shell_capability_matrix`: new `ExplicitSubmit` positive group asserting enter true + clear/compact `None` + maintenance false + switch false; near-miss negatives `hermes-wrapper`, `hermes-cli`, `my-hermes`, `opencode-proxy`, `opencode2`, `opencode-tui`, `grok-build`, `grokx`, `grok-cli` (never a stem beginning with `claude`/`codex`).
   - `supported_agent_final_snapshot_rejects_unsafe_recipient_records`: positives for the three stems; keep root/exited/agentless/`pwsh` negatives.
   - New in-module recorder tests on the existing `RecordingBackend`: `explicit_submit_stems_write_text_then_two_enters` (hermes/opencode/grok via `tokio::join!`, assert `[payload, \r, \r]`) and `unsupported_stem_writes_text_without_enter` (muse → `[payload]`).
   - Kept as recorder coverage: `a_waiting_user_write_cannot_splice_between_text_and_enters` (`[text, \r, \r, user]`), `exact_submission_phase_outcomes_and_backend_calls_are_pinned` (first CR fatal, second non-fatal), menu-guard block, plain-shell negatives, and mailbox `pi_canonical_injector_writes_arbitrary_text_then_two_enters`, `remote_established_command_branches_preserve_text_and_submission`, `remote_cursor_command_branches_preserve_text_and_submission`.
7. `src/shared/agent-presets.ts` (round-1 amended recipe; owner ac-dev-webpage-ui). Already implemented and committed in `05a700b` under the original recipe: E1 muse object removed; E2 grok row inserted after `antigravity` (`key: "grok"`, label `Grok Build`, color `#64748b`, command `grok`, `instructionsFilename: "AGENTS.md"`, `updateCommands: []`); E3 claude-row comment updated; E4 obsolete header comment replaced with the #1965 wording. Evidence: `room-shared/1999-fe-implementation-evidence.md`; its file/patch hashes are superseded by this delta and must be re-pinned after implementation. The Sonar amendment replaces the byte recipe with this **pending delta on the same single file**:

   - D1 Remove `envs: [], isolatedHome: false, removable: true, autoUpdate: false,` from each of the eight row literals. Keep every other field, comment and row order exactly as committed; keep `updateCommands` explicit in every row (including `[]` in cursor and grok).
   - D2 Close the array with this byte-exact map (architect's proposal):

```ts
].map((definition): CodingAgentDefinition => ({
  ...definition,
  envs: [],
  isolatedHome: false,
  removable: true,
  autoUpdate: false,
}));
```

   - D3 Nothing else: `FALLBACK_CODING_AGENTS` name/export/type `CodingAgentDefinition[]`, row order (grok last, muse absent), fields, comments, `definitionToSeed` and `newAgentId` stay byte-unchanged; no helper/export/import/module/dependency. `.map` evaluates the literal per element, so every row gets its own fresh `envs: []` — never a shared defaults object; `definitionToSeed` keeps copying `envs` by reference exactly as today.
   - Recipe review gate (Grinch, before implementation): reviews the recipe only — D1-D3, the four defaults, array independence by construction, helpers untouched, and the expected diff (only the four-field removal per row plus the map tail). The product bytes do not exist at this gate, so it must not claim to have reviewed them; the eight-row equivalence is asserted as an expected result, not as verified bytes.
   - Byte review gate (Grinch, after implementation and before commit/push): reviews the real implemented diff of `src/shared/agent-presets.ts` row by row — the eight rows are value-identical before/after the `.map` normalization (same keys, same values including `updateCommands`, same order), each row's `envs` is a distinct array, `definitionToSeed` and `newAgentId` are byte-unchanged, and the new file/patch hashes are re-pinned in `room-shared/1999-fe-implementation-evidence.md`, superseding the `05a700b` hashes. No commit or push before this gate passes.

Risks recorded (confirmed):
   - Typing: the spread weakens TS excess-property checking on the row literals; a stray or misspelled key in a future row would flow silently (none today).
   - Future overrides: `.map` assigns the four fields after `...definition`, so a future row with its own `envs`/`isolatedHome`/`removable`/`autoUpdate` would be silently overwritten (TS will not flag it); a future non-default must be handled explicitly.
   - Property order: the four defaults move to the end of each definition object. Harmless today — no production consumer of the constant (#1965 serves no embedded bytes on IPC failure); production importers use only `newAgentId`/`definitionToSeed`; the two test consumers use `toEqual`/`toMatchObject`, with no snapshots and no `JSON.stringify`.
   - Sonar is not predictable statically: whether the residual 4-vs-4 row block falls under the analyzer's ~100-token reporting floor depends on normalization depth (simulation: 86 literal-normalized tokens vs 114 with identifiers). Only the real run on the amended SHA decides; no exclusion, no threshold relaxation, no blind reruns. If it stays red, the decision returns to the architect for diagnosis (I2001 stays separate).
8. `src/shared/agent-presets.test.ts` (exact deltas from the same note): T1 `EXPECTED_BUILTINS` line 26 →

```ts
  { key: "grok", label: "Grok Build", description: "Coding agent Grok Build", color: "#64748b", command: "grok", instructionsFilename: "AGENTS.md" },
```

T2 first-test key list ends `..., "antigravity", "grok"` (muse removed). T3 add, next to the key assertion:

```ts
    // #1999 — muse stays in the embedded default as a disabled row, so the
    // enabled-row mirror must not carry it.
    expect(FALLBACK_CODING_AGENTS.some((a) => a.key === "muse")).toBe(false);
```

T4 first-test title: "matches the enabled built-ins: 8 rows (muse disabled, grok added), exact order and fields". T5 update-command test title: "#1318/#1325/#1546: claude, pi, codex, hermes, opencode, and antigravity ship update commands; cursor and grok ship none; every entry defaults autoUpdate off" (body unchanged; grok falls into the `else` → `[]`). T6 header: "#769 — second copy of the backend's ENABLED built-ins ... pins the enabled set and order (the backend's `embedded_default_matches_current_presets_exactly` pins the 9-row embedded default)"; the #1912 block stays. `SettingsModal.automation.test.ts` needs no change (its codex/opencode presets remain; it does not reference muse or grok). No length assertions exist; the set stays 8 and no dynamic test iterates muse. Round-1 delta: no test change — the existing assertions pin values, order and defaults and remain valid for the normalized form; do not regenerate `EXPECTED_BUILTINS` from the same data. Already committed in `05a700b`.

## Acceptance

- Exact stems (case-insensitive stem, `.exe/.cmd/.ps1/.sh`, cross-platform paths, whitespace) receive text then two CRs; near-misses and plain shells receive no CR; muse stays text-only.
- The three stems have no clear/compact, no auto-maintenance, no self-handoff; not Established; `PtySubmissionAgent` unchanged.
- Muse disabled on every catalog read path; fresh seed publishes 8 enabled rows (grok present, muse absent); managed base refresh drops muse; local layer, migration backup and corrupt bytes preserved; reversibility retained.
- Admission widening for internal notices/context alerts/restart-resume pinned, with root/exited/agentless still rejected.
- Frontend mirror equals the enabled rows (muse out, grok in) and its drift test passes; the parity is manual byte-copy (no cross-language check) and the corrected header no longer claims a runtime fallback.
- Round-1 delta, post-implementation byte gate (before commit/push): on the real implemented diff, the eight rows are value-identical before/after the `.map` normalization (row-by-row reviewer comparison: same keys, same values including `updateCommands`, same order), each row's `envs` is a distinct array, `definitionToSeed`/`newAgentId` are byte-unchanged, and the new file/patch hashes are re-pinned. The earlier pre-implementation gate covers the recipe only and claims no product bytes.
- Sonar: new-code duplication ≤3% on the exact amended PR SHA, proven by the real Sonar run only; no exclusions, no threshold changes, no blind reruns. If it stays red, the decision returns to the architect for diagnosis.
- Cursor, Claude, Codex, Pi, Antigravity tests green.

## Transitions

- Managed base: revision includes the table; `ensure_seeded` publishes the enabled rows and refreshes a verified base on the revision change; an edited base is never refreshed.
- Local layer: never modified; local muse entries/tombstones remain suppressed as unsupported; a custom key whose command is `muse` is still kept (gate is keyed).
- Migration backup: byte-for-byte user data (muse may remain inside).
- Corrupt persisted file: bytes preserved, `baseInvalid`, no self-heal, no writes.
- Masters: `.claude/.codex/.opencode` unaffected; muse has no master; already-seeded masters never removed.

## CI, toolchain and dependency gate

Toolchain: Rust stable + rustfmt + clippy; Node 22; npm 11.6.2. Zero new crates, modules, `use`/`mod` statements, IPC or schema; `Cargo.toml`/`Cargo.lock`/`package-lock.json`/`src-tauri/module-arcs.txt` byte-unchanged. Diff allowlist: the six files above plus this plan.

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib pty::inject
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib config::coding_agents_catalog
cargo test --locked --manifest-path src-tauri/Cargo.toml --test cli_project_registration new_project_seeds_catalog_into_ac
cargo test --locked --manifest-path src-tauri/Cargo.toml --test pty_writer_inventory
npm ci && npm run typecheck && npm run test -- src/shared/agent-presets.test.ts
npm run test -- src/sidebar/components/SettingsModal.automation.test.ts   # the other fixture consumer of FALLBACK_CODING_AGENTS
```
CI parity (`.github/workflows/pr-regression-gates.yml`): `rust-fmt`; `rust-regression` (windows: `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests`); `validate-branch-name`; `frontend-regression` (`npm ci`, `npm run typecheck`, `npm test`) once the mirror files are included. No check is claimed from a byte recorder alone.

Sonar gate (round-1): SonarCloud PR analysis on the exact amended SHA must report new-code duplication ≤3%. Measured baseline before the amendment: overall 30.8% (8 of 26 new lines), `src/shared/agent-presets.ts` 8/14 = 57.14% (total file duplication 76.1%, pre-existing), `agent-presets.test.ts` 0%. The analyzer's token normalization is not fully predictable locally, so this gate is decided only by the real run; exclusions, threshold relaxation and blind reruns are forbidden.

## Limits and runtime proof (not claimable statically)

- The recorder proves AC's write order/serialization only, not client compatibility.
- Grok: docs confirm Enter in normal mode; multiline mode may use different submit keys — unverified until a real Grok version is exercised (tester, later authorization).
- Grok AGENTS.md discovery: docs warn ignored files may be skipped and AC replicas are gitignored — unverified; do not add flags or touch `.gitignore` without evidence.
- The first CR can be dropped (#611); the second is the mitigation. Clear/compact, maintenance and switch stay unavailable by design.
- Frontend parity is enforced only within each half (Rust pins table↔JSON; the FE pins its own copy). The two halves are copied byte-exactly by hand, and `#64748b` intentionally repeats opencode's color (no uniqueness test).
- Sonar duplication analyzer uncertainty (declared, tooling): the `.map` removes real repetition, but whether the residual 4-row-vs-4-row block (~86 literal-normalized tokens, ~114 with identifiers, against the analyzer's ~100-token reporting floor) is reported at all depends on its normalization depth; ≤3% cannot be claimed statically, only by the real run on the amended SHA.
- `src/shared/types.ts` `CodingAgentKind` (`"claude" | "codex" | "pi" | "antigravity" | "muse"`) mirrors the Rust session enum (`session.rs:598`), not the catalog; muse stays a valid kind (a custom key whose command is `muse` is preserved). Not touched.

## Delivery gates

User approval before implementation (the round-1 amended recipe is pending that approval). Grinch review is two-stage: recipe review of this plan before implementation (no product bytes exist yet), and the real eight-row byte review of the implemented diff (step 7) after implementation and before commit/push. Runner capacity (single policy): at most 3 distinct repo+branch pairs with pending executions across all rooms and repos in the project; workflows/jobs of the same pair occupy one slot and `main` counts. With 1-2 active pairs another pair may start; with 3 the shipper waits. The previous batch on the same branch must finish before the next trigger. The shipper queries a fresh capture immediately before triggering and rechecks every 10 minutes while blocked by capacity or by its own unfinished batch (no extra 10 minutes when eligible). No reservations or atomic locks (accepted race risk), no cancellations, no skipping required validations, no global nonterminal-executions/idle gate. Shipper requires the applicable checks green on the exact PR head, including the real SonarCloud new-code duplication ≤3% on the amended SHA; no exclusions, no threshold relaxation, no blind reruns. If Sonar stays red, return to the architect for diagnosis. Recovery: revert only this change set; preserve `.codebase-memory/` and unrelated state.
