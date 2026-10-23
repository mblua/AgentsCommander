use super::*;
use std::sync::{mpsc, Arc, Barrier};

mod coordinator_and_seed;
mod default_context_access;
mod default_context_render;
mod host_platform;
mod issue_2832;
mod issue_2868;
mod legacy_sections;
mod local_overlay_1737;
mod materialize;
mod root_and_git_scope;
mod root_builder_and_repos;
mod shared_locations;
mod skill_index;
mod template_heal;
mod template_render;
mod token_accounting;

fn issue_2832_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    crate::config::seeded_context_templates::ensure_root_context_template(temp.path()).unwrap();
    std::fs::write(
        root.join("Role.md"),
        "---\nname: root\n---\n# Role\nROOT ROLE\n",
    )
    .unwrap();
    (temp, root)
}

fn with_issue_2832_root_config<T>(config_dir: &Path, action: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ROOT_CONTEXT_TEST_CONFIG.with(|slot| *slot.borrow_mut() = None);
        }
    }
    ROOT_CONTEXT_TEST_CONFIG.with(|slot| {
        assert!(slot.borrow().is_none(), "nested Root fixture");
        *slot.borrow_mut() = Some(config_dir.to_path_buf());
    });
    let _reset = Reset;
    action()
}

fn issue_2832_combined(root: &Path, config_dir: &Path, auto: bool) -> String {
    with_issue_2832_root_config(config_dir, || {
        let managed = materialize_agent_context_file_with_filename(
            &path_string(root),
            "AGENTS.md",
            &[],
            true,
            auto,
            None,
        )
        .unwrap()
        .unwrap();
        std::fs::read_to_string(managed).unwrap()
    })
}

fn no_skill_section() -> String {
    render_skills_section(&discover_skill_index(None))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn fixed_publication_time() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-07-20T10:30:45.123Z")
        .expect("parse fixed publication time")
        .with_timezone(&chrono::Utc)
}

fn canonical_display_path(path: &Path) -> String {
    std::fs::canonicalize(path)
        .map(|canonical| display_path(&canonical))
        .unwrap_or_else(|_| display_path(path))
}

fn assert_contains_canonical_path(content: &str, path: &Path) {
    let expected = canonical_display_path(path);
    assert!(
        content.contains(&expected),
        "content should contain canonical path {expected}"
    );
}

fn assert_no_raw_template_placeholders(out: &str) {
    assert!(
        !out.contains("{{"),
        "raw opening placeholder marker found:\n{out}"
    );
    assert!(
        !out.contains("}}"),
        "raw closing placeholder marker found:\n{out}"
    );
}

fn assert_mandatory_sections_once(out: &str) {
    assert_eq!(count_context_occurrences(out, "## GOLDEN RULE"), 1);
    assert_eq!(
        count_context_occurrences(out, "## Delegated Task Reporting"),
        1
    );
    assert_eq!(count_context_occurrences(out, "## Skills"), 1);
    assert_eq!(count_context_occurrences(out, "# Agent Repos"), 1);
    assert_eq!(count_context_occurrences(out, "## CLI executable"), 1);
    assert_eq!(count_context_occurrences(out, "## Host Platform Rules"), 1);
    assert_eq!(count_context_occurrences(out, "## Session credentials"), 1);
    assert_eq!(
        count_context_occurrences(out, "## Inter-Agent Messaging"),
        1
    );
}

fn assert_no_broad_read_grant(out: &str) {
    fn normalize(text: &str) -> String {
        let mut normalized = String::with_capacity(text.len());
        let mut pending_space = false;
        for ch in text.to_lowercase().chars() {
            if ch.is_alphanumeric() {
                if pending_space && !normalized.is_empty() {
                    normalized.push(' ');
                }
                normalized.push(ch);
                pending_space = false;
            } else {
                // This deliberately treats backticks, ASCII/Unicode hyphens,
                // punctuation, and repeated whitespace as the same separator.
                pending_space = true;
            }
        }
        normalized
    }

    const EXPLICIT_BROAD_GRANTS: [&str; 13] = [
        "Read any path",
        "Read-only operations on ANY path",
        "Read operations on any path",
        "Read any file",
        "Read all paths",
        "Read anywhere",
        "Any path is readable",
        "All paths are readable",
        "Unrestricted read access",
        "Read access is unrestricted",
        "No restrictions on reading",
        "May read without restriction",
        "Read the entire filesystem",
    ];
    const EXPLICIT_PROHIBITIVE_CROSS_WORKSPACE_READ: &str =
        "Do NOT list or explore the other workspace or room, read a second external file or any \
            other file or content there";

    let normalized = normalize(out);
    for fragment in EXPLICIT_BROAD_GRANTS {
        let fragment = normalize(fragment);
        assert!(
            !normalized.contains(&fragment),
            "broad read grant `{fragment}` found in:\n{out}"
        );
    }

    // This explicit policy sentence is a prohibition. Its comma-separated
    // trailing clause has read-any-file wording, so scan the whole
    // prohibition as one recognized negative rather than misclassifying it.
    let detector_input = out.replace(EXPLICIT_PROHIBITIVE_CROSS_WORKSPACE_READ, "");
    for raw_clause in detector_input.split(['\n', '\r', '.', ';', ':', ',', '!', '?']) {
        let clause = normalize(raw_clause);
        if clause.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = clause
            .split_whitespace()
            .map(|token| match token {
                "reads" | "reading" => "read",
                "accesses" | "accessing" => "access",
                "inspects" | "inspecting" => "inspect",
                "searches" | "searching" => "search",
                "paths" => "path",
                "files" => "file",
                "directories" => "directory",
                "locations" => "location",
                other => other,
            })
            .collect();
        let is_read_like = |token: &str| matches!(token, "read" | "access" | "inspect" | "search");
        let is_quantifier = |token: &str| matches!(token, "any" | "all");
        let is_universe = |token: &str| matches!(token, "path" | "file" | "directory" | "location");

        for (read_index, token) in tokens.iter().enumerate() {
            if !is_read_like(token) {
                continue;
            }
            let quantifier_end = (read_index + 7).min(tokens.len());
            for quantifier_index in (read_index + 1)..quantifier_end {
                if !is_quantifier(tokens[quantifier_index]) {
                    continue;
                }
                let universe_end = (quantifier_index + 4).min(tokens.len());
                if tokens[(quantifier_index + 1)..universe_end]
                    .iter()
                    .any(|token| is_universe(token))
                {
                    panic!("semantic broad read grant found in clause `{clause}`");
                }
            }
        }

        for (quantifier_index, token) in tokens.iter().enumerate() {
            if !is_quantifier(token) {
                continue;
            }
            let universe_end = (quantifier_index + 4).min(tokens.len());
            for universe_index in (quantifier_index + 1)..universe_end {
                if !is_universe(tokens[universe_index]) {
                    continue;
                }
                let readable_end = (universe_index + 7).min(tokens.len());
                for readable_index in (universe_index + 1)..readable_end {
                    let directly_readable = tokens[readable_index] == "readable";
                    let may_or_can_be_read = readable_index + 2 < tokens.len()
                        && matches!(tokens[readable_index], "may" | "can")
                        && tokens[readable_index + 1] == "be"
                        && tokens[readable_index + 2] == "read";
                    if directly_readable || may_or_can_be_read {
                        panic!("semantic broad read grant found in clause `{clause}`");
                    }
                }
            }
        }

        let has_read_like = tokens.iter().any(|token| is_read_like(token));
        let has_unbounded_modifier = tokens
            .iter()
            .any(|token| matches!(*token, "unrestricted" | "unlimited" | "everywhere"))
            || tokens.windows(2).any(|pair| {
                pair == ["without", "restriction"]
                    || pair == ["without", "limits"]
                    || pair == ["entire", "filesystem"]
            });
        assert!(
            !(has_read_like && has_unbounded_modifier),
            "semantic broad read grant found in clause `{clause}`"
        );
    }
}

// ---- #658 presence-aware append-fallback tests ----------------------------

/// Compact mirror of the real stale-hybrid `Context.AgentsCommander.md`: the
/// governance sections written INLINE with fine-grained tokens, but WITHOUT
/// the coarse `{{...}}` tokens the current renderer expects, and without an
/// inline `# Agent Repos` section. The Golden Rule heading carries a
/// trailing descriptor (exercises the prefix match) and the Self-discovery
/// prose carries the realistic backtick reference to `## Inter-Agent
/// Messaging` (exercises line-anchoring over `contains`).
const STALE_HYBRID_TEMPLATE: &str = concat!(
    r#"# AgentsCommander Context

You are running inside an AgentsCommander session.

## GOLDEN RULE - Repository Write Restrictions

You may ONLY modify files in the entries listed below:

1. Repositories whose root folder name starts with `repo-`.
2. Your own agent replica directory:
   ```
   {{AGENT_ROOT}}
   ```

{{MATRIX_SECTION}}{{MESSAGING_EXCEPTION}}Anything outside the allowed entries is READ-ONLY.

- **Allowed**: Read-only operations on ANY path
- **Allowed**: Full read/write inside your own replica root ({{AGENT_ROOT}})
{{MATRIX_ALLOWED}}{{MESSAGING_ALLOWED}}- **FORBIDDEN**: Any write operation outside "#,
    r#"{{FORBIDDEN_SCOPE}}.

**Clarification on git operations:** {{GIT_SCOPE}}

## Delegated Task Reporting

When finishing a delegated task or getting blocked, reply with a concrete artifact.

{{SKILLS_SECTION}}

## CLI executable

Your AgentsCommander credentials are in environment variables.

## Self-discovery via --help

For peer discovery, the sections below (`## Inter-Agent Messaging` and `### List available "#,
    r#"peers`) are the authoritative reference.

## Session credentials

Your session credentials are delivered only through the `AGENTSCOMMANDER_*` environment variables.

## Inter-Agent Messaging

### Send a message to another agent

Resolve the exact agent name via `list-peers-lean`.

**Peer name format**:

{{PEER_NAME_FORMAT}}

{{SEND_MESSAGE_INSTRUCTIONS}}

### List available peers

Run list-peers-lean.
"#
);

/// Render a raw global-context template through the function under change,
/// with an explicit `is_root_agent` flag. A temp-dir round trip cannot force
/// `is_root_agent` (it keys on `is_root_agent_path` vs the real config dir),
/// so the behavior is tested by calling the renderer directly (mirrors the
/// existing `default_context_as_root` helper).
fn render_global_template_for_test(
    template: &str,
    agent_root: &str,
    is_root_agent: bool,
) -> String {
    render_agent_context_template_inner(
        template,
        agent_root,
        None,
        &no_skill_section(),
        Path::new(agent_root),
        None,
        None,
        is_root_agent,
    )
}

/// Count real section HEADING LINES (a trimmed line, prefix-matched for the
/// Golden Rule descriptor, exact for the rest), NOT raw substrings. A raw
/// substring count over-counts the legitimate mid-line backtick reference
/// `` `## Inter-Agent Messaging` `` in the Self-discovery prose; this counts
/// only true headings, mirroring `mandatory_section_present_inline`.
fn count_section_headings(out: &str, heading: &str) -> usize {
    out.lines()
        .map(str::trim)
        .filter(|line| {
            if heading == "## GOLDEN RULE" {
                line.starts_with(heading)
            } else {
                *line == heading
            }
        })
        .count()
}

fn seed_stale_managed_context_files(agent_root: &Path) {
    for filename in MANAGED_CONTEXT_FILENAMES {
        let filename = *filename;
        std::fs::write(agent_root.join(filename), "STALE_MANAGED_CONTEXT")
            .expect("write stale managed context");
    }
}

fn assert_only_selected_managed_context_file_exists(agent_root: &Path, expected_filename: &str) {
    for filename in MANAGED_CONTEXT_FILENAMES {
        let filename = *filename;
        assert_eq!(
            agent_root.join(filename).exists(),
            filename == expected_filename,
            "managed context file presence mismatch for {filename}"
        );
    }
}

struct PartialFailWriter {
    file: std::fs::File,
    bytes_written: usize,
    fail_after_bytes: usize,
}

impl Write for PartialFailWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.bytes_written >= self.fail_after_bytes {
            return Err(std::io::Error::other("injected write failure"));
        }

        let remaining = self.fail_after_bytes - self.bytes_written;
        let write_len = remaining.min(buf.len());
        let written = self.file.write(&buf[..write_len])?;
        self.bytes_written += written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl ContextTemplateWriter for PartialFailWriter {
    fn sync_all(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct BlockingPartialWriter {
    file: std::fs::File,
    bytes_written: usize,
    first_chunk_len: usize,
    partial_written_tx: Option<mpsc::Sender<()>>,
    release_barrier: Arc<Barrier>,
}

impl Write for BlockingPartialWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.bytes_written == 0 {
            let write_len = self.first_chunk_len.min(buf.len());
            let written = self.file.write(&buf[..write_len])?;
            self.bytes_written += written;
            if let Some(tx) = self.partial_written_tx.take() {
                tx.send(()).expect("notify partial write");
            }
            self.release_barrier.wait();
            return Ok(written);
        }

        let written = self.file.write(buf)?;
        self.bytes_written += written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl ContextTemplateWriter for BlockingPartialWriter {
    fn sync_all(&mut self) -> std::io::Result<()> {
        self.file.sync_all()
    }
}

fn write_skill(matrix_root: &Path, folder: &str, content: &str) -> PathBuf {
    let skill_dir = matrix_root.join(SKILLS_DIR_NAME).join(folder);
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    let skill_path = skill_dir.join(SKILL_MD_FILENAME);
    std::fs::write(&skill_path, content).expect("write SKILL.md");
    skill_path
}

fn assert_global_context_before_one_role(content: &str, role_marker: &str) {
    assert!(content.contains("# AgentsCommander Context"));
    assert!(content.contains("# Context: Role.md"));
    assert_eq!(
        content.matches(role_marker).count(),
        1,
        "Role.md content should be included exactly once"
    );
    let global_index = content
        .find("# AgentsCommander Context")
        .expect("global context present");
    let role_index = content.find(role_marker).expect("role content present");
    assert!(
        global_index < role_index,
        "Role.md should be appended after the global context"
    );
}

// #664 self-heal tests --------------------------------------------------

/// Assert no `.Context.AgentsCommander.md.<pid>.<n>.tmp` scratch file was
/// left behind by an atomic replace in `dir`.
fn assert_no_context_template_temp_leftover(dir: &Path) {
    for entry in std::fs::read_dir(dir).expect("read context dir") {
        let name = entry.expect("dir entry").file_name();
        let name = name.to_string_lossy();
        assert!(
            !(name.starts_with(".Context.AgentsCommander.md") && name.ends_with(".tmp")),
            "stray temp template left behind: {name}"
        );
    }
}

fn issue_2868_cache(root: &Path) -> String {
    let cache = ensure_session_context(&display_path(root)).expect("publish session cache");
    std::fs::read_to_string(cache).unwrap()
}

// Deliberately independent of renderer constants: removing or weakening the
// authorization while leaving catalog entries must fail these cache tests.
fn issue_2868_assert_permission(content: &str, root: &Path) {
    // Canonicalize the authoritative parent, not an occupied source that
    // might be a rejected junction/link to an external target.
    let root = display_path(
        &canonical_or_original(root.parent().expect("project .ac parent"))
            .join(root.file_name().expect("project skills basename")),
    );
    let paragraph = format!(
        "Filesystem authorization amendment: You MAY READ {root} and its \
        descendants, including skill bodies and supporting resources. This is an explicit \
        additional exception to every preceding filesystem restriction in this context, including \
        the GOLDEN RULE absolute/exclusive entry ranges, the forbidden-read scope, the refusal \
        instruction, and any statement that nothing else under .ac is readable. Those restrictions \
        remain in force for all other paths. This amendment grants no write permission and no \
        access to external link/reference targets; those require an existing independent \
        permission. Private agent state and TASK.md write protection remain unchanged. This read \
        authorization also applies when no preceding filesystem rule exists."
    );
    assert_eq!(content.matches(&paragraph).count(), 1, "{content}");
    assert_eq!(content.matches("## Project Skills").count(), 1);
}

fn make_valid_replica(temp: &std::path::Path) -> (PathBuf, PathBuf, PathBuf, String) {
    let ac_root = temp.join(".ac");
    let room_root = ac_root.join("room-19-dev-team");
    let replica_root = room_root.join("__agent_dev-rust");
    let matrix_dir = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&replica_root).expect("replica dir");
    std::fs::create_dir_all(&matrix_dir).expect("matrix dir");
    std::fs::write(
        replica_root.join("config.json"),
        "{\"identity\": \"../../_agent_dev-rust\"}",
    )
    .expect("replica config");
    (
        canonical_or_original(&ac_root),
        canonical_or_original(&room_root),
        canonical_or_original(&replica_root),
        display_path(&canonical_or_original(&matrix_dir)),
    )
}
