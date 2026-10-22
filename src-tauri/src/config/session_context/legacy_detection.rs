use super::{
    discover_skill_index, legacy_rendered_default_context_for_compat,
    pre_1072_legacy_rendered_default_context_for_compat, render_skills_section,
    resolve_skill_owner_root, GENERATED_SKILLS_SECTION_INTRO,
    GENERATED_SKILLS_SECTION_REPLICA_LINE,
    GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME,
    LEGACY_GENERATED_SKILLS_SECTION_INTRO,
};

pub(crate) enum LegacyRenderedDefaultContext {
    Current,
    StaleGenerated,
    NotLegacy,
}

pub(crate) fn classify_legacy_rendered_default_context(
    template: &str,
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
) -> LegacyRenderedDefaultContext {
    let normalized = normalize_context_for_compat(template);
    let current = normalize_context_for_compat(&current_legacy_rendered_default_context(
        agent_root,
        matrix_root,
        skills_section,
    ));

    if normalized == current {
        return LegacyRenderedDefaultContext::Current;
    }

    if looks_like_generated_legacy_default_context(&normalized) {
        return LegacyRenderedDefaultContext::StaleGenerated;
    }

    LegacyRenderedDefaultContext::NotLegacy
}

pub(crate) fn current_legacy_rendered_default_context(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
) -> String {
    legacy_rendered_default_context_for_compat(agent_root, matrix_root, skills_section)
}

pub(crate) fn looks_like_generated_legacy_default_context(normalized: &str) -> bool {
    if normalized.contains("{{") || normalized.contains("}}") {
        return false;
    }
    // #1369 (C4): both spellings. `# Workspace Repos` is frozen: it is what
    // contexts rendered by v1/v2 builds carry on disk. `# Agent Repos` is what
    // this build renders. Neither is in `is_known_legacy_default_heading`, so
    // this guard is a fast path, not the only rejection - AC-3 pins that.
    if normalized.contains("## Core Concepts")
        || normalized.contains("# Workspace Repos")
        || normalized.contains("# Agent Repos")
    {
        return false;
    }

    let Some(expected_candidates) = reconstruct_legacy_rendered_default_context(normalized) else {
        return false;
    };

    expected_candidates
        .iter()
        .any(|expected| normalize_context_for_compat(expected) == normalized)
}

pub(crate) fn reconstruct_legacy_rendered_default_context(normalized: &str) -> Option<[String; 2]> {
    let required_once = [
        "# AgentsCommander Context",
        "## GOLDEN RULE",
        "## Delegated Task Reporting",
        "## CLI executable",
        "## Self-discovery via --help",
        "## Session credentials",
        "### Send a message to another agent",
    ];
    if required_once
        .iter()
        .any(|marker| count_context_occurrences(normalized, marker) != 1)
    {
        return None;
    }
    if !normalized.contains("## Inter-Agent Messaging")
        || !normalized.contains("### List available peers")
        || !has_legacy_default_tail(normalized)
        || has_unknown_legacy_default_heading(normalized)
    {
        return None;
    }

    if !context_markers_in_order(
        normalized,
        &[
            "# AgentsCommander Context",
            "## GOLDEN RULE",
            "## Delegated Task Reporting",
            "## CLI executable",
            "## Self-discovery via --help",
            "## Session credentials",
            "## Inter-Agent Messaging",
            "### List available peers",
        ],
    ) {
        return None;
    }
    if !normalized.contains("The CLI `--help` output documents every subcommand")
        || !normalized.contains("The filesystem directory name is NEVER a valid `--to` value")
        || !normalized.contains(
            "\"<AGENTSCOMMANDER_BINARY_PATH>\" list-peers-lean --token <AGENTSCOMMANDER_TOKEN> \
                --root \"<AGENTSCOMMANDER_ROOT>\"",
        )
    {
        return None;
    }

    let agent_root = extract_legacy_code_block_after(normalized, "assigned root:")?;
    let matrix_root = if normalized.contains("3. **Your origin Agent Matrix") {
        Some(extract_legacy_code_block_after(
            normalized,
            "3. **Your origin Agent Matrix",
        )?)
    } else {
        None
    };
    let skills_section = extract_legacy_skills_section(normalized)?;
    let skill_owner_root = resolve_skill_owner_root(&agent_root, matrix_root.as_deref());
    if !is_provably_generated_legacy_skills_section(&skills_section, skill_owner_root.as_deref()) {
        return None;
    }

    Some([
        legacy_rendered_default_context_for_compat(
            &agent_root,
            matrix_root.as_deref(),
            &skills_section,
        ),
        pre_1072_legacy_rendered_default_context_for_compat(
            &agent_root,
            matrix_root.as_deref(),
            &skills_section,
        ),
    ])
}

pub(crate) fn extract_legacy_code_block_after(value: &str, marker: &str) -> Option<String> {
    let marker_pos = value.find(marker)? + marker.len();
    let after_marker = &value[marker_pos..];
    let fence_pos = after_marker.find("```")? + 3;
    let after_fence = after_marker[fence_pos..].strip_prefix('\n')?;
    let fence_end = after_fence.find("```")?;
    Some(after_fence[..fence_end].trim().to_string())
}

pub(crate) fn extract_legacy_skills_section(value: &str) -> Option<String> {
    let delegated = "## Delegated Task Reporting\n\nWhen finishing a delegated task or getting \
        blocked, you must explicitly reply to the coordinator or peer with a concrete artifact or \
        message. Do not just remain idle, waiting, or set working to false.\n\n";
    let start = value.find(delegated)? + delegated.len();
    let rest = &value[start..];
    let end = rest.find("\n\n## CLI executable")?;
    Some(rest[..end].to_string())
}

pub(crate) fn is_provably_generated_legacy_skills_section(
    section: &str,
    skill_owner_root: Option<&str>,
) -> bool {
    let expected = normalize_context_for_compat(&render_skills_section(&discover_skill_index(
        skill_owner_root,
    )));
    // #1614: a section generated before the Room rename carries the frozen
    // pre-rename replica line while `render_skills_section` now emits the
    // current one. CRLF-normalize first as defence in depth, since the byte-
    // pinned constant carries LF; swap while the line still carries its
    // terminating newline; then finish normalizing. Both halves of the swap
    // are constants, so the two sides cannot drift apart.
    let normalized = normalize_context_for_compat(&section.replace("\r\n", "\n").replace(
        GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME,
        GENERATED_SKILLS_SECTION_REPLICA_LINE,
    ));
    if normalized == expected {
        return true;
    }
    // Two byte-pinned swaps now feed this compare. #1005 S1 swaps the legacy
    // intro: a legacy context rendered before the intro rewrite embeds the
    // frozen OLD intro while `render_skills_section` now emits the current one.
    // #1614 swaps the byte-pinned pre-Room-rename replica line, above, for the
    // same reason. Either one left un-swapped flips every such file to NotLegacy
    // and kills #664 healing. Every other literal in `render_skills_section` is
    // frozen for this project; if one ever changes, this compare must extend
    // with it or healing dies silently.
    let Some(rest) = normalized.strip_prefix(LEGACY_GENERATED_SKILLS_SECTION_INTRO) else {
        return false;
    };
    let mut swapped = String::with_capacity(GENERATED_SKILLS_SECTION_INTRO.len() + rest.len());
    swapped.push_str(GENERATED_SKILLS_SECTION_INTRO);
    swapped.push_str(rest);
    normalize_context_for_compat(&swapped) == expected
}

pub(crate) fn has_legacy_default_tail(normalized: &str) -> bool {
    normalized.ends_with(
        "\"<AGENTSCOMMANDER_BINARY_PATH>\" list-peers-lean --token <AGENTSCOMMANDER_TOKEN> --root \
            \"<AGENTSCOMMANDER_ROOT>\"\n```",
    )
}

pub(crate) fn has_unknown_legacy_default_heading(normalized: &str) -> bool {
    normalized
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("# ") || line.starts_with("## ") || line.starts_with("### ")
        })
        .any(|line| !is_known_legacy_default_heading(line))
}

pub(crate) fn is_known_legacy_default_heading(line: &str) -> bool {
    line == "# AgentsCommander Context"
        || line.starts_with("## GOLDEN RULE")
        || matches!(
            line,
            "## Delegated Task Reporting"
                | "## Skills"
                | "### Available Skills"
                | "### Skill Discovery Warnings"
                | "## CLI executable"
                | "## Self-discovery via --help"
                | "## Session credentials"
                | "## Inter-Agent Messaging"
                | "### Send a message to another agent"
                | "### List available peers"
        )
}

pub(crate) fn context_markers_in_order(value: &str, markers: &[&str]) -> bool {
    let mut offset = 0;
    for marker in markers {
        let Some(found) = value[offset..].find(marker) else {
            return false;
        };
        offset += found + marker.len();
    }
    true
}

pub(crate) fn count_context_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

pub(crate) fn normalize_context_for_compat(value: &str) -> String {
    value.replace("\r\n", "\n").trim_end().to_string()
}
