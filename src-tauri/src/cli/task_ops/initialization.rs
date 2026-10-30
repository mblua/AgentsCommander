//! Initial content and publication for newly allocated rooms.
use crate::filesystem::projects::entities::publication;
use std::io;
use std::path::Path;

pub(crate) const TASK_FILE_NAME: &str = "TASK.md";

pub(crate) fn initialize_new_room(root: &Path, prepared_content: &str) -> io::Result<()> {
    publication::write_initial_room_task(&root.join(TASK_FILE_NAME), prepared_content)
}

/// TASK.md content for a brand-new workgroup.
///
/// Empty titles start Clean; explicit titles use the title editor's frontmatter.
pub(crate) fn build_task_content(task_title: &str) -> String {
    if task_title.trim().is_empty() {
        return "---\ntitle: 'Clean'\n---\n".to_string();
    }
    // #738: the workgroup-creation title is a human decision, so store it as
    // user-owned (`USER:`). This locks it against coordinator auto-retitle until
    // a Clean resets the task. `user_owned_title` trims and avoids double-prefix.
    let title = super::user_owned_title(task_title);
    let escaped = title.replace('\'', "''");
    format!("---\ntitle: '{}'\n---\n", escaped)
}

/// Validate and prepare the initial task before the GUI creates any files.
pub(crate) fn prepare_new_room_task_content(task_title: &str) -> Result<String, String> {
    let task_title = task_title.trim();
    if !task_title.is_empty() {
        validate_task_title(task_title)?;
    }
    Ok(build_task_content(task_title))
}

pub(crate) fn validate_task_title(task_title: &str) -> Result<(), String> {
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
