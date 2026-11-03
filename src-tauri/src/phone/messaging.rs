//! File-based inter-agent messaging primitives.
//!
//! PTY injection carries only a short notification pointing to a file in
//! `<workgroup-root>/messaging/`. The recipient reads the file via filesystem,
//! bypassing PTY truncation for arbitrarily-sized payloads.

use crate::filesystem::phone_messaging;
use chrono::{DateTime, Utc};
use std::fs::File;
use std::path::{Component, Path, PathBuf};

pub const MESSAGING_DIR_NAME: &str = "messaging";
pub const PTY_SAFE_MAX: usize = 1024;
pub const FILE_NOTIFICATION_PREFIX: &str = "Process this inter-agent message: ";
pub const FILE_NOTIFICATION_SUFFIX: &str = "";

/// Display-only sender suffix for a message the Co-managed supervisor produced
/// (#2232 phase 7). The queue file's `from` stays the exact FQN, which is what
/// authorizes; this suffix only tells the recipient the message is automatic.
/// The single source for the mailbox render site and the supervisor's
/// line-budget check.
pub const CO_MANAGED_SENDER_SUFFIX: &str = " (Co-managed)";

/// Compose the sender a Co-managed-origin wake displays. Never used as an
/// authorization input: the FQN and this string are different fields, never
/// merged.
pub fn compose_sender_for_comanaged_origin(from: &str) -> String {
    format!("{from}{CO_MANAGED_SENDER_SUFFIX}")
}
const LEGACY_FILE_NOTIFICATION_PREFIX: &str = "New message: ";
const LEGACY_FILE_NOTIFICATION_SUFFIX: &str = ". Read this file.";

const MAX_SLUG_LEN: usize = 50;
const MAX_COLLISION_SUFFIX: u32 = 99;

/// Fixed-char portion of the interactive PTY wrap template
/// `\n[Message from {from}] {body}\n\r` with empty placeholders.
/// Kept as a public constant so the CLI clamp can size overhead precisely.
pub const PTY_WRAP_FIXED: usize = "\n[Message from ] \n\r".len();

/// Single-source render of the interactive PTY wrap. Both injection sites in
/// `phone::mailbox` call this, and the contract test measures its empty
/// expansion against `PTY_WRAP_FIXED` — any edit to the literal here trips
/// the test before the clamp accounting can drift.
pub fn format_pty_wrap(from: &str, body: &str) -> String {
    format!("\n[Message from {}] {}\n\r", from, body)
}

/// Fixed-char portion of the `--get-output` response-marker framing that the
/// mailbox appends to the PTY wrap, with empty request-id placeholders. This is
/// exactly the delta between `format_pty_wrap_with_markers` and `format_pty_wrap`
/// when every placeholder is empty; the contract test locks it. It sizes only
/// the retained non-interactive marker path, which is unreachable today. The
/// live interactive wake clamp in `cli::send` stays plain-wrap budgeting and does
/// NOT add this overhead when `--get-output` is set; only the inert
/// `notification_pty_overhead(_, true)` branch (reserved for a future
/// non-interactive consumer) adds it plus two request-id lengths.
pub const PTY_RESPONSE_MARKER_FIXED: usize =
    "\n(Reply between markers: %%AC_RESPONSE::::START%% ... %%AC_RESPONSE::::END%%)".len();

/// Single-source render of the PTY wrap used for `--get-output` messages, which
/// carries response markers so a non-interactive session can delimit its reply.
/// The mailbox only injects these markers on a non-interactive session
/// (`use_markers = get_output && !interactive`), which is unreachable today, so
/// this is retained future-proofing. Keeping the literal here (mirroring
/// `format_pty_wrap`) lets `PTY_RESPONSE_MARKER_FIXED` size the marker overhead
/// for that future non-interactive path without the accounting drifting; today's
/// live interactive wake clamp in `cli::send` budgets the plain wrap only and
/// does not size marker overhead.
pub fn format_pty_wrap_with_markers(from: &str, body: &str, request_id: &str) -> String {
    format!(
        "\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... \
        %%AC_RESPONSE::{}::END%%)\n\r",
        from, body, request_id, request_id
    )
}

pub fn format_file_notification(abs_path: &str) -> String {
    format!(
        "{}{}{}",
        FILE_NOTIFICATION_PREFIX, abs_path, FILE_NOTIFICATION_SUFFIX
    )
}

pub fn parse_file_notification(body: &str) -> Option<&str> {
    body.strip_prefix(FILE_NOTIFICATION_PREFIX)
        .and_then(|rest| rest.strip_suffix(FILE_NOTIFICATION_SUFFIX))
        .filter(|path| !path.is_empty())
        .or_else(|| {
            body.strip_prefix(LEGACY_FILE_NOTIFICATION_PREFIX)
                .and_then(|rest| rest.strip_suffix(LEGACY_FILE_NOTIFICATION_SUFFIX))
                .filter(|path| !path.is_empty())
        })
}

pub fn notification_filename(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|filename| !filename.is_empty())
}

pub fn validate_root_notification_filename(filename: &str) -> Result<(), MessagingError> {
    validate_filename_only(filename)?;

    let stem = filename
        .strip_suffix(".md")
        .ok_or_else(|| MessagingError::InvalidShape(filename.to_string()))?;
    let stem = match stem.rfind('.') {
        Some(dot_pos) => {
            let suffix = &stem[dot_pos + 1..];
            let is_collision_suffix = !suffix.is_empty()
                && suffix.len() <= 2
                && suffix.chars().all(|c| c.is_ascii_digit());
            if is_collision_suffix {
                let n: u32 = suffix
                    .parse()
                    .map_err(|_| MessagingError::InvalidShape(filename.to_string()))?;
                if n == 0 {
                    return Err(MessagingError::InvalidShape(filename.to_string()));
                }
                &stem[..dot_pos]
            } else {
                stem
            }
        }
        None => stem,
    };
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.get(2) == Some(&crate::config::root_agent::ROOT_AGENT_SHORT_NAME)
        && parts.get(3) == Some(&"to")
    {
        Ok(())
    } else {
        Err(MessagingError::InvalidShape(filename.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error("no room ancestor found for '{0}'")]
    NoWorkgroup(String),
    #[error("slug is empty after sanitization")]
    EmptySlug,
    #[error("filename '{0}' contains path separators or traversal")]
    InvalidFilename(String),
    #[error(
        "filename '{0}' does not match the required shape: \
         YYYYMMDD-HHMMSS-<from>-to-<to>-<slug>[.N].md; \
         if the sender is Root: require root-to immediately after the timestamp, e.g. \
         20261022-092418-root-to-room21-ac-tech-lead-v5-fixer-installation-confirmed.md"
    )]
    InvalidShape(String),
    #[error("message file not found: {0}")]
    FileNotFound(String),
    #[error("target is not a regular file: {0}")]
    NotAFile(String),
    #[error("collision suffix exhausted (99 retries) for {0}")]
    CollisionExhausted(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolve the workgroup root by walking up from `agent_root`.
///
/// Pure path operation — no filesystem touch, no canonicalization. Tests can
/// use synthetic paths.
pub fn workgroup_root(agent_root: &Path) -> Result<PathBuf, MessagingError> {
    for ancestor in agent_root.ancestors() {
        if let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) {
            if is_wg_dir(name) {
                return Ok(ancestor.to_path_buf());
            }
        }
    }
    Err(MessagingError::NoWorkgroup(
        agent_root.display().to_string(),
    ))
}

/// Messaging directory for a workgroup root. Creates the directory if missing.
pub fn messaging_dir(wg_root: &Path) -> Result<PathBuf, MessagingError> {
    let dir = wg_root.join(MESSAGING_DIR_NAME);
    phone_messaging::ensure_message_directory(&dir)?;
    Ok(dir)
}

pub fn root_messaging_dir(root_agent_dir: &Path) -> Result<PathBuf, MessagingError> {
    let dir = root_agent_dir.join(MESSAGING_DIR_NAME);
    phone_messaging::ensure_message_directory(&dir)?;
    Ok(dir)
}

/// Prove `wg_root` sits under its project's authoritative Project AC Root
/// before anything is written into its `messaging/` directory.
///
/// The CLI always did this; an in-process caller does not inherit a private
/// helper, so the check lives here (#2232 phase 7). `phone::messaging` is
/// already inside the 88-member SCC, so the call adds an SCC-to-leaf arc to
/// `config::ac_root` and no new member.
pub(crate) fn ensure_workgroup_root_is_authoritative(wg_root: &Path) -> Result<(), String> {
    let ac_root = wg_root.parent().ok_or_else(|| {
        format!(
            "room root '{}' has no parent Project AC Root directory",
            wg_root.display()
        )
    })?;
    crate::config::ac_root::ensure_authoritative_ac_root(ac_root)
}

/// Convert a full agent name (e.g. `"wg-7-dev-team/architect"`) to short form
/// (e.g. `"wg7-architect"`). See plan §2 for the rule.
pub fn agent_short_name(full_name: &str) -> String {
    match full_name.split_once('/') {
        Some((prefix, suffix)) => {
            let short_prefix = parse_wg_prefix(prefix).unwrap_or_else(|| sanitize(prefix));
            let short_suffix = sanitize(suffix);
            format!("{}-{}", short_prefix, short_suffix)
        }
        None => sanitize(full_name),
    }
}

/// Sanitize a slug: lowercase ASCII, kebab-case, ≤ `MAX_SLUG_LEN` chars.
/// Returns `EmptySlug` if the result is empty.
pub fn sanitize_slug(slug: &str) -> Result<String, MessagingError> {
    let s = sanitize(slug);
    let truncated: String = s.chars().take(MAX_SLUG_LEN).collect();
    let trimmed = truncated.trim_matches('-').to_string();
    if trimmed.is_empty() {
        Err(MessagingError::EmptySlug)
    } else {
        Ok(trimmed)
    }
}

/// Build the target filename (not a path). Caller supplies a sanitized slug.
pub fn build_filename(ts: DateTime<Utc>, from_short: &str, to_short: &str, slug: &str) -> String {
    format!(
        "{}-{}-to-{}-{}.md",
        ts.format("%Y%m%d-%H%M%S"),
        from_short,
        to_short,
        slug
    )
}

/// Validate the **shape** of a filename against the canonical pattern
/// `YYYYMMDD-HHMMSS-{from_short}-to-{to_short}-{slug}[.N].md`.
///
/// Hard-enforced per plan §13.2 P0-1 to prevent senders from reusing generic
/// names like `reply.md` and destroying the append-only audit convention.
pub fn validate_filename_shape(name: &str) -> Result<(), MessagingError> {
    let invalid = || MessagingError::InvalidShape(name.to_string());

    let stem = name.strip_suffix(".md").ok_or_else(invalid)?;

    // Strip optional ".N" collision suffix (1-2 digits, N >= 1).
    let stem = match stem.rfind('.') {
        Some(dot_pos) => {
            let suffix = &stem[dot_pos + 1..];
            let is_digit_suffix = !suffix.is_empty()
                && suffix.len() <= 2
                && suffix.chars().all(|c| c.is_ascii_digit());
            if is_digit_suffix {
                let n: u32 = suffix.parse().map_err(|_| invalid())?;
                if n == 0 {
                    return Err(invalid());
                }
                &stem[..dot_pos]
            } else {
                stem
            }
        }
        None => stem,
    };

    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 6 {
        return Err(invalid());
    }
    if parts[0].len() != 8 || !parts[0].chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    if parts[1].len() != 6 || !parts[1].chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }

    // Remaining segments must be non-empty and `[a-z0-9]+`.
    for p in &parts[2..] {
        if p.is_empty() {
            return Err(invalid());
        }
        if !p
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(invalid());
        }
    }

    // Locate the "to" literal. Must be at index >= 3 (from_short has >=1 part)
    // and <= parts.len()-3 (to_short and slug each have >=1 part). parts.len()
    // is already >= 6 here (checked above), so only the search-window bound
    // below can reject.
    let search_end = parts.len() - 2;
    if 3 >= search_end {
        return Err(invalid());
    }
    // Use `rposition` so filenames whose `from_short` contains the literal
    // segment `to` (e.g. `"wg7-to-lead"`) parse with the rightmost `to` as the
    // separator — matching the canonical `build_filename` output structure.
    // Shape validity is unaffected either way; this keeps round-trip parsing
    // unambiguous for any future caller that wants to split the filename back
    // into from/to/slug.
    let to_idx = parts[3..search_end]
        .iter()
        .rposition(|&p| p == "to")
        .map(|i| i + 3)
        .ok_or_else(invalid)?;

    if to_idx < 3 || parts.len() - to_idx - 1 < 2 {
        return Err(invalid());
    }

    Ok(())
}

/// Publish a snapshot under its filename date without modifying the source or an existing copy.
pub(crate) fn archive_message_snapshot(
    ac_root: &Path,
    canonical_source: &Path,
    basename: &str,
    snapshot: &[u8],
) -> Result<PathBuf, String> {
    archive_message_snapshot_with(ac_root, canonical_source, basename, snapshot, |_, _| Ok(()))
}

fn archive_message_snapshot_with(
    ac_root: &Path,
    canonical_source: &Path,
    basename: &str,
    snapshot: &[u8],
    mut before: impl FnMut(&str, &Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    validate_filename_only(basename).map_err(|e| e.to_string())?;
    let root = phone_messaging::canonical_message_path(ac_root).map_err(|e| e.to_string())?;
    let day = &basename[..8];
    let mut dir = root.clone();
    for component in ["message-archive", day] {
        dir.push(component);
        phone_messaging::ensure_message_directory(&dir).map_err(|e| e.to_string())?;
        dir = phone_messaging::canonical_message_path(&dir).map_err(|e| e.to_string())?;
        if !dir.starts_with(&root) {
            return Err("archive directory escapes Project AC Root".into());
        }
    }
    let final_path = dir.join(basename);
    fn compare(path: &Path, snapshot: &[u8]) -> Result<(), String> {
        if !phone_messaging::message_symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .is_file()
        {
            return Err("existing archive is not a regular file".into());
        }
        if phone_messaging::read_message_snapshot(path).map_err(|e| e.to_string())? != snapshot {
            return Err("archive conflict: archive filename already has different bytes".into());
        }
        Ok(())
    }
    let mut publish = || -> Result<PathBuf, String> {
        match phone_messaging::message_symlink_metadata(&final_path) {
            Ok(_) => return compare(&final_path, snapshot).map(|()| final_path.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        let mut owned = None;
        for _ in 0..3 {
            let temp = dir.join(format!(".archive-{}.tmp", uuid::Uuid::new_v4()));
            match phone_messaging::create_message_stage(&temp) {
                Ok(file) => {
                    owned = Some((temp, file));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let (temp, mut file) = owned.ok_or("archive temporary name collisions exhausted")?;
        let written = (|| -> Result<(), String> {
            before("write", &final_path).map_err(|e| e.to_string())?;
            phone_messaging::write_message_stage(&mut file, snapshot).map_err(|e| e.to_string())?;
            before("flush", &final_path).map_err(|e| e.to_string())?;
            phone_messaging::flush_message_stage(&mut file).map_err(|e| e.to_string())
        })();
        drop(file);
        let result = written.and_then(|()| {
            before("link", &final_path).map_err(|e| e.to_string())?;
            match phone_messaging::link_message_archive(&temp, &final_path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    compare(&final_path, snapshot)
                }
                Err(e) => Err(e.to_string()),
            }
        });
        let cleanup = before("cleanup", &final_path)
            .and_then(|()| phone_messaging::remove_message_stage(&temp));
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(final_path.clone()),
            (Err(e), Ok(())) => Err(e),
            (Ok(()), Err(e)) => Err(format!("copy published but temporary cleanup failed: {e}")),
            (Err(e), Err(cleanup)) => Err(format!("{e}; temporary cleanup failed: {cleanup}")),
        }
    };
    publish().map_err(|e| {
        format!(
            "{} -> {}: {e}",
            canonical_source.display(),
            final_path.display()
        )
    })
}

/// Atomically allocate a non-colliding path in `messaging_dir` starting from
/// `base_filename`. Returns the absolute path and an open file handle
/// (write-only, freshly created via `create_new`).
///
/// On collision, retries with `.1.md`, `.2.md`, … up to `MAX_COLLISION_SUFFIX`.
pub fn create_message_file(
    messaging_dir: &Path,
    base_filename: &str,
) -> Result<(PathBuf, File), MessagingError> {
    validate_filename_shape(base_filename)?;

    let stem = base_filename
        .strip_suffix(".md")
        .ok_or_else(|| MessagingError::InvalidFilename(base_filename.to_string()))?;

    for n in 0..=MAX_COLLISION_SUFFIX {
        let filename = if n == 0 {
            base_filename.to_string()
        } else {
            format!("{}.{}.md", stem, n)
        };
        let path = messaging_dir.join(&filename);
        match phone_messaging::create_message_stage(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(MessagingError::Io(e)),
        }
    }
    Err(MessagingError::CollisionExhausted(
        base_filename.to_string(),
    ))
}

/// Validate that `filename` exists inside `messaging_dir` and points to a
/// regular file. Returns the canonicalized absolute path on success.
///
/// Rejects path separators, `..`, non-`.md`, off-tree symlinks, and
/// directories. Shape is also hard-enforced.
pub fn validate_filename_only(filename: &str) -> Result<(), MessagingError> {
    let has_non_normal_component = Path::new(filename)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)));
    if filename.contains('/')
        || filename.contains('\\')
        || filename.contains(':')
        || has_non_normal_component
    {
        return Err(MessagingError::InvalidFilename(filename.to_string()));
    }
    validate_filename_shape(filename)
}

pub fn resolve_existing_message(
    messaging_dir: &Path,
    filename: &str,
) -> Result<PathBuf, MessagingError> {
    validate_filename_only(filename)?;

    let candidate = messaging_dir.join(filename);
    if !phone_messaging::message_path_exists(&candidate) {
        return Err(MessagingError::FileNotFound(filename.to_string()));
    }

    let abs = phone_messaging::canonical_message_path(&candidate)?;
    let canon_dir = phone_messaging::canonical_message_path(messaging_dir)?;

    let abs_parent = abs
        .parent()
        .ok_or_else(|| MessagingError::InvalidFilename(filename.to_string()))?;
    if abs_parent != canon_dir {
        return Err(MessagingError::InvalidFilename(filename.to_string()));
    }

    if !phone_messaging::message_metadata(&abs)?.is_file() {
        return Err(MessagingError::NotAFile(abs.display().to_string()));
    }

    Ok(abs)
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn is_wg_dir(name: &str) -> bool {
    let rest = match crate::config::entity_prefix::strip_entity_prefix(name) {
        Some(r) => r,
        None => return false,
    };
    let n_end = match rest.find('-') {
        Some(i) => i,
        None => return false,
    };
    let digits = &rest[..n_end];
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn parse_wg_prefix(prefix: &str) -> Option<String> {
    let matched = crate::config::entity_prefix::entity_prefix_of(prefix)?;
    let rest = &prefix[matched.len()..];
    let n_end = rest.find('-')?;
    let digits = &rest[..n_end];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}{}", matched.trim_end_matches('-'), digits))
}

fn sanitize(s: &str) -> String {
    let lowered: String = s
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let mut out = String::with_capacity(lowered.len());
    let mut last_dash = false;
    for c in lowered.chars() {
        if c == '-' {
            if !last_dash {
                out.push(c);
            }
            last_dash = true;
        } else {
            out.push(c);
            last_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
