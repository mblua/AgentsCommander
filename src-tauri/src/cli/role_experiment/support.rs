use super::CliError;
use super::CliWarning;
use super::DiffStats;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::path::PathBuf;

pub(super) fn source_matrix_path(ac_root: &Path, source_agent: &str) -> PathBuf {
    ac_root.join(format!("_agent_{}", source_agent))
}

pub(super) fn variant_agent_name(source_agent: &str, variant: &str) -> String {
    format!("{}-{}", source_agent, variant)
}

pub(super) fn variant_matrix_path(ac_root: &Path, source_agent: &str, variant: &str) -> PathBuf {
    ac_root.join(format!(
        "_agent_{}",
        variant_agent_name(source_agent, variant)
    ))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn map_err<T, E: std::fmt::Display>(
    result: Result<T, E>,
    code: &'static str,
    variant: Option<&str>,
) -> Result<T, Vec<CliError>> {
    result.map_err(|e| vec![err(code, e.to_string(), variant)])
}

pub(super) fn err(
    code: &'static str,
    message: impl Into<String>,
    variant: Option<&str>,
) -> CliError {
    CliError {
        code: code.to_string(),
        message: message.into(),
        variant: variant.map(str::to_string),
        line: None,
        field: None,
        path: None,
    }
}

pub(super) fn err_at_line(
    code: &'static str,
    message: impl Into<String>,
    line: usize,
    field: Option<&str>,
) -> CliError {
    CliError {
        code: code.to_string(),
        message: message.into(),
        variant: None,
        line: Some(line),
        field: field.map(str::to_string),
        path: None,
    }
}

pub(super) fn err_at_path(code: &'static str, message: impl Into<String>, path: &Path) -> CliError {
    CliError {
        code: code.to_string(),
        message: message.into(),
        variant: None,
        line: None,
        field: None,
        path: Some(path.to_string_lossy().to_string()),
    }
}

pub(super) fn warn(
    code: &'static str,
    message: impl Into<String>,
    variant: Option<String>,
) -> CliWarning {
    CliWarning {
        code: code.to_string(),
        message: message.into(),
        variant,
    }
}

pub(super) fn line_diff(
    left_name: &str,
    right_name: &str,
    left: &str,
    right: &str,
) -> (String, DiffStats) {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();
    if left == right {
        return (
            format!("--- {}\n+++ {}\n", left_name, right_name),
            DiffStats {
                added_lines: 0,
                removed_lines: 0,
                changed_lines: 0,
                identical: true,
            },
        );
    }
    let max = left_lines.len().max(right_lines.len());
    let mut added = 0;
    let mut removed = 0;
    let mut diff = format!("--- {}\n+++ {}\n", left_name, right_name);
    for i in 0..max {
        match (left_lines.get(i), right_lines.get(i)) {
            (Some(a), Some(b)) if a == b => {
                diff.push(' ');
                diff.push_str(a);
                diff.push('\n');
            }
            (Some(a), Some(b)) => {
                removed += 1;
                added += 1;
                diff.push('-');
                diff.push_str(a);
                diff.push('\n');
                diff.push('+');
                diff.push_str(b);
                diff.push('\n');
            }
            (Some(a), None) => {
                removed += 1;
                diff.push('-');
                diff.push_str(a);
                diff.push('\n');
            }
            (None, Some(b)) => {
                added += 1;
                diff.push('+');
                diff.push_str(b);
                diff.push('\n');
            }
            (None, None) => {}
        }
    }
    (
        diff,
        DiffStats {
            added_lines: added,
            removed_lines: removed,
            changed_lines: added + removed,
            identical: false,
        },
    )
}
