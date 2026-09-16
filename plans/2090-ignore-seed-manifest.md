# Plan #2090: ignore `.ac/seed-manifest.toml` in the project `.ac/.gitignore`

Status: READY_FOR_IMPLEMENTATION

Issue: `#2090` (OPEN), "Ignore .ac/seed-manifest.toml in the project .ac/.gitignore"
Repository: `repo-AgentsCommander`
Base: `main` = `5203c4e3b0b5909fdc12337194c886b1514966c3`; branch `fix/2090-ignore-seed-manifest` (local HEAD = `origin` = `5203c4e3b0b5909fdc12337194c886b1514966c3`, clean tree)
Band: Lite 1-25 (issue score 19; Grinch reviews the plan). Author: `ac-dev-rust-v4`.
Task class: an existing `.ac/.gitignore` block migration in one writer function, its unit tests, and one docs page. No IPC, no frontend, no manifest writer, no persistence-format change.
Revision: round 1.

## 1. Objective and evidence

Goal: `<project>/.ac/.gitignore` must ignore `.ac/seed-manifest.toml`, for new projects and for existing projects whose file still carries the retired `!/seed-manifest.toml` block. The manifest is a diagnostic inventory that no code reads to decide seeding, overwrite, repair or deletion (`docs/features/seed-manifest.md:5-8`); the user decided it must not be version-controlled (issue body; architect reply `messaging/20260916-131737-...-seed-manifest-policy-reply.md`, option B).

Discovery method: codebase-memory graph on this working copy at `5203c4e3` (`index_status`: ready, 24,225 nodes / 162,551 edges, indexed `2026-09-15T14:00:38Z`, `root_path` = this room's `repo-AgentsCommander`). `check_index_coverage` returned `no_recorded_issue` for all five cited Rust files; freshness was `metadata_changed`, so every cited file was then read directly. `docs/` and `plans/` are excluded from the index by design and were read directly. The Git-level rules below (parent exclusion, later negation, anchored patterns) were measured with real `git check-ignore` runs in a scratch repository outside the working tree.

| Fact | Where (measured at `5203c4e3`) |
|---|---|
| AC writes the manifest rules with an un-ignore | `ensure_ac_root_gitignore`, `src-tauri/src/commands/ac_discovery.rs:1552-1723`; block at `:1555`, patterns at `:1556-1560` |
| The block is: coordination comment, lock, tmp, blank, reviewable comment, negation | `ac_discovery.rs:1555`: `"# AgentsCommander: exclude seed-manifest coordination files.\n/.seed-manifest.lock\n/.seed-manifest.*.tmp\n\n# AgentsCommander: keep the seed publication manifest reviewable.\n!/seed-manifest.toml\n"` |
| Reconciliation of an existing file is append-only | `ac_discovery.rs:1680-1711`: per-entry presence test on `line.trim()` (`:1685-1691`), seed rules used as one all-or-nothing group (`:1696-1702`), write only when `additions` is non-empty (`:1704-1711`) |
| Fresh file content | `ac_discovery.rs:1712-1720`: required entries each `comment\npattern\n\n`, then the seed block |
| The function runs on create and on discovery, so a migration hook exists | doc comment `ac_discovery.rs:1550-1551`; callers `ac_discovery.rs:1149,1885,1904,1974` and `commands/entity_creation.rs:1151` |
| The only place asserting the old block | `ac_discovery.rs:4783-4790` in `ensure_ac_root_gitignore_includes_delete_sentinels_on_create` (test `:4753-4791`) |
| Git currently does NOT ignore the manifest, root or nested | `seed_manifest_gitignore_rules_are_anchored_to_ac_root`, `ac_discovery.rs:5077-5124`, negation assertion at `:5118` |
| Two Stage E tests build on the negation | comment `ac_discovery.rs:5126-5130` plus `stage_e_parent_gitignore_excluding_ac_hides_manifest_despite_negation` (`:5132`); `stage_e_later_user_rule_and_excludes_win_over_managed_negation` (`:5173`, part (a) `:5175-5214`) |
| Replica rows are already retired | `ManifestFileKind::ReplicaConfigFile` (`config/seed_manifest.rs:365`) is not canonical output (`:383`); module doc `:8-14`; regression test `config_seed_exact_publish_preserves_report_without_creating_manifest` (`config/config_seed.rs:1414-1436`) |
| Docs describe the retired un-ignore and the retired replica rows | `docs/features/seed-manifest.md:4, 14, 21-23, 32, 54-61, 122-129, 161-168, 178-184, 212-213` |
| Measured Git semantics (scratch repo, `git 2.x`) | `.ac/.gitignore` `/seed-manifest.toml` wins at the root and does not match `.ac/nested/seed-manifest.toml`; a later `!/seed-manifest.toml` in the same file re-includes the manifest (`check-ignore --quiet` exit 1; `-v --non-matching` names the negation); a parent `.gitignore` `/.ac/` hides the manifest and `check-ignore -v` names the parent rule |
| Nothing else writes or asserts the negation | `git grep -l -F -e 'seed-manifest.toml'` outside `docs/releases`: the only negation/assertion sites are `ac_discovery.rs` and the docs listed above; no `.snap`, TS, TSX or resource fixture carries it |

Nothing in this change touches the manifest writer, its schema, `coverage`, or time semantics (#2091 owns feature removal).

## 2. Scope

In scope: the `ensure_ac_root_gitignore` writer and its unit tests in `src-tauri/src/commands/ac_discovery.rs`, plus the Git behavior and replica-row corrections in `docs/features/seed-manifest.md`. Every behavior below is proven by a test in section 5.2.

Out of scope: the manifest writer and the feature (#2091); untracking a manifest already in a Git index (AC must not run `git rm`; documentation tells the user); any other doc page. Three other pages carry claims this change or #1480 made stale and are **reported, not edited**, because issue #2090 scopes docs to `docs/features/seed-manifest.md`: `docs/reference/directory-layout.md:55` ("un-ignores `seed-manifest.toml`"), `:124` (replica config folders "rows under `config:<dest>` scopes"), `docs/features/config-seed.md:133-136` ("A **successful** config-seed publication is recorded...") and `:175` ("where successful replica publications are recorded"). The coordinator can open a follow-up.

## 3. Cause

`ensure_ac_root_gitignore` un-ignores the manifest on purpose for a policy that no longer holds: the block's second comment says the manifest is kept reviewable and ends with `!/seed-manifest.toml` (`ac_discovery.rs:1555`). The function only appends when a pattern line is absent (`:1696-1702`), so changing the constant alone would leave existing installations un-ignored and would match neither the retired comment nor the negation. A migration path is therefore required, and it must preserve user bytes: the same function already guarantees byte preservation for user content in every existing test.

## 4. Decisions from the code

**D1 - The manifest is ignored with an anchored rule.** `/seed-manifest.toml` (leading slash), next to the unchanged `/.seed-manifest.lock` and `/.seed-manifest.*.tmp`. The measured semantics (section 1) are that the anchored rule ignores the `.ac` root manifest and leaves same-named files in nested user directories alone, which is what the current anchored test asserts for the lock and temp rules (`ac_discovery.rs:5114-5123`).

**D2 - The new managed block is split into two sub-blocks.** Coordination (`comment`, `/.seed-manifest.lock`, `/.seed-manifest.*.tmp`) and manifest (`comment`, `/seed-manifest.toml`). Reason: reconciliation is all-or-nothing per sub-block, so a legacy migration that leaves the coordination rules in place appends only the manifest sub-block and can never duplicate the lock/temp lines. A file missing only one coordination line keeps the pre-existing all-or-nothing behavior (the whole coordination sub-block is appended); this change does not add per-line deduplication.

**D3 - Migration is line-based, not substring-based.** The helper removes the two AC-managed lines only when the negation `!/seed-manifest.toml` is directly preceded (after trim) by the retired comment `# AgentsCommander: keep the seed publication manifest reviewable.`, plus the single blank line directly before that comment when present. Reasons: the managed block always wrote that pair, trim matching survives CRLF conversion by Git or an editor, and content outside those lines is re-emitted byte for byte.

**D4 - A bare `!/seed-manifest.toml` is user intent and is never removed.** Without the retired comment, a negation is a user's opt-in to tracking the manifest after AC began ignoring it. AC must not clobber it on the next reconciliation: the managed `/seed-manifest.toml` line is still present, so nothing is appended and the user's later negation keeps winning (Git last-match-wins, measured). Removing a bare negation would silently undo the user's choice on every discovery.

**D5 - Canonical migration is byte-identical to a fresh create.** A `.gitignore` written by the previous build, migrated once, must equal the bytes a fresh `.ac` receives in the same build. This is what makes the migration auditable and is asserted by a test (5.2 e, `ensure_ac_root_gitignore_migrated_root_matches_a_fresh_root`).

**D6 - The write condition gains the migration flag.** `if migrated || !additions.is_empty()`. Without it a legacy file that already contains every other managed rule would be read, migrated in memory, and never written.

**D7 - No Git index operation.** AC never runs `git rm --cached`. A manifest already tracked stays tracked; the docs say so and give the command. Ignoring is not untracking.

**D8 - Docs scope is exactly `docs/features/seed-manifest.md`.** The Git behavior section flips to "ignored by default", and every replica-row statement is corrected against the code: two publisher families write rows, legacy `replica_config_file` rows stay readable but are omitted from the next canonical write, and the config install/restore failure no longer removes rows. The three stale passages in other pages stay untouched and are reported in section 2.

## 5. Exact changes

### 5.1 `src-tauri/src/commands/ac_discovery.rs`, `ensure_ac_root_gitignore`

*(a) Replace lines 1555-1560 (the two seed constants) with:*

```rust
    const SEED_MANIFEST_COORDINATION_BLOCK: &str = "# AgentsCommander: exclude seed-manifest coordination files.\n/.seed-manifest.lock\n/.seed-manifest.*.tmp\n";
    const SEED_MANIFEST_MANIFEST_BLOCK: &str = "# AgentsCommander: exclude the seed publication manifest from Git tracking.\n/seed-manifest.toml\n";
    const SEED_MANIFEST_COORDINATION_PATTERNS: [&str; 2] = [
        "/.seed-manifest.lock",
        "/.seed-manifest.*.tmp",
    ];
    const SEED_MANIFEST_MANIFEST_PATTERN: &str = "/seed-manifest.toml";
```

*(b) In the existing-file branch, replace line 1680 (`let content = std::fs::read_to_string(&gitignore_path)`) through line 1702 (the closing brace of the old seed check) with:*

```rust
        let content = std::fs::read_to_string(&gitignore_path)
            .map_err(|e| format!("Failed to read Project AC Root .gitignore: {}", e))?;
        let (content, migrated) = migrate_legacy_seed_manifest_gitignore(content);

        let mut additions = String::new();
        for (pattern, comment) in required_entries {
            let is_present = content.lines().any(|line| {
                if *pattern == PROJECT_SETTINGS_GITIGNORE_PATTERN {
                    line == *pattern
                } else {
                    line.trim() == *pattern
                }
            });
            if !is_present {
                additions.push_str(&format!("\n{}\n{}\n", comment, pattern));
            }
        }
        if !SEED_MANIFEST_COORDINATION_PATTERNS
            .iter()
            .all(|pattern| content.lines().any(|line| line.trim() == *pattern))
        {
            additions.push('\n');
            additions.push_str(SEED_MANIFEST_COORDINATION_BLOCK);
        }
        if !content
            .lines()
            .any(|line| line.trim() == SEED_MANIFEST_MANIFEST_PATTERN)
        {
            additions.push('\n');
            additions.push_str(SEED_MANIFEST_MANIFEST_BLOCK);
        }
```

*(c) Replace line 1704 (`if !additions.is_empty() {`) with:*

```rust
        if migrated || !additions.is_empty() {
```

*(d) In the create branch, replace line 1717 (`content.push_str(SEED_MANIFEST_GITIGNORE_BLOCK);`) with:*

```rust
        content.push_str(SEED_MANIFEST_COORDINATION_BLOCK);
        content.push('\n');
        content.push_str(SEED_MANIFEST_MANIFEST_BLOCK);
```

The resulting fresh-file bytes are the same shape as before with the manifest sub-block replaced: `.../.*.tmp\n\n# AgentsCommander: exclude the seed publication manifest from Git tracking.\n/seed-manifest.toml\n`.

*(e) Insert the migration helper immediately after `ensure_ac_root_gitignore` (after line 1723, before the `/// Create a canonical .ac/ directory` comment at line 1725):*

```rust
/// #2090 - migrate the retired `!/seed-manifest.toml` un-ignore pair written by
/// older builds. Only the two AC-managed lines (the retired comment directly
/// above the negation and, when the managed block wrote one, the blank line
/// before that comment) are removed; a bare `!/seed-manifest.toml` without the
/// retired comment is user intent and is preserved. Every other byte, including
/// CRLF and a missing final newline, is preserved; the flag is false when
/// nothing changed.
fn migrate_legacy_seed_manifest_gitignore(content: String) -> (String, bool) {
    const LEGACY_COMMENT: &str = "# AgentsCommander: keep the seed publication manifest reviewable.";
    const LEGACY_NEGATION: &str = "!/seed-manifest.toml";

    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut removed = vec![false; lines.len()];
    let mut changed = false;
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != LEGACY_NEGATION {
            continue;
        }
        let Some(previous) = index.checked_sub(1) else {
            continue;
        };
        if lines[previous].trim() != LEGACY_COMMENT {
            continue;
        }
        removed[previous] = true;
        removed[index] = true;
        if let Some(blank) = previous.checked_sub(1) {
            if lines[blank].trim().is_empty() {
                removed[blank] = true;
            }
        }
        changed = true;
    }
    if !changed {
        return (content, false);
    }
    let migrated: String = lines
        .iter()
        .zip(&removed)
        .filter(|(_, drop)| !**drop)
        .map(|(line, _)| *line)
        .collect();
    (migrated, true)
}
```

### 5.2 Tests in the same file (`mod tests`, starts at line 3230)

*(a) Update the stale block assertion.* In `ensure_ac_root_gitignore_includes_delete_sentinels_on_create` (`:4753-4791`), replace lines 4783-4790 with:

```rust
        assert!(content.contains(concat!(
            "# AgentsCommander: exclude seed-manifest coordination files.\n",
            "/.seed-manifest.lock\n",
            "/.seed-manifest.*.tmp\n",
            "\n",
            "# AgentsCommander: exclude the seed publication manifest from Git tracking.\n",
            "/seed-manifest.toml\n"
        )));
```

*(b) Flip the Git-level assertion.* In `seed_manifest_gitignore_rules_are_anchored_to_ac_root` (`:5077-5124`), replace line 5118

```rust
        assert!(!ignored(".ac/seed-manifest.toml"));
```

with:

```rust
        assert!(ignored(".ac/seed-manifest.toml"));
```

Every nested assertion in that test stays as it is, because `/seed-manifest.toml` is anchored to the `.ac` root.

*(c) Drop the negation from the Stage E parent-exclusion test.* Replace the comment at lines 5126-5130 with:

```rust
    // Stage E (#1064) Git-visibility conformance (plan section 10.5 items 3-4,
    // section 7.3). AC's managed `.ac/.gitignore` ignores the manifest, but a
    // parent rule that excludes the `.ac/` directory still wins: Git does not
    // descend into an excluded directory. AC documents but does not override
    // this.
```

and rename the test at line 5132 from `stage_e_parent_gitignore_excluding_ac_hides_manifest_despite_negation` to:

```rust
    fn stage_e_parent_gitignore_excluding_ac_hides_manifest() {
```

The body and its assertions stay unchanged: measured `git check-ignore -v --no-index -- .ac/seed-manifest.toml` names the parent `.gitignore:1:/.ac/` rule and exits 0.

*(d) Rewrite Stage E part (a) to the user negation.* Replace line 5174 (`// (a) A later user rule in the SAME .ac/.gitignore re-hides the manifest.`) through line 5214 (the closing brace of block (a)) with:

```rust
        // (a) A later user negation in the SAME .ac/.gitignore re-includes the manifest.
        {
            let tmp = tempfile::tempdir().expect("tempdir");
            let project = tmp.path().join("project");
            let ac_root = project.join(".ac");
            std::fs::create_dir_all(&ac_root).expect("create .ac");
            ensure_ac_root_gitignore(&ac_root).expect("ensure .gitignore");
            // Append a later user negation (never reordered by AC).
            let mut content =
                std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
            content.push_str("\n# user rule\n!/seed-manifest.toml\n");
            std::fs::write(ac_root.join(".gitignore"), content).expect("append user rule");
            std::fs::write(ac_root.join("seed-manifest.toml"), b"x").expect("manifest");

            let init = std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&project)
                .status()
                .expect("git init");
            assert!(init.success());
            let verbose = std::process::Command::new("git")
                .args([
                    "check-ignore",
                    "-v",
                    "--non-matching",
                    "--no-index",
                    "--",
                    ".ac/seed-manifest.toml",
                ])
                .current_dir(&project)
                .output()
                .expect("check-ignore");
            assert!(
                verbose.status.success(),
                "check-ignore -v --non-matching must name the winning rule"
            );
            let line = String::from_utf8(verbose.stdout).expect("utf8");
            assert!(
                line.contains(".ac/.gitignore") && line.contains("!/seed-manifest.toml"),
                "a later same-file user negation must re-include the manifest, got {line:?}"
            );
            let ignored = std::process::Command::new("git")
                .args([
                    "check-ignore",
                    "--quiet",
                    "--no-index",
                    "--",
                    ".ac/seed-manifest.toml",
                ])
                .current_dir(&project)
                .status()
                .expect("check-ignore");
            assert!(
                !ignored.success(),
                "the manifest must not be ignored after the user negation, got exit {:?}",
                ignored.code()
            );
        }
```

Rename the test at line 5173 from `stage_e_later_user_rule_and_excludes_win_over_managed_negation` to:

```rust
    fn stage_e_later_user_rule_and_excludes_win_over_managed_rules() {
```

Blocks (b) and (c) of that test stay unchanged: parent-directory and global excludes still hide the manifest.

*(e) Insert four new tests after line 5124 (end of `seed_manifest_gitignore_rules_are_anchored_to_ac_root`), before the Stage E comment at line 5126:*

```rust
    /// #2090 - an existing `.ac/.gitignore` carrying the retired
    /// `!/seed-manifest.toml` pair must lose the pair, keep every other byte and
    /// gain the ignore rule exactly once. A bare user negation is not removed
    /// (covered separately).
    #[test]
    fn ensure_ac_root_gitignore_migrates_the_legacy_seed_manifest_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create .ac");
        let gitignore_path = ac_root.join(".gitignore");
        const USER_RULE: &str = "# user rule\n!important.txt\n";
        let legacy = format!(
            "{USER_RULE}{}",
            concat!(
                "# AgentsCommander: exclude seed-manifest coordination files.\n",
                "/.seed-manifest.lock\n",
                "/.seed-manifest.*.tmp\n",
                "\n",
                "# AgentsCommander: keep the seed publication manifest reviewable.\n",
                "!/seed-manifest.toml\n"
            )
        );
        std::fs::write(&gitignore_path, &legacy).expect("seed legacy .gitignore");

        ensure_ac_root_gitignore(&ac_root).expect("migrate legacy .gitignore");

        let content = std::fs::read_to_string(&gitignore_path).expect("read .gitignore");
        assert!(
            content.starts_with(USER_RULE),
            "user content must survive byte for byte, got {content:?}"
        );
        assert!(
            !content.contains("!/seed-manifest.toml"),
            "the retired negation must be removed"
        );
        assert!(
            !content.contains("# AgentsCommander: keep the seed publication manifest reviewable."),
            "the retired comment must be removed"
        );
        for pattern in [
            "/.seed-manifest.lock",
            "/.seed-manifest.*.tmp",
            "/seed-manifest.toml",
        ] {
            assert_eq!(
                content.lines().filter(|line| line.trim() == pattern).count(),
                1,
                "the migrated .gitignore must carry {pattern} exactly once"
            );
        }

        // Idempotent: a second ensure appends nothing.
        let before = content;
        ensure_ac_root_gitignore(&ac_root).expect("second ensure");
        assert_eq!(
            std::fs::read_to_string(&gitignore_path).expect("re-read"),
            before,
            "a second call must be a no-op"
        );
    }

    /// #2090 - the exact bytes an older build wrote must migrate to exactly the
    /// bytes a fresh root gets, with no duplicated coordination rules and no
    /// leftover blank line.
    #[test]
    fn ensure_ac_root_gitignore_migrated_root_matches_a_fresh_root() {
        const NEW_MANIFEST_BLOCK: &str = concat!(
            "# AgentsCommander: exclude the seed publication manifest from Git tracking.\n",
            "/seed-manifest.toml\n"
        );
        const LEGACY_MANIFEST_BLOCK: &str = concat!(
            "# AgentsCommander: keep the seed publication manifest reviewable.\n",
            "!/seed-manifest.toml\n"
        );

        let fresh = tempfile::tempdir().expect("tempdir");
        let fresh_ac = fresh.path().join(".ac");
        std::fs::create_dir(&fresh_ac).expect("create .ac");
        ensure_ac_root_gitignore(&fresh_ac).expect("ensure fresh .gitignore");
        let expected =
            std::fs::read_to_string(fresh_ac.join(".gitignore")).expect("read fresh .gitignore");

        let old = tempfile::tempdir().expect("tempdir");
        let old_ac = old.path().join(".ac");
        std::fs::create_dir(&old_ac).expect("create .ac");
        let legacy_content = expected.replace(NEW_MANIFEST_BLOCK, LEGACY_MANIFEST_BLOCK);
        assert!(
            legacy_content.contains("!/seed-manifest.toml"),
            "the fixture must carry the retired negation"
        );
        std::fs::write(old_ac.join(".gitignore"), &legacy_content).expect("seed legacy .gitignore");

        ensure_ac_root_gitignore(&old_ac).expect("migrate legacy .gitignore");

        assert_eq!(
            std::fs::read_to_string(old_ac.join(".gitignore")).expect("read migrated .gitignore"),
            expected,
            "migrating a prior-build .gitignore must reproduce a fresh root byte for byte"
        );
    }

    /// #2090 - a `!/seed-manifest.toml` line without the retired AC comment is
    /// user intent; reconciliation must not remove it.
    #[test]
    fn ensure_ac_root_gitignore_preserves_a_bare_user_seed_manifest_negation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create .ac");
        let gitignore_path = ac_root.join(".gitignore");
        ensure_ac_root_gitignore(&ac_root).expect("ensure .gitignore");
        let mut content = std::fs::read_to_string(&gitignore_path).expect("read .gitignore");
        content.push_str("\n# user rule\n!/seed-manifest.toml\n");
        std::fs::write(&gitignore_path, &content).expect("append user negation");

        ensure_ac_root_gitignore(&ac_root).expect("ensure again");

        assert_eq!(
            std::fs::read_to_string(&gitignore_path).expect("re-read"),
            content,
            "a bare user negation must survive reconciliation byte for byte"
        );
    }

    /// #2090 - the retired pair is matched on trimmed lines, so a `.gitignore`
    /// converted to CRLF by Git or an editor is migrated too.
    #[test]
    fn ensure_ac_root_gitignore_migrates_a_crlf_legacy_seed_manifest_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create .ac");
        let gitignore_path = ac_root.join(".gitignore");
        const USER_RULE: &str = "# user rule\r\n";
        let legacy = concat!(
            "# user rule\r\n",
            "# AgentsCommander: keep the seed publication manifest reviewable.\r\n",
            "!/seed-manifest.toml\r\n"
        );
        std::fs::write(&gitignore_path, legacy).expect("seed CRLF legacy .gitignore");

        ensure_ac_root_gitignore(&ac_root).expect("migrate legacy .gitignore");

        let content = std::fs::read_to_string(&gitignore_path).expect("read .gitignore");
        assert!(
            content.starts_with(USER_RULE),
            "the user rule must survive byte for byte, got {content:?}"
        );
        assert!(
            !content.contains("!/seed-manifest.toml"),
            "the CRLF negation must be removed"
        );
        assert_eq!(
            content
                .lines()
                .filter(|line| line.trim() == "/seed-manifest.toml")
                .count(),
            1,
            "the CRLF file must gain the manifest ignore rule exactly once"
        );
    }
```

### 5.3 `docs/features/seed-manifest.md`

*(a) Intro, lines 3-8. Replace with:*

```markdown
AgentsCommander records the project-scoped files it publishes into your project's
`.ac` folder in a small, deterministic text file:
`<project>/.ac/seed-manifest.toml`. It is a **diagnostic inventory**, not an
ownership ledger: it tells you which managed files AC last seeded and when. It
never grants ownership and never authorizes AC to overwrite, repair, or delete
anything. AC's managed `.ac/.gitignore` ignores it, so it is not
version-controlled by default (see [Git behavior](#git-behavior)).
```

*(b) "What it records". Replace line 14*

```markdown
publication by AgentsCommander. Three publisher families write rows:
```

*with*

```markdown
publication by AgentsCommander. Two publisher families write rows:
```

*Delete lines 21-23 (the replica bullet, between the template bullet and the catalog bullet) and insert this paragraph after line 32 (end of the catalog bullet, before the blank line and `Everything else is deliberately`):*

```markdown
Replica config folders are **not recorded**. [Config seed](config-seed.md) still
installs `.claude`/`.codex`/... into a room replica at every spawn, but since
[#1480](https://github.com/mblua/AgentsCommander/issues/1480) those publications
create no rows. A manifest written by an older build may still carry
`replica_config_file` rows: AC reads them, never updates them, and omits them from
the next canonical write. The `replica_config_folders` name remains in the
`coverage` declaration below as compatibility vocabulary.
```

The context-template and catalog bullets stay unchanged, and the bullet list stays contiguous.

*(c) Schema example. Delete lines 54-61, i.e. the middle `[[files]]` block and its trailing blank line:*

```toml
[[files]]
path = ".ac/room-14-dev-team/__agent_architect/.claude/settings.json"
path_encoding = "utf8"
kind = "replica_config_file"
scope = "config:.ac/room-14-dev-team/__agent_architect/.claude"
source = "workspace_base"
last_seeded_at = "2026-07-16T19:41:12.456Z"
```

The remaining example rows (`project_context_template`, `coding_agent_catalog`) and the `coverage` line stay as they are.

*(d) "Normal churn". Replace lines 122-129 with:*

```markdown
Every physical publication updates its row to that event's wall-clock time, even
when the published bytes are identical (see [Time semantics](#time-semantics)).
That is the accepted product cost of recording real publication time; AC does not
suppress the timestamp, compare content, or truncate the row list to reduce churn.
The manifest is ignored by Git by default (see [Git behavior](#git-behavior)), so
this churn no longer appears in your diffs.
```

*(e) The config install-and-restore bullet. Replace lines 161-168 with:*

```markdown
- **Config install-and-restore failure.** Config seed renames the old destination
  aside before installing the new one, and reports a typed failure when the
  install, the restore, or both fail. Since #1480 a config-seed publication
  neither adds nor removes a manifest row, so no failure mode of that install
  changes the manifest; a legacy `replica_config_file` row survives until the
  next canonical write or explicit lifecycle event.
```

*(f) "Git behavior". Replace lines 178-184 with:*

```markdown
## Git behavior

`.ac/seed-manifest.toml` is **not version-controlled by default**. AC's managed
`.ac/.gitignore` block ignores the manifest together with its lock and temp
companions: `/.seed-manifest.lock`, `/.seed-manifest.*.tmp` and
`/seed-manifest.toml`. The leading slash anchors all three rules to the `.ac`
root, so same-named files in nested user directories are unaffected. If a project
already tracks the manifest, the ignore rule alone does not untrack it; run
`git rm --cached <project>/.ac/seed-manifest.toml` to stop tracking it. If you
would rather keep reviewing the manifest in Git, add `!/seed-manifest.toml`
after AC's rule in `.ac/.gitignore`: Git applies the last matching rule. An
existing `.ac/.gitignore` that still carries the retired `!/seed-manifest.toml`
block is migrated in place by the next project registration or discovery.
```

*(g) "See also". Replace lines 212-213 with:*

```markdown
- [Config seed](config-seed.md) - the replica config publications, which no
  longer produce manifest rows (#1480)
```

### 5.4 Files touched by the implementation

- `src-tauri/src/commands/ac_discovery.rs` - constants, both reconciliation branches, the new migration helper (5.1), the two test edits and two test renames (5.2 a-d), four new tests (5.2 e).
- `docs/features/seed-manifest.md` - the seven passages in 5.3.
- No other source, test, doc, config or frontend file.

## 6. Behavior and edge cases

- Fresh `.ac`: the file carries the three anchored rules; `git check-ignore` ignores the manifest and the nested-path assertions stay false (test 5.2 b).
- Prior-build canonical `.ac/.gitignore`: the retired pair and its separator blank line are removed, the manifest sub-block is appended, and the bytes equal a fresh root (5.2 e, second test).
- Prior-build file with user content before or after: only the managed lines are removed (5.2 e, first test); the retired comment and negation are gone, lock/temp appear exactly once.
- CRLF file: trim matching removes the pair and leaves the CRLF user rule untouched (5.2 e, fourth test). The appended block is LF, the pre-existing mixed-endings behavior.
- Bare user negation: preserved, no rewrite on the second run (5.2 e, third test), and Git still re-includes the manifest because the user's rule comes later (5.2 d).
- Idempotency: a second `ensure_ac_root_gitignore` call writes nothing in every migrated state (test 5.2 e, all exact-equality assertions).
- Partial files: if only some coordination patterns are present, the whole coordination sub-block is appended, the pre-existing all-or-nothing behavior. A canonical legacy file cannot hit that branch: the managed block always wrote both coordination lines.
- Missing final newline: the existing separator rule is unchanged; no new behavior.
- Already-tracked manifest: nothing in AC changes its index state (D7); the docs sentence covers the user action.
- Nested same-named files: unaffected by the anchored rules (5.2 b).
- The manifest writer, schema, coverage list and time semantics are untouched; `seed-manifest.toml` keeps its bytes until a real publication.

## 7. Existing tests: impact

Four existing tests change (two break, two are rewritten and renamed); every other test stays green with no edit.

| Test | Impact |
|---|---|
| `ensure_ac_root_gitignore_includes_delete_sentinels_on_create` (`ac_discovery.rs:4753`) | **Breaks, must change.** Its exact-block assertion (`:4783-4790`) carries the retired pair. Updated by 5.2 a. |
| `seed_manifest_gitignore_rules_are_anchored_to_ac_root` (`:5077`) | **Breaks, must change.** Line 5118 asserts the manifest is NOT ignored; the new rule ignores it. Updated by 5.2 b; the nested assertions stay. |
| `stage_e_parent_gitignore_excluding_ac_hides_manifest_despite_negation` (`:5132`) | Passes as written (the parent rule already won), but its name and comment assert a negative that no longer exists. Renamed and re-commented by 5.2 c. |
| `stage_e_later_user_rule_and_excludes_win_over_managed_negation` (`:5173`) | Passes as written but its part (a) appends a duplicate managed pattern, so it proves nothing about user rules. Rewritten to the user negation by 5.2 d; blocks (b) and (c) unchanged. |
| `ensure_ac_root_gitignore_creates_both_patterns`, `managed_catalog_ac_root_gitignore_carries_sidecar_rules`, `ensure_ac_root_gitignore_appends_room_to_a_legacy_only_file`, and every other `ensure_ac_root_gitignore_*` test | Green: they do not read the seed sub-block and the migration helper is a no-op on their fixtures. The byte-preservation and idempotency assertions still hold because untouched content is re-emitted unchanged. |
| `cli_workgroup_team.rs:2100-2108`, `cli_project_registration.rs` | Green: the first checks `room-*/` and `wg-*/` only; the second never reads `.ac/.gitignore`. |

## 8. Verification and acceptance mapping

### 8.1 Commands

```
cd repo-AgentsCommander
cargo test -p agentscommander --lib commands::ac_discovery::tests::ensure_ac_root_gitignore_migrates_the_legacy_seed_manifest_block
cargo test -p agentscommander --lib commands::ac_discovery::tests::ensure_ac_root_gitignore_migrated_root_matches_a_fresh_root
cargo test -p agentscommander --lib commands::ac_discovery::tests::ensure_ac_root_gitignore_preserves_a_bare_user_seed_manifest_negation
cargo test -p agentscommander --lib commands::ac_discovery::tests::ensure_ac_root_gitignore_migrates_a_crlf_legacy_seed_manifest_block
cargo test -p agentscommander --lib commands::ac_discovery::tests::seed_manifest_gitignore_rules_are_anchored_to_ac_root
cargo test -p agentscommander --lib commands::ac_discovery::tests::stage_e_later_user_rule_and_excludes_win_over_managed_rules
cargo test -p agentscommander --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

All commands must exit 0. The full `--lib` run is the regression proof; the targeted runs are the new and changed tests.

Docs check (run from the repo root):

```
grep -n "committed and reviewed\|Three publisher families\|Config-seed tiers 1 through 4\|that produce\|keeps the root" docs/features/seed-manifest.md
grep -c "not version-controlled by default" docs/features/seed-manifest.md
grep -c "keep reviewing the manifest in Git" docs/features/seed-manifest.md
```

The first prints nothing; the second prints `1` (the Git behavior sentence wraps in the intro, so only the Git behavior line matches); the third prints `1`.

### 8.2 Controls (named mutation, pinned failure)

Each mutation is applied to the otherwise complete implementation, the named tests are run with output captured, then `git checkout -- src-tauri/src/commands/ac_discovery.rs` restores the tree and 8.1 is green again.

**C1 - migration off (kills 5.2 e).** Replace

```rust
        let (content, migrated) = migrate_legacy_seed_manifest_gitignore(content);
```

with

```rust
        let (content, migrated) = (content, false);
```

Run the four migration tests plus `seed_manifest_gitignore_rules_are_anchored_to_ac_root`. Required pinned failures:

- `ensure_ac_root_gitignore_migrates_the_legacy_seed_manifest_block` FAILED with `the retired negation must be removed`.
- `ensure_ac_root_gitignore_migrated_root_matches_a_fresh_root` FAILED with `migrating a prior-build .gitignore must reproduce a fresh root byte for byte`.
- `ensure_ac_root_gitignore_migrates_a_crlf_legacy_seed_manifest_block` FAILED with `the CRLF negation must be removed`.
- `ensure_ac_root_gitignore_preserves_a_bare_user_seed_manifest_negation` and `seed_manifest_gitignore_rules_are_anchored_to_ac_root` stay green, as they must: C1 disables migration, not the new create block.

A compile error, a different assertion, or a pass invalidates the control.

**C2 - un-ignore restored (kills 5.2 a and b).** Set `SEED_MANIFEST_MANIFEST_BLOCK` back to the retired text:

```rust
    const SEED_MANIFEST_MANIFEST_BLOCK: &str = "# AgentsCommander: keep the seed publication manifest reviewable.\n!/seed-manifest.toml\n";
```

Run `ensure_ac_root_gitignore_includes_delete_sentinels_on_create` and `seed_manifest_gitignore_rules_are_anchored_to_ac_root`. Required: both FAILED, the first on the `content.contains(concat!(...))` assertion for the new block and the second on `assert!(ignored(".ac/seed-manifest.toml"))`.

### 8.3 Acceptance mapping

| Issue requirement | Proof |
|---|---|
| New projects get an `.ac/.gitignore` that ignores the manifest | 5.1 a, d; block assertion in 5.2 a; real `git check-ignore` in 5.2 b |
| Existing `.ac/.gitignore` files carrying the old `!/seed-manifest.toml` block are migrated | 5.2 e: pair removed with bytes preserved, migration equals a fresh root, CRLF variant, bare user negation preserved; C1 |
| Tests updated to the new policy | 5.2 a-d, section 7 |
| `docs/features/seed-manifest.md` Git behavior updated and stale replica rows corrected | 5.3; the three greps in 8.1 |
| `cargo test` green, no unrelated file changed | 8.1; 5.4 |

## 9. Commit and handoff

- This plan's own commit: `docs(plan): #2090 ignore seed-manifest in the project .ac/.gitignore`. `plans/` is gitignored (`.gitignore` line 11 `/plans/`), so stage with `git add -f plans/2090-ignore-seed-manifest.md`; push the branch and confirm `git ls-remote --heads origin fix/2090-ignore-seed-manifest` reports a SHA other than `5203c4e3b0b5909fdc12337194c886b1514966c3`.
- Implementation commit (only after Grinch approves and the coordinator starts it): `fix(#2090): ignore seed-manifest.toml in the project .ac/.gitignore` - the two files of 5.4 in one commit, then 8.1 and 8.2. Rollback: revert that commit; the writer returns to the un-ignore block and migrated `.ac/.gitignore` files keep their new rule until the next reconciliation with reverted code, which restores nothing automatically (the old bytes are not stored). No data is lost either way: the manifest file itself is never written by this change.
- Out of scope and reported to the coordinator, not implemented here: the three stale passages in 5.3's siblings (`docs/reference/directory-layout.md:55,124`; `docs/features/config-seed.md:133-136,175`) and every item under #2091.

Status: READY_FOR_IMPLEMENTATION
