use std::sync::{Arc, Mutex};
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::manager::{
    PtyManager, PtyTerminalReplaySnapshot, PtyTerminalSeedlessReason,
    TerminalOutputObservationStage,
};
use crate::voice::tracker::VoiceTrackingState;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyScreenSnapshotPayload {
    pub session_id: String,
    pub data: Vec<u8>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub sequence: u64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyTerminalOutputActivationPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<PtyTerminalReplaySnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seedless_reason: Option<PtyTerminalSeedlessReason>,
    attach_generation: u32,
    document_epoch: String,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachViewKind {
    Embedded,
    Externalized,
}

impl PtyTerminalAttachViewKind {
    fn code(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::Externalized => "externalized",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachTransitionKind {
    Initial,
    Switch,
    Reattach,
}

impl PtyTerminalAttachTransitionKind {
    fn code(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Switch => "switch",
            Self::Reattach => "reattach",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachRenderer {
    Webgl,
    Dom,
}

impl PtyTerminalAttachRenderer {
    fn code(self) -> &'static str {
        match self {
            Self::Webgl => "webgl",
            Self::Dom => "dom",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachContextState {
    Active,
    Lost,
    Unavailable,
}

impl PtyTerminalAttachContextState {
    fn code(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Lost => "lost",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachActiveBuffer {
    Normal,
    Alternate,
}

impl PtyTerminalAttachActiveBuffer {
    fn code(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Alternate => "alternate",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachAlternateEntryMode {
    Mode47,
    Mode1047,
    Mode1049,
}

impl PtyTerminalAttachAlternateEntryMode {
    fn code(self) -> &'static str {
        match self {
            Self::Mode47 => "mode47",
            Self::Mode1047 => "mode1047",
            Self::Mode1049 => "mode1049",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachReplayStage {
    SemanticHistory,
    ScreenOnlyHistoryDisabled,
    ScreenOnlyCheckpointUnavailable,
}

impl PtyTerminalAttachReplayStage {
    fn code(self) -> &'static str {
        match self {
            Self::SemanticHistory => "semanticHistory",
            Self::ScreenOnlyHistoryDisabled => "screenOnlyHistoryDisabled",
            Self::ScreenOnlyCheckpointUnavailable => "screenOnlyCheckpointUnavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachHistoryTruncationReason {
    None,
    RowLimitReached,
    ByteLimitReached,
    RowAndByteLimitReached,
}

impl PtyTerminalAttachHistoryTruncationReason {
    fn code(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RowLimitReached => "rowLimitReached",
            Self::ByteLimitReached => "byteLimitReached",
            Self::RowAndByteLimitReached => "rowAndByteLimitReached",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PtyTerminalAttachOutcome {
    Success,
    Stale,
    Timeout,
    Disposed,
    ResizeFailed,
    InvariantFailed,
    SnapshotDiscarded,
    SeedlessParserUnavailable,
    SeedlessParserPoisoned,
    SeedlessContinuationUnsafe,
    SeedlessInvalidGrid,
    SeedlessResizeFailed,
    SeedlessResourceLimitExceeded,
    SeedlessReplayCapExceeded,
    SeedlessSequenceUnsafe,
    SeedlessCaptureFailed,
    SeedlessEncodeFailed,
    ScreenOnlyHistoryDisabled,
    ScreenOnlyCheckpointUnavailable,
}

impl PtyTerminalAttachOutcome {
    fn code(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Stale => "stale",
            Self::Timeout => "timeout",
            Self::Disposed => "disposed",
            Self::ResizeFailed => "resizeFailed",
            Self::InvariantFailed => "invariantFailed",
            Self::SnapshotDiscarded => "snapshotDiscarded",
            Self::SeedlessParserUnavailable => "seedlessParserUnavailable",
            Self::SeedlessParserPoisoned => "seedlessParserPoisoned",
            Self::SeedlessContinuationUnsafe => "seedlessContinuationUnsafe",
            Self::SeedlessInvalidGrid => "seedlessInvalidGrid",
            Self::SeedlessResizeFailed => "seedlessResizeFailed",
            Self::SeedlessResourceLimitExceeded => "seedlessResourceLimitExceeded",
            Self::SeedlessReplayCapExceeded => "seedlessReplayCapExceeded",
            Self::SeedlessSequenceUnsafe => "seedlessSequenceUnsafe",
            Self::SeedlessCaptureFailed => "seedlessCaptureFailed",
            Self::SeedlessEncodeFailed => "seedlessEncodeFailed",
            Self::ScreenOnlyHistoryDisabled => "screenOnlyHistoryDisabled",
            Self::ScreenOnlyCheckpointUnavailable => "screenOnlyCheckpointUnavailable",
        }
    }

    fn is_debug(self) -> bool {
        matches!(
            self,
            Self::Success | Self::Stale | Self::Disposed | Self::ScreenOnlyHistoryDisabled
        )
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PtyTerminalAttachObservation {
    session_id: String,
    stage: TerminalOutputObservationStage,
    document_epoch: String,
    xterm_instance_id: u32,
    view_kind: PtyTerminalAttachViewKind,
    transition_kind: PtyTerminalAttachTransitionKind,
    attach_generation: u32,
    sequence: u64,
    outcome: PtyTerminalAttachOutcome,
    parser_rows: Option<u16>,
    parser_cols: Option<u16>,
    conpty_rows: Option<u16>,
    conpty_cols: Option<u16>,
    snapshot_rows: Option<u16>,
    snapshot_cols: Option<u16>,
    xterm_rows: Option<u16>,
    xterm_cols: Option<u16>,
    history_requested: Option<bool>,
    history_included: Option<bool>,
    history_truncated: Option<bool>,
    history_truncation_reason: Option<PtyTerminalAttachHistoryTruncationReason>,
    history_boundary_hardened: Option<bool>,
    retained_history_rows: Option<u32>,
    included_history_rows: Option<u32>,
    semantic_history_bytes: Option<u32>,
    replay_bytes: Option<u32>,
    normal_screen_included: Option<bool>,
    active_buffer: Option<PtyTerminalAttachActiveBuffer>,
    alternate_entry_mode: Option<PtyTerminalAttachAlternateEntryMode>,
    replay_stage: Option<PtyTerminalAttachReplayStage>,
    retained_event_count: Option<u32>,
    retained_byte_count: Option<u64>,
    resource_sessions: Option<u32>,
    resource_steady_bytes: Option<u64>,
    resource_checkpoint_bytes: Option<u64>,
    resource_attach_bytes: Option<u64>,
    viewport_y: Option<u32>,
    base_y: Option<u32>,
    buffer_length: Option<u32>,
    visible_row_count: Option<u16>,
    missing_visible_row_count: Option<u16>,
    renderer: Option<PtyTerminalAttachRenderer>,
    context_state: Option<PtyTerminalAttachContextState>,
    container_connected: Option<bool>,
    xterm_connected: Option<bool>,
    screen_connected: Option<bool>,
    element_width: Option<u32>,
    element_height: Option<u32>,
    screen_width: Option<u32>,
    screen_height: Option<u32>,
    canvas_width: Option<u32>,
    canvas_height: Option<u32>,
    route_wait_micros: Option<u64>,
    ownership_wait_micros: Option<u64>,
    parser_lock_hold_micros: Option<u64>,
    clone_micros: Option<u64>,
    encode_micros: Option<u64>,
    backend_activation_micros: Option<u64>,
    fetch_micros: Option<u64>,
    write_micros: Option<u64>,
    fit_micros: Option<u64>,
    resize_micros: Option<u64>,
    settle_micros: Option<u64>,
    total_micros: Option<u64>,
    parser_prefix_included: Option<bool>,
    replay_barrier_completed: Option<bool>,
    retained_barrier_completed: Option<bool>,
    grid_agreement: Option<bool>,
    resize_confirmed: Option<bool>,
    visible_rows_present: Option<bool>,
    bottom_position_satisfied: Option<bool>,
    expected_active_screen_has_text: Option<bool>,
    observed_active_screen_has_text: Option<bool>,
    expected_bottom_line_has_text: Option<bool>,
    observed_bottom_line_has_text: Option<bool>,
}

fn parse_canonical_session_id(value: &str) -> Result<Uuid, String> {
    let parsed = Uuid::parse_str(value).map_err(|_| "invalidSessionId".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("invalidSessionId".to_string());
    }
    Ok(parsed)
}

fn parse_document_epoch(value: &str) -> Result<u64, String> {
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("invalidDocumentEpoch".to_string());
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| "invalidDocumentEpoch".to_string())?;
    if parsed == 0 || parsed.to_string() != value {
        return Err("invalidDocumentEpoch".to_string());
    }
    Ok(parsed)
}

fn validate_attach_generation(value: u32) -> Result<u32, String> {
    if value == 0 {
        Err("invalidAttachGeneration".to_string())
    } else {
        Ok(value)
    }
}

fn validate_observation_grid(
    rows: Option<u16>,
    cols: Option<u16>,
    allow_zero: bool,
) -> Result<(), String> {
    match (rows, cols) {
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => Err("observationGridPairRequired".to_string()),
        (Some(rows), Some(cols)) if !allow_zero && (rows == 0 || cols == 0) => {
            Err("observationGridInvalid".to_string())
        }
        (Some(_), Some(_)) => Ok(()),
    }
}

fn validate_observation_shape(
    observation: &PtyTerminalAttachObservation,
    label: &str,
) -> Result<(Uuid, u64), String> {
    if label.is_empty() || label.len() > 256 {
        return Err("invalidWebviewLabel".to_string());
    }
    let session_id = parse_canonical_session_id(&observation.session_id)?;
    let document_epoch = parse_document_epoch(&observation.document_epoch)?;
    if observation.xterm_instance_id == 0 || observation.attach_generation == 0 {
        return Err("invalidObservationIdentity".to_string());
    }
    if observation.sequence > 9_007_199_254_740_991 {
        return Err("unsafeObservationSequence".to_string());
    }

    validate_observation_grid(observation.parser_rows, observation.parser_cols, false)?;
    validate_observation_grid(observation.conpty_rows, observation.conpty_cols, false)?;
    validate_observation_grid(observation.snapshot_rows, observation.snapshot_cols, false)?;
    validate_observation_grid(observation.xterm_rows, observation.xterm_cols, true)?;

    for value in [
        observation.retained_byte_count,
        observation.resource_steady_bytes,
        observation.resource_checkpoint_bytes,
        observation.resource_attach_bytes,
    ]
    .into_iter()
    .flatten()
    {
        if value > 512 * 1024 * 1024 {
            return Err("unsafeObservationCounter".to_string());
        }
    }
    for value in [
        observation.route_wait_micros,
        observation.ownership_wait_micros,
        observation.parser_lock_hold_micros,
        observation.clone_micros,
        observation.encode_micros,
        observation.backend_activation_micros,
        observation.fetch_micros,
        observation.write_micros,
        observation.fit_micros,
        observation.resize_micros,
        observation.settle_micros,
        observation.total_micros,
    ]
    .into_iter()
    .flatten()
    {
        if value > 60_000_000 {
            return Err("observationDurationOutOfRange".to_string());
        }
    }
    for value in [
        observation.element_width,
        observation.element_height,
        observation.screen_width,
        observation.screen_height,
        observation.canvas_width,
        observation.canvas_height,
    ]
    .into_iter()
    .flatten()
    {
        if value > 131_072 {
            return Err("observationPixelOutOfRange".to_string());
        }
    }
    if observation
        .replay_bytes
        .is_some_and(|value| value > 512 * 1024)
        || observation
            .semantic_history_bytes
            .is_some_and(|value| value > 65_536)
        || observation
            .history_boundary_hardened
            .is_some_and(|hardened| hardened && observation.history_truncated != Some(true))
    {
        return Err("observationReplayMetadataInvalid".to_string());
    }
    if observation
        .resource_sessions
        .is_some_and(|sessions| sessions > 32)
    {
        return Err("observationResourceMetadataInvalid".to_string());
    }
    if observation
        .resource_steady_bytes
        .is_some_and(|bytes| bytes > 128 * 1024 * 1024)
        || observation
            .resource_checkpoint_bytes
            .is_some_and(|bytes| bytes > 256 * 1024 * 1024)
        || observation
            .resource_attach_bytes
            .is_some_and(|bytes| bytes > 512 * 1024 * 1024)
    {
        return Err("observationResourceMetadataInvalid".to_string());
    }
    if observation
        .retained_history_rows
        .zip(observation.included_history_rows)
        .is_some_and(|(retained, included)| included > retained)
        || observation
            .included_history_rows
            .is_some_and(|included| included > 1024)
        || observation
            .history_requested
            .is_some_and(|requested| !requested && observation.history_included != Some(false))
        || observation.history_included.is_some_and(|included| {
            !included && observation.included_history_rows.unwrap_or(0) != 0
        })
    {
        return Err("observationHistoryMetadataInvalid".to_string());
    }
    if observation.history_truncated.is_some_and(|truncated| {
        !truncated
            && observation
                .history_truncation_reason
                .is_some_and(|reason| reason != PtyTerminalAttachHistoryTruncationReason::None)
    }) || observation.history_truncation_reason.is_some_and(|reason| {
        reason == PtyTerminalAttachHistoryTruncationReason::None
            && observation.history_truncated == Some(true)
    }) {
        return Err("observationHistoryTruncationInvalid".to_string());
    }
    if observation.active_buffer == Some(PtyTerminalAttachActiveBuffer::Normal)
        && observation.alternate_entry_mode.is_some()
    {
        return Err("observationAlternateMetadataInvalid".to_string());
    }
    if observation.alternate_entry_mode.is_some()
        && observation.active_buffer != Some(PtyTerminalAttachActiveBuffer::Alternate)
    {
        return Err("observationAlternateMetadataInvalid".to_string());
    }
    match (observation.renderer, observation.context_state) {
        (None, None) => {}
        (Some(PtyTerminalAttachRenderer::Webgl), Some(PtyTerminalAttachContextState::Active))
        | (Some(PtyTerminalAttachRenderer::Dom), Some(PtyTerminalAttachContextState::Lost))
        | (
            Some(PtyTerminalAttachRenderer::Dom),
            Some(PtyTerminalAttachContextState::Unavailable),
        ) => {}
        _ => return Err("observationRendererContextInvalid".to_string()),
    }
    if observation.viewport_y.is_some()
        || observation.base_y.is_some()
        || observation.buffer_length.is_some()
    {
        let (Some(_viewport), Some(base), Some(buffer), Some(rows)) = (
            observation.viewport_y,
            observation.base_y,
            observation.buffer_length,
            observation.xterm_rows,
        ) else {
            return Err("observationBufferInvariantMissing".to_string());
        };
        if base
            .checked_add(u32::from(rows))
            .is_none_or(|end| end > buffer)
        {
            return Err("observationBufferInvariantFailed".to_string());
        }
    }
    if observation
        .missing_visible_row_count
        .zip(observation.visible_row_count)
        .is_some_and(|(missing, visible)| missing > visible)
    {
        return Err("observationVisibleRowsInvalid".to_string());
    }
    if observation
        .visible_rows_present
        .is_some_and(|present| present && observation.missing_visible_row_count != Some(0))
    {
        return Err("observationVisibleRowsInvalid".to_string());
    }
    if observation.container_connected.is_some_and(|connected| {
        connected
            && observation
                .element_width
                .zip(observation.element_height)
                .is_none()
    }) {
        return Err("observationGeometryMissing".to_string());
    }

    match observation.stage {
        TerminalOutputObservationStage::Settled => {
            let current_grid = observation.xterm_rows.zip(observation.xterm_cols);
            let current_grid_agrees = current_grid.is_some_and(|grid| {
                observation.parser_rows.zip(observation.parser_cols) == Some(grid)
                    && observation.conpty_rows.zip(observation.conpty_cols) == Some(grid)
            });
            let canvas_matches_renderer = match observation.renderer {
                Some(PtyTerminalAttachRenderer::Webgl) => observation
                    .canvas_width
                    .zip(observation.canvas_height)
                    .is_some_and(|(width, height)| width > 0 && height > 0),
                Some(PtyTerminalAttachRenderer::Dom) => {
                    matches!(
                        (observation.canvas_width, observation.canvas_height),
                        (None, None) | (Some(1..), Some(1..))
                    )
                }
                None => false,
            };
            let semantic_expectations_match = observation
                .expected_active_screen_has_text
                .zip(observation.observed_active_screen_has_text)
                .is_some_and(|(expected, observed)| expected == observed)
                && observation
                    .expected_bottom_line_has_text
                    .zip(observation.observed_bottom_line_has_text)
                    .is_some_and(|(expected, observed)| expected == observed);
            if !matches!(
                observation.outcome,
                PtyTerminalAttachOutcome::Success
                    | PtyTerminalAttachOutcome::ScreenOnlyHistoryDisabled
                    | PtyTerminalAttachOutcome::ScreenOnlyCheckpointUnavailable
            ) {
                return Err("observationSettledOutcomeInvalid".to_string());
            }
            if observation.renderer.is_none()
                || observation.context_state.is_none()
                || observation.container_connected != Some(true)
                || observation.xterm_connected != Some(true)
                || observation.screen_connected != Some(true)
                || observation
                    .element_width
                    .zip(observation.element_height)
                    .is_none_or(|(width, height)| width == 0 || height == 0)
                || observation
                    .screen_width
                    .zip(observation.screen_height)
                    .is_none_or(|(width, height)| width == 0 || height == 0)
                || !canvas_matches_renderer
                || observation.viewport_y.is_none()
                || observation.base_y.is_none()
                || observation.buffer_length.is_none()
                || current_grid.is_none_or(|(rows, cols)| rows == 0 || cols == 0)
                || observation
                    .snapshot_rows
                    .zip(observation.snapshot_cols)
                    .is_none()
                || observation.grid_agreement != Some(true)
                || !current_grid_agrees
                || observation.resize_confirmed != Some(true)
                || observation.visible_rows_present != Some(true)
                || observation.visible_row_count != observation.xterm_rows
                || observation.missing_visible_row_count.is_none()
                || observation.bottom_position_satisfied != Some(true)
                || observation.replay_barrier_completed != Some(true)
                || observation.retained_barrier_completed != Some(true)
                || observation.missing_visible_row_count != Some(0)
                || observation.viewport_y != observation.base_y
                || !semantic_expectations_match
            {
                return Err("observationSettlementInvariantFailed".to_string());
            }
        }
        TerminalOutputObservationStage::Aborted => {
            if observation.outcome == PtyTerminalAttachOutcome::Success {
                return Err("observationAbortedOutcomeInvalid".to_string());
            }
        }
        TerminalOutputObservationStage::PostWrite | TerminalOutputObservationStage::PostFit => {}
    }
    Ok((session_id, document_epoch))
}

fn render_terminal_attach_observation(
    observation: &PtyTerminalAttachObservation,
    session_id: Uuid,
    document_epoch: u64,
    label: &str,
) -> String {
    let message = format!(
        "[terminal-snapshot] event=terminal_attach_observation stage={} session={} label={label:?} epoch={document_epoch} instance={} view={} transition={} generation={} sequence={} outcome={} parser_rows={:?} parser_cols={:?} conpty_rows={:?} conpty_cols={:?} snapshot_rows={:?} snapshot_cols={:?} xterm_rows={:?} xterm_cols={:?} history_requested={:?} history_included={:?} history_truncated={:?} history_reason={} history_boundary_hardened={:?} retained_history_rows={:?} included_history_rows={:?} semantic_history_bytes={:?} replay_bytes={:?} normal_screen_included={:?} active_buffer={:?} alternate_entry_mode={:?} replay_stage={:?} retained_event_count={:?} retained_byte_count={:?} renderer={:?} context={:?} element_width={:?} element_height={:?} screen_width={:?} screen_height={:?} canvas_width={:?} canvas_height={:?} resource_sessions={:?} resource_steady_bytes={:?} resource_checkpoint_bytes={:?} resource_attach_bytes={:?} viewport_y={:?} base_y={:?} buffer_length={:?} visible_rows={:?} missing_visible_rows={:?} route_wait_us={:?} ownership_wait_us={:?} parser_lock_hold_us={:?} clone_us={:?} encode_us={:?} backend_activation_us={:?} fetch_us={:?} write_us={:?} fit_us={:?} resize_us={:?} settle_us={:?} total_us={:?} connected={} resize_confirmed={:?} grid_agreement={:?} parser_prefix_included={:?} replay_barrier_completed={:?} retained_barrier_completed={:?} visible_rows_present={:?} bottom_position_satisfied={:?} expected_active_text={:?} observed_active_text={:?} expected_bottom_text={:?} observed_bottom_text={:?}",
        observation.stage.code(),
        session_id,
        observation.xterm_instance_id,
        observation.view_kind.code(),
        observation.transition_kind.code(),
        observation.attach_generation,
        observation.sequence,
        observation.outcome.code(),
        observation.parser_rows,
        observation.parser_cols,
        observation.conpty_rows,
        observation.conpty_cols,
        observation.snapshot_rows,
        observation.snapshot_cols,
        observation.xterm_rows,
        observation.xterm_cols,
        observation.history_requested,
        observation.history_included,
        observation.history_truncated,
        observation
            .history_truncation_reason
            .map_or("none", PtyTerminalAttachHistoryTruncationReason::code),
        observation.history_boundary_hardened,
        observation.retained_history_rows,
        observation.included_history_rows,
        observation.semantic_history_bytes,
        observation.replay_bytes,
        observation.normal_screen_included,
        observation.active_buffer.map(PtyTerminalAttachActiveBuffer::code),
        observation
            .alternate_entry_mode
            .map(PtyTerminalAttachAlternateEntryMode::code),
        observation.replay_stage.map(PtyTerminalAttachReplayStage::code),
        observation.retained_event_count,
        observation.retained_byte_count,
        observation.renderer.map(PtyTerminalAttachRenderer::code),
        observation.context_state.map(PtyTerminalAttachContextState::code),
        observation.element_width,
        observation.element_height,
        observation.screen_width,
        observation.screen_height,
        observation.canvas_width,
        observation.canvas_height,
        observation.resource_sessions,
        observation.resource_steady_bytes,
        observation.resource_checkpoint_bytes,
        observation.resource_attach_bytes,
        observation.viewport_y,
        observation.base_y,
        observation.buffer_length,
        observation.visible_row_count,
        observation.missing_visible_row_count,
        observation.route_wait_micros,
        observation.ownership_wait_micros,
        observation.parser_lock_hold_micros,
        observation.clone_micros,
        observation.encode_micros,
        observation.backend_activation_micros,
        observation.fetch_micros,
        observation.write_micros,
        observation.fit_micros,
        observation.resize_micros,
        observation.settle_micros,
        observation.total_micros,
        observation.xterm_connected.unwrap_or(false)
            || observation.screen_connected.unwrap_or(false)
            || observation.container_connected.unwrap_or(false),
        observation.resize_confirmed,
        observation.grid_agreement,
        observation.parser_prefix_included,
        observation.replay_barrier_completed,
        observation.retained_barrier_completed,
        observation.visible_rows_present,
        observation.bottom_position_satisfied,
        observation.expected_active_screen_has_text,
        observation.observed_active_screen_has_text,
        observation.expected_bottom_line_has_text,
        observation.observed_bottom_line_has_text,
    );
    message
}

fn log_terminal_attach_observation(
    observation: &PtyTerminalAttachObservation,
    session_id: Uuid,
    document_epoch: u64,
    label: &str,
) {
    let message =
        render_terminal_attach_observation(observation, session_id, document_epoch, label);
    if observation.outcome.is_debug() {
        log::debug!("{message}");
    } else {
        log::warn!("{message}");
    }
}

#[tauri::command]
pub(crate) fn record_terminal_attach_observation<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    observation: PtyTerminalAttachObservation,
) -> Result<(), String> {
    let label = webview.label();
    let (session_id, document_epoch) = validate_observation_shape(&observation, label)?;
    let coordinator = {
        let manager = pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?;
        manager.terminal_output_coordinator()
    };
    coordinator
        .accept_observation_stage(
            label,
            session_id,
            document_epoch,
            observation.attach_generation,
            observation.stage,
        )
        .map_err(|error| error.code().to_string())?;
    log_terminal_attach_observation(&observation, session_id, document_epoch, label);
    Ok(())
}

/// (#871) Classifies user-input notifications so fresh-intent clearing can be
/// gated on substantive post-boundary submissions.
pub(crate) enum UserInputSource<'a> {
    /// xterm terminal keystrokes from the Tauri `pty_write` command.
    Terminal(&'a [u8]),
    /// Web UI raw keystrokes from binary frames or the web command path.
    Web(&'a [u8]),
    /// A complete submitted message, always substantive.
    CompleteMessage,
}

#[tauri::command]
pub async fn pty_write(
    app: AppHandle,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    voice_tracker: State<'_, VoiceTrackingState>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // (#885 J1) Keystrokes into a session being purged would flip it busy
    // between the readiness snapshot and its destroy. Scoped to the purge's
    // target set, so typing in unrelated sessions is unaffected.
    if let Some(g) = app.try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>() {
        if g.blocks_session(uuid) {
            return Err("purge-wg in progress for this session; input rejected".to_string());
        }
    }

    // Flag if user typed while voice recording is active for this session.
    // Lock scope closes before pty_mgr lock to avoid holding both.
    {
        let mut tracker = voice_tracker.lock().unwrap();
        if tracker.is_recording(uuid) {
            tracker.mark_typed(uuid);
        }
    }

    let permit = PtyManager::acquire_input_writer(pty_mgr.inner(), uuid)
        .await
        .map_err(|error| error.to_string())?;
    // Purge may have started while this writer waited behind another complete
    // submission. Recheck at the serialized boundary.
    if let Some(g) = app.try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>() {
        if g.blocks_session(uuid) {
            return Err("purge-wg in progress for this session; input rejected".to_string());
        }
    }
    PtyManager::write_with_permit(&permit, &data).map_err(|error| error.to_string())?;
    mark_successful_pty_write_busy(&app, uuid, data.len()).await;
    drop(permit);

    // #552 user input -> silence touch (+ badge reset if coordinator). Resolves
    // all state from `app`, so the same helper serves Telegram and web.
    note_user_message_to_session(&app, uuid, UserInputSource::Terminal(&data)).await;

    Ok(())
}

pub(crate) async fn mark_successful_pty_write_busy<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    byte_count: usize,
) {
    if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
        idle.record_activity_with_bytes(session_id, byte_count);
    }
    if let Some(sessions) =
        app.try_state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>()
    {
        let manager = {
            let guard = sessions.read().await;
            guard.clone()
        };
        manager.mark_busy(session_id).await;
    }
}

/// #552 Record a real user message to `session_id`: always reset the auto-close
/// silence clock; if the session is a coordinator, reset its badge clock and
/// emit `coordinator_clock_updated` (and clear any "auto-closed" marker).
/// Resolves all state from `app`, so every user-input surface (xterm
/// `pty_write`, Telegram inbound, web UI) can call it with its source tag.
/// Injection / auto-resume MUST NOT call this (they are not user messages).
///
/// Generic over the Tauri runtime so callers holding either a concrete
/// `AppHandle` or a generic `AppHandle<R>` (e.g. the Telegram bridge) can reuse it.
pub(crate) async fn note_user_message_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    source: UserInputSource<'_>,
) {
    let (substantive, source_class): (bool, &'static str) = match source {
        UserInputSource::Terminal(data) => {
            (classify_substantive(app, session_id, data), "terminal")
        }
        UserInputSource::Web(data) => (classify_substantive(app, session_id, data), "web"),
        UserInputSource::CompleteMessage => (true, "message"),
    };

    // (a) auto-close silence: any user message keeps the team alive.
    if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
        idle.touch_silence(session_id);
    }

    // (#630/#631 + #698) Apply both user-input state transitions and persist them
    // atomically. On the FIRST substantive post-boundary submission we re-arm the
    // resume intent, and every user write still lowers any visible raise-hand
    // communication. This is the unified user-input choke point
    // (xterm/Telegram/web); injection and auto-resume never call it, and it runs
    // before the coordinator-only early return below so non-coordinator members
    // re-arm too when the write is substantive.
    //
    // `clear_user_input_transitions_and_persist_result` flips BOTH fields in one
    // SessionManager critical section and runs the mutation + snapshot + save
    // under a single global save lock, so no concurrent persist can snapshot a
    // half-applied state or write an intermediate one (MEDIUM grinch fix). The
    // clear event is emitted only after persistence succeeds, so `list-sessions`
    // and the UI agree.
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    let cleared =
        match crate::config::sessions_persistence::clear_user_input_transitions_and_persist_result(
            &manager,
            session_id,
            substantive,
        )
        .await
        {
            Ok(cleared) => cleared,
            Err(e) => {
                log::error!(
                    "Failed to persist user-input session state transitions: {}",
                    e
                );
                // The in-memory clear still applied; only the snapshot failed.
                // Suppress the clear event so the live UI does not diverge from the
                // durable file (the next persist will reconcile disk).
                crate::config::sessions_persistence::ClearedUserInputTransitions::default()
            }
        };

    if cleared.cleared_start_fresh {
        log::info!(
            "[session-state] {} fresh intent cleared: substantive {} input (#871)",
            &session_id.to_string()[..8],
            source_class
        );
    } else if !substantive {
        log::debug!(
            "[session-state] {} non-substantive {} write; fresh intent preserved (#871)",
            &session_id.to_string()[..8],
            source_class
        );
    }

    if cleared.cleared_raise_hand {
        crate::session::selection::publish_session_communication(app, session_id, None);
    }

    // (b) badge: reset only when the typed-to session is a coordinator.
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    let Some(cwd) = cwd else { return };
    let Some(clocks) = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    else {
        return;
    };

    // agent_fqn_from_path returns String (teams.rs:80), not Option.
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let now = chrono::Utc::now();
    let (changed, cleared_auto, cleared_fresh) = {
        let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
        let changed = guard.note_user_message(&fqn, now);
        // #552 a real user message reopens the coordinator -> clear any
        // "auto-closed" marker (idempotent; no-op if not marked).
        let cleared_auto = guard.clear_auto_closed(&fqn);
        // (#871/#756) Drop the fresh-intent mirror only on a substantive
        // post-boundary submission. Non-substantive terminal writes leave it set
        // so an app restart still restores fresh.
        let cleared_fresh = if substantive {
            guard.clear_start_fresh(&fqn)
        } else {
            false
        };
        (changed, cleared_auto, cleared_fresh)
    };
    if changed {
        let _ = app.emit(
            "coordinator_clock_updated",
            serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": now.to_rfc3339() }),
        );
    }
    if cleared_auto {
        let _ = app.emit(
            "coordinator_auto_close_changed",
            serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
        );
    }
    if cleared_fresh {
        // (#756) Persist immediately: this transition must survive an app close
        // inside the 60s flush tick window (mirrors close_coordinator's
        // explicit save; the exit flush in lib.rs only covers clean exits).
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
            log::warn!("[coordinator-clocks] fresh-intent clear save failed: {}", e);
        }
    }
}

/// (#871) Run the substantive-submission classifier for a raw keystroke chunk.
/// Locks the managed tracker briefly with no await held. Fail-open to true if
/// the tracker state is absent, preserving the historical clear contract.
fn classify_substantive<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    data: &[u8],
) -> bool {
    let Some(state) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() else {
        return true;
    };
    let mut tracker = state.lock().unwrap_or_else(|e| e.into_inner());
    tracker.feed(session_id, data)
}

/// (#756) Record an AC-driven fresh-conversation boundary for `session_id`:
/// write the coordinator-clocks mirror (`start_fresh_at`, persisted
/// immediately) and THEN stamp the durable record intent
/// (`start_fresh_on_restore = true`, persisted under the sessions save lock).
/// Mirror-first (section 19.3): the death-between-halves residue must fail
/// FRESH, never resurrect. The intent survives record destruction (idle
/// auto-close, manual close). Call only after a successful logical clear
/// injection (/clear or Pi /new; C1 remote action, C2 self-clear phase 1). The restart
/// path (C3) stamps the record itself and calls only the mirror half.
/// Root-agent sessions skip the record half (the root restore path ignores the
/// marker, #630 scope; mirrors the restart site's exclusion in
/// commands/session.rs); the mirror half self-gates on coordinators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryMetadataOutcome {
    Applied,
    Unchanged,
    Failed,
}

fn combine_boundary_metadata(
    first: BoundaryMetadataOutcome,
    second: BoundaryMetadataOutcome,
) -> BoundaryMetadataOutcome {
    use BoundaryMetadataOutcome as O;
    match (first, second) {
        (O::Failed, _) | (_, O::Failed) => O::Failed,
        (O::Applied, _) | (_, O::Applied) => O::Applied,
        (O::Unchanged, O::Unchanged) => O::Unchanged,
    }
}

pub(crate) async fn stamp_fresh_boundary_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> BoundaryMetadataOutcome {
    // (#756, section 19.3) MIRROR-FIRST: if the app dies between the halves,
    // the residue (mirror=Some, record=false) forces fresh on every reopen
    // path and self-heals (E3 re-propagates; typed input or injected content
    // clears both). Record-first residue (record=true, mirror=None) would let
    // a later record destroy resurrect the pre-boundary conversation: the
    // exact #756 bug.
    let mirror = write_start_fresh_mirror_outcome(app, session_id, true).await;
    if let Some(state) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset(session_id);
    }
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    // Single-clone lookup; DUAL root predicate mirrors the restart path so the
    // two exclusions never disagree.
    let is_root = manager
        .get_session(session_id)
        .await
        .map(|s| {
            s.is_root_agent || crate::config::root_agent::is_root_agent_path(&s.working_directory)
        })
        .unwrap_or(false);
    let record = if is_root {
        BoundaryMetadataOutcome::Unchanged
    } else {
        match crate::config::sessions_persistence::set_start_fresh_and_persist_result(
            &manager, session_id,
        )
        .await
        {
            Ok(true) => {
                log::info!(
                    "[session-state] {} fresh-boundary stamped (record, #756)",
                    &session_id.to_string()[..8]
                );
                BoundaryMetadataOutcome::Applied
            }
            Ok(false) => BoundaryMetadataOutcome::Unchanged,
            Err(_) => {
                log::error!(
                    "[session-state] fresh-boundary stamp persist failed session={} code=boundary_metadata_failed",
                    session_id
                );
                BoundaryMetadataOutcome::Failed
            }
        }
    };
    combine_boundary_metadata(mirror, record)
}

/// (#756) Drop the durable fresh intent after AC successfully injected message
/// CONTENT into `session_id` (standard mailbox body, follow-up after a remote
/// command, phase-2 handoff prompts, loop prompts). The injected body creates a
/// post-boundary transcript, so provider resume becomes safe and desirable
/// again; a lingering stamp would wipe a live autonomous conversation on the
/// next reopen. DELIBERATELY NARROW: must NOT reuse
/// `note_user_message_to_session`, whose injection-exclusion protects
/// silence/badge/auto-close semantics (see its doc comment above); this helper
/// touches ONLY the fresh intent (mirror first, then record; section 19.3).
/// Never call it for bare logical-action text (/clear, Pi /new, or /compact).
pub(crate) async fn note_post_boundary_content_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> BoundaryMetadataOutcome {
    // (#756, section 19.3) MIRROR-FIRST: the drop residue (mirror=None,
    // record=true) only mis-freshes the record-alive restore until the next
    // heal; record-first residue (record=false, mirror=Some) would wrongly
    // force-fresh BOTH reopen paths.
    let mirror = write_start_fresh_mirror_outcome(app, session_id, false).await;
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    let record = match crate::config::sessions_persistence::clear_start_fresh_and_persist_result(
        &manager, session_id,
    )
    .await
    {
        Ok(true) => {
            log::info!(
                "[session-state] {} fresh intent dropped (post-boundary content, #756)",
                &session_id.to_string()[..8]
            );
            BoundaryMetadataOutcome::Applied
        }
        Ok(false) => BoundaryMetadataOutcome::Unchanged,
        Err(_) => {
            log::error!(
                "[session-state] post-boundary-content drop persist failed session={} code=boundary_metadata_failed",
                session_id
            );
            BoundaryMetadataOutcome::Failed
        }
    };
    combine_boundary_metadata(mirror, record)
}

/// (#756) Mirror half: write the coordinator-clocks `start_fresh_at` for the
/// session's cwd. Returns false without touching anything for non-coordinators
/// (`coordinator_cwd` -> None; root agents land here too) or when the value is
/// already in the target state. Persists the clocks file immediately on a real
/// transition: these boundaries are rare and must survive an app close inside
/// the 60s flush tick (same discipline as close_coordinator's explicit save).
pub(crate) async fn write_start_fresh_mirror_for_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    on: bool,
) -> bool {
    write_start_fresh_mirror_outcome(app, session_id, on).await == BoundaryMetadataOutcome::Applied
}

async fn write_start_fresh_mirror_outcome<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    on: bool,
) -> BoundaryMetadataOutcome {
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    let Some(cwd) = cwd else {
        return BoundaryMetadataOutcome::Unchanged;
    };
    let Some(clocks) = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    else {
        return BoundaryMetadataOutcome::Unchanged;
    };
    // agent_fqn_from_path returns String (teams.rs:80), not Option.
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let changed = {
        let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            guard.mark_start_fresh(&fqn, chrono::Utc::now())
        } else {
            guard.clear_start_fresh(&fqn)
        }
    };
    if changed {
        log::info!(
            "[coordinator-clocks] start_fresh_at {} for '{}' (#756)",
            if on { "set" } else { "cleared" },
            fqn
        );
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if crate::config::coordinator_clocks::save_map(&snapshot).is_err() {
            log::warn!(
                "[coordinator-clocks] start_fresh_at save failed code=boundary_metadata_failed"
            );
            return BoundaryMetadataOutcome::Failed;
        }
        BoundaryMetadataOutcome::Applied
    } else {
        BoundaryMetadataOutcome::Unchanged
    }
}

#[tauri::command]
pub fn pty_resize<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    // #1439: the only record of who drove a grid change; the incident was
    // diagnosable only by inference because no pty resize was ever logged.
    log::info!(
        "[pty] resize session={uuid} cols={cols} rows={rows} from={}",
        webview.label()
    );
    pty_mgr
        .lock()
        .unwrap()
        .resize(uuid, cols, rows)
        .map_err(|e| e.to_string())
}

/// #955/#956 - the backend half of the snapshot round-trip measurement.
///
/// The frontend logs the whole trip (`[terminal] snapshot <id> settled in Nms`). This logs
/// what the backend spent inside it, and between them they say WHERE the time went. Read
/// the three numbers together:
///
/// - `handler_ms` - everything this function did, including waiting for both mutexes. If
///   this is milliseconds and the frontend says seconds, the backend is not the problem.
/// - `lock_ms` - just the wait for the `PtyManager` mutex, so backend contention cannot
///   hide inside the total.
/// - **the timestamp of this line.** This is a SYNC tauri command, so it runs on the main
///   thread: a line that appears late is a request that queued before the handler ever
///   started, which is a different bug from a response that came back late. Compare it
///   against the session's `[pty] Spawned session ...` line.
///
/// Fires once per terminal attach. Never on the PTY hot path.
#[tauri::command]
pub fn get_screen_snapshot(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
) -> Result<Option<PtyScreenSnapshotPayload>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let started = Instant::now();

    let (snapshot, lock_ms) = {
        let pty_mgr = pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?;
        let lock_ms = started.elapsed().as_secs_f64() * 1000.0;
        (pty_mgr.get_screen_snapshot(uuid), lock_ms)
    };

    log::info!(
        "[pty] screen-snapshot session={} handler_ms={:.3} lock_ms={:.3} found={} bytes={} sequence={}",
        session_id,
        started.elapsed().as_secs_f64() * 1000.0,
        lock_ms,
        snapshot.is_some(),
        snapshot.as_ref().map(|s| s.data.len()).unwrap_or(0),
        snapshot.as_ref().map(|s| s.sequence).unwrap_or(0)
    );

    let Some(snapshot) = snapshot else {
        return Ok(None);
    };

    Ok(Some(PtyScreenSnapshotPayload {
        session_id,
        data: snapshot.data,
        rows: Some(snapshot.rows),
        cols: Some(snapshot.cols),
        sequence: snapshot.sequence,
    }))
}

/// The terminal-output attach.
///
/// The window label comes from Tauri, from the calling webview, so a frontend can only ever
/// attach the window it runs in and the label can be neither forged nor misattributed.
#[tauri::command]
pub(crate) fn terminal_output_document_epoch<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
) -> Result<String, String> {
    let coordinator = pty_mgr
        .lock()
        .map_err(|_| "PtyManager lock poisoned".to_string())?
        .terminal_output_coordinator();
    coordinator
        .document_epoch(webview.label())
        .map(|epoch| epoch.to_string())
        .map_err(|error| error.code().to_string())
}

#[tauri::command]
pub(crate) fn activate_terminal_output<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
    // Optional on purpose, which is how a Tauri command argument gets a default: an
    // attribute on the parameter is not in scope here. Without the default, a missing or
    // misspelled argument makes Tauri reject the command, and the frontend fails closed with
    // no retry, so the terminal panel stays blank until the user reselects the session by
    // hand. The default is `true`, which is what the only caller sends: the client always
    // resets before it writes the seed, so a duplicate is impossible, and the failure the
    // other default produces instead is the silent content gap the plan rules to be the
    // worse of the two.
    include_history: Option<bool>,
    document_epoch: String,
    attach_generation: u32,
) -> Result<PtyTerminalOutputActivationPayload, String> {
    let parsed = parse_canonical_session_id(&session_id)?;
    let epoch = parse_document_epoch(&document_epoch)?;
    let generation = validate_attach_generation(attach_generation)?;
    let coordinator = pty_mgr
        .lock()
        .map_err(|_| "PtyManager lock poisoned".to_string())?
        .terminal_output_coordinator();
    let activation = coordinator
        .activate(
            webview.label(),
            parsed,
            epoch,
            generation,
            include_history.unwrap_or(true),
        )
        .map_err(|error| match error {
            crate::pty::manager::TerminalOutputCoordinationError::RouteUnavailable => {
                AppError::SessionNotFound(parsed.to_string()).to_string()
            }
            other => other.code().to_string(),
        })?;
    Ok(PtyTerminalOutputActivationPayload {
        snapshot: activation.snapshot,
        seedless_reason: activation.seedless_reason,
        attach_generation: activation.attach_generation,
        document_epoch: activation.document_epoch.to_string(),
    })
}

/// Releases this window's attachment.
///
/// A session that is already gone maps to `Ok(())` on purpose: window close races session
/// destroy, so the frontend detaches a destroyed session on every normal teardown, and an
/// error there would turn routine shutdown into error spam and invite a retry. Nothing else is
/// mapped, so a poisoned lock or a genuine routing failure still surfaces.
#[tauri::command]
pub(crate) fn detach_terminal_output<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
    document_epoch: String,
    attach_generation: u32,
) -> Result<(), String> {
    let session_id = parse_canonical_session_id(&session_id)?;
    let document_epoch = parse_document_epoch(&document_epoch)?;
    let attach_generation = validate_attach_generation(attach_generation)?;
    let coordinator = pty_mgr
        .lock()
        .map_err(|_| "PtyManager lock poisoned".to_string())?
        .terminal_output_coordinator();
    coordinator
        .detach(
            webview.label(),
            session_id,
            document_epoch,
            attach_generation,
        )
        .map_err(|error| error.code().to_string())
}

#[tauri::command]
pub(crate) fn cancel_terminal_output_activation<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
    document_epoch: String,
    attach_generation: u32,
) -> Result<(), String> {
    let session_id = parse_canonical_session_id(&session_id)?;
    let document_epoch = parse_document_epoch(&document_epoch)?;
    let attach_generation = validate_attach_generation(attach_generation)?;
    let coordinator = pty_mgr
        .lock()
        .map_err(|_| "PtyManager lock poisoned".to_string())?
        .terminal_output_coordinator();
    coordinator
        .cancel(
            webview.label(),
            session_id,
            document_epoch,
            attach_generation,
        )
        .map_err(|error| error.code().to_string())
}

/// #1032 - the last context reading for a session, for a frontend that just mounted and
/// missed the `session_context` event.
///
/// `None` covers every unavailable case there is - no regex, no match, a truncated row, a
/// session that is over, a scraper that is not managed - and NEVER means 0.
#[tauri::command]
pub fn get_session_context(app: AppHandle, session_id: String) -> Result<Option<u8>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(scraper) = app.try_state::<Arc<crate::pty::context_scrape::ContextScraper>>() else {
        return Ok(None);
    };
    Ok(scraper.last_reading(uuid))
}

/// #1171 - one session's watcher activity, for the window on mount and on every poll.
///
/// SYNCHRONOUS, and it takes exactly one per-session mutex. That is possible only because the
/// engine publishes `activeWatchers`, `possiblyMissedFrames` and `warmedUp` into the history at
/// the end of each tick, instead of this command resolving settings and the session manager
/// itself - which would put a read of the session lock on the window's polling path.
///
/// A session with no buffer returns an EMPTY snapshot, not `None` and not an error: the window
/// distinguishes its four states from the values here, with no nullability to reason about.
#[tauri::command]
pub fn get_watcher_activity<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    limit: Option<usize>,
) -> Result<crate::pty::watchers::history::WatcherActivitySnapshot, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(history) = app.try_state::<crate::pty::watchers::history::WatcherHistoryState>()
    else {
        return Ok(crate::pty::watchers::history::WatcherActivitySnapshot::empty());
    };
    Ok(history.snapshot(uuid, limit))
}

/// #1171 - what a pattern does, before it is saved.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherPatternPreview {
    pub compiles: bool,
    pub error: Option<String>,
    /// False when no session was given, or the session had no readable frame. This is what
    /// distinguishes "matched nothing" from "could not look".
    pub sampled: bool,
    pub matched_rows: usize,
    pub total_rows: usize,
    /// Up to 3 matched logical rows, each truncated to 256 bytes.
    pub samples: Vec<String>,
    /// True when the captures of the lowest match differed between the two samples taken about
    /// a second apart. A pattern that captures a clock or a token counter matches one row of
    /// thirty and still emits five events a second in `state` mode, and `matchedRows` alone
    /// cannot say so.
    pub captures_volatile: bool,
}

/// How many matched rows the preview shows.
const WATCHER_PREVIEW_SAMPLES: usize = 3;

/// #1171 - compile a candidate pattern and, optionally, run it against a live session.
///
/// `session_id: None` compiles only. That is the COMMON case: a user opens Settings and writes
/// a regex with no agent session running, and without this the only signal for a syntax error
/// would be the absence of activations.
///
/// `async` and doing the PTY reads inside `spawn_blocking`, because this path goes through
/// `PtyManager::screen_rows_since` and therefore takes the manager mutex and the route
/// registry - "the one every terminal write, resize and kill locks on" - while a session may
/// be producing heavy output. The engine's own tick avoids both by holding its backend `Arc`;
/// a preview debounced at 300 ms can afford them, and blocking the async runtime on them could
/// not. (No child liveness probe is involved: the watcher seam deliberately has none, see
/// `local_backend.rs`'s `screen_rows_since`.)
#[tauri::command]
pub async fn preview_watcher_pattern<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: Option<String>,
    pattern: String,
) -> Result<WatcherPatternPreview, String> {
    let compiled = match crate::pty::watchers::pattern::compile(&pattern) {
        Ok(compiled) => compiled,
        Err(error) => {
            // A pattern that does not compile is a RESULT, not a command failure: the Settings
            // control needs the message to show, not an exception to swallow.
            return Ok(WatcherPatternPreview {
                compiles: false,
                error: Some(error),
                sampled: false,
                matched_rows: 0,
                total_rows: 0,
                samples: Vec::new(),
                captures_volatile: false,
            });
        }
    };

    let mut preview = WatcherPatternPreview {
        compiles: true,
        error: None,
        sampled: false,
        matched_rows: 0,
        total_rows: 0,
        samples: Vec::new(),
        captures_volatile: false,
    };

    let Some(session_id) = session_id else {
        return Ok(preview);
    };
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(pty_mgr) = app.try_state::<Arc<Mutex<PtyManager>>>() else {
        return Ok(preview);
    };
    let pty_mgr = Arc::clone(pty_mgr.inner());

    let first = read_watcher_preview_frame(Arc::clone(&pty_mgr), uuid).await;
    let Some(first) = first else {
        return Ok(preview);
    };

    let logical = crate::pty::watchers::frame::logical_rows(&first);
    let regex = compiled.regex();
    let matched: Vec<&crate::pty::watchers::frame::LogicalRow> = logical
        .iter()
        .filter(|row| regex.is_match(&row.text))
        .collect();

    preview.sampled = true;
    preview.total_rows = logical.len();
    preview.matched_rows = matched.len();
    preview.samples = matched
        .iter()
        .take(WATCHER_PREVIEW_SAMPLES)
        .map(|row| crate::pty::watchers::truncate_row(&row.text).0)
        .collect();

    let lowest_captures = |rows: &[&crate::pty::watchers::frame::LogicalRow]| {
        rows.last()
            .and_then(|row| regex.captures(&row.text))
            .map(|found| {
                found
                    .iter()
                    .skip(1)
                    .map(|group| group.map(|m| m.as_str().to_string()))
                    .collect::<Vec<_>>()
            })
    };
    let before = lowest_captures(&matched);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    if let Some(second) = read_watcher_preview_frame(pty_mgr, uuid).await {
        let logical = crate::pty::watchers::frame::logical_rows(&second);
        let matched: Vec<&crate::pty::watchers::frame::LogicalRow> = logical
            .iter()
            .filter(|row| regex.is_match(&row.text))
            .collect();
        let after = lowest_captures(&matched);
        preview.captures_volatile = before.is_some() && after.is_some() && before != after;
    }

    Ok(preview)
}

/// Read one session's frame off the async runtime.
///
/// `seen: None` on purpose: the preview always wants the rows, never an `Unchanged`.
async fn read_watcher_preview_frame(
    pty_mgr: Arc<Mutex<PtyManager>>,
    id: Uuid,
) -> Option<crate::pty::watchers::ScreenFrame> {
    tokio::task::spawn_blocking(move || {
        let mgr = pty_mgr.lock().ok()?;
        match mgr.screen_rows_since(id, None) {
            crate::pty::watchers::ScreenRowsSince::Frame(frame) => Some(frame),
            _ => None,
        }
    })
    .await
    .ok()
    .flatten()
}

/// #1171 - one watcher row of the draft the Settings modal holds in memory.
///
/// Only the three fields `reaches` and the budget depend on (plan 4.8). `pattern`, `mode`,
/// `dedupe` and `capturedAgainst` take part in neither and are deliberately not sent: the row
/// already shows its pattern, and `preview_watcher_pattern` answers compilability separately,
/// so carrying it here would inflate every debounced payload to restate an answer that is
/// already on screen next to this one.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherDraftEntry {
    pub id: String,
    pub enabled: bool,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
}

/// #1171 - one agent row of the same draft.
///
/// The modal edits agents and watchers in ONE store and one Save writes both, so resolving
/// against the SAVED agent list would answer about a state the user has already left. Two of
/// the three agent edits over-report that way: deleting an agent leaves it named in a reach
/// list it will not be in, and changing an agent's `command` leaves a watcher reported as
/// reaching it under the old stem. Only adding an agent under-reports.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherAgentDraftEntry {
    pub id: String,
    pub label: String,
    pub command: String,
}

/// #1171 - one agent that a draft row's selector reaches.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachEntry {
    pub agent_id: String,
    pub agent_label: String,
    pub command_stem: String,
    /// Whether this row is enabled in the draft AND holds one of this agent's 8 slots once
    /// every other ENABLED row of the draft is counted. It is membership of the engine's own
    /// `running` list, and NOT a promise that the watcher will emit anything: a resolved
    /// watcher whose pattern does not compile is allocated a slot and is inert. Compilability
    /// is a separate dimension, answered per row by `preview_watcher_pattern`, and this field
    /// deliberately does not restate it. A disabled row is always false here, and the editor,
    /// which owns `enabled`, must say "disabled" rather than "budget".
    pub allocated: bool,
}

/// #1171 - the reach of one draft row.
///
/// Exactly one per requested row, in request order. It carries `id` back because the editor
/// filters unrecognised rows out of the request, so its table positions do not match the
/// response positions.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachRow {
    pub id: String,
    /// Every agent this row's SELECTOR reaches, whether or not the row is enabled. Reach is a
    /// property of the selector alone; `allocated` is where enablement and budget land.
    pub entries: Vec<WatcherReachEntry>,
}

/// #1171 - resolve the WHOLE Settings draft, watchers and agents, and report per row which
/// agents its selector reaches and which of those it holds a slot on.
///
/// This exists so the Settings UI reimplements neither stem normalization nor the budget rule.
/// There is exactly one stem rule in the tree (`command_executable_basename`), the catalog
/// rejects prefix matching in writing because `pi` and `agent` false-match under it, and the
/// frontend's existing `starts_with` rule in `suggestedContextRegex` must not be ported.
/// Neither the `BTreeMap` key order nor the number 8 is written a second time in TypeScript.
///
/// It takes the whole draft and not one row because allocation is a property of the SET:
/// resolution walks the map in key order and takes the first 8 that reach an agent. A command
/// receiving a single row could only answer by inventing what the rest of the set is, and the
/// only set available to it is the saved one, which is not the one the user is editing. With
/// an empty saved map, adding nine rows before Save would make all nine report that they run,
/// and then only eight would - a positive claim about a watcher that will not run, which is
/// the opposite of the fail-closed direction this feature takes everywhere else.
///
/// Both halves come from the draft and nothing comes from disk: no settings are read and **no
/// lock is taken**, so a preview can never contend with a save.
#[tauri::command]
pub async fn preview_watcher_reach(
    watchers: Vec<WatcherDraftEntry>,
    agents: Vec<WatcherAgentDraftEntry>,
) -> Result<Vec<WatcherReachRow>, String> {
    // Synchronous CPU over an input the caller controls: one pass is O(agents x total selector
    // ENTRIES), and nothing bounds either, so one row carrying ten thousand selector entries
    // costs what ten thousand rows carrying one each cost. Off the async worker, following
    // `preview_watcher_pattern` above. No cap is introduced here on purpose: the engine
    // already runs exactly this resolution over exactly this data every 200 ms, so a payload
    // big enough to make two debounced passes expensive is already costing five times as much
    // per second inside the tick, and a cap only on the preview would feel like a fix while
    // leaving the cost where it is.
    //
    // The command owns its inputs and takes no lock, so the move is a wrapper and nothing else.
    tokio::task::spawn_blocking(move || resolve_draft_reach(&watchers, &agents))
        .await
        .map_err(|e| e.to_string())
}

/// The two passes behind `preview_watcher_reach`, pure and off the runtime.
///
/// Reach and allocation are different questions, and each gets its own pass over the SAME
/// draft, so no counterfactual budget is computed and no set of answers can disagree with
/// itself:
///
/// - **Pass A, every row forced enabled**, supplies `entries`. Reach does not depend on any
///   other row, so forcing enablement changes no row's answer but its own presence, and a
///   disabled row still shows the agents its selector reaches - the state where the control is
///   needed most.
/// - **Pass B, every row at its real draft `enabled`**, is the engine's own resolution and
///   supplies `allocated`.
///
/// Running one forced-enabled pass PER ROW instead would let nine rows all report a slot on an
/// agent that has eight, because each row's own pass silently displaces a different one.
///
/// Fixed points that the editor cannot produce but the contract still defines: a duplicate id
/// means the later row wins when the map is built and both response rows report that one
/// resolution, and an empty id is a legal key that sorts first and is not special-cased.
fn resolve_draft_reach(
    watchers: &[WatcherDraftEntry],
    agents: &[WatcherAgentDraftEntry],
) -> Vec<WatcherReachRow> {
    use crate::config::settings::{WatcherConfig, WatcherEntry, WatcherMode};
    use crate::pty::watchers::{resolve_watchers, WatcherAgent};
    use std::collections::BTreeMap;

    let resolution_agents: Vec<WatcherAgent> = agents
        .iter()
        .map(|agent| WatcherAgent {
            id: agent.id.clone(),
            command: agent.command.clone(),
        })
        .collect();

    // `mode`, `pattern`, `dedupe` and `dedupeWindowMs` take no part in resolution and do not
    // travel, so they are placeholders here and are never read back out.
    let draft_map = |force_enabled: bool| -> BTreeMap<String, WatcherEntry> {
        watchers
            .iter()
            .map(|row| {
                (
                    row.id.clone(),
                    WatcherEntry::Valid(WatcherConfig {
                        enabled: force_enabled || row.enabled,
                        mode: WatcherMode::Occurrence,
                        pattern: String::new(),
                        commands: row.commands.clone(),
                        dedupe: Default::default(),
                        dedupe_window_ms: 0,
                        captured_against: None,
                    }),
                )
            })
            .collect()
    };

    let (reach_pass, _notices) = resolve_watchers(&resolution_agents, &draft_map(true));
    let (allocation_pass, _notices) = resolve_watchers(&resolution_agents, &draft_map(false));

    // Keyed by agent id and not iterated straight off `agents`, so a draft that names the same
    // agent twice produces one entry rather than two, matching the map `resolve_watchers`
    // itself builds.
    let mut agent_meta: BTreeMap<String, (String, String)> = BTreeMap::new();
    for agent in agents {
        let stem =
            crate::config::coding_agents_catalog::command_executable_basename(&agent.command)
                .unwrap_or_default();
        agent_meta.insert(agent.id.clone(), (agent.label.clone(), stem));
    }

    let mut rows = Vec::with_capacity(watchers.len());
    for row in watchers {
        let mut entries: Vec<WatcherReachEntry> = Vec::new();
        for (agent_id, (label, stem)) in &agent_meta {
            // Reach is `running` OR `over_budget`: together they hold everything whose selector
            // matches this agent.
            let reaches = reach_pass.get(agent_id).is_some_and(|resolution| {
                resolution.running.iter().any(|w| w.id == row.id)
                    || resolution.over_budget.iter().any(|id| id == &row.id)
            });
            if !reaches {
                continue;
            }
            let allocated = allocation_pass
                .get(agent_id)
                .is_some_and(|resolution| resolution.running.iter().any(|w| w.id == row.id));
            entries.push(WatcherReachEntry {
                agent_id: agent_id.clone(),
                agent_label: label.clone(),
                command_stem: stem.clone(),
                allocated,
            });
        }
        // `resolve_watchers` returns a `HashMap`, so a stable order has to be imposed here
        // rather than assumed: the Settings list must not reshuffle between keystrokes.
        entries.sort_by(|left, right| {
            left.agent_label
                .cmp(&right.agent_label)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
        });
        rows.push(WatcherReachRow {
            id: row.id.clone(),
            entries,
        });
    }
    rows
}

#[cfg(test)]
mod watcher_preview_tests {
    use super::*;
    use crate::config::settings::{AppSettings, SettingsState};
    use crate::errors::AppError;
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use crate::pty::watchers::{FrameStamp, ScreenFrame, ScreenRowsSince};

    /// One watcher row of the Settings draft. `preview_watcher_reach` takes the whole draft,
    /// so every reach test below builds a set and never a single row.
    fn draft(id: &str, enabled: bool, commands: Option<&[&str]>) -> WatcherDraftEntry {
        WatcherDraftEntry {
            id: id.to_string(),
            enabled,
            commands: commands.map(|list| list.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn draft_agent(id: &str, label: &str, command: &str) -> WatcherAgentDraftEntry {
        WatcherAgentDraftEntry {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
        }
    }

    /// The row of `id` in a reach response, which is addressed by id and never by position.
    fn row<'a>(rows: &'a [WatcherReachRow], id: &str) -> &'a WatcherReachRow {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("no reach row for '{id}'"))
    }

    fn allocated_on(rows: &[WatcherReachRow], id: &str, agent_id: &str) -> bool {
        row(rows, id)
            .entries
            .iter()
            .find(|entry| entry.agent_id == agent_id)
            .unwrap_or_else(|| panic!("watcher '{id}' does not reach agent '{agent_id}'"))
            .allocated
    }

    fn reached_agents(rows: &[WatcherReachRow], id: &str) -> Vec<String> {
        row(rows, id)
            .entries
            .iter()
            .map(|entry| entry.agent_id.clone())
            .collect()
    }

    fn settings_app(settings: AppSettings) -> tauri::App<tauri::test::MockRuntime> {
        let state: SettingsState = Arc::new(tokio::sync::RwLock::new(settings));
        tauri::test::mock_builder()
            .manage(state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a watcher preview test app")
    }

    /// A backend that paints one fixed screen and refuses everything else.
    struct FixedScreenBackend {
        rows: Vec<String>,
    }

    impl PtyBackend for FixedScreenBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn spawn(
            &self,
            _spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            Box::pin(async { Ok(()) })
        }
        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: Uuid,
            _data: &[u8],
        ) -> Result<(), AppError> {
            unreachable!("a preview must never write to a PTY")
        }
        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
            unreachable!("a preview must never resize a PTY")
        }
        fn kill(&self, _id: Uuid) -> Result<(), AppError> {
            unreachable!("a preview must never kill a session")
        }
        fn has_session(&self, _id: Uuid) -> bool {
            true
        }
        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }
        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }
        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::Unavailable
        }
        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }
        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }
        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
        fn screen_rows_since(&self, _id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
            ScreenRowsSince::Frame(ScreenFrame {
                rows: self.rows.clone(),
                wrapped: vec![false; self.rows.len()],
                cursor_row: 0,
                stamp: Some(FrameStamp {
                    sequence: 1,
                    rows: self.rows.len() as u16,
                    cols: 120,
                }),
            })
        }
    }

    fn pty_app(rows: &[&str]) -> (tauri::App<tauri::test::MockRuntime>, Uuid) {
        let backend = Arc::new(FixedScreenBackend {
            rows: rows.iter().map(|r| r.to_string()).collect(),
        });
        let manager = PtyManager::new_for_test(backend);
        let id = Uuid::new_v4();
        manager.record_route(id, SessionBackendKind::LocalProcess);
        let app = tauri::test::mock_builder()
            .manage(Arc::new(Mutex::new(manager)))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a pty preview test app");
        (app, id)
    }

    /// 9.5.65 - a `session_id` that is not a UUID is an error, exactly like
    /// `get_session_context`. Everything else about this command answers with values.
    #[test]
    fn a_session_id_that_is_not_a_uuid_is_rejected() {
        let app = settings_app(AppSettings::default());

        assert!(get_watcher_activity(app.handle().clone(), "nope".into(), None).is_err());
    }

    /// 9.5.64 - with no history managed at all - a test app, a build without the engine - the
    /// command answers with the EMPTY snapshot rather than an error. `warmedUp: false` is what
    /// tells the window it is looking at a session the engine has not reached, so it shows a
    /// neutral starting state instead of "no watcher reaches this agent".
    #[test]
    fn an_unmanaged_history_answers_with_the_empty_snapshot() {
        let app = settings_app(AppSettings::default());

        let snapshot = get_watcher_activity(app.handle().clone(), Uuid::new_v4().to_string(), None)
            .expect("never an error");

        assert!(snapshot.matches.is_empty());
        assert!(!snapshot.warmed_up);
        assert!(!snapshot.truncated);
        assert_eq!(snapshot.last_seq, 0);
        assert_eq!(snapshot.possibly_missed_frames, 0);
        assert!(snapshot.active_watchers.is_empty());
    }

    /// 9.5.63 - the command hands `limit` straight to the ring, which trims from the NEW end
    /// and keeps the order.
    #[test]
    fn the_command_reads_the_ring_and_honours_the_limit() {
        use crate::pty::watchers::history::{SessionStatus, WatcherHistory, WatcherHistoryState};
        use crate::pty::watchers::WatcherMatchPayload;

        let id = Uuid::new_v4();
        let history: WatcherHistoryState = Arc::new(WatcherHistory::default());
        history.publish(id, SessionStatus::default());
        for seq in 1..=5u64 {
            history.record(
                id,
                &[WatcherMatchPayload {
                    session_id: id.to_string(),
                    seq,
                    watcher_id: "w".to_string(),
                    mode: crate::pty::watchers::WatcherMode::Occurrence,
                    at: chrono::Utc::now(),
                    captures: Vec::new(),
                    row: format!("row {seq}"),
                    row_truncated: false,
                }],
            );
        }
        let app = tauri::test::mock_builder()
            .manage(history)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a watcher activity test app");

        let all = get_watcher_activity(app.handle().clone(), id.to_string(), None).expect("all");
        assert_eq!(all.matches.len(), 5);
        assert!(all.warmed_up);

        let recent =
            get_watcher_activity(app.handle().clone(), id.to_string(), Some(2)).expect("recent");
        assert_eq!(
            recent.matches.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![4, 5]
        );
        assert_eq!(recent.last_seq, 5);
    }

    /// 9.6.86 (first case) - **the common case**: a user writes a regex in Settings with no
    /// agent session running. It compiles and says so, and says explicitly that it did not
    /// look at anything - which is not the same as having looked and found nothing.
    #[tokio::test]
    async fn a_compile_only_preview_reports_compiles_without_sampling() {
        let app = settings_app(AppSettings::default());

        let preview = preview_watcher_pattern(app.handle().clone(), None, r"Read \((.+)\)".into())
            .await
            .expect("compile-only preview never fails");

        assert!(preview.compiles);
        assert!(preview.error.is_none());
        assert!(!preview.sampled);
        assert_eq!((preview.matched_rows, preview.total_rows), (0, 0));
        assert!(preview.samples.is_empty());
        assert!(!preview.captures_volatile);
    }

    /// 9.6.86 (fourth case) - a pattern that does not compile is a RESULT, not a command
    /// failure. The Settings control needs the message to show the user.
    #[tokio::test]
    async fn an_uncompilable_pattern_returns_a_result_rather_than_an_error() {
        let app = settings_app(AppSettings::default());

        let preview = preview_watcher_pattern(app.handle().clone(), None, r"Read \((.+".into())
            .await
            .expect("a bad pattern must not fail the command");

        assert!(!preview.compiles);
        assert!(preview.error.is_some());
        assert!(!preview.sampled);
    }

    /// 9.6.86 (second case) - with a live session, the preview reports matched rows against
    /// total LOGICAL rows and shows at most three of them.
    #[tokio::test]
    async fn a_preview_against_a_live_session_reports_matches_against_total_rows() {
        let (app, id) = pty_app(&[
            "Read (a.rs)",
            "idle",
            "Read (b.rs)",
            "Read (c.rs)",
            "Read (d.rs)",
        ]);

        let preview = preview_watcher_pattern(
            app.handle().clone(),
            Some(id.to_string()),
            r"Read \((.+)\)".into(),
        )
        .await
        .expect("preview");

        assert!(preview.sampled);
        assert_eq!(preview.total_rows, 5);
        assert_eq!(preview.matched_rows, 4);
        assert_eq!(preview.samples.len(), WATCHER_PREVIEW_SAMPLES);
        assert_eq!(preview.samples[0], "Read (a.rs)");
        assert!(
            !preview.captures_volatile,
            "the same screen twice cannot be volatile"
        );
    }

    /// 9.6.87 - **the pattern that looks fine and is not.** A regex capturing a clock matches
    /// one row of thirty, so `matchedRows` says nothing is wrong, and in `state` mode it emits
    /// five events a second forever. The two samples a second apart are what catch it.
    ///
    /// Takes about a second in real time by construction: the interval between the samples IS
    /// the measurement.
    #[tokio::test]
    async fn a_pattern_capturing_a_clock_is_reported_as_volatile() {
        struct TickingClockBackend {
            reads: std::sync::atomic::AtomicUsize,
        }

        impl PtyBackend for TickingClockBackend {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn spawn(
                &self,
                _spec: BackendSpawnSpec,
            ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
                Box::pin(async { Ok(()) })
            }
            fn write(
                &self,
                _authority: &crate::pty::manager::BackendWriteAuthority,
                _id: Uuid,
                _data: &[u8],
            ) -> Result<(), AppError> {
                unreachable!("a preview must never write to a PTY")
            }
            fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
                unreachable!("a preview must never resize a PTY")
            }
            fn kill(&self, _id: Uuid) -> Result<(), AppError> {
                unreachable!("a preview must never kill a session")
            }
            fn has_session(&self, _id: Uuid) -> bool {
                true
            }
            fn get_screen_snapshot(
                &self,
                _id: Uuid,
            ) -> Option<crate::pty::output::PtyScreenSnapshot> {
                None
            }
            fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
                None
            }
            fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
                crate::pty::context_scrape::ScreenRowsRead::Unavailable
            }
            fn register_response_watcher(
                &self,
                _session_id: Uuid,
                _request_id: String,
                _response_dir: std::path::PathBuf,
            ) {
            }
            fn terminate_job_for_session(&self, _id: Uuid) -> bool {
                false
            }
            fn kill_all_jobs(&self) -> (usize, usize) {
                (0, 0)
            }
            fn screen_rows_since(&self, _id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
                let tick = self
                    .reads
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let rows = vec!["idle".to_string(), format!("elapsed 00:0{tick}")];
                ScreenRowsSince::Frame(ScreenFrame {
                    wrapped: vec![false; rows.len()],
                    cursor_row: 0,
                    stamp: Some(FrameStamp {
                        sequence: tick as u64 + 1,
                        rows: rows.len() as u16,
                        cols: 120,
                    }),
                    rows,
                })
            }
        }

        let backend = Arc::new(TickingClockBackend {
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let manager = PtyManager::new_for_test(backend);
        let id = Uuid::new_v4();
        manager.record_route(id, SessionBackendKind::LocalProcess);
        let app = tauri::test::mock_builder()
            .manage(Arc::new(Mutex::new(manager)))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a clock preview test app");

        let preview = preview_watcher_pattern(
            app.handle().clone(),
            Some(id.to_string()),
            r"elapsed (\d\d:\d\d)".into(),
        )
        .await
        .expect("preview");

        assert!(preview.sampled);
        assert_eq!(
            preview.matched_rows, 1,
            "one row of two: nothing looks wrong"
        );
        assert!(
            preview.captures_volatile,
            "...and yet in state mode this would emit five events a second, forever"
        );
    }

    /// 9.6.86 (third case) - a session that cannot be read reports `sampled: false` and KEEPS
    /// the compile result. "Could not look" must never read as "looked and found nothing".
    #[tokio::test]
    async fn a_session_that_cannot_be_read_keeps_the_compile_result_and_does_not_claim_a_sample() {
        let app = settings_app(AppSettings::default());

        let preview = preview_watcher_pattern(
            app.handle().clone(),
            Some(Uuid::new_v4().to_string()),
            "Read".into(),
        )
        .await
        .expect("preview");

        assert!(preview.compiles);
        assert!(!preview.sampled);
    }

    #[tokio::test]
    async fn a_session_id_that_is_not_a_uuid_is_an_error() {
        let app = settings_app(AppSettings::default());

        assert!(preview_watcher_pattern(
            app.handle().clone(),
            Some("not-a-uuid".into()),
            "x".into()
        )
        .await
        .is_err());
    }

    /// 9.4.58a - nine enabled selectorless rows against one agent: the first eight in ID order
    /// hold a slot and the ninth does not, with nothing on disk contributing.
    ///
    /// The rows are sent in an order deliberately different from lexicographic, so an
    /// implementation that honours request order instead of `BTreeMap` key order fails here.
    /// Its failure mode is the UI telling the user that a watcher holds a slot it does not.
    #[tokio::test]
    async fn a_draft_of_nine_rows_allocates_the_first_eight_in_id_order() {
        let ids = ["w3", "w9", "w1", "w7", "w5", "w2", "w8", "w4", "w6"];
        let rows = preview_watcher_reach(
            ids.iter().map(|id| draft(id, true, None)).collect(),
            vec![draft_agent("a1", "Claude", "claude")],
        )
        .await
        .expect("reach");

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ids,
            "one response row per request row, in request order"
        );
        for id in ["w1", "w2", "w3", "w4", "w5", "w6", "w7", "w8"] {
            assert!(allocated_on(&rows, id, "a1"), "{id} is within the budget");
        }
        assert!(
            !allocated_on(&rows, "w9", "a1"),
            "w9 sorts ninth and the agent has eight slots"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.entries.iter().any(|entry| entry.allocated))
                .count(),
            crate::pty::watchers::WATCHERS_PER_AGENT_BUDGET,
            "no draft yields more allocated rows on one agent than that agent has slots"
        );
    }

    /// 9.4.58b - the draft is the whole input. A row the user deleted contributes nothing to
    /// any agent's budget, however many rows the saved map still holds.
    #[tokio::test]
    async fn rows_absent_from_the_draft_consume_no_budget() {
        let rows = preview_watcher_reach(
            ["w1", "w2", "w3", "w4"]
                .iter()
                .map(|id| draft(id, true, None))
                .collect(),
            vec![draft_agent("a1", "Claude", "claude")],
        )
        .await
        .expect("reach");

        assert_eq!(rows.len(), 4);
        for id in ["w1", "w2", "w3", "w4"] {
            assert!(allocated_on(&rows, id, "a1"));
        }
    }

    /// 9.4.58c - the displacement fixture: one disabled row plus eight enabled ones.
    ///
    /// This is the case a per-row forced-enabled pass gets wrong. There, `a`'s own pass
    /// displaced `i` while every other row's pass displaced nobody, so all nine reported
    /// themselves in budget on an agent with eight slots.
    #[tokio::test]
    async fn a_disabled_row_reaches_everything_and_allocates_nothing() {
        let mut watchers = vec![draft("a", false, None)];
        for id in ["b", "c", "d", "e", "f", "g", "h", "i"] {
            watchers.push(draft(id, true, None));
        }
        let rows = preview_watcher_reach(watchers, vec![draft_agent("a1", "Claude", "claude")])
            .await
            .expect("reach");

        assert_eq!(
            reached_agents(&rows, "a"),
            vec!["a1"],
            "a disabled row still reports the agents its selector reaches"
        );
        assert!(
            !allocated_on(&rows, "a", "a1"),
            "a disabled row holds no slot, and the editor must call that 'disabled', not 'budget'"
        );
        for id in ["b", "c", "d", "e", "f", "g", "h", "i"] {
            assert!(
                allocated_on(&rows, id, "a1"),
                "{id} is one of the eight enabled rows and the disabled row displaces none of them"
            );
        }
        assert_eq!(
            rows.iter()
                .filter(|row| row.entries.iter().any(|entry| entry.allocated))
                .count(),
            crate::pty::watchers::WATCHERS_PER_AGENT_BUDGET
        );
    }

    /// 9.4.58d - reach does not depend on enablement. The `entries` of a row are identical
    /// whether it is enabled or disabled, all else equal; only `allocated` moves.
    #[tokio::test]
    async fn reach_does_not_depend_on_enablement() {
        let agents = vec![
            draft_agent("a1", "Claude", "claude"),
            draft_agent("a2", "Codex", "codex"),
        ];
        let others = || {
            vec![
                draft("w1", true, Some(&["claude"])),
                draft("w2", true, None),
            ]
        };

        let mut enabled = others();
        enabled.push(draft("w3", true, Some(&["claude", "codex"])));
        let enabled = preview_watcher_reach(enabled, agents.clone())
            .await
            .expect("reach");

        let mut disabled = others();
        disabled.push(draft("w3", false, Some(&["claude", "codex"])));
        let disabled = preview_watcher_reach(disabled, agents)
            .await
            .expect("reach");

        assert_eq!(reached_agents(&enabled, "w3"), vec!["a1", "a2"]);
        assert_eq!(
            reached_agents(&disabled, "w3"),
            reached_agents(&enabled, "w3")
        );
        assert!(allocated_on(&enabled, "w3", "a1"));
        assert!(!allocated_on(&disabled, "w3", "a1"));
        assert_eq!(
            reached_agents(&disabled, "w1"),
            reached_agents(&enabled, "w1"),
            "and no other row's reach moved either"
        );
    }

    /// 9.4.58e - the selector rules, unchanged: absent reaches every agent, `[]` reaches none,
    /// a selector that does not tokenize skips the whole watcher, and the reported stem is the
    /// AGENT's. In both reach-nobody cases the response row is still present, with no entries.
    #[tokio::test]
    async fn the_selector_rules_survive_the_draft_shape() {
        let rows = preview_watcher_reach(
            vec![
                draft("all", true, None),
                draft("none", true, Some(&[])),
                draft("broken", true, Some(&["claude", "   "])),
                draft("claude-only", true, Some(&["claude"])),
                draft("typo", true, Some(&["gemni"])),
            ],
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Claude Sandbox", r"C:\rt\claude.cmd"),
                draft_agent("a3", "Codex", "codex"),
                draft_agent("a4", "Pi via Claude", "pi --provider claude"),
            ],
        )
        .await
        .expect("reach");

        assert_eq!(reached_agents(&rows, "all"), vec!["a1", "a2", "a3", "a4"]);
        assert!(
            row(&rows, "none").entries.is_empty(),
            "an empty selector reaches nobody and is the opposite of an absent one"
        );
        assert!(
            row(&rows, "broken").entries.is_empty(),
            "one unreadable selector entry skips the WHOLE watcher, never 'reaches everybody'"
        );
        assert!(row(&rows, "typo").entries.is_empty());
        assert_eq!(reached_agents(&rows, "claude-only"), vec!["a1", "a2"]);

        let claude_only = &row(&rows, "claude-only").entries;
        assert_eq!(claude_only[0].agent_label, "Claude");
        assert_eq!(
            claude_only[1].command_stem, "claude",
            "the stem reported is the agent's, resolved through the one rule in the tree"
        );
    }

    /// 9.4.58f - the agents come from the draft too. Each of these three is a case where
    /// resolving against the saved agent list would have answered about a state the user had
    /// already left, and two of the three would have over-reported.
    #[tokio::test]
    async fn the_agent_half_of_the_draft_decides_the_reach() {
        let watchers = || {
            vec![
                draft("on-claude", true, Some(&["claude"])),
                draft("on-codex", true, Some(&["codex"])),
            ]
        };

        let before = preview_watcher_reach(
            watchers(),
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Second", "claude"),
            ],
        )
        .await
        .expect("reach");
        assert_eq!(reached_agents(&before, "on-claude"), vec!["a1", "a2"]);
        assert!(row(&before, "on-codex").entries.is_empty());

        // The command changed in the draft: the agent leaves one watcher and joins the other.
        let retargeted = preview_watcher_reach(
            watchers(),
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Second", "codex"),
            ],
        )
        .await
        .expect("reach");
        assert_eq!(reached_agents(&retargeted, "on-claude"), vec!["a1"]);
        assert_eq!(reached_agents(&retargeted, "on-codex"), vec!["a2"]);

        // The agent was deleted in the draft: it is named by nobody.
        let removed =
            preview_watcher_reach(watchers(), vec![draft_agent("a1", "Claude", "claude")])
                .await
                .expect("reach");
        assert_eq!(reached_agents(&removed, "on-claude"), vec!["a1"]);

        // The agent was added in the draft: it is reached before it is ever saved.
        let added = preview_watcher_reach(
            watchers(),
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Second", "claude"),
                draft_agent("a3", "Third", "codex"),
            ],
        )
        .await
        .expect("reach");
        assert_eq!(reached_agents(&added, "on-claude"), vec!["a1", "a2"]);
        assert_eq!(reached_agents(&added, "on-codex"), vec!["a3"]);
    }

    /// 9.4.58j - the fixed points of the contract that the editor cannot produce but the
    /// command still has to define: duplicate ids, an empty id, response order, entry order.
    #[tokio::test]
    async fn the_defined_behavior_for_drafts_the_editor_cannot_produce() {
        let rows = preview_watcher_reach(
            vec![
                draft("dup", true, Some(&["codex"])),
                draft("", true, None),
                draft("dup", true, Some(&["claude"])),
            ],
            vec![
                // Labels out of alphabetical order and one pair sharing a label, so the
                // entry sort is exercised on both keys.
                draft_agent("a2", "Zed", "claude"),
                draft_agent("a3", "Alpha", "claude"),
                draft_agent("a1", "Alpha", "claude"),
            ],
        )
        .await
        .expect("reach");

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["dup", "", "dup"],
            "one response row per request row, in request order, id carried back"
        );
        assert_eq!(
            reached_agents(&rows, "dup"),
            vec!["a1", "a3", "a2"],
            "entries sort by label with the agent id as tie-break"
        );
        assert_eq!(
            rows[0].entries.len(),
            rows[2].entries.len(),
            "a duplicate id is later-wins in the map and BOTH response rows report that one \
             resolution"
        );
        assert_eq!(reached_agents(&rows, ""), vec!["a1", "a3", "a2"]);
        assert!(
            allocated_on(&rows, "", "a1"),
            "an empty id is a legal key that sorts first, and is not special-cased"
        );
    }

    /// 9.4.58l - allocation is slot assignment and not a promise of output.
    ///
    /// The pattern does not travel to the reach command at all, so a row whose regex does not
    /// compile is allocated a slot and is inert. The two dimensions are asserted together, so
    /// nobody later reads `allocated` as a promise that the watcher will emit.
    #[tokio::test]
    async fn an_uncompilable_pattern_is_allocated_and_inert() {
        let app = settings_app(AppSettings::default());

        let rows = preview_watcher_reach(
            vec![draft("broken-regex", true, None)],
            vec![draft_agent("a1", "Claude", "claude")],
        )
        .await
        .expect("reach");
        assert!(
            allocated_on(&rows, "broken-regex", "a1"),
            "an enabled row within budget holds its slot whatever its pattern is"
        );

        let compile = preview_watcher_pattern(app.handle().clone(), None, "Read (".to_string())
            .await
            .expect("preview");
        assert!(
            !compile.compiles,
            "and the other dimension is answered next to it, by the row's own pattern preview"
        );
        assert!(compile.error.is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use crate::config::coordinator_clocks::{CoordinatorClocks, CoordinatorClocksState};
    use crate::session::manager::SessionManager;
    use crate::session::session::SessionRepo;

    struct FreshIntentFixture {
        app: tauri::App<tauri::test::MockRuntime>,
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        clocks: CoordinatorClocksState,
        session_id: Uuid,
        fqn: String,
    }

    fn user_input_test_app(
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        clocks: CoordinatorClocksState,
    ) -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .manage(session_mgr)
            .manage(clocks)
            .manage(crate::pty::input_activity::new_state())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build user input test app")
    }

    async fn fresh_intent_fixture() -> FreshIntentFixture {
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
        let app = user_input_test_app(session_mgr.clone(), clocks.clone());
        let cwd = "C:/ac-test/project/.ac/wg-871-dev-team/__agent_tech-lead".to_string();
        let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
        let session = {
            let mgr = session_mgr.read().await;
            mgr.create_session(
                "codex".to_string(),
                Vec::new(),
                cwd,
                None,
                None,
                Vec::<SessionRepo>::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create coordinator session")
        };

        {
            let mgr = session_mgr.read().await;
            mgr.set_start_fresh_on_restore(session.id, true).await;
        }
        {
            let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
            assert!(guard.mark_start_fresh(&fqn, chrono::Utc::now()));
        }

        FreshIntentFixture {
            app,
            session_mgr,
            clocks,
            session_id: session.id,
            fqn,
        }
    }

    async fn record_fresh(f: &FreshIntentFixture) -> bool {
        let mgr = f.session_mgr.read().await;
        mgr.get_session(f.session_id)
            .await
            .expect("session should exist")
            .start_fresh_on_restore
    }

    fn mirror_fresh(f: &FreshIntentFixture) -> bool {
        f.clocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .start_fresh_at(&f.fqn)
            .is_some()
    }

    fn inject_continue_after_restore(start_fresh_on_restore: bool) -> bool {
        !start_fresh_on_restore
    }

    #[tokio::test]
    async fn restart_non_substantive_terminal_writes_keep_restore_fresh() {
        let f = fresh_intent_fixture().await;
        for chunk in [
            b"\x1b[I".as_slice(),
            b"\x1b[A".as_slice(),
            b"\x1b]11;rgb:1234/5678/9abc\x07".as_slice(),
            b"\r".as_slice(),
        ] {
            note_user_message_to_session(
                f.app.handle(),
                f.session_id,
                UserInputSource::Terminal(chunk),
            )
            .await;
            assert!(record_fresh(&f).await);
            assert!(mirror_fresh(&f));
        }

        assert!(!inject_continue_after_restore(record_fresh(&f).await));
    }

    #[tokio::test]
    async fn restart_substantive_terminal_prompt_allows_resume_on_restore() {
        let f = fresh_intent_fixture().await;
        note_user_message_to_session(
            f.app.handle(),
            f.session_id,
            UserInputSource::Terminal(b"do the thing\r"),
        )
        .await;

        assert!(!record_fresh(&f).await);
        assert!(!mirror_fresh(&f));
        assert!(inject_continue_after_restore(record_fresh(&f).await));
    }

    #[test]
    fn boundary_metadata_failure_dominates_applied_and_unchanged_outcomes() {
        use BoundaryMetadataOutcome as O;
        assert_eq!(
            combine_boundary_metadata(O::Applied, O::Unchanged),
            O::Applied
        );
        assert_eq!(
            combine_boundary_metadata(O::Unchanged, O::Unchanged),
            O::Unchanged
        );
        assert_eq!(combine_boundary_metadata(O::Failed, O::Applied), O::Failed);
        assert_eq!(combine_boundary_metadata(O::Applied, O::Failed), O::Failed);
    }

    #[test]
    fn terminal_output_identifiers_are_strictly_canonical() {
        let id = Uuid::new_v4();
        let canonical = id.hyphenated().to_string();
        assert_eq!(parse_canonical_session_id(&canonical), Ok(id));
        for invalid in [
            canonical.to_uppercase(),
            id.simple().to_string(),
            format!("{{{canonical}}}"),
            format!(" {canonical}"),
            "not-a-uuid".to_string(),
        ] {
            assert_eq!(
                parse_canonical_session_id(&invalid),
                Err("invalidSessionId".to_string())
            );
        }

        assert_eq!(parse_document_epoch("1"), Ok(1));
        assert_eq!(parse_document_epoch(&u64::MAX.to_string()), Ok(u64::MAX));
        for invalid in [
            "",
            "0",
            "00",
            "01",
            "+1",
            "-1",
            " 1",
            "1 ",
            "18446744073709551616",
        ] {
            assert_eq!(
                parse_document_epoch(invalid),
                Err("invalidDocumentEpoch".to_string())
            );
        }
        assert_eq!(
            validate_attach_generation(0),
            Err("invalidAttachGeneration".to_string())
        );
        assert_eq!(validate_attach_generation(u32::MAX), Ok(u32::MAX));
    }

    fn observation_fixture(id: Uuid, stage: &str, outcome: &str) -> PtyTerminalAttachObservation {
        serde_json::from_value(serde_json::json!({
            "sessionId": id.to_string(),
            "stage": stage,
            "documentEpoch": "9",
            "xtermInstanceId": 1,
            "viewKind": "embedded",
            "transitionKind": "initial",
            "attachGeneration": 1,
            "sequence": 1,
            "outcome": outcome,
        }))
        .expect("valid observation fixture")
    }

    fn settled_observation_fixture(
        id: Uuid,
        renderer: PtyTerminalAttachRenderer,
        context_state: PtyTerminalAttachContextState,
    ) -> PtyTerminalAttachObservation {
        let mut observation = observation_fixture(id, "settled", "success");
        observation.parser_rows = Some(24);
        observation.parser_cols = Some(81);
        observation.conpty_rows = Some(24);
        observation.conpty_cols = Some(81);
        observation.snapshot_rows = Some(24);
        observation.snapshot_cols = Some(81);
        observation.xterm_rows = Some(24);
        observation.xterm_cols = Some(81);
        observation.viewport_y = Some(0);
        observation.base_y = Some(0);
        observation.buffer_length = Some(24);
        observation.visible_row_count = Some(24);
        observation.missing_visible_row_count = Some(0);
        observation.renderer = Some(renderer);
        observation.context_state = Some(context_state);
        observation.container_connected = Some(true);
        observation.xterm_connected = Some(true);
        observation.screen_connected = Some(true);
        observation.element_width = Some(810);
        observation.element_height = Some(480);
        observation.screen_width = Some(810);
        observation.screen_height = Some(480);
        if renderer == PtyTerminalAttachRenderer::Webgl {
            observation.canvas_width = Some(810);
            observation.canvas_height = Some(480);
        }
        observation.replay_barrier_completed = Some(true);
        observation.retained_barrier_completed = Some(true);
        observation.grid_agreement = Some(true);
        observation.resize_confirmed = Some(true);
        observation.visible_rows_present = Some(true);
        observation.bottom_position_satisfied = Some(true);
        observation.expected_active_screen_has_text = Some(true);
        observation.observed_active_screen_has_text = Some(true);
        observation.expected_bottom_line_has_text = Some(false);
        observation.observed_bottom_line_has_text = Some(false);
        observation
    }

    #[test]
    fn terminal_attach_observation_rejects_hostile_shapes_and_contradictions() {
        let id = Uuid::new_v4();
        let base = serde_json::json!({
            "sessionId": id.to_string(),
            "stage": "postWrite",
            "documentEpoch": "9",
            "xtermInstanceId": 1,
            "viewKind": "embedded",
            "transitionKind": "initial",
            "attachGeneration": 1,
            "sequence": 1,
            "outcome": "success",
        });
        for (field, value) in [
            ("stage", serde_json::json!("unknown")),
            ("viewKind", serde_json::json!("other")),
            ("transitionKind", serde_json::json!("other")),
        ] {
            let mut value_object = base.as_object().expect("base object").clone();
            value_object.insert(field.to_string(), value);
            assert!(
                serde_json::from_value::<PtyTerminalAttachObservation>(serde_json::Value::Object(
                    value_object
                ))
                .is_err(),
                "hostile field {field} must be rejected"
            );
        }
        let mut unsafe_sequence = observation_fixture(id, "postWrite", "success");
        unsafe_sequence.sequence = 9_007_199_254_740_992;
        assert_eq!(
            validate_observation_shape(&unsafe_sequence, "main"),
            Err("unsafeObservationSequence".to_string())
        );
        let mut zero_generation = observation_fixture(id, "postWrite", "success");
        zero_generation.attach_generation = 0;
        assert_eq!(
            validate_observation_shape(&zero_generation, "main"),
            Err("invalidObservationIdentity".to_string())
        );
        let mut duration = observation_fixture(id, "postWrite", "success");
        duration.total_micros = Some(60_000_001);
        assert_eq!(
            validate_observation_shape(&duration, "main"),
            Err("observationDurationOutOfRange".to_string())
        );
        let mut pixels = observation_fixture(id, "postWrite", "success");
        pixels.element_width = Some(131_073);
        assert_eq!(
            validate_observation_shape(&pixels, "main"),
            Err("observationPixelOutOfRange".to_string())
        );
        let mut extra = base.as_object().expect("base object").clone();
        extra.insert("terminalBytes".to_string(), serde_json::json!([1, 2, 3]));
        assert!(
            serde_json::from_value::<PtyTerminalAttachObservation>(serde_json::Value::Object(
                extra
            ))
            .is_err()
        );

        let mut renderer = observation_fixture(id, "postWrite", "success");
        renderer.renderer = Some(PtyTerminalAttachRenderer::Dom);
        renderer.context_state = Some(PtyTerminalAttachContextState::Active);
        assert_eq!(
            validate_observation_shape(&renderer, "main"),
            Err("observationRendererContextInvalid".to_string())
        );

        let mut buffer = observation_fixture(id, "postWrite", "success");
        buffer.viewport_y = Some(0);
        buffer.base_y = Some(1);
        buffer.buffer_length = Some(10);
        buffer.xterm_rows = Some(24);
        buffer.xterm_cols = Some(81);
        assert_eq!(
            validate_observation_shape(&buffer, "main"),
            Err("observationBufferInvariantFailed".to_string())
        );

        let sparse = observation_fixture(id, "settled", "success");
        assert_eq!(
            validate_observation_shape(&sparse, "main"),
            Err("observationSettlementInvariantFailed".to_string())
        );

        let mut semantic = settled_observation_fixture(
            id,
            PtyTerminalAttachRenderer::Webgl,
            PtyTerminalAttachContextState::Active,
        );
        semantic.observed_active_screen_has_text = Some(false);
        assert_eq!(
            validate_observation_shape(&semantic, "main"),
            Err("observationSettlementInvariantFailed".to_string())
        );

        let mut visible_rows = settled_observation_fixture(
            id,
            PtyTerminalAttachRenderer::Webgl,
            PtyTerminalAttachContextState::Active,
        );
        visible_rows.visible_row_count = Some(23);
        assert_eq!(
            validate_observation_shape(&visible_rows, "main"),
            Err("observationSettlementInvariantFailed".to_string())
        );

        let mut grid = settled_observation_fixture(
            id,
            PtyTerminalAttachRenderer::Webgl,
            PtyTerminalAttachContextState::Active,
        );
        grid.conpty_rows = Some(27);
        assert_eq!(
            validate_observation_shape(&grid, "main"),
            Err("observationSettlementInvariantFailed".to_string())
        );

        let mut claimed_grid = settled_observation_fixture(
            id,
            PtyTerminalAttachRenderer::Webgl,
            PtyTerminalAttachContextState::Active,
        );
        claimed_grid.grid_agreement = Some(false);
        assert_eq!(
            validate_observation_shape(&claimed_grid, "main"),
            Err("observationSettlementInvariantFailed".to_string())
        );

        let mut webgl_without_canvas = settled_observation_fixture(
            id,
            PtyTerminalAttachRenderer::Webgl,
            PtyTerminalAttachContextState::Active,
        );
        webgl_without_canvas.canvas_width = None;
        webgl_without_canvas.canvas_height = None;
        assert_eq!(
            validate_observation_shape(&webgl_without_canvas, "main"),
            Err("observationSettlementInvariantFailed".to_string())
        );

        for context_state in [
            PtyTerminalAttachContextState::Lost,
            PtyTerminalAttachContextState::Unavailable,
        ] {
            let dom_without_canvas =
                settled_observation_fixture(id, PtyTerminalAttachRenderer::Dom, context_state);
            assert_eq!(
                validate_observation_shape(&dom_without_canvas, "main"),
                Ok((id, 9))
            );
        }
    }

    #[test]
    fn terminal_attach_observation_validates_and_escapes_labels() {
        let id = Uuid::new_v4();
        let observation = observation_fixture(id, "postWrite", "success");
        assert_eq!(
            validate_observation_shape(&observation, ""),
            Err("invalidWebviewLabel".to_string())
        );
        assert_eq!(
            validate_observation_shape(&observation, &"x".repeat(257)),
            Err("invalidWebviewLabel".to_string())
        );
        assert_eq!(
            validate_observation_shape(&observation, "main"),
            Ok((id, 9))
        );
        let injected = "main\nsecret";
        let rendered = render_terminal_attach_observation(&observation, id, 9, injected);
        assert!(!rendered.contains('\n'));
        assert!(rendered.contains("\\n"));
        assert!(!rendered.contains("terminalBytes"));
        assert!(!rendered.contains("commandText"));
    }

    #[test]
    fn terminal_attach_observation_outcome_levels_and_codes_are_closed() {
        assert!(PtyTerminalAttachOutcome::Success.is_debug());
        assert!(PtyTerminalAttachOutcome::Stale.is_debug());
        assert!(!PtyTerminalAttachOutcome::Timeout.is_debug());
        assert!(!PtyTerminalAttachOutcome::InvariantFailed.is_debug());
        assert_eq!(
            PtyTerminalAttachOutcome::SeedlessParserPoisoned.code(),
            "seedlessParserPoisoned"
        );
    }

    #[test]
    fn terminal_output_activation_serializes_the_exact_camel_case_contract() {
        use crate::pty::output::{
            PtyTerminalActiveBuffer, PtyTerminalAlternateEntryMode,
            PtyTerminalHistoryTruncationReason, PtyTerminalReplayStage,
        };

        let fanout = crate::pty::output::SessionIoFanout::new_with_isolated_replay_budget(
            Arc::new(Mutex::new(HashMap::new())),
            crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {}),
            None,
        );
        let id = Uuid::new_v4();
        let token = fanout
            .register_session_for_test(id, crate::session::profile::IdleTuning::DEFAULT, 6, 20)
            .expect("register serialization session");
        fanout.handle_output(
            &token,
            &id.to_string(),
            b"normal\r\n\x1b[?1049halt".to_vec(),
        );
        let activation = fanout
            .activate_terminal_output(id, "main", true, 42, 7)
            .expect("activation");
        let payload = PtyTerminalOutputActivationPayload {
            snapshot: activation.snapshot,
            seedless_reason: activation.seedless_reason,
            attach_generation: activation.attach_generation,
            document_epoch: activation.document_epoch.to_string(),
        };
        let serialized = serde_json::to_value(payload).expect("serialize activation");
        let envelope = serialized.as_object().expect("activation object");
        assert_eq!(envelope.get("attachGeneration").unwrap(), 7);
        assert_eq!(envelope.get("documentEpoch").unwrap(), "42");
        assert!(!envelope.contains_key("seedlessReason"));
        assert!(!envelope.contains_key("attach_generation"));
        assert!(!envelope.contains_key("document_epoch"));

        let snapshot = envelope
            .get("snapshot")
            .and_then(serde_json::Value::as_object)
            .expect("snapshot object");
        for required in [
            "replayData",
            "rows",
            "cols",
            "sequence",
            "activeBuffer",
            "alternateEntryMode",
            "replayStage",
            "historyIncluded",
            "historyTruncated",
            "historyTruncationReason",
            "historyBoundaryHardened",
            "normalScreenIncluded",
            "retainedHistoryRows",
            "includedHistoryRows",
            "semanticHistoryBytes",
            "replayBytes",
            "pendingParserBytes",
            "activeScreenHasText",
            "activeBottomLineHasText",
        ] {
            assert!(snapshot.contains_key(required), "missing {required}");
        }
        assert_eq!(snapshot.get("rows").unwrap(), 6);
        assert_eq!(snapshot.get("cols").unwrap(), 20);
        assert_eq!(snapshot.get("sequence").unwrap(), 1);
        assert_eq!(snapshot.get("activeBuffer").unwrap(), "alternate");
        assert_eq!(snapshot.get("alternateEntryMode").unwrap(), "mode1049");
        assert!(snapshot.get("replayData").unwrap().is_array());
        assert!(!snapshot.contains_key("replay_data"));

        let seedless = PtyTerminalOutputActivationPayload {
            snapshot: None,
            seedless_reason: Some(PtyTerminalSeedlessReason::SeedlessSequenceUnsafe),
            attach_generation: u32::MAX,
            document_epoch: u64::MAX.to_string(),
        };
        let serialized = serde_json::to_value(seedless).expect("serialize seedless activation");
        let envelope = serialized.as_object().expect("seedless object");
        assert!(!envelope.contains_key("snapshot"));
        assert_eq!(
            envelope.get("seedlessReason").unwrap(),
            "seedlessSequenceUnsafe"
        );
        assert_eq!(envelope.get("attachGeneration").unwrap(), u32::MAX);
        assert_eq!(
            envelope.get("documentEpoch").unwrap(),
            &serde_json::Value::String(u64::MAX.to_string())
        );

        for (value, expected) in [
            (
                PtyTerminalSeedlessReason::SeedlessParserUnavailable,
                "seedlessParserUnavailable",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessParserPoisoned,
                "seedlessParserPoisoned",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessContinuationUnsafe,
                "seedlessContinuationUnsafe",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessInvalidGrid,
                "seedlessInvalidGrid",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessResizeFailed,
                "seedlessResizeFailed",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded,
                "seedlessResourceLimitExceeded",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessReplayCapExceeded,
                "seedlessReplayCapExceeded",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessSequenceUnsafe,
                "seedlessSequenceUnsafe",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessCaptureFailed,
                "seedlessCaptureFailed",
            ),
            (
                PtyTerminalSeedlessReason::SeedlessEncodeFailed,
                "seedlessEncodeFailed",
            ),
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
        for (value, expected) in [
            (PtyTerminalActiveBuffer::Normal, "normal"),
            (PtyTerminalActiveBuffer::Alternate, "alternate"),
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
        for (value, expected) in [
            (PtyTerminalAlternateEntryMode::Mode47, "mode47"),
            (PtyTerminalAlternateEntryMode::Mode1047, "mode1047"),
            (PtyTerminalAlternateEntryMode::Mode1049, "mode1049"),
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
        for (value, expected) in [
            (PtyTerminalReplayStage::SemanticHistory, "semanticHistory"),
            (
                PtyTerminalReplayStage::ScreenOnlyHistoryDisabled,
                "screenOnlyHistoryDisabled",
            ),
            (
                PtyTerminalReplayStage::ScreenOnlyCheckpointUnavailable,
                "screenOnlyCheckpointUnavailable",
            ),
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
        for (value, expected) in [
            (PtyTerminalHistoryTruncationReason::None, "none"),
            (
                PtyTerminalHistoryTruncationReason::RowLimitReached,
                "rowLimitReached",
            ),
            (
                PtyTerminalHistoryTruncationReason::ByteLimitReached,
                "byteLimitReached",
            ),
            (
                PtyTerminalHistoryTruncationReason::RowAndByteLimitReached,
                "rowAndByteLimitReached",
            ),
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn restart_injected_body_allows_resume_on_restore() {
        let f = fresh_intent_fixture().await;
        note_post_boundary_content_to_session(f.app.handle(), f.session_id).await;

        assert!(!record_fresh(&f).await);
        assert!(!mirror_fresh(&f));
        assert!(inject_continue_after_restore(record_fresh(&f).await));
    }

    #[tokio::test]
    async fn ctrl_c_cancelled_terminal_line_keeps_restore_fresh() {
        let f = fresh_intent_fixture().await;
        for chunk in [
            b"do the thing".as_slice(),
            b"\x03".as_slice(),
            b"\r".as_slice(),
        ] {
            note_user_message_to_session(
                f.app.handle(),
                f.session_id,
                UserInputSource::Terminal(chunk),
            )
            .await;
        }

        assert!(record_fresh(&f).await);
        assert!(mirror_fresh(&f));
        assert!(!inject_continue_after_restore(record_fresh(&f).await));
    }
}
