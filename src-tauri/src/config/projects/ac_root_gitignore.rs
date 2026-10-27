use crate::config::instance_artifacts::{
    CODING_AGENTS_BASE_TMP_ARTIFACT, CODING_AGENTS_LOCAL_ARTIFACT,
    CODING_AGENTS_LOCAL_TMP_ARTIFACT, CODING_AGENTS_LOCK_ARTIFACT,
    CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT, CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
    CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT, CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT,
    CODING_AGENTS_RETIRED_LOCK_ARTIFACT, CODING_AGENTS_SET_ASIDE_ARTIFACT,
};
use std::path::Path;

use crate::filesystem::projects::discovery;

/// #2434 - escape a bare instructions filename for a gitignore pattern: `\`,
/// `*`, `?`, `[`, `]` anywhere, and a leading `#` or `!`, each get a `\`
/// prefix. Separators and trailing spaces are already rejected upstream.
pub(crate) fn escape_gitignore_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (index, ch) in name.chars().enumerate() {
        if matches!(ch, '\\' | '*' | '?' | '[' | ']') || (index == 0 && matches!(ch, '#' | '!')) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// [`ensure_ac_root_gitignore`] with the already-resolved instructions
/// filenames. The `CLAUDE.md` / `AGENTS.md` floor rows are always written; each
/// other distinct name adds an escaped `**/__agent_*/<name>` row. A name failing
/// `is_safe_instructions_filename` is dropped with a warning.
pub(crate) fn ensure_ac_root_gitignore_with_names(
    ac_root: &Path,
    configured: &[String],
) -> Result<(), String> {
    let gitignore_path = ac_root.join(".gitignore");
    // #2717 - the project-settings rows have one home, shared with the
    // migration's sweep.
    let project_settings_rows = crate::config::naming_migration::project_settings_ignore_rows();
    let [project_settings_row, project_settings_lock_row, set_aside_row] = project_settings_rows
        .each_ref()
        .map(|(pattern, comment)| (pattern.as_str(), *comment));
    let project_settings_gitignore_pattern = project_settings_row.0;
    // #2718 - the Loop state rows, shared with the Loop scope's sweep.
    let loop_state_rows = crate::config::naming_migration::loop_state_ignore_rows();
    let [loop_state_row, loop_state_tmp_row, loop_set_aside_row] = loop_state_rows
        .each_ref()
        .map(|(pattern, comment)| (pattern.as_str(), *comment));
    // #2807 - the agent state rows, shared with the config pair's sweep.
    let config_state_rows = crate::config::naming_migration::config_state_ignore_rows();
    let [config_state_row, config_state_lock_row, config_state_tmp_row] = config_state_rows
        .each_ref()
        .map(|(pattern, comment)| (pattern.as_str(), *comment));
    const SEED_MANIFEST_COORDINATION_BLOCK: &str = "# AgentsCommander: exclude seed-manifest \
        coordination files.\n/.seed-manifest.lock\n/.seed-manifest.*.tmp\n";
    const SEED_MANIFEST_MANIFEST_BLOCK: &str = "# AgentsCommander: exclude the seed publication \
        manifest from Git tracking.\n/seed-manifest.toml\n";
    const SEED_MANIFEST_COORDINATION_PATTERNS: [&str; 2] =
        ["/.seed-manifest.lock", "/.seed-manifest.*.tmp"];
    const SEED_MANIFEST_MANIFEST_PATTERN: &str = "/seed-manifest.toml";

    // #1968 - the machine-local coding-agent catalog children, rooted at `.ac/`.
    let catalog = [
        CODING_AGENTS_LOCAL_ARTIFACT,
        CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT,
        CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT,
        CODING_AGENTS_LOCK_ARTIFACT,
        CODING_AGENTS_BASE_TMP_ARTIFACT,
        CODING_AGENTS_LOCAL_TMP_ARTIFACT,
        CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
        CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT,
        CODING_AGENTS_RETIRED_LOCK_ARTIFACT,
        CODING_AGENTS_SET_ASIDE_ARTIFACT,
    ]
    .map(|artifact| format!("/{artifact}"));
    let [catalog_local, catalog_migration_backup, catalog_migration_journal, ..] = &catalog;
    let [_, _, _, catalog_lock, catalog_base_tmp, catalog_local_tmp, ..] = &catalog;
    let [.., catalog_migration_backup_tmp, catalog_migration_journal_tmp, _, _] = &catalog;
    let [.., catalog_retired_lock, catalog_set_aside] = &catalog;

    // #2434 - configured instructions filenames beyond the floor, escaped.
    let mut custom_patterns: Vec<String> = Vec::new();
    for name in configured {
        if !crate::config::agent_command::is_safe_instructions_filename(name) {
            log::warn!(
                target: "agentscommander_lib::commands::ac_discovery::gitignore",
                "[ac-gitignore] dropping unsafe instructions filename {:?}",
                name
            );
            continue;
        }
        if name == "CLAUDE.md" || name == "AGENTS.md" {
            continue;
        }
        let pattern = format!("**/__agent_*/{}", escape_gitignore_name(name));
        if !custom_patterns.contains(&pattern) {
            custom_patterns.push(pattern);
        }
    }

    // Each entry: (pattern, comment explaining why)
    let mut required_entries: Vec<(&str, &str)> = vec![
        (
            "room-*/",
            "# AgentsCommander: exclude room cloned repos from parent git tracking.\n# Without \
                this, parent repo operations (checkout, reset) corrupt child clones.",
        ),
        (
            "wg-*/",
            "# AgentsCommander: exclude legacy room cloned repos from parent git tracking.\n# \
                Without this, parent repo operations (checkout, reset) corrupt child clones.",
        ),
        (
            ".deleting-*/",
            "# AgentsCommander: exclude temporary room delete sentinels/orphans.",
        ),
        loop_state_row,
        loop_state_tmp_row,
        loop_set_aside_row,
        (
            "_loop_*/audit.jsonl",
            "# AgentsCommander: exclude Loop runtime audit logs with prompt snapshots.",
        ),
        (
            "**/__agent_*/last_ac_context.md",
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ),
        (
            "**/__agent_*/CLAUDE.md",
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ),
        (
            "**/__agent_*/AGENTS.md",
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ),
        project_settings_row,
        (
            "/.team-config-write.lock",
            "# AgentsCommander: exclude team-config coordination files.",
        ),
        (
            "**/.config.json.lock",
            "# AgentsCommander: exclude local config write-lock sidecars.",
        ),
        (
            "/.project-settings.json.lock",
            "# AgentsCommander: exclude project-settings write-lock sidecars.",
        ),
        // #2717 - the migrated project settings take their own sidecar, and the
        // naming migration sets a displaced project file aside in `.ac/`.
        project_settings_lock_row,
        set_aside_row,
        (
            "_agent_*/rtk-matrix-history*.db",
            "# AgentsCommander: exclude RTK matrix-history databases.",
        ),
        (
            "_agent_*/rtk-matrix-history*.db-shm",
            "# AgentsCommander: exclude RTK matrix-history database sidecars.",
        ),
        (
            "_agent_*/rtk-matrix-history*.db-wal",
            "# AgentsCommander: exclude RTK matrix-history database sidecars.",
        ),
        (
            "_agent_*/rtk_ignored_tools*.md",
            "# AgentsCommander: exclude RTK ignored-tools runtime records.",
        ),
        (
            "_agent_*/rtk-ignored-tools-*.md",
            "# AgentsCommander: exclude RTK ignored-tools runtime records.",
        ),
        (
            "_agent_*/**/__pycache__/",
            "# AgentsCommander: exclude Python bytecode caches inside agent folders.",
        ),
        (
            "_agent_*/**/*.pyc",
            "# AgentsCommander: exclude Python bytecode inside agent folders.",
        ),
        // #1968 - the machine-local coding-agent catalog children. The tracked
        // `agents.json` and `_seed/` masters stay tracked; these are the
        // sidecars, lock and publication temporaries that must never be
        // committed. Composed from the registry: `config::instance_artifacts`
        // is a leaf with zero outgoing arcs, so an arc into it carries no path
        // back into the cyclic SCC, and modules across the tree, among them
        // `commands::config`, already hold that arc.
        (
            catalog_local.as_str(),
            "# AgentsCommander: exclude machine-local coding-agent overrides.",
        ),
        (
            catalog_migration_backup.as_str(),
            "# AgentsCommander: exclude the coding-agent migration backup (byte-exact \
                pre-migration source copy).",
        ),
        (
            catalog_migration_journal.as_str(),
            "# AgentsCommander: exclude the coding-agent migration journal.",
        ),
        (
            catalog_lock.as_str(),
            "# AgentsCommander: exclude the coding-agent catalog write-lock sidecar.",
        ),
        (
            catalog_base_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent base publication temporaries.",
        ),
        (
            catalog_local_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent local override publication temporaries.",
        ),
        (
            catalog_migration_backup_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent migration backup publication temporaries.",
        ),
        (
            catalog_migration_journal_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent migration journal publication temporaries \
                (the journal name starts with a dot).",
        ),
        // #2715 - the pre-migration lock sidecar the naming migration leaves on
        // disk for good, and every catalog file it sets aside.
        (
            catalog_retired_lock.as_str(),
            "# AgentsCommander: exclude the retired coding-agent catalog write-lock sidecar.",
        ),
        (
            catalog_set_aside.as_str(),
            "# AgentsCommander: exclude coding-agent catalog files the naming migration set aside.",
        ),
        // #2807 - the agent state file beside each tracked `config.json`, its
        // lock sidecar and its publish temporary.
        config_state_row,
        config_state_lock_row,
        config_state_tmp_row,
    ];
    for pattern in &custom_patterns {
        required_entries.push((
            pattern.as_str(),
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ));
    }

    if discovery::ac_root_gitignore_exists(&gitignore_path) {
        let content = discovery::read_ac_root_gitignore(&gitignore_path)
            .map_err(|e| format!("Failed to read Project AC Root .gitignore: {}", e))?;
        let (content, migrated) = migrate_legacy_seed_manifest_gitignore(content);
        // #2715 - the append-only writer never removes a row, so the catalog
        // rows the naming migration renamed are retired here.
        let (content, retired) = crate::config::naming_migration::retire_ignore_pairs(
            &content,
            &[
                RETIRED_CATALOG_IGNORE_PAIRS.as_slice(),
                &RETIRED_PROJECT_SETTINGS_IGNORE_PAIRS,
                &RETIRED_LOOP_STATE_IGNORE_PAIRS,
            ]
            .concat(),
        );
        let migrated = migrated || retired;

        let mut additions = String::new();
        for (pattern, comment) in &required_entries {
            let is_present = content.lines().any(|line| {
                if *pattern == project_settings_gitignore_pattern {
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

        if migrated || !additions.is_empty() {
            let separator = if content.ends_with('\n') { "" } else { "\n" };
            discovery::write_ac_root_gitignore(
                &gitignore_path,
                &format!("{}{}{}", content, separator, additions),
            )
            .map_err(|e| format!("Failed to update Project AC Root .gitignore: {}", e))?;
        }
    } else {
        let mut content = String::new();
        for (pattern, comment) in &required_entries {
            content.push_str(&format!("{}\n{}\n\n", comment, pattern));
        }
        content.push_str(SEED_MANIFEST_COORDINATION_BLOCK);
        content.push('\n');
        content.push_str(SEED_MANIFEST_MANIFEST_BLOCK);
        discovery::write_ac_root_gitignore(&gitignore_path, &content)
            .map_err(|e| format!("Failed to create Project AC Root .gitignore: {}", e))?;
    }

    Ok(())
}

/// #2715 - the three `.ac/.gitignore` pairs whose value the `agents.*` rename
/// changed. The lock row is not here: its leftover stays on disk and ignored.
pub(crate) const RETIRED_CATALOG_IGNORE_PAIRS: [(&str, &str); 3] = [
    (
        "# AgentsCommander: exclude machine-local coding-agent overrides.",
        "/coding-agents/agents.local.json",
    ),
    (
        "# AgentsCommander: exclude coding-agent base publication temporaries.",
        "/coding-agents/.agents.json.*.tmp",
    ),
    (
        "# AgentsCommander: exclude coding-agent local override publication temporaries.",
        "/coding-agents/.agents.local.json.*.tmp",
    ),
];

/// #2717 - the `.ac/.gitignore` pair whose value the project-settings rename
/// changed. The lock row is not here: its leftover stays on disk and ignored.
pub(crate) const RETIRED_PROJECT_SETTINGS_IGNORE_PAIRS: [(&str, &str); 1] = [(
    "# AgentsCommander: exclude generated project-local settings.",
    "/project-settings.json",
)];

/// #2718 - the `.ac/.gitignore` pair whose value the Loop state rename changed.
pub(crate) const RETIRED_LOOP_STATE_IGNORE_PAIRS: [(&str, &str); 1] = [(
    "# AgentsCommander: exclude Loop scheduler runtime state.",
    "_loop_*/state.json",
)];

/// #2090 - migrate the retired `!/seed-manifest.toml` un-ignore pair written by
/// older builds. Only the two AC-managed lines (the retired comment directly
/// above the negation and, when the managed block wrote one, the blank line
/// before that comment) are removed; a bare `!/seed-manifest.toml` without the
/// retired comment is user intent and is preserved. Every other byte, including
/// CRLF and a missing final newline, is preserved; the flag is false when
/// nothing changed.
pub(crate) fn migrate_legacy_seed_manifest_gitignore(content: String) -> (String, bool) {
    const LEGACY_COMMENT: &str =
        "# AgentsCommander: keep the seed publication manifest reviewable.";
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
