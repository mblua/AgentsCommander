use super::*;

#[test]
fn poll_partial_only_and_clean_seed_deliver_explicit_null_with_new_revision() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("TASK.md"), "  \n").unwrap();
    std::fs::write(root.join("TASK-status.jsonl"), "partial").unwrap();
    let partial = poll_task_snapshot(root, None).unwrap().unwrap();
    assert!(partial.task.is_none());
    assert!(partial.status.is_none());
    assert!(partial.tail_incomplete);
    assert_eq!(partial.revision, "legacy:0");
    std::fs::write(root.join("TASK.md"), "---\ntitle: 'Clean'\n---\n").unwrap();
    std::fs::write(
        root.join("TASK-status.jsonl"),
        serde_json::json!({
            "schemaVersion":1,"kind":"topic_started",
            "topicId":"12345678-1234-4234-8234-123456789abc","sequence":0,
            "requestId":null,"baseRevision":null,"recordedAt":"2026-10-05T18:00:00Z",
            "author":null,"status":null
        })
        .to_string()
            + "\n",
    )
    .unwrap();
    let clean = poll_task_snapshot(root, Some(partial.sentinel.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(clean.task_title.as_deref(), Some("Clean"));
    assert!(!clean.tail_incomplete);
    assert_eq!(clean.revision, "12345678-1234-4234-8234-123456789abc:0");
    let mut emitted = false;
    let clean = deliver_poll_task(Some(&partial), clean, |value| {
        emitted = true;
        assert!(value.status.is_none());
        Ok(())
    })
    .unwrap();
    assert!(emitted);
    std::fs::remove_file(root.join("TASK.md")).unwrap();
    let missing = poll_task_snapshot(root, Some(clean.sentinel))
        .unwrap()
        .unwrap();
    assert!(missing.task.is_none());
    assert!(missing.task_title.is_none());
}

// --- Project file difference (#3060) ---

/// Runs fixture Git with a local identity and no signing; global config is never written.
fn fixture_git(dir: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new("git");
    for var in PROJECT_GIT_REDIRECT_ENV {
        cmd.env_remove(var);
    }
    let status = cmd
        .args([
            "-c",
            "user.name=AC Test",
            "-c",
            "user.email=ac-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("spawn fixture git");
    assert!(status.success(), "fixture git {args:?} failed");
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

/// A committed Project (`.ac` included) whose `origin/main` is its base commit.
fn project_file_difference_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    write_file(&project.join("a.txt"), "a\n");
    write_file(&project.join("b.txt"), "b\n");
    write_file(&project.join(".ac").join("keep.md"), "keep\n");
    write_file(&project.join(".gitignore"), "ignored.txt\n.ac/room-*/\n");
    fixture_git(&project, &["init", "-q", "-b", "main"]);
    fixture_git(&project, &["add", "-A"]);
    fixture_git(&project, &["commit", "-q", "-m", "base"]);
    fixture_git(
        &project,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    (temp, project)
}

async fn project_file_difference_count(project: &Path) -> Option<u32> {
    get_project_file_difference_inner(project.to_str().expect("utf8 path"))
        .await
        .count
}

#[tokio::test]
async fn project_file_difference_clean_is_zero_and_tracked_change_is_one() {
    let (_temp, project) = project_file_difference_fixture();
    assert_eq!(project_file_difference_count(&project).await, Some(0));

    // Positive control: a constant zero or null counter fails here.
    write_file(&project.join("a.txt"), "changed\n");
    assert_eq!(project_file_difference_count(&project).await, Some(1));
}

#[tokio::test]
async fn project_file_difference_counts_each_kind_of_difference() {
    let (_temp, project) = project_file_difference_fixture();

    // committed-only: HEAD moved past origin/main, worktree equals HEAD
    write_file(&project.join("a.txt"), "committed\n");
    fixture_git(&project, &["commit", "-q", "-am", "ahead"]);
    assert_eq!(project_file_difference_count(&project).await, Some(1));

    // staged-only
    write_file(&project.join("b.txt"), "staged\n");
    fixture_git(&project, &["add", "b.txt"]);
    assert_eq!(project_file_difference_count(&project).await, Some(2));

    // deletion, then restoring it removes the difference again
    std::fs::remove_file(project.join(".gitignore")).expect("delete");
    assert_eq!(project_file_difference_count(&project).await, Some(3));
    write_file(&project.join(".gitignore"), "ignored.txt\n.ac/room-*/\n");
    assert_eq!(project_file_difference_count(&project).await, Some(2));

    // added to the index
    write_file(&project.join("added.txt"), "added\n");
    fixture_git(&project, &["add", "added.txt"]);
    assert_eq!(project_file_difference_count(&project).await, Some(3));

    // untracked
    write_file(&project.join("untracked.txt"), "new\n");
    assert_eq!(project_file_difference_count(&project).await, Some(4));

    // ignored does not count
    write_file(&project.join("ignored.txt"), "ignored\n");
    assert_eq!(project_file_difference_count(&project).await, Some(4));

    // versionable `.ac` content gets no special exclusion: one modified, one new
    write_file(&project.join(".ac").join("keep.md"), "changed\n");
    write_file(&project.join(".ac").join("new.md"), "new\n");
    assert_eq!(project_file_difference_count(&project).await, Some(6));
}

#[tokio::test]
async fn project_file_difference_unstaged_deleted_and_staged_plus_unstaged_count_once() {
    let (_temp, project) = project_file_difference_fixture();

    write_file(&project.join("a.txt"), "staged\n");
    fixture_git(&project, &["add", "a.txt"]);
    write_file(&project.join("a.txt"), "staged then unstaged\n");
    assert_eq!(project_file_difference_count(&project).await, Some(1));

    std::fs::remove_file(project.join("b.txt")).expect("delete");
    assert_eq!(project_file_difference_count(&project).await, Some(2));
}

#[tokio::test]
async fn project_file_difference_is_net_and_rename_is_two_paths() {
    let (_temp, project) = project_file_difference_fixture();

    // committed away and back again: worktree equals origin/main
    write_file(&project.join("a.txt"), "temporary\n");
    fixture_git(&project, &["commit", "-q", "-am", "temporary"]);
    write_file(&project.join("a.txt"), "a\n");
    assert_eq!(project_file_difference_count(&project).await, Some(0));

    fixture_git(&project, &["mv", "b.txt", "renamed.txt"]);
    assert_eq!(project_file_difference_count(&project).await, Some(2));
}

#[tokio::test]
async fn project_file_difference_counts_spaced_and_unicode_names() {
    let (_temp, project) = project_file_difference_fixture();
    write_file(
        &project.join("spaced name \u{f1} \u{fc} \u{6f22}.txt"),
        "x\n",
    );
    write_file(&project.join(" leading space.txt"), "x\n");
    assert_eq!(project_file_difference_count(&project).await, Some(2));
}

#[cfg(unix)]
#[tokio::test]
async fn project_file_difference_counts_newline_name_once() {
    let (_temp, project) = project_file_difference_fixture();
    write_file(&project.join("line\nbreak.txt"), "x\n");
    assert_eq!(project_file_difference_count(&project).await, Some(1));
}

#[tokio::test]
async fn project_file_difference_ignores_room_clones_but_counts_project() {
    let (_temp, project) = project_file_difference_fixture();
    let clone = project.join(".ac").join("room-01").join("repo-x");
    write_file(&clone.join("inner.txt"), "inner\n");
    fixture_git(&clone, &["init", "-q", "-b", "main"]);
    fixture_git(&clone, &["add", "-A"]);
    fixture_git(&clone, &["commit", "-q", "-m", "inner"]);
    write_file(&clone.join("inner.txt"), "inner changed\n");
    write_file(&clone.join("inner-new.txt"), "new\n");
    assert_eq!(project_file_difference_count(&project).await, Some(0));

    write_file(&project.join("a.txt"), "changed\n");
    assert_eq!(project_file_difference_count(&project).await, Some(1));
}

#[tokio::test]
async fn project_file_difference_is_unknown_without_origin_main() {
    let (_temp, project) = project_file_difference_fixture();
    fixture_git(&project, &["update-ref", "-d", "refs/remotes/origin/main"]);
    assert_eq!(project_file_difference_count(&project).await, None);
}

#[tokio::test]
async fn project_file_difference_is_unknown_for_non_project_and_nested_project() {
    let (temp, project) = project_file_difference_fixture();

    // a Project nested inside another repository never reports the parent's count
    write_file(&project.join("a.txt"), "changed\n");
    let nested = project.join("nested");
    std::fs::create_dir_all(nested.join(".ac")).expect("nested .ac");
    assert_eq!(project_file_difference_count(&nested).await, None);

    // a repository without `.ac` is not a Project
    std::fs::remove_dir_all(project.join(".ac")).expect("remove .ac");
    assert_eq!(project_file_difference_count(&project).await, None);

    // a `.ac` directory with no repository of its own
    let bare = temp.path().join("bare");
    std::fs::create_dir_all(bare.join(".ac")).expect("bare .ac");
    assert_eq!(project_file_difference_count(&bare).await, None);
}

/// Answers the reads like Git would for `project`, with `diff` scripted.
fn scripted_git(
    project: PathBuf,
    diff: &'static [u8],
) -> impl Fn(PathBuf, Vec<String>) -> std::future::Ready<Result<Vec<u8>, String>> {
    move |_cwd, args| {
        let out = match args.get(3).map(String::as_str) {
            Some("rev-parse") if args.iter().any(|a| a == "--show-toplevel") => {
                format!("{}\n", project.display()).into_bytes()
            }
            Some("rev-parse") => b"0123456789abcdef0123456789abcdef01234567\n".to_vec(),
            Some("diff") => diff.to_vec(),
            Some("ls-files") => Vec::new(),
            other => return std::future::ready(Err(format!("unexpected {other:?}"))),
        };
        std::future::ready(Ok(out))
    }
}

#[tokio::test]
async fn project_file_difference_scripted_reads_parse_or_are_unknown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).expect(".ac");
    let limit = Duration::from_secs(5);
    let project: &Path = &project;
    let read = move |diff: &'static [u8]| {
        read_project_file_difference(project, limit, scripted_git(project.to_path_buf(), diff))
    };

    // Sensitivity control for the scripted runner itself; duplicates count once.
    assert_eq!(read(b"a.txt\0b.txt\0a.txt\0").await.count, Some(2));
    assert_eq!(read(b"").await.count, Some(0));
    // not NUL-terminated, and an empty record, are malformed
    assert_eq!(read(b"a.txt").await.count, None);
    assert_eq!(read(b"a.txt\0\0").await.count, None);

    let failing = read_project_file_difference(project, limit, |_, _| {
        std::future::ready(Err::<Vec<u8>, String>("spawn failed".to_string()))
    })
    .await;
    assert_eq!(failing.count, None);
}

#[test]
fn project_file_difference_parser_keeps_raw_bytes() {
    let records = parse_nul_records(b"new\nline\0\xff\xfe\0 spaced \0").expect("parse");
    assert_eq!(
        records,
        vec![&b"new\nline"[..], &b"\xff\xfe"[..], &b" spaced "[..]]
    );
    assert_eq!(strip_line_terminator(b" path \r\n"), b" path ");
    assert_eq!(strip_line_terminator(b" path \n"), b" path ");
    assert_eq!(strip_line_terminator(b" path "), b" path ");
}

#[tokio::test]
async fn project_file_difference_timeout_is_unknown_and_bounded() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).expect(".ac");

    // The outer bound fails if the internal timeout is removed.
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        read_project_file_difference(&project, Duration::from_millis(50), |_, _| {
            std::future::pending::<Result<Vec<u8>, String>>()
        }),
    )
    .await
    .expect("internal timeout must bound a stalled git");
    assert_eq!(result.count, None);
}

#[test]
fn project_file_difference_serializes_count_contract() {
    let json_of = |count| serde_json::to_value(ProjectFileDifference { count }).unwrap();
    assert_eq!(json_of(Some(0)), serde_json::json!({ "count": 0 }));
    assert_eq!(json_of(None), serde_json::json!({ "count": null }));
    assert_eq!(json_of(Some(7)), serde_json::json!({ "count": 7 }));
}
