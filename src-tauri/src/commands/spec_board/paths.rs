use std::path::PathBuf;

pub(super) use crate::spec_board::paths::path_compare_key;
pub(super) use crate::spec_board::paths::{
    resolve_repo_root, validate_existing_user_path, validate_new_or_existing_user_path,
};

// Retained for existing spec-path callers; discovery now owns a private equivalent.
#[allow(dead_code)]
pub(super) fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let key = path_compare_key(&path);
    if !paths.iter().any(|p| path_compare_key(p) == key) {
        paths.push(path);
    }
}
