use crate::cli::task_ops;
use std::path::Path;

/// Serialize the existing storage snapshot, preserving the original room key.
/// Reading may complete pending Clean recovery under the storage lock.
pub(crate) fn read_room_task_snapshot_json(root: &Path) -> Result<serde_json::Value, String> {
    let snapshot = task_ops::read_snapshot(root).map_err(|error| error.to_string())?;
    let mut value = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    value["workgroupRoot"] = serde_json::Value::String(
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(root),
    );
    Ok(value)
}

/// Extract a `title:` field from the YAML frontmatter at the start of `content`.
///
/// Best-effort frontmatter detection — NOT a YAML implementation. Suitable
/// only for the narrow case of one optional scalar field at the top of
/// TASK.md.
///
/// Returns `Some(title)` when:
///   - `content` starts with `---`,
///   - a closing `---` exists,
///   - a line of the form `<key>: <value>` exists between the delimiters
///     where `<key>` matches `title` case-insensitively (`title:`, `Title:`,
///     `TITLE:`, mixed casing all accepted).
///
/// The value half is preserved verbatim (case-sensitive), then stripped of
/// surrounding `"` or `'` quote pairs.
///
/// Returns `None` otherwise (no frontmatter, no title key, or empty value).
///
/// Mirrors `parse_role_frontmatter`'s shape — both speak the same on-disk
/// format. See plan `_plans/107-auto-brief-title.md` §6 for why we do not
/// pull in `serde_yaml`.
pub(crate) fn parse_task_title(content: &str) -> Option<String> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let frontmatter = &rest[..end];

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        // Case-insensitive key match on `title:`. Round 2 fold (F3 / G3):
        // agents stochastically capitalize keys (`Title:`, `TITLE:`); a
        // case-sensitive match would let duplicate `title:` lines accumulate
        // across restarts. Split on the first `:` so we compare just the key.
        let Some((key, value_raw)) = trimmed.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("title") {
            continue;
        }
        let value = value_raw
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        if value.is_empty() {
            return None;
        }
        return Some(value);
    }
    None
}

/// TASK content for a newly allocated room, owned by the TASK store.
pub(super) fn build_task_content(task_title: &str) -> String {
    task_ops::build_task_content(task_title)
}

pub(super) fn prepare_new_room_task_content(task_title: &str) -> Result<String, String> {
    task_ops::prepare_new_room_task_content(task_title)
}

pub(super) fn validate_task_title(task_title: &str) -> Result<(), String> {
    task_ops::validate_task_title(task_title)
}

pub(super) fn initialize_new_room(root: &Path, prepared_content: &str) -> std::io::Result<()> {
    task_ops::initialize_new_room(root, prepared_content)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------
