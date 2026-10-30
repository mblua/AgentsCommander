//! Confirm: crop, save, copy and emit the selection.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::screenshot::ScreenshotHost;

use super::{
    destroy_overlays_for_capture, is_link_or_reparse, revalidate_replica_target,
    ActiveScreenshotCapture, ScreenshotCaptureEvent, ScreenshotCaptureLifecycle,
    ScreenshotCaptureResult, ScreenshotCaptureState, ScreenshotSelection, MAX_COLLISION_SUFFIX,
};

// ── Confirm ────────────────────────────────────────────────────────────────

/// Crop the selection out of the stored monitor bitmap, save a PNG into the
/// resolved replica root, copy the path to the clipboard, and tear down overlays.
/// The async mutex is dropped before any file I/O. Any post-take failure destroys
/// overlays, deletes a partial PNG, and emits a failure event.
pub async fn confirm_selection<H: ScreenshotHost>(
    app: &H,
    state: &ScreenshotCaptureState,
    selection: ScreenshotSelection,
) -> Result<ScreenshotCaptureResult, String> {
    // Take the active capture iff it matches, and move to `Finishing` (NOT `Idle`)
    // so a new hotkey capture cannot start and photograph the still-visible
    // overlays while we save and tear down. The guard is dropped before file I/O.
    let capture = {
        let mut guard = state.lock().await;
        let is_match = matches!(
            &*guard,
            ScreenshotCaptureLifecycle::Active(c) if c.id.to_string() == selection.capture_id
        );
        if !is_match {
            return Err("Screenshot capture is no longer active".to_string());
        }
        let ScreenshotCaptureLifecycle::Active(c) =
            std::mem::replace(&mut *guard, ScreenshotCaptureLifecycle::Idle)
        else {
            unreachable!("matched Active above")
        };
        // Install the finishing gate keyed by this capture's id; the transient
        // `Idle` above is never observed because the lock is still held.
        *guard = ScreenshotCaptureLifecycle::Finishing(c.id);
        c
    };

    let capture_id = capture.id;
    let result = save_selection(capture, &selection).await;
    complete_confirmation(app, state, capture_id, result).await
}

pub(crate) async fn complete_confirmation<H: ScreenshotHost>(
    app: &H,
    state: &ScreenshotCaptureState,
    capture_id: Uuid,
    result: Result<ScreenshotCaptureResult, (Option<PathBuf>, String)>,
) -> Result<ScreenshotCaptureResult, String> {
    let outcome = match result {
        Ok(saved) => {
            // Best-effort clipboard copy; the PNG already exists, so a clipboard
            // failure must not fail the capture (plan §E).
            if let Err(e) = app.copy_path(saved.path.clone()) {
                log::warn!("[screenshot] clipboard copy failed: {e}");
            }
            app.capture_saved(saved.clone());
            destroy_overlays_for_capture(app, capture_id);
            Ok(saved)
        }
        Err((partial, msg)) => {
            if let Some(path) = partial {
                let _ = std::fs::remove_file(&path);
            }
            destroy_overlays_for_capture(app, capture_id);
            app.capture_failed(ScreenshotCaptureEvent {
                message: msg.clone(),
            });
            Err(msg)
        }
    };

    // Overlays are destroyed; release the finishing gate so the next hotkey press
    // can start a fresh capture. Runs on both success and failure.
    finish_capture_to_idle(state, capture_id).await;
    outcome
}

/// Release the `Finishing` gate installed by `confirm_selection` once its overlays
/// are destroyed, returning the lifecycle to `Idle`. Guarded by id so it never
/// clobbers a state some other path already changed.
pub(super) async fn finish_capture_to_idle(state: &ScreenshotCaptureState, capture_id: Uuid) {
    let mut guard = state.lock().await;
    if matches!(&*guard, ScreenshotCaptureLifecycle::Finishing(id) if *id == capture_id) {
        *guard = ScreenshotCaptureLifecycle::Idle;
    }
}

/// Validate the selection, crop, and write the PNG. On error the second tuple
/// element is the message and the first is a partial file to delete, if any.
pub(super) async fn save_selection(
    capture: ActiveScreenshotCapture,
    selection: &ScreenshotSelection,
) -> Result<ScreenshotCaptureResult, (Option<PathBuf>, String)> {
    let monitor = capture
        .monitors
        .into_iter()
        .find(|m| m.monitor_id == selection.monitor_id)
        .ok_or_else(|| {
            (
                None,
                format!("monitor {} not in capture", selection.monitor_id),
            )
        })?;

    let (iw, ih) = (monitor.image.width(), monitor.image.height());
    validate_selection(selection, iw, ih).map_err(|e| (None, e))?;

    let target = capture.target_directory.clone();
    let filename = build_filename(capture.target_session_id);
    let image = monitor.image;
    let (sx, sy, sw, sh) = (selection.x, selection.y, selection.width, selection.height);

    let saved_path =
        tokio::task::spawn_blocking(move || -> Result<PathBuf, (Option<PathBuf>, String)> {
            // Revalidate the destination immediately before creating the file.
            revalidate_replica_target(&target).map_err(|e| (None, e))?;
            let screenshots = prepare_screenshots_directory(&target).map_err(|e| (None, e))?;
            let (path, file) = create_png_file(&screenshots, &filename).map_err(|e| (None, e))?;

            let cropped = image::imageops::crop_imm(&image, sx, sy, sw, sh).to_image();
            let mut writer = std::io::BufWriter::new(file);
            if let Err(e) = image::DynamicImage::ImageRgba8(cropped)
                .write_to(&mut writer, image::ImageFormat::Png)
            {
                return Err((Some(path), format!("PNG encode/write failed: {e}")));
            }
            if let Err(e) = writer.flush() {
                return Err((Some(path), format!("PNG flush failed: {e}")));
            }
            drop(writer);

            // Recheck root identity and the real child after writing. These path-based
            // guards retain the existing non-atomic replacement limitation.
            revalidate_replica_target(&target).map_err(|e| (Some(path.clone()), e))?;
            let meta = std::fs::symlink_metadata(&screenshots).map_err(|e| {
                (
                    Some(path.clone()),
                    format!("Cannot read screenshots directory: {e}"),
                )
            })?;
            if !meta.is_dir() || is_link_or_reparse(&meta) {
                return Err((
                    Some(path),
                    "Screenshots path is not a real directory".to_string(),
                ));
            }
            let canonical_child = std::fs::canonicalize(&screenshots).map_err(|e| {
                (
                    Some(path.clone()),
                    format!("Cannot canonicalize screenshots directory: {e}"),
                )
            })?;
            if canonical_child != screenshots || canonical_child.parent() != Some(target.as_path())
            {
                return Err((
                    Some(path),
                    "Screenshots directory changed during save".to_string(),
                ));
            }
            let canonical = std::fs::canonicalize(&path).map_err(|e| {
                (
                    Some(path.clone()),
                    format!("canonicalize saved file failed: {e}"),
                )
            })?;
            let parent_ok = canonical
                .parent()
                .map(|p| p == canonical_child)
                .unwrap_or(false);
            if !parent_ok {
                return Err((
                    Some(path),
                    "saved file escaped the screenshots directory".to_string(),
                ));
            }
            Ok(canonical)
        })
        .await
        .map_err(|e| (None, format!("save task panicked: {e}")))??;

    // `saved_path` is canonical (verbatim `\\?\` on Windows). Every containment
    // check above ran on the raw canonical PathBuf; only the user-facing string
    // (clipboard, saved event, result payload, log) is normalized via the shared
    // #730 helper so it matches the clean paths the rest of the app now emits.
    let path_string =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&saved_path);
    log::info!(
        "[screenshot] saved {} for session '{}'",
        path_string,
        capture.target_session_name
    );
    Ok(ScreenshotCaptureResult {
        path: path_string,
        session_id: capture.target_session_id.to_string(),
        session_name: capture.target_session_name,
    })
}

/// Lazily create only the direct child of the already validated canonical root.
pub(super) fn prepare_screenshots_directory(root: &Path) -> Result<PathBuf, String> {
    let child = root.join("screenshots");
    match std::fs::create_dir(&child) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("Cannot create screenshots directory: {e}")),
    }
    let meta = std::fs::symlink_metadata(&child)
        .map_err(|e| format!("Cannot read screenshots directory: {e}"))?;
    if !meta.is_dir() || is_link_or_reparse(&meta) {
        return Err("Screenshots path is not a real directory".to_string());
    }
    let canonical = std::fs::canonicalize(&child)
        .map_err(|e| format!("Cannot canonicalize screenshots directory: {e}"))?;
    if canonical != child || canonical.parent() != Some(root) {
        return Err("Screenshots directory is not directly inside the replica root".to_string());
    }
    Ok(canonical)
}

pub(super) fn validate_selection(
    selection: &ScreenshotSelection,
    image_w: u32,
    image_h: u32,
) -> Result<(), String> {
    if selection.width < 2 || selection.height < 2 {
        return Err("Selection is too small".to_string());
    }
    let x_end = selection.x.checked_add(selection.width);
    let y_end = selection.y.checked_add(selection.height);
    match (x_end, y_end) {
        (Some(xe), Some(ye)) if xe <= image_w && ye <= image_h => Ok(()),
        _ => Err("Selection is outside the captured image".to_string()),
    }
}

pub(super) fn build_filename(session_id: Uuid) -> String {
    let short = &session_id.simple().to_string()[..8];
    // Local time: the filename is user-facing, so it should match the user's clock.
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("agentscommander-screenshot-{stamp}-{short}.png")
}

/// Collision loop mirroring `phone::messaging::create_message_file`: never
/// overwrite an existing file; retry with `-<n>.png` up to the bound.
pub(super) fn create_png_file(
    dir: &Path,
    base_filename: &str,
) -> Result<(PathBuf, std::fs::File), String> {
    let stem = base_filename
        .strip_suffix(".png")
        .ok_or_else(|| format!("filename '{base_filename}' must end in .png"))?;
    for n in 0..=MAX_COLLISION_SUFFIX {
        let filename = if n == 0 {
            base_filename.to_string()
        } else {
            format!("{stem}-{n}.png")
        };
        let path = dir.join(&filename);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("failed to create {}: {e}", path.display())),
        }
    }
    Err(format!(
        "exhausted {MAX_COLLISION_SUFFIX} filename collisions for '{base_filename}'"
    ))
}
