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
        ArtifactKind::RootRelativeGlob => format!("/{}", artifact.name),
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
    if retire(&bytes).is_some() {
        drop(read_file);
        return rewrite_retired_file(path, rules);
    }
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

/// #2714 - the AC-written pairs of the nine rows B1b renamed, as the generator
/// wrote them before the switch. The retired settings lock keeps a row of its
/// own under a new comment, so its old pair retires too.
const RETIRED_PAIRS: [(&str, &str); 9] = [
    (
        "# AgentsCommander: remote blocking-menu download throttle stamp",
        "/blocking-menus-remote-check.json",
    ),
    (
        "# AgentsCommander: shipped blocking-menu patterns; rewritten from the binary at every \
        start",
        "/settings-blocking-menus.json",
    ),
    (
        "# AgentsCommander: operator-owned blocking-menu overlay; machine-local by design",
        "/settings-blocking-menus.local.json",
    ),
    (
        "# AgentsCommander: downloaded blocking-menu patterns; replaced by the next accepted \
        download",
        "/settings-blocking-menus.remote.json",
    ),
    (
        "# AgentsCommander: rotated previous generations of the application settings; the same \
        runtime artifact under a numeric slot",
        "/settings.backup.*.json",
    ),
    ("# AgentsCommander: application settings", "/settings.json"),
    (
        "# AgentsCommander: transient settings write lock",
        "/settings.json.lock",
    ),
    (
        "# AgentsCommander: operator-owned settings overlay; machine-local by design and never \
        written by AC",
        "/settings.local.json",
    ),
    (
        "# AgentsCommander: settings migration backups",
        "/settings.pre-*.json",
    ),
];

/// The body with every retired pair removed, or `None` when there is none to
/// remove. A body that is not UTF-8 is left to the append-only path.
fn retire(bytes: &[u8]) -> Option<Vec<u8>> {
    let content = std::str::from_utf8(bytes).ok()?;
    let (retired, changed) = super::naming_migration::retire_ignore_pairs(content, &RETIRED_PAIRS);
    changed.then(|| retired.into_bytes())
}

/// #2714 - the one non-append path: a body carrying retired pairs is rewritten
/// in place through a read-write handle, under the same lock and the same
/// validation as the append path, and gains any missing rule in the same write.
/// An append handle cannot truncate on Windows, hence the separate handle.
fn rewrite_retired_file(path: &Path, rules: &[RenderedRule]) -> Result<AttemptResult, String> {
    let mut file = match open_read_write_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AttemptResult::RetryClassification);
        }
        Err(error) => {
            return Err(format!(
                "failed to open instance .gitignore for rule retirement at {}: {error}",
                path.display()
            ));
        }
    };
    validate_opened_regular_file(path, &file, "retirement target")?;
    acquire_nonblocking_lock(path, &file)?;
    validate_opened_regular_file(path, &file, "locked retirement target")?;

    let locked_bytes = read_all(path, &mut file)?;
    validate_opened_regular_file(path, &file, "retirement target after locked read")?;
    let retired = retire(&locked_bytes);
    let kept = retired.as_deref().unwrap_or(&locked_bytes);
    let missing = missing_rule_indexes(kept, rules);
    if retired.is_none() && missing.is_empty() {
        return Ok(AttemptResult::Done);
    }
    let mut body = kept.to_vec();
    body.extend(append_buffer(kept, rules, &missing));
    let written = file
        .seek(SeekFrom::Start(0))
        .and_then(|_| file.set_len(0))
        .and_then(|()| file.write_all(&body));
    written.map_err(|error| {
        format!(
            "failed to rewrite instance .gitignore for rule retirement at {}: {error}",
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

fn open_read_write_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
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
mod tests;
