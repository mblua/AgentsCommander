use std::path::{Path, PathBuf};

// ── #1077 portable instance-relative path codec ────────────────────────────
//
// Encodes/decodes the OS-neutral, `/`-separated companion form that pairs an
// absolute project path with the running executable's instance base. The wire
// grammar is deliberately strict and fail-closed: every decode rejection means
// the companion "did not load" and the absolute side is used instead. See
// plan #1077 §3.3. JSON extraction/serialization of these strings lives in
// `config/settings.rs`; this module owns only the lexical grammar.

/// Reasons a persisted instance-relative companion string fails to decode.
/// Each is a fail-closed rejection (the value is treated as not loading).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RelDecodeError {
    /// Empty string.
    Empty,
    /// Contains a NUL byte.
    Nul,
    /// Contains a `\` on any host (Unix filenames with `\` have no portable form).
    Backslash,
    /// A leading, trailing, or repeated `/` produced an empty segment.
    EmptySegment,
    /// An embedded `.` component (only a whole-string `.` names the base).
    DotSegment,
    /// A drive-relative component such as `C:` or `C:dir` (rejected on every host).
    DriveRelative,
    /// Parent traversal that would walk above the filesystem root.
    EscapesRoot,
    /// A Windows-illegal character (`< > : " | ? *` or a control char).
    #[cfg(windows)]
    IllegalWindowsChar,
    /// A reserved DOS device basename (CON, PRN, AUX, NUL, COM1-9, LPT1-9).
    #[cfg(windows)]
    ReservedDosName,
    /// A component ending in a space or dot (Win32 strips these).
    #[cfg(windows)]
    TrailingDotOrSpace,
    /// The instance base handed in was not absolute.
    BaseNotAbsolute,
}

/// Root/prefix identity used only for case-insensitive Windows root-compat
/// comparison. Ordinary and verbatim disk/UNC roots normalize to the same key;
/// device namespaces (`\\.\`, non-disk/UNC `\\?\`, GLOBALROOT) are unsupported
/// and never stripped.
#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RootKey {
    Disk(u8),
    Unc(String, String),
}

#[cfg(not(windows))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RootKey;

/// Split an absolute canonical path into its root key and normal components.
/// Returns `None` for an unsupported prefix, a non-absolute path, or any
/// non-normal interior component (canonical inputs never contain `.`/`..`).
#[cfg(windows)]
pub(super) fn split_root_and_normals(path: &Path) -> Option<(RootKey, Vec<std::ffi::OsString>)> {
    use std::path::{Component, Prefix};
    let mut comps = path.components();
    let prefix = match comps.next() {
        Some(Component::Prefix(p)) => p.kind(),
        _ => return None,
    };
    let root_key = match prefix {
        Prefix::Disk(l) | Prefix::VerbatimDisk(l) => RootKey::Disk(l.to_ascii_uppercase()),
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => RootKey::Unc(
            server.to_string_lossy().to_ascii_lowercase(),
            share.to_string_lossy().to_ascii_lowercase(),
        ),
        // Verbatim(non-disk), DeviceNS: unsupported. Never pass through the
        // broad prefix stripper; return None so the caller writes a null
        // companion and keeps absolute-only behavior.
        _ => return None,
    };
    // A drive-absolute path has RootDir after the prefix; `C:dir` (drive-relative)
    // does not and is rejected here.
    match comps.next() {
        Some(Component::RootDir) => {}
        _ => return None,
    }
    let mut norms = Vec::new();
    for c in comps {
        match c {
            Component::Normal(s) => norms.push(s.to_os_string()),
            _ => return None,
        }
    }
    Some((root_key, norms))
}

#[cfg(not(windows))]
pub(super) fn split_root_and_normals(path: &Path) -> Option<(RootKey, Vec<std::ffi::OsString>)> {
    use std::path::Component;
    let mut comps = path.components();
    match comps.next() {
        Some(Component::RootDir) => {}
        _ => return None,
    }
    let mut norms = Vec::new();
    for c in comps {
        match c {
            Component::Normal(s) => norms.push(s.to_os_string()),
            _ => return None,
        }
    }
    Some((RootKey, norms))
}

/// Root compatibility. Windows drive letters and UNC server/share compare
/// case-insensitively (already normalized in `RootKey`); POSIX roots always
/// match. Incompatible roots (different drive letters or UNC shares) have no
/// lexical relative form.
#[cfg(windows)]
pub(super) fn roots_compatible(a: &RootKey, b: &RootKey) -> bool {
    a == b
}

#[cfg(not(windows))]
pub(super) fn roots_compatible(_a: &RootKey, _b: &RootKey) -> bool {
    true
}

/// Encode `project` relative to `base`. Both must be absolute, canonical paths.
/// Returns the OS-neutral `/`-separated wire form, or `None` when the roots are
/// incompatible, a prefix is unsupported, or any emitted component cannot be
/// represented losslessly by the wire grammar (non-UTF-8, contains `/` or `\`,
/// or is literally `.`/`..`). Interior components are compared EXACTLY (both
/// come from `canonicalize`, which fixes on-disk casing), so only the root is
/// case-insensitive on Windows.
pub(crate) fn encode_instance_relative(project: &Path, base: &Path) -> Option<String> {
    if !project.is_absolute() || !base.is_absolute() {
        return None;
    }
    let (proj_root, proj_norms) = split_root_and_normals(project)?;
    let (base_root, base_norms) = split_root_and_normals(base)?;
    if !roots_compatible(&proj_root, &base_root) {
        return None;
    }

    let mut common = 0usize;
    while common < proj_norms.len()
        && common < base_norms.len()
        && proj_norms[common] == base_norms[common]
    {
        common += 1;
    }

    let mut out: Vec<String> = Vec::new();
    for _ in common..base_norms.len() {
        out.push("..".to_string());
    }
    for c in &proj_norms[common..] {
        let s = c.to_str()?; // non-UTF-8 → no portable form
        if s.is_empty() || s.contains('/') || s.contains('\\') || s == "." || s == ".." {
            return None;
        }
        out.push(s.to_string());
    }

    if out.is_empty() {
        return Some(".".to_string());
    }
    Some(out.join("/"))
}

/// Decode a persisted `wire` companion against an absolute canonical `base`,
/// producing the syntactically resolved absolute path (before any filesystem
/// probe). Fail-closed: every malformed or unsafe spelling is an `Err`.
pub(crate) fn decode_instance_relative(wire: &str, base: &Path) -> Result<PathBuf, RelDecodeError> {
    if !base.is_absolute() {
        return Err(RelDecodeError::BaseNotAbsolute);
    }
    if wire.is_empty() {
        return Err(RelDecodeError::Empty);
    }
    if wire.contains('\0') {
        return Err(RelDecodeError::Nul);
    }
    // Reject `\` on every host so a Windows-authored value cannot smuggle a
    // separator and a Unix filename containing `\` has no portable companion.
    if wire.contains('\\') {
        return Err(RelDecodeError::Backslash);
    }
    // `.` is the only spelling for the base itself.
    if wire == "." {
        return Ok(base.to_path_buf());
    }

    let mut result = base.to_path_buf();
    for seg in wire.split('/') {
        if seg.is_empty() {
            // Leading/trailing/repeated separator.
            return Err(RelDecodeError::EmptySegment);
        }
        if seg == "." {
            return Err(RelDecodeError::DotSegment);
        }
        if seg == ".." {
            // Popping past the root is walking above the filesystem root.
            if !result.pop() {
                return Err(RelDecodeError::EscapesRoot);
            }
            continue;
        }
        validate_wire_component(seg)?;
        result.push(seg);
    }
    Ok(result)
}

/// Validate one non-`.`/`..` wire component before it is pushed onto the base.
pub(super) fn validate_wire_component(seg: &str) -> Result<(), RelDecodeError> {
    // Drive-relative like `C:` or `C:dir` is rejected on every host so a
    // relative field can never smuggle a drive root.
    let bytes = seg.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(RelDecodeError::DriveRelative);
    }
    #[cfg(windows)]
    {
        for ch in seg.chars() {
            if (ch as u32) < 0x20 || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                return Err(RelDecodeError::IllegalWindowsChar);
            }
        }
        if seg.ends_with(' ') || seg.ends_with('.') {
            return Err(RelDecodeError::TrailingDotOrSpace);
        }
        if is_reserved_dos_name(seg) {
            return Err(RelDecodeError::ReservedDosName);
        }
    }
    Ok(())
}

/// Case-insensitive DOS device basename check (CON, PRN, AUX, NUL, COM1-9,
/// LPT1-9). The device name matches on the portion before the first `.`, so
/// `CON.txt` and an ADS spelling both resolve to the reserved device.
#[cfg(windows)]
pub(super) fn is_reserved_dos_name(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or(seg);
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = upper.as_bytes();
    upper.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}
