# Plan #2031: seed the Answering section into the project global context template

Status: READY_FOR_IMPLEMENTATION

Issue: `#2031` (OPEN), "Seed an Answering section into the project global context template"
Repository: `repo-AgentsCommander`
Base: `main` = `329ab94e`; branch `feature/2031-seed-answering-section` (local HEAD = `origin` = `329ab94e`)
Band: Lite 1-25 (issue score 16, verification veto; Grinch reviews the proof). Author: `ac-dev-rust-v4`. Reviewer: Grinch, assigned by the coordinator.
Task class: one literal in `get_default_agent_template()` plus tests in the two config modules that own the template. No IPC, no frontend, no persistence-format change, no migration.
Revision: round 2 (`0c30f080` got CHANGES_REQUIRED): add the size-budget V6 rung (section 4.6), make the negative control order-independent and miscompile-proof (7.2), fix the Root test comment (4.2). Plan only, no code.

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
| The template body is size-budgeted by a test | `token_accounting::summarized_default_context_meets_size_budget` (`session_context.rs:13006`): rung constants at `:13013-13032`, pre-#1795 gates at `:13099-13107`, current gates at `:13208-13226` |
| The section is +294 bytes and breaks exactly one test | Grinch applied the round-1 plan: `4436 passed; 1 failed`; first failure at the pre-#1795 full gate "pre-#1795 WG profile is 9386 bytes against v4 ceiling 9109", then the V5 full-profile gate (10220) by 277. `touched_owners` does not move: it sums the dynamic blocks, not the template body |

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
        // Control: `default_context` renders a replica through a different
        // function (`render_default_agent_context`) and does carry the section,
        // so the zero above is the Root path, not a broken renderer.
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

### 4.6 `session_context.rs`, `token_accounting::summarized_default_context_meets_size_budget`: add the V6 rung

The Answering section is template body text, so it grows the full WG profile by exactly 294 bytes and leaves `touched_owners` unchanged: `touched_owners` sums `write_restrictions`, `messaging`, `DEFAULT_CLI_CONTEXT`, `DEFAULT_SESSION_CREDENTIALS`, `DEFAULT_DELEGATED_TASK_REPORTING`, and the section is in none of them. Measured by Grinch on the round-1 patch: `pre_full_wg` = 9386 (V4 ceiling 9109), `full_wg` = 10497 (V5 full ceiling 10220), delta 294, headroom 17. Only the two full-profile gates fail; the touched gates stay green untouched. Post-fix expected: `pre_full_wg` 9386 <= 9403, `full_wg` 10497 <= 10514 (reduction 774 >= 757), both touched gates unchanged and green.

Exact new integers:

- `V6_FULL_WG_PROFILE_BYTES` = `11_271` (`10_977 + 294`)
- `V6_MAX_FULL_WG_PROFILE_BYTES` = `10_514` (`10_220 + 294`)
- `V6_PRE_1795_MAX_FULL_WG_PROFILE_BYTES` = `9_403` (`9_109 + 294`)
- `V6_DELTA_BYTES` = `294`; `V6_PRE_TEMPLATE_BYTES` = `559`

(a) Insert after the `V5_MAX_FULL_WG_PROFILE_BYTES` declaration at line 13032:

```rust
        // #2031 V6 generation: the Answering section is TEMPLATE BODY text, so it
        // moves the full WG profile only. The five touched owners are summed from
        // the dynamic blocks (write restrictions + messaging + CLI + credentials +
        // delegated reporting), which this change does not touch, so their V5
        // constants and gates stay exactly where they are.
        //
        // The delta is measured INSIDE this test against the frozen pre-Answering
        // template size (559, pinned by
        // `seeded_context_templates::tests::global_before_answering_snapshot_is_byte_exact`),
        // so a later text edit cannot ride silently under the ceiling.
        const V6_PRE_TEMPLATE_BYTES: usize = 559;
        const V6_DELTA_BYTES: usize = 294;
        const V6_FULL_WG_PROFILE_BYTES: usize = 11_271;
        const V6_MAX_FULL_WG_PROFILE_BYTES: usize = 10_514;
        // The pre-#1795 fixture renders no shared-location entries, so its full
        // profile is the V4-shaped render plus the V6 template delta.
        const V6_PRE_1795_MAX_FULL_WG_PROFILE_BYTES: usize =
            V4_MAX_FULL_WG_PROFILE_BYTES + V6_DELTA_BYTES;
```

(b) Insert after the V5 ladder block (after the `V5_FULL_WG_PROFILE_BYTES - V5_MAX_FULL_WG_PROFILE_BYTES` assert that ends at line 13132):

```rust
        assert_eq!(
            V6_DELTA_BYTES,
            super::get_default_agent_template().len() - V6_PRE_TEMPLATE_BYTES,
            "the V6 delta must be exactly the Answering template increase"
        );
        assert_eq!(
            V6_FULL_WG_PROFILE_BYTES,
            V5_FULL_WG_PROFILE_BYTES + V6_DELTA_BYTES
        );
        assert_eq!(
            V6_MAX_FULL_WG_PROFILE_BYTES,
            V5_MAX_FULL_WG_PROFILE_BYTES + V6_DELTA_BYTES
        );
        assert_eq!(
            V6_FULL_WG_PROFILE_BYTES - V6_MAX_FULL_WG_PROFILE_BYTES,
            REQUIRED_REDUCTION_BYTES
        );
```

(c) Replace the pre-#1795 full gate at lines 13103-13107:

```rust
        assert!(
            pre_full_wg.len() <= V6_PRE_1795_MAX_FULL_WG_PROFILE_BYTES,
            "pre-#1795 WG profile is {} bytes against the V6 pre-#1795 ceiling {V6_PRE_1795_MAX_FULL_WG_PROFILE_BYTES}",
            pre_full_wg.len()
        );
```

The pre-#1795 touched gate at 13099-13102 stays on `V4_MAX_TOUCHED_OWNERS_BYTES` (7_606): the fixture renders the same dynamic blocks.

(d) Replace the two current full-profile gates at lines 13217-13226:

```rust
        assert!(
            full_wg.len() <= V6_MAX_FULL_WG_PROFILE_BYTES,
            "WG profile is {} bytes; v6 baseline {V6_FULL_WG_PROFILE_BYTES}, ceiling {V6_MAX_FULL_WG_PROFILE_BYTES}",
            full_wg.len()
        );
        assert!(
            V6_FULL_WG_PROFILE_BYTES - full_wg.len() >= REQUIRED_REDUCTION_BYTES,
            "WG reduction is only {} bytes",
            V6_FULL_WG_PROFILE_BYTES - full_wg.len()
        );
```

The two current touched gates at 13208-13216 stay on the V5 constants. Every V3, V4, and V5 constant and every existing V3/V4/V5 ladder assert stays unchanged and stays green: they are pure constant relations. Gates that move: pre-#1795 full profile, current full profile, and the current full-profile reduction. Gates that do not move: pre-#1795 touched owners, current touched owners, current touched-owner reduction.

### 4.7 Files touched by the implementation

- `src-tauri/src/config/session_context.rs` - the template literal, the three tests in 4.2, and the V6 rung in 4.6.
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

## 6. Existing tests: impact

Exactly one test breaks and must change; every other test stays green. Grinch measured the full suite on the round-1 patch: `4436 passed; 1 failed`, the one failure the size-budget test.

| Test | Impact |
|---|---|
| `token_accounting::summarized_default_context_meets_size_budget` (`session_context.rs:13006`) | **Breaks, must change.** The Answering text is template body, so both full-profile renders grow 294 bytes and blow the V4 pre-#1795 ceiling (first failure) and the V5 full ceiling (next). Section 4.6 adds the V6 rung: ceilings 10_514 (current) and 9_403 (pre-#1795), the V6 ladder asserts, and the measured-delta oracle. The touched-owner gates do not move because the metric is unchanged |
| `default_agent_template_keeps_coarse_placeholder_order_after_summarization` (`:5175`) | Green: placeholder vector unchanged; `ends_with('\n')` still holds |
| `assert_mandatory_sections_once` (`:4802`) and heading-count tests (`:5097-5160`) | Green: they count mandatory placeholder headings; `## Answering` is not one |
| `seeded_template_versions_were_bumped` (`seeded_context_templates.rs:2549`), `project_specs_bump_global_to_v6_and_add_platform_specs` (`:2634`) | Green: `current_version` is not bumped (3.1) |
| `global_*_snapshot_is_byte_exact`, `both_frozen_global_generations_...` (`:2497`) | Green: existing frozen consts untouched and still `!=` the new default |
| `scan_replaces_pre_token_minimization_global_template` (`:3084`), `scan_replaces_v3_global_near_matches_in_both_project_state_shapes` (`:3349`) | Green: still `assert_ne!` against the default; replacement path unchanged |
| `coordinator_template_carries_cross_workgroup_rule` (`session_context.rs:9237`) | Green: the new text carries no coordinator rule |
| `frozen_pre_room_rename_global_template_is_recognized` (`:2464`) | Green: the new text has no "workgroup" |
| `root_prologue_*` tests (`session_context.rs:5917-7100`) | Green: Root does not read the template; its ten blocks are unchanged |

No test pins the length or sha256 of the current default. The 294-byte budget delta is now pinned by the V6 oracle in section 4.6; the new template body is 853 bytes (559 + 294).

## 7. Verification and acceptance mapping

### 7.1 Commands

```
cd repo-AgentsCommander
cargo test -p agentscommander --lib config::session_context::tests::default_agent_template_carries_answering_section_once_after_messaging
cargo test -p agentscommander --lib config::session_context::tests::materialized_replica_context_carries_answering_section
cargo test -p agentscommander --lib config::session_context::tests::root_runtime_prologue_omits_answering_section
cargo test -p agentscommander --lib config::session_context::token_accounting::summarized_default_context_meets_size_budget
cargo test -p agentscommander --lib config::seeded_context_templates::tests::global_before_answering_snapshot_is_byte_exact
cargo test -p agentscommander --lib config::seeded_context_templates::tests::frozen_pre_answering_global_template_is_recognized
cargo test -p agentscommander --lib config::seeded_context_templates::tests::scan_replaces_pre_answering_global_template_and_backs_it_up
cargo test -p agentscommander --lib config::seeded_context_templates::tests::scan_replaces_pre_answering_global_template_silently_with_trusted_state
cargo test -p agentscommander --lib
```

All tests must be green. The only pre-existing test that changes is `summarized_default_context_meets_size_budget` (section 4.6); every other pre-existing test stays green with no edits.

### 7.2 Negative control (order-independent, pinned panic)

Run on the committed implementation, with a clean tree. It never relies on uncommitted changes, so it cannot go vacuous like the round-1 stash form; it checks the old blob out of `329ab94e` and restores the file from HEAD. Do not run it before committing: the restore step takes the file from HEAD, so an uncommitted implementation would be lost.

```
cd repo-AgentsCommander
mkdir -p target
git checkout 329ab94e -- src-tauri/src/config/session_context.rs
cargo test -p agentscommander --lib config::seeded_context_templates::tests::scan_replaces_pre_answering_global_template_and_backs_it_up 2>&1 | tee target/control-2031.log
git checkout HEAD -- src-tauri/src/config/session_context.rs
```

The control is valid only if `target/control-2031.log` contains ALL of:

- ``assertion `left == right` failed: with no state entry the replacement is notified``
- `left: 0` and `right: 1`
- `scan_replaces_pre_answering_global_template_and_backs_it_up ... FAILED`

With the pre-Answering template restored, the on-disk fixture equals `get_default_agent_template()`, so `sync_one_template` takes the `AlreadyCurrent` branch (`seeded_context_templates.rs:1308-1310`): 0 replacements, no backup, and the file still lacks `## Answering`. Grinch measured exactly this panic on the round-1 patch (`seeded_context_templates.rs:2545:9`, left 0 right 1). A compile error, a different panic, or a pass invalidates the control: the pinned text is what separates a real control from a broken build. This is equivalent to the issue's "same test on `f9ee9f6`": `f9ee9f6` and `329ab94e` hold byte-identical templates (section 1). After the restore, re-run 7.1 to confirm green.

### 7.3 Acceptance mapping

| Issue acceptance criterion | Proof |
|---|---|
| `get_default_agent_template()` contains `## Answering` exactly once, after `{{INTER_AGENT_MESSAGING}}`, with no U+2014; pinned by a test | 4.1 plus `default_agent_template_carries_answering_section_once_after_messaging` (whole template asserted em-dash-free, measured 0 today) |
| A project `.ac/Context.AgentsCommander.md` holding the previous default is replaced on scan with a backup; control: the pre-change revision leaves it absent | `scan_replaces_pre_answering_global_template_and_backs_it_up` (notified path) and `..._silently_with_trusted_state` (real upgrade path), plus the 7.2 probe and the fixture assertion in `global_before_answering_snapshot_is_byte_exact` |
| A rendered replica `CLAUDE.md`/`AGENTS.md` contains the section; a rendered Root context does not | `materialized_replica_context_carries_answering_section` and `root_runtime_prologue_omits_answering_section` |
| `cargo test` green | 7.1 |

The size-budget rung (4.6) is a repository gate, not an issue criterion: its proof is the V6 oracle in 4.6 plus the green `summarized_default_context_meets_size_budget` run in 7.1.

## 8. Commit and handoff

- Implementation commit (by the implementer, on `feature/2031-seed-answering-section`): `feat(#2031): seed the Answering section into the project global context template` - both source files from 4.7 in one commit (template, const, arm, all tests including the 4.6 rung), then run 7.1 and 7.2. 7.2 restores the file from HEAD, so it works after the commit and never depends on uncommitted changes.
- This plan's own commit: `docs(plan): #2031 seed the Answering section into the project global context template`; `plans/` is gitignored, so stage with `git add -f plans/2031-seed-answering-section.md`, push the branch, and confirm `git ls-remote --heads origin feature/2031-seed-answering-section` reports a SHA other than `329ab94e`.
- Rollback: revert the implementation commit. The template returns to 559 bytes; the next scan repairs project files back to the previous default with a backup. The frozen const and recognizer arm are inert for the rolled-back state (the const would no longer be a predecessor, but it stays recognized, which is harmless).

Status: READY_FOR_IMPLEMENTATION
