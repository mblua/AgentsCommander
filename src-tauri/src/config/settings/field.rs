use super::{AppSettings, ProfileSlotConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Optional config-folder seed for a coding agent. When active, AC copies a
/// template config folder (chosen by convention across profile > matrix >
/// coding-agent-base; see `config_seed.rs`) into the replica at spawn. `dest` is
/// the destination folder NAME under `%AC_REPLICA_ROOT%/` (e.g. ".claude"). No
/// `source` field: the template locations are derived by naming convention.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSeedConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub dest: String,
}

impl ConfigSeedConfig {
    pub fn is_active(&self) -> bool {
        self.enabled && !self.dest.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MainSidebarSide {
    Left,
    #[default]
    Right,
}

/// #2348 - saved display state of the unified main window. Deliberately only two
/// states: minimized and fullscreen are unrepresentable, so an odd window state
/// can never be persisted as a placement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MainWindowDisplayState {
    #[default]
    Normal,
    Maximized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelegramPollFailureLogLevel {
    Debug,
    Warn,
    Error,
}

impl TelegramPollFailureLogLevel {
    pub fn as_log_level(self) -> log::Level {
        match self {
            Self::Debug => log::Level::Debug,
            Self::Warn => log::Level::Warn,
            Self::Error => log::Level::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelegramPollRecoveryLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl TelegramPollRecoveryLogLevel {
    pub fn as_log_level(self) -> log::Level {
        match self {
            Self::Debug => log::Level::Debug,
            Self::Info => log::Level::Info,
            Self::Warn => log::Level::Warn,
            Self::Error => log::Level::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TelegramNetworkPollErrorLogging {
    #[serde(default = "default_telegram_network_first_failure_level")]
    pub first_failure_level: TelegramPollFailureLogLevel,
    #[serde(default = "default_telegram_network_transient_repeat_level")]
    pub transient_repeat_level: TelegramPollFailureLogLevel,
    #[serde(default = "default_telegram_network_sustained_level")]
    pub sustained_level: TelegramPollFailureLogLevel,
    #[serde(default = "default_telegram_network_sustained_after_seconds")]
    pub sustained_after_seconds: u64,
    #[serde(default = "default_telegram_network_sustained_repeat_seconds")]
    pub sustained_repeat_seconds: u64,
    #[serde(default = "default_telegram_network_recovery_level")]
    pub recovery_level: TelegramPollRecoveryLogLevel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResourceWatchdogAction {
    Warn,
    KillGroup,
}

impl Default for TelegramNetworkPollErrorLogging {
    fn default() -> Self {
        Self {
            first_failure_level: default_telegram_network_first_failure_level(),
            transient_repeat_level: default_telegram_network_transient_repeat_level(),
            sustained_level: default_telegram_network_sustained_level(),
            sustained_after_seconds: default_telegram_network_sustained_after_seconds(),
            sustained_repeat_seconds: default_telegram_network_sustained_repeat_seconds(),
            recovery_level: default_telegram_network_recovery_level(),
        }
    }
}

/// #1171 - one entry of the root `watchers` map, or whatever the user wrote there.
///
/// **This wrapper is what stops a malformed watcher from destroying the settings file.**
/// `parse_settings_json` (`:870-871`) deserializes `AppSettings` in ONE shot and
/// `load_settings_from_path` (`:1661-1664`) replaces any failure with
/// `AppSettings::default()`, leaving one log line. A hand-written `"mode": "State"`,
/// `"commands": "claude"` or `"dedupeWindowMs": "2000"` would therefore start
/// AgentsCommander with NO AGENTS CONFIGURED, and every later save would be refused by the
/// #1077 write gate (`read_disk_object_for_write`, `:2565-2591`). With the wrapper the
/// consequence is one skipped watcher.
///
/// `untagged` tries `Valid` first, so `Invalid` only ever catches what `WatcherConfig`
/// rejected. The value is kept verbatim so a save round-trips the user's bytes instead of
/// deleting what it could not read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WatcherEntry {
    Valid(WatcherConfig),
    /// Anything that did not deserialize as a `WatcherConfig`. Skipped by resolution and
    /// logged once per changed value.
    Invalid(serde_json::Value),
}

impl WatcherEntry {
    pub fn valid(&self) -> Option<&WatcherConfig> {
        match self {
            WatcherEntry::Valid(config) => Some(config),
            WatcherEntry::Invalid(_) => None,
        }
    }
}

/// #1171 - one user-configured watcher.
///
/// `mode` and `pattern` stay REQUIRED. A watcher without either is not a watcher, and with
/// the wrapper above the consequence of omitting one is a single skipped entry rather than a
/// lost configuration. A defaulted `mode` would silently run a watcher the user never
/// described.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatcherConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub mode: WatcherMode,
    pub pattern: String,
    /// Absent or null: reaches every configured agent. Present: only entries whose `command`
    /// executable stem matches EXACTLY. Present and empty: reaches none.
    ///
    /// `Option` and not `#[serde(default)] Vec`, because absent and `[]` are opposites here
    /// and only `Option` lets serde tell them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    #[serde(default)]
    pub dedupe: WatcherDedupe,
    #[serde(default = "default_dedupe_window_ms")]
    pub dedupe_window_ms: u64,
    /// Free text, e.g. "claude 2.1.212". Never validated, never parsed. It exists because
    /// `context_scrape/rows.rs:183-186` documents that a TUI format already had to be
    /// re-captured once, and that fact currently lives only in a Rust comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_against: Option<String>,
}

/// #1171 - what a match means.
///
/// `state` is a reading: idempotent, gated, taken over the whole frame. `occurrence` is an
/// event: every match the frame diff declares evaluable counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatcherMode {
    State,
    Occurrence,
}

/// #1171 - what makes two `occurrence` matches "the same one" inside the dedupe window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatcherDedupe {
    /// The matched logical row text.
    #[default]
    Row,
    /// The joined capture groups: two rows truncated differently that capture the same path
    /// are one event.
    Capture,
    /// Every match counts.
    None,
}

/// #2482 - one entry of the root `quotaSources` map, or whatever the user wrote
/// there. Same wrapper, same reason, as `WatcherEntry`: settings deserialize in
/// one shot and any failure is replaced by `AppSettings::default()`, so a
/// hand-written or newer-AC entry must cost one skipped source and never the whole
/// file. `untagged` tries `Valid` first, so `Invalid` only catches what
/// `QuotaSourceConfig` rejected, and its bytes are kept verbatim so a save
/// round-trips what the user wrote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QuotaSourceEntry {
    Valid(QuotaSourceConfig),
    Invalid(serde_json::Value),
}

impl QuotaSourceEntry {
    pub fn valid(&self) -> Option<&QuotaSourceConfig> {
        match self {
            QuotaSourceEntry::Valid(config) => Some(config),
            QuotaSourceEntry::Invalid(_) => None,
        }
    }
}

/// #2482 - one configured quota source. **The multi-agent extension point.**
/// `kind` is the discriminant: adding Codex, Gemini or any other agent is one new
/// variant here plus one match arm in `pty::agent_quota::source`. Nothing else in
/// the engine changes, and an unrecognized `kind` from a newer AC lands in
/// `QuotaSourceEntry::Invalid` rather than destroying the file. Capture group 1 of
/// `ScreenRegex` is the USED percentage; group 1 of `ScreenRegexRemaining` is the
/// REMAINING one, which the engine converts to used.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QuotaSourceConfig {
    /// Read the percentage from what the agent draws in its terminal, with the
    /// screen-row scrape #1032 uses for the context badge. Capture group 1 is the
    /// USED percentage of the 7-day window.
    ScreenRegex {
        pattern: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
    /// #2686 - the same screen-row scrape, for an agent that draws what is LEFT of the
    /// window. Capture group 1 is the REMAINING percentage.
    ScreenRegexRemaining {
        pattern: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
}

pub(crate) fn default_dedupe_window_ms() -> u64 {
    2000
}

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_screenshot_capture_hotkey() -> String {
    "Ctrl+Q".to_string()
}

pub(crate) fn default_sidebar_compact_hotkey() -> String {
    "Ctrl+Shift+E".to_string()
}

pub(crate) fn default_room_number_mask() -> String {
    "#".to_string()
}

/// #640 Resolve the effective auto-self-clear flag for an agent.
/// Precedence: the global master `auto_self_clear_enabled` is an absolute kill
/// switch (off => off for all); else an explicit per-agent override
/// (`auto_self_clear_by_agent[name]`) wins; else the class-aware default
/// (`default_on` = true for coordinator/Root, false for specialists), computed
/// at the call site. Master-first so one toggle reliably disables everything.
pub fn resolve_auto_self_clear(settings: &AppSettings, agent_name: &str, default_on: bool) -> bool {
    if !settings.auto_self_clear_enabled {
        return false; // global master kill switch wins
    }
    if let Some(explicit) = settings.auto_self_clear_by_agent.get(agent_name) {
        return *explicit; // explicit per-agent override wins while enabled
    }
    default_on // class-aware default
}

pub(crate) fn default_profiles_schema_version() -> u32 {
    2
}

pub(crate) fn default_profile_slots() -> BTreeMap<String, ProfileSlotConfig> {
    BTreeMap::from([(
        "A".to_string(),
        ProfileSlotConfig {
            label: String::new(),
        },
    )])
}

pub(crate) fn default_telegram_network_first_failure_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Warn
}

pub(crate) fn default_telegram_network_transient_repeat_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Debug
}

pub(crate) fn default_telegram_network_sustained_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Error
}

pub(crate) fn default_telegram_network_sustained_after_seconds() -> u64 {
    60
}

pub(crate) fn default_telegram_network_sustained_repeat_seconds() -> u64 {
    60
}

pub(crate) fn default_telegram_network_recovery_level() -> TelegramPollRecoveryLogLevel {
    TelegramPollRecoveryLogLevel::Info
}

pub(crate) fn default_gemini_model() -> String {
    "gemini-2.5-flash".to_string()
}

pub(crate) fn default_restart_resume_orchestrator_prompt() -> String {
    "AgentsCommander was restarted. Continue with the work that was in flight.".to_string()
}

pub(crate) fn default_restart_resume_agent_prompt() -> String {
    ".".to_string()
}

pub(crate) fn default_voice_delay() -> u32 {
    15
}

pub(crate) fn default_zoom() -> f64 {
    1.0
}

pub(crate) fn default_web_port() -> u16 {
    super::super::profile::web_server_port()
}

pub(crate) fn default_web_bind() -> String {
    "127.0.0.1".to_string()
}

/// #791 - profile-aware default control-plane API port (delegates to
/// `profile::api_server_port`, mirroring `default_web_port`).
pub(crate) fn default_api_port() -> u16 {
    super::super::profile::api_server_port()
}

/// #791 - default control-plane API bind: loopback, safe-by-default. Mirrors
/// `default_web_bind`. Widening for Docker is a deliberate operator action.
pub(crate) fn default_api_bind() -> String {
    "127.0.0.1".to_string()
}

pub(crate) fn default_sidebar_style() -> String {
    "noir-minimal".to_string()
}

pub(crate) fn default_selected_row_rail_width() -> String {
    "9px".to_string()
}

pub(crate) fn default_main_sidebar_width() -> f64 {
    240.0
}

pub(crate) fn default_git_sweep_concurrency() -> u8 {
    1
}

pub(crate) fn default_git_sweep_min_interval_secs() -> u64 {
    10
}

pub(crate) fn default_ci_activity_enabled() -> bool {
    true
}

pub(crate) fn default_ci_activity_notify_orchestrator() -> bool {
    true
}

pub(crate) fn default_branch_staleness_enabled() -> bool {
    true
}

pub(crate) fn default_branch_staleness_notify_orchestrator() -> bool {
    true
}

pub(crate) fn default_ci_sweep_min_interval_secs() -> u64 {
    30
}

pub(crate) fn default_branch_staleness_interval_secs() -> u64 {
    260
}

pub(crate) fn default_resource_monitor_enabled() -> bool {
    true
}

pub(crate) fn default_terminal_snapshots_enabled() -> bool {
    true
}

pub(crate) fn default_max_concurrent_agent_processes() -> u32 {
    32
}

pub(crate) fn default_coord_badge_yellow_minutes() -> u32 {
    30
}

pub(crate) fn default_coord_badge_red_minutes() -> u32 {
    60
}

pub(crate) fn default_coord_auto_close_minutes() -> u32 {
    60
}

pub(crate) fn default_resource_watchdog_action() -> ResourceWatchdogAction {
    ResourceWatchdogAction::Warn
}

pub(crate) fn default_agent_group_warn_private_bytes() -> u64 {
    8 * 1024 * 1024 * 1024
}

pub(crate) fn default_agent_group_kill_private_bytes() -> u64 {
    12 * 1024 * 1024 * 1024
}

pub(crate) fn default_agent_process_kill_private_bytes() -> u64 {
    12 * 1024 * 1024 * 1024
}

/// #2336 - default `typing_hold_seconds`.
pub const DEFAULT_TYPING_HOLD_SECONDS: u32 = 30;
/// #2336 - accepted `typing_hold_seconds` bounds; outside them the Settings API
/// rejects the update and the live value is left unchanged.
pub const TYPING_HOLD_SECONDS_MIN: u32 = 1;
pub const TYPING_HOLD_SECONDS_MAX: u32 = 3600;

pub(crate) fn default_typing_hold_seconds() -> u32 {
    DEFAULT_TYPING_HOLD_SECONDS
}

pub(crate) const DEFAULT_RESPONSE_CLOSE_IDLE_SECONDS: u32 = 30;
pub(crate) const RESPONSE_CLOSE_IDLE_SECONDS_MIN: u32 = 1;
pub(crate) const RESPONSE_CLOSE_IDLE_SECONDS_MAX: u32 = 3600;

pub(crate) fn default_response_close_idle_seconds() -> u32 {
    DEFAULT_RESPONSE_CLOSE_IDLE_SECONDS
}

/// Disk repair changes only the effective value, without requesting a write.
pub(crate) fn repair_loaded_response_close_settings(settings: &mut AppSettings, source: &str) {
    if !(RESPONSE_CLOSE_IDLE_SECONDS_MIN..=RESPONSE_CLOSE_IDLE_SECONDS_MAX)
        .contains(&settings.response_close_idle_seconds)
    {
        log::warn!(
            "Invalid responseCloseIdleSeconds={} from {source}: expected 1..=3600; using 30 in \
                memory",
            settings.response_close_idle_seconds
        );
        settings.response_close_idle_seconds = DEFAULT_RESPONSE_CLOSE_IDLE_SECONDS;
    }
}

pub(crate) const DEFAULT_ROOM_IDLE_CONFIRMATION_SECONDS: u32 = 15;
pub(crate) const ROOM_IDLE_CONFIRMATION_SECONDS_MIN: u32 = 1;
pub(crate) const ROOM_IDLE_CONFIRMATION_SECONDS_MAX: u32 = 3600;

pub(crate) fn default_room_idle_confirmation_seconds() -> u32 {
    DEFAULT_ROOM_IDLE_CONFIRMATION_SECONDS
}

/// Disk repair changes only the effective value, without requesting a write.
pub(crate) fn repair_loaded_room_idle_confirmation_settings(
    settings: &mut AppSettings,
    source: &str,
) {
    if !(ROOM_IDLE_CONFIRMATION_SECONDS_MIN..=ROOM_IDLE_CONFIRMATION_SECONDS_MAX)
        .contains(&settings.room_idle_confirmation_seconds)
    {
        log::warn!(
            "Invalid roomIdleConfirmationSeconds={} from {source}: expected 1..=3600; using 15 in \
                memory",
            settings.room_idle_confirmation_seconds
        );
        settings.room_idle_confirmation_seconds = DEFAULT_ROOM_IDLE_CONFIRMATION_SECONDS;
    }
}

pub(crate) fn default_resource_keep_last_snapshot() -> bool {
    true
}

pub(crate) fn default_resource_backoff_polling() -> bool {
    true
}
