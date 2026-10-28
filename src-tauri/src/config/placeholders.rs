use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const AC_REPLICA_ROOT_PLACEHOLDER: &str = "%AC_REPLICA_ROOT%";
pub const AC_WORKSPACE_ROOT_PLACEHOLDER: &str = "%AC_WORKSPACE_ROOT%";
pub const AC_MATRIX_ROOT_PLACEHOLDER: &str = "%AC_MATRIX_ROOT%";

/// The current user's home directory (#924). Deliberately NOT a member of
/// [`AC_PLACEHOLDER_TOKENS`]: that array is the launch-root GATE, and this token
/// needs no AC context (it resolves from `dirs::home_dir()`, which is
/// process-global). It is expanded in seeded file CONTENT only, never in the
/// strict env/arg path.
pub const USER_HOME_PLACEHOLDER: &str = "%USER_HOME%";

/// Single source of truth for the AC path-placeholder token set. Every site that
/// gates on, validates, or scans these tokens MUST iterate this list, so adding a
/// token wires the gate AND the CODEX_HOME validators at once (the #576 hard rule).
/// Expansion in `expand_placeholders` stays as explicit per-token `if` blocks: each
/// token has its own root field + error and fails closed if forgotten, so a loop
/// there would obscure more than it saves.
pub const AC_PLACEHOLDER_TOKENS: &[&str] = &[
    AC_REPLICA_ROOT_PLACEHOLDER,
    AC_WORKSPACE_ROOT_PLACEHOLDER,
    AC_MATRIX_ROOT_PLACEHOLDER,
];

const AC_REPLICA_ROOT_ERROR: &str =
    "%AC_REPLICA_ROOT% requires an AC replica or root-agent launch root";
const AC_WORKSPACE_ROOT_ERROR: &str =
    "%AC_WORKSPACE_ROOT% requires a launch root inside an AC (.ac) workspace";
const AC_MATRIX_ROOT_ERROR: &str = "%AC_MATRIX_ROOT% requires an AC room replica launch root";
const AC_PLACEHOLDER_LAUNCH_ROOT_ERROR: &str = "AC path placeholders require an AC launch root";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderRootKind {
    AcReplicaOrRootAgent,
    NormalLaunchCwd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderContext {
    pub replica_root: PathBuf,
    pub root_kind: PlaceholderRootKind,
    pub ac_root: Option<PathBuf>,
    pub matrix_root: Option<PathBuf>,
}

pub fn placeholder_context_for_launch_root(path: &Path) -> Result<PlaceholderContext, String> {
    let metadata = crate::filesystem::directories::launch_root_metadata(path)
        .map_err(|e| format!("Launch root '{}' is not readable: {}", path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Launch root '{}' is not a real directory",
            path.display()
        ));
    }
    let canonical = crate::filesystem::directories::canonical_launch_root(path)
        .map(strip_extended_prefix)
        .map_err(|e| {
            format!(
                "Failed to canonicalize launch root '{}': {}",
                path.display(),
                e
            )
        })?;
    let root_kind = if is_ac_replica_dir(&canonical) || is_root_agent_dir(&canonical) {
        PlaceholderRootKind::AcReplicaOrRootAgent
    } else {
        PlaceholderRootKind::NormalLaunchCwd
    };

    let ac_root = crate::config::ac_root::find_ac_root_ancestor(&canonical);
    let matrix_root = derive_matrix_root(&canonical, ac_root.as_deref());

    Ok(PlaceholderContext {
        replica_root: canonical,
        root_kind,
        ac_root,
        matrix_root,
    })
}

/// Matrix dir for a WG replica launch: `<workspace>/_agent_<name>`.
/// Derives the agent name the SAME way as the canonical
/// `crate::config::replica_identity::expected_wg_replica_identity`: strip the
/// `__agent_` prefix and require a non-empty `[A-Za-z0-9-]` name (the exact rule in
/// that module's `validate_agent_name`). It INTENTIONALLY omits the canonical
/// formula's on-disk `validate_local_matrix` (existence + containment) check, because
/// the placeholder path must stay pure: no filesystem read, no config repair (#576 §8).
/// Returns None for the root-agent, for normal (non-replica) launch roots, and for a
/// degenerate/invalid replica leaf (e.g. `__agent_` empty name, or `__agent_bad.name`).
fn derive_matrix_root(canonical: &Path, ac_root: Option<&Path>) -> Option<PathBuf> {
    if !is_ac_replica_dir(canonical) {
        return None;
    }
    let agent_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("__agent_"))
        .filter(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })?;
    ac_root.map(|ac_root| ac_root.join(format!("_agent_{}", agent_name)))
}

pub fn expand_placeholders(value: &str, context: &PlaceholderContext) -> Result<String, String> {
    let mut expanded = value.to_string();

    if expanded.contains(AC_REPLICA_ROOT_PLACEHOLDER) {
        if context.root_kind != PlaceholderRootKind::AcReplicaOrRootAgent {
            return Err(AC_REPLICA_ROOT_ERROR.to_string());
        }
        expanded = expanded.replace(
            AC_REPLICA_ROOT_PLACEHOLDER,
            &context.replica_root.to_string_lossy(),
        );
    }

    if expanded.contains(AC_WORKSPACE_ROOT_PLACEHOLDER) {
        let ac_root = context
            .ac_root
            .as_ref()
            .ok_or_else(|| AC_WORKSPACE_ROOT_ERROR.to_string())?;
        expanded = expanded.replace(AC_WORKSPACE_ROOT_PLACEHOLDER, &ac_root.to_string_lossy());
    }

    if expanded.contains(AC_MATRIX_ROOT_PLACEHOLDER) {
        let matrix = context
            .matrix_root
            .as_ref()
            .ok_or_else(|| AC_MATRIX_ROOT_ERROR.to_string())?;
        expanded = expanded.replace(AC_MATRIX_ROOT_PLACEHOLDER, &matrix.to_string_lossy());
    }

    reject_unexpanded_markers(&expanded, "placeholder value", false)?;
    Ok(expanded)
}

pub fn expand_placeholders_in_args(
    values: &[String],
    context: &PlaceholderContext,
) -> Result<Vec<String>, String> {
    values
        .iter()
        .map(|value| expand_placeholders(value, context))
        .collect()
}

/// Process-global home directory, resolved once. `None` when `dirs::home_dir()`
/// cannot determine it (no HOME / USERPROFILE); callers leave the token literal.
fn user_home() -> Option<&'static Path> {
    static USER_HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    USER_HOME
        .get_or_init(|| {
            let home = dirs::home_dir();
            if home.is_none() {
                log::warn!(
                    "[placeholders] home directory is undeterminable; \
                     {} stays literal in seeded content",
                    USER_HOME_PLACEHOLDER
                );
            }
            home
        })
        .as_deref()
}

/// Expand the 3 known AC tokens plus `%USER_HOME%` inside file CONTENT
/// (#598 config-folder seed, #924 user home).
/// Unlike [`expand_placeholders`] this does NOT fail-closed on unknown `%...%`:
/// a seeded template may legitimately contain other `%VAR%` text the tool reads
/// at runtime, so unknown markers are left literal. Substituted paths are
/// forward-slash normalized (JSON/TOML safe; accepted by Node/Rust/Win32).
pub fn expand_placeholders_in_content(value: &str, context: &PlaceholderContext) -> String {
    let fwd = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
    let mut out = value.to_string();
    if context.root_kind == PlaceholderRootKind::AcReplicaOrRootAgent {
        out = out.replace(AC_REPLICA_ROOT_PLACEHOLDER, &fwd(&context.replica_root));
    }
    if let Some(ws) = &context.ac_root {
        out = out.replace(AC_WORKSPACE_ROOT_PLACEHOLDER, &fwd(ws));
    }
    if let Some(mx) = &context.matrix_root {
        out = out.replace(AC_MATRIX_ROOT_PLACEHOLDER, &fwd(mx));
    }
    // #924: best-effort, exactly like the AC tokens above. An undeterminable home
    // leaves the token literal (already warned once by `user_home`) and NEVER
    // aborts the spawn.
    if out.contains(USER_HOME_PLACEHOLDER) {
        if let Some(home) = user_home() {
            out = out.replace(USER_HOME_PLACEHOLDER, &fwd(home));
        }
    }
    out
}

pub fn reject_unexpanded_markers(
    value: &str,
    context: &str,
    strict_path_value: bool,
) -> Result<(), String> {
    if strict_path_value {
        if value.contains('%') || value.contains('$') {
            return Err(format!(
                "{context}: value must be expanded before use and must not contain variable markers"
            ));
        }
        return Ok(());
    }

    if contains_percent_marker(value) {
        return Err(format!("{context}: unknown placeholder marker in value"));
    }
    Ok(())
}

pub fn value_contains_ac_placeholder(value: &str) -> bool {
    AC_PLACEHOLDER_TOKENS
        .iter()
        .any(|token| value.contains(*token))
}

pub fn ac_placeholder_error() -> &'static str {
    AC_PLACEHOLDER_LAUNCH_ROOT_ERROR
}

fn contains_percent_marker(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            idx += 1;
            continue;
        }
        let start = idx + 1;
        if start >= bytes.len() || !is_marker_start(bytes[start]) {
            idx += 1;
            continue;
        }
        let mut end = start + 1;
        while end < bytes.len() && is_marker_continue(bytes[end]) {
            end += 1;
        }
        if end < bytes.len() && bytes[end] == b'%' {
            return true;
        }
        idx = end;
    }
    false
}

fn is_marker_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_marker_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn strip_extended_prefix(path: PathBuf) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(&path)
}

fn is_ac_replica_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !name.starts_with("__agent_") {
        return false;
    }
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .is_some_and(crate::config::entity_prefix::has_entity_prefix)
}

fn is_root_agent_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.eq_ignore_ascii_case(crate::config::root_agent::ROOT_AGENT_DIR_NAME)
        && crate::config::root_agent::root_agent_dir()
            .map(|root| same_path_text(&PathBuf::from(root), path))
            .unwrap_or(false)
}

#[cfg(windows)]
fn same_path_text(left: &Path, right: &Path) -> bool {
    let left = crate::path_utils::path_to_string_without_windows_verbatim_prefix(left);
    let right = crate::path_utils::path_to_string_without_windows_verbatim_prefix(right);
    left.eq_ignore_ascii_case(&right)
}

#[cfg(not(windows))]
fn same_path_text(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(test)]
mod tests;
