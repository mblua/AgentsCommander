use std::path::{Path, PathBuf};

const AC_ROOT_PLACEHOLDER: &str = "%AC_ROOT%";
const AC_ROOT_ERROR: &str = "%AC_ROOT% requires an AC replica or root-agent launch root";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderRootKind {
    AcReplicaOrRootAgent,
    NormalLaunchCwd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderContext {
    pub ac_root: PathBuf,
    pub root_kind: PlaceholderRootKind,
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

    Ok(PlaceholderContext {
        ac_root: canonical,
        root_kind,
    })
}

pub fn expand_placeholders(value: &str, context: &PlaceholderContext) -> Result<String, String> {
    if !value.contains(AC_ROOT_PLACEHOLDER) {
        reject_unexpanded_markers(value, "placeholder value", false)?;
        return Ok(value.to_string());
    }
    if context.root_kind != PlaceholderRootKind::AcReplicaOrRootAgent {
        return Err(AC_ROOT_ERROR.to_string());
    }
    let expanded = value.replace(AC_ROOT_PLACEHOLDER, &context.ac_root.to_string_lossy());
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

pub fn value_contains_ac_root(value: &str) -> bool {
    value.contains(AC_ROOT_PLACEHOLDER)
}

pub fn ac_root_error() -> &'static str {
    AC_ROOT_ERROR
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
}
