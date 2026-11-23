//! #4845 layering guard: the acyclic core `crate::config::coding_agent_files` may name
//! only itself, `crate::filesystem::settings_sources` and `crate::path_utils` under
//! `crate::`; its root file may not use `super::` nor re-export a child (AC-4845-003).
//! A text scan: comments are stripped, names are not resolved. The cycle detector
//! (`npm run check:cycles`, "new: 0") is the authoritative check.

use std::fs;
use std::path::{Path, PathBuf};

const ALLOWED: [&str; 3] = [
    "crate::config::coding_agent_files",
    "crate::filesystem::settings_sources",
    "crate::path_utils",
];
const CHILDREN: [&str; 5] = [
    "locator",
    "inventory",
    "last_used",
    "launch_chain",
    "launched",
];

fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|line| line.find("//").map_or(line, |at| &line[..at]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `crate::` path in `code` that starts with none of the allowed prefixes.
fn forbidden_paths(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = code[from..].find("crate::") {
        let start = from + at;
        let preceded = code[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let end = code[start..]
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
            .map_or(code.len(), |len| start + len);
        let path = &code[start..end];
        if !preceded && !ALLOWED.iter().any(|allowed| path.starts_with(allowed)) {
            found.push(path.to_string());
        }
        from = end.max(start + 1);
    }
    found
}

fn root_reexports(code: &str) -> Vec<String> {
    code.lines()
        .map(str::trim)
        .filter_map(|line| {
            let rest = line
                .strip_prefix("pub(crate) use ")
                .or_else(|| line.strip_prefix("pub use "));
            let rest = rest.or_else(|| line.strip_prefix("use "))?;
            let path = rest.trim_start_matches("self::");
            CHILDREN
                .iter()
                .any(|child| {
                    path.starts_with(&format!("{child}::"))
                        || path.starts_with(&format!("{child};"))
                })
                .then(|| line.to_string())
        })
        .collect()
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn core_files() -> Vec<PathBuf> {
    let root = src().join("config/coding_agent_files.rs");
    let mut files = vec![root];
    let dir = src().join("config/coding_agent_files");
    let mut children: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("core directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| path.file_name().is_some_and(|name| name != "tests.rs"))
        .collect();
    children.sort();
    files.extend(children);
    files
}

#[test]
fn the_core_names_only_its_allowed_modules() {
    let synthetic = "use crate::config::settings::AppSettings; // crate::commands::x";
    assert_eq!(
        forbidden_paths(&strip_comments(synthetic)),
        ["crate::config::settings::AppSettings"],
        "positive control: the scan must report a settings path"
    );
    assert_eq!(
        root_reexports("pub(crate) use self::locator::context_key;").len(),
        1
    );

    let files = core_files();
    assert!(files.len() >= 3, "the scan read too few files: {files:?}");
    let mut offenders = Vec::new();
    for path in &files {
        let code = strip_comments(&fs::read_to_string(path).expect("read core file"));
        for found in forbidden_paths(&code) {
            offenders.push(format!("{}: {found}", path.display()));
        }
        if path.ends_with("coding_agent_files.rs") {
            if code.contains("super::") {
                offenders.push(format!("{}: super:: in the root file", path.display()));
            }
            for line in root_reexports(&code) {
                offenders.push(format!("{}: re-export `{line}`", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "config::coding_agent_files must stay acyclic (no settings types, no re-exports; \
         AC-4845-003): {offenders:#?}"
    );
}
