//! Overlay windows and overlay IPC.

use tauri::{AppHandle, Manager};
use uuid::Uuid;

use super::{
    CapturedMonitor, ScreenshotCaptureLifecycle, ScreenshotCaptureState, ScreenshotOverlayState,
};

// ── Overlay windows ────────────────────────────────────────────────────────

/// Where an overlay window goes. `frame` names the coordinate space `x`, `y`,
/// `width` and `height` are expressed in.
pub(super) struct OverlayPlacement {
    pub(super) label: String,
    pub(super) monitor_id: u32,
    /// Physical pixels on `PlacementFrame::Physical`, logical points on
    /// `PlacementFrame::Logical`.
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) scale_factor: f64,
    pub(super) frame: PlacementFrame,
}

/// A monitor as either source can describe it. `scale` is that source's own
/// scale factor.
///
/// The rule this type exists to enforce (#2173): **tao owns the screen
/// coordinate space** — every value handed to Tauri is derived from
/// `tauri::Monitor` alone — and **xcap owns pixels** — the captured bitmap stays
/// the sole authority for the crop. The xcap scale factor is used for exactly
/// one thing: converting the bitmap size to points, so that an xcap monitor can
/// be compared to a tao monitor. The two scales disagree on macOS and the
/// disagreement is not reconcilable by a factor: xcap reports
/// `pixel_width / CGDisplayBounds.width` (1.714 in "More Space") while tao
/// reports `backingScaleFactor` (2.0) for the same display. Points are the only
/// frame in which both describe the monitor exactly.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MonitorGeometry {
    pub(super) name: Option<String>,
    /// The source's own frame: physical pixels for tao, points for xcap on macOS.
    pub(super) position: (i32, i32),
    /// tao: the physical monitor size. xcap: the captured bitmap, always pixels.
    pub(super) size: (u32, u32),
    pub(super) scale: f64,
}

/// A monitor's frame in logical points — the only cross-source comparison space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LogicalFrame {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

/// Geometry for one overlay, before it is paired with a label.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlacementGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) scale_factor: f64,
    pub(super) frame: PlacementFrame,
}

/// The coordinate frame overlay geometry is expressed in. macOS must use
/// `Logical`: its two sources disagree on the physical scale, so points are the
/// only frame where `tauri::Monitor` describes the monitor exactly. Windows and
/// Linux keep `Physical`, where the two sources agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlacementFrame {
    Physical,
    Logical,
}

pub(super) const fn placement_frame() -> PlacementFrame {
    #[cfg(target_os = "macos")]
    {
        PlacementFrame::Logical
    }
    #[cfg(not(target_os = "macos"))]
    {
        PlacementFrame::Physical
    }
}

/// Rounding slack when pairing the two sources by logical frame. Both tolerances
/// are compared against a **sum over the two axes**, not per axis.
///
/// Origin: 1 pt in total. Deliberately strict — a monitor that rounds a point on
/// x *and* on y sums to 2.0, misses this, and lands on the nearest-origin
/// fallback, which is the safe outcome for an ambiguous pairing.
///
/// Size: 2 pt in total, which does admit one point on each axis, because each
/// side divides a whole-pixel size by its own scale factor.
const LOGICAL_ORIGIN_TOLERANCE_PT: f64 = 1.0;
const LOGICAL_SIZE_TOLERANCE_PT: f64 = 2.0;

/// Neither source promises a positive scale; a zero would produce infinities.
fn positive_scale(scale: f64) -> f64 {
    if scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// tao reports origin and size in physical pixels, so points are both divided by
/// its own scale factor.
fn tauri_logical_frame(g: &MonitorGeometry) -> LogicalFrame {
    let scale = positive_scale(g.scale);
    LogicalFrame {
        x: g.position.0 as f64 / scale,
        y: g.position.1 as f64 / scale,
        width: g.size.0 as f64 / scale,
        height: g.size.1 as f64 / scale,
    }
}

/// xcap on macOS already reports the origin in points (`CGDisplayBounds`); only
/// the captured bitmap needs dividing by the xcap scale factor.
pub(super) fn xcap_logical_frame(g: &MonitorGeometry) -> LogicalFrame {
    let scale = positive_scale(g.scale);
    LogicalFrame {
        x: g.position.0 as f64,
        y: g.position.1 as f64,
        width: g.size.0 as f64 / scale,
        height: g.size.1 as f64 / scale,
    }
}

fn logical_origin_distance(a: &LogicalFrame, b: &LogicalFrame) -> f64 {
    (a.x - b.x).abs() + (a.y - b.y).abs()
}

/// Pair an xcap monitor with a tao monitor, returning its index.
///
/// On `Physical` (Windows, Linux) this is the historical criterion: by name,
/// else by nearest origin.
///
/// On `Logical` (macOS) the name is useless — xcap says `"Display #<model>"` and
/// tao says `"Monitor #<model>"`, so the comparison is never equal, and both
/// embed the display *model* number, so repairing the prefix would make two
/// identical displays collide. The logical frame identifies a monitor exactly
/// and disambiguates twins by origin: accept the first candidate whose origin
/// and size both fall inside the rounding tolerance, else fall back to the
/// nearest origin, which is what shipped before.
pub(super) fn match_monitor(
    tauri: &[MonitorGeometry],
    captured: &MonitorGeometry,
    frame: PlacementFrame,
) -> Option<usize> {
    if frame == PlacementFrame::Physical {
        if let Some(idx) = tauri
            .iter()
            .position(|tm| tm.name.is_some() && tm.name == captured.name)
        {
            return Some(idx);
        }
        let (cx, cy) = captured.position;
        return tauri
            .iter()
            .enumerate()
            .min_by_key(|(_, tm)| {
                let dx = (tm.position.0 - cx).unsigned_abs() as u64;
                let dy = (tm.position.1 - cy).unsigned_abs() as u64;
                dx + dy
            })
            .map(|(idx, _)| idx);
    }

    let want = xcap_logical_frame(captured);
    if let Some(idx) = tauri.iter().position(|tm| {
        let got = tauri_logical_frame(tm);
        logical_origin_distance(&got, &want) <= LOGICAL_ORIGIN_TOLERANCE_PT
            && (got.width - want.width).abs() + (got.height - want.height).abs()
                <= LOGICAL_SIZE_TOLERANCE_PT
    }) {
        return Some(idx);
    }
    tauri
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            logical_origin_distance(&tauri_logical_frame(a), &want)
                .total_cmp(&logical_origin_distance(&tauri_logical_frame(b), &want))
        })
        .map(|(idx, _)| idx)
}

impl PlacementGeometry {
    fn from_logical(f: LogicalFrame, scale_factor: f64) -> Self {
        Self {
            x: f.x.round() as i32,
            y: f.y.round() as i32,
            width: f.width.round().max(0.0) as u32,
            height: f.height.round().max(0.0) as u32,
            scale_factor,
            frame: PlacementFrame::Logical,
        }
    }
}

/// Geometry for one overlay. Prefer the matched tao monitor — it is authoritative
/// for every Tauri coordinate — and fall back to the xcap geometry when nothing
/// matched.
pub(super) fn build_placement(
    tauri: Option<&MonitorGeometry>,
    captured: &MonitorGeometry,
    frame: PlacementFrame,
) -> PlacementGeometry {
    match (frame, tauri) {
        (PlacementFrame::Physical, Some(tm)) => PlacementGeometry {
            x: tm.position.0,
            y: tm.position.1,
            width: tm.size.0,
            height: tm.size.1,
            scale_factor: tm.scale,
            frame,
        },
        (PlacementFrame::Physical, None) => PlacementGeometry {
            x: captured.position.0,
            y: captured.position.1,
            width: captured.size.0,
            height: captured.size.1,
            scale_factor: captured.scale,
            frame,
        },
        (PlacementFrame::Logical, Some(tm)) => {
            PlacementGeometry::from_logical(tauri_logical_frame(tm), tm.scale)
        }
        (PlacementFrame::Logical, None) => {
            PlacementGeometry::from_logical(xcap_logical_frame(captured), captured.scale)
        }
    }
}

/// Decide where each overlay goes. The adapter around the pure functions above:
/// describe both sides as `MonitorGeometry`, pair them, and build the placement
/// in this target's frame.
pub(super) fn build_placements(
    app: &AppHandle,
    capture_id: Uuid,
    monitors: &[CapturedMonitor],
) -> Vec<OverlayPlacement> {
    let frame = placement_frame();
    let tauri_monitors: Vec<MonitorGeometry> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|tm| {
            let pos = tm.position();
            let size = tm.size();
            MonitorGeometry {
                name: tm.name().cloned(),
                position: (pos.x, pos.y),
                size: (size.width, size.height),
                scale: tm.scale_factor(),
            }
        })
        .collect();
    monitors
        .iter()
        .map(|m| {
            let captured = MonitorGeometry {
                name: Some(m.name.clone()),
                position: (m.x, m.y),
                size: (m.width, m.height),
                scale: m.scale_factor,
            };
            let matched = match_monitor(&tauri_monitors, &captured, frame)
                .and_then(|idx| tauri_monitors.get(idx));
            #[cfg(target_os = "macos")]
            if let Some(tm) = matched {
                log::info!(
                    "[screenshot] monitor {} tauri raw position=({},{}) size={}x{} scale={}",
                    m.monitor_id,
                    tm.position.0,
                    tm.position.1,
                    tm.size.0,
                    tm.size.1,
                    tm.scale
                );
            }
            let g = build_placement(matched, &captured, frame);
            if g.frame == PlacementFrame::Physical && (g.width != m.width || g.height != m.height) {
                log::warn!(
                    "[screenshot] monitor {} placement size {}x{} differs from captured bitmap \
                        {}x{}; using placement for window, bitmap for crop",
                    m.monitor_id,
                    g.width,
                    g.height,
                    m.width,
                    m.height
                );
            }
            OverlayPlacement {
                label: overlay_label(capture_id, m.monitor_id),
                monitor_id: m.monitor_id,
                x: g.x,
                y: g.y,
                width: g.width,
                height: g.height,
                scale_factor: g.scale_factor,
                frame: g.frame,
            }
        })
        .collect()
}

pub(super) fn overlay_label(capture_id: Uuid, monitor_id: u32) -> String {
    format!("screenshot-overlay-{}-{}", capture_id.simple(), monitor_id)
}

/// Per-target overlay window configuration. A genuine behavioural divergence,
/// not an attribute — and a value rather than a `#[cfg]` inside the function,
/// so it is unit-testable (`open_overlay_windows` needs an `AppHandle`).
///
/// Measured (E5a, Mutter 46.2): a `_NET_WM_WINDOW_TYPE_NORMAL` overlay is
/// constrained into `_NET_WORKAREA`, so it landed at `+0+32`, never covering
/// the GNOME top bar, and re-issuing `set_position` did not move it; only
/// fullscreen escapes the struts. `resizable(false)` silently blocks fullscreen
/// by setting `min == max` size hints while Tauri still returns `Ok(())`.
/// `resizable(true)` + `set_fullscreen(true)` measured `2560x1440+0+0`, Δ = 0 on
/// all four values. macOS takes this same `not(linux)` arm (it uses the Windows
/// traits), because native fullscreen there would open a new Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OverlayWindowTraits {
    pub(super) resizable: bool,
    pub(super) fullscreen: bool,
}

pub(super) const fn overlay_window_traits() -> OverlayWindowTraits {
    #[cfg(target_os = "linux")]
    {
        OverlayWindowTraits {
            resizable: true,
            fullscreen: true,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        OverlayWindowTraits {
            resizable: false,
            fullscreen: false,
        }
    }
}

/// Build one always-on-top overlay per monitor. Transparency is applied off
/// macOS only (the frozen overlay image is opaque). Mirrors the
/// Resource Monitor builder (`commands::window`): logical inner size/position from
/// the monitor scale, corrected to physical bounds post-build. On any failure the
/// caller clears state and destroys already-created overlays.
pub(super) fn open_overlay_windows(
    app: &AppHandle,
    capture_id: Uuid,
    placements: &[OverlayPlacement],
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let traits = overlay_window_traits();
    for p in placements {
        // Builder-time size/position are logical points on every target. On the
        // `Logical` frame the placement already is points; on `Physical` it is
        // pixels and divides by the monitor scale, exactly as before.
        let (logical_w, logical_h, logical_x, logical_y) = match p.frame {
            PlacementFrame::Logical => (p.width as f64, p.height as f64, p.x as f64, p.y as f64),
            PlacementFrame::Physical => {
                let scale = positive_scale(p.scale_factor);
                (
                    p.width as f64 / scale,
                    p.height as f64 / scale,
                    p.x as f64 / scale,
                    p.y as f64 / scale,
                )
            }
        };
        let url = format!(
            "index.html?window=screenshot-overlay&captureId={capture_id}&monitorId={}",
            p.monitor_id
        );
        let builder = WebviewWindowBuilder::new(app, &p.label, WebviewUrl::App(url.into()))
            .title("Screenshot Capture")
            .decorations(false);
        // Tauri gates `transparent` behind `macos-private-api` on macOS; the
        // overlay paints an opaque frozen image, so transparency is unnecessary
        // there.
        #[cfg(not(target_os = "macos"))]
        let builder = builder.transparent(true);
        let window = builder
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(traits.resizable)
            .focused(true)
            .zoom_hotkeys_enabled(false)
            .inner_size(logical_w, logical_h)
            .position(logical_x, logical_y)
            .build()
            .map_err(|e| format!("failed to create overlay '{}': {e}", p.label))?;

        // Post-build correction, in the placement's own frame. `Physical` is the
        // historical path byte for byte; `Logical` is macOS, where the overlay
        // must be stated in points or a scaled display mode undersizes it.
        let (size, position) = match p.frame {
            PlacementFrame::Physical => (
                tauri::Size::Physical(tauri::PhysicalSize {
                    width: p.width,
                    height: p.height,
                }),
                tauri::Position::Physical(tauri::PhysicalPosition { x: p.x, y: p.y }),
            ),
            PlacementFrame::Logical => (
                tauri::Size::Logical(tauri::LogicalSize {
                    width: p.width as f64,
                    height: p.height as f64,
                }),
                tauri::Position::Logical(tauri::LogicalPosition {
                    x: p.x as f64,
                    y: p.y as f64,
                }),
            ),
        };
        window
            .set_size(size)
            .map_err(|e| format!("failed to size overlay '{}': {e}", p.label))?;
        window
            .set_position(position)
            .map_err(|e| format!("failed to position overlay '{}': {e}", p.label))?;

        if traits.fullscreen {
            // AFTER set_position: fullscreen targets the monitor the window
            // is on. Tauri returns Ok(()) even when the WM declines (E5a), so
            // this is best-effort and must not fail the capture.
            if let Err(e) = window.set_fullscreen(true) {
                log::warn!("[screenshot] overlay '{}' fullscreen refused: {e}", p.label);
            }
        }
    }
    Ok(())
}

/// Destroy every overlay window belonging to a capture id (by label prefix).
/// Idempotent: missing windows are skipped. Returns how many were destroyed.
pub(super) fn destroy_overlays_for_capture(app: &AppHandle, capture_id: Uuid) -> usize {
    let prefix = format!("screenshot-overlay-{}-", capture_id.simple());
    let mut destroyed = 0;
    for (label, window) in app.webview_windows() {
        if label.starts_with(&prefix) {
            match window.destroy() {
                Ok(()) => destroyed += 1,
                Err(e) => log::warn!("[screenshot] failed to destroy overlay '{label}': {e}"),
            }
        }
    }
    destroyed
}

// ── Overlay IPC ────────────────────────────────────────────────────────────

pub(super) enum OverlayDecision {
    Ready(Box<ScreenshotOverlayState>),
    Starting,
    Stale,
}

/// Return overlay state for an `Active` capture matching `capture_id`. While the
/// capture is still `Starting`, report that. For a stale id, destroy that id's
/// overlays without touching a different active capture.
pub async fn overlay_state(
    app: &AppHandle,
    state: &ScreenshotCaptureState,
    capture_id: &str,
    monitor_id: u32,
) -> Result<ScreenshotOverlayState, String> {
    let decision = {
        let guard = state.lock().await;
        match &*guard {
            ScreenshotCaptureLifecycle::Active(c) if c.id.to_string() == capture_id => {
                match c.monitors.iter().find(|m| m.monitor_id == monitor_id) {
                    Some(m) => OverlayDecision::Ready(Box::new(ScreenshotOverlayState {
                        capture_id: c.id.to_string(),
                        monitor_id: m.monitor_id,
                        monitor_x: m.x,
                        monitor_y: m.y,
                        width: m.width,
                        height: m.height,
                        scale_factor: m.scale_factor,
                        image_data_url: m.preview_data_url.clone(),
                        session_id: c.target_session_id.to_string(),
                        session_name: c.target_session_name.clone(),
                        // Strip the Windows `\\?\` verbatim prefix that `canonicalize`
                        // leaves on the resolved root, so the overlay shows a clean save
                        // location (shared #730 helper; internal PathBuf checks keep the
                        // raw canonical value).
                        target_directory:
                            crate::path_utils::path_to_string_without_windows_verbatim_prefix(
                                &c.target_directory,
                            ),
                    })),
                    None => OverlayDecision::Stale,
                }
            }
            ScreenshotCaptureLifecycle::Starting(s) if s.id.to_string() == capture_id => {
                OverlayDecision::Starting
            }
            _ => OverlayDecision::Stale,
        }
    };

    match decision {
        OverlayDecision::Ready(state) => Ok(*state),
        OverlayDecision::Starting => Err("Screenshot capture is still starting".to_string()),
        OverlayDecision::Stale => {
            if let Ok(id) = Uuid::parse_str(capture_id) {
                destroy_overlays_for_capture(app, id);
            }
            Err("Screenshot capture is no longer active".to_string())
        }
    }
}
