//! Maintains the running instance's root `.gitignore`.
//!
//! The rules are not written here. `config::instance_artifacts` is the single
//! source: every static rule this module emits is derived from that registry's
//! table, one `# AgentsCommander: ...` comment line above each pattern, and the
//! two rules that depend on the running agent's local directory name are
//! composed at runtime because they cannot be static. Rows the registry marks
//! `Track` are deliberately never emitted.
//!
//! Coverage is the enumerated registry set, not blanket completeness. That set
//! includes the atomic-write temporary scheme and the off-shape temporary
//! schemes the registry enumerates, and a new artifact needs a new registry row
//! in the same change that introduces it.
//!
//! Reconciliation is append-only and byte-exact: an existing file keeps its
//! bytes and receives only the pairs whose pattern line is absent, so every
//! installation repairs itself on the next start with no migration. Comments
//! are transparent to that detection, so a user-authored pattern counts as
//! present and never has a comment retrofitted onto it.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::instance_artifacts::{ArtifactKind, Disposition, InstanceArtifact};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptResult {
    Done,
    RetryClassification,
}

/// One emitted entry: the exact `.gitignore` line, and the comment line the
/// generator writes above it.
///
/// Only `pattern` participates in detection. A comment can never equal a
/// pattern, so comments stay transparent to the append-only reconciliation.
pub(crate) struct RenderedRule {
    pattern: String,
    comment: &'static str,
}

/// Ensure the running instance's root `.gitignore` contains the complete
/// AgentsCommander runtime-file policy.
pub(crate) fn ensure_instance_gitignore() -> Result<(), String> {
    let config_dir = super::config_dir().ok_or_else(|| {
        "cannot seed the instance .gitignore because the config directory is unavailable"
            .to_string()
    })?;
    std::fs::create_dir_all(&config_dir).map_err(|error| {
        format!(
            "failed to create instance config directory {}: {error}",
            config_dir.display()
        )
    })?;

    ensure_instance_gitignore_at(&config_dir, &super::agent_local_dir_name())
}

fn ensure_instance_gitignore_at(config_dir: &Path, agent_local_dir: &str) -> Result<(), String> {
    let rules = required_rules(agent_local_dir)?;
    let path = config_dir.join(".gitignore");
    let mut retried = false;

    loop {
        let result = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                validate_regular_metadata(&path, &metadata, "named target")?;
                ensure_existing_file(&path, &rules)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_fresh_file(&path, &rules)?
            }
            Err(error) => {
                return Err(format!(
                    "failed to inspect instance .gitignore at {}: {error}",
                    path.display()
                ));
            }
        };

        match result {
            AttemptResult::Done => return Ok(()),
            AttemptResult::RetryClassification if !retried => retried = true,
            AttemptResult::RetryClassification => {
                return Err(format!(
                    "instance .gitignore target changed repeatedly at {}",
                    path.display()
                ));
            }
        }
    }
}

fn escape_gitignore_path_segment(segment: &str) -> Result<String, String> {
    let mut escaped = String::with_capacity(segment.len());
    for character in segment.chars() {
        match character {
            '\r' | '\n' => {
                return Err("agent local directory name contains a line break".to_string());
            }
            '/' => {
                return Err("agent local directory name contains a path separator".to_string());
            }
            '\0' => return Err("agent local directory name contains NUL".to_string()),
            '\\' | '*' | '?' | '[' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    Ok(escaped)
}

/// Render one registry row into the line git will read.
///
/// `Dir` rows carry a trailing slash so a plain file of the same name is not
/// silently ignored, and `GlobAnyDepth` rows are emitted unanchored so git
/// applies them at every depth under the instance directory. Everything else is
/// anchored to the instance root. The leading slash is added here and nowhere
/// else, which is what a registry test relies on when it refuses a name that
/// tries to decide its own anchoring.
fn render(artifact: &InstanceArtifact) -> RenderedRule {
    let pattern = match artifact.kind {
        ArtifactKind::File | ArtifactKind::Glob => format!("/{}", artifact.name),
        ArtifactKind::Dir => format!("/{}/", artifact.name),
        ArtifactKind::GlobAnyDepth => artifact.name.to_string(),
    };
    RenderedRule {
        pattern,
        comment: artifact.comment,
    }
}

fn required_rules(agent_local_dir: &str) -> Result<Vec<RenderedRule>, String> {
    let escaped_agent_local_dir = escape_gitignore_path_segment(agent_local_dir)?;

    let mut rules = vec![
        RenderedRule {
            pattern: format!(
                "/{}/{}/config.json",
                super::ROOT_AGENT_DIR_NAME,
                escaped_agent_local_dir
            ),
            comment: "# AgentsCommander: per-instance override of the root agent's config.",
        },
        RenderedRule {
            pattern: format!("/{}/config.json", super::ROOT_AGENT_DIR_NAME),
            comment: "# AgentsCommander: runtime config of the managed root agent.",
        },
    ];
    rules.extend(
        super::instance_artifacts::INSTANCE_ARTIFACTS
            .iter()
            .filter(|artifact| artifact.disposition == Disposition::Ignore)
            .map(render),
    );
    Ok(rules)
}

fn create_fresh_file(path: &Path, rules: &[RenderedRule]) -> Result<AttemptResult, String> {
    let mut file = match open_new_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(AttemptResult::RetryClassification);
        }
        Err(error) => {
            return Err(format!(
                "failed to create instance .gitignore at {}: {error}",
                path.display()
            ));
        }
    };

    validate_opened_regular_file(path, &file, "newly created target")?;
    acquire_nonblocking_lock(path, &file)?;
    validate_opened_regular_file(path, &file, "locked newly created target")?;

    let bytes = fresh_file_bytes(rules);
    file.write_all(&bytes).map_err(|error| {
        format!(
            "failed to write new instance .gitignore at {}: {error}",
            path.display()
        )
    })?;
    Ok(AttemptResult::Done)
}

fn ensure_existing_file(path: &Path, rules: &[RenderedRule]) -> Result<AttemptResult, String> {
    let mut read_file = match open_read_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AttemptResult::RetryClassification);
        }
        Err(error) => {
            return Err(format!(
                "failed to open instance .gitignore for reading at {}: {error}",
                path.display()
            ));
        }
    };
    validate_opened_regular_file(path, &read_file, "read target")?;
    let bytes = read_all(path, &mut read_file)?;
    validate_opened_regular_file(path, &read_file, "read target after inspection")?;
    if missing_rule_indexes(&bytes, rules).is_empty() {
        return Ok(AttemptResult::Done);
    }
    drop(read_file);

    let mut append_file = match open_append_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AttemptResult::RetryClassification);
        }
        Err(error) => {
            return Err(format!(
                "failed to open incomplete instance .gitignore for append at {}: {error}",
                path.display()
            ));
        }
    };
    validate_opened_regular_file(path, &append_file, "append target")?;
    acquire_nonblocking_lock(path, &append_file)?;
    validate_opened_regular_file(path, &append_file, "locked append target")?;

    let locked_bytes = read_all(path, &mut append_file)?;
    validate_opened_regular_file(path, &append_file, "append target after locked read")?;
    let missing = missing_rule_indexes(&locked_bytes, rules);
    if missing.is_empty() {
        return Ok(AttemptResult::Done);
    }

    let suffix = append_buffer(&locked_bytes, rules, &missing);
    append_file.write_all(&suffix).map_err(|error| {
        format!(
            "failed to append instance .gitignore rules at {}: {error}",
            path.display()
        )
    })?;
    Ok(AttemptResult::Done)
}

fn fresh_file_bytes(rules: &[RenderedRule]) -> Vec<u8> {
    let capacity = rules
        .iter()
        .map(|rule| rule.comment.len() + rule.pattern.len() + 2)
        .sum();
    let mut bytes = Vec::with_capacity(capacity);
    for rule in rules {
        push_rule(&mut bytes, rule);
    }
    bytes
}

fn push_rule(bytes: &mut Vec<u8>, rule: &RenderedRule) {
    bytes.extend_from_slice(rule.comment.as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(rule.pattern.as_bytes());
    bytes.push(b'\n');
}

fn missing_rule_indexes(bytes: &[u8], rules: &[RenderedRule]) -> Vec<usize> {
    rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            (!contains_exact_line(bytes, rule.pattern.as_bytes())).then_some(index)
        })
        .collect()
}

fn contains_exact_line(bytes: &[u8], rule: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        let logical_line = if line.last() == Some(&b'\r') {
            &line[..line.len() - 1]
        } else {
            line
        };
        logical_line == rule
    })
}

fn append_buffer(existing: &[u8], rules: &[RenderedRule], missing: &[usize]) -> Vec<u8> {
    let capacity = missing
        .iter()
        .map(|index| rules[*index].comment.len() + rules[*index].pattern.len() + 2)
        .sum::<usize>()
        + usize::from(!existing.is_empty() && !existing.ends_with(b"\n"));
    let mut suffix = Vec::with_capacity(capacity);
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        suffix.push(b'\n');
    }
    for index in missing {
        push_rule(&mut suffix, &rules[*index]);
    }
    suffix
}

fn read_all(path: &Path, file: &mut File) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!(
            "failed to seek instance .gitignore at {}: {error}",
            path.display()
        )
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        if is_lock_contention_error(&error) {
            format!(
                "instance .gitignore lock contention while reading at {}: {error}",
                path.display()
            )
        } else {
            format!(
                "failed to read instance .gitignore at {}: {error}",
                path.display()
            )
        }
    })?;
    Ok(bytes)
}

fn acquire_nonblocking_lock(path: &Path, file: &File) -> Result<(), String> {
    file.try_lock().map_err(|error| {
        let error: io::Error = error.into();
        if is_lock_contention_error(&error) {
            format!("instance .gitignore lock contention at {}", path.display())
        } else {
            format!(
                "failed to lock instance .gitignore at {}: {error}",
                path.display()
            )
        }
    })
}

fn is_lock_contention_error(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
        error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn validate_regular_metadata(
    path: &Path,
    metadata: &Metadata,
    context: &str,
) -> Result<(), String> {
    if metadata.is_file() && !is_link_or_reparse(metadata) {
        Ok(())
    } else {
        Err(format!(
            "instance .gitignore {context} at {} is not a regular, non-reparse file",
            path.display()
        ))
    }
}

fn validate_opened_regular_file(path: &Path, file: &File, context: &str) -> Result<(), String> {
    let opened_metadata = file.metadata().map_err(|error| {
        format!(
            "failed to inspect opened instance .gitignore at {}: {error}",
            path.display()
        )
    })?;
    validate_regular_metadata(path, &opened_metadata, context)?;

    let leaf_metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "instance .gitignore target changed during validation at {}: {error}",
            path.display()
        )
    })?;
    validate_regular_metadata(path, &leaf_metadata, "revalidated named target")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened_metadata.dev() != leaf_metadata.dev()
            || opened_metadata.ino() != leaf_metadata.ino()
        {
            return Err(format!(
                "instance .gitignore target changed during validation at {}",
                path.display()
            ));
        }
    }

    Ok(())
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn apply_safe_open_flags(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = options;
    }
}

fn open_new_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

fn open_read_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

fn open_append_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).append(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::process::{Command, Output};
    use std::time::{Duration, Instant};

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
        let escaped =
            escape_gitignore_path_segment(agent_local_dir).expect("escape agent local dir");
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
        let escaped =
            escape_gitignore_path_segment(agent_local_dir).expect("escape agent local dir");
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

    #[test]
    fn fresh_file_matches_the_registry_and_dynamic_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure fresh file");

        let actual = std::fs::read(temp.path().join(".gitignore")).expect("read .gitignore");
        assert_eq!(actual, expected_fresh_bytes(TEST_AGENT_LOCAL_DIR));
        assert!(actual.ends_with(b"\n"));

        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        assert_eq!(rules.len(), 2 + ignore_rows().len());
        let lines: Vec<&[u8]> = actual[..actual.len() - 1]
            .split(|byte| *byte == b'\n')
            .collect();
        assert_eq!(lines.len(), rules.len() * 2);
        assert_eq!(
            rules
                .iter()
                .map(|rule| rule.pattern.as_str())
                .collect::<HashSet<_>>()
                .len(),
            rules.len()
        );

        assert_eq!(
            rules
                .iter()
                .filter(|rule| rule.pattern.contains(TEST_AGENT_LOCAL_DIR))
                .count(),
            1,
            "the running agent's local directory name belongs to the first rule only"
        );
        assert!(!actual
            .windows(b"<agent-local-dir>".len())
            .any(|window| window == b"<agent-local-dir>"));

        for artifact in ignore_rows() {
            let pattern = render(artifact).pattern;
            if matches!(artifact.kind, ArtifactKind::GlobAnyDepth) {
                assert!(
                    !pattern.starts_with('/'),
                    "{pattern} is depth-independent by design and must not be anchored"
                );
            } else {
                assert!(
                    pattern.starts_with('/'),
                    "{pattern} must be anchored to the instance root"
                );
            }
        }
        assert!(lines.iter().all(|line| !line.starts_with(b"!")));
        assert!(lines.iter().all(|line| *line != b"/*"));
    }

    #[test]
    fn legacy_fourteen_rule_file_gains_exactly_the_new_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let legacy = legacy_complete_bytes(TEST_AGENT_LOCAL_DIR);
        std::fs::write(&path, &legacy).expect("seed legacy complete file");

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
            .expect("repair legacy file");
        let repaired = std::fs::read(&path).expect("read repaired legacy file");
        assert!(
            repaired.starts_with(&legacy),
            "reconciliation must never rewrite what the file already had"
        );

        let mut expected_appended = Vec::new();
        for artifact in ignore_rows() {
            let rendered = render(artifact);
            if HISTORICAL_FIXED_RULES.contains(&rendered.pattern.as_str()) {
                continue;
            }
            push_pair(&mut expected_appended, rendered.comment, &rendered.pattern);
        }
        assert!(
            !expected_appended.is_empty(),
            "this change adds rules, so the appended block cannot be empty"
        );
        assert_eq!(&repaired[legacy.len()..], expected_appended.as_slice());

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("second ensure");
        assert_eq!(
            std::fs::read(&path).expect("read after second ensure"),
            repaired
        );
    }

    #[test]
    fn read_only_legacy_complete_file_fails_without_modification() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let legacy = legacy_complete_bytes(TEST_AGENT_LOCAL_DIR);
        std::fs::write(&path, &legacy).expect("seed legacy complete file");

        let original_permissions = std::fs::metadata(&path).expect("metadata").permissions();
        let mut read_only = original_permissions.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&path, read_only).expect("make legacy file read-only");
        let can_bypass_read_only = OpenOptions::new().append(true).open(&path).is_ok();
        let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        let after = std::fs::read(&path).expect("read read-only legacy file");
        std::fs::set_permissions(&path, original_permissions).expect("restore permissions");

        if can_bypass_read_only {
            eprintln!(
                "skipping permission-denial assertion because this process can write a read-only file"
            );
            return;
        }
        assert!(
            result.is_err(),
            "this change makes every pre-existing complete file partial, so a read-only \
             one takes the failing branch instead of the silent-OK one"
        );
        assert_eq!(after, legacy);
    }

    #[test]
    fn root_agent_track_row_matches_the_root_agent_dir_constant() {
        let row = track_rows()
            .into_iter()
            .find(|artifact| artifact.name == super::super::ROOT_AGENT_DIR_NAME)
            .expect("root-agent Track row");
        assert!(matches!(row.kind, ArtifactKind::Dir));
    }

    #[test]
    fn track_rows_are_exactly_the_declared_track_set() {
        let mut names: Vec<&str> = track_rows()
            .into_iter()
            .map(|artifact| artifact.name)
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "Context.AgentsCommander.md",
                "Context.AgentsCommander.md.retired-*.bak",
                "Context.root-agent.md",
                "ac-root-agent",
                "agency-agents_templates",
                "agent-templates",
                "coding-agents",
            ],
            "the tracked set is a product decision; a row leaving or joining it changes \
             what the generated file ignores and has to be argued, not edited"
        );
    }

    #[test]
    fn injected_name_with_line_break_is_rejected_without_creating_a_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = ensure_instance_gitignore_at(temp.path(), ".valid\n/app.log");
        assert!(result.is_err());
        assert!(!temp.path().join(".gitignore").exists());
    }

    #[test]
    fn partial_file_preserves_prefix_and_appends_only_missing_rules() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        let seed = format!(
            "# user comment\r\n/custom/**\r\n{}\r\n{}\r\n{}\r\n# /daemon.pid\r\n!/sessions.json\r\n{}\r\n!/update-check.json\r\n/web-token.txt # note\r\nlast-user-rule",
            rules[0].pattern, rules[3].pattern, rules[3].pattern, rules[8].pattern
        )
        .into_bytes();
        std::fs::write(&path, &seed).expect("seed partial file");

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
            .expect("repair partial file");
        let actual = std::fs::read(&path).expect("read repaired file");
        assert!(actual.starts_with(&seed));

        let mut expected = seed.clone();
        expected.push(b'\n');
        for (index, rule) in rules.iter().enumerate() {
            if !matches!(index, 0 | 3 | 8) {
                push_pair(&mut expected, rule.comment, &rule.pattern);
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn complete_file_is_byte_stable_across_repeated_ensure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        std::fs::write(&path, b"# retained user rule\n/custom\n").expect("seed file");

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("first ensure");
        let after_first = std::fs::read(&path).expect("read first result");
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("second ensure");
        let after_second = std::fs::read(&path).expect("read second result");

        assert_eq!(after_second, after_first);
    }

    #[test]
    fn byte_scan_preserves_invalid_utf8_and_recognizes_crlf_rules() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        let mut seed = b"\xff\xfe user bytes\r\n".to_vec();
        for rule in &rules {
            seed.extend_from_slice(rule.pattern.as_bytes());
            seed.extend_from_slice(b"\r\n");
        }
        std::fs::write(&path, &seed).expect("seed invalid UTF-8 file");

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure CRLF file");
        assert_eq!(std::fs::read(path).expect("read file"), seed);
    }

    #[test]
    fn directory_target_is_rejected_and_untouched() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join(".gitignore");
        std::fs::create_dir(&target).expect("create directory target");
        let sentinel = target.join("sentinel.txt");
        std::fs::write(&sentinel, b"untouched").expect("write sentinel");

        let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        assert!(result.is_err());
        assert!(target.is_dir());
        assert_eq!(
            std::fs::read(sentinel).expect("read sentinel"),
            b"untouched"
        );
    }

    #[test]
    fn symlink_target_is_rejected_without_touching_referent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join("config");
        std::fs::create_dir(&config_dir).expect("create config dir");
        let referent = temp.path().join("external-sentinel.txt");
        std::fs::write(&referent, b"external sentinel").expect("write referent");
        let link = config_dir.join(".gitignore");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&referent, &link).expect("create symlink");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&referent, &link).is_err() {
            return;
        }
        #[cfg(not(any(unix, windows)))]
        return;

        let result = ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR);
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&referent).expect("read referent"),
            b"external sentinel"
        );
    }

    #[test]
    fn read_only_complete_file_needs_no_write_but_partial_file_fails_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
            .expect("create complete file");
        let complete = std::fs::read(&path).expect("read complete file");

        let original_permissions = std::fs::metadata(&path)
            .expect("complete metadata")
            .permissions();
        let mut read_only_permissions = original_permissions.clone();
        read_only_permissions.set_readonly(true);
        std::fs::set_permissions(&path, read_only_permissions)
            .expect("make complete file read-only");
        let complete_result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        let complete_after = std::fs::read(&path).expect("read complete read-only file");
        std::fs::set_permissions(&path, original_permissions)
            .expect("restore complete permissions");
        assert!(complete_result.is_ok());
        assert_eq!(complete_after, complete);

        let partial = b"# partial user file\n".to_vec();
        std::fs::write(&path, &partial).expect("seed partial file");
        let partial_permissions = std::fs::metadata(&path)
            .expect("partial metadata")
            .permissions();
        let mut partial_read_only = partial_permissions.clone();
        partial_read_only.set_readonly(true);
        std::fs::set_permissions(&path, partial_read_only).expect("make partial file read-only");
        let can_bypass_read_only = OpenOptions::new().append(true).open(&path).is_ok();
        let partial_result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        let partial_after = std::fs::read(&path).expect("read partial read-only file");
        std::fs::set_permissions(&path, partial_permissions).expect("restore partial permissions");

        if can_bypass_read_only {
            eprintln!(
                "skipping permission-denial assertion because this process can write a read-only file"
            );
            return;
        }
        assert!(partial_result.is_err());
        assert_eq!(partial_after, partial);
    }

    #[test]
    fn locked_partial_file_fails_fast_and_remains_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let partial = b"# partial\n".to_vec();
        std::fs::write(&path, &partial).expect("seed partial file");
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open lock holder");
        held.try_lock().expect("hold target lock");

        let started = Instant::now();
        let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        let elapsed = started.elapsed();
        assert!(
            result
                .as_ref()
                .is_err_and(|error| error.contains("lock contention")),
            "unexpected lock result: {result:?}"
        );
        assert!(elapsed < Duration::from_secs(5));

        drop(held);
        assert_eq!(
            std::fs::read(&path).expect("read file after releasing lock"),
            partial
        );
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
            .expect("repair after releasing lock");
        let repaired = std::fs::read(path).expect("read repaired file");
        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        assert!(missing_rule_indexes(&repaired, &rules).is_empty());
    }

    #[test]
    fn git_fixture_ignores_exactly_required_paths_without_untracking() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);

        let parent_gitignore = b"/.parent-ignore-sentinel\n";
        let info_exclude = b"/.exclude-sentinel\n";
        std::fs::write(repo.join(".gitignore"), parent_gitignore).expect("seed parent .gitignore");
        std::fs::write(repo.join(".git/info/exclude"), info_exclude).expect("seed info exclude");

        let config_dir = repo.join("instance");
        let required_paths = [
            "ac-root-agent/.agentscommander_amp-office/config.json",
            "ac-root-agent/config.json",
            ".settings.json.12345.tmp",
            // Depth independence of the atomic-write glob, inside a directory
            // this policy deliberately tracks: exactly the leftover class an
            // anchored rule would leave visible in `git status`.
            "coding-agents/.agents.json.4242.0.tmp",
            ".agentscommander-context-templates.json",
            ".agentscommander-injected-messages.json",
            ".api-clients-1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed.tmp",
            "activity.jsonl",
            "activity.jsonl.1",
            // The four agency template cache siblings, one sample per shape the
            // writers can produce. The three staging shapes are whole directory
            // trees and their rule carries no trailing slash, so the nested
            // samples are what prove a matched directory still ignores its
            // contents.
            "agency-agents_templates.lock",
            "agency-agents_templates.next-1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed/engineering/role.md",
            "agency-agents_templates.download-2c8e7dda-ccae-5c3e-ac6e-bc9eacce5df1/.git-clone-marker",
            "agency-agents_templates.prev-3d9f8eeb-ddbf-6d4f-bd7f-cdafbddf6ea2/design/role.md",
            "api-audit.log",
            "api-audit.log.1",
            "api-clients.json",
            "api-clients.lock",
            "api-message-bus.sqlite3",
            "api-message-bus.sqlite3-shm",
            "api-message-bus.sqlite3-wal",
            // The sample that makes the glob's reason for existing testable:
            // rollback-journal mode produces this sidecar and three literals
            // would not have covered it.
            "api-message-bus.sqlite3-journal",
            "app-outbox-path.txt",
            "app.log",
            // Both ends of the rotation range (`APP_LOG_KEEP = 5`), so the
            // sample set proves the glob spans every generation rather than only
            // its first. `app.log.1` used to be a control asserting it must NOT
            // be ignored; covering it is a declared reversal of that #1164
            // narrowness control, because it is a live rotated generation.
            "app.log.1",
            "app.log.5",
            "codex-home/agent-1/config.toml",
            // The nested sample proves the Dir row covers the `results/` subtree
            // in one rule.
            "coding-agent-requests/req-1.json",
            "coding-agent-requests/results/res-1.json",
            "context-cache/ac-context-1.md",
            "coordinator_clocks.json",
            "coordinator_clocks.json.4242.7.tmp",
            "daemon.pid",
            "debug-logs.txt",
            "diag-raw.log",
            "diag-sent.log",
            "git-guard/git.cmd",
            "injected-messages.default.toml",
            "injected-messages.toml",
            "injected-messages.toml.bak-20260801T221533Z",
            "instances/0f0e/instance.json",
            "logs/harness.log",
            "master-token.txt",
            "orphaned-sessions.archive.json",
            // The `ORPHAN_ARCHIVE_KEEP` edge.
            "orphaned-sessions.archive.json.3",
            "project-refresh-requests/req-1.json",
            "pty-input-locks/operation-1.lock",
            "session-requests/create-1.json",
            "sessions.json",
            "settings.json",
            "settings.json.lock",
            // #1737: the operator-owned overlay and the two managed context
            // template overrides. The `settings.json` row is an exact-name rule and
            // does not reach `settings.local.json`, which is why the row exists.
            "settings.local.json",
            "Context.AgentsCommander.local.md",
            "Context.root-agent.local.md",
            "settings.pre-384-v1.json",
            "settings.pre-999-v9.json",
            "telegram-bridge.log",
            "ui-automation/session.json",
            "update-check.json",
            "update-check.json.tmp",
            "web-token.txt",
        ];
        for relative in required_paths {
            let path = config_dir.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, b"fixture").expect("write required fixture");
        }

        // `app.log` was already covered before this change; the database is a
        // newly covered artifact, which is the population a new rule actually
        // affects. Neither may be untracked by adding a rule.
        for tracked in ["instance/app.log", "instance/api-message-bus.sqlite3"] {
            assert_git_success(repo, &["add", "--", tracked]);
            assert_git_success(repo, &["ls-files", "--error-unmatch", "--", tracked]);
        }

        ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
            .expect("ensure fixture .gitignore");
        let generated = std::fs::read(config_dir.join(".gitignore")).expect("read generated file");

        for relative in required_paths {
            let repo_relative = format!("instance/{relative}");
            let output = git(
                repo,
                &[
                    "check-ignore",
                    "--no-index",
                    "--quiet",
                    "--",
                    &repo_relative,
                ],
            );
            assert!(
                output.status.success(),
                "required path was not ignored: {repo_relative}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let control_paths = [
            "cache/entry.bin",
            "state.sqlite",
            "ac-root-agent/unrelated/config.json",
            "injected-messages.toml.bak",
            "injected-messages.json",
            "agentscommander-injected-messages.json",
            "sub/injected-messages.toml",
            // The Track set of product decision 6: these are user-editable and
            // must stay visible to git.
            "agent-templates/default-role.md",
            // Load-bearing: this is what proves the `agency-agents_templates.*`
            // glob does not reach the suffix-less tracked directory. The literal
            // dot is the whole mechanism.
            "agency-agents_templates/engineering/role.md",
            // Load-bearing: this is what proves the `coding-agent-requests/` row
            // does not reach its byte-order neighbour.
            "coding-agents/agents.json",
            // #1737: these two also prove the `*.local.md` glob does not reach the
            // tracked base templates whose overrides it covers.
            "Context.root-agent.md",
            // The standalone global context and both retirement-backup shapes:
            // all three hold the user's own bytes.
            "Context.AgentsCommander.md",
            "Context.AgentsCommander.md.retired-20260820-101112Z.bak",
            "Context.AgentsCommander.md.retired-20260820-101112Z.3.bak",
            "ac-root-agent/CLAUDE.md",
            // The atomic-write glob stays narrow, at the root and at depth.
            "foo.tmp",
            ".foo.tmp",
            "sub/foo.tmp",
            // The rotation rows are anchored globs, not a second any-depth rule:
            // they must not reach a subdirectory.
            "sub/app.log.1",
            "sub/activity.jsonl.1",
            // The literal dot in each rotation glob is load-bearing, not
            // decorative.
            "applog.1",
        ];
        for relative in control_paths {
            let path = config_dir.join(relative);
            std::fs::create_dir_all(path.parent().expect("control parent"))
                .expect("create control parent");
            std::fs::write(path, b"control").expect("write control fixture");
            let repo_relative = format!("instance/{relative}");
            let output = git(
                repo,
                &[
                    "check-ignore",
                    "--no-index",
                    "--quiet",
                    "--",
                    &repo_relative,
                ],
            );
            assert_eq!(
                output.status.code(),
                Some(1),
                "unrequested control was ignored: {repo_relative}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // The generated file is asserted here, OUTSIDE the control loop above.
        // That loop writes `b"control"` into every path it checks, so a
        // `.gitignore` control would have overwritten the ruleset and left every
        // later control passing against an empty file. The byte comparison is
        // the assertion that would have caught that class.
        assert_git_ignore_status(repo, "instance/.gitignore", 1);
        assert_eq!(
            std::fs::read(config_dir.join(".gitignore")).expect("re-read generated file"),
            generated,
            "the fixture's own writes must not have touched the generated file"
        );

        for tracked in ["instance/app.log", "instance/api-message-bus.sqlite3"] {
            assert_git_success(repo, &["ls-files", "--error-unmatch", "--", tracked]);
        }
        assert_eq!(
            std::fs::read(repo.join(".gitignore")).expect("read parent .gitignore"),
            parent_gitignore
        );
        assert_eq!(
            std::fs::read(repo.join(".git/info/exclude")).expect("read info exclude"),
            info_exclude
        );
    }

    #[test]
    fn dir_rows_require_a_real_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);
        let config_dir = repo.join("instance");
        std::fs::create_dir(&config_dir).expect("create config dir");
        ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
            .expect("ensure dir-semantics fixture");

        let dir_rows: Vec<&str> = ignore_rows()
            .into_iter()
            .filter(|artifact| matches!(artifact.kind, ArtifactKind::Dir))
            .map(|artifact| artifact.name)
            .collect();
        assert!(
            !dir_rows.is_empty(),
            "the table has Dir rows to prove this on"
        );

        for name in dir_rows {
            // A plain file bearing the row's name is NOT ignored: that is the
            // whole reason `Dir` renders a trailing slash.
            let plain = config_dir.join(name);
            std::fs::write(&plain, b"fixture").expect("write plain-file sample");
            assert_git_ignore_status(repo, &format!("instance/{name}"), 1);
            std::fs::remove_file(&plain).expect("remove plain-file sample");

            // A file under a real directory of that name IS ignored.
            let nested = config_dir.join(name).join("nested-sample.txt");
            std::fs::create_dir_all(nested.parent().expect("nested parent"))
                .expect("create real directory");
            std::fs::write(&nested, b"fixture").expect("write nested sample");
            assert_git_ignore_status(repo, &format!("instance/{name}/nested-sample.txt"), 0);
        }
    }

    #[test]
    fn literal_gitignore_segment_encoding_is_canonical() {
        for (input, expected) in [("\\", "\\\\"), ("*", "\\*"), ("?", "\\?"), ("[", "\\[")] {
            assert_eq!(
                escape_gitignore_path_segment(input).expect("encode metacharacter"),
                expected
            );
        }
        assert_eq!(
            escape_gitignore_path_segment(r"\*?[]-^#! spaced._(), café 東京")
                .expect("encode combined segment"),
            r"\\\*\?\[]-^#! spaced._(), café 東京"
        );

        for invalid in [
            ".agents\rinjected",
            ".agents\ninjected",
            ".agents/injected",
            ".agents\0injected",
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let result = ensure_instance_gitignore_at(temp.path(), invalid);
            assert!(result.is_err(), "invalid segment was accepted: {invalid:?}");
            assert!(
                !temp.path().join(".gitignore").exists(),
                "invalid segment touched .gitignore: {invalid:?}"
            );
        }

        let agent_local_dir = r".agents\*?[]-^#! café";
        let escaped_agent_local_dir =
            escape_gitignore_path_segment(agent_local_dir).expect("encode agent local dir");
        let rules = required_rules(agent_local_dir).expect("construct canonical rules");
        assert_eq!(rules.len(), 2 + ignore_rows().len());
        assert_eq!(
            rules[0].pattern,
            format!(
                "/{}/{escaped_agent_local_dir}/config.json",
                super::super::ROOT_AGENT_DIR_NAME
            )
        );
        assert_eq!(
            rules[1].pattern,
            format!("/{}/config.json", super::super::ROOT_AGENT_DIR_NAME)
        );
        assert_eq!(
            rules
                .iter()
                .filter(|rule| rule.pattern.contains(&escaped_agent_local_dir))
                .count(),
            1
        );
        let raw_companion = format!(
            "/{}/{agent_local_dir}/config.json",
            super::super::ROOT_AGENT_DIR_NAME
        );
        assert!(!rules.iter().any(|rule| rule.pattern == raw_companion));
    }

    #[test]
    fn git_fixture_treats_bracketed_agent_name_as_literal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);
        let config_dir = repo.join("instance");
        std::fs::create_dir(&config_dir).expect("create config dir");

        let agent_local_dir = ".agents[1]";
        ensure_instance_gitignore_at(&config_dir, agent_local_dir)
            .expect("ensure bracketed fixture");
        let rules = required_rules(agent_local_dir).expect("canonical bracketed rules");
        assert_eq!(rules[0].pattern, r"/ac-root-agent/.agents\[1]/config.json");
        let generated = std::fs::read(config_dir.join(".gitignore")).expect("read .gitignore");
        assert_eq!(generated, fresh_file_bytes(&rules));
        assert_eq!(
            generated
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            rules.len() * 2
        );

        let literal = "instance/ac-root-agent/.agents[1]/config.json";
        let sibling = "instance/ac-root-agent/.agents1/config.json";
        for relative in [literal, sibling] {
            let path = repo.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, b"fixture").expect("write fixture");
        }
        assert_git_ignore_status(repo, literal, 0);
        assert_git_ignore_status(repo, sibling, 1);
    }

    #[cfg(unix)]
    #[test]
    fn git_fixture_treats_unix_metacharacter_agent_names_as_literal() {
        let cases = [
            (".agents*literal", ".agentsZZliteral"),
            (".agents?literal", ".agentsXliteral"),
            (".agents\\literal", ".agentsliteral"),
        ];

        for (agent_local_dir, unintended_sibling) in cases {
            let temp = tempfile::tempdir().expect("tempdir");
            let repo = temp.path();
            assert_git_success(repo, &["init", "--quiet"]);
            let config_dir = repo.join("instance");
            std::fs::create_dir(&config_dir).expect("create config dir");

            ensure_instance_gitignore_at(&config_dir, agent_local_dir)
                .expect("ensure metacharacter fixture");
            let rules = required_rules(agent_local_dir).expect("canonical metacharacter rules");
            let generated = std::fs::read(config_dir.join(".gitignore")).expect("read .gitignore");
            assert_eq!(generated, fresh_file_bytes(&rules));
            assert_eq!(
                generated
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                    .count(),
                rules.len() * 2
            );
            let raw_companion = format!(
                "/{}/{agent_local_dir}/config.json",
                super::super::ROOT_AGENT_DIR_NAME
            );
            assert!(!rules.iter().any(|rule| rule.pattern == raw_companion));

            let literal = format!(
                "instance/{}/{agent_local_dir}/config.json",
                super::super::ROOT_AGENT_DIR_NAME
            );
            let sibling = format!(
                "instance/{}/{unintended_sibling}/config.json",
                super::super::ROOT_AGENT_DIR_NAME
            );
            for relative in [&literal, &sibling] {
                let path = repo.join(relative);
                std::fs::create_dir_all(path.parent().expect("fixture parent"))
                    .expect("create fixture parent");
                std::fs::write(path, b"fixture").expect("write fixture");
            }
            assert_git_ignore_status(repo, &literal, 0);
            assert_git_ignore_status(repo, &sibling, 1);
        }
    }

    #[test]
    fn escaped_canonical_line_controls_detection_and_repair() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let agent_local_dir = ".agents[1]";
        let rules = required_rules(agent_local_dir).expect("canonical rules");
        let canonical_first = &rules[0].pattern;
        let raw_first = format!(
            "/{}/{agent_local_dir}/config.json",
            super::super::ROOT_AGENT_DIR_NAME
        );

        assert!(!contains_exact_line(
            format!("{raw_first}\n").as_bytes(),
            canonical_first.as_bytes()
        ));
        assert!(contains_exact_line(
            format!("{canonical_first}\n").as_bytes(),
            canonical_first.as_bytes()
        ));

        let mut seed = Vec::new();
        seed.extend_from_slice(raw_first.as_bytes());
        seed.push(b'\n');
        for rule in &rules[1..] {
            seed.extend_from_slice(rule.pattern.as_bytes());
            seed.push(b'\n');
        }
        assert_eq!(missing_rule_indexes(&seed, &rules), vec![0]);
        std::fs::write(&path, &seed).expect("seed raw predecessor");

        ensure_instance_gitignore_at(temp.path(), agent_local_dir).expect("repair raw predecessor");
        let repaired = std::fs::read(&path).expect("read repaired file");
        let mut expected = seed.clone();
        push_pair(&mut expected, rules[0].comment, canonical_first);
        assert_eq!(repaired, expected);
        assert_eq!(missing_rule_indexes(&repaired, &rules), Vec::<usize>::new());

        ensure_instance_gitignore_at(temp.path(), agent_local_dir).expect("repeat repaired ensure");
        assert_eq!(std::fs::read(path).expect("read repeated result"), repaired);
    }

    #[test]
    fn instance_gitignore_covers_every_injected_messages_artifact() {
        use super::super::injected_messages::{
            INJECTED_MESSAGES_FILENAME, INJECTED_MESSAGES_REFERENCE_FILENAME,
            INJECTED_MESSAGES_STATE_FILENAME,
        };

        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        for expected in [
            format!("/{INJECTED_MESSAGES_FILENAME}"),
            format!("/{INJECTED_MESSAGES_REFERENCE_FILENAME}"),
            format!("/{INJECTED_MESSAGES_STATE_FILENAME}"),
            format!("/{INJECTED_MESSAGES_FILENAME}.bak-*"),
        ] {
            assert!(
                rules.iter().any(|rule| rule.pattern == expected),
                "injected-messages artifact is not covered by the instance policy: {expected}"
            );
        }
    }

    #[test]
    fn instance_gitignore_ignores_injected_messages_artifacts_narrowly() {
        use super::super::injected_messages::{
            INJECTED_MESSAGES_FILENAME, INJECTED_MESSAGES_REFERENCE_FILENAME,
            INJECTED_MESSAGES_STATE_FILENAME,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);
        let config_dir = repo.join("instance");
        std::fs::create_dir(&config_dir).expect("create config dir");

        ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
            .expect("ensure injected-messages fixture");

        let stamped_backup = format!("{INJECTED_MESSAGES_FILENAME}.bak-20260801T221533Z");
        let unstamped_backup = format!("{INJECTED_MESSAGES_FILENAME}.bak");
        let nested = format!("sub/{INJECTED_MESSAGES_FILENAME}");
        let cases = [
            (INJECTED_MESSAGES_STATE_FILENAME, 0),
            (INJECTED_MESSAGES_REFERENCE_FILENAME, 0),
            (INJECTED_MESSAGES_FILENAME, 0),
            (stamped_backup.as_str(), 0),
            (unstamped_backup.as_str(), 1),
            ("injected-messages.json", 1),
            ("agentscommander-injected-messages.json", 1),
            (nested.as_str(), 1),
        ];

        for (relative, expected_code) in cases {
            let path = config_dir.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, b"fixture").expect("write fixture");
            assert_git_ignore_status(repo, &format!("instance/{relative}"), expected_code);
        }
    }
}
