//! File-based inter-agent messaging primitives.
//!
//! PTY injection carries only a short notification pointing to a file in
//! `<workgroup-root>/messaging/`. The recipient reads the file via filesystem,
//! bypassing PTY truncation for arbitrarily-sized payloads.

use chrono::{DateTime, Utc};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub const MESSAGING_DIR_NAME: &str = "messaging";
pub const PTY_SAFE_MAX: usize = 1024;

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

#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error("no workgroup ancestor found for '{0}'")]
    NoWorkgroup(String),
    #[error("slug is empty after sanitization")]
    EmptySlug,
    #[error("filename '{0}' contains path separators or traversal")]
    InvalidFilename(String),
    #[error("filename '{0}' does not match the required shape")]
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
    Err(MessagingError::NoWorkgroup(agent_root.display().to_string()))
}

/// Messaging directory for a workgroup root. Creates the directory if missing.
pub fn messaging_dir(wg_root: &Path) -> Result<PathBuf, MessagingError> {
    let dir = wg_root.join(MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
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
        if !p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(invalid());
        }
    }

    // Locate the "to" literal. Must be at index >= 3 (from_short has >=1 part)
    // and <= parts.len()-3 (to_short and slug each have >=1 part).
    if parts.len() < 3 {
        return Err(invalid());
    }
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
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(MessagingError::Io(e)),
        }
    }
    Err(MessagingError::CollisionExhausted(base_filename.to_string()))
}

/// Validate that `filename` exists inside `messaging_dir` and points to a
/// regular file. Returns the canonicalized absolute path on success.
///
/// Rejects path separators, `..`, non-`.md`, off-tree symlinks, and
/// directories. Shape is also hard-enforced.
pub fn resolve_existing_message(
    messaging_dir: &Path,
    filename: &str,
) -> Result<PathBuf, MessagingError> {
    if filename.contains("..") {
        return Err(MessagingError::InvalidFilename(filename.to_string()));
    }

    let normalized_owned: String;
    let filename: &str = if filename.contains('/') || filename.contains('\\') {
        let as_path = Path::new(filename);
        let parent = as_path
            .parent()
            .ok_or_else(|| MessagingError::InvalidFilename(filename.to_string()))?;
        let canon_msg_dir = std::fs::canonicalize(messaging_dir)
            .map_err(|_| MessagingError::InvalidFilename(filename.to_string()))?;
        let canon_parent = std::fs::canonicalize(parent)
            .map_err(|_| MessagingError::InvalidFilename(filename.to_string()))?;
        if canon_parent != canon_msg_dir {
            return Err(MessagingError::InvalidFilename(filename.to_string()));
        }
        normalized_owned = as_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| MessagingError::InvalidFilename(filename.to_string()))?
            .to_string();
        &normalized_owned
    } else {
        filename
    };
    if !filename.ends_with(".md") {
        return Err(MessagingError::InvalidFilename(filename.to_string()));
    }
    validate_filename_shape(filename)?;

    let candidate = messaging_dir.join(filename);
    if !candidate.exists() {
        return Err(MessagingError::FileNotFound(filename.to_string()));
    }

    let abs = std::fs::canonicalize(&candidate)?;
    let canon_dir = std::fs::canonicalize(messaging_dir)?;

    let abs_parent = abs
        .parent()
        .ok_or_else(|| MessagingError::InvalidFilename(filename.to_string()))?;
    if abs_parent != canon_dir {
        return Err(MessagingError::InvalidFilename(filename.to_string()));
    }

    if !abs.metadata()?.is_file() {
        return Err(MessagingError::NotAFile(abs.display().to_string()));
    }

    Ok(abs)
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn is_wg_dir(name: &str) -> bool {
    let rest = match name.strip_prefix("wg-") {
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
    let rest = prefix.strip_prefix("wg-")?;
    let n_end = rest.find('-')?;
    let digits = &rest[..n_end];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("wg{}", digits))
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
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn unique_tmp(prefix: &str) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            Utc::now().timestamp_nanos_opt().unwrap_or(0),
            h.finish()
        ))
    }

    #[test]
    fn agent_short_name_wg() {
        assert_eq!(agent_short_name("wg-7-dev-team/architect"), "wg7-architect");
        assert_eq!(agent_short_name("wg-12-foo-bar/dev-rust"), "wg12-dev-rust");
    }

    #[test]
    fn agent_short_name_non_wg() {
        assert_eq!(agent_short_name("repos/my-project"), "repos-my-project");
        assert_eq!(agent_short_name("Agents/Shipper"), "agents-shipper");
    }

    #[test]
    fn agent_short_name_no_slash() {
        assert_eq!(agent_short_name("solo-name"), "solo-name");
    }

    #[test]
    fn sanitize_slug_typical() {
        assert_eq!(
            sanitize_slug("Messaging Redesign!").unwrap(),
            "messaging-redesign"
        );
    }

    #[test]
    fn sanitize_slug_empty_after_normalize() {
        assert!(matches!(
            sanitize_slug("   ---  "),
            Err(MessagingError::EmptySlug)
        ));
    }

    #[test]
    fn sanitize_slug_truncates_at_max() {
        let input = "a".repeat(100);
        assert_eq!(sanitize_slug(&input).unwrap(), "a".repeat(50));
    }

    #[test]
    fn workgroup_root_ok() {
        let p = Path::new("/tmp/wg-7-dev-team/__agent_architect");
        assert_eq!(
            workgroup_root(p).unwrap(),
            PathBuf::from("/tmp/wg-7-dev-team")
        );
    }

    #[test]
    fn workgroup_root_ok_windows_style() {
        let p = Path::new(r"C:\foo\wg-42-team-x\__agent_a");
        let wg = workgroup_root(p).unwrap();
        assert_eq!(
            wg.file_name().and_then(|n| n.to_str()),
            Some("wg-42-team-x")
        );
    }

    #[test]
    fn workgroup_root_missing() {
        let p = Path::new("/tmp/plain/agent");
        assert!(matches!(
            workgroup_root(p),
            Err(MessagingError::NoWorkgroup(_))
        ));
    }

    #[test]
    fn build_filename_canonical() {
        let ts = Utc.with_ymd_and_hms(2026, 4, 18, 14, 30, 52).unwrap();
        assert_eq!(
            build_filename(ts, "wg7-lead", "wg7-arch", "redesign"),
            "20260418-143052-wg7-lead-to-wg7-arch-redesign.md"
        );
    }

    #[test]
    fn validate_filename_shape_accepts_canonical() {
        assert!(validate_filename_shape(
            "20260418-143052-wg7-lead-to-wg7-arch-redesign.md"
        )
        .is_ok());
    }

    #[test]
    fn validate_filename_shape_accepts_collision_suffix() {
        assert!(validate_filename_shape(
            "20260418-143052-wg7-lead-to-wg7-arch-redesign.1.md"
        )
        .is_ok());
        assert!(validate_filename_shape(
            "20260418-143052-wg7-lead-to-wg7-arch-redesign.99.md"
        )
        .is_ok());
    }

    #[test]
    fn validate_filename_shape_rejects_variants() {
        let cases = [
            "reply.md",
            "20260418-143052-noto.md",
            "20260418-143052-from-to-to.md", // slug missing
            "20260418-143052-from-to-to-slug.txt",
            "20260418-143052-FROM-to-TO-slug.md", // uppercase
            "20260418-143052-from-to-to-slug.100.md", // 3-digit suffix
            "20260418-143052-from-to-to-slug.0.md", // zero suffix
            "2026041-143052-from-to-to-slug.md", // date too short
            "20260418-14305-from-to-to-slug.md", // time too short
        ];
        for c in cases {
            assert!(
                validate_filename_shape(c).is_err(),
                "expected rejection for {:?}",
                c
            );
        }
    }

    #[test]
    fn create_and_resolve_round_trip() {
        let tmp = std::env::temp_dir().join(format!(
            "ac-msg-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let ts = Utc.with_ymd_and_hms(2026, 4, 18, 14, 30, 52).unwrap();
        let base = build_filename(ts, "wg7-a", "wg7-b", "rt");

        let (p1, f1) = create_message_file(&tmp, &base).unwrap();
        drop(f1);
        assert_eq!(p1.file_name().and_then(|n| n.to_str()).unwrap(), base);

        let (p2, f2) = create_message_file(&tmp, &base).unwrap();
        drop(f2);
        assert_eq!(
            p2.file_name().and_then(|n| n.to_str()).unwrap(),
            "20260418-143052-wg7-a-to-wg7-b-rt.1.md"
        );

        let (p3, f3) = create_message_file(&tmp, &base).unwrap();
        drop(f3);
        assert_eq!(
            p3.file_name().and_then(|n| n.to_str()).unwrap(),
            "20260418-143052-wg7-a-to-wg7-b-rt.2.md"
        );

        let abs = resolve_existing_message(&tmp, &base).unwrap();
        assert!(abs.is_absolute());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_rejects_traversal() {
        let tmp = std::env::temp_dir().join(format!(
            "ac-msg-trav-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        for bad in [
            "../etc/passwd",
            "foo/bar.md",
            r"foo\bar.md",
            "foo.txt",
            "bare.md",
            "..",
        ] {
            assert!(
                resolve_existing_message(&tmp, bad).is_err(),
                "expected rejection for {:?}",
                bad
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_accepts_abs_path_inside_messaging_dir() {
        let tmp = unique_tmp("ac-msg-abs-ok");
        std::fs::create_dir_all(&tmp).unwrap();

        let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
        let base = build_filename(ts, "wg7-a", "wg7-b", "abs-ok");
        let (written_abs, f) = create_message_file(&tmp, &base).unwrap();
        drop(f);

        let abs_str = written_abs.to_string_lossy().to_string();
        let resolved = resolve_existing_message(&tmp, &abs_str).unwrap();
        assert_eq!(
            resolved.file_name().and_then(|n| n.to_str()).unwrap(),
            base
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_rejects_abs_path_outside_messaging_dir() {
        let tmp_msg = unique_tmp("ac-msg-abs-out");
        let tmp_other = unique_tmp("ac-msg-abs-other");
        std::fs::create_dir_all(&tmp_msg).unwrap();
        std::fs::create_dir_all(&tmp_other).unwrap();

        let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
        let base = build_filename(ts, "wg7-a", "wg7-b", "other");
        let bad_path = tmp_other.join(&base);
        std::fs::write(&bad_path, b"x").unwrap();

        let bad_abs = bad_path.to_string_lossy().to_string();
        assert!(matches!(
            resolve_existing_message(&tmp_msg, &bad_abs),
            Err(MessagingError::InvalidFilename(_))
        ));

        let _ = std::fs::remove_dir_all(&tmp_msg);
        let _ = std::fs::remove_dir_all(&tmp_other);
    }

    #[test]
    fn resolve_rejects_abs_path_with_dotdot() {
        let tmp_msg = unique_tmp("ac-msg-dd");
        std::fs::create_dir_all(&tmp_msg).unwrap();

        let sneaky = format!(
            "{}/../{}/foo.md",
            tmp_msg.display(),
            tmp_msg.file_name().and_then(|n| n.to_str()).unwrap()
        );
        assert!(matches!(
            resolve_existing_message(&tmp_msg, &sneaky),
            Err(MessagingError::InvalidFilename(_))
        ));

        let _ = std::fs::remove_dir_all(&tmp_msg);
    }

    #[test]
    fn resolve_rejects_abs_path_with_missing_parent() {
        let tmp_msg = unique_tmp("ac-msg-missing");
        std::fs::create_dir_all(&tmp_msg).unwrap();

        let bogus = std::env::temp_dir()
            .join("ac-no-such-dir-xyz-unlikely")
            .join("20260419-143052-wg7-a-to-wg7-b-nope.md");
        let bogus_abs = bogus.to_string_lossy().to_string();
        assert!(matches!(
            resolve_existing_message(&tmp_msg, &bogus_abs),
            Err(MessagingError::InvalidFilename(_))
        ));

        let _ = std::fs::remove_dir_all(&tmp_msg);
    }

    #[test]
    fn resolve_accepts_relative_path_inside_messaging_dir() {
        let tmp = unique_tmp("ac-msg-rel");
        std::fs::create_dir_all(&tmp).unwrap();

        let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
        let base = build_filename(ts, "wg7-a", "wg7-b", "rel-ok");
        let (abs_written, f) = create_message_file(&tmp, &base).unwrap();
        drop(f);

        let rel = format!("{}/./{}", tmp.display(), base);
        let resolved = resolve_existing_message(&tmp, &rel).unwrap();
        assert_eq!(
            resolved.file_name().and_then(|n| n.to_str()).unwrap(),
            base
        );
        let _ = abs_written;

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_rejects_directory_with_md_suffix() {
        let tmp = std::env::temp_dir().join(format!(
            "ac-msg-dir-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let name = "20260418-143052-wg7-a-to-wg7-b-dir.md";
        std::fs::create_dir_all(tmp.join(name)).unwrap();

        assert!(matches!(
            resolve_existing_message(&tmp, name),
            Err(MessagingError::NotAFile(_))
        ));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Contract test: `format_pty_wrap("", "")` is the empty-placeholder
    /// expansion used by both mailbox.rs injection sites. Its length MUST
    /// equal `PTY_WRAP_FIXED`. If someone edits the `format!` string inside
    /// `format_pty_wrap` (e.g. drops the trailing `\r`, adds a space), this
    /// test fires before the clamp accounting can drift silently.
    #[test]
    fn format_pty_wrap_matches_pty_wrap_fixed() {
        assert_eq!(format_pty_wrap("", "").len(), PTY_WRAP_FIXED);
        assert_eq!(PTY_WRAP_FIXED, 19);
    }

    /// Structural round-trip: non-empty placeholders are rendered visibly.
    #[test]
    fn format_pty_wrap_round_trips_inputs() {
        let rendered = format_pty_wrap("wg7-me", "hello");
        assert!(rendered.contains("[Message from wg7-me] hello"));
        assert!(rendered.starts_with('\n'));
        assert!(rendered.ends_with("\n\r"));
    }

    /// PTY_SAFE_MAX clamp arithmetic — pathological long path must exceed
    /// `PTY_WRAP_FIXED + from.len() + body.len() > PTY_SAFE_MAX=1024`.
    #[test]
    fn pty_safe_max_clamp_rejects_long_path() {
        let from = "wg99-extremely-long-agent-name-segment";
        // Synthetic body large enough to push the total past 1024.
        let body_len = 34 + 1000; // 1000-char abs_path
        let overhead = PTY_WRAP_FIXED + from.len();
        assert!(
            body_len + overhead > PTY_SAFE_MAX,
            "expected clamp to fire for body={} overhead={} max={}",
            body_len,
            overhead,
            PTY_SAFE_MAX,
        );
    }

    /// Happy-path: realistic production body (155) + typical from (13) fits
    /// comfortably under the 1024 clamp — the case that motivated this trim.
    #[test]
    fn pty_safe_max_clamp_accepts_typical() {
        let from = "wg7-architect";
        let body_len = 155;
        let overhead = PTY_WRAP_FIXED + from.len();
        assert!(
            body_len + overhead <= PTY_SAFE_MAX,
            "expected clamp to accept body={} overhead={} max={}",
            body_len,
            overhead,
            PTY_SAFE_MAX,
        );
    }
}
