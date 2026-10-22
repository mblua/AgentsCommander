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

/// TASK.md content for a brand-new workgroup.
///
/// Empty titles start Clean; explicit titles use the title editor's frontmatter.
pub(super) fn build_task_content(task_title: &str) -> String {
    if task_title.trim().is_empty() {
        return "---\ntitle: 'Clean'\n---\n".to_string();
    }
    // #738: the workgroup-creation title is a human decision, so store it as
    // user-owned (`USER:`). This locks it against coordinator auto-retitle until
    // a Clean resets the task. `user_owned_title` trims and avoids double-prefix.
    let title = task_ops::user_owned_title(task_title);
    let escaped = title.replace('\'', "''");
    format!("---\ntitle: '{}'\n---\n", escaped)
}

/// Validate and prepare the initial task before the GUI creates any files.
pub(super) fn prepare_new_room_task_content(task_title: &str) -> Result<String, String> {
    let task_title = task_title.trim();
    if !task_title.is_empty() {
        validate_task_title(task_title)?;
    }
    Ok(build_task_content(task_title))
}

pub(super) fn validate_task_title(task_title: &str) -> Result<(), String> {
    if task_title.is_empty() {
        return Err("Task title cannot be empty".to_string());
    }
    if task_title.chars().any(|c| c.is_control() && c != '\t') {
        return Err("Task title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
            .to_string());
    }
    if task_title.chars().count() > 256 {
        return Err("Task title is too long (max 256 characters)".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------
