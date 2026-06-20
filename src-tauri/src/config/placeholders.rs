use std::path::{Path, PathBuf};

pub const AC_REPLICA_ROOT_PLACEHOLDER: &str = "%AC_REPLICA_ROOT%";
pub const AC_WORKSPACE_ROOT_PLACEHOLDER: &str = "%AC_WORKSPACE_ROOT%";
pub const AC_MATRIX_ROOT_PLACEHOLDER: &str = "%AC_MATRIX_ROOT%";

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
const AC_MATRIX_ROOT_ERROR: &str =
    "%AC_MATRIX_ROOT% requires an AC workgroup replica launch root";
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
    pub workspace_root: Option<PathBuf>,
    pub matrix_root: Option<PathBuf>,
}

pub fn placeholder_context_for_launch_root(path: &Path) -> Result<PlaceholderContext, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Launch root '{}' is not readable: {}", path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Launch root '{}' is not a real directory",
            path.display()
        ));
    }
    let canonical = std::fs::canonicalize(path)
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

    let workspace_root = crate::config::workspace::find_workspace_ancestor(&canonical);
    let matrix_root = derive_matrix_root(&canonical, workspace_root.as_deref());

    Ok(PlaceholderContext {
        replica_root: canonical,
        root_kind,
        workspace_root,
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
fn derive_matrix_root(canonical: &Path, workspace_root: Option<&Path>) -> Option<PathBuf> {
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
    workspace_root.map(|workspace| workspace.join(format!("_agent_{}", agent_name)))
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
        let workspace = context
            .workspace_root
            .as_ref()
            .ok_or_else(|| AC_WORKSPACE_ROOT_ERROR.to_string())?;
        expanded = expanded.replace(AC_WORKSPACE_ROOT_PLACEHOLDER, &workspace.to_string_lossy());
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
    AC_PLACEHOLDER_TOKENS.iter().any(|token| value.contains(*token))
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
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").map(PathBuf::from).unwrap_or(path)
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
        .is_some_and(|name| name.starts_with("wg-"))
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
    left.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .eq_ignore_ascii_case(right.to_string_lossy().trim_start_matches(r"\\?\"))
}

#[cfg(not(windows))]
fn same_path_text(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_path_values_allow_dollar_markers() {
        reject_unexpanded_markers("s3cr$tP4ss ${NAME} $NAME", "env", false).unwrap();
    }

    #[test]
    fn non_path_values_reject_percent_markers() {
        let err = reject_unexpanded_markers("%UNKNOWN%", "env", false).unwrap_err();
        assert!(err.contains("unknown placeholder"), "{err}");
    }

    #[test]
    fn strict_path_values_reject_any_marker() {
        let err = reject_unexpanded_markers("C:/x/$HOME", "CODEX_HOME", true).unwrap_err();
        assert!(err.contains("variable markers"), "{err}");
    }

    /// Canonicalize + strip the verbatim prefix exactly like
    /// `placeholder_context_for_launch_root` does, so expected paths match the
    /// context's `replica_root` even when the OS hands back an 8.3 short form.
    fn canonical_stripped(path: &Path) -> PathBuf {
        strip_extended_prefix(std::fs::canonicalize(path).unwrap())
    }

    /// Create a real WG-replica launch root `<temp>/.ac/wg-7-dev-team/__agent_dev-rust`.
    fn make_replica(temp: &Path) -> PathBuf {
        let replica = temp.join(".ac").join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        replica
    }

    #[test]
    fn replica_launch_expands_all_three_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();

        let expected_replica = canonical_stripped(&replica);
        let expected_workspace = expected_replica
            .parent()
            .and_then(|parent| parent.parent())
            .expect("replica has a .ac workspace ancestor");
        let expected_matrix = expected_workspace.join("_agent_dev-rust");

        assert_eq!(ctx.root_kind, PlaceholderRootKind::AcReplicaOrRootAgent);
        assert_eq!(ctx.replica_root, expected_replica);
        assert_eq!(ctx.workspace_root.as_deref(), Some(expected_workspace));
        assert_eq!(ctx.matrix_root.as_deref(), Some(expected_matrix.as_path()));

        assert_eq!(
            expand_placeholders(r"%AC_REPLICA_ROOT%\x", &ctx).unwrap(),
            format!(r"{}\x", expected_replica.to_string_lossy())
        );
        assert_eq!(
            expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap(),
            format!(r"{}\x", expected_workspace.to_string_lossy())
        );
        assert_eq!(
            expand_placeholders(r"%AC_MATRIX_ROOT%\x", &ctx).unwrap(),
            format!(r"{}\x", expected_matrix.to_string_lossy())
        );
    }

    #[test]
    fn non_replica_outside_ac_errors_for_every_token() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo-thing");
        std::fs::create_dir_all(&repo).unwrap();
        let ctx = placeholder_context_for_launch_root(&repo).unwrap();

        assert_eq!(ctx.root_kind, PlaceholderRootKind::NormalLaunchCwd);
        assert_eq!(ctx.workspace_root, None);
        assert_eq!(ctx.matrix_root, None);

        assert_eq!(
            expand_placeholders(r"%AC_REPLICA_ROOT%\x", &ctx).unwrap_err(),
            AC_REPLICA_ROOT_ERROR
        );
        assert_eq!(
            expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap_err(),
            AC_WORKSPACE_ROOT_ERROR
        );
        assert_eq!(
            expand_placeholders(r"%AC_MATRIX_ROOT%\x", &ctx).unwrap_err(),
            AC_MATRIX_ROOT_ERROR
        );
    }

    #[test]
    fn legacy_ac_root_token_is_now_an_unknown_marker() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();
        // No alias: a leftover %AC_ROOT% is no longer expanded and trips the backstop.
        let err = expand_placeholders(r"%AC_ROOT%\x", &ctx).unwrap_err();
        assert!(err.contains("unknown placeholder"), "{err}");
    }

    #[test]
    fn value_contains_ac_placeholder_matches_only_known_tokens() {
        assert!(value_contains_ac_placeholder(r"%AC_REPLICA_ROOT%\x"));
        assert!(value_contains_ac_placeholder(r"%AC_WORKSPACE_ROOT%\x"));
        assert!(value_contains_ac_placeholder(r"%AC_MATRIX_ROOT%\x"));
        assert!(!value_contains_ac_placeholder(r"%AC_ROOT%\x"));
        assert!(!value_contains_ac_placeholder("%UNKNOWN%"));
        assert!(!value_contains_ac_placeholder("plain text"));
    }

    #[test]
    fn root_agent_and_normal_contexts_resolve_replica_token_only() {
        // Hermetic: build the context directly because `is_root_agent_dir` reads the
        // real configured root_agent_dir() from disk and cannot be faked in a unit test.
        let root_agent = PlaceholderContext {
            replica_root: PathBuf::from(if cfg!(windows) {
                r"C:\some\replica"
            } else {
                "/some/replica"
            }),
            root_kind: PlaceholderRootKind::AcReplicaOrRootAgent,
            workspace_root: None,
            matrix_root: None,
        };
        assert!(expand_placeholders(r"%AC_REPLICA_ROOT%\x", &root_agent).is_ok());
        assert_eq!(
            expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &root_agent).unwrap_err(),
            AC_WORKSPACE_ROOT_ERROR
        );
        assert_eq!(
            expand_placeholders(r"%AC_MATRIX_ROOT%\x", &root_agent).unwrap_err(),
            AC_MATRIX_ROOT_ERROR
        );

        let normal = PlaceholderContext {
            replica_root: PathBuf::from(if cfg!(windows) {
                r"C:\some\cwd"
            } else {
                "/some/cwd"
            }),
            root_kind: PlaceholderRootKind::NormalLaunchCwd,
            workspace_root: None,
            matrix_root: None,
        };
        assert_eq!(
            expand_placeholders(r"%AC_REPLICA_ROOT%\x", &normal).unwrap_err(),
            AC_REPLICA_ROOT_ERROR
        );
    }

    #[test]
    fn derive_matrix_root_matches_canonical_formula() {
        let workspace = PathBuf::from(if cfg!(windows) { r"C:\proj\.ac" } else { "/proj/.ac" });
        let replica = workspace.join("wg-7-dev-team").join("__agent_dev-rust");
        assert_eq!(
            derive_matrix_root(&replica, Some(&workspace)),
            Some(workspace.join("_agent_dev-rust"))
        );

        // Non-replica leaf -> None.
        let repo = workspace.join("wg-7-dev-team").join("repo-thing");
        assert_eq!(derive_matrix_root(&repo, Some(&workspace)), None);

        // Replica leaf but no .ac ancestor -> None (guards the workspace_root.map arm).
        assert_eq!(derive_matrix_root(&replica, None), None);

        // F1: degenerate / invalid agent name -> None (parity with validate_agent_name).
        let empty = workspace.join("wg-7-dev-team").join("__agent_");
        assert_eq!(derive_matrix_root(&empty, Some(&workspace)), None);
        let dotted = workspace.join("wg-7-dev-team").join("__agent_bad.name");
        assert_eq!(derive_matrix_root(&dotted, Some(&workspace)), None);
    }

    #[test]
    fn non_replica_inside_ac_resolves_workspace_only() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join(".ac").join("wg-7-dev-team").join("repo-x");
        std::fs::create_dir_all(&repo).unwrap();
        let ctx = placeholder_context_for_launch_root(&repo).unwrap();

        assert_eq!(ctx.root_kind, PlaceholderRootKind::NormalLaunchCwd);
        assert_eq!(ctx.matrix_root, None);

        let expected_replica = canonical_stripped(&repo);
        let expected_workspace = expected_replica
            .parent()
            .and_then(|parent| parent.parent())
            .expect("repo-x has a .ac workspace ancestor");
        assert_eq!(ctx.workspace_root.as_deref(), Some(expected_workspace));

        assert_eq!(
            expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap(),
            format!(r"{}\x", expected_workspace.to_string_lossy())
        );
        assert_eq!(
            expand_placeholders(r"%AC_REPLICA_ROOT%\x", &ctx).unwrap_err(),
            AC_REPLICA_ROOT_ERROR
        );
        assert_eq!(
            expand_placeholders(r"%AC_MATRIX_ROOT%\x", &ctx).unwrap_err(),
            AC_MATRIX_ROOT_ERROR
        );
    }
}
