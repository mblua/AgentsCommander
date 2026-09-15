# Plan #2031: seed the Answering section into the project global context template

Status: READY_FOR_IMPLEMENTATION

Issue: `#2031` (OPEN), "Seed an Answering section into the project global context template"
Repository: `repo-AgentsCommander`
Base: `main` = `329ab94e`; branch `feature/2031-seed-answering-section` (local HEAD = `origin` = `329ab94e`)
Band: Lite 1-25 (issue score 16, verification veto; Grinch reviews the proof). Author: `ac-dev-rust-v4`. Reviewer: Grinch, assigned by the coordinator.
Task class: one literal in `get_default_agent_template()` plus tests in the two config modules that own the template. No IPC, no frontend, no persistence-format change, no migration.

## 1. Objective and evidence

Goal: the Answering rule must ship in the built-in default template, so every project and every replica gets it from source, instead of only the one installation that carries a machine-local `.ac/Context.AgentsCommander.local.md`.

Discovery method: codebase-memory graph on this working copy at `329ab94e` (24,225 nodes / 162,551 edges, indexed `2026-09-15T14:00:38Z`); `check_index_coverage` returned `no_recorded_issue` for every cited Rust file, and those files were then read directly; `docs/` is excluded from the index by design and was read directly. The two zero-match claims were each confirmed twice (plain grep and graph-augmented search), per the codebase-memory rule that a zero is not a finding until a second shape agrees.

| Fact | Where (measured at `329ab94e`) |
|---|---|
| The rule is absent from source | `grep -rn "Report both counts" src-tauri/src` = 0 matches; `grep -rn "## Answering"` = 0; graph-augmented search for `Report both counts` = 0 |
| The built-in default is one raw string ending at the messaging token | `get_default_agent_template()`, `src-tauri/src/config/session_context.rs:2679-2705`; body = 559 bytes, sha256 `ee456d60802157ecc8a5f5c5c97522442276524e1f4979481326a80cc6ec5f09`; last content line `{{INTER_AGENT_MESSAGING}}` at 2703, raw string closes at 2704 |
| The rule reaches agents today only through the operator override | `local_overlay.rs:1-10` (markdown overlay layer); `session_context.rs:1308-1321` (`read_context_local_override`); `:3002-3008`: when `<base>.local.md` exists, its bytes are rendered instead of the base file's bytes |
| Project `global` spec | `seeded_context_templates.rs:625-635`: `current_version: 6`, `is_known_generated: None`, `distribution_owned: true` |
| Distribution-owned repair is byte-keyed and scan-only | `sync_one_template`, `seeded_context_templates.rs:1323-1406`: any file bytes != current default are backed up (`create_backup`) and atomically replaced; size and version are never consulted. The read path defers (`:1324-1332`) |
| Replacement notification rule | `seeded_context_templates.rs:1357-1360`: `notify = last_seeded_sha256 != snapshot.sha256`; silent when the replaced bytes are the ones we last wrote |
| The scan is what runs the repair | `ac_discovery.rs:1033-1068` (`scan_project_context_templates_recorded`), called from project discovery at `:1163` and `:1988` |
| Root never reads the template | `session_context.rs:3413-3458` (`ROOT_RUNTIME_PROLOGUE_HEADER` and the code-owned prologue); `docs/agent-matrix-conventions.md:38` ("The Root Agent does not use the global template (#979)") |
| The issue's measurement commit agrees with base | `git show f9ee9f6:src-tauri/src/config/session_context.rs` -> same 559 bytes, same sha256; `f9ee9f6` is an ancestor of `329ab94e` |

## 2. Cause

The shipped default template carries no Answering rule. Only a project whose operator authored a `.local.md` override receives it, because the override layer replaces the base content at render time. The fix is to seed the rule into the default, which is the file every project receives.

## 3. Decisions from the code

### 3.1 `current_version` stays 6; nothing else in the spec changes

Bumping the version is unnecessary and would persist nothing. Proof from code:

- `LoadedState::entry_mut`, `seeded_context_templates.rs:552-570`: for a `distribution_owned` spec the recorded version is `(!spec.distribution_owned).then_some(spec.current_version)`, i.e. `None`. A bump would never reach disk: the tests at `:3291`-style assertions already pin `"templates"."global"."currentVersion" == null`.
- `sync_one_template`, `:1323-1406`: the distribution-owned branch decides only on `snapshot.sha256 != current_default_sha256`; no version, no recognizer.
- `compute_pending_update`, `:1570-1579`: returns `Ok(None)` for `distribution_owned` before any version is read, so `make_update`'s `current_default_version` (`:1142`) is unreachable for `global`.
- The only other reads of `global.current_version` are the tests `seeded_template_versions_were_bumped` (`:2549-2554`) and `project_specs_bump_global_to_v6_and_add_platform_specs` (`:2634`). A bump would force both to change for zero behavior change.

Decision: leave the global spec exactly as it is. Only the template body and tests change.

### 3.2 Freeze the pre-Answering bytes and extend the standalone recognizer

Two parts, both decided:

1. Add `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION` holding the exact 559 bytes shipped up to and including `329ab94e`. It is the fixture that makes the scan test write the bytes a real installation holds today; deriving the fixture from the live function would be self-referential and would silently pass under any edit.
2. Add it as one arm of `is_known_generated_standalone_global_template` (`seeded_context_templates.rs:744-760`). That classifier is the only consumer of frozen global predecessors (`is_known_generated_standalone_global_template` at `:751`, used by app-config retirement classification at `:2048-2052`): recognized bytes are deleted after retirement; unknown bytes are kept as an inert backup. Leaving the predecessor out would silently recategorize bytes we generated as custom. The project replacement path does not consult it (the `global` spec names no recognizer), so the issue's "no recognizer extension is needed" statement remains true for project files; this arm governs only app-config retirement.

## 4. Exact changes

### 4.1 `src-tauri/src/config/session_context.rs`, `get_default_agent_template()` (insert after line 2703)

Before:

```rust
{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
}
```

After:

```rust
{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}

## Answering

Draft, count words, then rewrite to half or fewer; report both counts. Paths, commands, digests, figures, and quoted evidence are exempt: not counted, not cut. Never cut a fact to reach half; stop at the smallest size that keeps every fact and say so.

Use plain, easy language.
"#
}
```

Six added lines; the function body then ends at line 2711. No placeholder is added, removed, or reordered. The text carries no U+2014 (whole template measured at 0 today) and no `{{`/`}}`.

### 4.2 New tests in `session_context.rs` (`mod tests`)

Insert after `default_agent_template_keeps_coarse_placeholder_order_after_summarization` (ends at line 5223):

```rust
    /// #2031: the shipped default carries the Answering rule exactly once, after
    /// the messaging token, byte-for-byte, with no U+2014.
    #[test]
    fn default_agent_template_carries_answering_section_once_after_messaging() {
        const SECTION: &str = "## Answering\n\nDraft, count words, then rewrite to half or fewer; report both counts. Paths, commands, digests, figures, and quoted evidence are exempt: not counted, not cut. Never cut a fact to reach half; stop at the smallest size that keeps every fact and say so.\n\nUse plain, easy language.";
        let template = get_default_agent_template();
        assert_eq!(template.matches(SECTION).count(), 1, "{template}");
        let messaging = template
            .find("{{INTER_AGENT_MESSAGING}}")
            .expect("messaging token");
        let answering = template.find(SECTION).expect("answering section");
        assert!(
            messaging < answering,
            "## Answering must follow {{INTER_AGENT_MESSAGING}}"
        );
        assert!(
            !template.contains('\u{2014}'),
            "the default template must stay em-dash-free"
        );
    }
```

Insert next to the other materialization tests (e.g. after `materialized_context_gates_self_maintenance_directive_by_flag`, line ~9320):

```rust
    /// #2031: a rendered replica context file carries the Answering section once.
    #[test]
    fn materialized_replica_context_carries_answering_section() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        let matrix_root = ac_root.join("_agent_dev-rust");
        std::fs::create_dir_all(&matrix_root).expect("create matrix root");
        let cwd = path_string(&matrix_root);
        for filename in ["CLAUDE.md", "AGENTS.md"] {
            let path =
                materialize_agent_context_file_with_filename(&cwd, filename, &[], false, false, None)
                    .expect("materialize")
                    .expect("context path");
            let content = std::fs::read_to_string(&path).expect("read context");
            assert_eq!(
                count_section_headings(&content, "## Answering"),
                1,
                "{filename}: {content}"
            );
            assert!(content.contains("Use plain, easy language."), "{filename}");
        }
    }
```

Insert with the Root prologue tests (after `root_prologue_renders_every_mandatory_block_exactly_once`, line ~7064):

```rust
    /// #2031 negative case: the canonical Root Agent never reads the global
    /// template, so its code-owned prologue must not carry the Answering section.
    #[test]
    fn root_runtime_prologue_omits_answering_section() {
        let out = render_root_runtime_prologue_inner(
            "C:/fake/ac-root-agent",
            &no_skill_section(),
            Path::new("C:/fake/ac-root-agent"),
            None,
            None,
            true,
        );
        assert_eq!(count_section_headings(&out, "## Answering"), 0, "{out}");
        assert!(!out.contains("Report both counts"), "{out}");
        // Control: the same render helper on a replica does carry it, so the zero
        // above is the Root path and not a broken renderer.
        let replica = default_context(
            "C:/fake/room-1-ac-dev-team-v4/__agent_ac-dev-rust-v4",
            None,
            &no_skill_section(),
        );
        assert_eq!(
            count_section_headings(&replica, "## Answering"),
            1,
            "{replica}"
        );
    }
```

### 4.3 `src-tauri/src/config/seeded_context_templates.rs`: frozen const (after `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME`, which ends at line 404)

Exact content, byte-for-byte:

```rust
/// #2031: the global context template as shipped through `329ab94e`, the
/// generation every existing project file and every existing standalone app-config
/// file holds. The Answering seed turns it into a generated predecessor: it is the
/// fixture for the scan replacement test, and it MUST stay recognized by the
/// standalone #979 classifier so retirement still deletes our own bytes instead of
/// reclassifying them as custom. Never edit.
/// Provenance: the 329ab94e blob, `session_context.rs` lines 2679-2705; value
/// 559 bytes sha256 EE456D60802157ECC8A5F5C5C97522442276524E1F4979481326A80CC6EC5F09;
/// pinned by `global_before_answering_snapshot_is_byte_exact`.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION: &str = r#"# AgentsCommander Context

You are in AgentsCommander, a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and which repos are available.
- **Room**: a runtime replica of a team for a specific task. It contains replica agents and `repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{HOST_PLATFORM_RULES}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;
```

### 4.4 `seeded_context_templates.rs`: recognizer arm

```rust
fn is_known_generated_standalone_global_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_agent_template()
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        || content == STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
}
```

### 4.5 New tests in `seeded_context_templates.rs` (`mod tests`, next to the other global-snapshot tests)

```rust
    #[test]
    fn global_before_answering_snapshot_is_byte_exact() {
        assert_eq!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION.len(),
            559,
            "frozen pre-Answering global snapshot must be the 329ab94e bytes"
        );
        assert_eq!(
            hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION),
            "ee456d60802157ecc8a5f5c5c97522442276524e1f4979481326a80cc6ec5f09",
            "frozen pre-Answering snapshot changed; it must stay byte-identical to what shipped"
        );
        assert!(
            !GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION.contains("## Answering"),
            "control: the frozen predecessor must not already carry the section"
        );
    }

    #[test]
    fn frozen_pre_answering_global_template_is_recognized() {
        assert!(is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION
        ));
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
            crate::config::session_context::get_default_agent_template(),
            "the Answering seed must actually change the global default or the freeze is pointless"
        );
        assert!(crate::config::session_context::get_default_agent_template()
            .contains("## Answering"));
    }

    #[test]
    fn scan_replaces_pre_answering_global_template_and_backs_it_up() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
        )
        .expect("write pristine pre-Answering global");

        let replacements =
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan pre-Answering global");
        assert_eq!(
            replacements.len(),
            1,
            "with no state entry the replacement is notified"
        );
        let current = crate::config::session_context::get_default_agent_template();
        let content = std::fs::read_to_string(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        )
        .expect("read repaired global");
        assert_eq!(content, current);
        assert_eq!(content.matches("## Answering").count(), 1);
        let backups = backup_files(&ac_root);
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(
            std::fs::read_to_string(&backups[0]).expect("read backup"),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
            "the backup must hold the pre-run bytes"
        );
        assert!(
            scan_project_context_template_updates(temp.path(), &ac_root)
                .expect("scan updates")
                .is_empty(),
            "a distribution-owned template never yields a pending update"
        );
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read seeded state"),
        )
        .expect("parse seeded state");
        assert_eq!(
            state["templates"]["global"]["currentVersion"],
            serde_json::Value::Null
        );
        assert_eq!(
            state["templates"]["global"]["lastSeededSha256"],
            hash_text(current)
        );
        assert!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("second scan")
                .is_empty()
        );
        assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
    }

    #[test]
    fn scan_replaces_pre_answering_global_template_silently_with_trusted_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
        )
        .expect("write pristine pre-Answering global");
        let mut state = SeededContextTemplateState::default();
        state.templates.insert(
            "global".to_string(),
            SeededContextTemplateEntry {
                template_id: "global".to_string(),
                current_version: None,
                last_seeded_sha256: Some(hash_text(
                    GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
                )),
                last_observed_sha256: None,
                ignored_default_sha256: None,
                ignored_observed_sha256: None,
            },
        );
        persist_state(&ac_root, &state).expect("persist trusted pre-Answering state");

        let replacements =
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan pre-Answering global");
        assert!(
            replacements.is_empty(),
            "a trusted entry naming these exact bytes makes the repair silent"
        );
        let current = crate::config::session_context::get_default_agent_template();
        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("read repaired global"),
            current
        );
        assert_eq!(backup_files(&ac_root).len(), 1, "silent is still backed up");
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read seeded state"),
        )
        .expect("parse seeded state");
        assert_eq!(state["templates"]["global"]["lastSeededSha256"], hash_text(current));
        assert!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("second scan")
                .is_empty()
        );
        assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
    }
```

### 4.6 Files touched by the implementation

- `src-tauri/src/config/session_context.rs` - the template literal plus the three tests in 4.2.
- `src-tauri/src/config/seeded_context_templates.rs` - the const (4.3), the arm (4.4), the four tests (4.5).
- No other source, test, doc, config, or frontend file. `docs/agent-matrix-conventions.md` lists placeholder tokens, not section headings, and this change adds no token, so it stays as is.

## 5. Behavior and edge cases

- Next project scan: `scan_project_context_templates_with_clock` (`seeded_context_templates.rs:1781-1830`) iterates `project_specs()`, so the `global` file is backed up and replaced whenever its bytes differ from the new default. This includes pristine old defaults, CRLF copies, near matches (the existing policy at `:3349`), and operator-customized files (distribution-owned repair has no preserve path; unchanged pre-existing #1748 behavior).
- Notification: the replacement notice (`ContextTemplateReplacement`) is emitted only when the replaced bytes were not the ones we last seeded (`:1357-1360`). A real upgrade has state `lastSeededSha256 == old default`, so the repair is silent but still backed up. A missing or untrusted state entry notifies.
- Read path: `sync_project_context_template_for_read` (`:1833-1882`) passes `repair = false`, so a stale project file is not rewritten on spawn; it is repaired on the next scan. If the file is absent, the read path creates the current default (`create_missing_template`, `:1146-1167`), so the section appears immediately.
- Second scan: state records the new default hash, the file matches, so no replacement and no new backup (both scan tests assert this).
- `.local.md` override: if `.ac/Context.AgentsCommander.local.md` exists, `session_context.rs:3002-3008` renders it instead of the repaired base file. This plan does not touch that file; the issue's manual follow-up (delete its Answering lines, or the whole file if the `## Core Concepts WOOW` heading is unintended) stays an operator action.
- Root Agent: unaffected; its prologue is assembled from code (`session_context.rs:3443-3458`) and pinned by 4.2's negative test.
- Orchestrator sessions: `Context.coordinator.md` is unchanged; orchestrators receive the global base plus the appended coordinator body, so they see the section through the base.
- Symlinks, directories, invalid UTF-8: the scan keeps its existing error behavior; nothing in this change alters it.

## 6. Existing tests: impact (expected: none break)

| Test | Why it stays green |
|---|---|
| `default_agent_template_keeps_coarse_placeholder_order_after_summarization` (`session_context.rs:5175`) | The placeholder vector is unchanged; `ends_with('\n')` still holds (the raw string still ends with a newline) |
| `assert_mandatory_sections_once` (`:4802`) and the heading-count tests (`:5097-5160`) | They count mandatory placeholder headings; `## Answering` is not a placeholder and is not in the checked set |
| `seeded_template_versions_were_bumped` (`seeded_context_templates.rs:2549`), `project_specs_bump_global_to_v6_and_add_platform_specs` (`:2634`) | `current_version` is not bumped (3.1) |
| `global_*_snapshot_is_byte_exact` and `both_frozen_global_generations_are_standalone_recognized_and_distinct` (`:2497`) | Existing frozen consts are untouched; each is still `!=` the new default |
| `scan_replaces_pre_token_minimization_global_template` (`:3084`), `scan_replaces_v3_global_near_matches_in_both_project_state_shapes` (`:3349`) | Still `assert_ne!` against the default; the replacement path is unchanged |
| `coordinator_template_carries_cross_workgroup_rule` (`session_context.rs:9237`) | The new text contains no coordinator rule, so `!get_default_agent_template().contains(RULE)` still holds |
| `frozen_pre_room_rename_global_template_is_recognized` (`:2464`) | It asserts the default has no "workgroup"; the new text has none |
| `root_prologue_*` tests (`session_context.rs:5917-7100`) | Root does not read the template; its ten blocks are unchanged |

No test pins the length or the sha256 of the current default: `grep` for `get_default_agent_template().len()` / a hash of it returns nothing. The existing length/hash tests pin only frozen consts.

## 7. Verification and acceptance mapping

### 7.1 Commands

```
cd repo-AgentsCommander
cargo test -p agentscommander --lib config::session_context::tests::default_agent_template_carries_answering_section_once_after_messaging
cargo test -p agentscommander --lib config::session_context::tests::materialized_replica_context_carries_answering_section
cargo test -p agentscommander --lib config::session_context::tests::root_runtime_prologue_omits_answering_section
cargo test -p agentscommander --lib config::seeded_context_templates::tests::global_before_answering_snapshot_is_byte_exact
cargo test -p agentscommander --lib config::seeded_context_templates::tests::frozen_pre_answering_global_template_is_recognized
cargo test -p agentscommander --lib config::seeded_context_templates::tests::scan_replaces_pre_answering_global_template_and_backs_it_up
cargo test -p agentscommander --lib config::seeded_context_templates::tests::scan_replaces_pre_answering_global_template_silently_with_trusted_state
cargo test -p agentscommander --lib
```

All pre-existing tests must stay green with no edits.

### 7.2 Negative control (exactly as the issue asks: the same test on the pre-change revision leaves the section absent)

```
cd repo-AgentsCommander
git stash push -- src-tauri/src/config/session_context.rs
cargo test -p agentscommander --lib config::seeded_context_templates::tests::scan_replaces_pre_answering_global_template_and_backs_it_up
git stash pop
```

Expected on the stashed run: FAIL. With the literal reverted, the on-disk fixture equals `get_default_agent_template()`, so `sync_one_template` takes the `AlreadyCurrent` branch (`seeded_context_templates.rs:1308-1310`): `replacements.len() == 0`, no backup, and the file still lacks `## Answering`. That is the issue's control, reproducible at HEAD because `f9ee9f6` and `329ab94e` hold byte-identical templates (section 1).

### 7.3 Acceptance mapping

| Issue acceptance criterion | Proof |
|---|---|
| `get_default_agent_template()` contains `## Answering` exactly once, after `{{INTER_AGENT_MESSAGING}}`, with no U+2014; pinned by a test | 4.1 plus `default_agent_template_carries_answering_section_once_after_messaging` (whole template asserted em-dash-free, measured 0 today) |
| A project `.ac/Context.AgentsCommander.md` holding the previous default is replaced on scan with a backup; control: the pre-change revision leaves it absent | `scan_replaces_pre_answering_global_template_and_backs_it_up` (notified path) and `..._silently_with_trusted_state` (real upgrade path), plus the 7.2 probe and the fixture assertion in `global_before_answering_snapshot_is_byte_exact` |
| A rendered replica `CLAUDE.md`/`AGENTS.md` contains the section; a rendered Root context does not | `materialized_replica_context_carries_answering_section` and `root_runtime_prologue_omits_answering_section` |
| `cargo test` green | 7.1 |

## 8. Commit and handoff

- Implementation commit (by the implementer, on `feature/2031-seed-answering-section`): `feat(#2031): seed the Answering section into the project global context template` - both source files from 4.6 in one commit, then run 7.1 and 7.2.
- This plan's own commit: `docs(plan): #2031 seed the Answering section into the project global context template`; `plans/` is gitignored, so stage with `git add -f plans/2031-seed-answering-section.md`, push the branch, and confirm `git ls-remote --heads origin feature/2031-seed-answering-section` reports a SHA other than `329ab94e`.
- Rollback: revert the implementation commit. The template returns to 559 bytes; the next scan repairs project files back to the previous default with a backup. The frozen const and recognizer arm are inert for the rolled-back state (the const would no longer be a predecessor, but it stays recognized, which is harmless).

Status: READY_FOR_IMPLEMENTATION
