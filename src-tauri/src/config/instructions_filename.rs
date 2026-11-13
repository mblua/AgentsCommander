/// AC-internal managed context file name; never a user instructions file.
pub(crate) const AC_CONTEXT_FILE_NAME: &str = "last_ac_context.md";
/// Built-in managed instructions file for Claude.
pub(crate) const CLAUDE_MD_FILE_NAME: &str = "CLAUDE.md";
/// Built-in managed instructions file for Codex, Antigravity and Pi.
pub(crate) const AGENTS_MD_FILE_NAME: &str = "AGENTS.md";

/// True only for a bare, safe `*.md` filename that can be `cwd.join`ed and
/// written without escaping the agent root (R1.5 + G9). This is a strict
/// predicate: callers trim before calling, so any surrounding whitespace here
/// is a genuine rejection (Windows silently strips trailing space/dot, which
/// would desync the validated, written, and cleanup names).
pub fn is_safe_instructions_filename(filename: &str) -> bool {
    if filename.is_empty() {
        return false;
    }
    // Length cap so `root + filename` stays well under MAX_PATH.
    if filename.chars().count() > 128 {
        return false;
    }
    // No path separators or parent-dir traversal.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return false;
    }
    // No colon: rejects a drive prefix (`C:x.md`) AND an NTFS Alternate Data
    // Stream (`AGENTS.md:evil`).
    if filename.contains(':') {
        return false;
    }
    // No control chars.
    if filename.chars().any(char::is_control) {
        return false;
    }
    // No leading/trailing whitespace and no trailing dot (Windows strips them).
    if filename.starts_with(char::is_whitespace)
        || filename.ends_with(char::is_whitespace)
        || filename.ends_with('.')
    {
        return false;
    }
    // Require a `.md` extension, matched case-insensitively (accept `AGENTS.MD`);
    // the caller keeps the user's original spelling.
    let lower = filename.to_ascii_lowercase();
    let Some(stem) = lower.strip_suffix(".md") else {
        return false;
    };
    // Non-empty stem (reject bare `.md`) and no space immediately before `.md`.
    if stem.is_empty() || stem.ends_with(char::is_whitespace) {
        return false;
    }
    // G9: reject the exact AC-internal sentinel (case-insensitive). The
    // user-facing built-ins CLAUDE.md / AGENTS.md stay allowed.
    if filename.eq_ignore_ascii_case(AC_CONTEXT_FILE_NAME) {
        return false;
    }
    // Windows reserved device names, checked case-insensitively on the segment
    // BEFORE THE FIRST DOT: Windows resolves a device by that base segment and
    // ignores the extension, so `CON.md` AND `CON.foo.md` both hit the CON
    // device. CON, PRN, AUX, NUL, COM1..COM9, LPT1..LPT9.
    let device_segment = stem.split('.').next().unwrap_or(stem);
    if is_reserved_device_segment(device_segment) {
        return false;
    }
    true
}

/// True iff `segment` (already lowercased) is a Windows reserved device name.
/// COM/LPT match digits 1-9 only (COM0/LPT0 are not devices).
fn is_reserved_device_segment(segment: &str) -> bool {
    if matches!(segment, "con" | "prn" | "aux" | "nul") {
        return true;
    }
    let bytes = segment.as_bytes();
    bytes.len() == 4
        && (segment.starts_with("com") || segment.starts_with("lpt"))
        && matches!(bytes[3], b'1'..=b'9')
}
