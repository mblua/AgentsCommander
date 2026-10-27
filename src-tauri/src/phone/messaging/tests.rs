const ARCHIVE_NAME: &str = "20261017-000000-room1-a-to-room1-b-snapshot.md";

#[test]
fn issue_3081_bytes_identity_conflict_and_room_removal() {
    let temp = tempfile::tempdir().unwrap();
    let ac = temp.path().join(".ac");
    let room = ac.join("room-1-team");
    std::fs::create_dir_all(&room).unwrap();
    let source = room.join(ARCHIVE_NAME);
    let bytes = b"\xef\xbb\xbf# bytes\r\n\x00\xff";
    std::fs::write(&source, bytes).unwrap();
    let source = std::fs::canonicalize(source).unwrap();
    let archived = super::archive_message_snapshot(&ac, &source, ARCHIVE_NAME, bytes).unwrap();
    assert_eq!(
        archived,
        std::fs::canonicalize(&ac)
            .unwrap()
            .join("message-archive/20261017")
            .join(ARCHIVE_NAME)
    );
    assert_eq!(std::fs::read(&archived).unwrap(), bytes);
    assert_eq!(
        super::archive_message_snapshot(&ac, &source, ARCHIVE_NAME, bytes).unwrap(),
        archived
    );
    std::fs::write(&source, b"edited").unwrap();
    assert!(
        super::archive_message_snapshot(&ac, &source, ARCHIVE_NAME, b"edited")
            .unwrap_err()
            .contains("conflict")
    );
    let other = room.join("other").join(ARCHIVE_NAME);
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, bytes).unwrap();
    let other = std::fs::canonicalize(other).unwrap();
    assert_eq!(
        super::archive_message_snapshot(&ac, &other, ARCHIVE_NAME, bytes).unwrap(),
        archived
    );
    std::fs::write(&other, b"other").unwrap();
    assert!(
        super::archive_message_snapshot(&ac, &other, ARCHIVE_NAME, b"other")
            .unwrap_err()
            .contains("conflict")
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"edited");
    assert_eq!(std::fs::read(&other).unwrap(), b"other");
    std::fs::remove_dir_all(&room).unwrap();
    assert_eq!(std::fs::read(archived).unwrap(), bytes);
}

#[test]
fn issue_3120_daily_layout_preserves_names_and_legacy_entries() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let archive = root.join("message-archive");
    let legacy = archive.join("a".repeat(64));
    let day = archive.join("20261017");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(legacy.join(ARCHIVE_NAME), b"legacy").unwrap();
    let sibling = "20261017-000000-room1-a-to-room1-b-existing.md";
    std::fs::write(day.join(sibling), b"existing").unwrap();
    let names = [
        ARCHIVE_NAME,
        "20261017-000000-room1-a-to-room1-b-snapshot.1.md",
        "20261018-000000-room1-a-to-room1-b-snapshot.md",
    ];
    for name in names {
        let source = root.join(name);
        std::fs::write(&source, name.as_bytes()).unwrap();
        let result =
            super::archive_message_snapshot(&root, &source, name, name.as_bytes()).unwrap();
        assert_eq!(result, archive.join(&name[..8]).join(name));
        assert_eq!(std::fs::read(&result).unwrap(), name.as_bytes());
        assert_eq!(std::fs::read(&source).unwrap(), name.as_bytes());
    }
    let entries = |dir: &std::path::Path| {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    let mut expected = vec![
        ARCHIVE_NAME.to_string(),
        names[1].to_string(),
        sibling.to_string(),
    ];
    expected.sort();
    assert_eq!(entries(&day), expected);
    assert_eq!(entries(&archive.join("20261018")), vec![names[2]]);
    assert_eq!(entries(&legacy), vec![ARCHIVE_NAME]);
    assert_eq!(std::fs::read(legacy.join(ARCHIVE_NAME)).unwrap(), b"legacy");
    assert_eq!(std::fs::read(day.join(sibling)).unwrap(), b"existing");
    assert_eq!(
        entries(&archive),
        vec![
            "20261017".to_string(),
            "20261018".to_string(),
            "a".repeat(64)
        ]
    );
    let invalid_root = root.join("invalid");
    std::fs::create_dir(&invalid_root).unwrap();
    assert!(super::archive_message_snapshot(
        &invalid_root,
        &root.join(ARCHIVE_NAME),
        "invalid.md",
        b"invalid"
    )
    .is_err());
    assert!(entries(&invalid_root).is_empty());
}

#[test]
fn issue_3081_failures_clean_only_owned_temporary_and_keep_source() {
    for stage in ["write", "flush", "link", "cleanup"] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join(ARCHIVE_NAME);
        std::fs::write(&source, b"source").unwrap();
        let source = std::fs::canonicalize(source).unwrap();
        let mut destination = None;
        let error = super::archive_message_snapshot_with(
            temp.path(),
            &source,
            ARCHIVE_NAME,
            b"snapshot",
            |at, path| {
                destination = Some(path.to_path_buf());
                if at == stage {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        format!("injected {stage}"),
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(error.contains(stage));
        let destination = destination.unwrap();
        assert_eq!(std::fs::read(source).unwrap(), b"source");
        let entries: Vec<_> = std::fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        if stage == "cleanup" {
            assert_eq!(std::fs::read(destination).unwrap(), b"snapshot");
            assert_eq!(entries.len(), 2);
        } else {
            assert!(!destination.exists());
            assert!(entries.is_empty());
        }
    }
}

#[test]
fn issue_3081_concurrent_publication_is_complete_and_never_overwrites() {
    for same in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join(ARCHIVE_NAME);
        std::fs::write(&source, b"source").unwrap();
        let source = std::fs::canonicalize(source).unwrap();
        let other = temp.path().join("other").join(ARCHIVE_NAME);
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"other source").unwrap();
        let sources = [source, std::fs::canonicalize(other).unwrap()];
        let barrier = std::sync::Barrier::new(2);
        let snapshots = [vec![b'a'; 8192], vec![if same { b'a' } else { b'b' }; 8192]];
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = snapshots
                .iter()
                .zip(&sources)
                .map(|(bytes, source)| {
                    let barrier = &barrier;
                    let root = temp.path();
                    scope.spawn(move || {
                        super::archive_message_snapshot_with(
                            root,
                            source,
                            ARCHIVE_NAME,
                            bytes,
                            |stage, final_path| {
                                if stage == "link" {
                                    assert!(
                                        !final_path.exists(),
                                        "no final is visible while both writers are paused"
                                    );
                                    barrier.wait();
                                }
                                Ok(())
                            },
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            if same { 2 } else { 1 }
        );
        if !same {
            assert!(results
                .iter()
                .find_map(|r| r.as_ref().err())
                .unwrap()
                .contains("conflict"));
        }
        assert_eq!(std::fs::read(&sources[0]).unwrap(), b"source");
        assert_eq!(std::fs::read(&sources[1]).unwrap(), b"other source");
        let path = results.iter().find_map(|r| r.as_ref().ok()).unwrap();
        let bytes = std::fs::read(path).unwrap();
        assert!(snapshots.contains(&bytes));
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}

#[test]
fn issue_3081_nonregular_final_is_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join(ARCHIVE_NAME);
    std::fs::write(&source, b"source").unwrap();
    let source = std::fs::canonicalize(source).unwrap();
    let path =
        super::archive_message_snapshot(temp.path(), &source, ARCHIVE_NAME, b"source").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(
        super::archive_message_snapshot(temp.path(), &source, ARCHIVE_NAME, b"source")
            .unwrap_err()
            .contains("regular file")
    );
    assert!(path.is_dir());
}

#[test]
fn issue_3081_primary_error_and_foreign_temporary_survive_cleanup_failure() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join(ARCHIVE_NAME);
    std::fs::write(&source, b"source").unwrap();
    let source = std::fs::canonicalize(source).unwrap();
    let mut foreign = None;
    let error = super::archive_message_snapshot_with(
        temp.path(),
        &source,
        ARCHIVE_NAME,
        b"snapshot",
        |stage, final_path| {
            let path = final_path.with_file_name(".archive-foreign.tmp");
            if stage == "write" {
                std::fs::write(&path, b"foreign")?;
                foreign = Some(path);
                return Err(std::io::Error::other("primary write failure"));
            }
            if stage == "cleanup" {
                return Err(std::io::Error::other("cleanup failure"));
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("primary write failure"));
    assert!(error.contains("cleanup failure"));
    assert_eq!(std::fs::read(foreign.unwrap()).unwrap(), b"foreign");
    assert_eq!(std::fs::read(source).unwrap(), b"source");
}
use super::*;
use chrono::TimeZone;

fn unique_tmp(prefix: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    std::env::temp_dir().join(format!(
        "{}-{}-{}",
        prefix,
        Utc::now().timestamp_nanos_opt().unwrap_or(0),
        h.finish()
    ))
}

#[test]
fn agent_short_name_wg() {
    assert_eq!(agent_short_name("wg-7-dev-team/architect"), "wg7-architect");
    assert_eq!(agent_short_name("wg-12-foo-bar/dev-rust"), "wg12-dev-rust");
}

#[test]
fn agent_short_name_non_wg() {
    assert_eq!(agent_short_name("repos/my-project"), "repos-my-project");
    assert_eq!(agent_short_name("Agents/Shipper"), "agents-shipper");
}

#[test]
fn agent_short_name_no_slash() {
    assert_eq!(agent_short_name("solo-name"), "solo-name");
}

#[test]
fn sanitize_slug_typical() {
    assert_eq!(
        sanitize_slug("Messaging Redesign!").unwrap(),
        "messaging-redesign"
    );
}

#[test]
fn sanitize_slug_empty_after_normalize() {
    assert!(matches!(
        sanitize_slug("   ---  "),
        Err(MessagingError::EmptySlug)
    ));
}

#[test]
fn sanitize_slug_truncates_at_max() {
    let input = "a".repeat(100);
    assert_eq!(sanitize_slug(&input).unwrap(), "a".repeat(50));
}

#[test]
fn workgroup_root_ok() {
    let p = Path::new("/tmp/wg-7-dev-team/__agent_architect");
    assert_eq!(
        workgroup_root(p).unwrap(),
        PathBuf::from("/tmp/wg-7-dev-team")
    );
}

#[test]
fn workgroup_root_ok_windows_style() {
    let p = Path::new(r"C:\foo\wg-42-team-x\__agent_a");
    let wg = workgroup_root(p).unwrap();
    assert_eq!(
        wg.file_name().and_then(|n| n.to_str()),
        Some("wg-42-team-x")
    );
}

#[test]
fn workgroup_root_missing() {
    let p = Path::new("/tmp/plain/agent");
    assert!(matches!(
        workgroup_root(p),
        Err(MessagingError::NoWorkgroup(_))
    ));
}

#[test]
fn build_filename_canonical() {
    let ts = Utc.with_ymd_and_hms(2026, 4, 18, 14, 30, 52).unwrap();
    assert_eq!(
        build_filename(ts, "wg7-lead", "wg7-arch", "redesign"),
        "20260418-143052-wg7-lead-to-wg7-arch-redesign.md"
    );
}

#[test]
fn validate_filename_shape_accepts_canonical() {
    assert!(validate_filename_shape("20260418-143052-wg7-lead-to-wg7-arch-redesign.md").is_ok());
}

#[test]
fn validate_filename_shape_accepts_collision_suffix() {
    assert!(validate_filename_shape("20260418-143052-wg7-lead-to-wg7-arch-redesign.1.md").is_ok());
    assert!(validate_filename_shape("20260418-143052-wg7-lead-to-wg7-arch-redesign.99.md").is_ok());
}

#[test]
fn validate_filename_shape_rejects_variants() {
    let cases = [
        "reply.md",
        "20260418-143052-noto.md",
        "20260418-143052-from-to-to.md", // slug missing
        "20260418-143052-from-to-to-slug.txt",
        "20260418-143052-FROM-to-TO-slug.md",     // uppercase
        "20260418-143052-from-to-to-slug.100.md", // 3-digit suffix
        "20260418-143052-from-to-to-slug.0.md",   // zero suffix
        "2026041-143052-from-to-to-slug.md",      // date too short
        "20260418-14305-from-to-to-slug.md",      // time too short
    ];
    for c in cases {
        assert!(
            validate_filename_shape(c).is_err(),
            "expected rejection for {:?}",
            c
        );
    }
}

#[test]
fn create_and_resolve_round_trip() {
    let tmp = std::env::temp_dir().join(format!(
        "ac-msg-test-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();

    let ts = Utc.with_ymd_and_hms(2026, 4, 18, 14, 30, 52).unwrap();
    let base = build_filename(ts, "wg7-a", "wg7-b", "rt");

    let (p1, f1) = create_message_file(&tmp, &base).unwrap();
    drop(f1);
    assert_eq!(p1.file_name().and_then(|n| n.to_str()).unwrap(), base);

    let (p2, f2) = create_message_file(&tmp, &base).unwrap();
    drop(f2);
    assert_eq!(
        p2.file_name().and_then(|n| n.to_str()).unwrap(),
        "20260418-143052-wg7-a-to-wg7-b-rt.1.md"
    );

    let (p3, f3) = create_message_file(&tmp, &base).unwrap();
    drop(f3);
    assert_eq!(
        p3.file_name().and_then(|n| n.to_str()).unwrap(),
        "20260418-143052-wg7-a-to-wg7-b-rt.2.md"
    );

    let abs = resolve_existing_message(&tmp, &base).unwrap();
    assert!(abs.is_absolute());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn resolve_rejects_traversal() {
    let tmp = std::env::temp_dir().join(format!(
        "ac-msg-trav-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();

    for bad in [
        "../etc/passwd",
        "foo/bar.md",
        r"foo\bar.md",
        "foo.txt",
        "bare.md",
        "..",
    ] {
        assert!(
            resolve_existing_message(&tmp, bad).is_err(),
            "expected rejection for {:?}",
            bad
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn resolve_rejects_abs_path_inside_messaging_dir() {
    let tmp = unique_tmp("ac-msg-abs-ok");
    std::fs::create_dir_all(&tmp).unwrap();

    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
    let base = build_filename(ts, "wg7-a", "wg7-b", "abs-ok");
    let (written_abs, f) = create_message_file(&tmp, &base).unwrap();
    drop(f);

    let abs_str = written_abs.to_string_lossy().to_string();
    assert!(matches!(
        resolve_existing_message(&tmp, &abs_str),
        Err(MessagingError::InvalidFilename(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn resolve_rejects_abs_path_outside_messaging_dir() {
    let tmp_msg = unique_tmp("ac-msg-abs-out");
    let tmp_other = unique_tmp("ac-msg-abs-other");
    std::fs::create_dir_all(&tmp_msg).unwrap();
    std::fs::create_dir_all(&tmp_other).unwrap();

    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
    let base = build_filename(ts, "wg7-a", "wg7-b", "other");
    let bad_path = tmp_other.join(&base);
    std::fs::write(&bad_path, b"x").unwrap();

    let bad_abs = bad_path.to_string_lossy().to_string();
    assert!(matches!(
        resolve_existing_message(&tmp_msg, &bad_abs),
        Err(MessagingError::InvalidFilename(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp_msg);
    let _ = std::fs::remove_dir_all(&tmp_other);
}

#[test]
fn resolve_rejects_abs_path_with_dotdot() {
    let tmp_msg = unique_tmp("ac-msg-dd");
    std::fs::create_dir_all(&tmp_msg).unwrap();

    let sneaky = format!(
        "{}/../{}/foo.md",
        tmp_msg.display(),
        tmp_msg.file_name().and_then(|n| n.to_str()).unwrap()
    );
    assert!(matches!(
        resolve_existing_message(&tmp_msg, &sneaky),
        Err(MessagingError::InvalidFilename(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp_msg);
}

#[test]
fn resolve_rejects_abs_path_with_missing_parent() {
    let tmp_msg = unique_tmp("ac-msg-missing");
    std::fs::create_dir_all(&tmp_msg).unwrap();

    let bogus = std::env::temp_dir()
        .join("ac-no-such-dir-xyz-unlikely")
        .join("20260419-143052-wg7-a-to-wg7-b-nope.md");
    let bogus_abs = bogus.to_string_lossy().to_string();
    assert!(matches!(
        resolve_existing_message(&tmp_msg, &bogus_abs),
        Err(MessagingError::InvalidFilename(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp_msg);
}

#[test]
fn resolve_rejects_relative_path_inside_messaging_dir() {
    let tmp = unique_tmp("ac-msg-rel");
    std::fs::create_dir_all(&tmp).unwrap();

    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 14, 30, 52).unwrap();
    let base = build_filename(ts, "wg7-a", "wg7-b", "rel-ok");
    let (abs_written, f) = create_message_file(&tmp, &base).unwrap();
    drop(f);

    let rel = format!("{}/./{}", tmp.display(), base);
    assert!(matches!(
        resolve_existing_message(&tmp, &rel),
        Err(MessagingError::InvalidFilename(_))
    ));
    let _ = abs_written;

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn validate_filename_only_rejects_windows_drive_prefix() {
    let bad = r"C:\tmp\20260419-143052-wg7-a-to-wg7-b-nope.md";

    assert!(matches!(
        validate_filename_only(bad),
        Err(MessagingError::InvalidFilename(_))
    ));
}

#[test]
fn root_messaging_dir_creates_child_dir() {
    let tmp = unique_tmp("ac-root-msg-dir");
    std::fs::create_dir_all(&tmp).unwrap();

    let dir = root_messaging_dir(&tmp).unwrap();

    assert_eq!(dir, tmp.join(MESSAGING_DIR_NAME));
    assert!(dir.is_dir());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn root_filename_shape_is_accepted() {
    let name = build_filename(
        Utc.with_ymd_and_hms(2026, 5, 24, 4, 0, 0).unwrap(),
        crate::config::root_agent::ROOT_AGENT_SHORT_NAME,
        "wg1-tech-lead",
        "smoke",
    );

    assert_eq!(name, "20260524-040000-root-to-wg1-tech-lead-smoke.md");
    assert!(validate_filename_only(&name).is_ok());
}

#[test]
fn root_notification_filename_rejects_non_root_sender_shape() {
    let name = "20260524-040000-wg1-dev-rust-to-wg1-tech-lead-smoke.md";

    assert!(validate_root_notification_filename(name).is_err());
}

#[test]
fn file_notification_round_trips_path() {
    let path = r"C:\tmp\ac-root-agent\messaging\20260524-040000-root-to-wg1-tech-lead-smoke.md";
    let body = format_file_notification(path);

    assert!(body.contains("Process this inter-agent message:"));
    assert!(!body.contains("Read this file"));
    assert_eq!(parse_file_notification(&body), Some(path));
    assert_eq!(
        notification_filename(path),
        Some("20260524-040000-root-to-wg1-tech-lead-smoke.md")
    );
}

#[test]
fn legacy_file_notification_still_parses() {
    let path = r"C:\tmp\wg-1-dev-team\messaging\20260630-120000-wg1-a-to-wg1-b-smoke.md";
    let legacy = format!("New message: {}. Read this file.", path);

    assert_eq!(parse_file_notification(&legacy), Some(path));
}

#[test]
fn resolve_rejects_directory_with_md_suffix() {
    let tmp = std::env::temp_dir().join(format!(
        "ac-msg-dir-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();

    let name = "20260418-143052-wg7-a-to-wg7-b-dir.md";
    std::fs::create_dir_all(tmp.join(name)).unwrap();

    assert!(matches!(
        resolve_existing_message(&tmp, name),
        Err(MessagingError::NotAFile(_))
    ));

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Contract test: `format_pty_wrap("", "")` is the empty-placeholder
/// expansion used by both mailbox.rs injection sites. Its length MUST
/// equal `PTY_WRAP_FIXED`. If someone edits the `format!` string inside
/// `format_pty_wrap` (e.g. drops the trailing `\r`, adds a space), this
/// test fires before the clamp accounting can drift silently.
#[test]
fn format_pty_wrap_matches_pty_wrap_fixed() {
    assert_eq!(format_pty_wrap("", "").len(), PTY_WRAP_FIXED);
    assert_eq!(PTY_WRAP_FIXED, 19);
}

/// #2232 phase 7: the composed sender is the display-only attribution for a
/// Co-managed-origin wake. The exact FQN stays the authorization input and
/// is never merged with this string.
#[test]
fn comanaged_sender_appends_the_display_suffix_without_changing_the_fqn() {
    let fqn = "proj-a:room-1-dev-team/tech-lead";
    let composed = compose_sender_for_comanaged_origin(fqn);
    assert_eq!(composed, format!("{fqn}{CO_MANAGED_SENDER_SUFFIX}"));
    assert!(composed.starts_with(fqn));
    assert_eq!(CO_MANAGED_SENDER_SUFFIX, " (Co-managed)");
}

/// Contract test: the empty expansion of the `--get-output` marker wrap must
/// equal the plain wrap plus `PTY_RESPONSE_MARKER_FIXED`. Any edit to the
/// marker literal in `format_pty_wrap_with_markers` (kept byte-identical with
/// the `phone::mailbox` get-output injection) trips this before the retained
/// marker accounting (the inert `notification_pty_overhead(_, true)` branch in
/// `cli::send`, reserved for the future non-interactive path) can drift. The
/// live interactive wake clamp itself budgets the plain wrap only.
#[test]
fn format_pty_wrap_with_markers_overhead_matches_constant() {
    assert_eq!(
        format_pty_wrap_with_markers("", "", "").len(),
        PTY_WRAP_FIXED + PTY_RESPONSE_MARKER_FIXED
    );
}

/// Both the START and END markers carry the request id, so a non-empty id
/// adds `2 * id.len()` on top of `PTY_RESPONSE_MARKER_FIXED`.
#[test]
fn format_pty_wrap_with_markers_embeds_both_request_ids() {
    let rid = "0123456789abcdef0123456789abcdef0123"; // 36 chars (v4 length)
    let rendered = format_pty_wrap_with_markers("wg7-me", "hi", rid);
    assert_eq!(rendered.matches(rid).count(), 2);
    assert_eq!(
        rendered.len(),
        PTY_WRAP_FIXED + "wg7-me".len() + "hi".len() + PTY_RESPONSE_MARKER_FIXED + 2 * rid.len()
    );
    assert!(rendered.starts_with('\n'));
    assert!(rendered.ends_with("\n\r"));
    assert!(rendered.contains("%%AC_RESPONSE::"));
}

/// Structural round-trip: non-empty placeholders are rendered visibly.
#[test]
fn format_pty_wrap_round_trips_inputs() {
    let rendered = format_pty_wrap("wg7-me", "hello");
    assert!(rendered.contains("[Message from wg7-me] hello"));
    assert!(rendered.starts_with('\n'));
    assert!(rendered.ends_with("\n\r"));
}

/// PTY_SAFE_MAX clamp arithmetic — pathological long path must exceed
/// `PTY_WRAP_FIXED + from.len() + body.len() > PTY_SAFE_MAX=1024`.
#[test]
fn pty_safe_max_clamp_rejects_long_path() {
    let from = "wg99-extremely-long-agent-name-segment";
    // Synthetic body large enough to push the total past 1024.
    let body_len = FILE_NOTIFICATION_PREFIX.len() + FILE_NOTIFICATION_SUFFIX.len() + 1000;
    let overhead = PTY_WRAP_FIXED + from.len();
    assert!(
        body_len + overhead > PTY_SAFE_MAX,
        "expected clamp to fire for body={} overhead={} max={}",
        body_len,
        overhead,
        PTY_SAFE_MAX,
    );
}

/// Happy-path: realistic production body (155) + typical from (13) fits
/// comfortably under the 1024 clamp — the case that motivated this trim.
#[test]
fn pty_safe_max_clamp_accepts_typical() {
    let from = "wg7-architect";
    let body_len = 155;
    let overhead = PTY_WRAP_FIXED + from.len();
    assert!(
        body_len + overhead <= PTY_SAFE_MAX,
        "expected clamp to accept body={} overhead={} max={}",
        body_len,
        overhead,
        PTY_SAFE_MAX,
    );
}

#[test]
fn issue_3171_filename_grammar_matches_documented_edges() {
    let cases = [
        ("20261022-093000-root-to-room21-status.md", true),
        ("20261022-093000-root-to-room21-status.MD", false),
        ("20261022-093000-Root-to-room21-status.md", false),
        ("20261022-093000-root-to-room21--status.md", false),
        ("20261022-093000-root-room21-status.md", false),
        ("20261022-093000-root-to-room21.md", false),
        ("20261022-093000-root-to-room21-status.1.md", true),
        ("20261022-093000-root-to-room21-status.01.md", true),
        ("20261022-093000-root-to-room21-status.99.md", true),
        ("20261022-093000-root-to-room21-status.0.md", false),
        ("20261022-093000-root-to-room21-status.00.md", false),
        ("20261022-093000-root-to-room21-status.100.md", false),
        ("20261022-093000-root-to-room21-status.extra.md", false),
        ("2026102-093000-root-to-room21-status.md", false),
        ("202610222-093000-root-to-room21-status.md", false),
        ("20261022-09300-root-to-room21-status.md", false),
        ("20261022-0930000-root-to-room21-status.md", false),
        ("2026102x-093000-root-to-room21-status.md", false),
        ("２０２６１０２２-093000-root-to-room21-status.md", false),
        ("99999999-999999-root-to-room21-status.md", true),
        ("../20261022-093000-root-to-room21-status.md", false),
        ("dir/20261022-093000-root-to-room21-status.md", false),
        (r"dir\20261022-093000-root-to-room21-status.md", false),
        ("C:20261022-093000-root-to-room21-status.md", false),
        (r"C:\20261022-093000-root-to-room21-status.md", false),
    ];
    for (name, valid) in cases {
        assert_eq!(
            validate_filename_only(name).is_ok(),
            valid,
            "general: {name}"
        );
        assert_eq!(
            validate_root_notification_filename(name).is_ok(),
            valid,
            "Root: {name}"
        );
    }
    let long_slug = format!("20261022-093000-root-to-room21-{}.md", "a".repeat(51));
    assert!(validate_filename_only(&long_slug).is_ok());
    assert!(validate_root_notification_filename(&long_slug).is_ok());
}
