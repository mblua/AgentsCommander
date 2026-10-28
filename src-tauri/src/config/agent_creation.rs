use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::filesystem::agent_files;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedAgentFolder {
    pub agent_dir: PathBuf,
    pub display_name: String,
    pub claude_md: String,
}

/// Creates an agent folder with a CLAUDE.md inside it.
///
/// This is the single backend implementation used by both the UI
/// `create_agent_folder` command and the CLI `create-agent` verb.
pub fn create_agent_folder_on_disk(
    parent_path: &str,
    agent_name: &str,
) -> Result<CreatedAgentFolder, String> {
    let parent = PathBuf::from(parent_path);
    if !crate::filesystem::directories::agent_parent_exists(&parent) {
        return Err(format!("Parent folder does not exist: {}", parent_path));
    }

    let agent_name = agent_name.trim();
    if agent_name.is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }
    if agent_name.contains('/') || agent_name.contains('\\') || agent_name.contains('\0') {
        return Err("Agent name cannot contain path separators".to_string());
    }

    let agent_dir = parent.join(agent_name);
    match agent_files::create_agent_directory(&agent_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            return Err(format!("Folder already exists: {}", agent_dir.display()));
        }
        Err(e) => return Err(format!("Failed to create folder: {}", e)),
    }

    let parent_name = parent
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| parent_path.to_string());
    let display_name = format!("{}/{}", parent_name, agent_name);

    let claude_md = format!("You are the agent {}", display_name);
    let claude_path = agent_dir.join("CLAUDE.md");
    agent_files::write_agent_prompt(&claude_path, &claude_md)
        .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;

    // TODO: When replica creation is added (for __agent_* dirs inside workgroups),
    // write config.json with: { "context": ["$AGENTSCOMMANDER_CONTEXT"] }
    // so that replicas get the global context by default.

    Ok(CreatedAgentFolder {
        agent_dir,
        display_name,
        claude_md,
    })
}

pub(crate) fn existing_picker_default(path: &Path) -> bool {
    crate::filesystem::directories::picker_default_exists(path)
}

#[cfg(test)]
mod tests;
