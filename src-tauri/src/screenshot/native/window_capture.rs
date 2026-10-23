//! #1285 window capture (Windows-only).

// #1842: the window-capture region is Windows-only. Its consumers are gated
// separately (`cli/mod.rs:28-31`, `api/handlers/mod.rs:60-61`); ungated on
// Linux, `pub(crate) capture_window_png` and `parse_window_id` are dead code and
// `cargo clippy --all-targets -- -D warnings` fails. Do NOT widen these to
// `#[cfg(any(target_os = "windows", test))]`:
// `crate::api::WINDOW_SCREENSHOT_MAX_PNG_BYTES` (`api/mod.rs:60-61`) is
// `#[cfg(target_os = "windows")]` with no `test` arm, unlike its siblings at
// 51-63, so the widened form breaks the Linux test compile. The compiler is the
// completeness check: a non-gated `impl` for a gated type is a hard error. The
// imports stay used off Windows (`std::io::Write as _` at the save path, the
// `image::` items in the preview encoder), so no import becomes unused.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowScreenshotCaptureError {
    NotFound,
    TooLarge,
    Unavailable,
}

#[cfg(target_os = "windows")]
pub(super) trait TargetWindowCaptureOps {
    type Window;
    type Image;

    fn all_windows(&mut self) -> Result<Vec<Self::Window>, String>;
    fn window_id(&mut self, window: &Self::Window) -> Result<String, String>;
    fn window_width(&mut self, window: &Self::Window) -> Result<u32, String>;
    fn window_height(&mut self, window: &Self::Window) -> Result<u32, String>;
    fn is_minimized(&mut self, window: &Self::Window) -> Result<bool, String>;
    fn capture_image(&mut self, window: &Self::Window) -> Result<Self::Image, String>;
    fn image_dimensions(&mut self, image: &Self::Image) -> (u32, u32);
    fn encode_png_bounded(
        &mut self,
        image: &Self::Image,
    ) -> Result<Vec<u8>, WindowScreenshotCaptureError>;
}

#[cfg(target_os = "windows")]
struct XcapTargetWindowCaptureOps;

#[cfg(target_os = "windows")]
impl TargetWindowCaptureOps for XcapTargetWindowCaptureOps {
    type Window = xcap::Window;
    type Image = image::RgbaImage;

    fn all_windows(&mut self) -> Result<Vec<Self::Window>, String> {
        xcap::Window::all().map_err(|error| error.to_string())
    }

    fn window_id(&mut self, window: &Self::Window) -> Result<String, String> {
        window
            .id()
            .map(|id| id.to_string())
            .map_err(|error| error.to_string())
    }

    fn window_width(&mut self, window: &Self::Window) -> Result<u32, String> {
        window.width().map_err(|error| error.to_string())
    }

    fn window_height(&mut self, window: &Self::Window) -> Result<u32, String> {
        window.height().map_err(|error| error.to_string())
    }

    fn is_minimized(&mut self, window: &Self::Window) -> Result<bool, String> {
        window.is_minimized().map_err(|error| error.to_string())
    }

    fn capture_image(&mut self, window: &Self::Window) -> Result<Self::Image, String> {
        window.capture_image().map_err(|error| error.to_string())
    }

    fn image_dimensions(&mut self, image: &Self::Image) -> (u32, u32) {
        (image.width(), image.height())
    }

    fn encode_png_bounded(
        &mut self,
        image: &Self::Image,
    ) -> Result<Vec<u8>, WindowScreenshotCaptureError> {
        encode_png_bounded(image)
    }
}

#[cfg(target_os = "windows")]
pub(super) struct BoundedPngWriter {
    pub(super) bytes: Vec<u8>,
    pub(super) maximum: usize,
    pub(super) exceeded: bool,
}

#[cfg(target_os = "windows")]
impl BoundedPngWriter {
    pub(super) fn new(maximum: usize) -> Self {
        Self {
            bytes: Vec::new(),
            maximum,
            exceeded: false,
        }
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(target_os = "windows")]
impl std::io::Write for BoundedPngWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > self.maximum.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("PNG exceeds configured size limit"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub(super) fn encode_png_bounded(
    image: &image::RgbaImage,
) -> Result<Vec<u8>, WindowScreenshotCaptureError> {
    let width = image.width();
    let height = image.height();
    let mut writer = BoundedPngWriter::new(crate::api::WINDOW_SCREENSHOT_MAX_PNG_BYTES);
    let encode_result = {
        let encoder = image::codecs::png::PngEncoder::new(&mut writer);
        image::ImageEncoder::write_image(
            encoder,
            image.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
    };
    if let Err(error) = encode_result {
        if writer.exceeded {
            return Err(WindowScreenshotCaptureError::TooLarge);
        }
        log::warn!("window screenshot PNG encoding failed: {error}");
        return Err(WindowScreenshotCaptureError::Unavailable);
    }
    Ok(writer.into_bytes())
}

#[cfg(target_os = "windows")]
pub(super) fn capture_window_worker<T: TargetWindowCaptureOps>(
    ops: &mut T,
    window_id: &str,
) -> Result<Vec<u8>, WindowScreenshotCaptureError> {
    let windows = ops.all_windows().map_err(|error| {
        log::warn!("window screenshot enumeration failed: {error}");
        WindowScreenshotCaptureError::Unavailable
    })?;
    let mut selected = None;
    for candidate in windows {
        let candidate_id = ops.window_id(&candidate).map_err(|error| {
            log::warn!("window screenshot ID inspection failed: {error}");
            WindowScreenshotCaptureError::Unavailable
        })?;
        if candidate_id == window_id {
            selected = Some(candidate);
            break;
        }
    }
    let selected = selected.ok_or(WindowScreenshotCaptureError::NotFound)?;

    let width = ops.window_width(&selected).map_err(|error| {
        log::warn!("window screenshot width inspection failed: {error}");
        WindowScreenshotCaptureError::Unavailable
    })?;
    let height = ops.window_height(&selected).map_err(|error| {
        log::warn!("window screenshot height inspection failed: {error}");
        WindowScreenshotCaptureError::Unavailable
    })?;
    let reported_pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(WindowScreenshotCaptureError::TooLarge)?;
    if reported_pixels > crate::api::WINDOW_SCREENSHOT_ADVISORY_SOURCE_PIXELS {
        return Err(WindowScreenshotCaptureError::TooLarge);
    }

    if ops.is_minimized(&selected).map_err(|error| {
        log::warn!("window screenshot minimized-window inspection failed: {error}");
        WindowScreenshotCaptureError::Unavailable
    })? {
        return Err(WindowScreenshotCaptureError::Unavailable);
    }

    let image = ops.capture_image(&selected).map_err(|error| {
        log::warn!("window screenshot native capture failed: {error}");
        WindowScreenshotCaptureError::Unavailable
    })?;
    let (captured_width, captured_height) = ops.image_dimensions(&image);
    let captured_pixels = u64::from(captured_width)
        .checked_mul(u64::from(captured_height))
        .ok_or(WindowScreenshotCaptureError::TooLarge)?;
    if captured_pixels > crate::api::WINDOW_SCREENSHOT_ADVISORY_SOURCE_PIXELS {
        return Err(WindowScreenshotCaptureError::TooLarge);
    }
    ops.encode_png_bounded(&image)
}

/// Captures the live native window selected by `window_id` inside one
/// `spawn_blocking` task and returns the encoded PNG bytes.
///
/// Lease protocol: `lease` is moved into the blocking closure and lives there
/// until enumeration, capture, and bounded PNG encoding finish. The handler's
/// request future may be dropped (client disconnect) while the task runs; the
/// lease still bounds capacity for the full native lifetime, and the dropped
/// request can neither publish a response nor retain the result.
#[cfg(target_os = "windows")]
pub(crate) async fn capture_window_png(
    window_id: String,
    lease: crate::api::WindowScreenshotLease,
) -> Result<Vec<u8>, WindowScreenshotCaptureError> {
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let mut ops = XcapTargetWindowCaptureOps;
        capture_window_worker(&mut ops, &window_id)
    })
    .await
    .map_err(|error| {
        log::warn!("window screenshot blocking worker failed: {error}");
        WindowScreenshotCaptureError::Unavailable
    })?
}

/// Canonical window-id validation shared by the HTTP route and the CLI verbs.
/// Accepts only the canonical ASCII decimal rendering of an xcap window id:
/// nonempty, at most 20 digits, no leading zeros except "0", parseable as u64.
#[cfg(target_os = "windows")]
pub(crate) fn parse_window_id(raw_window_id: &str) -> Option<String> {
    let bytes = raw_window_id.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 20
        || !bytes.iter().all(u8::is_ascii_digit)
        || (bytes.len() > 1 && bytes[0] == b'0')
        || raw_window_id.parse::<u64>().is_err()
    {
        return None;
    }
    Some(raw_window_id.to_string())
}
