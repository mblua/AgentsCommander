use std::path::Path;

pub(crate) fn read_agent_help_text(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn read_agent_help_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn read_blocking_menus_text(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn read_blocking_menus_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}
