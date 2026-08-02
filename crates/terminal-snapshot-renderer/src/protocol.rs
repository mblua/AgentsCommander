use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub const API_VERSION: &str = "1";
pub const MAX_ROWS: u16 = 200;
pub const MAX_COLUMNS: u16 = 400;
pub const MAX_CELLS: usize = 40_000;
pub const MAX_PIXEL_SIDE: u32 = 4_096;
pub const MAX_PIXELS: usize = 8_200_000;
pub const MAX_RGB_BYTES: usize = 24_288_768;
pub const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PNG_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PNG_DECODER_ALLOCATION_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_TRANSPORT_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_ERROR_BYTES: usize = 8 * 1024;
pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
pub const MAX_BASE64_TEXT_BYTES: usize = 22_369_624;
pub const MAX_JSON_DEPTH: usize = 32;
pub const MAX_GLYPH_MASK_BYTES: usize = 8_388_608;

pub const CELL_WIDTH_PX: u32 = 10;
pub const CELL_HEIGHT_PX: u32 = 20;
pub const CELL_BASELINE_PX: u32 = 15;
pub const PADDING_PX: u32 = 8;
pub const RENDERER_ID: &str = "ac-terminal-png-v1";
pub const PALETTE_ID: &str = "ac-dark-v1";
pub const FONT_FAMILY: &str = "DejaVu Sans Mono";
pub const FONT_VERSION: &str = "2.37";
pub const FONT_SHA256: &str = "b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582";
pub const FONT_SIZE_PX: f32 = 16.0;
pub const CURSOR_POLICY: &str = "fixedVisibleBlock";
pub const ROOT_REQUESTER_IDENTITY: &str = "agentscommander://root-agent";

pub const FIDELITY_OMITTED: &[&str] = &[
    "applicationCursorMode",
    "applicationKeypadMode",
    "audibleBellCount",
    "bracketedPasteMode",
    "iconName",
    "inactiveBuffer",
    "mouseProtocolEncoding",
    "mouseProtocolMode",
    "title",
    "visualBellCount",
];

pub const FIDELITY_UNSUPPORTED: &[&str] = &[
    "blink",
    "colorEmoji",
    "cursorBlinkPhase",
    "cursorShape",
    "faint",
    "frontendFontMetrics",
    "frontendOverlays",
    "frontendScrollOffset",
    "frontendScrollback",
    "hyperlinks",
    "ligatures",
    "overline",
    "selection",
    "strikethrough",
    "terminalImages",
    "uiChrome",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalSnapshotFormat {
    Json,
    Png,
}

impl fmt::Display for TerminalSnapshotFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json => formatter.write_str("json"),
            Self::Png => formatter.write_str("png"),
        }
    }
}

impl std::str::FromStr for TerminalSnapshotFormat {
    type Err = ProtocolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "json" => Ok(Self::Json),
            "png" => Ok(Self::Png),
            _ => Err(ProtocolError::Invalid),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSnapshotReasonCode {
    InvalidRequest,
    RequesterUnavailable,
    TerminalSnapshotsDisabled,
    NotAuthorized,
    TargetUnavailable,
    SnapshotUnavailable,
    SnapshotTooLarge,
    AuthorityChanged,
    RateLimited,
    SnapshotTimeout,
    ServiceUnavailable,
    RenderFailed,
    UnsafePath,
    OutputFailed,
    ResponseUnavailable,
    Internal,
}

impl TerminalSnapshotReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::RequesterUnavailable => "requester_unavailable",
            Self::TerminalSnapshotsDisabled => "terminal_snapshots_disabled",
            Self::NotAuthorized => "not_authorized",
            Self::TargetUnavailable => "target_unavailable",
            Self::SnapshotUnavailable => "snapshot_unavailable",
            Self::SnapshotTooLarge => "snapshot_too_large",
            Self::AuthorityChanged => "authority_changed",
            Self::RateLimited => "rate_limited",
            Self::SnapshotTimeout => "snapshot_timeout",
            Self::ServiceUnavailable => "service_unavailable",
            Self::RenderFailed => "render_failed",
            Self::UnsafePath => "unsafe_path",
            Self::OutputFailed => "output_failed",
            Self::ResponseUnavailable => "response_unavailable",
            Self::Internal => "internal",
        }
    }

    pub const fn detail(self) -> &'static str {
        match self {
            Self::InvalidRequest => "The terminal snapshot request is invalid.",
            Self::RequesterUnavailable => "A unique live terminal snapshot requester is required.",
            Self::TerminalSnapshotsDisabled => "Terminal snapshots are disabled.",
            Self::NotAuthorized => "The terminal snapshot route is not authorized.",
            Self::TargetUnavailable => "The authorized target has no eligible live session.",
            Self::SnapshotUnavailable => "The authorized target screen is temporarily unavailable.",
            Self::SnapshotTooLarge => "The terminal snapshot exceeds a fixed resource limit.",
            Self::AuthorityChanged => "Terminal snapshot authority changed before disclosure.",
            Self::RateLimited => "The terminal snapshot rate or concurrency limit was reached.",
            Self::SnapshotTimeout => "The terminal snapshot did not complete before its deadline.",
            Self::ServiceUnavailable => {
                "A terminal snapshot security dependency is temporarily unavailable."
            }
            Self::RenderFailed => "The deterministic terminal renderer failed.",
            Self::UnsafePath => "A terminal snapshot path failed confinement checks.",
            Self::OutputFailed => {
                "The requested terminal snapshot output could not be completed safely."
            }
            Self::ResponseUnavailable => {
                "The terminal snapshot response could not be published or validated."
            }
            Self::Internal => "An internal terminal snapshot invariant failed.",
        }
    }

    pub const fn http_status(self) -> Option<u16> {
        match self {
            Self::InvalidRequest => Some(400),
            Self::RequesterUnavailable => Some(401),
            Self::TerminalSnapshotsDisabled | Self::NotAuthorized | Self::UnsafePath => Some(403),
            Self::TargetUnavailable => Some(404),
            Self::SnapshotUnavailable | Self::AuthorityChanged => Some(409),
            Self::SnapshotTooLarge => Some(413),
            Self::RateLimited => Some(429),
            Self::SnapshotTimeout => Some(504),
            Self::ServiceUnavailable => Some(503),
            Self::RenderFailed | Self::ResponseUnavailable | Self::Internal => Some(500),
            Self::OutputFailed => None,
        }
    }
}

impl fmt::Display for TerminalSnapshotReasonCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("invalid terminal snapshot protocol data")]
    Invalid,
    #[error("terminal snapshot protocol limit exceeded")]
    TooLarge,
    #[error("invalid terminal snapshot PNG")]
    InvalidPng,
    #[error("terminal snapshot serialization failed")]
    Serialization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalBackendKind {
    LocalProcess,
    ContainerTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalActiveBuffer {
    Normal,
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalCellWidth {
    Narrow,
    WideLead,
    WideContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TerminalColor {
    Default,
    Indexed { index: u8 },
    Rgb { red: u8, green: u8, blue: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalCellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalCell {
    pub text: String,
    pub width: TerminalCellWidth,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub style: TerminalCellStyle,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalLine {
    pub wrapped: bool,
    pub cells: Vec<TerminalCell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalDimensions {
    pub rows: u16,
    pub columns: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalImageDimensions {
    pub cells: usize,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub pixels: usize,
    pub rgb_bytes: usize,
}

impl TerminalDimensions {
    pub fn checked_image_dimensions(self) -> Result<TerminalImageDimensions, ProtocolError> {
        if self.rows == 0 || self.columns == 0 || self.rows > MAX_ROWS || self.columns > MAX_COLUMNS
        {
            return Err(ProtocolError::TooLarge);
        }
        let rows = usize::from(self.rows);
        let columns = usize::from(self.columns);
        let cells = rows
            .checked_mul(columns)
            .filter(|value| *value <= MAX_CELLS)
            .ok_or(ProtocolError::TooLarge)?;
        let pixel_width = u32::from(self.columns)
            .checked_mul(CELL_WIDTH_PX)
            .and_then(|value| value.checked_add(PADDING_PX * 2))
            .filter(|value| *value <= MAX_PIXEL_SIDE)
            .ok_or(ProtocolError::TooLarge)?;
        let pixel_height = u32::from(self.rows)
            .checked_mul(CELL_HEIGHT_PX)
            .and_then(|value| value.checked_add(PADDING_PX * 2))
            .filter(|value| *value <= MAX_PIXEL_SIDE)
            .ok_or(ProtocolError::TooLarge)?;
        let pixels = usize::try_from(pixel_width)
            .ok()
            .and_then(|width| {
                usize::try_from(pixel_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .filter(|value| *value <= MAX_PIXELS)
            .ok_or(ProtocolError::TooLarge)?;
        let rgb_bytes = pixels
            .checked_mul(3)
            .filter(|value| *value <= MAX_RGB_BYTES)
            .ok_or(ProtocolError::TooLarge)?;
        Ok(TerminalImageDimensions {
            cells,
            pixel_width,
            pixel_height,
            pixels,
            rgb_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalCursor {
    pub row: u16,
    pub column: u16,
    pub visible: bool,
    pub in_bounds: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalScreen {
    pub dimensions: TerminalDimensions,
    pub sequence: u64,
    pub active_buffer: TerminalActiveBuffer,
    pub cursor: TerminalCursor,
    pub parser_errors: u64,
    pub lines: Vec<TerminalLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotSession {
    pub id: String,
    pub backend: TerminalBackendKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotFidelity {
    pub scope: String,
    pub backend_parser: String,
    pub backend_scrollback_rows: u32,
    pub atomic_at_output_sequence: bool,
    pub application_frame_atomic: bool,
    pub all_active_viewport_cells_included: bool,
    pub includes_frontend_state: bool,
    pub exact_frontend_pixels: bool,
    pub parser_had_errors: bool,
    pub parser_error_coverage: String,
    pub omitted: Vec<String>,
    pub unsupported: Vec<String>,
}

impl TerminalSnapshotFidelity {
    pub fn version_one(parser_had_errors: bool) -> Self {
        Self {
            scope: "currentBackendViewport".to_string(),
            backend_parser: "vt100-0.15.2".to_string(),
            backend_scrollback_rows: 0,
            atomic_at_output_sequence: true,
            application_frame_atomic: false,
            all_active_viewport_cells_included: true,
            includes_frontend_state: false,
            exact_frontend_pixels: false,
            parser_had_errors,
            parser_error_coverage: "replacementC1AndUnhandledControls".to_string(),
            omitted: FIDELITY_OMITTED
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            unsupported: FIDELITY_UNSUPPORTED
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        }
    }

    pub fn validate(&self, parser_errors: u64) -> Result<(), ProtocolError> {
        if self.scope != "currentBackendViewport"
            || self.backend_parser != "vt100-0.15.2"
            || self.backend_scrollback_rows != 0
            || !self.atomic_at_output_sequence
            || self.application_frame_atomic
            || !self.all_active_viewport_cells_included
            || self.includes_frontend_state
            || self.exact_frontend_pixels
            || self.parser_had_errors != (parser_errors != 0)
            || self.parser_error_coverage != "replacementC1AndUnhandledControls"
            || !equal_fixed_strings(&self.omitted, FIDELITY_OMITTED)
            || !equal_fixed_strings(&self.unsupported, FIDELITY_UNSUPPORTED)
        {
            return Err(ProtocolError::Invalid);
        }
        Ok(())
    }
}

fn equal_fixed_strings(actual: &[String], expected: &[&str]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| actual == expected)
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalScreenModel {
    pub captured_at: String,
    pub session: TerminalSnapshotSession,
    pub screen: TerminalScreen,
    pub fidelity: TerminalSnapshotFidelity,
}

impl TerminalScreenModel {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_timestamp(&self.captured_at)?;
        validate_uuid(&self.session.id, None)?;
        validate_screen(&self.screen)?;
        self.fidelity.validate(self.screen.parser_errors)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotDocument {
    pub schema_version: u32,
    pub request_id: String,
    pub captured_at: String,
    pub requester: String,
    pub target: String,
    pub session: TerminalSnapshotSession,
    pub screen: TerminalScreen,
    pub fidelity: TerminalSnapshotFidelity,
}

impl TerminalSnapshotDocument {
    pub fn from_model(
        request_id: String,
        requester: String,
        target: String,
        model: &TerminalScreenModel,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            request_id,
            captured_at: model.captured_at.clone(),
            requester,
            target,
            session: model.session.clone(),
            screen: model.screen.clone(),
            fidelity: model.fidelity.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ProtocolError::Invalid);
        }
        validate_uuid(&self.request_id, Some(4))?;
        validate_timestamp(&self.captured_at)?;
        validate_requester_identity(&self.requester, true)?;
        validate_wg_fqn(&self.target)?;
        validate_uuid(&self.session.id, None)?;
        validate_screen(&self.screen)?;
        self.fidelity.validate(self.screen.parser_errors)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalPngScreenMetadata {
    pub dimensions: TerminalDimensions,
    pub sequence: u64,
    pub active_buffer: TerminalActiveBuffer,
    pub cursor: TerminalCursor,
    pub parser_errors: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalPngInfo {
    pub bytes: u64,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRendererFontMetadata {
    pub family: String,
    pub version: String,
    pub sha256: String,
    pub size_px: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRendererCellMetadata {
    pub width_px: u32,
    pub height_px: u32,
    pub baseline_px: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRendererMetadata {
    pub id: String,
    pub font: TerminalRendererFontMetadata,
    pub cell: TerminalRendererCellMetadata,
    pub padding_px: u32,
    pub palette_id: String,
    pub cursor_policy: String,
    pub fallback_glyph_count: u64,
}

impl TerminalRendererMetadata {
    pub fn version_one(fallback_glyph_count: u64) -> Self {
        Self {
            id: RENDERER_ID.to_string(),
            font: TerminalRendererFontMetadata {
                family: FONT_FAMILY.to_string(),
                version: FONT_VERSION.to_string(),
                sha256: FONT_SHA256.to_string(),
                size_px: FONT_SIZE_PX,
            },
            cell: TerminalRendererCellMetadata {
                width_px: CELL_WIDTH_PX,
                height_px: CELL_HEIGHT_PX,
                baseline_px: CELL_BASELINE_PX,
            },
            padding_px: PADDING_PX,
            palette_id: PALETTE_ID.to_string(),
            cursor_policy: CURSOR_POLICY.to_string(),
            fallback_glyph_count,
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.id != RENDERER_ID
            || self.font.family != FONT_FAMILY
            || self.font.version != FONT_VERSION
            || self.font.sha256 != FONT_SHA256
            || self.font.size_px.to_bits() != FONT_SIZE_PX.to_bits()
            || self.cell.width_px != CELL_WIDTH_PX
            || self.cell.height_px != CELL_HEIGHT_PX
            || self.cell.baseline_px != CELL_BASELINE_PX
            || self.padding_px != PADDING_PX
            || self.palette_id != PALETTE_ID
            || self.cursor_policy != CURSOR_POLICY
        {
            return Err(ProtocolError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotPngMetadata {
    pub schema_version: u32,
    pub request_id: String,
    pub captured_at: String,
    pub requester: String,
    pub target: String,
    pub session: TerminalSnapshotSession,
    pub screen: TerminalPngScreenMetadata,
    pub fidelity: TerminalSnapshotFidelity,
    pub format: TerminalSnapshotFormat,
    pub png: TerminalPngInfo,
    pub renderer: TerminalRendererMetadata,
}

impl TerminalSnapshotPngMetadata {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema_version != SCHEMA_VERSION || self.format != TerminalSnapshotFormat::Png {
            return Err(ProtocolError::Invalid);
        }
        validate_uuid(&self.request_id, Some(4))?;
        validate_timestamp(&self.captured_at)?;
        validate_requester_identity(&self.requester, true)?;
        validate_wg_fqn(&self.target)?;
        validate_uuid(&self.session.id, None)?;
        let image = self.screen.dimensions.checked_image_dimensions()?;
        if self.screen.cursor.in_bounds
            != (self.screen.cursor.row < self.screen.dimensions.rows
                && self.screen.cursor.column < self.screen.dimensions.columns)
            || self.png.bytes == 0
            || self.png.bytes > MAX_PNG_BYTES as u64
            || self.png.pixel_width != image.pixel_width
            || self.png.pixel_height != image.pixel_height
        {
            return Err(ProtocolError::Invalid);
        }
        self.fidelity.validate(self.screen.parser_errors)?;
        self.renderer.validate()
    }
}

/// Validated snapshot payload used in memory by the daemon and clients.
/// PNG bytes remain raw so no owned base64 copy can outlive wire decoding.
#[derive(PartialEq)]
pub enum TerminalSnapshotPayload {
    Json {
        snapshot: TerminalSnapshotDocument,
    },
    Png {
        metadata: TerminalSnapshotPngMetadata,
        png: Vec<u8>,
    },
}

impl fmt::Debug for TerminalSnapshotPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { snapshot } => formatter
                .debug_struct("Json")
                .field("rows", &snapshot.screen.dimensions.rows)
                .field("columns", &snapshot.screen.dimensions.columns)
                .finish(),
            Self::Png { metadata, png } => formatter
                .debug_struct("Png")
                .field("declared_bytes", &metadata.png.bytes)
                .field("decoded_bytes", &png.len())
                .finish(),
        }
    }
}

impl TerminalSnapshotPayload {
    pub fn format(&self) -> TerminalSnapshotFormat {
        match self {
            Self::Json { .. } => TerminalSnapshotFormat::Json,
            Self::Png { .. } => TerminalSnapshotFormat::Png,
        }
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::Json { snapshot } => &snapshot.request_id,
            Self::Png { metadata, .. } => &metadata.request_id,
        }
    }

    pub fn requester(&self) -> &str {
        match self {
            Self::Json { snapshot } => &snapshot.requester,
            Self::Png { metadata, .. } => &metadata.requester,
        }
    }

    pub fn target(&self) -> &str {
        match self {
            Self::Json { snapshot } => &snapshot.target,
            Self::Png { metadata, .. } => &metadata.target,
        }
    }
}

#[derive(PartialEq)]
pub struct TerminalSnapshotApiSuccess {
    pub api_version: String,
    pub result: TerminalSnapshotPayload,
}

impl fmt::Debug for TerminalSnapshotApiSuccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotApiSuccess")
            .field("api_version", &self.api_version)
            .field("result", &self.result)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotApiError {
    pub api_version: String,
    pub error: TerminalSnapshotReasonCode,
    pub detail: String,
}

impl TerminalSnapshotApiError {
    pub fn new(error: TerminalSnapshotReasonCode) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            error,
            detail: error.detail().to_string(),
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.api_version != API_VERSION || self.detail != self.error.detail() {
            return Err(ProtocolError::Invalid);
        }
        Ok(())
    }
}

#[derive(PartialEq)]
pub struct TerminalSnapshotHostResponse {
    pub api_version: String,
    pub request_id: String,
    pub confirmation_tag: String,
    pub expires_at: String,
    pub result: Option<TerminalSnapshotPayload>,
    pub error: Option<TerminalSnapshotReasonCode>,
    pub detail: Option<String>,
}

impl fmt::Debug for TerminalSnapshotHostResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotHostResponse")
            .field("request_id", &self.request_id)
            .field("has_result", &self.result.is_some())
            .field("error", &self.error)
            .finish()
    }
}

impl TerminalSnapshotHostResponse {
    pub fn failure(
        request_id: String,
        confirmation_tag: String,
        expires_at: String,
        error: TerminalSnapshotReasonCode,
    ) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            request_id,
            confirmation_tag,
            expires_at,
            result: None,
            error: Some(error),
            detail: Some(error.detail().to_string()),
        }
    }

    pub fn validate_shape(&self) -> Result<(), ProtocolError> {
        if self.api_version != API_VERSION
            || validate_uuid(&self.request_id, Some(4)).is_err()
            || validate_hex(&self.confirmation_tag, 64).is_err()
            || validate_timestamp(&self.expires_at).is_err()
        {
            return Err(ProtocolError::Invalid);
        }
        match (&self.result, self.error, &self.detail) {
            (Some(result), None, None) if result.request_id() == self.request_id => Ok(()),
            (None, Some(error), Some(detail)) if detail == error.detail() => Ok(()),
            _ => Err(ProtocolError::Invalid),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotApiRequest {
    pub api_version: String,
    pub request_id: String,
    pub to: String,
    pub format: TerminalSnapshotFormat,
}

impl fmt::Debug for TerminalSnapshotApiRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotApiRequest")
            .field("request_id", &self.request_id)
            .field("format", &self.format)
            .finish()
    }
}

impl TerminalSnapshotApiRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.api_version != API_VERSION {
            return Err(ProtocolError::Invalid);
        }
        validate_uuid(&self.request_id, Some(4))?;
        validate_target_syntax(&self.to)
    }
}

pub fn canonical_timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn validate_timestamp(value: &str) -> Result<DateTime<Utc>, ProtocolError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| ProtocolError::Invalid)?;
    let parsed = parsed.with_timezone(&Utc);
    if canonical_timestamp(parsed) != value {
        return Err(ProtocolError::Invalid);
    }
    Ok(parsed)
}

pub fn validate_uuid(value: &str, required_version: Option<usize>) -> Result<Uuid, ProtocolError> {
    let parsed = Uuid::parse_str(value).map_err(|_| ProtocolError::Invalid)?;
    if parsed.to_string() != value
        || required_version.is_some_and(|version| parsed.get_version_num() != version)
    {
        return Err(ProtocolError::Invalid);
    }
    Ok(parsed)
}

pub fn validate_hex(value: &str, length: usize) -> Result<(), ProtocolError> {
    if value.len() != length
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ProtocolError::Invalid);
    }
    Ok(())
}

pub fn validate_requester_identity(value: &str, allow_root: bool) -> Result<(), ProtocolError> {
    if allow_root && value == ROOT_REQUESTER_IDENTITY {
        return Ok(());
    }
    validate_wg_fqn(value)
}

pub fn validate_target_syntax(value: &str) -> Result<(), ProtocolError> {
    if value == ROOT_REQUESTER_IDENTITY {
        return Ok(());
    }
    if value.contains(':') {
        return validate_wg_fqn(value);
    }
    if value.is_empty() || value.len() > 1_024 || value.matches('/').count() != 1 {
        return Err(ProtocolError::Invalid);
    }
    let (project, agent) = value.split_once('/').ok_or(ProtocolError::Invalid)?;
    for component in [project, agent] {
        if component.is_empty()
            || component == "."
            || component == ".."
            || !component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ProtocolError::Invalid);
        }
    }
    Ok(())
}

pub fn validate_wg_fqn(value: &str) -> Result<(), ProtocolError> {
    if value.is_empty() || value.len() > 1_024 || value.matches(':').count() != 1 {
        return Err(ProtocolError::Invalid);
    }
    let (project, local) = value.split_once(':').ok_or(ProtocolError::Invalid)?;
    if project.is_empty()
        || matches!(project, "." | "..")
        || project.chars().any(|scalar| {
            scalar.is_control()
                || matches!(
                    scalar,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{2028}'
                        | '\u{2029}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
                || matches!(scalar, '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        || local.matches('/').count() != 1
    {
        return Err(ProtocolError::Invalid);
    }
    let (workgroup, agent) = local.split_once('/').ok_or(ProtocolError::Invalid)?;
    let rest = workgroup
        .strip_prefix("wg-")
        .ok_or(ProtocolError::Invalid)?;
    let (digits, team) = rest.split_once('-').ok_or(ProtocolError::Invalid)?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || team.is_empty()
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || agent.is_empty()
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ProtocolError::Invalid);
    }
    Ok(())
}

pub fn validate_screen(screen: &TerminalScreen) -> Result<(), ProtocolError> {
    let dimensions = screen.dimensions.checked_image_dimensions()?;
    if screen.lines.len() != usize::from(screen.dimensions.rows)
        || dimensions.cells
            != screen
                .lines
                .iter()
                .map(|line| line.cells.len())
                .sum::<usize>()
        || screen.cursor.in_bounds
            != (screen.cursor.row < screen.dimensions.rows
                && screen.cursor.column < screen.dimensions.columns)
    {
        return Err(ProtocolError::Invalid);
    }

    for line in &screen.lines {
        if line.cells.len() != usize::from(screen.dimensions.columns) {
            return Err(ProtocolError::Invalid);
        }
        for (column, cell) in line.cells.iter().enumerate() {
            if cell.text.chars().count() > 6 || cell.text.len() > 24 {
                return Err(ProtocolError::Invalid);
            }
            match cell.width {
                TerminalCellWidth::Narrow => {}
                TerminalCellWidth::WideLead => {
                    if column + 1 >= line.cells.len()
                        || line.cells[column + 1].width != TerminalCellWidth::WideContinuation
                    {
                        return Err(ProtocolError::Invalid);
                    }
                }
                TerminalCellWidth::WideContinuation => {
                    if !cell.text.is_empty()
                        || column == 0
                        || line.cells[column - 1].width != TerminalCellWidth::WideLead
                    {
                        return Err(ProtocolError::Invalid);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> TerminalCell {
        TerminalCell {
            text: String::new(),
            width: TerminalCellWidth::Narrow,
            foreground: TerminalColor::Default,
            background: TerminalColor::Default,
            style: TerminalCellStyle::default(),
        }
    }

    #[test]
    fn exact_dimension_boundaries_are_checked() {
        assert!(TerminalDimensions {
            rows: 100,
            columns: 400,
        }
        .checked_image_dimensions()
        .is_ok());
        assert!(TerminalDimensions {
            rows: 101,
            columns: 400,
        }
        .checked_image_dimensions()
        .is_err());
        assert!(TerminalDimensions {
            rows: 0,
            columns: 1,
        }
        .checked_image_dimensions()
        .is_err());
    }

    #[test]
    fn wide_pair_invariants_are_closed() {
        let mut screen = TerminalScreen {
            dimensions: TerminalDimensions {
                rows: 1,
                columns: 2,
            },
            sequence: 0,
            active_buffer: TerminalActiveBuffer::Normal,
            cursor: TerminalCursor {
                row: 0,
                column: 0,
                visible: true,
                in_bounds: true,
            },
            parser_errors: 0,
            lines: vec![TerminalLine {
                wrapped: false,
                cells: vec![cell(), cell()],
            }],
        };
        assert!(validate_screen(&screen).is_ok());
        screen.lines[0].cells[0].width = TerminalCellWidth::WideLead;
        assert!(validate_screen(&screen).is_err());
        screen.lines[0].cells[1].width = TerminalCellWidth::WideContinuation;
        assert!(validate_screen(&screen).is_ok());
        screen.lines[0].cells[1].text = "x".to_string();
        assert!(validate_screen(&screen).is_err());
    }

    #[test]
    fn reason_contract_is_fixed() {
        for code in [
            TerminalSnapshotReasonCode::InvalidRequest,
            TerminalSnapshotReasonCode::RequesterUnavailable,
            TerminalSnapshotReasonCode::TerminalSnapshotsDisabled,
            TerminalSnapshotReasonCode::NotAuthorized,
            TerminalSnapshotReasonCode::TargetUnavailable,
            TerminalSnapshotReasonCode::SnapshotUnavailable,
            TerminalSnapshotReasonCode::SnapshotTooLarge,
            TerminalSnapshotReasonCode::AuthorityChanged,
            TerminalSnapshotReasonCode::RateLimited,
            TerminalSnapshotReasonCode::SnapshotTimeout,
            TerminalSnapshotReasonCode::ServiceUnavailable,
            TerminalSnapshotReasonCode::RenderFailed,
            TerminalSnapshotReasonCode::UnsafePath,
            TerminalSnapshotReasonCode::OutputFailed,
            TerminalSnapshotReasonCode::ResponseUnavailable,
            TerminalSnapshotReasonCode::Internal,
        ] {
            assert!(!code.as_str().is_empty());
            assert!(code.detail().ends_with('.'));
        }
    }
}
