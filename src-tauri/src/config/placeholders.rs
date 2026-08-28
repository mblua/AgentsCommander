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
const AC_MATRIX_ROOT_ERROR: &str = "%AC_MATRIX_ROOT% requires an AC workgroup replica launch root";
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
                    "[placeholders] home directory is undeterminable; {} stays literal in seeded content",
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
mod tests {
    use super::*;

    #[test]
    fn non_path_values_allow_dollar_markers() {
        reject_unexpanded_markers("s3cr$tP4ss ${NAME} $NAME", "env", false).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn strip_extended_prefix_converts_verbatim_unc() {
        assert_eq!(
            strip_extended_prefix(PathBuf::from(r"\\?\UNC\server\share\repo")),
            PathBuf::from(r"\\server\share\repo")
        );
    }

    #[test]
    #[cfg(windows)]
    fn same_path_text_matches_ordinary_and_verbatim_unc() {
        assert!(same_path_text(
            Path::new(r"\\server\share\repo"),
            Path::new(r"\\?\UNC\server\share\repo")
        ));
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
        let replica = temp
            .join(".ac")
            .join("wg-7-dev-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        replica
    }

    #[test]
    fn replica_launch_expands_all_three_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();

        let expected_replica = canonical_stripped(&replica);
        let expected_ac_root = expected_replica
            .parent()
            .and_then(|parent| parent.parent())
            .expect("replica has a .ac workspace ancestor");
        let expected_matrix = expected_ac_root.join("_agent_dev-rust");

        assert_eq!(ctx.root_kind, PlaceholderRootKind::AcReplicaOrRootAgent);
        assert_eq!(ctx.replica_root, expected_replica);
        assert_eq!(ctx.ac_root.as_deref(), Some(expected_ac_root));
        assert_eq!(ctx.matrix_root.as_deref(), Some(expected_matrix.as_path()));

        assert_eq!(
            expand_placeholders(r"%AC_REPLICA_ROOT%\x", &ctx).unwrap(),
            format!(r"{}\x", expected_replica.to_string_lossy())
        );
        assert_eq!(
            expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap(),
            format!(r"{}\x", expected_ac_root.to_string_lossy())
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
        assert_eq!(ctx.ac_root, None);
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
    fn expand_in_content_substitutes_known_tokens_forward_slashed() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();

        let expected_replica = canonical_stripped(&replica)
            .to_string_lossy()
            .replace('\\', "/");
        let out = expand_placeholders_in_content(
            "root=%AC_REPLICA_ROOT%/x and %AC_WORKSPACE_ROOT% and %AC_MATRIX_ROOT%",
            &ctx,
        );
        assert!(
            out.contains(&format!("root={}/x", expected_replica)),
            "{out}"
        );
        // No backslashes from the substituted paths (forward-slash normalized).
        assert!(!out.contains('\\'), "{out}");
    }

    #[test]
    fn expand_in_content_leaves_unknown_markers_literal() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();
        // Unlike expand_placeholders, content expansion is lenient: unknown
        // %...% markers survive verbatim (the template may use them at runtime).
        let out = expand_placeholders_in_content("%UNKNOWN% and %AC_ROOT% stay", &ctx);
        assert_eq!(out, "%UNKNOWN% and %AC_ROOT% stay");
    }

    #[test]
    fn expand_in_content_skips_tokens_with_no_matching_root() {
        // Normal cwd: replica token is NOT substituted (no replica root kind),
        // and workspace/matrix are None, so all three are left literal.
        let normal = PlaceholderContext {
            replica_root: PathBuf::from(if cfg!(windows) { r"C:\cwd" } else { "/cwd" }),
            root_kind: PlaceholderRootKind::NormalLaunchCwd,
            ac_root: None,
            matrix_root: None,
        };
        let out = expand_placeholders_in_content(
            "%AC_REPLICA_ROOT% %AC_WORKSPACE_ROOT% %AC_MATRIX_ROOT%",
            &normal,
        );
        assert_eq!(
            out,
            "%AC_REPLICA_ROOT% %AC_WORKSPACE_ROOT% %AC_MATRIX_ROOT%"
        );
    }

    // ---- %USER_HOME% (#924) ------------------------------------------------

    /// `dirs::home_dir()` forward-slashed, or `None` on a machine without a home.
    /// Tests derive the expectation instead of hardcoding a literal, so they stay
    /// deterministic across OSes and CI users.
    fn expected_home_fwd() -> Option<String> {
        dirs::home_dir().map(|home| home.to_string_lossy().replace('\\', "/"))
    }

    #[test]
    fn content_expands_user_home_forward_slashed() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();

        let out = expand_placeholders_in_content("excl=%USER_HOME%/.claude/CLAUDE.md", &ctx);
        match expected_home_fwd() {
            Some(home) => {
                assert_eq!(out, format!("excl={}/.claude/CLAUDE.md", home));
                assert!(!out.contains('\\'), "{out}");
                assert!(!out.contains(USER_HOME_PLACEHOLDER), "{out}");
            }
            // No home on this machine: the token stays literal, the spawn never aborts.
            None => assert_eq!(out, "excl=%USER_HOME%/.claude/CLAUDE.md"),
        }
    }

    #[test]
    fn content_leaves_unknown_percent_markers_literal() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();
        // %USER_HOME% is the ONLY new token: sibling %VAR% markers that a tool reads
        // at runtime (hook commands, shell vars) must survive verbatim.
        let out = expand_placeholders_in_content("%TEMP% %CD% %USERPROFILE%", &ctx);
        assert_eq!(out, "%TEMP% %CD% %USERPROFILE%");
    }

    #[test]
    fn content_expands_user_home_alongside_ac_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();

        let out = expand_placeholders_in_content(
            "%AC_REPLICA_ROOT% %AC_WORKSPACE_ROOT% %AC_MATRIX_ROOT% %USER_HOME%",
            &ctx,
        );
        let expected_replica = canonical_stripped(&replica)
            .to_string_lossy()
            .replace('\\', "/");
        assert!(out.starts_with(&expected_replica), "{out}");
        assert!(!out.contains("%AC_"), "{out}");
        match expected_home_fwd() {
            Some(home) => {
                assert!(out.ends_with(&home), "{out}");
                assert!(!out.contains('\\'), "{out}");
            }
            // No home: the AC tokens still expand, %USER_HOME% stays literal.
            None => {
                assert!(out.ends_with(USER_HOME_PLACEHOLDER), "{out}");
                assert!(out.contains("%USER_HOME%"), "{out}");
            }
        }
    }

    #[test]
    fn user_home_is_not_an_ac_placeholder_token() {
        // GATE, not catalog: `value_contains_ac_placeholder` drives the "this spawn
        // requires an AC launch root" decision (agent_command.rs) and the CODEX_HOME
        // validators. %USER_HOME% needs no AC context, so it must stay out.
        assert!(!AC_PLACEHOLDER_TOKENS.contains(&USER_HOME_PLACEHOLDER));
        assert!(!value_contains_ac_placeholder("%USER_HOME%/.claude"));
    }

    #[test]
    fn strict_expand_still_rejects_user_home() {
        // v1 scope: env values and command args keep their fail-closed contract,
        // because `home_dir()` can return None and strict mode cannot degrade.
        let temp = tempfile::tempdir().unwrap();
        let replica = make_replica(temp.path());
        let ctx = placeholder_context_for_launch_root(&replica).unwrap();
        let err = expand_placeholders("%USER_HOME%/x", &ctx).unwrap_err();
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
            ac_root: None,
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
            ac_root: None,
            matrix_root: None,
        };
        assert_eq!(
            expand_placeholders(r"%AC_REPLICA_ROOT%\x", &normal).unwrap_err(),
            AC_REPLICA_ROOT_ERROR
        );
    }

    #[test]
    fn derive_matrix_root_matches_canonical_formula() {
        let ac_root = PathBuf::from(if cfg!(windows) {
            r"C:\proj\.ac"
        } else {
            "/proj/.ac"
        });
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        assert_eq!(
            derive_matrix_root(&replica, Some(&ac_root)),
            Some(ac_root.join("_agent_dev-rust"))
        );

        // Non-replica leaf -> None.
        let repo = ac_root.join("wg-7-dev-team").join("repo-thing");
        assert_eq!(derive_matrix_root(&repo, Some(&ac_root)), None);

        // Replica leaf but no .ac ancestor -> None (guards the ac_root.map arm).
        assert_eq!(derive_matrix_root(&replica, None), None);

        // F1: degenerate / invalid agent name -> None (parity with validate_agent_name).
        let empty = ac_root.join("wg-7-dev-team").join("__agent_");
        assert_eq!(derive_matrix_root(&empty, Some(&ac_root)), None);
        let dotted = ac_root.join("wg-7-dev-team").join("__agent_bad.name");
        assert_eq!(derive_matrix_root(&dotted, Some(&ac_root)), None);
    }

    #[test]
    fn non_replica_inside_ac_resolves_ac_root_only() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join(".ac").join("wg-7-dev-team").join("repo-x");
        std::fs::create_dir_all(&repo).unwrap();
        let ctx = placeholder_context_for_launch_root(&repo).unwrap();

        assert_eq!(ctx.root_kind, PlaceholderRootKind::NormalLaunchCwd);
        assert_eq!(ctx.matrix_root, None);

        let expected_replica = canonical_stripped(&repo);
        let expected_ac_root = expected_replica
            .parent()
            .and_then(|parent| parent.parent())
            .expect("repo-x has a .ac workspace ancestor");
        assert_eq!(ctx.ac_root.as_deref(), Some(expected_ac_root));

        assert_eq!(
            expand_placeholders(r"%AC_WORKSPACE_ROOT%\x", &ctx).unwrap(),
            format!(r"{}\x", expected_ac_root.to_string_lossy())
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
