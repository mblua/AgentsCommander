use super::*;
use std::collections::HashSet;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use super::super::instance_artifacts::{
    AGENTS_INSTANCE_FILE_NAME, AGENTS_INSTANCE_TARGET_NAME, BLOCKING_MENUS_LOCAL_FILE_NAME,
    BLOCKING_MENUS_LOCAL_TARGET_NAME, BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
    BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME, BLOCKING_MENUS_REMOTE_FILE_NAME,
    BLOCKING_MENUS_REMOTE_TARGET_NAME, BLOCKING_MENUS_SHIPPED_FILE_NAME,
    BLOCKING_MENUS_SHIPPED_TARGET_NAME, CODING_AGENTS_BASE_FILENAME,
    CODING_AGENTS_BASE_TARGET_NAME, CODING_AGENTS_LOCAL_FILENAME, CODING_AGENTS_LOCAL_TARGET_NAME,
    CODING_AGENTS_LOCK_FILENAME, CODING_AGENTS_LOCK_TARGET_NAME,
    CODING_AGENTS_MIGRATION_BACKUP_FILENAME, CODING_AGENTS_MIGRATION_JOURNAL_FILENAME,
    CODING_AGENTS_PROJECT_TARGET_NAME, LAYERED_NAME_PROBES, LAYER_DEFAULT, LAYER_INSTANCE,
    LAYER_PERSONAL, LAYER_PROJECT, LAYER_REMOTE, NO_GIT_MARKER, SETTINGS_BACKUP_PREFIX,
    SETTINGS_BACKUP_ROTATION_GLOB, SETTINGS_BACKUP_ROTATION_TARGET_GLOB, SETTINGS_BACKUP_SUFFIX,
    SETTINGS_BACKUP_TARGET_PREFIX, SETTINGS_FILE_NAME, SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
    SETTINGS_LOCAL_TARGET_NAME, SETTINGS_LOCK_FILE_NAME, SETTINGS_LOCK_TARGET_NAME,
    SETTINGS_MIGRATION_BACKUP_GLOB, SETTINGS_MIGRATION_BACKUP_TARGET_GLOB, SETTINGS_TARGET_NAME,
    STATE_MARKER,
};

const TEST_AGENT_LOCAL_DIR: &str = ".agentscommander_amp-office";

fn git(repo: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git")
}

fn assert_git_success(repo: &Path, args: &[&str]) -> Output {
    let output = git(repo, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_git_ignore_status(repo: &Path, relative_path: &str, expected_code: i32) {
    let output = git(
        repo,
        &["check-ignore", "--no-index", "--quiet", "--", relative_path],
    );
    assert_eq!(
        output.status.code(),
        Some(expected_code),
        "unexpected ignore status for {relative_path}; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The 12 rules this policy emitted before the registry existed, frozen as
/// bytes because they leave the production code in this change.
///
/// They are what an existing installation's complete `.gitignore` contains,
/// so they are the seed of the compatibility tests and the definition of
/// which registry rows are new.
const HISTORICAL_FIXED_RULES: [&str; 12] = [
    "/.agentscommander-injected-messages.json",
    "/app-outbox-path.txt",
    "/app.log",
    "/daemon.pid",
    "/injected-messages.default.toml",
    "/injected-messages.toml",
    "/injected-messages.toml.bak-*",
    "/master-token.txt",
    "/sessions.json",
    "/settings.json",
    "/update-check.json",
    "/web-token.txt",
];

fn ignore_rows() -> Vec<&'static InstanceArtifact> {
    super::super::instance_artifacts::INSTANCE_ARTIFACTS
        .iter()
        .filter(|artifact| artifact.disposition == Disposition::Ignore)
        .collect()
}

fn track_rows() -> Vec<&'static InstanceArtifact> {
    super::super::instance_artifacts::INSTANCE_ARTIFACTS
        .iter()
        .filter(|artifact| artifact.disposition == Disposition::Track)
        .collect()
}

fn push_pair(bytes: &mut Vec<u8>, comment: &str, pattern: &str) {
    bytes.extend_from_slice(comment.as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(pattern.as_bytes());
    bytes.push(b'\n');
}

/// The exact bytes a fresh file must contain, derived from the registry
/// table.
///
/// This is the single place any expected count comes from: the two dynamic
/// pairs are spelled out here because they are code, every other pair comes
/// from the table, and no test retypes a total.
fn expected_fresh_bytes(agent_local_dir: &str) -> Vec<u8> {
    let escaped = escape_gitignore_path_segment(agent_local_dir).expect("escape agent local dir");
    let mut bytes = Vec::new();
    push_pair(
        &mut bytes,
        "# AgentsCommander: per-instance override of the root agent's config.",
        &format!(
            "/{}/{escaped}/config.json",
            super::super::ROOT_AGENT_DIR_NAME
        ),
    );
    push_pair(
        &mut bytes,
        "# AgentsCommander: runtime config of the managed root agent.",
        &format!("/{}/config.json", super::super::ROOT_AGENT_DIR_NAME),
    );
    for artifact in ignore_rows() {
        let rendered = render(artifact);
        push_pair(&mut bytes, rendered.comment, &rendered.pattern);
    }
    bytes
}

/// The byte-exact content of a pre-#1446 complete file: the two dynamic
/// rules composed from the constant, then the 12 historical rules.
///
/// The dynamic lines are built rather than frozen on purpose. Freezing them
/// would couple this seed to the value of `ROOT_AGENT_DIR_NAME`, so a change
/// to that constant would surface as a wrong appended count instead of
/// naming its own cause.
fn legacy_complete_bytes(agent_local_dir: &str) -> Vec<u8> {
    let escaped = escape_gitignore_path_segment(agent_local_dir).expect("escape agent local dir");
    let mut bytes = Vec::new();
    for line in [
        format!(
            "/{}/{escaped}/config.json",
            super::super::ROOT_AGENT_DIR_NAME
        ),
        format!("/{}/config.json", super::super::ROOT_AGENT_DIR_NAME),
    ] {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    for line in HISTORICAL_FIXED_RULES {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

mod ensure;
mod git_fixture;
mod registry_names;
