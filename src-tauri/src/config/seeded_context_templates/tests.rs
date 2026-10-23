use super::*;
use crate::config::session_context::{
    get_default_coordinator_template, COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
    GLOBAL_CONTEXT_TEMPLATE_FILENAME,
};

mod coordinator_read_sync;
mod frozen_snapshots;
mod global_entry_version;
mod project_publication;
mod retire_global;
mod scan_global_platform;
mod scan_global_v3;

fn hash_text(content: &str) -> String {
    sha256_hex(content.as_bytes())
}

/// #1748: until phase 02 returns the replacements from the public scan, the
/// tests read them through the internal collector the scan already builds.
fn scan_project_context_template_replacements_for_test(
    project_dir: &Path,
    ac_root: &Path,
) -> Result<Vec<ContextTemplateReplacement>, String> {
    let mut clock = fixed_publication_time;
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    scan_project_context_templates_with_clock(project_dir, ac_root, &mut clock, &mut on_publication)
        .map(|(_updates, replacements)| replacements)
}

/// Every `*.bak` in the workspace, sorted. The backup COUNT is what detects a
/// spurious rewrite; mtime cannot (see the new-test-3 comment).
fn backup_files(ac_root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(ac_root)
        .expect("read workspace")
        .map(|entry| entry.expect("workspace entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".bak"))
        })
        .collect();
    found.sort();
    found
}

/// A trusted `global` state entry written as raw JSON, so the fixture pins the
/// wire keys rather than the struct shape phase 01b retypes.
fn write_trusted_global_state(ac_root: &Path, last_seeded_sha256: &str) {
    std::fs::write(
        ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
        format!(
            concat!(
                "{{\"schemaVersion\":1,\"templates\":{{\"global\":{{",
                "\"templateId\":\"global\",\"currentVersion\":3,",
                "\"lastSeededSha256\":\"{}\"}}}}}}"
            ),
            last_seeded_sha256
        ),
    )
    .expect("write trusted global state");
}

fn fixed_publication_time() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-07-20T10:30:45.123Z")
        .expect("parse fixed publication time")
        .with_timezone(&chrono::Utc)
}

fn sync_for_read_at(
    ac_root: &Path,
    filename: &str,
    published_at: chrono::DateTime<chrono::Utc>,
) -> Vec<(&'static str, ContextPublication)> {
    let mut clock = || published_at;
    let mut publications = Vec::new();
    sync_project_context_template_for_read_with_clock(
        ac_root,
        filename,
        &mut clock,
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect("sync for read");
    publications
}

fn assert_one_publication(
    publications: &[(&'static str, ContextPublication)],
    filename: &'static str,
    published_at: chrono::DateTime<chrono::Utc>,
) {
    assert_eq!(
        publications,
        &[(filename, ContextPublication { published_at })]
    );
}

// ---------------------------------------------------------------- #979 retirement

fn live_global(config_dir: &Path) -> PathBuf {
    config_dir.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME)
}

fn state_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)
}
