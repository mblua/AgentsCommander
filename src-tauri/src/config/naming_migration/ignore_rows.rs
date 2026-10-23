use super::{
    CONFIG_STATE_TARGET_NAME, LOOP_STATE_TARGET_NAME, LOOP_STATE_TMP_TARGET_GLOB,
    PROJECT_SETTINGS_TARGET_NAME, SET_ASIDE_GLOB,
};

/// #2717 (B4a) - the three `.ac/.gitignore` rows the project-settings rename
/// needs, as `(pattern, comment)`. The one home of this table: the writer at
/// registration (`commands::ac_discovery`) and the migration's own sweep
/// (`config::project_settings`) both read it here. Composed from the registry
/// at runtime, so no second spelling of a name exists in production.
pub(crate) fn project_settings_ignore_rows() -> [(String, &'static str); 3] {
    [
        (
            format!("/{PROJECT_SETTINGS_TARGET_NAME}"),
            "# AgentsCommander: exclude generated project-local settings.",
        ),
        (
            format!("/.{PROJECT_SETTINGS_TARGET_NAME}.lock"),
            "# AgentsCommander: exclude the project settings write-lock sidecar.",
        ),
        (
            format!("/{SET_ASIDE_GLOB}"),
            "# AgentsCommander: exclude project files the naming migration set aside.",
        ),
    ]
}

/// #2718 (B4b) - the three `.ac/.gitignore` rows the Loop state rename needs,
/// as `(pattern, comment)`, read by the writer at registration and by the Loop
/// scope's own sweep. Composed from the registry at runtime.
pub(crate) fn loop_state_ignore_rows() -> [(String, &'static str); 3] {
    [
        (
            format!("_loop_*/{LOOP_STATE_TARGET_NAME}"),
            "# AgentsCommander: exclude Loop scheduler runtime state.",
        ),
        (
            format!("_loop_*/{LOOP_STATE_TMP_TARGET_GLOB}"),
            "# AgentsCommander: exclude Loop state write temporaries.",
        ),
        (
            format!("_loop_*/{SET_ASIDE_GLOB}"),
            "# AgentsCommander: exclude Loop files the naming migration set aside.",
        ),
    ]
}

/// #2807 (C2) - the three `.ac/.gitignore` rows the agent state file needs, as
/// `(pattern, comment)`, read by the writer at registration and by the config
/// pair's own sweep. Composed from the registry at runtime.
pub(crate) fn config_state_ignore_rows() -> [(String, &'static str); 3] {
    [
        (
            format!("_agent_*/**/{CONFIG_STATE_TARGET_NAME}"),
            "# AgentsCommander: exclude agent runtime state.",
        ),
        (
            format!("**/.{CONFIG_STATE_TARGET_NAME}.lock"),
            "# AgentsCommander: exclude the agent state write-lock sidecar.",
        ),
        (
            format!("_agent_*/**/.{CONFIG_STATE_TARGET_NAME}.*.tmp"),
            "# AgentsCommander: exclude agent state write temporaries.",
        ),
    ]
}

/// The managed blocks, `\n<comment>\n<pattern>\n`, of each `(pattern, comment)`
/// pair of `rows` whose pattern line is absent from `content`; empty when there
/// is nothing to add. Presence is exact line equality, so a line with a leading
/// space, which Git does not read as the rule, does not count. Pure text.
pub(crate) fn missing_ignore_rows(content: &str, rows: &[(String, &str)]) -> String {
    let mut blocks = String::new();
    for (pattern, comment) in rows {
        if !content.lines().any(|line| line == pattern) {
            blocks.push_str(&format!("\n{comment}\n{pattern}\n"));
        }
    }
    blocks
}

/// Removes exactly the AC-written `comment` + `pattern` pairs named in `retired`
/// from an ignore body, with the one blank line the managed block wrote before
/// the comment. A `pattern` line with no comment of its own directly above it
/// is user intent and is preserved. CRLF and a missing final newline survive
/// byte for byte. Pure text. Generalizes `migrate_legacy_seed_manifest_gitignore`.
pub(crate) fn retire_ignore_pairs(content: &str, retired: &[(&str, &str)]) -> (String, bool) {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut removed = vec![false; lines.len()];
    let mut changed = false;
    for (index, line) in lines.iter().enumerate() {
        let Some(previous) = index.checked_sub(1) else {
            continue;
        };
        let is_pair = retired.iter().any(|(comment, pattern)| {
            line.trim() == *pattern && lines[previous].trim() == *comment
        });
        if !is_pair {
            continue;
        }
        removed[previous] = true;
        removed[index] = true;
        if let Some(blank) = previous.checked_sub(1) {
            if lines[blank].trim().is_empty() && !removed[blank] {
                removed[blank] = true;
            }
        }
        changed = true;
    }
    if !changed {
        return (content.to_string(), false);
    }
    let migrated: String = lines
        .iter()
        .zip(&removed)
        .filter(|(_, drop)| !**drop)
        .map(|(line, _)| *line)
        .collect();
    (migrated, true)
}
