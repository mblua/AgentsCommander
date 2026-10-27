use std::path::Path;

pub(crate) fn agent_matrix_directory_present(path: &Path) -> bool {
    path.is_dir()
}
