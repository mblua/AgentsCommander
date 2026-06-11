use clap::Args;
use serde::Serialize;
use std::path::{Path, PathBuf};

use super::window_placement::TESTABLE_EXE_NAME;

const TESTABLE_CONFIG_DIR: &str = ".agentscommander_testeable";
const TESTABLE_PROJECT_DIR: &str = "agentscommander_testeable";

#[derive(Debug, Args)]
pub struct TestResetArgs {
    #[arg(long)]
    pub confirm_testeable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestResetOutput {
    ok: bool,
    exe_path: PathBuf,
    deleted: Vec<PathBuf>,
    missing: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestResetPlan {
    ok: bool,
    exe_path: PathBuf,
    planned_delete: Vec<PathBuf>,
}

pub fn execute(args: TestResetArgs) -> i32 {
    match execute_inner(args) {
        Ok(output) => {
            print_stdout_json(&output);
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn execute_inner(args: TestResetArgs) -> Result<TestResetOutput, String> {
    if !args.confirm_testeable {
        return Err(error_json(
            "missing_confirm_testeable",
            serde_json::json!({"required": "--confirm-testeable"}),
        ));
    }

    let exe_path = std::env::current_exe().map_err(|e| {
        error_json(
            "current_exe_failed",
            serde_json::json!({"message": e.to_string()}),
        )
    })?;
    let exe_name = exe_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if exe_name != TESTABLE_EXE_NAME {
        return Err(error_json(
            "refusing_non_testeable_binary",
            serde_json::json!({"exePath": exe_path, "expected": TESTABLE_EXE_NAME}),
        ));
    }

    let exe_parent = exe_path.parent().ok_or_else(|| {
        error_json(
            "current_exe_has_no_parent",
            serde_json::json!({"exePath": exe_path}),
        )
    })?;

    let (active, _mutex_guard) =
        crate::testability::acquire_profile_mutex_probe().map_err(|e| {
            error_json(
                "profile_mutex_probe_failed",
                serde_json::json!({"message": e}),
            )
        })?;
    if active {
        return Err(error_json("testable_gui_active", serde_json::json!({})));
    }

    let candidates = candidate_paths(exe_parent);
    for candidate in &candidates {
        validate_candidate(exe_parent, candidate).map_err(|e| {
            error_json(
                e.as_str(),
                serde_json::json!({"path": candidate, "exeParent": exe_parent}),
            )
        })?;
    }

    print_stdout_json(&TestResetPlan {
        ok: true,
        exe_path: exe_path.clone(),
        planned_delete: candidates.clone(),
    });

    let mut deleted = Vec::new();
    let mut missing = Vec::new();
    for candidate in &candidates {
        validate_candidate(exe_parent, candidate).map_err(|e| {
            error_json(
                e.as_str(),
                serde_json::json!({"path": candidate, "exeParent": exe_parent}),
            )
        })?;

        if candidate.exists() {
            std::fs::remove_dir_all(candidate).map_err(|e| {
                error_json(
                    "remove_dir_all_failed",
                    serde_json::json!({"path": candidate, "message": e.to_string()}),
                )
            })?;
            deleted.push(candidate.clone());
        } else {
            missing.push(candidate.clone());
        }
    }

    Ok(TestResetOutput {
        ok: true,
        exe_path,
        deleted,
        missing,
    })
}

fn candidate_paths(exe_parent: &Path) -> Vec<PathBuf> {
    vec![
        exe_parent.join(TESTABLE_CONFIG_DIR),
        exe_parent.join(TESTABLE_PROJECT_DIR),
    ]
}

fn validate_candidate(exe_parent: &Path, candidate: &Path) -> Result<(), String> {
    let parent_ok = candidate.parent() == Some(exe_parent);
    if !parent_ok {
        return Err("reset_candidate_parent_mismatch".to_string());
    }

    let file_name = candidate.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if file_name != TESTABLE_CONFIG_DIR && file_name != TESTABLE_PROJECT_DIR {
        return Err("reset_candidate_name_not_allowed".to_string());
    }

    let metadata = match std::fs::symlink_metadata(candidate) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("reset_candidate_metadata_failed: {e}")),
    };

    if metadata.file_type().is_symlink() {
        return Err("reset_candidate_is_symlink".to_string());
    }
    if !metadata.is_dir() {
        return Err("reset_candidate_not_directory".to_string());
    }
    if has_windows_reparse_point(&metadata) {
        return Err("reset_candidate_is_reparse_point".to_string());
    }

    let canonical_parent = exe_parent
        .canonicalize()
        .map_err(|e| format!("reset_parent_canonicalize_failed: {e}"))?;
    let expected = canonical_parent.join(file_name);
    let actual = candidate
        .canonicalize()
        .map_err(|e| format!("reset_candidate_canonicalize_failed: {e}"))?;
    if actual != expected {
        return Err("reset_candidate_canonical_path_mismatch".to_string());
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn has_windows_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn has_windows_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn print_stdout_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => crate::cli_println!("{json}"),
        Err(e) => crate::cli_println!(
            "{{\"ok\":false,\"error\":\"json_serialize_failed\",\"message\":{}}}",
            serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"unknown\"".to_string())
        ),
    }
}

fn error_json(error: &str, extra: serde_json::Value) -> String {
    let mut value = serde_json::json!({
        "ok": false,
        "error": error,
    });
    if let (Some(dst), Some(src)) = (value.as_object_mut(), extra.as_object()) {
        for (key, value) in src {
            dst.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_string(&value)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"json_serialize_failed\"}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_candidates_are_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join(TESTABLE_CONFIG_DIR);
        validate_candidate(tmp.path(), &candidate).unwrap();
    }

    #[test]
    fn file_candidate_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join(TESTABLE_CONFIG_DIR);
        std::fs::write(&candidate, "not a dir").unwrap();
        assert_eq!(
            validate_candidate(tmp.path(), &candidate).unwrap_err(),
            "reset_candidate_not_directory"
        );
    }
}
