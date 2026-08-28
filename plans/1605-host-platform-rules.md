# Plan #1605: `{{HOST_PLATFORM_RULES}}` global context block — file-configurable per-platform shell-routing rules, versioned seeding, fallback self-heal

Author: ac-architect-v3, workgroup wg-19-ac-dev-team-v3. Sole editor of this plan per round-1 consensus dispatch (`20260828-004800-wg19-ac-tech-lead-v3-to-wg19-ac-architect-v3-issue1605-round1-plan.md`) as amended by the mandatory owner amendment (`20260828-005300-wg19-ac-tech-lead-v3-to-wg19-ac-architect-v3-issue1605-enmienda-archivo.md`), which elevates spec point 6 (file-configurable platform rules) from "propose as PHASE 2" to a HARD acceptance requirement: the rendered `{{HOST_PLATFORM_RULES}}` content must be changeable by editing a file, without code changes or a release.

Status: READY_FOR_IMPLEMENTATION

Revision: round 2 (2026-08-28 UTC) — amendment of the round-1 plan per `20260828-043000-wg19-ac-tech-lead-v3-to-wg19-ac-architect-v3-issue1605-round2-enmienda.md`, documenting what the implementation actually executed: S4 (`src-tauri/src/config/seed_manifest.rs` scope literals, §6.4), the measured budget values (8262 / 6504 / 8042, §3.7), and the bounded base drift (§1). No D1–D11 decision changes; the round-1 certified digest is invalidated by this amendment. Plan-SHA256 of the certified bytes is reported to the tech-lead in the reply message, never embedded here (a file cannot contain its own hash; see `plans/1446-...` §Certification conventions).

Issue: [mblua/AgentsCommander#1605](https://github.com/mblua/AgentsCommander/issues/1605) (OPEN) — "{{HOST_PLATFORM_RULES}} en seed de contexto global" — owner directive via WG17.

Objective: add an 8th mandatory placeholder `{{HOST_PLATFORM_RULES}}` to the global default agent template (version 4 → 5) immediately after `{{CLI_CONTEXT}}`, rendered per the session's EXECUTION platform (host for backend=local; nothing for container/transport-api sessions). On Windows host sessions the block carries the Git Bash CLI-routing rule (single source of truth; the #1596 paragraph in `{{INTER_AGENT_MESSAGING}}` shrinks to a pointer to it). The block content is configurable per platform through `.ac/Context.platform.<os>.md` files (seeded absent-only, versioned in the seeded-template state, edits preserved, embedded-default fallback with WARN when missing/empty). Coordinator sessions receive the block through the global template render (no coordinator-template change needed). Byte budget `full_wg <= 8_313` and `touched_owners <= 6_810` are preserved with specified wording trims; the 3 manual gates stay green.

---

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `feat/1605-host-platform-rules` (created by the tech-lead from `main`). At authoring time (2026-08-28 UTC):

- `git status --porcelain` is empty; local `HEAD` == `origin/main` == `047248bc568d7d5470ed504c6ccf9d572d5cd60a` (verified by `git log origin/main -1`).
- Codebase Memory gate: `ready` (project `D-0_repos-AgentsCommander_iac-.ac-wg-19-ac-dev-team-v3-repo-AgentsCommander`, 25291 nodes / 136245 edges, index at head `047248bc568d7d5470ed504c6ccf9d572d5cd60a`). Evidence gathered by direct reads of the anchor files below (the budget profile was re-measured by running `token_accounting_report` against the debug build of this exact tree; see §3.7).
- The plan file is the ONLY file this plan may create; `.gitignore` line 11 ignores `/plans/`, so the implementer MUST force-add it: `git add -f plans/1605-host-platform-rules.md`. Do not remove or weaken the ignore rule.

The implementers must repeat the authority ritual: fetch `origin/main` and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. If a quoted line number no longer matches the quoted text, re-anchor on the text, never on the number. Target-branch drift after this round that does not touch the selected files (§6) does not reopen the design; it is recorded at the next bounded gate (skill: delivery-nonfunctional-invariants §Bounded target-branch drift).

**Round-2 amendment (2026-08-28 UTC) — bounded base drift registered:** between round 1 and the implementation, `origin/main` advanced `047248bc568d7d5470ed504c6ccf9d572d5cd60a` → `d7008b34` (6 commits, all CI/fmt: the `#1608` rust-fmt gate, the `#1610` dtolnay/rust-toolchain full-SHA pin, the `#1602` cargo-fmt sweep). Registered as a bounded gate per the delivery-nonfunctional-invariants §Bounded target-branch drift rule: the implementer's word/token-diff of the merge is empty (streams identical modulo whitespace/pins), the v4 freeze operand is intact at the merged tree (`get_default_agent_template()` len 539, sha256 `f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a`), and the round-1 certified digest (reported out-of-band, never embedded here) was verified unchanged at implementation start until this amendment. The implementation base is therefore `d7008b34`; §9.1's workflow source-of-truth reference is updated to it. The drift does not reopen the design.

## 2. Task class and threat model

Routine application change: one new context placeholder + renderer branch, three new seeded content files with one state extension (additive map entries, no schema bump), wording trims in four existing context blocks, one doc, and unit tests. No release, no signing, no packaging, no security-boundary change, no migration of stored data, no new binary. Baseline gates apply; **no enhanced controls are applicable** (no hostile-host threat model is claimed; host executables are trusted per the repository contract — GitHub CI is the authoritative host-dependent evidence, §9.1).

## 3. Verified evidence (re-verified at `047248bc`, not predicted)

### 3.1 The v4 global template and render chain (`src-tauri/src/config/session_context.rs`)

1. `get_default_agent_template()` (~:2469) is the v4 template: 7 mandatory placeholders in order `WRITE_RESTRICTIONS`, `DELEGATED_TASK_REPORTING`, `SKILLS_SECTION`, `AGENT_REPOS`, `CLI_CONTEXT`, `SESSION_CREDENTIALS`, `INTER_AGENT_MESSAGING`. Measured from the accessor bytes at this head: **len 539, sha256 `f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a`** (no CRLF; Rust raw literals normalize `\r\n` → `\n`). This is the v4 operand to freeze for the bump (§5.4).
2. `render_agent_context_template_inner` (~:2555) runs the mandatory-placeholder loop (append-fallback for missing tokens, `MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS` ~:3037) and then the replace chain (~:2647-2657) which fills each placeholder. The new `.replace("{{HOST_PLATFORM_RULES}}", ...)` goes between the `{{CLI_CONTEXT}}` and `{{SESSION_CREDENTIALS}}` replaces. The platform block value must be computed BEFORE the chain (like `agent_repos` at :2612).
3. The append-fallback machinery: `mandatory_section_heading` (~:3049) maps each placeholder to its unique heading; `mandatory_placeholder_aliases` (~:3085); `mandatory_section_present_inline` (~:3098, line-anchored); `coarse_section_dedup_safe` (~:3138). Test `mandatory_section_heading_map_is_complete_and_collision_free` (~:4989) auto-checks completeness/collisions for whatever the list contains.
4. `get_default_coordinator_template()` (~:2508) is plain text (no placeholders); the coordinator body is appended AFTER the combined replica context in `resolve_session_context_content_with_activation` (~:2105-2170, `---\n\n# Orchestrator Context\n\n`). Coordinator sessions render the global template through the same `ensure_session_context_with_config` → `resolve_agent_context_with_activation` → `render_agent_context_template` path (replica `config.json` requires `$AGENTSCOMMANDER_CONTEXT`, `replica_identity.rs:8`), so the new block reaches coordinator sessions automatically via the global render. **No coordinator-template change** (§5.2).

### 3.2 Platform discriminator (precedent #935) and the #1596 rule

1. `repo_mounts: Option<&RepoMountResolution>` threads through `render_agent_context_template_inner` and `render_root_runtime_prologue_inner`; `Some` = containerized session (transport api), `None` = host local session (`render_agent_repos_string` ~:1581, #935). The host OS is decided with `cfg!(windows)`/`target_os`, exactly like `WINDOWS_SHELL_ROUTING` (~:3472, empty on non-Windows).
2. `WINDOWS_SHELL_ROUTING` is injected into `render_inter_agent_messaging_block` (~:3436-3467) as `{windows_shell_routing}`; current bytes (measured): **201 chars** — `"\n\n**Windows:** the release binary is GUI-subsystem; PowerShell direct capture is empty. Run AC CLI invocations via Git Bash (`C:\Program Files\Git\bin\bash.exe`); never `& $bin ... | ConvertFrom-Json`."`. Requirement 5: this becomes a pointer to the platform block (no duplication).

### 3.3 Root prologue (#979 G4) and the preserved root template

1. `render_root_runtime_prologue_inner` (~:3225-3270) assembles **9 direct blocks** (`ROOT_RUNTIME_PROLOGUE_HEADER`, `CORE_CONCEPTS_SECTION`, write_restrictions, `DEFAULT_DELEGATED_TASK_REPORTING`, skills, agent_repos, `DEFAULT_CLI_CONTEXT`, `DEFAULT_SESSION_CREDENTIALS`, inter_agent_messaging). It never passes the placeholder machinery, so a missing platform block cannot be suppressed by an editable file. The platform block becomes the **10th block, between `DEFAULT_CLI_CONTEXT` and `DEFAULT_SESSION_CREDENTIALS`** (mirroring the template order), computed by the same `render_host_platform_rules_block` (§5.5).
2. `Context.root-agent.md` (`ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME`, `root_spec` v7, `project_actionable: false`) is the root ROLE template — preserved by the daemon, never rendered through the placeholder machinery, and NOT part of the runtime prologue. It is untouched by this plan; the acceptance "probar con `Context.root-agent.md`" is realized as a self-heal unit test using a personalized-template fixture with root-agent-shaped content (§8.1, T-6).

### 3.4 Seeded-template state and specs (`src-tauri/src/config/seeded_context_templates.rs`)

1. `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME` = `.agentscommander-context-templates.json`; `STATE_SCHEMA_VERSION = 1`; `SeededContextTemplateState { schema_version, templates: BTreeMap<String, SeededContextTemplateEntry> }`; `SeededContextTemplateEntry { template_id, current_version, last_seeded_sha256, last_observed_sha256, ignored_default_sha256, ignored_observed_sha256 }` (~:147-175). `SeededContextTemplateSpec` (~:177) drives everything: id, filename, label, current_version, current_content: fn() -> &'static str, is_known_generated, project_actionable, suppress_unknown_without_state.
2. `project_specs()` (~:477) currently returns `[global(v4), coordinator(v5)]`; `root_spec()` (~:505) is the separate non-actionable root template. `is_known_generated_global_template` (~:535) recognizes current + frozen v1/v2/v3; `is_known_generated_standalone_global_template` (~:553) additionally recognizes the 307-byte #979 standalone snapshot; `is_known_generated_coordinator_template` (~:561) recognizes current + 4 frozen coordinator generations.
3. `sync_one_template` (~:1000-1170) implements the full lifecycle reused here: create-missing absent-only (`write_template_if_missing`, hard-link publish); byte-equal-default → `mark_seeded`; seeded + known-generated + default-changed → `auto_update_generated_template`; customized → preserved + `mark_observed` (+ pending update in scan mode, or the WARN "a newer default is available"); stateless unknown → `suppress_unknown_without_state` (AmbiguousWithoutState) or preserved.
4. Seed call sites: `ensure_project_context_templates_with_clock` (~:1314, project open/create/scan, `allow_create_missing: true`) and `scan_project_context_template_updates_with_clock` (~:1363, read-only). `sync_project_context_template_for_read` (~:1419) is per-filename; `read_or_create_context_template_with_sync` (`session_context.rs` ~:1199) invokes it only for the two `is_managed_project_template` filenames (global, coordinator). **Platform files are deliberately NOT read-time-synced** (§5.6) — the render reads them directly.
5. Seed-manifest scope mapping: `manifest_scope_for_project_context_filename` (`session_context.rs` ~:29) maps global→`context:agentscommander`, coordinator→`context:coordinator`, else `None` (no row). Platform publications need a scope (§5.7).
6. Standalone retirement (`retire_standalone_global_context`, ~:1478) and `remove_global_state_entry` (~:1709) operate ONLY on the app-config directory and the standalone global name; platform files live only in project `.ac` roots and are never retired or clobbered by it (§5.8).

### 3.5 Version-pin and assumption tests that the bump must update

- `project_specs_bump_only_the_global_template_to_v4` (~:2059) destructures `let [global, coordinator] = project_specs();` — **will not compile** with 5 specs; must be rewritten.
- Six tests assert `templates["global"]["currentVersion"] == 4` (~:2471, :2569, :2628, :2847, :2905, :2973) → 5.
- Byte-exact pins: `global_pre_token_minimization_snapshot_is_byte_exact` (:2406), `global_pre_agent_repos_snapshot_is_byte_exact` (:2481), `global_before_summarization_snapshot_is_byte_exact` (:2495) model the new v4 pin.
- `read_sync_updates_pre_token_minimization_global_template` (:2414) is the failing-first template for the v4→v5 upgrade proof (auto-upgrade a pristine v4 on disk + state currentVersion + both recognizers).
- `context_create_records_both_project_templates_under_the_gate` (~:2140) asserts manifest scopes (`context:agentscommander`, `context:coordinator`) — platform files add `context:platform` rows; the test's `contains` asserts still pass, but the name/comment must be updated.
- `create_default_context_templates_does_not_create_root_template` (`root_agent.rs` ~:2963) asserts global + coordinator exist and no root template — passes with platform files present.
- Legacy classification (`classify_legacy_rendered_default_context` ~:4033): the LEGACY compat renderer is frozen and untouched; baked v4 renders already classify as NotLegacy (they contain `# Agent Repos`), so the bump needs no legacy-renderer change. The append-fallback covers baked templates (they lack the new heading).

### 3.6 Byte budget — measured on this tree (Windows debug build, `token_accounting_report`, `--ignored --nocapture`)

Measured at `047248bc` (same machine shape as the CI windows `rust-regression` job):

| item | chars |
|---|---|
| write restrictions (A2, replica) | 3475 |
| inter-agent messaging (A3, replica, incl. 201-byte Windows paragraph) | 2103 |
| CLI context (A4a) | 726 |
| session credentials (A4b) | 257 |
| delegated task reporting (A4c) | 205 |
| **profile: WG replica (`full_wg`)** | **8245** |
| profile: Root Agent | 14074 |

Budget constants in `summarized_default_context_meets_size_budget` (~:10346): `MAX_FULL_WG_PROFILE_BYTES = 8_313`, `V3_FULL_WG_PROFILE_BYTES = 9_070`, `REQUIRED_REDUCTION_BYTES = 757`, `MAX_TOUCHED_OWNERS_BYTES = 6_810` (5 "touched owner" blocks), `V3_TOUCHED_OWNERS_BYTES = 7_567`. Current slack: full_wg 68, touched_owners 44 (6766). The fixture is deterministic per CI OS: `default_context(FAKE_REPLICA_ROOT, ...)` renders with `repo_mounts=None` and a fake root with no `.ac` ancestor → the platform block comes from the embedded default for the build's OS (§5.9). The budget test itself reads NO platform file.

### 3.7 Budget arithmetic for the final texts (§5.10) — exact, script-counted

- Current Windows paragraph (removed from messaging): 201 chars.
- New pointer in messaging: `"\n\n**Windows:** see **Host Platform Rules** above."` = 49 chars (Windows builds only).
- Windows platform default: **277 chars** (exact text §5.10, sha256 `5fd5dd5f7d3d097f90e58cee6e6a210e2b2a6070c24e4164a7ac06d3854286a7`).
- Linux/macOS platform defaults: **106 chars** each (sha256 `848e6bd25a001b091dd419fb665d2114abc5e5c2a19429fc820144aaa6190790` / `a305af4fce8148ac80d8e197b02142738a6056959e9b72bee43689b3d01cdc4d`).
- Compensating wording trims (exact before/after in §5.10): total **110 chars** (wait 15, help 7, invoke 12, auth 19, peer-format 13, recipient 44).
- Round-1 expectation (superseded by the measured values below): Windows `full_wg` = 8245 − 201 + 49 + 277 − 110 = 8260; Linux/macOS `full_wg` = 8245 − 201 + 106 = 8150; `touched_owners` = 6766 − 110 = 6656. The round-1 arithmetic omitted (a) the `\n\n` blank-line separators around the new block (+2) on every OS, (b) the −110 trims on Linux/macOS (they apply on every OS), and (c) the −201 + 49 Windows-paragraph swap in the touched-owners sum.
- **Measured (round 2 — implementation executed, protocol §5.10 verbatim; re-verified first-party by running `token_accounting_report` + the budget test at the amended head, Windows debug build, same fixture as §3.6)**: Windows `full_wg` = 8245 − 201 + 49 + 277 + 2 − 110 = **8262** (slack 51 ≤ 8313; reduction 9070−8262 = 808 ≥ 757). Linux/macOS `full_wg` = 8245 − 201 + 106 + 2 − 110 = **8042** (slack 271). `touched_owners` = 6766 − 201 + 49 − 110 = **6504** ≤ 6810 (slack 306; reduction 7567−6504 = 1063 ≥ 757).
- The fixture is deterministic: the budget test's fake roots resolve no `.ac`, so the render falls back to the embedded default (per-OS const); Windows is the binding case on the CI windows job, and the linux/macos jobs are strictly smaller. No budget constant changes.

## 4. Root cause / requirement statement

Today the Windows Git Bash routing rule exists only as a 201-byte paragraph inside `{{INTER_AGENT_MESSAGING}}` (#1596), hardcoded and not present on non-Windows messaging (where it is correctly absent). The owner requires: (a) a dedicated, mandatory, per-EXECUTION-platform block `{{HOST_PLATFORM_RULES}}` right after `{{CLI_CONTEXT}}` in the global template (v4→v5) — Windows host sessions get the Git Bash rule (bash.exe for shell work and every AC CLI invocation; PowerShell wrap `& 'C:\Program Files\Git\bin\bash.exe' -lc '...'`; never capture CLI output without `2>&1 | Out-String`), Linux/macOS host sessions get a minimal note, container sessions get nothing; (b) mandatory self-heal for preserved personalized templates lacking the token (append fallback, same as `{{AGENT_REPOS}}`); (c) NO duplication with the #1596 rule — the messaging section points at the platform block; (d) the byte budget measured and respected; (e) the block's content configurable by editing `.ac/Context.platform.<os>.md` (seeded absent-only, versioned in the seeded-template state, edited content preserved, missing/empty → embedded default + WARN, never an empty section on Windows); (f) docs in `docs/agents/`. The file lives in `.ac/` so the IaC repo can track it.

## 5. Design decisions (decision-complete; no TBD)

### 5.1 D1 — The placeholder, version bump, and freeze

- Add `{{HOST_PLATFORM_RULES}}` to `get_default_agent_template()` between `{{CLI_CONTEXT}}` and `{{SESSION_CREDENTIALS}}` (blank-line separated, exactly like the other tokens): v4 → **v5**. v5 draft bytes (informative): len 564.
- Freeze the CURRENT v4 accessor bytes as `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` with provenance doc (len 539, sha256 `f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a` at 047248bc) and a new byte-exact pin test (`global_before_host_platform_rules_snapshot_is_byte_exact`), mirroring the v3 pin. Never edit the frozen const.
- Add the v4 snapshot to BOTH `is_known_generated_global_template` and `is_known_generated_standalone_global_template` (a pristine v4 project template must auto-upgrade; a pristine v4 standalone template must stay recognizable for retirement).
- `project_specs()` global `current_version` 4 → 5. Coordinator stays 5, unchanged.

### 5.2 D2 — Coordinator: no template change

The coordinator body (`Context.coordinator.md`, plain text) is appended after the rendered global, and coordinator sessions render the global template (evidence §3.1.4). The block therefore reaches coordinator sessions through the global render. Adding the block to the coordinator body too would duplicate it. **No `get_default_coordinator_template()` change, no coordinator snapshot, no coordinator version bump.** The plan's test T-9 asserts a coordinator-shaped session profile contains the block exactly once.

### 5.3 D3 — Platform-file mechanism (owner amendment, hard requirement)

- Three new managed project templates, reusing `SeededContextTemplateSpec` and the whole `sync_one_template` lifecycle (absent-only seed, seeded/observed state, edit preservation, pending-update offer, auto-update of seeded files when a future default ships):

| id | filename | label | current_version | current_content | is_known_generated |
|---|---|---|---|---|---|
| `platform.windows` | `Context.platform.windows.md` | Windows host platform rules | 1 | `DEFAULT_HOST_PLATFORM_RULES_WINDOWS` | equality with the current default |
| `platform.linux` | `Context.platform.linux.md` | Linux host platform rules | 1 | `DEFAULT_HOST_PLATFORM_RULES_LINUX` | equality with the current default |
| `platform.macos` | `Context.platform.macos.md` | macOS host platform rules | 1 | `DEFAULT_HOST_PLATFORM_RULES_MACOS` | equality with the current default |

  All three specs: `project_actionable: true`, `suppress_unknown_without_state: true` (a pre-existing unowned file is preserved silently, never prompted — same posture as the global template). `project_specs()` returns the 5-element array (order: global, coordinator, then the three platform specs). `current_content` uses non-capturing closures (`|| DEFAULT_HOST_PLATFORM_RULES_WINDOWS`), which coerce to `fn() -> &'static str`.
- Defaults live as `pub(crate) const` in `session_context.rs` (single source for seed + render fallback), plus filename consts `HOST_PLATFORM_RULES_FILENAME_WINDOWS/LINUX/MACOS = "Context.platform.windows.md" / "Context.platform.linux.md" / "Context.platform.macos.md"` next to `COORDINATOR_CONTEXT_TEMPLATE_FILENAME`.
- When a future release changes a platform default, freeze the previous default as a snapshot const and extend its recognizer (exact same pattern as the global template generations), so seeded files auto-update and edited files are preserved with the pending-update offer. Documented in `docs/agents/host-platform-rules.md` (§6.8) and in the const provenance doc. Version 1 needs no snapshot.

### 5.4 D4 — State: reuse, no schema bump

The three platform entries are additive keys in the existing `templates` map of `.agentscommander-context-templates.json` (`STATE_SCHEMA_VERSION` stays 1; `SeededContextTemplateEntry` is unchanged). Forward/backward compatibility: an older build parses the new state file losslessly (same entry struct; map round-trips), a newer build reading an older state creates the entries on first `entry_mut`. No migration, no new state file, no new mechanism.

### 5.5 D5 — Render: read per materialization, no cache

New helper in `session_context.rs`:

```rust
fn render_host_platform_rules_block(agent_root: &str,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>) -> String
```

- `repo_mounts.is_some()` → return `String::new()` (container session: no host platform rules, no file read — the file is never mounted or injected for containers; the discriminator IS the execution platform).
- Host session: pick the filename by `cfg!(target_os)` (windows/linux/macos); resolve the project `.ac` via `resolve_ac_root_context_dir(Path::new(agent_root))` and read through `read_context_template` (regular-file + UTF-8 validated, symlink/reparse rejected, **no size cap** — the file is owner-authored content in their own repo; a cap would silently drop their bytes, worse than a large context; the byte budget binds the DEFAULTS, and an oversized custom file is the owner's deliberate choice).
- `Ok(Some(content))` and `!content.trim().is_empty()` → the file content verbatim (no cache: every materialization re-reads, so edit → respawn → new text without rebuild).
- Missing, empty, invalid UTF-8, non-regular file, or unresolvable `.ac` → `log::warn!` + embedded default for the build's OS; never an empty section on Windows.
- Called from `render_agent_context_template_inner` (value computed before the replace chain; `.replace("{{HOST_PLATFORM_RULES}}", &platform_rules)` after the `{{CLI_CONTEXT}}` replace) and from `render_root_runtime_prologue_inner` as the 10th block between `DEFAULT_CLI_CONTEXT` and `DEFAULT_SESSION_CREDENTIALS` (the existing `if block.is_empty() { continue; }` naturally drops it for container roots). Update the "Nine blocks (#979 G4)" comment to ten blocks.
- `MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS`: insert `"{{HOST_PLATFORM_RULES}}"` after `"{{CLI_CONTEXT}}"`. `mandatory_section_heading("{{HOST_PLATFORM_RULES}}") = "## Host Platform Rules"` (the file content includes the heading — the file IS the block, per the owner's "su contenido es lo que se rinde"). `mandatory_placeholder_aliases`: none. `coarse_section_dedup_safe("{{HOST_PLATFORM_RULES}}") = false` — never dedup an inline copy: the block is platform- and file-dependent, so an inline baked copy can be stale or platform-wrong; appending the current block is the safe side (mirrors `SKILLS_SECTION`/`AGENT_REPOS`). No existing template generation carries the heading, so no real-world duplication can occur.

### 5.6 D6 — Seed timing: project open/create/scan only, never read-time

Platform files are seeded by the existing `ensure_project_context_templates_with_clock` (project registration/create and project open) and scanned by `scan_project_context_template_updates_with_clock` (UI "newer default available" flow) — automatically, because they join `project_specs()`. They are deliberately NOT in the `is_managed_project_template` read-time-sync set and NOT synced by `sync_project_context_template_for_read`: the acceptance criterion "delete the file and respawn → embedded default + WARN" requires the render to observe the deletion, not to silently re-seed before reading. The next project open re-seeds a deleted file (absent-only). Standalone (app-config) seeding does NOT include platform files — they are project-level by design.

### 5.7 D7 — Seed-manifest scope

`manifest_scope_for_project_context_filename` gains a branch: the three platform filenames → `"context:platform"`. Platform seed publications are recorded under the held project gate like the other templates (fail-soft no-op on degraded permit, unchanged).

### 5.8 D8 — Retirement/auto-update interaction

`retire_standalone_global_context` and `remove_global_state_entry` touch only the standalone app-config global; platform files (project `.ac` only) are never retired, backed up, or clobbered by it. `is_known_generated_standalone_global_template` grows ONLY the v4 global snapshot (§5.1), never platform content. Platform recognizers are per-file equality with their current default (+ frozen snapshots added on future default changes, §5.3).

### 5.9 D9 — Budget fixture determinism

The budget test computes `full_wg` from `default_context(FAKE_REPLICA_ROOT, ...)` with `repo_mounts=None` and fake roots that have no `.ac` ancestor → the platform block comes from the embedded default of the build OS (Windows: 277; Linux/macOS: 106). The test needs NO fixture file and stays platform-deterministic per CI job; Windows is the binding case. The test adds one assertion: `full_wg.contains("## Host Platform Rules")` (all OSes — linux/macos render their minimal block too). `token_accounting_report` gains two rows ("block: host platform rules (Windows default)" / "(linux/macos default)"). Budget constants are untouched.

### 5.10 D10 — Exact texts and trims (byte-counted; apply verbatim)

**Platform defaults (embedded consts AND seeded file contents):**

Windows (277 bytes):
```
## Host Platform Rules

Windows host session: use `C:\Program Files\Git\bin\bash.exe` for all shell work and every AgentsCommander CLI invocation; from PowerShell wrap with `& 'C:\Program Files\Git\bin\bash.exe' -lc '...'`; never capture CLI output without `2>&1 | Out-String`.
```

Linux (106 bytes): `## Host Platform Rules` + blank line + `This session runs on a Linux host; no platform-specific shell routing rules apply.`
macOS (106 bytes): same with `macOS`.

**Messaging pointer (replaces `WINDOWS_SHELL_ROUTING`, cfg(windows), 49 bytes):** `\n\n**Windows:** see **Host Platform Rules** above.` — non-Windows stays `""`. Update the const's doc comment from #1596 to #1605 (single source of truth moved to the platform block). The pointer is the only Windows text left in messaging.

**Compensating trims (exact before → after, savings in parentheses):**

1. Messaging: `After sending, wait for the reply.` → `Wait for the reply.` (15)
2. `DEFAULT_CLI_CONTEXT`: `Use `--help` only for commands or flags not documented here:` → `Use `--help` only for undocumented commands or flags:` (7)
3. `DEFAULT_CLI_CONTEXT`: `Invoke only `AGENTSCOMMANDER_BINARY_PATH`; never hardcode or guess another executable.` → `Invoke only `AGENTSCOMMANDER_BINARY_PATH`; never guess another executable.` (12)
4. `DEFAULT_CLI_CONTEXT`: `The Inter-Agent Messaging section is authoritative for sending and peer discovery.` → `The Inter-Agent Messaging section is authoritative for sending.` (19)
5. Messaging peer-name-format line: `**Peer name format** (canonical FQN, the `list-peers-lean` `name` field):` → `**Peer name format** (canonical FQN from `list-peers-lean`):` (13)
6. Messaging: `The recipient reads the notified file path. ` → `` (44; the following `Do NOT use `--get-output` ...` sentence remains)

All six are wording-level; no instruction is deleted (the `--get-output` prohibition, the receipt rule, the FQN guidance, and the `--help`/invocation rules all stay). Verified: no unit test asserts any trimmed string (the asserted strings in `default_context_documents_env_only_credentials`, `default_context_documents_delegated_task_reporting`, `default_context_embeds_send_receipt_rule`, `default_context_embeds_filename_only_warning`, `default_context_documents_incoming_inter_agent_processing`, `default_context_embeds_fqn_format_and_filesystem_warning`, and the budget test's `for required in [...]` list are all untouched).

Round-1 expectation (superseded): `full_wg` = 8260 (slack 53), `touched_owners` = 6656 (slack 154, reduction 911), Linux/macOS `full_wg` = 8150 — the round-1 arithmetic omitted the `\n\n` separators and the touched-owners Windows swap (§3.7). **Measured post-change values (round 2, Windows debug build, same fixture as §3.6): `full_wg` = 8262 (slack 51), `touched_owners` = 6504 (slack 306, reduction 1063), Linux/macOS `full_wg` = 8042 (slack 271).** Protocol executed: the six trims applied verbatim, budget constants untouched, ceilings respected. If any future re-measurement differs by more than a couple of bytes, apply only the trims above verbatim, re-measure, and report the measured value with the discrepancy — do NOT change the budget constants; if the ceiling is still exceeded after the verbatim trims, stop and report to the tech-lead.

### 5.11 D11 — Doc and out-of-repo split

| Change | Location | Owner |
|---|---|---|
| Template, renderer, platform defaults/filenames, trims | `src-tauri/src/config/session_context.rs` | implementer (this PR) |
| Platform specs, state, recognizers, v4 freeze, sync coverage | `src-tauri/src/config/seeded_context_templates.rs` | implementer (this PR) |
| Manifest scope | `src-tauri/src/config/session_context.rs` | implementer (this PR) |
| Seed-manifest scope literals (round-2 S4) | `src-tauri/src/config/seed_manifest.rs` | implementer (this PR) |
| Operator doc | `docs/agents/host-platform-rules.md` (new) | implementer (this PR) |
| Seed/reseed platform files into existing projects, optional IaC commit of `.ac/Context.platform.*.md` | harness (outside repo) | tech-lead (harness operator), after release, per §7 |

## 6. In-repo change plan (file-by-file)

### 6.1 S1 — `src-tauri/src/config/session_context.rs`

(a) `get_default_agent_template()`: insert `{{HOST_PLATFORM_RULES}}` between `{{CLI_CONTEXT}}` and `{{SESSION_CREDENTIALS}}` (blank lines around it, like the other tokens).
(b) New pub(crate) consts: `HOST_PLATFORM_RULES_FILENAME_WINDOWS/LINUX/MACOS`, `DEFAULT_HOST_PLATFORM_RULES_WINDOWS/LINUX/MACOS` (§5.10 texts), `WINDOWS_SHELL_ROUTING` → the §5.10 pointer (cfg(windows); `""` otherwise) with an updated #1605 doc comment.
(c) New `fn render_host_platform_rules_block(agent_root, repo_mounts) -> String` (§5.5) + `fn host_platform_rules_filename()` / `host_platform_rules_default()` helpers (cfg-selected).
(d) `render_agent_context_template_inner`: add the platform block to the mandatory loop set via `MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS` (after `{{CLI_CONTEXT}}`), `mandatory_section_heading` (`"## Host Platform Rules"`), `coarse_section_dedup_safe` (false); compute `let host_platform_rules = render_host_platform_rules_block(agent_root, repo_mounts);` before the chain and add `.replace("{{HOST_PLATFORM_RULES}}", &host_platform_rules)` after the CLI_CONTEXT replace.
(e) `render_root_runtime_prologue_inner`: compute the block and add it as the 10th element between `DEFAULT_CLI_CONTEXT` and `DEFAULT_SESSION_CREDENTIALS`; update the "Nine blocks" comment to ten.
(f) Apply the six §5.10 trims to `render_inter_agent_messaging_block`, `default_context_dynamic_values` (peer-name-format line), and `DEFAULT_CLI_CONTEXT`.
(g) `manifest_scope_for_project_context_filename`: add the three platform filenames → `"context:platform"`.
(h) Tests (with the module): T-1..T-10, updates listed in §8.1.

### 6.2 S2 — `src-tauri/src/config/seeded_context_templates.rs`

(a) New `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` (exact v4 accessor bytes) with provenance doc (len 539, sha256 `f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a` at 047248bc); extend `is_known_generated_global_template` and `is_known_generated_standalone_global_template` with it.
(b) New `platform_specs()` returning the three `SeededContextTemplateSpec`s (§5.3); `project_specs()` returns 5 (global v5, coordinator v5, platform v1 each) — order: global, coordinator, windows, linux, macos.
(c) New recognizers `is_known_generated_platform_windows/linux/macos` (equality with the corresponding current default const).
(d) Tests: T-11..T-15 + the updates in §8.1 (version-pin tests 4→5, destructure test rewrite, v4 pin test, recognizer asserts, manifest scope assert).

### 6.3 S3 — `docs/agents/host-platform-rules.md` (new)

Content: what `{{HOST_PLATFORM_RULES}}` is and where it renders (global template after `{{CLI_CONTEXT}}`, root prologue, coordinator sessions via the global render; never in container sessions); the three files `.ac/Context.platform.windows.md` / `.linux.md` / `.macos.md` (path, format — the file content IS the rendered block including its `## Host Platform Rules` heading; absent-only seeding; edits preserved; deleting or emptying a file falls back to the embedded default with a WARN in app.log; how a new app default interacts with seeded vs edited files via `.agentscommander-context-templates.json`); how to apply a change (edit the file, respawn the session — no rebuild); and the note that the files are project-level and can be committed to the IaC repo.

### 6.4 S4 — `src-tauri/src/config/seed_manifest.rs` (round-2 amendment; executed in commit B, +7 lines, 0 deletions)

`validate_row` (~:1268) holds a CLOSED path→scope map — only `.ac/Context.AgentsCommander.md` and `.ac/Context.coordinator.md` — so the `context:platform` rows that D7 (§5.7) requires were rejected and the platform seed publications were dropped fail-soft; the test this plan itself mandates to update (§8.1, assert `scope = "context:platform"`) would fail without the change. The change: in the `ProjectContextTemplate` branch of `validate_row`, the three literals `.ac/Context.platform.windows.md` / `.ac/Context.platform.linux.md` / `.ac/Context.platform.macos.md` → `"context:platform"`, with a comment mirroring `manifest_scope_for_project_context_filename` (session_context.rs); the literals are deliberate — referencing `session_context` from `seed_manifest` would create a new module arc. Verified: 7 insertions, 0 deletions; Gate 1 re-run green at the implemented head (`cyclicSccs` identical set-to-set, `module-arcs.txt` byte-identical; §10).

### 6.5 No changes to

`cli/*`, `config_seed.rs`, `docs/agents/inter-agent-messaging.md`, workflows, `Cargo.*`, `package.json`, smoke scripts, the standalone retirement machinery, or any budget constant.

## 7. Out-of-repo handoff checklist (owner: ac-tech-lead-v3 as harness operator; NOT implementable in this PR)

1. After the release binary ships, existing projects re-seed `.ac/Context.platform.{windows,linux,macos}.md` absent-only at the next project open (automatic). For the IaC-tracked project (e.g. `D:\0_repos\AgentsCommander_iac`), optionally commit the three seeded files so the owner can edit them in-repo from day one.
2. Verify on this machine: edit `.ac/Context.platform.windows.md` in the IaC project, respawn a Windows replica, confirm the materialized `CLAUDE.md`/`AGENTS.md` carries the edited text; delete the file, respawn, confirm the embedded default appears and app.log records the WARN; container sessions (transport api) confirm no `Host Platform Rules` section.
3. No role-template or seed-template changes are needed (the block is rendered by the app, not by harness templates).

## 8. Tests and acceptance evidence

### 8.1 Unit/CI tests (implementer-run; exact-head CI is authoritative per §9.1)

New (session_context.rs):
- T-1 `default_context_embeds_host_platform_rules_block` (`#[cfg(windows)]`): `default_context("C:/fake/wg-7-dev-team/__agent_architect", None, &no_skill_section())` contains `## Host Platform Rules`, `bash.exe`, `2>&1 | Out-String`, and the `**Windows:** see **Host Platform Rules**` pointer.
- T-2 `default_context_non_windows_embeds_minimal_platform_block` (`#[cfg(not(windows))]`): contains `## Host Platform Rules` and NOT `bash.exe`/`2>&1 | Out-String`.
- T-3 `container_session_omits_host_platform_rules_block`: `render_agent_context_template_inner(v5 template, fake root, ..., Some(&RepoMountResolution::default()), false)` → `!out.contains("Host Platform Rules")` and no raw `{{` (uses the `RepoMountResolution::default()` empty-resolution fixture; `render_agent_repos_containerized` already handles empty entries).
- T-4 `root_prologue_embeds_host_platform_rules_block` (`#[cfg(windows)]`): `render_root_runtime_prologue_inner(FAKE_ROOT_AGENT, ..., None, None, true)` contains the Windows default text; T-5 container-root variant omits it.
- T-6 `personalized_template_without_host_platform_rules_token_appends_fallback`: a personalized template fixture WITHOUT the token (including a root-agent-shaped fixture per the owner's `Context.root-agent.md` probe) renders with exactly one `## Host Platform Rules` section (assert count 1 + no `{{`), mirroring the `{{AGENT_REPOS}}` fallback tests; also `custom_agent_template_is_used_for_wg_replica`-shaped coverage for the both-tokens pathological case (token present → single block).
- T-7 `host_platform_rules_reads_project_file_each_materialization` (all OSes): temp `Proj/.ac/wg-1-team/__agent_dev` layout; write `Context.platform.<os>.md` with custom text → render contains it; rewrite with different text → render again → new text (proves per-materialization read, no rebuild, no cache).
- T-8 `host_platform_rules_missing_or_empty_file_falls_back_to_embedded_default`: same layout, file absent → embedded default; file present but empty → embedded default.
- T-9 `coordinator_session_profile_embeds_host_platform_rules_once`: coordinator-shaped profile (replica + `# Orchestrator Context` + coordinator body) contains exactly one `## Host Platform Rules`.
- T-10 `assert_mandatory_sections_once` + `assert_no_raw_template_placeholders`-based fixtures: extend `assert_mandatory_sections_once` with `## Host Platform Rules` count 1; the existing "no unexpanded placeholders" and heading-count tests now cover the 8-token render automatically.
- `default_context_embeds_windows_shell_routing_paragraph` (existing, Windows): update assertions to the pointer + platform block (`**Windows:**`, `Host Platform Rules`, `bash.exe`, `2>&1 | Out-String` — drop the obsolete `ConvertFrom-Json` assert).
- `token_accounting_report` (`#[ignore]`): add the two platform-block rows.

New (seeded_context_templates.rs):
- T-11 `global_before_host_platform_rules_snapshot_is_byte_exact`: len 539 + sha256 `f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a`; and `assert_ne!` vs the v5 accessor; both recognizers accept the frozen v4 bytes; the project recognizer rejects a platform default (no widening).
- T-12 `read_sync_updates_pre_host_platform_rules_global_template` (failing-first, modeled on `read_sync_updates_pre_token_minimization_global_template`): pristine v4 template on disk → read-sync auto-upgrades to v5; state `currentVersion == 5`; one publication.
- T-13 `ensure_project_context_templates_seeds_platform_files_absent_only`: fresh `.ac` → three platform files byte-equal to their defaults; state entries `platform.windows/linux/macos` with `lastSeededSha256 == default sha`; a pre-existing custom platform file is NOT overwritten (absent-only) and lands in the observed/ignored posture.
- T-14 `platform_file_edit_is_preserved_and_observed`: seed → edit the file → `sync_one_template`/read-sync → content unchanged, state `lastObservedSha256` updated; a later default change would produce a pending update (scan path), never a silent overwrite.
- T-15 `project_specs_bump_global_to_v5_and_add_platform_specs` (rewrite of `project_specs_bump_only_the_global_template_to_v4`, which cannot compile with 5 specs): global v5, coordinator v5, three platform specs v1 with correct filenames/labels/recognizers.
- Update the six `currentVersion == 4` assertions to 5; update `context_create_records_both_project_templates_under_the_gate` name/comment + assert `scope = "context:platform"` (executed: renamed `context_create_records_project_templates_under_the_gate`, scope assert present and green; the S4 literal in `seed_manifest.rs` is what makes that assert pass, §6.4).

### 8.2 Acceptance criteria (owner + spec; objective)

1. Windows-rendered `CLAUDE.md`/`AGENTS.md` contains the platform block for claude, codex, and pi sessions; container sessions (transport api) do NOT contain it (T-3, T-1, T-2).
2. Personalized preserved templates without the placeholder receive the fallback block (T-6, incl. the `Context.root-agent.md`-shaped probe).
3. Editing `.ac/Context.platform.windows.md` and respawning a replica renders the edited text WITHOUT rebuild (T-7); deleting the file and respawning renders the embedded default with a WARN in app.log (T-8).
4. Seeded absent-only + edit preservation + versioned state (T-13, T-14); no duplication with the `{{INTER_AGENT_MESSAGING}}` rule (pointer only).
5. Byte budget measured (round 2): Windows `full_wg` = 8262 ≤ 8313 (slack 51), `touched_owners` = 6504 ≤ 6810 (slack 306), Linux/macOS `full_wg` = 8042 ≤ 8313 (slack 271) — budget test green, constants untouched, reductions 808/1063 ≥ 757 (§3.7).
6. The 3 manual gates, run by dev/grinch and reported in implementation/review:
   - rust levelization: no new SCC (arc record byte-identical, criterion §10);
   - layering guards (e.g. `loops_layering`, `instance_gitignore_layering`) green;
   - `check:frontend-dependencies` = 0.

### 8.3 Manual behavioral verification (implementer, on this machine, release binary)

1. Edit the IaC project's `.ac/Context.platform.windows.md` (e.g. append a line), respawn a replica, inspect the materialized `AGENTS.md`/`CLAUDE.md` → edited text present, no rebuild.
2. Delete the file, respawn → embedded default present; `app.log` contains the WARN line.
3. Container session (transport api): spawned context contains no `## Host Platform Rules` section.

### 8.4 Final Git/diff evidence (implementer, before PR) — round-2 amended path set

- `git status --porcelain` shows ONLY: the plan file (staged with `-f`), `src-tauri/src/config/session_context.rs`, `src-tauri/src/config/seeded_context_templates.rs`, `src-tauri/src/config/seed_manifest.rs` (round-2 addition, §6.4), `docs/agents/host-platform-rules.md`. Real state at round-2 verification (`d7008b34..HEAD`): `A plans/1605-host-platform-rules.md` staged + the three `.rs` modified + the new doc — nothing else.
- `git diff` is limited to the anchors in §6 (no budget constants, no state schema version, no `Cargo.*`, no workflow changes).
- Plan file force-added: `git add -f plans/1605-host-platform-rules.md`.

## 9. Delivery gates (baseline; evidence owner per gate)

Task class: routine (§2). No enhanced controls apply — recorded per the delivery-nonfunctional-invariants skill; host-tool provenance, signed-release, and hostile-host attestations are out of the accepted threat model and are NOT gates.

### 9.1 CI-to-plan parity

Source of truth: `.github/workflows/pr-regression-gates.yml` at `d7008b34` (the drifted base, §1; jobs: `test-debt`, `rust-regression` (windows: cargo check/clippy/`cargo test --lib --bins --tests`), `rust-regression-linux`, `rust-regression-macos`, `rust-fmt` (`cargo fmt --all -- --check`, added by #1608 in the drift), `terminal-snapshot-portable`, `windows-release-cli-smoke`, `frontend-regression`). The diff touches Rust (all three OS jobs exercise the platform-conditional render) + docs. Every triggered and configured-required check must pass on the exact PR-head SHA; evidence from another SHA, an unexplained skip, or a waiver does not satisfy the gate. `rust-regression` windows is the authoritative run for the binding budget case (full_wg 8262) and the `#[cfg(windows)]` tests; linux/macos jobs verify `full_wg` 8042 and the non-Windows defaults; `rust-fmt` verifies `cargo fmt --all -- --check` (exit 0 locally). Failure behavior: any red required check blocks delivery.

### 9.2 Deterministic toolchain and build

Repository contract: `Cargo.lock` committed; `npm ci` pinned (`npm@11.6.2`); stable toolchain via `dtolnay/rust-toolchain@stable`. Local: `--locked` for all cargo commands; record `rustc --version`/`cargo --version` in the PR. Expected: resolution from the lockfile; the platform blocks are compile-time consts, so builds are byte-deterministic per OS. Failure behavior: resolution/build errors block.

### 9.3 Authorized, traceable Git

Issue #1605 OPEN; branch `feat/1605-host-platform-rules` created from `main` @ `047248bc` (verified §1). State-changing Git runs only inside `repo-AgentsCommander`; delivery via PR, never direct push to `main`. Plan file force-added (§1). Preconditions: clean tree + pinned base before the first product mutation, re-fetched before PR creation (§1 ritual). Failure behavior: unknown/dirty base, missing issue linkage, or scope drift blocks readiness.

### 9.4 Process state, configuration, working directory

No inherited env/config materially changes the commands (cargo/npm standard). All mutating and cwd-sensitive commands run from the repo root with explicit paths. Expected: reproducible output. Failure behavior: cwd drift or ambient config interference is recorded and fixed before acceptance.

### 9.5 Validation and scope before acceptance

Frozen path set (§8.4). Postcondition: `git status --porcelain` matches §8.4; the budget fixture needs no platform file (deterministic per OS, §5.9). Failure behavior: any file outside the set, or any change to budget constants / state schema version / workflows / Cargo files, is scope drift → stop and report.

### 9.6 Mutation ownership and no-clobber recovery

The implementer is the only writer on this branch during implementation. Before writes: recheck frozen base + clean status. On failure: restore only paths this run changed, and only when their current state is demonstrably that run's output (`git diff`/`git restore -- <file>` on the specific file); never broad `git reset`/`git restore` of the tree; preserve and report any externally changed bytes. The plan file is added with `-f` only. Success: prove the §8.4 path set, index state (plan file staged), and ordinary-untracked state (nothing else).

### 9.7 Bounded execution and durable diagnostics

`cargo test`/`cargo clippy` run with a runner timeout (≥30 min cold, ≥15 min warm) and non-interactive stdin; retain stdout/stderr + exit/timing context. A timed-out or cancelled run is never reported as success. Failure behavior: timeout/cancel → rerun with the recorded diagnostics; cleanup defects must not erase the primary failure.

### 9.8 Evidence discipline

Zero and absence are valid typed states: "no platform file" → embedded default + WARN (asserted, T-8); "empty platform file" → embedded default (T-8); "container session" → no block (T-3); "personalized template without token" → fallback append (T-6); "platform file present" → verbatim content (T-7). Each gate states its expected result and failure behavior; remote-only evidence is owned by the exact-head CI checks (§9.1).

## 10. Dependency-cycle and layering statement (planning rule 8)

Enumerated arcs introduced by this plan:

- `config/session_context.rs`: new private helpers (`render_host_platform_rules_block`, `host_platform_rules_filename/default`) using only existing same-module functions (`resolve_ac_root_context_dir`, `read_context_template`) and new `pub(crate)` consts in the SAME module; a new branch in `manifest_scope_for_project_context_filename` (same module). **NO new module arc.**
- `config/seeded_context_templates.rs`: three new `SeededContextTemplateSpec`s whose `current_content`/`is_known_generated` reference `crate::config::session_context::{DEFAULT_HOST_PLATFORM_RULES_*, HOST_PLATFORM_RULES_FILENAME_*}` — the module arc `seeded_context_templates → session_context` ALREADY exists (`current_content: crate::config::session_context::get_default_agent_template` and `get_default_coordinator_template` in `project_specs()` at :477-505); the render path does NOT call into `seeded_context_templates` for platform files (direct read), so no new arc in either direction. **Zero new module-to-module arcs**; `cyclicSccs` and every SCC member set cannot change; there is no cross-boundary arc to classify.
- Role/layering hygiene: the file read and text consts stay in the config layer (file seeding and context rendering are already its job); no lower layer gains a UI-transport/`AppHandle`/tauri dependency; no `cli/*` or `pty/*` changes.
- `config/seed_manifest.rs` (round-2 S4, §6.4): the three `.ac/Context.platform.*.md → "context:platform"` entries are string literals in the existing closed map — no module reference, hence no new arc (a `session_context` import was deliberately avoided). Gate 1 re-verified at the implemented head: `cyclicSccs` identical set-to-set, regenerated `module-arcs.txt` byte-identical (1037 arcs, empty `git status` on it).

Acceptance criterion for the implementer (base `047248bc` vs final branch head, clean tree for both):

```
node "<VAULT>/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "<VAULT>/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

Green iff: (1) `cyclicSccs` equal pre/post; (2) cyclic SCC member sets identical set-to-set; (3) zero new `from -> to` pairs cross a previously-clean SCC boundary; (4) regenerated `module-arcs.txt` byte-identical (empty `git status` on it); (5) structural layering guards (e.g. `loops_layering`, `instance_gitignore_layering`) stay green. Exit code 1 from the detector is the normal gating outcome; only exit 3 means no graph.

## 11. Implementation order

1. Commit A — S2 seed layer first (v4 freeze const + recognizers + platform specs + state/sync tests): establishes the frozen operand and the file lifecycle the renderer will read.
2. Commit B — S1 render layer (template v5, placeholder machinery, `render_host_platform_rules_block`, root 10th block, pointer + trims, manifest scope) + S4 `seed_manifest.rs` scope literals (§6.4) + its tests.
3. Commit C — S3 docs.
4. Force-add this plan file and run the full §8 battery: `cargo test --locked --lib --bins --tests` (Windows — binding budget case), clippy, check, `cargo fmt --all -- --check`, budget test (measured 8262/6504/8042, §3.7), manual §8.3 checks, §10 dependency-cycle gate.
5. Open/refresh the PR with the §9.3 evidence; report exact-head CI results (windows/linux/macos budget rows).

## 12. Residual risks and accepted tradeoffs (decision-complete)

- **Byte budget tightness**: slack drops from 68 to 51 bytes on Windows `full_wg` (round-2 measured 8262; the +2 `\n\n` separators around the new block, omitted in round-1 arithmetic, cost the last 2 bytes — cosmetic, within ceiling: slack 51/306/271, §3.7). The fixture is deterministic and the trims are exact; if re-measurement deviates, the verbatim-trim re-measure protocol (§5.10) applies — constants never change.
- **A customized platform file can exceed the budget**: the ceiling binds the DEFAULTS; an owner-authored longer file is their deliberate choice (no size cap, §5.5). Accepted and documented.
- **Deleted platform file between project opens**: renders the embedded default + WARN until the next project open re-seeds (absent-only). This is the required behavior (§5.6).
- **First upgrade on an existing project**: platform files appear only after the next project open/ensure; until then the render falls back to the embedded default (identical content). No gap.
- **Future default change on platform files**: seeded files auto-update only after the previous default is frozen into the recognizer (documented extension rule, §5.3); until then they are preserved with a pending-update offer rather than silently overwritten — the safe side of the owner requirement.
- **`Context.root-agent.md` (root role template)**: untouched; the root gets the block from the code-owned prologue (10th block), never from the editable template. Root sessions in the installed layout (no `.ac` ancestor) use the embedded default; in a `.ac`-nested layout they read the project file. Deterministic per install.
- **Coordinator**: block arrives via the global render; the coordinator template is unchanged (no duplication).
- **Non-Windows**: Linux/macOS host sessions render the minimal block (106 bytes); messaging pointer and Windows default are `#[cfg]`-gated, so no behavior change on non-Windows beyond the new minimal section.
- **GUI behavior**: untouched — no `main.rs`, no Tauri surface, no binary, no packaging changes.
