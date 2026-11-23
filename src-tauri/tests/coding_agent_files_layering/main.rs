//! #4845 layering guard: `config::coding_agent_files` stays acyclic. Its files may
//! name only `crate::config::coding_agent_files`, `crate::filesystem::settings_sources`
//! and `crate::path_utils`; the root file may not use `super::`.
//!
//! A text scan, like `tests/project_settings_layering`: it matches spellings and does
//! not resolve names. The cycle detector (`npm run check:cycles`) is authoritative.

use std::fs;
use std::path::{Path, PathBuf};

const ROOT_FILE: &str = "src/config/coding_agent_files.rs";
const CHILD_DIR: &str = "src/config/coding_agent_files";
const ALLOWED: [&str; 3] = [
    "crate::config::coding_agent_files",
    "crate::filesystem::settings_sources",
    "crate::path_utils",
];

fn strip_line_comments(text: &str) -> String {
    text.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `crate::` path in `text` that does not start with an allowed prefix,
/// plus `super::` when `is_root`.
fn violations(text: &str, is_root: bool) -> Vec<String> {
    let code = strip_line_comments(text);
    let mut found = Vec::new();
    for (index, _) in code.match_indices("crate::") {
        let before = code[..index].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let path: String = code[index..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
            .collect();
        if !ALLOWED.iter().any(|allowed| path.starts_with(allowed)) {
            found.push(path);
        }
    }
    if is_root && code.contains("super::") {
        found.push("super::".to_string());
    }
    found
}

fn guarded_files(manifest: &Path) -> Vec<PathBuf> {
    let mut files = vec![manifest.join(ROOT_FILE)];
    let mut pending = vec![manifest.join(CHILD_DIR)];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("read guarded dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && path.file_name().is_some_and(|name| name != "tests.rs")
            {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn coding_agent_files_names_only_allowed_modules() {
    let probe = "use crate::config::settings::AppSettings;";
    assert_eq!(
        violations(probe, false),
        vec!["crate::config::settings::AppSettings".to_string()],
        "positive control"
    );
    assert_eq!(
        violations("use super::x;", true),
        vec!["super::".to_string()]
    );
    assert!(violations("use crate::path_utils; // crate::commands", true).is_empty());

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = guarded_files(manifest);
    assert!(files.len() >= 3, "guarded files: {files:?}");
    let mut failures = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read guarded file");
        let is_root = file.ends_with(ROOT_FILE);
        for path in violations(&text, is_root) {
            failures.push(format!("{}: {path}", file.display()));
        }
    }
    assert!(failures.is_empty(), "layering violations: {failures:#?}");
}
