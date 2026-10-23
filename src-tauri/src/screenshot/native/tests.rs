use super::*;

#[cfg(target_os = "windows")]
mod window_screenshot;

/// Build an absolute, multi-component path for the current target. The old
/// `r"C:\proj\.ac\..."` literals are ONE path component on Linux (a
/// backslash is an ordinary filename character there), so `find_replica_root`
/// walked to `None` regardless of content: one test failed outright and three
/// passed vacuously, since every assertion in them is `is_none()` or `is_err()`.
fn abs_path(parts: &[&str]) -> PathBuf {
    #[cfg(target_os = "windows")]
    let mut p = PathBuf::from(r"C:\");
    #[cfg(not(target_os = "windows"))]
    let mut p = PathBuf::from("/");
    for part in parts {
        p.push(part);
    }
    p
}

#[tokio::test]
async fn live_capture_resolution_rejects_none_and_dormant_before_accepting_live() {
    let manager = crate::session::manager::SessionManager::new();
    // A real directory: whether `SessionManager::create_session` canonicalizes
    // its cwd on Linux is unknown from a source read, so a real path removes
    // the question.
    let cwd = tempfile::TempDir::new().expect("cwd fixture");
    let session = manager
        .create_session(
            "shell".to_string(),
            Vec::new(),
            cwd.path().to_string_lossy().into_owned(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("capture session fixture");
    let rows = manager.list_sessions().await;
    let none = crate::session::selection::SessionSelection::none_for_test();
    let dormant = crate::session::selection::SessionSelection::dormant_for_test(session.id, 7);
    let live = crate::session::selection::SessionSelection::live_for_test(session.id);

    assert_eq!(
        resolve_live_capture_session(&none, &rows).unwrap_err(),
        "No active session is selected in AgentsCommander"
    );
    assert_eq!(
        resolve_live_capture_session(&dormant, &rows).unwrap_err(),
        "No active session is selected in AgentsCommander"
    );
    assert_eq!(
        resolve_live_capture_session(&live, &rows).unwrap(),
        session.id
    );
}

#[test]
fn shortcut_conversion_maps_modifier_and_code() {
    let parsed = super::super::parse_screenshot_hotkey("Ctrl+Q").unwrap();
    let shortcut = parsed_to_shortcut(&parsed).unwrap();
    assert_eq!(shortcut.mods, Modifiers::CONTROL);
    assert_eq!(shortcut.key, Code::KeyQ);
}

#[test]
fn shortcut_conversion_maps_digit() {
    let parsed = super::super::parse_screenshot_hotkey("Ctrl+1").unwrap();
    let shortcut = parsed_to_shortcut(&parsed).unwrap();
    assert_eq!(shortcut.key, Code::Digit1);
}

#[test]
fn find_replica_root_accepts_replica_and_subdirs() {
    let replica = abs_path(&["proj", ".ac", "wg-6-team", "__agent_dev_rust"]);
    assert_eq!(
        find_replica_root(&replica).as_deref(),
        Some(replica.as_path())
    );

    let child = abs_path(&[
        "proj",
        ".ac",
        "wg-6-team",
        "__agent_dev_rust",
        "sub",
        "deeper",
    ]);
    assert_eq!(
        find_replica_root(&child).as_deref(),
        Some(replica.as_path())
    );
}

#[test]
fn find_replica_root_rejects_non_replica_paths() {
    // Ad-hoc CWD.
    assert!(find_replica_root(&abs_path(&["tmp"])).is_none());
    // repo-* workgroup sibling (parent is wg-*, but name is not __agent_*).
    assert!(find_replica_root(&abs_path(&[
        "proj",
        ".ac",
        "wg-6-team",
        "repo-AgentsCommander"
    ]))
    .is_none());
    // __agent_* whose parent is NOT a wg-* folder.
    assert!(find_replica_root(&abs_path(&["proj", "__agent_x"])).is_none());
    // Root-agent directory name.
    assert!(find_replica_root(&abs_path(&["proj", ".ac", "ac-root-agent"])).is_none());
}

#[test]
fn resolve_rejects_missing_and_non_replica() {
    // A missing path is rejected by `canonicalize`, which runs FIRST; the
    // stale comment here used to claim rejection "before any filesystem
    // access".
    let missing_non_replica = abs_path(&["definitely", "not", "a", "replica"]);
    assert!(resolve_session_replica_root(&missing_non_replica.to_string_lossy()).is_err());
    // Replica-shaped but missing on disk: canonicalize fails.
    let missing_name = format!("__agent_{}", Uuid::new_v4().simple());
    let missing = abs_path(&["proj", ".ac", "wg-6-team", missing_name.as_str()]);
    assert!(resolve_session_replica_root(&missing.to_string_lossy()).is_err());
    // A real, existing directory that is simply not a replica: rejected by
    // the ancestor walk, not by canonicalization. Without this, every
    // assertion above still passes on any input that fails to canonicalize.
    let real = tempfile::TempDir::new().unwrap();
    assert!(resolve_session_replica_root(&real.path().to_string_lossy()).is_err());
}

#[test]
fn resolve_rejects_regular_file() {
    // A file shaped like a replica root must be rejected (is_dir == false).
    let dir = std::env::temp_dir().join(format!("wg-6-team-{}", Uuid::new_v4().simple()));
    let replica = dir.join("__agent_filecase");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&replica, b"not a dir").unwrap();
    let result = resolve_session_replica_root(&replica.to_string_lossy());
    let _ = std::fs::remove_file(&replica);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err());
}

#[test]
fn resolve_rejects_dotdot_spoof_to_repo_sibling() {
    // A CWD that walks INTO the replica then back out via `..` into a `repo-*`
    // workgroup sibling must resolve (canonically) to the repo and be rejected,
    // not spoof the `__agent_*` ancestor via a raw path-name walk (Grinch #714
    // MEDIUM). Requires real dirs so `canonicalize` can resolve `..`.
    let base = std::env::temp_dir().join(format!("ac-shot-spoof-{}", Uuid::new_v4().simple()));
    let wg = base.join("wg-6-dev-team");
    let replica = wg.join("__agent_x");
    let repo = wg.join("repo-foo");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::create_dir_all(&repo).unwrap();

    let spoof = replica.join("..").join("repo-foo");
    let result = resolve_session_replica_root(&spoof.to_string_lossy());

    let _ = std::fs::remove_dir_all(&base);
    assert!(
        result.is_err(),
        "`..` spoof into a repo sibling must be rejected, got {result:?}"
    );
}

#[test]
fn resolve_rejects_missing_child_of_replica() {
    // The replica exists, but the session CWD points to a now-missing child.
    // With no existing path to canonicalize, resolution must fail rather than
    // fall back to the raw replica ancestor (Grinch #714 MEDIUM).
    let base = std::env::temp_dir().join(format!("ac-shot-missing-{}", Uuid::new_v4().simple()));
    let replica = base.join("wg-6-dev-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();

    let missing_child = replica.join("gone");
    let result = resolve_session_replica_root(&missing_child.to_string_lossy());

    let _ = std::fs::remove_dir_all(&base);
    assert!(result.is_err(), "missing child CWD must be rejected");
}

#[test]
fn revalidate_rejects_plain_recreated_dir() {
    // A directory shaped like a replica root but WITHOUT the WG replica config
    // / matrix (models delete + recreate-as-plain between capture and confirm)
    // must be rejected at confirm time by the re-run identity checks, even
    // though it passes the symlink/is_dir guard (Grinch #714 MEDIUM).
    let base = std::env::temp_dir().join(format!("ac-shot-reval-{}", Uuid::new_v4().simple()));
    let replica = base.join("wg-6-dev-team").join("__agent_x");
    std::fs::create_dir_all(&replica).unwrap();
    let canonical = std::fs::canonicalize(&replica).unwrap();

    let result = revalidate_replica_target(&canonical);

    let _ = std::fs::remove_dir_all(&base);
    assert!(
        result.is_err(),
        "plain (non-config) replica-shaped dir must be rejected at confirm"
    );
}

#[tokio::test]
async fn finishing_gate_blocks_then_releases() {
    // The `Finishing` lifecycle is non-Idle, so begin_capture's Idle gate
    // rejects a new capture (returns Busy) until confirm_selection destroys the
    // overlays and releases the gate (Grinch #714 HIGH finishing race).
    let id = Uuid::new_v4();
    let state: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
        ScreenshotCaptureLifecycle::Finishing(id),
    ));

    // Non-Idle while finishing => a new hotkey press would be Busy.
    assert!(!matches!(
        &*state.lock().await,
        ScreenshotCaptureLifecycle::Idle
    ));

    // A mismatched id must NOT release the gate.
    finish_capture_to_idle(&state, Uuid::new_v4()).await;
    assert!(matches!(
        &*state.lock().await,
        ScreenshotCaptureLifecycle::Finishing(_)
    ));

    // The matching id releases the gate to Idle.
    finish_capture_to_idle(&state, id).await;
    assert!(matches!(
        &*state.lock().await,
        ScreenshotCaptureLifecycle::Idle
    ));
}

#[tokio::test]
async fn finish_capture_to_idle_ignores_non_finishing() {
    // Defensive: releasing the gate never clobbers a Starting/Active state.
    let state: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
        ScreenshotCaptureLifecycle::Starting(ScreenshotCaptureStarting {
            id: Uuid::new_v4(),
            target_session_id: Uuid::new_v4(),
            target_session_name: "s".to_string(),
            target_directory: PathBuf::from("x"),
            created_at: Utc::now(),
        }),
    ));
    finish_capture_to_idle(&state, Uuid::new_v4()).await;
    assert!(matches!(
        &*state.lock().await,
        ScreenshotCaptureLifecycle::Starting(_)
    ));
}

#[tokio::test]
async fn busy_pregate_returns_busy_for_non_idle_before_resolution() {
    // Idle: no short-circuit, so begin_capture proceeds to active-session/CWD
    // resolution.
    let idle: ScreenshotCaptureState =
        std::sync::Arc::new(tokio::sync::Mutex::new(ScreenshotCaptureLifecycle::Idle));
    assert!(busy_pregate(&idle).await.is_none());

    // Every non-Idle state returns Busy carrying the capture this press may
    // cancel. Because busy_pregate takes ONLY the lifecycle state (no
    // AppHandle/SessionManager) it structurally cannot run the
    // active-session/CWD resolution begin_capture does afterwards, proving
    // the Busy no-op happens before any fallible resolution could surface an
    // Err and raise AC mid-capture (Grinch #714 HIGH pre-gate ordering).
    let starting_id = Uuid::new_v4();
    let starting: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
        ScreenshotCaptureLifecycle::Starting(ScreenshotCaptureStarting {
            id: starting_id,
            target_session_id: Uuid::new_v4(),
            target_session_name: "s".to_string(),
            target_directory: PathBuf::from("x"),
            created_at: Utc::now(),
        }),
    ));
    // The guard is what compares. A pattern-position `starting_id` would be a
    // FRESH binding shadowing the fixture, matching any Some(_) — the
    // assertion would pass for the wrong id and no probe would notice.
    assert!(matches!(
        busy_pregate(&starting).await,
        Some(BeginOutcome::Busy(Some(id))) if id == starting_id
    ));

    let active_id = Uuid::new_v4();
    let active: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
        ScreenshotCaptureLifecycle::Active(ActiveScreenshotCapture {
            id: active_id,
            target_session_id: Uuid::new_v4(),
            target_session_name: "s".to_string(),
            target_directory: PathBuf::from("x"),
            monitors: Vec::new(),
            overlay_labels: Vec::new(),
            created_at: Utc::now(),
        }),
    ));
    assert!(matches!(
        busy_pregate(&active).await,
        Some(BeginOutcome::Busy(Some(id))) if id == active_id
    ));

    let finishing_id = Uuid::new_v4();
    let finishing: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
        ScreenshotCaptureLifecycle::Finishing(finishing_id),
    ));
    // `None` is a literal pattern with no binding, so this one is sensitive
    // as written — it is the assertion P4 fails against.
    assert!(matches!(
        busy_pregate(&finishing).await,
        Some(BeginOutcome::Busy(None))
    ));
}

#[test]
fn hotkey_status_reflects_configured_shortcut_only() {
    // Success: the configured shortcut is active.
    let ok = hotkey_status_after_attempt("Ctrl+Q", Ok(()));
    assert_eq!(ok.configured, "Ctrl+Q");
    assert!(ok.registered);
    assert!(ok.error.is_none());

    // Failure (e.g. OS conflict): the configured shortcut is NOT active even if
    // an internal fallback stays registered. `registered` must be false and the
    // error present so the sidebar's `!registered && error` check warns that the
    // configured combo does nothing (Grinch #714 LOW).
    let err = hotkey_status_after_attempt("Ctrl+P", Err("hotkey already registered"));
    assert_eq!(err.configured, "Ctrl+P");
    assert!(!err.registered);
    assert_eq!(err.error.as_deref(), Some("hotkey already registered"));
}

#[test]
fn validate_selection_bounds() {
    let sel = |x, y, w, h| ScreenshotSelection {
        capture_id: "c".to_string(),
        monitor_id: 0,
        x,
        y,
        width: w,
        height: h,
    };
    // OK: inside bounds.
    assert!(validate_selection(&sel(0, 0, 10, 10), 100, 100).is_ok());
    assert!(validate_selection(&sel(90, 90, 10, 10), 100, 100).is_ok());
    // Too small.
    assert!(validate_selection(&sel(0, 0, 1, 10), 100, 100).is_err());
    assert!(validate_selection(&sel(0, 0, 10, 1), 100, 100).is_err());
    // Out of bounds.
    assert!(validate_selection(&sel(95, 0, 10, 10), 100, 100).is_err());
    assert!(validate_selection(&sel(0, 95, 10, 10), 100, 100).is_err());
}

#[test]
fn crop_returns_requested_dimensions_and_pixels() {
    // 4x4 image; top-left 2x2 is red, rest is blue. Crop (0,0,2,2) -> all red.
    let mut img = image::RgbaImage::new(4, 4);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = if x < 2 && y < 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        };
    }
    let crop = image::imageops::crop_imm(&img, 0, 0, 2, 2).to_image();
    assert_eq!(crop.width(), 2);
    assert_eq!(crop.height(), 2);
    assert_eq!(*crop.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
    assert_eq!(*crop.get_pixel(1, 1), image::Rgba([255, 0, 0, 255]));

    // Crop the bottom-right 2x2 -> all blue, and origin must be respected.
    let crop_br = image::imageops::crop_imm(&img, 2, 2, 2, 2).to_image();
    assert_eq!(*crop_br.get_pixel(0, 0), image::Rgba([0, 0, 255, 255]));
}

#[test]
fn create_png_file_never_overwrites() {
    let dir = std::env::temp_dir().join(format!("ac-shot-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = prepare_screenshots_directory(&std::fs::canonicalize(&dir).unwrap()).unwrap();

    let (p0, f0) = create_png_file(&dir, "shot.png").unwrap();
    drop(f0);
    std::fs::write(&p0, b"original").unwrap();
    assert!(p0.ends_with("shot.png"));

    let (p1, f1) = create_png_file(&dir, "shot.png").unwrap();
    drop(f1);
    assert!(p1.ends_with("shot-1.png"), "got {p1:?}");
    assert_ne!(p0, p1);
    assert!(p0.exists() && p1.exists());
    assert_eq!(std::fs::read(&p0).unwrap(), b"original");

    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

fn screenshot_replica_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let ac = temp.path().join("project").join(".ac");
    let matrix = ac.join("_agent_shot");
    let replica = ac.join("room-6-test").join("__agent_shot");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::write(matrix.join("Role.md"), "# Shot\n").unwrap();
    std::fs::create_dir_all(replica.join("nested")).unwrap();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_shot"}"#,
    )
    .unwrap();
    let root = resolve_session_replica_root(&replica.join("nested").to_string_lossy()).unwrap();
    revalidate_replica_target(&root).unwrap();
    (temp, root)
}

fn synthetic_selection_capture(root: &Path) -> (ActiveScreenshotCapture, ScreenshotSelection) {
    let id = Uuid::new_v4();
    let image = image::RgbaImage::from_fn(4, 4, |x, y| image::Rgba([x as u8, y as u8, 127, 255]));
    (
        ActiveScreenshotCapture {
            id,
            target_session_id: id,
            target_session_name: "synthetic".to_string(),
            target_directory: root.to_path_buf(),
            monitors: vec![CapturedMonitor {
                monitor_id: 0,
                name: "synthetic".to_string(),
                x: 0,
                y: 0,
                width: 4,
                height: 4,
                scale_factor: 1.0,
                image,
                preview_data_url: String::new(),
            }],
            overlay_labels: vec![],
            created_at: Utc::now(),
        },
        ScreenshotSelection {
            capture_id: id.to_string(),
            monitor_id: 0,
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        },
    )
}

#[tokio::test]
async fn save_selection_creates_and_reuses_screenshots_child() {
    let (_temp, root) = screenshot_replica_fixture();
    let child = root.join("screenshots");
    assert!(!child.exists());
    let mut paths = Vec::new();
    for _ in 0..2 {
        let (capture, selection) = synthetic_selection_capture(&root);
        let session_id = capture.target_session_id.to_string();
        let result = save_selection(capture, &selection).await.unwrap();
        let path = std::fs::canonicalize(&result.path).unwrap();
        assert_eq!(path.parent(), Some(child.as_path()));
        assert_eq!(
            result.path,
            crate::path_utils::path_to_string_without_windows_verbatim_prefix(&path)
        );
        assert_eq!(result.session_id, session_id);
        assert_eq!(result.session_name, "synthetic");
        let decoded = image::open(&path).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 2));
        for (x, y, pixel) in decoded.enumerate_pixels() {
            assert_eq!(
                *pixel,
                image::Rgba([(x + 1) as u8, (y + 1) as u8, 127, 255])
            );
        }
        paths.push(path);
    }
    assert_ne!(paths[0], paths[1]);
    assert_eq!(std::fs::read_dir(&child).unwrap().count(), 2);
    assert!(std::fs::read_dir(&root).unwrap().all(|entry| entry
        .unwrap()
        .path()
        .extension()
        .is_none_or(|ext| ext != "png")));
}

#[tokio::test]
async fn save_selection_rejects_file_named_screenshots() {
    let (_temp, root) = screenshot_replica_fixture();
    let child = root.join("screenshots");
    std::fs::write(&child, b"sentinel").unwrap();
    let before = std::fs::read_dir(&root).unwrap().count();
    let (capture, selection) = synthetic_selection_capture(&root);
    let (partial, error) = save_selection(capture, &selection).await.unwrap_err();
    assert!(partial.is_none());
    assert!(error.contains("not a real directory"), "{error}");
    assert_eq!(std::fs::read(&child).unwrap(), b"sentinel");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), before);
}

#[test]
fn overlay_label_and_hex_round_trip() {
    let id = Uuid::new_v4();
    let label = overlay_label(id, 3);
    assert!(label.starts_with("screenshot-overlay-"));
    assert_eq!(
        capture_hex_from_overlay_label(&label),
        Some(id.simple().to_string().as_str())
    );
    assert!(capture_hex_from_overlay_label("terminal-abc").is_none());
    assert!(capture_hex_from_overlay_label("screenshot-overlay-xyz-0").is_none());
}

#[test]
fn build_filename_shape() {
    let id = Uuid::new_v4();
    let name = build_filename(id);
    assert!(name.starts_with("agentscommander-screenshot-"));
    assert!(name.ends_with(".png"));
    // Contains the 8-char short id.
    assert!(name.contains(&id.simple().to_string()[..8]));
}

#[test]
fn overlay_window_traits_match_target() {
    // Guards the divergence both ways: Windows must never gain fullscreen,
    // Linux must never lose `resizable(true)` (which silently blocks
    // fullscreen by setting min == max size hints while Tauri returns Ok).
    let expected = OverlayWindowTraits {
        resizable: cfg!(target_os = "linux"),
        fullscreen: cfg!(target_os = "linux"),
    };
    assert_eq!(overlay_window_traits(), expected);
}

/// Frames are f64; compare with a tolerance far below the 1 pt matching slack.
fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 0.01,
        "{what}: expected ~{expected}, got {actual}"
    );
}

/// The internal display in macOS "More Space": xcap reports the mode pixels
/// and a derived 1.714 scale, tao reports 2.0. Only points agree.
fn more_space_captured() -> MonitorGeometry {
    MonitorGeometry {
        name: Some("Display #123".to_string()),
        position: (0, 0),
        size: (2880, 1800),
        scale: 2880.0 / 1680.0,
    }
}

fn more_space_tauri() -> MonitorGeometry {
    MonitorGeometry {
        name: Some("Monitor #123".to_string()),
        position: (0, 0),
        size: (3360, 2100),
        scale: 2.0,
    }
}

#[test]
fn xcap_logical_frame_divides_bitmap_by_xcap_scale() {
    let f = xcap_logical_frame(&more_space_captured());
    // The origin is already points on macOS (`CGDisplayBounds`).
    assert_close(f.x, 0.0, "x");
    assert_close(f.y, 0.0, "y");
    assert_close(f.width, 1680.0, "width");
    assert_close(f.height, 1050.0, "height");
    // A zero scale must not produce infinities.
    let zero = MonitorGeometry {
        scale: 0.0,
        ..more_space_captured()
    };
    assert_close(xcap_logical_frame(&zero).width, 2880.0, "zero-scale width");
}

#[test]
fn match_monitor_pairs_more_space_display_by_logical_frame() {
    // 2880/1.714 = 1680 pt and 3360/2.0 = 1680 pt: the same monitor, which
    // no physical-pixel comparison could tell.
    let tauri = [more_space_tauri()];
    assert_eq!(
        match_monitor(&tauri, &more_space_captured(), PlacementFrame::Logical),
        Some(0)
    );
}

#[test]
fn match_monitor_pairs_mixed_scale_pair() {
    // M6: internal "More Space" at the origin plus a 1x external to its
    // right. Both xcap names are identical, so the pairing rests entirely on
    // the logical frame. The tao list is reversed so that index order alone
    // cannot produce the right answer.
    let external_captured = MonitorGeometry {
        name: Some("Display #123".to_string()),
        position: (1680, 0),
        size: (1920, 1080),
        scale: 1.0,
    };
    let external_tauri = MonitorGeometry {
        name: Some("Monitor #123".to_string()),
        position: (1680, 0),
        size: (1920, 1080),
        scale: 1.0,
    };
    let tauri = [external_tauri, more_space_tauri()];
    assert_eq!(
        match_monitor(&tauri, &more_space_captured(), PlacementFrame::Logical),
        Some(1)
    );
    assert_eq!(
        match_monitor(&tauri, &external_captured, PlacementFrame::Logical),
        Some(0)
    );
}

#[test]
fn match_monitor_pairs_identical_twins_by_origin() {
    // Same model, same size: the name criterion could never separate these.
    let twin = |x: i32| MonitorGeometry {
        name: Some("Display #7".to_string()),
        position: (x, 0),
        size: (1920, 1080),
        scale: 1.0,
    };
    let tauri = [twin(0), twin(1920)];
    assert_eq!(
        match_monitor(&tauri, &twin(0), PlacementFrame::Logical),
        Some(0)
    );
    assert_eq!(
        match_monitor(&tauri, &twin(1920), PlacementFrame::Logical),
        Some(1)
    );
}

#[test]
fn match_monitor_falls_back_to_nearest_origin() {
    // Nothing inside tolerance (the sizes never agree), so the pre-#2173
    // behaviour stands: nearest origin wins.
    let at = |x: i32| MonitorGeometry {
        name: Some("Monitor #1".to_string()),
        position: (x, 0),
        size: (1920, 1080),
        scale: 1.0,
    };
    let tauri = [at(0), at(1920)];
    let stray = MonitorGeometry {
        name: Some("Display #1".to_string()),
        position: (5000, 0),
        size: (800, 600),
        scale: 1.0,
    };
    assert_eq!(
        match_monitor(&tauri, &stray, PlacementFrame::Logical),
        Some(1)
    );
}

#[test]
fn match_monitor_returns_none_for_empty_list() {
    assert_eq!(
        match_monitor(&[], &more_space_captured(), PlacementFrame::Logical),
        None
    );
    assert_eq!(
        match_monitor(&[], &more_space_captured(), PlacementFrame::Physical),
        None
    );
}

#[test]
fn build_placement_logical_frame_uses_tauri_points() {
    let tauri = more_space_tauri();
    let g = build_placement(
        Some(&tauri),
        &more_space_captured(),
        PlacementFrame::Logical,
    );
    assert_eq!(
        g,
        PlacementGeometry {
            x: 0,
            y: 0,
            width: 1680,
            height: 1050,
            scale_factor: 2.0,
            frame: PlacementFrame::Logical,
        }
    );
    // The regression this phase fixes: stating the bitmap as the overlay size
    // produced a 1440 pt window on a 1680 pt screen.
    assert_ne!((g.width, g.height), (2880, 1800));
}

#[test]
fn build_placement_physical_frame_unchanged() {
    struct Case {
        n: u32,
        tauri: Option<MonitorGeometry>,
        captured: MonitorGeometry,
        expected: PlacementGeometry,
    }
    let cases = [
        // Matched, negative origin: tao geometry verbatim.
        Case {
            n: 1,
            tauri: Some(MonitorGeometry {
                name: Some("Monitor #1".to_string()),
                position: (-1920, 0),
                size: (1920, 1080),
                scale: 1.0,
            }),
            captured: MonitorGeometry {
                name: Some("Monitor #1".to_string()),
                position: (-1920, 0),
                size: (1920, 1080),
                scale: 1.0,
            },
            expected: PlacementGeometry {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
                scale_factor: 1.0,
                frame: PlacementFrame::Physical,
            },
        },
        // Matched, tao size differs from the bitmap: tao still wins the
        // window size, the bitmap stays the crop authority.
        Case {
            n: 2,
            tauri: Some(MonitorGeometry {
                name: Some("Monitor #2".to_string()),
                position: (0, 0),
                size: (2560, 1440),
                scale: 1.5,
            }),
            captured: MonitorGeometry {
                name: Some("Monitor #2".to_string()),
                position: (0, 0),
                size: (1920, 1080),
                scale: 1.5,
            },
            expected: PlacementGeometry {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
                scale_factor: 1.5,
                frame: PlacementFrame::Physical,
            },
        },
        // Unmatched: the xcap geometry verbatim, origin included.
        Case {
            n: 3,
            tauri: None,
            captured: MonitorGeometry {
                name: Some("Monitor #3".to_string()),
                position: (3000, -100),
                size: (1280, 720),
                scale: 2.0,
            },
            expected: PlacementGeometry {
                x: 3000,
                y: -100,
                width: 1280,
                height: 720,
                scale_factor: 2.0,
                frame: PlacementFrame::Physical,
            },
        },
    ];
    for case in cases {
        let got = build_placement(
            case.tauri.as_ref(),
            &case.captured,
            PlacementFrame::Physical,
        );
        assert_eq!(got, case.expected, "case {}", case.n);
    }
    // The Physical frame also keeps name matching, which macOS cannot use.
    let tauri = [
        MonitorGeometry {
            name: Some("Monitor #9".to_string()),
            position: (4000, 4000),
            size: (1920, 1080),
            scale: 1.0,
        },
        MonitorGeometry {
            name: Some("Monitor #8".to_string()),
            position: (0, 0),
            size: (1920, 1080),
            scale: 1.0,
        },
    ];
    let captured = MonitorGeometry {
        name: Some("Monitor #9".to_string()),
        position: (0, 0),
        size: (1920, 1080),
        scale: 1.0,
    };
    // Name beats the nearer origin at index 1.
    assert_eq!(
        match_monitor(&tauri, &captured, PlacementFrame::Physical),
        Some(0)
    );
}

#[test]
fn placement_frame_matches_target() {
    // macOS must be Logical: its two sources disagree on the physical scale.
    let expected = if cfg!(target_os = "macos") {
        PlacementFrame::Logical
    } else {
        PlacementFrame::Physical
    };
    assert_eq!(placement_frame(), expected);
}

/// Link proof for the CoreGraphics gate; `CGRequestScreenCaptureAccess` is
/// never called from tests (it would prompt).
#[cfg(target_os = "macos")]
#[test]
fn macos_screen_recording_gate() {
    assert!(MACOS_SCREEN_RECORDING_REQUIRED.contains("Screen Recording"));
    assert!(MACOS_SCREEN_RECORDING_REQUIRED.contains("System Settings"));
    let _ = unsafe { CGPreflightScreenCaptureAccess() };
}

#[cfg(target_os = "linux")]
#[test]
fn linux_display_server_classification() {
    struct Case {
        n: u32,
        xdg_session_type: Option<&'static str>,
        wayland_socket: Option<&'static str>,
        wayland_display: Option<&'static str>,
        default_socket_present: bool,
        expected: LinuxDisplayServer,
    }
    let cases = [
        Case {
            n: 1,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        Case {
            n: 2,
            xdg_session_type: Some("wayland"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 3,
            xdg_session_type: Some("Wayland"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 4,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: Some("wayland-0"),
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        Case {
            n: 5,
            xdg_session_type: Some("wayland"),
            wayland_socket: None,
            wayland_display: Some("wayland-0"),
            default_socket_present: true,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 6,
            xdg_session_type: None,
            wayland_socket: None,
            wayland_display: Some("wayland-0"),
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 7,
            xdg_session_type: Some(""),
            wayland_socket: Some(""),
            wayland_display: Some(""),
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        Case {
            n: 8,
            xdg_session_type: None,
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        // The measured E5a host, kept as its own row so a change to the
        // host's shape is visible, not inferred from row 1.
        Case {
            n: 9,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        // B4 row A: an inherited Wayland fd via WAYLAND_SOCKET, nothing else set.
        Case {
            n: 10,
            xdg_session_type: None,
            wayland_socket: Some("7"),
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        // B4 row B: wl_display_connect(NULL)'s live wayland-0 fallback.
        Case {
            n: 11,
            xdg_session_type: None,
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: true,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 12,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: true,
            expected: LinuxDisplayServer::X11,
        },
        // E row: WAYLAND_SOCKET must parse as a descriptor number.
        Case {
            n: 13,
            xdg_session_type: None,
            wayland_socket: Some("abc"),
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
    ];

    // Accumulate and assert once: a panic on the first mismatched row hides
    // every later row, and the §7 probes must name *all* the rows they kill.
    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let env = DisplayEnvSnapshot {
            xdg_session_type: case.xdg_session_type.map(str::to_string),
            wayland_display: case.wayland_display.map(str::to_string),
            wayland_socket: case.wayland_socket.map(str::to_string),
            default_socket_present: case.default_socket_present,
        };
        let got = classify_display_server(&env);
        if got != case.expected {
            failures.push(format!(
                "row {}: {env:?} -> {got:?}, expected {:?}",
                case.n, case.expected
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} row(s) misclassified:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_wayland_refusal_status_is_unregistered_with_reason() {
    let status = hotkey_status_after_attempt("Ctrl+Q", Err(WAYLAND_UNSUPPORTED));
    assert_eq!(status.configured, "Ctrl+Q");
    assert!(!status.registered);
    assert_eq!(status.error.as_deref(), Some(WAYLAND_UNSUPPORTED));
    // The sidebar toasts on `!registered && error`, and the wording must
    // stay actionable for a user who is already on Xorg.
    assert!(WAYLAND_UNSUPPORTED.contains("Xorg"));
}
