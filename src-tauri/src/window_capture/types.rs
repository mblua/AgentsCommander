use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use uuid::Uuid;

pub(crate) const TARGET_ID_PREFIX: &str = "wct_";
pub(crate) const TARGET_TTL: Duration = Duration::from_secs(60);
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const MIN_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const MAX_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const MAX_DIMENSION: u32 = 16_384;
pub(crate) const MAX_PIXELS: u64 = 33_554_432;
pub(crate) const MAX_RAW_RGBA_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_ENCODED_PNG_BYTES: u64 = 64 * 1024 * 1024;

/// An opaque, caller-scoped identifier. The inner UUID is never derived from
/// a native window handle or other discoverable target attribute.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct WindowTargetId(Uuid);

impl WindowTargetId {
    pub(crate) fn mint() -> Self {
        Self(Uuid::new_v4())
    }

    pub(crate) fn parse(value: &str) -> Result<Self, WindowCaptureError> {
        let Some(raw_uuid) = value.strip_prefix(TARGET_ID_PREFIX) else {
            return Err(WindowCaptureError::InvalidRequest);
        };
        let uuid = Uuid::parse_str(raw_uuid).map_err(|_| WindowCaptureError::InvalidRequest)?;
        if uuid.get_version_num() != 4 {
            return Err(WindowCaptureError::InvalidRequest);
        }

        let target_id = Self(uuid);
        if target_id.to_string() != value {
            return Err(WindowCaptureError::InvalidRequest);
        }
        Ok(target_id)
    }
}

impl fmt::Display for WindowTargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{TARGET_ID_PREFIX}{}", self.0.hyphenated())
    }
}

impl fmt::Debug for WindowTargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("WindowTargetId")
            .field(&self.to_string())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowVisibility {
    Visible,
    Hidden,
    Cloaked,
    Minimized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MonitorIntersection {
    IntersectsMonitor,
    OffScreen,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WindowBounds {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WindowDiagnostics {
    pub(crate) process_id: u32,
    pub(crate) process_name: String,
    pub(crate) title: String,
    pub(crate) class_name: String,
    pub(crate) bounds: Option<WindowBounds>,
    pub(crate) session_id: u32,
    pub(crate) visible: bool,
    pub(crate) minimized: bool,
    pub(crate) cloaked: bool,
    pub(crate) foreground: bool,
    pub(crate) protected: bool,
    pub(crate) monitor_intersection: MonitorIntersection,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DiscoveryFilter {
    pub(crate) process_id: Option<u32>,
    pub(crate) include_nonvisible: bool,
}

/// A native target identity retained only in daemon memory. It deliberately has
/// no serde, Debug, or public constructor implementation.
pub(crate) struct TargetFingerprint {
    hwnd: isize,
    process_id: u32,
    process_creation_time: u64,
    thread_id: u32,
    class_name_utf16: Vec<u16>,
    process_session_id: u32,
    logon_luid: u64,
    window_station: Vec<u16>,
    desktop: Vec<u16>,
}

impl TargetFingerprint {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::window_capture) fn new(
        hwnd: isize,
        process_id: u32,
        process_creation_time: u64,
        thread_id: u32,
        class_name_utf16: Vec<u16>,
        process_session_id: u32,
        logon_luid: u64,
        window_station: Vec<u16>,
        desktop: Vec<u16>,
    ) -> Self {
        Self {
            hwnd,
            process_id,
            process_creation_time,
            thread_id,
            class_name_utf16,
            process_session_id,
            logon_luid,
            window_station,
            desktop,
        }
    }

    pub(in crate::window_capture) fn hwnd(&self) -> isize {
        self.hwnd
    }

    pub(in crate::window_capture) fn process_id(&self) -> u32 {
        self.process_id
    }

    pub(in crate::window_capture) fn process_creation_time(&self) -> u64 {
        self.process_creation_time
    }

    pub(in crate::window_capture) fn thread_id(&self) -> u32 {
        self.thread_id
    }

    pub(in crate::window_capture) fn class_name_utf16(&self) -> &[u16] {
        &self.class_name_utf16
    }

    pub(in crate::window_capture) fn process_session_id(&self) -> u32 {
        self.process_session_id
    }

    pub(in crate::window_capture) fn logon_luid(&self) -> u64 {
        self.logon_luid
    }

    pub(in crate::window_capture) fn window_station(&self) -> &[u16] {
        &self.window_station
    }

    pub(in crate::window_capture) fn desktop(&self) -> &[u16] {
        &self.desktop
    }
}

/// Discovery returns the diagnostics separately from the private fingerprint so
/// the registry can bind the latter without making it part of a public DTO.
pub(crate) struct DiscoveredWindow {
    diagnostics: WindowDiagnostics,
    fingerprint: TargetFingerprint,
}

impl DiscoveredWindow {
    pub(in crate::window_capture) fn new(
        diagnostics: WindowDiagnostics,
        fingerprint: TargetFingerprint,
    ) -> Self {
        Self {
            diagnostics,
            fingerprint,
        }
    }

    pub(crate) fn into_parts(self) -> (WindowDiagnostics, TargetFingerprint) {
        (self.diagnostics, self.fingerprint)
    }

    pub(crate) fn compare_for_registry(&self, other: &Self) -> Ordering {
        self.diagnostics
            .process_name
            .to_lowercase()
            .cmp(&other.diagnostics.process_name.to_lowercase())
            .then_with(|| {
                self.diagnostics
                    .process_id
                    .cmp(&other.diagnostics.process_id)
            })
            .then_with(|| {
                self.diagnostics
                    .class_name
                    .encode_utf16()
                    .cmp(other.diagnostics.class_name.encode_utf16())
            })
            .then_with(|| {
                self.diagnostics
                    .title
                    .encode_utf16()
                    .cmp(other.diagnostics.title.encode_utf16())
            })
            .then_with(|| self.fingerprint.hwnd.cmp(&other.fingerprint.hwnd))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CaptureOptions {
    pub(crate) timeout: Duration,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl CaptureOptions {
    pub(crate) fn from_seconds(timeout_seconds: Option<u8>) -> Result<Self, WindowCaptureError> {
        let timeout = timeout_seconds
            .map(u64::from)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TIMEOUT);
        if !(MIN_TIMEOUT..=MAX_TIMEOUT).contains(&timeout) {
            return Err(WindowCaptureError::InvalidRequest);
        }
        Ok(Self { timeout })
    }
}

#[derive(Clone)]
pub(crate) struct CaptureCancellation {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}

impl CaptureCancellation {
    pub(crate) fn new(deadline: Instant) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline,
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, AtomicOrdering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire) || Instant::now() >= self.deadline
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// A verified final output reservation. Only the artifact module may construct
/// one, and it is intentionally non-cloneable and non-serializable.
pub(crate) struct PreparedOutput {
    pub(in crate::window_capture) final_path: PathBuf,
    pub(in crate::window_capture) temporary_path: PathBuf,
}

/// A completed temporary PNG whose final path remains absent until the
/// authority handoff permits atomic publication.
pub(crate) struct PendingCaptureArtifact {
    pub(in crate::window_capture) final_path: PathBuf,
    pub(in crate::window_capture) temporary_path: PathBuf,
    pub(in crate::window_capture) width: u32,
    pub(in crate::window_capture) height: u32,
    pub(in crate::window_capture) encoded_bytes: u64,
    pub(in crate::window_capture) sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CaptureMetadata {
    pub(crate) path: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) encoded_bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureSupportLevel {
    Supported,
    Conditional,
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum WindowCaptureError {
    #[error("invalid_request")]
    InvalidRequest,
    #[error("no_interactive_desktop")]
    NoInteractiveDesktop,
    #[error("capture_busy")]
    CaptureBusy,
    #[error("target_not_found")]
    TargetNotFound,
    #[error("stale_target")]
    StaleTarget,
    #[error("scope_denied")]
    ScopeDenied,
    #[error("window_hidden")]
    WindowHidden,
    #[error("window_cloaked")]
    WindowCloaked,
    #[error("window_minimized")]
    WindowMinimized,
    #[error("offscreen_no_frame")]
    OffscreenNoFrame,
    #[error("capture_protected")]
    CaptureProtected,
    #[error("capture_unsupported")]
    CaptureUnsupported,
    #[error("capture_timeout")]
    CaptureTimeout,
    #[error("capture_device_lost")]
    CaptureDeviceLost,
    #[error("capture_too_large")]
    CaptureTooLarge,
    #[error("target_closed")]
    TargetClosed,
    #[error("encode_failed")]
    EncodeFailed,
    #[error("output_denied")]
    OutputDenied,
    #[error("output_exists")]
    OutputExists,
}

impl WindowCaptureError {
    pub(crate) fn stable_code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::NoInteractiveDesktop => "no_interactive_desktop",
            Self::CaptureBusy => "capture_busy",
            Self::TargetNotFound => "target_not_found",
            Self::StaleTarget => "stale_target",
            Self::ScopeDenied => "scope_denied",
            Self::WindowHidden => "window_hidden",
            Self::WindowCloaked => "window_cloaked",
            Self::WindowMinimized => "window_minimized",
            Self::OffscreenNoFrame => "offscreen_no_frame",
            Self::CaptureProtected => "capture_protected",
            Self::CaptureUnsupported => "capture_unsupported",
            Self::CaptureTimeout => "capture_timeout",
            Self::CaptureDeviceLost => "capture_device_lost",
            Self::CaptureTooLarge => "capture_too_large",
            Self::TargetClosed => "target_closed",
            Self::EncodeFailed => "encode_failed",
            Self::OutputDenied => "output_denied",
            Self::OutputExists => "output_exists",
        }
    }
}

#[cfg(test)]
pub(crate) fn fixture_target_fingerprint(seed: u64) -> TargetFingerprint {
    TargetFingerprint::new(
        seed as isize,
        seed as u32,
        seed,
        seed as u32,
        vec![seed as u16],
        seed as u32,
        seed,
        vec![seed as u16],
        vec![seed as u16],
    )
}

#[cfg(test)]
pub(crate) fn fixture_discovered_window(seed: u64) -> DiscoveredWindow {
    DiscoveredWindow::new(
        WindowDiagnostics {
            process_id: seed as u32,
            process_name: format!("fixture-{seed}.exe"),
            title: format!("fixture-title-{seed}"),
            class_name: format!("fixture-class-{seed}"),
            bounds: Some(WindowBounds {
                left: seed as i32,
                top: seed as i32,
                right: seed as i32 + 10,
                bottom: seed as i32 + 10,
            }),
            session_id: seed as u32,
            visible: true,
            minimized: false,
            cloaked: false,
            foreground: false,
            protected: false,
            monitor_intersection: MonitorIntersection::IntersectsMonitor,
            warnings: Vec::new(),
        },
        fixture_target_fingerprint(seed),
    )
}

#[cfg(test)]
mod tests {
    use super::{CaptureOptions, WindowCaptureError, WindowTargetId, MAX_TIMEOUT, MIN_TIMEOUT};

    #[test]
    fn target_ids_are_canonical_v4_values() {
        let target_id = WindowTargetId::mint();
        let encoded = target_id.to_string();

        assert_eq!(
            WindowTargetId::parse(&encoded).unwrap().to_string(),
            encoded
        );
        assert!(matches!(
            WindowTargetId::parse("wct_550e8400-e29b-11d4-a716-446655440000"),
            Err(WindowCaptureError::InvalidRequest)
        ));
        assert!(matches!(
            WindowTargetId::parse(&encoded.to_uppercase()),
            Err(WindowCaptureError::InvalidRequest)
        ));
    }

    #[test]
    fn capture_timeout_is_bounded() {
        assert_eq!(
            CaptureOptions::from_seconds(Some(MIN_TIMEOUT.as_secs() as u8))
                .unwrap()
                .timeout,
            MIN_TIMEOUT
        );
        assert_eq!(
            CaptureOptions::from_seconds(Some(MAX_TIMEOUT.as_secs() as u8))
                .unwrap()
                .timeout,
            MAX_TIMEOUT
        );
        assert!(matches!(
            CaptureOptions::from_seconds(Some(4)),
            Err(WindowCaptureError::InvalidRequest)
        ));
        assert!(matches!(
            CaptureOptions::from_seconds(Some(61)),
            Err(WindowCaptureError::InvalidRequest)
        ));
    }
}
