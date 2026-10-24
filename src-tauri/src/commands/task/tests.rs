//! Covers `cli::task_ops::perform` and `validate_wg_root` through the
//! local `read_task_fields_at` reader: a single read of TASK.md must
//! return BOTH the trimmed body and the parsed YAML `title:`. The reader
//! is test-only (#1177); #301 shipped without a production caller, and the
//! assertions it backs still cover the live save and clean paths.
use std::path::Path;

use super::*;
use crate::cli::task_ops::{perform, EditOutcome, TaskOp};

/// Read TASK.md once and return both the trimmed full body and the parsed
/// YAML `title:` value. Returning both from one read avoids torn results when
/// an external writer races us between two reads. Caller emits both fields so
/// the sidebar can update its title without waiting for the next 15s poll.
fn read_task_fields_at(wg_root: &Path) -> (Option<String>, Option<String>) {
    let Ok(content) = std::fs::read_to_string(wg_root.join("TASK.md")) else {
        return (None, None);
    };
    let task_title = crate::commands::entity_creation::parse_task_title(&content);
    let trimmed = content.trim();
    let task = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    (task, task_title)
}

#[test]
fn read_task_fields_at_missing_file_returns_none_pair() {
    let dir = tempfile::tempdir().unwrap();
    let (task, title) = read_task_fields_at(dir.path());
    assert_eq!(task, None);
    assert_eq!(title, None);
}

#[test]
fn read_task_fields_at_empty_file_returns_none_pair() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("TASK.md"), "").unwrap();
    let (task, title) = read_task_fields_at(dir.path());
    assert_eq!(task, None);
    assert_eq!(title, None);
}

#[test]
fn read_task_fields_at_parses_frontmatter_title_and_body() {
    let dir = tempfile::tempdir().unwrap();
    let content = "---\ntitle: 'My Brief'\n---\nbody line\n";
    std::fs::write(dir.path().join("TASK.md"), content).unwrap();
    let (task, title) = read_task_fields_at(dir.path());
    assert_eq!(title.as_deref(), Some("My Brief"));
    assert_eq!(task.as_deref(), Some(content.trim()));
}

#[test]
fn read_task_fields_at_body_only_no_frontmatter_returns_body_and_no_title() {
    // grinch LOW (PR #304 review): coverage gap — a file with body content
    // but no YAML frontmatter must return (Some(body), None), not (None, _).
    let dir = tempfile::tempdir().unwrap();
    let content = "Just a body line\nmore body\n";
    std::fs::write(dir.path().join("TASK.md"), content).unwrap();
    let (task, title) = read_task_fields_at(dir.path());
    assert_eq!(title, None);
    assert_eq!(task.as_deref(), Some(content.trim()));
}

#[test]
fn set_title_via_task_ops_round_trip_returns_title() {
    // End-to-end mirror of the task_set_title body: perform() then
    // read_task_fields_at(). Validates that the path the Tauri command
    // takes ends up with a non-empty taskTitle in the payload.
    let dir = tempfile::tempdir().unwrap();
    perform(dir.path(), TaskOp::SetTitle("Hello World".to_string())).expect("set title");
    let (task, title) = read_task_fields_at(dir.path());
    assert_eq!(title.as_deref(), Some("Hello World"));
    assert!(
        task.is_some(),
        "task body should not be empty after set-title"
    );
}

#[test]
fn clean_via_task_ops_round_trip_returns_clean_title() {
    // After Clean, the canonical title is "Clean" (see TaskOp::Clean
    // docs). The important thing for issue #301 is the payload carries
    // the title at all (no None → no undefined → no spread-clobber).
    let dir = tempfile::tempdir().unwrap();
    perform(dir.path(), TaskOp::SetTitle("Old Title".to_string())).expect("set initial title");
    perform(dir.path(), TaskOp::Clean).expect("clean");
    let (_task, title) = read_task_fields_at(dir.path());
    assert_eq!(title.as_deref(), Some("Clean"));
}

// ---- #545: validate_wg_root + task_clean_at path-based clean ----
//
// `validate_wg_root` takes `&[String]`, so it is unit-testable directly with
// no Tauri harness. Gate 1 canonicalizes, so every fixture dir MUST exist on
// disk. `%TEMP%` is often itself a junction; the helper canonicalizes both the
// candidate and each project base, so the component-wise `starts_with` still
// holds. The `#[tauri::command]` wrapper (AppHandle/State) is not unit-testable
// without a Tauri runtime, same as `task_clean`; the pure helper + perform
// tests cover the logic.

/// Create `<base>/.ac/wg-1` on disk and return its path as the string the
/// frontend would send. Centralizes the gate-1-needs-existence requirement.
fn make_real_wg(base: &std::path::Path) -> String {
    let wg = base.join(".ac").join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    wg.to_string_lossy().into_owned()
}

fn paths_of(p: &std::path::Path) -> Vec<String> {
    vec![p.to_string_lossy().into_owned()]
}

// 1.
#[test]
fn validate_wg_root_accepts_real_wg_under_project_root() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    assert!(validate_wg_root(&wg, &paths_of(proj.path())).is_ok());
}

// 2. gate 1 (canonicalize fails on a missing path)
#[test]
fn validate_wg_root_rejects_nonexistent_path() {
    let proj = tempfile::tempdir().unwrap();
    let bogus = proj
        .path()
        .join(".ac")
        .join("wg-does-not-exist")
        .to_string_lossy()
        .into_owned();
    assert!(validate_wg_root(&bogus, &paths_of(proj.path())).is_err());
}

// 3. gate 3 (leaf does not start with `wg-`)
#[test]
fn validate_wg_root_rejects_non_wg_dir() {
    let proj = tempfile::tempdir().unwrap();
    let notwg = proj.path().join(".ac").join("notwg");
    std::fs::create_dir_all(&notwg).unwrap();
    let arg = notwg.to_string_lossy().into_owned();
    assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_err());
}

// 4. gate 2 (a file, not a directory)
#[test]
fn validate_wg_root_rejects_file_not_dir() {
    let proj = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ac")).unwrap();
    let file = proj.path().join(".ac").join("wg-x");
    std::fs::write(&file, "x").unwrap();
    let arg = file.to_string_lossy().into_owned();
    assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_err());
}

// 5. gate 4 (a sibling root, not under the configured project path). The
//    fixture parent is `.ac` so it passes gate 5, isolating gate 4.
#[test]
fn validate_wg_root_rejects_path_outside_project_roots() {
    let proj = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let wg = make_real_wg(other.path());
    // project_paths points at `proj`, but the candidate lives under `other`.
    assert!(validate_wg_root(&wg, &paths_of(proj.path())).is_err());
}

// 6. gate 4 via traversal: `..` is normalized by canonicalize, then confined,
//    not naively string-rejected.
#[test]
fn validate_wg_root_traversal_cannot_escape() {
    let root = tempfile::tempdir().unwrap();
    let proj = root.path().join("proj");
    let other = root.path().join("other");
    std::fs::create_dir_all(proj.join(".ac").join("wg-1")).unwrap();
    std::fs::create_dir_all(other.join(".ac").join("wg-1")).unwrap();
    // proj/.ac/wg-1/../../../other/.ac/wg-1 canonicalizes to other/.ac/wg-1.
    let arg = proj
        .join(".ac")
        .join("wg-1")
        .join("..")
        .join("..")
        .join("..")
        .join("other")
        .join(".ac")
        .join("wg-1")
        .to_string_lossy()
        .into_owned();
    assert!(validate_wg_root(&arg, &paths_of(&proj)).is_err());
}

// 7. end-to-end: validate then perform(Clean) yields the canonical Clean title.
#[test]
fn task_clean_at_round_trip_returns_clean_title() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    perform(
        std::path::Path::new(&wg),
        TaskOp::SetTitle("Real Brief".to_string()),
    )
    .expect("seed title");
    let validated = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
    perform(&validated, TaskOp::Clean).expect("clean");
    let (_task, title) = read_task_fields_at(&validated);
    assert_eq!(title.as_deref(), Some("Clean"));
}

// 8. gate 4 fail-closed: an empty project_paths slice rejects everything.
#[test]
fn validate_wg_root_rejects_empty_project_paths() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    let empty: Vec<String> = vec![];
    assert!(validate_wg_root(&wg, &empty).is_err());
}

// 9. an un-canonicalizable project entry is skipped, never a wildcard match;
//    one bad entry does not poison a valid one.
#[test]
fn validate_wg_root_skips_uncanonicalizable_project_entry() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    let only_bogus = vec!["Z:\\does\\not\\exist".to_string()];
    assert!(validate_wg_root(&wg, &only_bogus).is_err());
    let bogus_plus_real = vec![
        "Z:\\does\\not\\exist".to_string(),
        proj.path().to_string_lossy().into_owned(),
    ];
    assert!(validate_wg_root(&wg, &bogus_plus_real).is_ok());
}

// 10. a single descendant-of-base check (gate 4) covers the
//     `<base>/<child>/.ac/wg-*` discovery layout; gate 5 still passes.
#[test]
fn validate_wg_root_accepts_wg_under_child_of_base() {
    let proj = tempfile::tempdir().unwrap();
    let wg = proj.path().join("child").join(".ac").join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let arg = wg.to_string_lossy().into_owned();
    assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_ok());
}

// 11. core contract: the validated path returned is the ORIGINAL arg, never
//     the canonical form (a canonical return would break event-key matching
//     at runtime while passing every other test).
#[test]
fn validate_wg_root_returns_original_not_canonical() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    let result = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
    assert_eq!(result, std::path::PathBuf::from(&wg));
    // On a junction-backed temp dir (common for %TEMP% on Windows) the
    // canonical form differs from the arg; when it does, the result must be
    // the arg, not the canonical path.
    if let Ok(canon) = std::fs::canonicalize(&wg) {
        if canon.as_path() != std::path::Path::new(&wg) {
            assert_ne!(result, canon, "must return original arg, not canonical");
        }
    }
}

// 12. canonicalize normalizes case, so gate 4's case-sensitive `starts_with`
//     still matches a case-variant input. Windows-only: case-insensitive path
//     semantics are platform-specific.
#[cfg(windows)]
#[test]
fn validate_wg_root_accepts_case_variant_input() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    // Flip the drive-letter case (e.g. C:\ -> c:\); canonicalize resolves it
    // back to the real on-disk casing.
    let mut chars: Vec<char> = wg.chars().collect();
    if let Some(first) = chars.first_mut() {
        *first = if first.is_ascii_uppercase() {
            first.to_ascii_lowercase()
        } else {
            first.to_ascii_uppercase()
        };
    }
    let variant: String = chars.into_iter().collect();
    let r = validate_wg_root(&variant, &paths_of(proj.path()));
    assert!(
        r.is_ok(),
        "case-variant drive letter should validate: {:?}",
        r
    );
}

// B1. create-on-missing: a wg dir with no TASK.md gets a canonical Clean file
//     written (backup None because nothing pre-existed). Pins this capability
//     as a conscious, gate-5-guarded behavior.
#[test]
fn task_clean_at_creates_task_when_missing() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    let validated = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
    assert!(
        !validated.join("TASK.md").exists(),
        "precondition: no TASK.md"
    );
    let outcome = perform(&validated, TaskOp::Clean).expect("clean creates file");
    assert!(matches!(outcome, EditOutcome::Wrote { backup: None, .. }));
    assert!(validated.join("TASK.md").exists());
    let (_task, title) = read_task_fields_at(&validated);
    assert_eq!(title.as_deref(), Some("Clean"));
}

// B2. gate 5: a `wg-*` dir whose parent is NOT an `.ac` workspace dir is
//     rejected even though it passes gates 1-4. This is the footgun closer.
#[test]
fn validate_wg_root_rejects_wg_dir_not_under_dot_ac() {
    let proj = tempfile::tempdir().unwrap();
    let wg = proj.path().join("sub").join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let arg = wg.to_string_lossy().into_owned();
    assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_err());
}

// ---- #1536: task_set_title_at (path-based user title) + validate_user_title ----
//
// SetUserTitle writes the reserved `USER: ` marker into the file and the
// readers (parse_task_title / title_value_of) return the raw scalar, so the
// readback assertions below expect the `USER: ` prefix (round-3 marker
// correction) — the same scalar the live task_set_title path writes.

// 1. create-on-missing: a wg dir with no TASK.md gets a user title written
//    (backup None because nothing pre-existed), mirroring
//    task_clean_at_creates_task_when_missing.
#[test]
fn task_set_title_at_creates_task_when_missing() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    let validated = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
    assert!(
        !validated.join("TASK.md").exists(),
        "precondition: no TASK.md"
    );
    let outcome = perform(&validated, TaskOp::SetUserTitle("My Title".into()))
        .expect("set user title creates file");
    assert!(matches!(outcome, EditOutcome::Wrote { backup: None, .. }));
    assert!(validated.join("TASK.md").exists());
    let (_task, title) = read_task_fields_at(&validated);
    assert_eq!(title.as_deref(), Some("USER: My Title"));
}

// 2. round trip: seed a coordinator title, then SetUserTitle over it; the
//    readback carries the `USER: ` marker, mirroring
//    task_clean_at_round_trip_returns_clean_title.
#[test]
fn task_set_title_at_round_trip_returns_title() {
    let proj = tempfile::tempdir().unwrap();
    let wg = make_real_wg(proj.path());
    perform(
        std::path::Path::new(&wg),
        TaskOp::SetTitle("Real Brief".to_string()),
    )
    .expect("seed title");
    let validated = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
    perform(&validated, TaskOp::SetUserTitle("Edited Title".into())).expect("set user title");
    let (_task, title) = read_task_fields_at(&validated);
    assert_eq!(title.as_deref(), Some("USER: Edited Title"));
}

// 3. the extracted validator is pure: same checks, messages, and order as
//    the inline block it was lifted from.
#[test]
fn validate_user_title_rejects_empty_and_whitespace() {
    assert!(validate_user_title("").is_err());
    assert!(validate_user_title("   \t  ").is_err());
}

#[test]
fn validate_user_title_rejects_control_chars_except_tab() {
    assert!(validate_user_title("bad\u{0}title").is_err());
    assert!(validate_user_title("line1\nline2").is_err());
    assert!(validate_user_title("line1\rline2").is_err());
    assert!(validate_user_title("tab\tbetween").is_ok());
}

#[test]
fn validate_user_title_rejects_over_256_scalars() {
    let ok: String = "a".repeat(256);
    let too_long: String = "a".repeat(257);
    assert!(validate_user_title(&ok).is_ok());
    assert!(validate_user_title(&too_long).is_err());
}

#[test]
fn validate_user_title_accepts_normal_title() {
    assert!(validate_user_title("My Title").is_ok());
    assert!(validate_user_title("tab\tbetween").is_ok());
}

mod committed_snapshot_tests;
