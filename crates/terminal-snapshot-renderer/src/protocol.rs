use std::fmt;

use serde::{Deserialize, Serialize};

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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TerminalColor {
    Default,
    Indexed { index: u8 },
    Rgb { red: u8, green: u8, blue: u8 },
}

impl fmt::Debug for TerminalColor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Default => "Default",
            Self::Indexed { .. } => "Indexed",
            Self::Rgb { .. } => "Rgb",
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalCellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl fmt::Debug for TerminalCellStyle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enabled_attributes = [self.bold, self.italic, self.underline, self.inverse]
            .into_iter()
            .filter(|enabled| *enabled)
            .count();
        formatter
            .debug_struct("TerminalCellStyle")
            .field("enabled_attributes", &enabled_attributes)
            .finish()
    }
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

impl fmt::Debug for TerminalCell {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalCell")
            .field("text_bytes", &self.text.len())
            .field("width", &self.width)
            .field("foreground", &self.foreground)
            .field("background", &self.background)
            .field("style", &self.style)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalLine {
    pub wrapped: bool,
    pub cells: Vec<TerminalCell>,
}

impl fmt::Debug for TerminalLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalLine")
            .field("wrapped", &self.wrapped)
            .field("cells", &self.cells.len())
            .finish()
    }
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

impl fmt::Debug for TerminalScreen {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cells = self
            .lines
            .iter()
            .fold(0usize, |total, line| total.saturating_add(line.cells.len()));
        formatter
            .debug_struct("TerminalScreen")
            .field("dimensions", &self.dimensions)
            .field("sequence", &self.sequence)
            .field("active_buffer", &self.active_buffer)
            .field("cursor", &self.cursor)
            .field("parser_errors", &self.parser_errors)
            .field("lines", &self.lines.len())
            .field("cells", &cells)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotSession {
    pub id: String,
    pub backend: TerminalBackendKind,
}

impl fmt::Debug for TerminalSnapshotSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotSession")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl fmt::Debug for TerminalSnapshotFidelity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotFidelity")
            .field("backend_scrollback_rows", &self.backend_scrollback_rows)
            .field("atomic_at_output_sequence", &self.atomic_at_output_sequence)
            .field("application_frame_atomic", &self.application_frame_atomic)
            .field(
                "all_active_viewport_cells_included",
                &self.all_active_viewport_cells_included,
            )
            .field("includes_frontend_state", &self.includes_frontend_state)
            .field("exact_frontend_pixels", &self.exact_frontend_pixels)
            .field("parser_had_errors", &self.parser_had_errors)
            .field("omitted", &self.omitted.len())
            .field("unsupported", &self.unsupported.len())
            .finish_non_exhaustive()
    }
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

impl fmt::Debug for TerminalScreenModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalScreenModel")
            .field("session", &self.session)
            .field("screen", &self.screen)
            .field("fidelity", &self.fidelity)
            .finish_non_exhaustive()
    }
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

impl fmt::Debug for TerminalSnapshotDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotDocument")
            .field("schema_version", &self.schema_version)
            .field("session", &self.session)
            .field("screen", &self.screen)
            .field("fidelity", &self.fidelity)
            .finish_non_exhaustive()
    }
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

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRendererFontMetadata {
    pub family: String,
    pub version: String,
    pub sha256: String,
    pub size_px: f32,
}

impl fmt::Debug for TerminalRendererFontMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalRendererFontMetadata")
            .field("size_px", &self.size_px)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRendererCellMetadata {
    pub width_px: u32,
    pub height_px: u32,
    pub baseline_px: u32,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
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

impl fmt::Debug for TerminalRendererMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalRendererMetadata")
            .field("font", &self.font)
            .field("cell", &self.cell)
            .field("padding_px", &self.padding_px)
            .field("fallback_glyph_count", &self.fallback_glyph_count)
            .finish_non_exhaustive()
    }
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

#[derive(Clone, PartialEq, Serialize, Deserialize)]
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

impl fmt::Debug for TerminalSnapshotPngMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotPngMetadata")
            .field("schema_version", &self.schema_version)
            .field("session", &self.session)
            .field("screen", &self.screen)
            .field("fidelity", &self.fidelity)
            .field("format", &self.format)
            .field("png", &self.png)
            .field("renderer", &self.renderer)
            .finish_non_exhaustive()
    }
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
            .field("result", &self.result)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSnapshotApiError {
    pub api_version: String,
    pub error: TerminalSnapshotReasonCode,
    pub detail: String,
}

impl fmt::Debug for TerminalSnapshotApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotApiError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
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
            .field("result", &self.result)
            .field("error", &self.error)
            .field("has_detail", &self.detail.is_some())
            .finish_non_exhaustive()
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
            .field("format", &self.format)
            .finish_non_exhaustive()
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

mod validation;

pub use validation::{
    canonical_timestamp, validate_hex, validate_requester_identity, validate_screen,
    validate_target_syntax, validate_timestamp, validate_uuid, validate_wg_fqn,
};

#[cfg(test)]
mod tests;
