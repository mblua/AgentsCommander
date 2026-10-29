//! Physical filesystem operations for agent creation and memory directories.

use std::fs::{Metadata, ReadDir};
use std::path::{Path, PathBuf};

pub(crate) fn create_agent_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn write_agent_prompt(path: &Path, prompt: &str) -> std::io::Result<()> {
    std::fs::write(path, prompt)
}

pub(crate) fn create_memory_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn canonical_memory_root(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn memory_directory_metadata(path: &Path) -> std::io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn memory_directory_entries(path: &Path) -> std::io::Result<ReadDir> {
    std::fs::read_dir(path)
}

pub(crate) fn rename_memory_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

pub(crate) use super::context_templates::{
    ensure_template_directory as ensure_root_agent_directory,
    open_template_directory as open_root_agent_directory, TemplateFile as RootAgentDirectory,
};
pub(crate) use super::local_config::{
    canonical_local_config_parent as canonical_root_agent_path,
    create_local_config_stage as create_root_agent_stage,
    flush_local_config_stage as flush_root_agent_stage,
    inspect_local_config_target as inspect_root_agent_entry,
    link_absent_local_config_stage as publish_root_agent_missing,
    read_local_config_text as read_root_agent_role,
    remove_local_config_stage as remove_root_agent_temp,
    rename_local_config_stage as rename_root_agent_stage,
    sync_local_config_stage as sync_root_agent_stage,
    write_local_config_stage as write_root_agent_stage, LocalConfigStage as RootAgentStage,
};
#[cfg(windows)]
pub(crate) use super::local_config::{
    local_config_target_exists as root_agent_target_exists,
    replace_local_config_once as replace_root_agent_once,
};
pub(crate) use create_agent_directory as create_root_agent_skill_directory;

pub(crate) fn sync_root_agent_directory(directory: &RootAgentDirectory) -> std::io::Result<()> {
    directory.sync_all()
}

pub(crate) struct RootAgentSkillReader {
    file: std::fs::File,
}

pub(crate) fn open_root_agent_skill(path: &Path) -> std::io::Result<RootAgentSkillReader> {
    std::fs::File::open(path).map(|file| RootAgentSkillReader { file })
}

pub(crate) fn read_root_agent_skill_prefix(
    reader: RootAgentSkillReader,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader.file.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}
