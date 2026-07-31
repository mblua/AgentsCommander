use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::placeholders::AC_PLACEHOLDER_TOKENS;
use crate::pty::backend::SessionBackendKind;
use crate::session::profile::CodingAgentKind;
use crate::telegram::types::TelegramBotConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendConfig {
    #[serde(default)]
    pub kind: SessionBackendKind,
    /// #868 - optional per-agent Docker image override for container runtime.
    /// None falls back to AGENTSCOMMANDER_CONTAINER_IMAGE at launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl Default for AgentBackendConfig {
    fn default() -> Self {
        Self {
            kind: SessionBackendKind::LocalProcess,
            image: None,
        }
    }
}

impl AgentBackendConfig {
    fn is_default(&self) -> bool {
        self.kind == SessionBackendKind::LocalProcess
    }
}

impl From<&AgentBackendConfig> for SessionBackendKind {
    fn from(config: &AgentBackendConfig) -> Self {
        config.kind
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub id: String,
    pub label: String,
    pub command: String,
    pub color: String,
    /// Base environment rows applied to every launch of this coding agent.
    #[serde(default)]
    pub envs: Vec<CodingAgentEnv>,
    /// When true for Codex, AC provides an isolated CODEX_HOME at spawn time.
    #[serde(default, alias = "isolateCodexHome")]
    pub isolated_home: bool,
    /// #529 - filename AC writes into the agent root at launch (content = AC
    /// context + Role.md). `None`/empty falls back to the command-derived default
    /// (Claude -> CLAUDE.md, Gemini -> GEMINI.md, Codex/Pi/else -> AGENTS.md). Serialized as
    /// `instructionsFilename`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    /// #598 - optional config-folder seed (e.g. `.claude`) copied from a
    /// convention-chosen template into the replica at spawn. `None`/inactive
    /// means no seeding. Serialized as `configSeed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed: Option<ConfigSeedConfig>,
    /// #1032 - per-agent regex run over this agent's screen rows to read its
    /// context-window usage. Capture group 1 is the percentage. `None`/absent means
    /// the feature is off for this agent: no event, no reading, no PTY lock, no
    /// compile. Serialized as `contextRegex`.
    ///
    /// The engine ships no anchoring rules of its own; every rule that makes a
    /// pattern trustworthy lives in the pattern, because only the user knows what
    /// their agent renders. A reading may enqueue an informational notice to the exact
    /// workgroup coordinator, but it never drives remedial or destructive session action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_regex: Option<String>,
    /// Backend used for future non-local session transports. Omitted/default
    /// keeps today's local-process behavior.
    #[serde(default, skip_serializing_if = "AgentBackendConfig::is_default")]
    pub backend: AgentBackendConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentEnv {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub source: CodingAgentEnvSource,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CodingAgentEnvSource {
    #[default]
    User,
    #[serde(alias = "agentsCommander")]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentProfilesConfig {
    #[serde(default = "default_profiles_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_profile_slots", alias = "letters")]
    pub profile_slots: BTreeMap<String, ProfileSlotConfig>,
    #[serde(default, alias = "agentDefaults")]
    pub default_profile_by_agent: BTreeMap<String, String>,
    #[serde(default, alias = "matrix")]
    pub profiles_by_agent: BTreeMap<String, BTreeMap<String, ProfileCellConfig>>,
    /// #548: per-(agent, letter) label override. Empty for an agent/letter means
    /// "inherit": primigenio (agents[0]) label, else legacy profile_slots[letter].label,
    /// else the bare letter. Always serializes (no skip_serializing_if) like
    /// profiles_by_agent, so the key is always present on disk.
    #[serde(default)]
    pub profile_labels_by_agent: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for CodingAgentProfilesConfig {
    fn default() -> Self {
        Self {
            schema_version: default_profiles_schema_version(),
            profile_slots: default_profile_slots(),
            default_profile_by_agent: BTreeMap::new(),
            profiles_by_agent: BTreeMap::new(),
            profile_labels_by_agent: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSlotConfig {
    #[serde(default, alias = "name")]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCellConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: String,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_shell: String,
    pub default_shell_args: Vec<String>,
    /// Available coding agents
    pub agents: Vec<AgentConfig>,
    /// Reusable coding-agent profile letters, defaults, and per-agent cells.
    #[serde(default)]
    pub coding_agent_profiles: CodingAgentProfilesConfig,
    /// Configured Telegram bots for bridge
    #[serde(default)]
    pub telegram_bots: Vec<TelegramBotConfig>,
    /// Controls log severity for transient and sustained Telegram getUpdates
    /// network failures. Non-network poll failures still log at ERROR.
    #[serde(default)]
    pub telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging,
    /// When true, on app start, wake coordinators whose PTY was awake at shutdown.
    /// Coordinators that were asleep at shutdown remain asleep. Non-coordinator
    /// team members are never auto-woken on startup (the user must click their
    /// replica in the sidebar to wake them). Issue #248.
    #[serde(default)]
    pub restore_coordinator_wake_state: bool,
    /// Migration carrier: legacy field name from before issue #248. Read on
    /// deserialization, then translated into `restore_coordinator_wake_state` by
    /// `apply_issue_248_migration`. `skip_serializing_if = "Option::is_none"`
    /// elides it on the next save once the migration has run.
    ///
    /// One-shot migration semantics:
    ///   - legacy `startOnlyCoordinators: true`  → `restoreCoordinatorWakeState: true`
    ///   - legacy `startOnlyCoordinators: false` → `restoreCoordinatorWakeState: false`
    ///
    /// In both cases the legacy value is dropped from disk on next save.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "startOnlyCoordinators"
    )]
    pub legacy_start_only_coordinators: Option<bool>,
    /// Keep sidebar window always on top
    #[serde(default)]
    pub sidebar_always_on_top: bool,
    /// When true, play a short beep when an entire team transitions from
    /// busy→all-idle. The transition is computed in the FE from `waitingForInput`.
    #[serde(default = "default_true")]
    pub team_idle_beep_enabled: bool,
    /// Master switch for all app-emitted sounds. When false, every current and
    /// future app-generated playback path must stay silent. Per-feature toggles
    /// (e.g. `team_idle_beep_enabled`) act as additional gates underneath this one.
    #[serde(default = "default_true")]
    pub sounds_enabled: bool,
    /// Raise terminal window when sidebar is clicked
    #[serde(default = "default_true")]
    pub raise_terminal_on_click: bool,
    /// #714 Native global hotkey for screenshot capture, e.g. "Ctrl+Q".
    #[serde(default = "default_screenshot_capture_hotkey")]
    pub screenshot_capture_hotkey: String,
    /// Enable voice-to-text microphone button on session items
    #[serde(default)]
    pub voice_to_text_enabled: bool,
    /// Gemini API key for voice transcription
    #[serde(default)]
    pub gemini_api_key: String,
    /// Gemini model for voice transcription
    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,
    /// Auto-execute (send Enter) after voice transcription
    #[serde(default = "default_true")]
    pub voice_auto_execute: bool,
    /// Delay in seconds before auto-executing after transcription
    #[serde(default = "default_voice_delay")]
    pub voice_auto_execute_delay: u32,
    /// Zoom level for the sidebar window (1.0 = 100%). DEPRECATED in 0.8.0 — see `main_zoom`.
    /// Retained for one version for downgrade safety; seeded into `main_zoom` on first load.
    #[serde(default = "default_zoom")]
    pub sidebar_zoom: f64,
    /// Zoom level for the terminal window (1.0 = 100%). Still used by detached windows in 0.8.0.
    #[serde(default = "default_zoom")]
    pub terminal_zoom: f64,
    /// Zoom level for the unified main window (1.0 = 100%). Introduced in 0.8.0.
    #[serde(default = "default_zoom")]
    pub main_zoom: f64,
    /// Zoom level for the guide window (1.0 = 100%)
    #[serde(default = "default_zoom")]
    pub guide_zoom: f64,
    /// Legacy: zoom level for the removed dark factory window. Kept for backwards-compat reads.
    #[serde(default = "default_zoom")]
    pub darkfactory_zoom: f64,
    /// DEPRECATED in 0.8.0 — previously held the sidebar window geometry under the
    /// two-window model. `skip_serializing_if` drops it on next save. See `main_geometry`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_geometry: Option<WindowGeometry>,
    /// DEPRECATED in 0.8.0 — previously held the terminal window geometry. Seeded into
    /// `main_geometry` by the first-boot migration. `skip_serializing_if` drops it on next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_geometry: Option<WindowGeometry>,
    /// Saved geometry for the unified main window. Introduced in 0.8.0.
    #[serde(default)]
    pub main_geometry: Option<WindowGeometry>,
    /// Width of the sidebar pane inside the main window, in logical pixels.
    /// Clamped to [200, 600] at drag time and on load.
    #[serde(default = "default_main_sidebar_width")]
    pub main_sidebar_width: f64,
    /// Side of the unified main window where the sidebar is placed.
    #[serde(default)]
    pub main_sidebar_side: MainSidebarSide,
    /// Keep the unified main window always on top.
    #[serde(default)]
    pub main_always_on_top: bool,
    /// #587 - whether the Resource Monitor occupies the main central pane (vs the
    /// terminal). Restored on startup; default false (terminal).
    #[serde(default)]
    pub main_resource_monitor_attached: bool,
    /// Enable the embedded web server for remote browser access
    #[serde(default)]
    pub web_server_enabled: bool,
    /// Port for the web server
    #[serde(default = "default_web_port")]
    pub web_server_port: u16,
    /// Bind address: "127.0.0.1" (local only) or "0.0.0.0" (all interfaces)
    #[serde(default = "default_web_bind")]
    pub web_server_bind: String,
    /// #791 - enable the in-daemon control-plane API server (Docker/distributed
    /// agents). Default false: no new listening socket unless the operator opts in.
    #[serde(default)]
    pub api_server_enabled: bool,
    /// #791 - control-plane API port. Profile-aware default so dev/prod builds
    /// do not collide on one host.
    #[serde(default = "default_api_port")]
    pub api_server_port: u16,
    /// #791 - control-plane API bind address. Default "127.0.0.1" (safe, loopback).
    /// Reaching a Linux container requires a deliberate operator widening; any
    /// non-loopback bind logs a loud startup warning.
    #[serde(default = "default_api_bind")]
    pub api_server_bind: String,
    /// Currently loaded project path (legacy single-project, kept for backward compat)
    #[serde(default)]
    pub project_path: Option<String>,
    /// Currently loaded project paths (multi-project support)
    #[serde(default)]
    pub project_paths: Vec<String>,
    /// #881 - projects hidden from the sidebar but still registered. Disk-authoritative
    /// under the same Design S rules as `project_paths`: the default `save_settings`
    /// preserves the on-disk copy and only dedicated list commands mutate it.
    #[serde(default)]
    pub archived_project_paths: Vec<String>,
    /// #1077 - hidden persistence state for the portable dual project paths. Holds
    /// the resolved raw/selected pairs, their instance-relative companions,
    /// outcomes, and reconcile-eligibility bits so the codec can rebuild the six
    /// disk fields without re-reading disk. Never serialized directly (the codec
    /// owns the six on-disk fields); behind an `Arc` so the ubiquitous
    /// `AppSettings::clone()` stays cheap and copy-on-write mutation is explicit.
    #[serde(skip, default)]
    pub(crate) project_path_state: Arc<crate::config::projects::ProjectPathPersistenceState>,
    /// Sidebar visual style: "noir-minimal", "card-sections", "command-center", "deep-space", "arctic-ops", "obsidian-mesh", "neon-circuit"
    #[serde(default = "default_sidebar_style")]
    pub sidebar_style: String,
    /// Root token that bypasses all routing checks in the send command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_token: Option<String>,
    /// Whether the user has dismissed the first-run onboarding wizard
    #[serde(default)]
    pub onboarding_dismissed: bool,
    /// When true, sort the Coordinator Quick-Access list by most-recent-activity descending.
    /// Activity = busy→idle transition (IdleDetector emits session_idle).
    /// Per-session timestamps live in the frontend store and are NOT persisted.
    #[serde(default)]
    pub coord_sort_by_activity: bool,
    #[serde(default = "default_true")]
    pub always_show_selected_workgroup: bool,
    /// Optional logger filter expression. Applied at startup if `RUST_LOG` is unset.
    /// Uses standard `env_logger` filter syntax (e.g. `info,agentscommander_lib::config::teams=trace`).
    /// Phase 1 of #93 — settings-level control with `RUST_LOG` env override (backwards-compat).
    /// Phase 2 (UI dropdown) and Phase 3 (live reload) are deferred per the issue.
    #[serde(default)]
    pub log_level: Option<String>,
    /// When true, on Coordinator session spawn AC injects a prompt asking the
    /// agent to add a YAML frontmatter `title:` line to its workgroup
    /// `TASK.md` (only if the brief is non-empty and has no `title:` yet).
    /// See plan `_plans/107-auto-brief-title.md`.
    #[serde(default = "default_true")]
    pub auto_generate_task_title: bool,
    /// Optional override for the local agent-templates directory used by the
    /// New Agent role-template picker (#271). Empty/missing ⇒ default
    /// `<config_dir>/agent-templates/`. Relative ⇒ resolved against
    /// `<config_dir>/`. Absolute ⇒ used as-is.
    #[serde(default)]
    pub agent_templates_path: Option<String>,
    /// When true, the app uses the light theme; when false, the dark theme.
    /// Defaults to `false` so fresh and missing values use dark mode.
    #[serde(default)]
    pub theme_light: bool,
    /// #965 - rail project sections the user has explicitly collapsed by clicking
    /// their header. This is the RAIL's own state; it is deliberately NOT the
    /// ProjectPanel's collapse (which stays session-only and auto-focus driven).
    /// Entries are FRONTEND-NORMALIZED project paths (lowercase, forward slashes,
    /// no trailing slash: `normalizeProjectPathForCompare` in
    /// `src/sidebar/stores/project-refresh.ts`).
    /// WRITTEN ONLY BY `set_rail_collapse`. A whole-object settings payload from the
    /// GUI/CLI/API carries no authority for this field: both whole-object writers
    /// restore it from live memory (see `build_protected_settings_candidate`).
    #[serde(default)]
    pub rail_collapsed_projects: Vec<String>,
    /// #965 - collapsed state of the rail's cross-project `Favorites` section.
    /// Belongs to no project, which is why collapse cannot live in project-settings.json.
    /// Same protection as `rail_collapsed_projects`.
    #[serde(default)]
    pub rail_favorites_collapsed: bool,
    /// When true, show the Spec Board toolbar button. Defaults off because the
    /// board is an opt-in feature enabled manually from settings.json.
    #[serde(default)]
    pub spec_board_enabled: bool,
    #[serde(default = "default_resource_monitor_enabled")]
    pub resource_monitor_enabled: bool,
    #[serde(default = "default_max_concurrent_agent_processes")]
    pub max_concurrent_agent_processes: u32,
    #[serde(default = "default_resource_watchdog_action")]
    pub resource_watchdog_action: ResourceWatchdogAction,
    #[serde(default = "default_agent_group_warn_private_bytes")]
    pub agent_group_warn_private_bytes: u64,
    #[serde(default = "default_agent_group_kill_private_bytes")]
    pub agent_group_kill_private_bytes: u64,
    #[serde(default = "default_agent_process_kill_private_bytes")]
    pub agent_process_kill_private_bytes: u64,
    #[serde(default = "default_resource_keep_last_snapshot")]
    pub resource_keep_last_snapshot: bool,
    #[serde(default = "default_resource_backoff_polling")]
    pub resource_backoff_polling: bool,
    /// #552 badge color thresholds (minutes). green < yellow <= value < red <= value.
    #[serde(default = "default_coord_badge_yellow_minutes")]
    pub coordinator_idle_badge_yellow_minutes: u32,
    #[serde(default = "default_coord_badge_red_minutes")]
    pub coordinator_idle_badge_red_minutes: u32,
    /// #552 auto-close lifecycle clock.
    #[serde(default = "default_true")]
    pub coordinator_auto_close_enabled: bool,
    #[serde(default = "default_coord_auto_close_minutes")]
    pub coordinator_auto_close_minutes: u32,
    /// #817 When true, auto-close skips sessions with a persisted Telegram bot
    /// assignment. Default false preserves legacy auto-close behavior.
    #[serde(default)]
    pub coordinator_auto_close_skip_telegram_assigned: bool,
    /// #588 When true, manually closing a coordinator also closes its team
    /// agents (cascade). When false, only the coordinator closes. Default true.
    #[serde(default = "default_true")]
    pub coordinator_cascade_close_enabled: bool,
    /// #609 When true, check npm on startup (<=1x/24h) and notify in-app when
    /// a newer published version is available. Default true.
    #[serde(default = "default_true")]
    pub npm_update_notifications_enabled: bool,
    /// #640 Global master for auto self-handoff-and-clear. Absolute kill switch:
    /// false => off for every agent. When true, the class-aware default applies
    /// (ON for coordinator/Root, OFF for specialists), subject to per-agent
    /// overrides in `auto_self_clear_by_agent`.
    #[serde(default = "default_true")]
    pub auto_self_clear_enabled: bool,
    /// #640 Per-agent override of the class default, keyed by agent name (same
    /// key as `coding_agent_profiles.default_profile_by_agent`). Applies only
    /// while the global master is on; absent = use the class default.
    #[serde(default)]
    pub auto_self_clear_by_agent: std::collections::BTreeMap<String, bool>,
    /// #930 - when true (default), container coding-agent sessions copy the host
    /// user's credential file for that agent into the replica config dir at spawn
    /// and delete it on teardown. When false, the user supplies credentials
    /// themselves (e.g. a CLAUDE_CODE_OAUTH_TOKEN env row).
    #[serde(default = "default_true")]
    pub container_credentials_from_host: bool,
    /// #1171 - root-level watcher patterns, keyed by watcher id.
    ///
    /// Root-level and not a field on `AgentConfig`, so the 20-plus struct-construction sites
    /// that already had to write `context_regex: None` are untouched, AND so a pattern can
    /// apply to every agent - which is exactly what the per-agent shape cannot express. Same
    /// shape as `auto_self_clear_by_agent` (`:530-531`). `BTreeMap` for a stable on-disk
    /// order and clean diffs, and because the 8-watcher budget resolves in key order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub watchers: BTreeMap<String, WatcherEntry>,
    /// #1171 - geometry of the watcher activity window.
    ///
    /// `skip_serializing_if` and deliberately NOT a copy of `main_geometry` (`:367-369`),
    /// which lacks it: without the skip, `"watchersGeometry": null` would appear in every
    /// user's file on the next save, so configuring nothing would still leave a trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watchers_geometry: Option<WindowGeometry>,
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

fn default_dedupe_window_ms() -> u64 {
    2000
}

fn default_true() -> bool {
    true
}

fn default_screenshot_capture_hotkey() -> String {
    "Ctrl+Q".to_string()
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

fn default_profiles_schema_version() -> u32 {
    2
}

fn default_profile_slots() -> BTreeMap<String, ProfileSlotConfig> {
    BTreeMap::from([(
        "A".to_string(),
        ProfileSlotConfig {
            label: String::new(),
        },
    )])
}

fn default_telegram_network_first_failure_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Warn
}

fn default_telegram_network_transient_repeat_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Debug
}

fn default_telegram_network_sustained_level() -> TelegramPollFailureLogLevel {
    TelegramPollFailureLogLevel::Error
}

fn default_telegram_network_sustained_after_seconds() -> u64 {
    60
}

fn default_telegram_network_sustained_repeat_seconds() -> u64 {
    60
}

fn default_telegram_network_recovery_level() -> TelegramPollRecoveryLogLevel {
    TelegramPollRecoveryLogLevel::Info
}

fn default_gemini_model() -> String {
    "gemini-2.5-flash".to_string()
}

fn default_voice_delay() -> u32 {
    15
}

fn default_zoom() -> f64 {
    1.0
}

fn default_web_port() -> u16 {
    super::profile::web_server_port()
}

fn default_web_bind() -> String {
    "127.0.0.1".to_string()
}

/// #791 - profile-aware default control-plane API port (delegates to
/// `profile::api_server_port`, mirroring `default_web_port`).
fn default_api_port() -> u16 {
    super::profile::api_server_port()
}

/// #791 - default control-plane API bind: loopback, safe-by-default. Mirrors
/// `default_web_bind`. Widening for Docker is a deliberate operator action.
fn default_api_bind() -> String {
    "127.0.0.1".to_string()
}

fn default_sidebar_style() -> String {
    "noir-minimal".to_string()
}

fn default_main_sidebar_width() -> f64 {
    240.0
}

fn default_resource_monitor_enabled() -> bool {
    true
}

fn default_max_concurrent_agent_processes() -> u32 {
    32
}

fn default_coord_badge_yellow_minutes() -> u32 {
    30
}

fn default_coord_badge_red_minutes() -> u32 {
    60
}

fn default_coord_auto_close_minutes() -> u32 {
    60
}

fn default_resource_watchdog_action() -> ResourceWatchdogAction {
    ResourceWatchdogAction::Warn
}

fn default_agent_group_warn_private_bytes() -> u64 {
    8 * 1024 * 1024 * 1024
}

fn default_agent_group_kill_private_bytes() -> u64 {
    12 * 1024 * 1024 * 1024
}

fn default_agent_process_kill_private_bytes() -> u64 {
    12 * 1024 * 1024 * 1024
}

fn default_resource_keep_last_snapshot() -> bool {
    true
}

fn default_resource_backoff_polling() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        let (default_shell, default_shell_args) = if cfg!(target_os = "windows") {
            ("powershell.exe".to_string(), vec!["-NoLogo".to_string()])
        } else {
            ("/bin/bash".to_string(), vec![])
        };

        Self {
            default_shell,
            default_shell_args,
            agents: vec![],
            coding_agent_profiles: CodingAgentProfilesConfig::default(),
            telegram_bots: vec![],
            telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging::default(),
            restore_coordinator_wake_state: false,
            legacy_start_only_coordinators: None,
            sidebar_always_on_top: false,
            team_idle_beep_enabled: true,
            sounds_enabled: true,
            raise_terminal_on_click: true,
            screenshot_capture_hotkey: default_screenshot_capture_hotkey(),
            voice_to_text_enabled: false,
            gemini_api_key: String::new(),
            gemini_model: default_gemini_model(),
            voice_auto_execute: true,
            voice_auto_execute_delay: default_voice_delay(),
            sidebar_zoom: default_zoom(),
            terminal_zoom: default_zoom(),
            main_zoom: default_zoom(),
            guide_zoom: default_zoom(),
            darkfactory_zoom: default_zoom(),
            sidebar_geometry: None,
            terminal_geometry: None,
            main_geometry: None,
            main_sidebar_width: default_main_sidebar_width(),
            main_sidebar_side: MainSidebarSide::default(),
            main_always_on_top: false,
            main_resource_monitor_attached: false,
            web_server_enabled: false,
            web_server_port: default_web_port(),
            web_server_bind: default_web_bind(),
            api_server_enabled: false,
            api_server_port: default_api_port(),
            api_server_bind: default_api_bind(),
            project_path: None,
            project_paths: vec![],
            archived_project_paths: vec![],
            project_path_state: Arc::default(),
            sidebar_style: default_sidebar_style(),
            root_token: None,
            onboarding_dismissed: false,
            coord_sort_by_activity: false,
            always_show_selected_workgroup: true,
            log_level: None,
            auto_generate_task_title: true,
            agent_templates_path: None,
            theme_light: false,
            rail_collapsed_projects: Vec::new(),
            rail_favorites_collapsed: false,
            spec_board_enabled: false,
            resource_monitor_enabled: default_resource_monitor_enabled(),
            max_concurrent_agent_processes: default_max_concurrent_agent_processes(),
            resource_watchdog_action: default_resource_watchdog_action(),
            agent_group_warn_private_bytes: default_agent_group_warn_private_bytes(),
            agent_group_kill_private_bytes: default_agent_group_kill_private_bytes(),
            agent_process_kill_private_bytes: default_agent_process_kill_private_bytes(),
            resource_keep_last_snapshot: default_resource_keep_last_snapshot(),
            resource_backoff_polling: default_resource_backoff_polling(),
            coordinator_idle_badge_yellow_minutes: default_coord_badge_yellow_minutes(),
            coordinator_idle_badge_red_minutes: default_coord_badge_red_minutes(),
            coordinator_auto_close_enabled: true,
            coordinator_auto_close_minutes: default_coord_auto_close_minutes(),
            coordinator_auto_close_skip_telegram_assigned: false,
            coordinator_cascade_close_enabled: true,
            npm_update_notifications_enabled: true,
            auto_self_clear_enabled: true,
            auto_self_clear_by_agent: std::collections::BTreeMap::new(),
            container_credentials_from_host: true,
            watchers: BTreeMap::new(),
            watchers_geometry: None,
        }
    }
}

pub(crate) fn command_token_basename(token: &str) -> String {
    std::path::Path::new(token)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(token)
        .to_lowercase()
}

fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
    if start >= tokens.len() {
        return start;
    }

    let mut idx = start;
    let mut in_single = false;
    let mut in_double = false;

    while idx < tokens.len() {
        let token = tokens[idx];
        if token_has_unclosed_quote(token, '\'') {
            in_single = !in_single;
        }
        if token_has_unclosed_quote(token, '"') {
            in_double = !in_double;
        }
        idx += 1;
        if !in_single && !in_double {
            break;
        }
    }

    idx
}

fn find_provider_token(tokens: &[&str], provider: &str) -> Option<usize> {
    tokens
        .iter()
        .position(|token| command_token_basename(token) == provider)
}

fn gemini_has_manual_resume(tokens: &[&str], gemini_idx: usize) -> bool {
    let mut idx = gemini_idx + 1;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("--resume") || token.to_lowercase().starts_with("--resume=") {
            return true;
        }
        idx += 1;
    }
    false
}

fn codex_has_manual_resume(tokens: &[&str], codex_idx: usize) -> bool {
    let mut idx = codex_idx + 1;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("resume") || token.eq_ignore_ascii_case("--last") {
            return true;
        }
        idx += 1;
    }
    false
}

pub fn is_valid_profile_letter(letter: &str) -> bool {
    letter.len() == 1 && letter.as_bytes()[0].is_ascii_uppercase()
}

pub fn normalize_profile_letter(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() == 1 {
        let upper = trimmed.to_ascii_uppercase();
        if is_valid_profile_letter(&upper) {
            return Some(upper);
        }
    }
    None
}

fn parse_settings_json(contents: &str, source: &str) -> Result<(AppSettings, bool), String> {
    let mut value: Value = serde_json::from_str(contents)
        .map_err(|e| format!("Failed to parse settings file: {}", e))?;
    let migrated = migrate_settings_value_to_v2(&mut value);
    // #1077: decode the six project fields and replace the three runtime fields
    // with the SELECTED canonical paths BEFORE deserializing the rest of
    // AppSettings, so a wrong-typed project field cannot fail unrelated settings
    // deserialization and unresolved/conflicting pairs survive in hidden state.
    let base = production_instance_base();
    let state =
        apply_project_decode_to_value(&mut value, base.as_deref(), &projects::FsCandidateResolver);
    let mut settings: AppSettings = serde_json::from_value(value)
        .map_err(|e| format!("Failed to deserialize settings from {source}: {e}"))?;
    settings.project_path_state = Arc::new(state);
    Ok((settings, migrated))
}

fn migrate_settings_value_to_v2(value: &mut Value) -> bool {
    let Some(root) = value.as_object_mut() else {
        return false;
    };
    let agent_commands = agent_command_map_from_value(root.get("agents"));
    let Some(profiles_value) = root.get_mut("codingAgentProfiles") else {
        return false;
    };
    let Some(profiles_obj) = profiles_value.as_object() else {
        return false;
    };
    let migrated_profiles = migrate_profiles_object_to_v2(profiles_obj, &agent_commands);
    let changed = *profiles_value != migrated_profiles;
    if changed {
        *profiles_value = migrated_profiles;
    }
    changed
}

fn agent_command_map_from_value(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_array)
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| {
                    let obj = agent.as_object()?;
                    let id = obj.get("id")?.as_str()?;
                    let command = obj.get("command")?.as_str()?;
                    Some((id.to_string(), command.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn migrate_profiles_object_to_v2(
    obj: &Map<String, Value>,
    agent_commands: &BTreeMap<String, String>,
) -> Value {
    let mut out = Map::new();
    out.insert("schemaVersion".to_string(), Value::Number(2.into()));
    out.insert(
        "profileSlots".to_string(),
        migrate_profile_slots(obj.get("profileSlots").or_else(|| obj.get("letters"))),
    );
    out.insert(
        "defaultProfileByAgent".to_string(),
        obj.get("defaultProfileByAgent")
            .or_else(|| obj.get("agentDefaults"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    out.insert(
        "profilesByAgent".to_string(),
        migrate_profiles_by_agent(
            obj.get("profilesByAgent").or_else(|| obj.get("matrix")),
            agent_commands,
        ),
    );
    Value::Object(out)
}

fn migrate_profile_slots(value: Option<&Value>) -> Value {
    let mut out = Map::new();
    if let Some(slots) = value.and_then(Value::as_object) {
        for (letter, slot) in slots {
            let label = slot
                .get("label")
                .or_else(|| slot.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            out.insert(
                letter.clone(),
                serde_json::json!({
                    "label": label,
                }),
            );
        }
    }
    if !out.contains_key("A") {
        out.insert("A".to_string(), serde_json::json!({ "label": "" }));
    }
    Value::Object(out)
}

fn migrate_profiles_by_agent(
    value: Option<&Value>,
    agent_commands: &BTreeMap<String, String>,
) -> Value {
    let mut out = Map::new();
    if let Some(by_agent) = value.and_then(Value::as_object) {
        for (agent_id, cells_value) in by_agent {
            let mut cells_out = Map::new();
            if let Some(cells) = cells_value.as_object() {
                for (letter, cell_value) in cells {
                    cells_out.insert(
                        letter.clone(),
                        migrate_profile_cell(agent_id, letter, cell_value, agent_commands),
                    );
                }
            }
            out.insert(agent_id.clone(), Value::Object(cells_out));
        }
    }
    Value::Object(out)
}

fn migrate_profile_cell(
    agent_id: &str,
    letter: &str,
    value: &Value,
    agent_commands: &BTreeMap<String, String>,
) -> Value {
    let Some(obj) = value.as_object() else {
        return serde_json::json!({
            "enabled": false,
            "command": "",
            "env": {},
            "notes": "",
        });
    };

    let mut enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let command = obj
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_string);
    let legacy_argv = legacy_string_array(obj.get("argv"));
    let legacy_args = legacy_string_array(obj.get("args"));
    if legacy_argv.is_some() && legacy_args.is_some() {
        log::warn!(
            "[settings-migration] profile {}:{} has both legacy argv and args; args ignored",
            agent_id,
            letter
        );
    }
    let legacy_tokens = legacy_argv.or(legacy_args);
    let command = command.unwrap_or_else(|| {
        let Some(legacy_tokens) = legacy_tokens else {
            return String::new();
        };
        match agent_commands.get(agent_id) {
            Some(agent_command) => {
                match crate::config::agent_command::normalize_legacy_agent_command(agent_command) {
                    Ok(normalized) => {
                        let mut tokens = Vec::with_capacity(1 + normalized.shell_args.len() + legacy_tokens.len());
                        tokens.push(normalized.shell);
                        tokens.extend(normalized.shell_args);
                        tokens.extend(legacy_tokens);
                        crate::config::agent_command::stringify_agent_command_tokens(&tokens)
                    }
                    Err(e) => {
                        enabled = false;
                        log::error!(
                            "[settings-migration] profile {}:{} could not parse owning agent command {:?}: {}; preserving legacy args disabled",
                            agent_id,
                            letter,
                            agent_command,
                            e
                        );
                        crate::config::agent_command::stringify_agent_command_tokens(&legacy_tokens)
                    }
                }
            }
            None => {
                enabled = false;
                log::warn!(
                    "[settings-migration] profile {}:{} has legacy args but no owning agent command; preserving disabled",
                    agent_id,
                    letter
                );
                crate::config::agent_command::stringify_agent_command_tokens(&legacy_tokens)
            }
        }
    });

    serde_json::json!({
        "enabled": enabled,
        "command": command,
        "env": string_map_value(obj.get("env")),
        "notes": obj.get("notes").and_then(Value::as_str).unwrap_or(""),
    })
}

fn legacy_string_array(value: Option<&Value>) -> Option<Vec<String>> {
    value.and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()
    })
}

fn string_map_value(value: Option<&Value>) -> Value {
    let mut out = Map::new();
    if let Some(map) = value.and_then(Value::as_object) {
        for (key, value) in map {
            if let Some(value) = value.as_str() {
                out.insert(key.clone(), Value::String(value.to_string()));
            }
        }
    }
    Value::Object(out)
}

pub fn empty_profile_cell() -> ProfileCellConfig {
    ProfileCellConfig {
        enabled: true,
        command: String::new(),
        env: BTreeMap::new(),
        notes: String::new(),
    }
}

pub fn repair_coding_agent_profiles_config(
    profiles: &mut CodingAgentProfilesConfig,
    agents: &[AgentConfig],
) -> bool {
    let mut changed = false;

    if profiles.schema_version < 2 {
        profiles.schema_version = default_profiles_schema_version();
        changed = true;
    }

    let original_letters_len = profiles.profile_slots.len();
    profiles
        .profile_slots
        .retain(|letter, _| is_valid_profile_letter(letter));
    changed |= profiles.profile_slots.len() != original_letters_len;

    if !profiles.profile_slots.contains_key("A") {
        profiles.profile_slots.insert(
            "A".to_string(),
            ProfileSlotConfig {
                label: String::new(),
            },
        );
        changed = true;
    }

    let original_defaults_len = profiles.default_profile_by_agent.len();
    profiles
        .default_profile_by_agent
        .retain(|_, letter| is_valid_profile_letter(letter));
    changed |= profiles.default_profile_by_agent.len() != original_defaults_len;

    for cells in profiles.profiles_by_agent.values_mut() {
        let original_cells_len = cells.len();
        cells.retain(|letter, _| is_valid_profile_letter(letter));
        changed |= cells.len() != original_cells_len;
    }

    // #548: prune invalid (non A..Z) letters from each agent's label-override map,
    // mirroring the cell-letter prune above. Never creates override entries and never
    // prunes by agent id (an override for a not-yet-loaded agent is harmless and may
    // precede its agent in load order, per plan 3.7). `retain` keeps every valid key,
    // so a clean config leaves `changed` false and is not rewritten on load.
    for labels in profiles.profile_labels_by_agent.values_mut() {
        let original_len = labels.len();
        labels.retain(|letter, _| is_valid_profile_letter(letter));
        changed |= labels.len() != original_len;
    }

    for agent in agents {
        let cells = profiles
            .profiles_by_agent
            .entry(agent.id.clone())
            .or_default();
        if !cells.contains_key("A") {
            cells.insert("A".to_string(), empty_profile_cell());
            changed = true;
        }
    }

    changed
}

pub fn normalize_env_key_for_platform(key: &str) -> String {
    if cfg!(windows) {
        key.to_ascii_uppercase()
    } else {
        key.to_string()
    }
}

pub fn is_codex_home_key(key: &str) -> bool {
    normalize_env_key_for_platform(key) == normalize_env_key_for_platform("CODEX_HOME")
}

/// True for opencode's config-dir env key. Analogous to [`is_codex_home_key`];
/// the launch path uses it to auto-create the resolved `OPENCODE_CONFIG_DIR`
/// before spawn (opencode does not create that dir itself and exits 1 if it is
/// missing). Case-insensitive on Windows via `normalize_env_key_for_platform`.
pub fn is_opencode_config_dir_key(key: &str) -> bool {
    normalize_env_key_for_platform(key) == normalize_env_key_for_platform("OPENCODE_CONFIG_DIR")
}

pub fn is_reserved_env_key(key: &str) -> bool {
    let normalized = normalize_env_key_for_platform(key);
    let ac_prefix = normalize_env_key_for_platform("AGENTSCOMMANDER_");
    normalized.starts_with(&ac_prefix)
        || normalized == normalize_env_key_for_platform("AC_REAL_GIT")
        || normalized == normalize_env_key_for_platform("TERM")
        || normalized == normalize_env_key_for_platform("GIT_CEILING_DIRECTORIES")
        || normalized == normalize_env_key_for_platform("PATH")
        || normalized == normalize_env_key_for_platform("PATHEXT")
}

pub fn validate_user_env_key(key: &str, context: &str) -> Result<(), String> {
    if key.trim() != key {
        return Err(format!(
            "{context}: env key must not have leading or trailing whitespace"
        ));
    }
    if key.is_empty() {
        return Err(format!("{context}: env key must not be empty"));
    }
    if key.contains('=') {
        return Err(format!("{context}: env key '{key}' must not contain '='"));
    }
    if key.contains('\0') || key.contains('\n') || key.contains('\r') {
        return Err(format!(
            "{context}: env key '{key}' must not contain NUL or newline characters"
        ));
    }
    if is_reserved_env_key(key) {
        return Err(format!(
            "{context}: env key '{key}' is reserved by AgentsCommander"
        ));
    }
    Ok(())
}

fn validate_codex_home_basic<'a>(value: &'a str, context: &str) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{context}: CODEX_HOME must not be empty"));
    }
    if trimmed.contains('\0') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(format!(
            "{context}: CODEX_HOME must not contain NUL or newline characters"
        ));
    }
    Ok(trimmed)
}

pub fn validate_codex_home_template_value(value: &str, context: &str) -> Result<(), String> {
    let trimmed = validate_codex_home_basic(value, context)?;

    // Accept when the value's COMPLETE LEADING path segment is a known token.
    // Do NOT pick "first token contained anywhere" (it would name the wrong token
    // for a value that starts with one token but mentions another later; #576 R1 F3).
    let starts_with_token = AC_PLACEHOLDER_TOKENS.iter().any(|&token| {
        trimmed == token
            || trimmed.starts_with(&format!("{token}/"))
            || trimmed.starts_with(&format!("{token}\\"))
    });
    if starts_with_token {
        // Leading segment OK; the remainder scanner rejects any unknown %marker%
        // (and skips known tokens that appear later in the path).
        reject_unknown_codex_home_template_markers(trimmed, context)?;
        return Ok(());
    }

    // No valid leading token. If a known token still appears anywhere, the leading
    // segment is wrong: report the "complete path segment" error naming that token.
    if let Some(&token) = AC_PLACEHOLDER_TOKENS
        .iter()
        .find(|&&token| trimmed.contains(token))
    {
        return Err(format!(
            "{context}: CODEX_HOME template must start with {token} as a complete path segment"
        ));
    }

    // No token at all: must already be a literal absolute path.
    reject_unknown_codex_home_template_markers(trimmed, context)?;
    validate_expanded_codex_home_value(trimmed, context).map(|_| ())
}

pub fn validate_expanded_codex_home_value(value: &str, context: &str) -> Result<PathBuf, String> {
    let trimmed = validate_codex_home_basic(value, context)?;
    crate::config::placeholders::reject_unexpanded_markers(trimmed, context, true)?;
    if trimmed.contains('$') || trimmed.contains('%') {
        return Err(format!(
            "{context}: CODEX_HOME must be an absolute literal path without variable markers"
        ));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(format!("{context}: CODEX_HOME must be an absolute path"));
    }
    Ok(path)
}

pub fn validate_codex_home_value(value: &str, context: &str) -> Result<PathBuf, String> {
    validate_expanded_codex_home_value(value, context)
}

fn reject_unknown_codex_home_template_markers(value: &str, context: &str) -> Result<(), String> {
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        rest = &rest[start..];
        if let Some(&token) = AC_PLACEHOLDER_TOKENS
            .iter()
            .find(|&&token| rest.starts_with(token))
        {
            rest = &rest[token.len()..];
            continue;
        }
        if let Some(end) = rest[1..].find('%') {
            let marker = &rest[..end + 2];
            return Err(format!(
                "{context}: CODEX_HOME contains unknown placeholder {marker}"
            ));
        }
        break;
    }
    Ok(())
}

fn validate_env_map(env: &BTreeMap<String, String>, context: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    for (key, value) in env {
        validate_user_env_key(key, context)?;
        let normalized = normalize_env_key_for_platform(key);
        if !seen.insert(normalized) {
            return Err(format!("{context}: duplicate env key '{key}'"));
        }
        if is_codex_home_key(key) {
            validate_codex_home_template_value(value, context)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_env_rows(rows: &[CodingAgentEnv], context: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    for row in rows.iter().filter(|row| row.enabled) {
        validate_user_env_key(&row.key, context)?;
        let normalized = normalize_env_key_for_platform(&row.key);
        if !seen.insert(normalized) {
            return Err(format!("{context}: duplicate env key '{}'", row.key));
        }
        if is_codex_home_key(&row.key) {
            validate_codex_home_template_value(&row.value, context)?;
        }
    }
    Ok(())
}

pub fn normalize_container_image_input(value: &str, context: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{context} must not be empty"));
    }
    if trimmed.starts_with('-') {
        return Err(format!("{context} must not start with '-'"));
    }
    Ok(trimmed.to_string())
}

fn normalize_agent_backend_config(
    backend: &mut AgentBackendConfig,
    context: &str,
) -> Result<(), String> {
    if backend.kind == SessionBackendKind::LocalProcess {
        backend.image = None;
        return Ok(());
    }

    let Some(raw_image) = backend.image.take() else {
        return Ok(());
    };
    let trimmed = raw_image.trim();
    if trimmed.is_empty() {
        backend.image = None;
        return Ok(());
    }
    backend.image = Some(normalize_container_image_input(trimmed, context)?);
    Ok(())
}

fn normalize_agent_backend_configs(settings: &mut AppSettings) -> Result<(), String> {
    for agent in &mut settings.agents {
        normalize_agent_backend_config(
            &mut agent.backend,
            &format!("Agent \"{}\" container image", agent.label),
        )?;
    }
    Ok(())
}

pub fn validate_and_repair_settings(settings: &mut AppSettings) -> Result<(), String> {
    normalize_agent_backend_configs(settings)?;
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    validate_agent_commands(settings)?;
    validate_screenshot_hotkey(&settings.screenshot_capture_hotkey)?;
    validate_api_server_settings(settings)?;
    validate_resource_settings(settings)
}

pub(crate) fn parse_api_server_socket_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let bind = bind.trim();
    if bind.is_empty() {
        return Err("apiServerBind must not be empty".to_string());
    }
    if port == 0 {
        return Err("apiServerPort must be between 1 and 65535".to_string());
    }
    let ip: IpAddr = bind.parse().map_err(|e| {
        format!("apiServerBind must be an IP address such as 127.0.0.1, 0.0.0.0, ::1, or :: ({e})")
    })?;
    Ok(SocketAddr::new(ip, port))
}

pub fn validate_api_server_settings(settings: &mut AppSettings) -> Result<(), String> {
    let bind = settings.api_server_bind.trim().to_string();
    parse_api_server_socket_addr(&bind, settings.api_server_port)?;
    settings.api_server_bind = bind;
    Ok(())
}

/// #714 Reject a screenshot hotkey that the native parser cannot accept. Syntax
/// errors block a settings save; OS-level registration conflicts do not (they are
/// surfaced as runtime status by `screenshot::register_configured_hotkey`).
pub fn validate_screenshot_hotkey(value: &str) -> Result<(), String> {
    crate::screenshot::parse_screenshot_hotkey(value)
        .map(|_| ())
        .map_err(|e| format!("Screenshot hotkey: {}", e))
}

pub fn merge_protected_coding_agent_settings(
    current: &AppSettings,
    mut incoming: AppSettings,
) -> AppSettings {
    incoming.coding_agent_profiles = current.coding_agent_profiles.clone();

    for incoming_agent in &mut incoming.agents {
        if let Some(current_agent) = current.agents.iter().find(|a| a.id == incoming_agent.id) {
            incoming_agent.envs = current_agent.envs.clone();
            incoming_agent.isolated_home = current_agent.isolated_home;
        }
    }

    incoming
}

/// A config-seed `dest` must be a single, relative folder NAME under the replica
/// root. The convention prefixes (`default`, `default_profile_<letter>`) are
/// concatenated onto this name for the workspace-root tiers, so it must be a
/// single path segment and a legal Windows directory name.
pub fn validate_config_seed_dest(dest: &str) -> Result<(), String> {
    let d = dest.trim();
    if d.is_empty() {
        return Err("config seed destination must not be empty".to_string());
    }
    if d.contains('/') || d.contains('\\') {
        return Err(
            "config seed destination must be a single folder name (no path separators)".to_string(),
        );
    }
    if d == "." || d == ".." || d.contains("..") {
        return Err("config seed destination must not contain '..'".to_string());
    }
    if Path::new(d).is_absolute() {
        return Err("config seed destination must be relative".to_string());
    }
    // M5: ':' makes "C:foo" drive-relative (is_absolute()==false) or "x:y" an ADS.
    if d.contains(':') {
        return Err("config seed destination must not contain ':'".to_string());
    }
    if d.contains('%') || d.contains('$') {
        return Err("config seed destination must not contain placeholder markers".to_string());
    }
    // M5: reject a trailing dot (Windows strips it, causing target drift). A
    // trailing space cannot reach here: `d` was trimmed above.
    if d.ends_with('.') {
        return Err("config seed destination must not end with a dot".to_string());
    }
    // M5: reject Windows reserved device names (case-insensitive, before any extension).
    let stem = d.split('.').next().unwrap_or(d).to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!(
            "config seed destination '{}' is a reserved Windows device name",
            d
        ));
    }
    Ok(())
}

pub fn validate_agent_commands(settings: &AppSettings) -> Result<(), String> {
    for agent in &settings.agents {
        validate_agent_command_text(&format!("Agent \"{}\"", agent.label), &agent.command)?;

        validate_env_rows(
            &agent.envs,
            &format!("Agent \"{}\" env settings", agent.label),
        )?;

        // #529 (G8): reject a bad instructions filename here so the user gets a
        // clear IPC error at save, not a silent fallback at launch. Empty/absent
        // is allowed (means "use the command-derived default").
        if let Some(f) = agent.instructions_filename.as_deref() {
            let f = f.trim();
            if !f.is_empty() && !crate::config::agent_command::is_safe_instructions_filename(f) {
                return Err(format!(
                    "Agent \"{}\" instructions filename is invalid: must be a bare .md filename with no path separators",
                    agent.label
                ));
            }
        }

        // #598: reject a bad config-seed dest at save so the user gets a clear
        // IPC error here, not a silent fail-soft skip at launch. Inactive
        // (disabled or empty dest) is allowed (means "no seeding").
        if let Some(seed) = agent.config_seed.as_ref() {
            if seed.is_active() {
                validate_config_seed_dest(&seed.dest).map_err(|e| {
                    format!("Agent \"{}\" config seed is invalid: {}", agent.label, e)
                })?;
            }
        }
    }

    for (agent_id, cells) in &settings.coding_agent_profiles.profiles_by_agent {
        for (letter, cell) in cells {
            if !is_valid_profile_letter(letter) {
                return Err(format!(
                    "Coding agent profile cell '{}:{}' uses an invalid profile letter",
                    agent_id, letter
                ));
            }
            validate_env_map(
                &cell.env,
                &format!(
                    "Coding agent profile '{}:{}' env settings",
                    agent_id, letter
                ),
            )?;
            if cell.enabled && !cell.command.trim().is_empty() {
                // #597 - the cell holds params appended to the agent base command,
                // so validate the COMPOSED effective command. Otherwise a banned
                // provider flag (Claude --continue/-c or Codex/Gemini manual resume)
                // placed in the cell params escapes the check. Pi session selectors
                // are intentionally allowed and remain user-authoritative. The provider token
                // lives in the base, not the cell. Falls back to the cell text when
                // the cell references an agent id that has no configured agent.
                let base = settings
                    .agents
                    .iter()
                    .find(|a| a.id == *agent_id)
                    .map(|a| a.command.as_str())
                    .unwrap_or("");
                let effective =
                    crate::config::agent_command::compose_effective_command(base, &cell.command);
                validate_agent_command_text(
                    &format!("Coding agent profile '{}:{}'", agent_id, letter),
                    &effective,
                )?;
            }
        }
    }

    for (agent_name, letter) in &settings.coding_agent_profiles.default_profile_by_agent {
        if !is_valid_profile_letter(letter) {
            return Err(format!(
                "Coding agent default profile for '{}' must be A through Z",
                agent_name
            ));
        }
    }

    Ok(())
}

pub(crate) fn validate_agent_command_text(context: &str, command: &str) -> Result<(), String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command)
        .map_err(|e| format!("{context}: invalid command: {e}. command={command:?}"))?;

    // Pi selectors are user-authored configuration and intentionally outrank AC
    // automation. Canonical Pi identity must win before the independent legacy
    // provider-token scans below inspect model or provider option values.
    if CodingAgentKind::detect(&normalized.shell, &normalized.shell_args)
        == Some(CodingAgentKind::Pi)
    {
        return Ok(());
    }

    let mut token_strings = Vec::with_capacity(normalized.shell_args.len() + 1);
    token_strings.push(normalized.shell);
    token_strings.extend(normalized.shell_args);
    let tokens: Vec<&str> = token_strings.iter().map(String::as_str).collect();

    if let Some(claude_idx) = find_provider_token(&tokens, "claude") {
        if tokens[claude_idx + 1..].iter().any(|token| {
            token.eq_ignore_ascii_case("--continue") || token.eq_ignore_ascii_case("-c")
        }) {
            return Err(format!(
                "{context}: Claude commands must not include --continue or -c"
            ));
        }
    }

    if let Some(codex_idx) = find_provider_token(&tokens, "codex") {
        if codex_has_manual_resume(&tokens, codex_idx) {
            return Err(format!(
                "{context}: Codex commands must not include resume or --last; AgentsCommander injects codex resume --last automatically"
            ));
        }
    }

    if let Some(gemini_idx) = find_provider_token(&tokens, "gemini") {
        if gemini_has_manual_resume(&tokens, gemini_idx) {
            return Err(format!(
                "{context}: Gemini commands must not include --resume; AgentsCommander injects gemini --resume latest automatically"
            ));
        }
    }

    Ok(())
}

pub fn validate_resource_settings(settings: &AppSettings) -> Result<(), String> {
    // Floor at 1 (a value of 0 would block every agent launch); no upper ceiling.
    // The user sets this deliberately and is responsible for sizing it to their machine.
    if settings.max_concurrent_agent_processes == 0 {
        return Err("maxConcurrentAgentProcesses must be at least 1".to_string());
    }
    if settings.agent_group_warn_private_bytes > settings.agent_group_kill_private_bytes {
        return Err(
            "agentGroupWarnPrivateBytes must be less than or equal to agentGroupKillPrivateBytes"
                .to_string(),
        );
    }
    if settings.agent_group_kill_private_bytes == 0 {
        return Err("agentGroupKillPrivateBytes must be greater than 0".to_string());
    }
    if settings.agent_process_kill_private_bytes == 0 {
        return Err("agentProcessKillPrivateBytes must be greater than 0".to_string());
    }
    Ok(())
}

fn settings_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("settings.json"))
}

/// Load settings from the app config directory (see config_dir()), falling back to defaults.
/// Auto-generates a root_token if missing and persists it.
pub fn load_settings() -> AppSettings {
    let path = match settings_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory, using defaults");
            return AppSettings::default();
        }
    };

    load_settings_from_path(&path)
}

fn load_settings_from_path(path: &Path) -> AppSettings {
    let mut profile_migrated_to_v2 = false;
    let mut pre_migration_contents: Option<String> = None;
    let mut settings = if !path.exists() {
        log::info!("No settings file found at {:?}, using defaults", path);
        AppSettings::default()
    } else {
        match std::fs::read_to_string(path) {
            Ok(contents) => match parse_settings_json(&contents, &path.to_string_lossy()) {
                Ok((s, migrated)) => {
                    log::debug!("Loaded settings from {:?}", path);
                    if migrated {
                        profile_migrated_to_v2 = true;
                        pre_migration_contents = Some(contents);
                    }
                    s
                }
                Err(e) => {
                    log::error!("{}", e);
                    AppSettings::default()
                }
            },
            Err(e) => {
                log::error!("Failed to read settings file: {}", e);
                AppSettings::default()
            }
        }
    };

    // 0.8.0 unified-window migration — seed main_* from legacy fields on first load
    // after upgrade. Runs BEFORE root_token auto-gen so the migrated values persist
    // via the same save. The deprecated `sidebar_geometry`/`terminal_geometry` fields
    // are automatically dropped from disk by `skip_serializing_if` on the next save.
    if settings.main_geometry.is_none() {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
            log::info!("[settings-migration] seeded main_geometry from legacy terminal_geometry");
        }
    }
    // Seed main_zoom from sidebar_zoom on first boot. EPSILON guard: avoid clobbering
    // a user-set main_zoom=1.0 (which would equal default_zoom) with an effectively-unity
    // sidebar_zoom. See A3.10 / Arb-2.
    if (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
        log::info!("[settings-migration] seeded main_zoom from legacy sidebar_zoom");
    }
    // Seed main_always_on_top from legacy sidebar_always_on_top.
    if !settings.main_always_on_top && settings.sidebar_always_on_top {
        settings.main_always_on_top = true;
        log::info!(
            "[settings-migration] seeded main_always_on_top from legacy sidebar_always_on_top"
        );
    }

    // #248 migration — translates legacy startOnlyCoordinators (if present).
    // Track whether the legacy field was on disk so we can fire a single save
    // at the end of the function (Grinch Z3 — without this, upgrade users
    // with an existing root_token never persist the migration and the legacy
    // key lingers in settings.json, spamming the migration log on every launch).
    let issue_248_migrated = settings.legacy_start_only_coordinators.is_some();
    apply_issue_248_migration(&mut settings);

    // Auto-generate root token if missing.
    let mut needs_save = issue_248_migrated || profile_migrated_to_v2;
    if repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents) {
        log::info!("[settings-migration] repaired codingAgentProfiles invariants");
        needs_save = true;
    }
    if settings.root_token.is_none() {
        settings.root_token = Some(uuid::Uuid::new_v4().to_string());
        log::info!("Generated new root token");
        needs_save = true;
    }
    if needs_save {
        let backup_ok = if profile_migrated_to_v2 {
            match pre_migration_contents.as_deref() {
                Some(contents) => match write_pre_384_v1_backup(path, contents) {
                    Ok(()) => true,
                    Err(e) => {
                        log::error!(
                            "Failed to persist settings v2 migration backup; leaving settings.json untouched: {}",
                            e
                        );
                        false
                    }
                },
                None => true,
            }
        } else {
            true
        };
        if backup_ok {
            // #1077: route the startup root-token/migration write through PRESERVE
            // mode so it cannot erase project companions/conflicts before the
            // first snapshot, and so a present-but-invalid file is never
            // overwritten (the preserve writer's disk gate returns Err). On
            // success adopt the fresh-decoded settings so runtime/hidden agree.
            match save_settings_to_path_preserving_project_paths(&settings, path) {
                Ok(written) => settings = written,
                Err(e) => log::error!(
                    "Failed to persist settings (root_token gen and/or settings migration): {}",
                    e
                ),
            }
        }
    }

    settings
}

fn write_pre_384_v1_backup(settings_path: &Path, contents: &str) -> Result<(), String> {
    let backup_path = settings_path.with_file_name("settings.pre-384-v1.json");
    if backup_path.exists() {
        return Ok(());
    }
    std::fs::write(&backup_path, contents)
        .map_err(|e| format!("Failed to write {}: {}", backup_path.display(), e))?;
    log::info!(
        "[settings-migration] wrote pre-384 v1 settings backup to {:?}",
        backup_path
    );
    Ok(())
}

/// CLI-only variant of `load_settings`. Reads disk and applies the same
/// in-memory migrations as `load_settings`, but does NOT auto-generate or
/// persist a `root_token`. Used by CLI verbs that mutate settings
/// (`open-project`, `new-project`) so error paths and pre-validation reads
/// do NOT silently rewrite `settings.json` (Round-1 G5 in #191's plan).
///
/// The CLI verbs do not consume the root_token; if a future verb needs it,
/// `settings.root_token == None` on a brand-new install is fine — the CLI
/// is read-only with respect to it. The GUI still owns root_token
/// generation via the next `load_settings()` call when it boots.
///
/// **Migration duplication is intentional for this PR.** Extracting an
/// `apply_in_memory_migrations(&mut AppSettings)` helper that both loaders
/// share is a clean follow-up, but pulls in scope outside #191 (touches
/// `load_settings`'s control flow). Keep both copies in lockstep until
/// then; if you add a new in-memory migration to `load_settings`, mirror
/// it here too.
pub fn load_settings_for_cli() -> AppSettings {
    let path = match settings_path() {
        Some(p) => p,
        None => {
            log::warn!("[cli] Could not determine home directory, using defaults");
            return AppSettings::default();
        }
    };

    let mut settings = if !path.exists() {
        log::info!("[cli] No settings file found at {:?}, using defaults", path);
        AppSettings::default()
    } else {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match parse_settings_json(&contents, &path.to_string_lossy()) {
                Ok((s, _migrated)) => {
                    log::debug!("[cli] Loaded settings from {:?}", path);
                    s
                }
                Err(e) => {
                    log::error!("[cli] {}", e);
                    AppSettings::default()
                }
            },
            Err(e) => {
                log::error!("[cli] Failed to read settings file: {}", e);
                AppSettings::default()
            }
        }
    };

    // 0.8.0 unified-window migration — must mirror `load_settings` exactly,
    // EXCEPT for the root_token auto-gen + save_settings call.
    if settings.main_geometry.is_none() {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings.main_always_on_top && settings.sidebar_always_on_top {
        settings.main_always_on_top = true;
    }

    // #248 migration — translate in-memory only. The CLI loader is forbidden
    // from writing settings.json per the §463 contract (load_settings_for_cli
    // is the read-only variant used by mutating verbs like `open-project` and
    // `new-project`; it must not race with the GUI's settings writes). The
    // next GUI launch finalizes the migration to disk via load_settings.
    apply_issue_248_migration(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    // NO root_token auto-gen, NO save_settings call.
    settings
}

/// #786 R1: strict CLI loader for MUTATING paths and `list`/`show`. Identical to
/// `load_settings_for_cli` EXCEPT it distinguishes an ABSENT settings.json (fine:
/// start from default) from a PRESENT-but-unparseable one (Err: refuse to touch
/// it). This closes the silent-wipe hole: `load_settings_for_cli` returns a
/// default on a strict parse error, and a subsequent `save_settings` (which
/// preserves only `project_paths`, via a LENIENT `Value` read) would rewrite
/// settings.json to defaults, destroying every agent/profile/hotkey. NEVER call
/// the silent-default `load_settings_for_cli` for a write.
///
/// Keep the in-memory migration tail in lockstep with `load_settings_for_cli`
/// (see the note above that function).
pub fn load_settings_for_cli_strict() -> Result<AppSettings, String> {
    let mut settings = match settings_path() {
        // No home dir to locate settings.json: nothing to protect, and a later
        // save would fail to resolve the dir anyway. Start from default.
        None => AppSettings::default(),
        Some(path) if !path.exists() => AppSettings::default(),
        Some(path) => {
            let contents = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "settings.json exists at {} but could not be read ({e}); refusing to modify it - fix or remove the file first",
                    path.display()
                )
            })?;
            let (s, _migrated) =
                parse_settings_json(&contents, &path.to_string_lossy()).map_err(|e| {
                    format!(
                        "settings.json exists but could not be parsed ({e}); refusing to modify it - fix or remove the file first"
                    )
                })?;
            s
        }
    };

    // Mirror `load_settings_for_cli`'s in-memory migrations (no disk write).
    if settings.main_geometry.is_none() {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings.main_always_on_top && settings.sidebar_always_on_top {
        settings.main_always_on_top = true;
    }
    apply_issue_248_migration(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    Ok(settings)
}

/// One-shot migration for issue #248: translate the legacy
/// `startOnlyCoordinators` field into the new state-sensitive
/// `restore_coordinator_wake_state`. Idempotent — once the legacy carrier is
/// cleared (`.take()`), subsequent calls see `None` and do nothing.
///
/// Translation rules:
///   - legacy `true`  → new `true`  (preserve "smart startup" intent under new semantics).
///   - legacy `false` → new `false` (legacy "wake everything" mode is removed; closest
///     equivalent under the new model is "wake nothing").
///
/// Conflict handling (Grinch Z4): if the user (or a third-party tool) wrote
/// BOTH keys and the new field already differs from the legacy intent, emit a
/// `warn!` and keep the new field's existing value — never silently overwrite
/// a deliberate `restoreCoordinatorWakeState` with a stale legacy value.
fn apply_issue_248_migration(settings: &mut AppSettings) {
    if let Some(legacy) = settings.legacy_start_only_coordinators.take() {
        if !settings.restore_coordinator_wake_state {
            settings.restore_coordinator_wake_state = legacy;
            log::info!(
                "[settings-migration] #248 — translated legacy startOnlyCoordinators={} → restoreCoordinatorWakeState={}",
                legacy, settings.restore_coordinator_wake_state
            );
        } else if legacy != settings.restore_coordinator_wake_state {
            log::warn!(
                "[settings-migration] #248 — conflicting state on disk: legacy startOnlyCoordinators={} but restoreCoordinatorWakeState={} already set; keeping the new value, dropping legacy",
                legacy, settings.restore_coordinator_wake_state
            );
        }
        // else: legacy and new agree → silent drop, no log.
    }
}

/// Read only the `logLevel` field from `settings.json` without triggering migrations,
/// auto-token-gen, or any in-memory mutation. Used by `lib.rs` at logger-init time so
/// the full `load_settings` flow can run post-init with log calls captured.
///
/// Returns `None` on missing file, missing field, malformed JSON, unreadable filesystem,
/// or any other read error — fully read-only and side-effect-free.
fn read_log_level_from_path(path: &std::path::Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
    v.get("logLevel")?.as_str().map(String::from)
}

/// See `read_log_level_from_path`. Resolves the canonical settings path and delegates.
pub fn read_log_level_only() -> Option<String> {
    read_log_level_from_path(&settings_path()?)
}

/// #774: counter feeding the per-call unique temp filename for settings saves.
/// Combined with the PID it makes `settings.json.<pid>.<op_id>.tmp` distinct from
/// any concurrent cross-process save (GUI startup, CLI verbs, closing flush) and
/// from any leftover temp written by a prior crashed run. Mirrors
/// `sessions_persistence::SAVE_OP_ID` (sessions_persistence.rs:47); a separate
/// counter so the two can be reasoned about independently in diagnostics.
static SAVE_OP_ID: AtomicU64 = AtomicU64::new(0);

// ── #1077 six-field project-path codec (JSON extraction / serialization) ────
//
// The three legacy primary fields keep their names/types; each gains a paired
// `…RelativeToInstance` companion (string|null, array-aligned). On load the
// codec resolves both candidates, replaces the runtime fields with the SELECTED
// canonical paths, and stashes the raw pairs/outcomes in the hidden
// `AppSettings.project_path_state`. On write, the preserve/reconcile modes
// re-inject all six fields (companions are `#[serde(skip)]`, so a plain
// AppSettings serialization would otherwise drop them). See plan #1077 §3.2-§4.3.

use super::projects::{
    self, ProjectPathPersistenceState, ProjectSource, RawJsonField, RawPair, RawStringField,
    RepairKind, ResolvedPair, SideOutcome, SideStatus, StructuralIssue,
};

/// An absent-side outcome (used when synthesizing a legacy hidden state).
fn absent_side() -> SideOutcome {
    SideOutcome {
        status: SideStatus::Absent,
        syntactic_path: None,
        canonical_path: None,
        identity: None,
    }
}

const FIELD_PROJECT_PATH: &str = "projectPath";
const FIELD_PROJECT_PATH_REL: &str = "projectPathRelativeToInstance";
const FIELD_PROJECT_PATHS: &str = "projectPaths";
const FIELD_PROJECT_PATHS_REL: &str = "projectPathsRelativeToInstance";
const FIELD_ARCHIVED: &str = "archivedProjectPaths";
const FIELD_ARCHIVED_REL: &str = "archivedProjectPathsRelativeToInstance";

/// The authoritative instance base for path pairing, canonicalized at the codec
/// boundary. `None` in any degraded mode (no base, or a base that fails to
/// canonicalize); never falls back to the process CWD.
fn production_instance_base() -> Option<PathBuf> {
    let base = super::instance_base()?;
    std::fs::canonicalize(&base).ok()
}

/// Presence + JSON value of a raw field (absent vs null vs value).
fn json_field(v: Option<&Value>) -> RawJsonField {
    match v {
        None => RawJsonField {
            present: false,
            value: None,
        },
        Some(Value::Null) => RawJsonField {
            present: true,
            value: None,
        },
        Some(other) => RawJsonField {
            present: true,
            value: Some(other.clone()),
        },
    }
}

/// Extract one plural group (primary + companion arrays) into ordered raw pairs,
/// returning the companion-present bit and any structural corruption. On
/// corruption, no entry is exposed (empty pairs) and the raw values are
/// preserved via the returned `StructuralIssue`.
fn extract_group(
    root: &Map<String, Value>,
    source: ProjectSource,
    primary_key: &str,
    companion_key: &str,
) -> (Vec<RawPair>, bool, Option<StructuralIssue>) {
    let primary = root.get(primary_key);
    let companion = root.get(companion_key);
    let companion_present = companion.is_some();
    let raw_absolute = json_field(primary);
    let raw_relative = json_field(companion);
    let structural = |reason: &str| StructuralIssue {
        source,
        reason: reason.to_string(),
        raw_absolute: raw_absolute.clone(),
        raw_relative: raw_relative.clone(),
    };
    let corrupt = |reason: &str| (Vec::new(), companion_present, Some(structural(reason)));

    // Companion present while primary absent/null → structural corruption.
    if companion_present {
        match primary {
            None => return corrupt("companion present while primary field is absent"),
            Some(Value::Null) => return corrupt("companion present while primary field is null"),
            _ => {}
        }
    }

    // Interpret the primary field.
    let primary_list: Vec<String> = match primary {
        // Absent or null → no active disk truth (a valid empty group here).
        None | Some(Value::Null) => return (Vec::new(), companion_present, None),
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                match el {
                    Value::String(s) => out.push(s.clone()),
                    _ => return corrupt("plural primary contains a non-string element"),
                }
            }
            out
        }
        Some(_) => return corrupt("plural primary is not an array of strings"),
    };

    // Interpret the companion field.
    let companion_list: Option<Vec<RawStringField>> = match companion {
        None => None,
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                match el {
                    Value::String(s) => out.push(RawStringField::string(s.clone())),
                    Value::Null => out.push(RawStringField::null()),
                    _ => return corrupt("plural companion contains a non-string/non-null element"),
                }
            }
            Some(out)
        }
        Some(Value::Null) => return corrupt("plural companion is a present null"),
        Some(_) => return corrupt("plural companion is not an array"),
    };

    if let Some(ref comp) = companion_list {
        if comp.len() != primary_list.len() {
            return corrupt("companion array length differs from primary array");
        }
    }

    let pairs = primary_list
        .into_iter()
        .enumerate()
        .map(|(i, abs)| RawPair {
            source,
            index: Some(i),
            absolute: RawStringField::string(abs),
            relative: companion_list
                .as_ref()
                .map(|c| c[i].clone())
                .unwrap_or_else(RawStringField::absent),
        })
        .collect();

    (pairs, companion_present, None)
}

/// Extract the legacy singular pair, or a structural corruption.
fn extract_singular(root: &Map<String, Value>) -> (Option<RawPair>, Option<StructuralIssue>) {
    let primary = root.get(FIELD_PROJECT_PATH);
    let companion = root.get(FIELD_PROJECT_PATH_REL);
    let raw_absolute = json_field(primary);
    let raw_relative = json_field(companion);
    let structural = |reason: &str| StructuralIssue {
        source: ProjectSource::ProjectPath,
        reason: reason.to_string(),
        raw_absolute: raw_absolute.clone(),
        raw_relative: raw_relative.clone(),
    };

    let primary_field = match primary {
        None => RawStringField::absent(),
        Some(Value::Null) => RawStringField::null(),
        Some(Value::String(s)) => RawStringField::string(s.clone()),
        Some(_) => {
            return (
                None,
                Some(structural("projectPath is not a string or null")),
            )
        }
    };
    let companion_field = match companion {
        None => RawStringField::absent(),
        Some(Value::Null) => RawStringField::null(),
        Some(Value::String(s)) => RawStringField::string(s.clone()),
        Some(_) => {
            return (
                None,
                Some(structural(
                    "projectPathRelativeToInstance is not a string or null",
                )),
            )
        }
    };
    if companion.is_some() && primary.is_none() {
        return (
            None,
            Some(structural(
                "singular companion present while projectPath is absent",
            )),
        );
    }
    if matches!(companion, Some(Value::String(_))) && matches!(primary, Some(Value::Null)) {
        return (
            None,
            Some(structural(
                "non-null singular companion paired with null projectPath",
            )),
        );
    }

    // A pair exists only when the primary is an actual string candidate; a
    // null/absent primary is the valid empty singular.
    let has_primary = primary_field.value.is_some();
    let pair = has_primary.then_some(RawPair {
        source: ProjectSource::ProjectPath,
        index: None,
        absolute: primary_field,
        relative: companion_field,
    });
    (pair, None)
}

/// Decode the six raw project fields from a settings object into the hidden
/// persistence state (selected paths, raw pairs, outcomes, structural issues).
pub(crate) fn decode_project_state(
    root: &Map<String, Value>,
    base: Option<&Path>,
    resolver: &dyn projects::CandidateResolver,
) -> ProjectPathPersistenceState {
    let (active_pairs, active_companion, active_structural) = extract_group(
        root,
        ProjectSource::ProjectPaths,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
    );
    let (archived_pairs, archived_companion, archived_structural) = extract_group(
        root,
        ProjectSource::ArchivedProjectPaths,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    );
    let (singular, singular_structural) = extract_singular(root);

    // A structurally-corrupt active group exposes no active entry at all,
    // including its singular mirror.
    let effective_singular = if active_structural.is_some() {
        None
    } else {
        singular
    };

    let mut state = projects::resolve_registrations(
        &active_pairs,
        effective_singular,
        &archived_pairs,
        base,
        active_companion,
        archived_companion,
        resolver,
    );

    let mut structural = Vec::new();
    structural.extend(active_structural);
    structural.extend(archived_structural);
    structural.extend(singular_structural);
    if !structural.is_empty() {
        // Any structural corruption blocks all reconciliation/mutation.
        state.active_reconcile_eligible = false;
        state.archived_reconcile_eligible = false;
    }
    state.structural_issues = structural;
    state
}

/// Decode the six fields from `value`, then overwrite the three primary runtime
/// fields with the selected values and drop the companion keys so the remainder
/// of `AppSettings` deserializes cleanly even when a project field was
/// wrong-typed. Returns the hidden state to attach.
pub(crate) fn apply_project_decode_to_value(
    value: &mut Value,
    base: Option<&Path>,
    resolver: &dyn projects::CandidateResolver,
) -> ProjectPathPersistenceState {
    let state = match value.as_object() {
        Some(root) => decode_project_state(root, base, resolver),
        None => ProjectPathPersistenceState::default(),
    };
    if let Some(root) = value.as_object_mut() {
        root.insert(
            FIELD_PROJECT_PATH.to_string(),
            state
                .selected_head
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        root.insert(
            FIELD_PROJECT_PATHS.to_string(),
            Value::Array(
                state
                    .active_selected()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        root.insert(
            FIELD_ARCHIVED.to_string(),
            Value::Array(
                state
                    .archived_management_paths()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        // Companion keys are not AppSettings fields; strip so nothing lingers.
        root.remove(FIELD_PROJECT_PATH_REL);
        root.remove(FIELD_PROJECT_PATHS_REL);
        root.remove(FIELD_ARCHIVED_REL);
    }
    state
}

/// The project-field write mode. `Preserve` copies the six raw fields from the
/// fresh disk object (materializing a group only when disk has no truth for it);
/// `Reconcile` rebuilds the requested eligible group(s) from hidden state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectWriteMode {
    Preserve,
    Reconcile { active: bool, archived: bool },
}

/// A slot value for a retained (unresolved/conflict/missing) raw field: string
/// stays a string, absent/null becomes null (inside an array there is no absent).
fn raw_slot_value(field: &RawStringField) -> Value {
    match &field.value {
        Some(s) => Value::String(s.clone()),
        None => Value::Null,
    }
}

/// The companion wire value for a selected pair: encode against the base when
/// available; otherwise preserve any existing raw companion (or null).
fn companion_slot_value(pair: &ResolvedPair, base: Option<&Path>) -> Value {
    match base {
        Some(base) => match &pair.selected_canonical_raw {
            Some(canon) => match projects::encode_instance_relative(Path::new(canon), base) {
                Some(wire) => Value::String(wire),
                None => Value::Null,
            },
            None => raw_slot_value(&pair.raw_relative),
        },
        None => raw_slot_value(&pair.raw_relative),
    }
}

/// Rebuild one group's (primary array, companion array) from its resolved pairs:
/// emit selected pairs (repaired), retain unresolved pairs value-for-value, and
/// drop silent duplicates.
fn rebuild_group_arrays(pairs: &[ResolvedPair], base: Option<&Path>) -> (Vec<Value>, Vec<Value>) {
    let mut primary = Vec::new();
    let mut companion = Vec::new();
    for pair in pairs {
        if let Some(sel) = &pair.selected {
            primary.push(Value::String(sel.clone()));
            companion.push(companion_slot_value(pair, base));
        } else if pair.issue.is_some() {
            primary.push(raw_slot_value(&pair.raw_absolute));
            companion.push(raw_slot_value(&pair.raw_relative));
        }
        // else: silent dedupe drop — emit nothing.
    }
    (primary, companion)
}

/// Insert the four active fields from hidden state.
fn write_active_group(
    out: &mut Map<String, Value>,
    state: &ProjectPathPersistenceState,
    base: Option<&Path>,
) {
    let (primary, companion) = rebuild_group_arrays(state.active_pairs(), base);
    out.insert(
        FIELD_PROJECT_PATH.to_string(),
        primary.first().cloned().unwrap_or(Value::Null),
    );
    out.insert(
        FIELD_PROJECT_PATH_REL.to_string(),
        companion.first().cloned().unwrap_or(Value::Null),
    );
    out.insert(FIELD_PROJECT_PATHS.to_string(), Value::Array(primary));
    out.insert(FIELD_PROJECT_PATHS_REL.to_string(), Value::Array(companion));
}

/// Insert the two archived fields from hidden state.
fn write_archived_group(
    out: &mut Map<String, Value>,
    state: &ProjectPathPersistenceState,
    base: Option<&Path>,
) {
    let (primary, companion) = rebuild_group_arrays(state.archived_pairs(), base);
    out.insert(FIELD_ARCHIVED.to_string(), Value::Array(primary));
    out.insert(FIELD_ARCHIVED_REL.to_string(), Value::Array(companion));
}

/// Copy a field from `disk` into `out`, preserving its present/absent bit.
fn copy_or_remove(out: &mut Map<String, Value>, disk: &Map<String, Value>, key: &str) {
    match disk.get(key) {
        Some(v) => {
            out.insert(key.to_string(), v.clone());
        }
        None => {
            out.remove(key);
        }
    }
}

/// Whether the disk object has authoritative active truth (§4.1): `projectPaths`
/// is an array, or (absent/null plural) `projectPath` is a string.
fn active_has_disk_truth(disk: &Map<String, Value>) -> bool {
    match disk.get(FIELD_PROJECT_PATHS) {
        Some(Value::Array(_)) => true,
        None | Some(Value::Null) => matches!(disk.get(FIELD_PROJECT_PATH), Some(Value::String(_))),
        Some(_) => true, // wrong-typed: structural, but still "present" disk truth to preserve
    }
}

/// Whether the disk object has authoritative archived truth: `archivedProjectPaths`
/// is an array (or any present value to preserve).
fn archived_has_disk_truth(disk: &Map<String, Value>) -> bool {
    matches!(disk.get(FIELD_ARCHIVED), Some(v) if !v.is_null())
}

/// If `AppSettings` was constructed directly (empty hidden state) yet carries
/// runtime project lists, synthesize a legacy absolute-only state so the writers
/// behave like a legacy-decoded file (companions null, all entries selected).
fn hidden_state_for_write(
    settings: &AppSettings,
) -> std::borrow::Cow<'_, ProjectPathPersistenceState> {
    let state = &settings.project_path_state;
    let empty = state.pairs.is_empty() && state.structural_issues.is_empty();
    let runtime_nonempty = settings.project_path.is_some()
        || !settings.project_paths.is_empty()
        || !settings.archived_project_paths.is_empty();
    if empty && runtime_nonempty {
        std::borrow::Cow::Owned(synthesize_legacy_state(settings))
    } else {
        std::borrow::Cow::Borrowed(state)
    }
}

fn legacy_selected_pair(source: ProjectSource, index: Option<usize>, path: &str) -> ResolvedPair {
    ResolvedPair {
        source,
        index,
        raw_absolute: RawStringField::string(path),
        raw_relative: RawStringField::absent(),
        absolute_side: absent_side(),
        relative_side: absent_side(),
        selected: Some(path.to_string()),
        selected_canonical_raw: None,
        selected_identity: None,
        issue: None,
        repair: RepairKind::None,
    }
}

fn synthesize_legacy_state(settings: &AppSettings) -> ProjectPathPersistenceState {
    let mut pairs: Vec<ResolvedPair> = settings
        .project_paths
        .iter()
        .enumerate()
        .map(|(i, p)| legacy_selected_pair(ProjectSource::ProjectPaths, Some(i), p))
        .collect();
    let active_registration_count = pairs.len();
    for (i, p) in settings.archived_project_paths.iter().enumerate() {
        pairs.push(legacy_selected_pair(
            ProjectSource::ArchivedProjectPaths,
            Some(i),
            p,
        ));
    }
    ProjectPathPersistenceState {
        pairs,
        selected_head: settings.project_paths.first().cloned(),
        active_registration_count,
        archived_registration_count: settings.archived_project_paths.len(),
        active_companion_present: false,
        archived_companion_present: false,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: Vec::new(),
        runtime_authoritative: true,
    }
}

/// A selected pair for a runtime path, canonicalizing so the reconcile write can
/// encode a real instance-relative companion (a non-existent path yields a null
/// companion).
fn resynced_selected_pair(source: ProjectSource, index: usize, path: &str) -> ResolvedPair {
    let canonical_raw = std::fs::canonicalize(path)
        .ok()
        .and_then(|c| c.to_str().map(str::to_string));
    ResolvedPair {
        source,
        index: Some(index),
        raw_absolute: RawStringField::string(path),
        raw_relative: RawStringField::absent(),
        absolute_side: absent_side(),
        relative_side: absent_side(),
        selected: Some(path.to_string()),
        selected_canonical_raw: canonical_raw,
        selected_identity: None,
        issue: None,
        repair: RepairKind::PopulateCompanion,
    }
}

/// #1077: after an explicit project mutation updated the three runtime lists,
/// rebuild the hidden state to match while retaining any unresolved
/// (conflict/missing/invalid) pairs from the pre-mutation state value-for-value,
/// so a register/remove/archive/unarchive never drops a preserved conflict or
/// missing record. The result is runtime-authoritative: a Reconcile write emits
/// the runtime selection plus the retained raw records. Callers must reject a
/// structurally-corrupt or conflict-scoped mutation BEFORE calling this.
pub(crate) fn resync_project_state_from_runtime(settings: &mut AppSettings) {
    let old = settings.project_path_state.clone();
    let retained_active: Vec<ResolvedPair> = old
        .active_pairs()
        .iter()
        .filter(|p| p.issue.is_some())
        .cloned()
        .collect();
    let retained_archived: Vec<ResolvedPair> = old
        .archived_pairs()
        .iter()
        .filter(|p| p.issue.is_some())
        .cloned()
        .collect();

    let active = reconcile_group_with_retained(
        &settings.project_paths,
        ProjectSource::ProjectPaths,
        retained_active,
    );
    let active_registration_count = active.len();
    let archived = reconcile_group_with_retained(
        &settings.archived_project_paths,
        ProjectSource::ArchivedProjectPaths,
        retained_archived,
    );
    let archived_registration_count = archived.len();

    let mut pairs = active;
    pairs.extend(archived);

    settings.project_path_state = Arc::new(ProjectPathPersistenceState {
        pairs,
        selected_head: settings.project_paths.first().cloned(),
        active_registration_count,
        archived_registration_count,
        active_companion_present: old.active_companion_present,
        archived_companion_present: old.archived_companion_present,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: old.structural_issues.clone(),
        runtime_authoritative: true,
    });
}

/// Rebuild one group's pairs from the mutated `runtime` list while reconciling
/// the pre-mutation `retained` unresolved (missing/conflict/invalid) records:
///
/// - a runtime entry that matches a retained record by normalized key reuses
///   that record IN PLACE (preserving its raw values, companion, and issue), so
///   a preserved conflict/missing record is never rebuilt as "selected" nor
///   duplicated;
/// - a runtime entry with no retained match becomes a fresh selected pair;
/// - a retained record whose key is absent from the runtime list was removed by
///   the mutation and is dropped (so "Remove from list" actually removes it).
fn reconcile_group_with_retained(
    runtime: &[String],
    source: ProjectSource,
    mut retained: Vec<ResolvedPair>,
) -> Vec<ResolvedPair> {
    let mut out = Vec::with_capacity(runtime.len());
    for (i, path) in runtime.iter().enumerate() {
        let key = projects::normalize_for_compare(path);
        let matched = retained.iter().position(|r| {
            r.raw_absolute
                .value
                .as_deref()
                .map(projects::normalize_for_compare)
                == Some(key.clone())
        });
        match matched {
            Some(pos) => {
                let mut pair = retained.remove(pos);
                pair.source = source;
                pair.index = Some(i);
                out.push(pair);
            }
            None => out.push(resynced_selected_pair(source, i, path)),
        }
    }
    // Any retained record not matched by the runtime list was removed; drop it.
    out
}

/// Fresh-read the whole disk object for a project-preserving/reconciling write.
/// `None` = absent (materialize). `Err` = present-but-unreadable/invalid JSON,
/// non-object root, or non-project settings that fail to deserialize — in which
/// case the caller must NOT write (§4.1). A valid object is returned for reuse.
fn read_disk_object_for_write(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "Failed to read {} for a project-preserving save (aborting to avoid dropping a project): {e}",
            path.display()
        )),
        Ok(contents) => {
            let value: Value = serde_json::from_str(&contents).map_err(|e| {
                format!(
                    "Refusing to overwrite {}: the existing settings file is not valid JSON ({e})",
                    path.display()
                )
            })?;
            match value {
                Value::Object(map) => {
                    validate_non_project_settings(&map)?;
                    Ok(Some(map))
                }
                _ => Err(format!(
                    "Refusing to overwrite {}: the existing settings root is not a JSON object",
                    path.display()
                )),
            }
        }
    }
}

/// Confirm the non-project part of a disk object still deserializes as
/// `AppSettings` (after neutralizing the six project fields), so a present file
/// with corrupt non-project settings is never silently overwritten.
fn validate_non_project_settings(disk: &Map<String, Value>) -> Result<(), String> {
    let mut probe = Value::Object(disk.clone());
    if let Some(root) = probe.as_object_mut() {
        root.insert(FIELD_PROJECT_PATH.to_string(), Value::Null);
        root.insert(FIELD_PROJECT_PATHS.to_string(), Value::Array(Vec::new()));
        root.insert(FIELD_ARCHIVED.to_string(), Value::Array(Vec::new()));
        root.remove(FIELD_PROJECT_PATH_REL);
        root.remove(FIELD_PROJECT_PATHS_REL);
        root.remove(FIELD_ARCHIVED_REL);
    }
    migrate_settings_value_to_v2(&mut probe);
    serde_json::from_value::<AppSettings>(probe)
        .map(|_| ())
        .map_err(|e| {
            format!(
                "Refusing to overwrite present settings whose non-project fields are invalid: {e}"
            )
        })
}

/// #1077 automatic reconciliation boundary (§4.3): reconcile the requested
/// eligible project group(s) from `settings`' hidden state to `path`, returning
/// the fresh-decoded settings. A structurally-corrupt or no-eligible-repair
/// state performs no write and returns the settings unchanged.
pub(crate) fn reconcile_project_state_to_path(
    settings: &AppSettings,
    path: &Path,
    active: bool,
    archived: bool,
) -> Result<AppSettings, String> {
    save_settings_value(
        settings,
        path,
        ProjectWriteMode::Reconcile { active, archived },
    )
}

/// The #1077 project-aware atomic writer. Builds the output object per `mode`,
/// writes it atomically, then re-decodes the exact written value so the returned
/// `AppSettings` carries fresh runtime projections + hidden state (never the
/// possibly-stale caller state). See §4.2/§4.3.
fn save_settings_value(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
) -> Result<AppSettings, String> {
    let base = production_instance_base();
    let disk = read_disk_object_for_write(path)?;
    let state = hidden_state_for_write(settings);

    let serialize_object = || -> Result<Map<String, Value>, String> {
        match serde_json::to_value(settings)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?
        {
            Value::Object(m) => Ok(m),
            _ => Err("settings did not serialize to a JSON object".to_string()),
        }
    };

    // Base object: Preserve starts from the incoming settings (non-project edits
    // apply); Reconcile is project-only and starts from the fresh disk object
    // (or the live settings when the file is absent).
    let mut out: Map<String, Value> = match mode {
        ProjectWriteMode::Preserve => serialize_object()?,
        ProjectWriteMode::Reconcile { .. } => match &disk {
            Some(m) => m.clone(),
            None => serialize_object()?,
        },
    };

    match mode {
        ProjectWriteMode::Preserve => match &disk {
            None => {
                write_active_group(&mut out, &state, base.as_deref());
                write_archived_group(&mut out, &state, base.as_deref());
            }
            Some(disk) => {
                if active_has_disk_truth(disk) {
                    copy_or_remove(&mut out, disk, FIELD_PROJECT_PATH);
                    copy_or_remove(&mut out, disk, FIELD_PROJECT_PATH_REL);
                    copy_or_remove(&mut out, disk, FIELD_PROJECT_PATHS);
                    copy_or_remove(&mut out, disk, FIELD_PROJECT_PATHS_REL);
                } else {
                    write_active_group(&mut out, &state, base.as_deref());
                }
                if archived_has_disk_truth(disk) {
                    copy_or_remove(&mut out, disk, FIELD_ARCHIVED);
                    copy_or_remove(&mut out, disk, FIELD_ARCHIVED_REL);
                } else {
                    write_archived_group(&mut out, &state, base.as_deref());
                }
            }
        },
        ProjectWriteMode::Reconcile { active, archived } => {
            let blocked = state.has_structural();
            // A synthesized (runtime-authoritative) state writes its runtime
            // groups verbatim, matching the pre-#1077 verbatim writer. A real
            // decoded state rebuilds only a dirty+eligible group and otherwise
            // copies disk raw to retain unresolved entries.
            let write_active_from_state = active
                && !blocked
                && (state.runtime_authoritative || state.active_reconcile_eligible);
            let write_archived_from_state = archived
                && !blocked
                && (state.runtime_authoritative || state.archived_reconcile_eligible);
            // Active group.
            if write_active_from_state {
                write_active_group(&mut out, &state, base.as_deref());
            } else if let Some(disk) = &disk {
                copy_or_remove(&mut out, disk, FIELD_PROJECT_PATH);
                copy_or_remove(&mut out, disk, FIELD_PROJECT_PATH_REL);
                copy_or_remove(&mut out, disk, FIELD_PROJECT_PATHS);
                copy_or_remove(&mut out, disk, FIELD_PROJECT_PATHS_REL);
            } else {
                write_active_group(&mut out, &state, base.as_deref());
            }
            // Archived group.
            if write_archived_from_state {
                write_archived_group(&mut out, &state, base.as_deref());
            } else if let Some(disk) = &disk {
                copy_or_remove(&mut out, disk, FIELD_ARCHIVED);
                copy_or_remove(&mut out, disk, FIELD_ARCHIVED_REL);
            } else {
                write_archived_group(&mut out, &state, base.as_deref());
            }
        }
    }

    // A synthesized legacy state (direct-constructed AppSettings) has no dirty
    // repair, so the eligible branches above never fire for it; its groups are
    // written verbatim from the live settings/disk, matching pre-#1077 behavior.
    let value = Value::Object(out);
    write_value_atomic(&value, path)?;

    let mut written_value = value;
    let fresh_state = apply_project_decode_to_value(
        &mut written_value,
        base.as_deref(),
        &projects::FsCandidateResolver,
    );
    let mut written: AppSettings = serde_json::from_value(written_value)
        .map_err(|e| format!("Failed to re-decode written settings: {e}"))?;
    written.project_path_state = Arc::new(fresh_state);
    Ok(written)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskProjectLists {
    pub project_paths: Vec<String>,
    pub project_path: Option<String>,
    pub archived_project_paths: Option<Vec<String>>,
}

/// #778/#881: side-effect-free reader of ONLY the project lists from
/// `settings.json` on disk. Under Design S those lists are disk-authoritative,
/// so the default `save_settings` uses this to preserve whatever another process
/// (a CLI verb, a second instance) wrote, and the list commands use it to
/// reconcile before a deliberate mutation.
///
/// The outer `Option` means no disk truth at all: the file is absent, or
/// `projectPaths` is absent/null. `archived_project_paths` has its own inner
/// `Option`: absent/null means no disk truth for that list only, while
/// `Some(vec![])` means disk explicitly says nothing is archived.
///
/// Error policy (grinch G2): a genuine `NotFound` yields `None` (fresh install:
/// no disk truth to substitute). ANY other error (a transient os 5/32/33/1175
/// lock, a permission failure, or unparseable JSON) returns `Err` so the caller
/// ABORTS the save. The read runs BEFORE the #774 temp write, so disk is
/// untouched and #774 is unaffected. For #778's threat model, aborting a
/// geometry/theme save (retried later) beats silently dropping a project. A few
/// tight retries absorb a transient lock without stacking latency onto
/// `rename_with_retry`. This deliberately does NOT swallow-to-empty like
/// `read_log_level_from_path`, whose transient-default is harmless.
///
/// G4a: `read_to_string` opens, reads, and CLOSES the handle before any write,
/// so the later `MoveFileEx` rename cannot hit a sharing violation (os 32).
/// MUST NOT call `load_settings` (it migrates, generates root_token, and
/// re-saves: infinite recursion + side effects).
pub(crate) fn read_pty_input_project_paths_strict() -> Result<Option<Vec<String>>, String> {
    let Some(path) = settings_path() else {
        return Err("settings_unavailable".to_string());
    };
    read_pty_input_project_paths_strict_from_path(&path)
}

fn read_pty_input_project_paths_strict_from_path(
    path: &Path,
) -> Result<Option<Vec<String>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("unsafe_path".to_string()),
    }
    let (bytes, _) = crate::path_identity::read_bounded_regular(path, 1024 * 1024)?;
    let value = crate::path_identity::parse_json_no_duplicates(&bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| "settings_invalid".to_string())?;
    let Some(project_paths) = object.get("projectPaths") else {
        return Ok(Some(Vec::new()));
    };
    if project_paths.is_null() {
        return Ok(Some(Vec::new()));
    }
    let project_paths = project_paths
        .as_array()
        .filter(|paths| paths.len() <= 1_024)
        .ok_or_else(|| "settings_invalid".to_string())?;
    let mut strict = Vec::with_capacity(project_paths.len());
    for path in project_paths {
        let path = path
            .as_str()
            .filter(|path| !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0'))
            .ok_or_else(|| "settings_invalid".to_string())?;
        strict.push(path.to_string());
    }
    Ok(Some(strict))
}

pub(crate) async fn read_pty_input_project_paths_strict_offloaded(
) -> Result<Option<Vec<String>>, String> {
    tokio::task::spawn_blocking(read_pty_input_project_paths_strict)
        .await
        .map_err(|_| "settings_unavailable".to_string())?
}

fn read_project_paths_from_disk(path: &Path) -> Result<Option<DiskProjectLists>, String> {
    // 1 initial attempt + up to 2 retries on a transient (non-NotFound) read error.
    const READ_RETRY_BACKOFFS_MS: [u64; 2] = [10, 40];
    let mut attempt = 0usize;
    let contents = loop {
        match std::fs::read_to_string(path) {
            Ok(c) => break c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Fresh install / never-saved: no disk truth to preserve.
                return Ok(None);
            }
            Err(e) => {
                if attempt < READ_RETRY_BACKOFFS_MS.len() {
                    std::thread::sleep(std::time::Duration::from_millis(
                        READ_RETRY_BACKOFFS_MS[attempt],
                    ));
                    attempt += 1;
                    continue;
                }
                return Err(format!(
                    "Failed to read {} to preserve project_paths (aborting save to avoid dropping a project): {}",
                    path.display(),
                    e
                ));
            }
        }
    };
    let value: serde_json::Value = serde_json::from_str(&contents).map_err(|e| {
        format!(
            "Failed to parse {} to preserve project_paths (aborting save to avoid dropping a project): {}",
            path.display(),
            e
        )
    })?;
    let project_paths = match value.get("projectPaths") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        // #881: non-string elements are dropped here, but rejected in the
        // archivedProjectPaths arm below. Deliberate. `projectPaths` only feeds
        // discovery; `archivedProjectPaths` feeds session retention, where a
        // silently-emptied list deletes sessions. Tightening this one is
        // #888's call, not #881's.
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect::<Vec<_>>(),
        Some(other) => {
            return Err(format!(
                "{}: projectPaths is {}, not an array (aborting save)",
                path.display(),
                other
            ));
        }
    };
    let project_path = value
        .get("projectPath")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let archived_project_paths = match value.get("archivedProjectPaths") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(arr)) => {
            // #881 R2-G5: a non-string element is corruption, not an entry to
            // skip. The `projectPaths` arm above silently drops non-strings;
            // do not copy that here. `[123]` would read as `Some(vec![])`,
            // meaning disk authoritatively says nothing is archived.
            let mut archived = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(archived_path) = item.as_str() else {
                    return Err(format!(
                        "{}: archivedProjectPaths contains {}, not a string (aborting save)",
                        path.display(),
                        item
                    ));
                };
                archived.push(archived_path.to_string());
            }
            Some(archived)
        }
        Some(other) => {
            return Err(format!(
                "{}: archivedProjectPaths is {}, not an array (aborting save)",
                path.display(),
                other
            ));
        }
    };
    Ok(Some(DiskProjectLists {
        project_paths,
        project_path,
        archived_project_paths,
    }))
}

/// #778/#881: overwrite `settings`' in-memory project lists with the
/// disk-authoritative ones. The deliberate list mutators
/// (`open_project`/`new_project`/`remove_project`)
/// call this under the `SettingsState` write lock BEFORE they add/remove, so a
/// concurrent CLI append is reconciled into the list rather than clobbered by the
/// following verbatim write. Aborts (propagates `Err`) on a non-`NotFound` read
/// error per G2; a missing home dir is a no-op (the following save surfaces it).
pub fn refresh_project_paths_from_disk(settings: &mut AppSettings) -> Result<(), String> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    refresh_project_paths_from_path(settings, &path)
}

pub(crate) fn refresh_project_paths_from_path(
    settings: &mut AppSettings,
    path: &Path,
) -> Result<(), String> {
    if let Some(lists) = read_project_paths_from_disk(path)? {
        settings.project_paths = lists.project_paths;
        settings.project_path = lists.project_path;
        if let Some(archived) = lists.archived_project_paths {
            settings.archived_project_paths = archived;
        }
    }
    Ok(())
}

/// #1077 §4.3 step 2: fresh-read and fully re-decode the six disk fields (so a
/// post-startup CLI write is authoritative), replacing the runtime lists with the
/// SELECTED canonical paths and installing the fresh hidden pair state. Aborts
/// (`Err`) on a present-but-invalid whole object; keeps the live runtime/hidden
/// state when the file is absent.
pub(crate) fn refresh_and_decode_project_paths_from_path(
    settings: &mut AppSettings,
    path: &Path,
) -> Result<(), String> {
    if let Some(map) = read_disk_object_for_write(path)? {
        let base = production_instance_base();
        let state = decode_project_state(&map, base.as_deref(), &projects::FsCandidateResolver);
        settings.project_paths = state.active_selected();
        settings.project_path = state.selected_head.clone();
        settings.archived_project_paths = state.archived_management_paths();
        settings.project_path_state = Arc::new(state);
    }
    Ok(())
}

/// Whether the current hidden project state carries structural corruption, which
/// must block any explicit project-list mutation (§4.2).
pub(crate) fn project_state_has_structural(settings: &AppSettings) -> bool {
    settings.project_path_state.has_structural()
}

/// Save settings to the app config directory (see config_dir()).
///
/// #778/#881: this is the DEFAULT writer and treats the project lists as
/// disk-authoritative (Design S). It preserves whatever `project_paths`/
/// `project_path`/`archived_project_paths` is currently on disk and discards this
/// (possibly stale)
/// in-memory candidate's copy, so ANY whole-object GUI save is fail-safe for the
/// project lists: a project a CLI verb registered while the GUI ran is never
/// clobbered. Deliberate list changes (the add/remove commands, the CLI verbs,
/// the startup root_token/migration write) go through
/// `save_settings_with_project_paths` instead.
///
/// The underlying write is the #774-hardened atomic tmp+rename
/// (`save_settings_to_path`): a per-writer unique temp plus
/// `sessions_persistence::rename_with_retry`. The disk read that preserves
/// project lists runs first and closes its handle before that write (G4a).
///
/// G4b (accepted, documented): a remove/add racing another writer within the
/// sub-millisecond window between this preserve-read and the rename could
/// momentarily resurrect an entry; orders of magnitude smaller than the prior
/// snapshot-age window. Airtight cross-process safety would need an advisory
/// file lock (tracked separately), deliberately not added here.
pub fn save_settings(settings: &AppSettings) -> Result<AppSettings, String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("settings.json");
    save_settings_to_path_preserving_project_paths(settings, &path)
}

/// #1077: atomic tmp+rename writer over an already-built JSON `Value`. Shared by
/// the raw and the project-aware writers. Preserves the #774 unique-temp +
/// `rename_with_retry` behavior; still not fsynced.
fn write_value_atomic(value: &Value, path: &Path) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("Settings path {} has no parent", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create settings directory: {}", e))?;

    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    let op_id = SAVE_OP_ID.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let tmp_path = dir.join(format!("settings.json.{}.{}.tmp", pid, op_id));

    if let Err(e) = std::fs::write(&tmp_path, &json) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Failed to write temp settings file: {}", e));
    }

    if let Err((err_msg, _diag)) =
        crate::config::sessions_persistence::rename_with_retry(&tmp_path, path)
    {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Failed to rename settings file: {}", err_msg));
    }

    log::debug!("Saved settings to {:?}", path);
    Ok(())
}

/// #778/#881: EXPLICIT writer. Persists `project_paths`/`project_path` and
/// `archived_project_paths` VERBATIM (the
/// pre-#778 `save_settings` behavior, still #774-hardened). ONLY for deliberate
/// list mutators that have already reconciled the list against disk: the add
/// commands (`open_project`/`new_project`), the `remove_project` command, the two
/// CLI verbs (which load fresh disk via `load_settings_for_cli`), and the startup
/// root_token/migration save (whose `project_paths` was just loaded from disk).
pub fn save_settings_with_project_paths(settings: &AppSettings) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("settings.json");
    save_settings_with_project_paths_to_path(settings, &path)
}

pub(crate) fn save_settings_with_project_paths_to_path(
    settings: &AppSettings,
    path: &Path,
) -> Result<(), String> {
    // #1077: deliberate list mutators reconcile both groups from the hidden
    // state (retaining unresolved/conflict entries value-for-value), so the
    // now-filtered runtime lists cannot drop an unresolved project.
    save_settings_value(
        settings,
        path,
        ProjectWriteMode::Reconcile {
            active: true,
            archived: true,
        },
    )
    .map(|_| ())
}

/// #778/#881 + #1077: the preserve-disk wrapper behind the default
/// `save_settings`. Preserves all six on-disk project fields (materializing a
/// group only when disk has no truth), then hands off to the #774-hardened
/// atomic writer and returns the fresh-decoded settings.
pub(crate) fn save_settings_to_path_preserving_project_paths(
    settings: &AppSettings,
    path: &Path,
) -> Result<AppSettings, String> {
    save_settings_value(settings, path, ProjectWriteMode::Preserve)
}

/// Raw writer: serialize `AppSettings` verbatim (three primary project fields;
/// the companion fields are `#[serde(skip)]`, yielding a legacy-shaped file).
/// Used only for test seeding; production writes go through the preserve/
/// reconcile modes so companions are materialized.
#[cfg(test)]
fn save_settings_to_path(settings: &AppSettings, path: &Path) -> Result<(), String> {
    let value = serde_json::to_value(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    write_value_atomic(&value, path)
}

pub type SettingsState = Arc<RwLock<AppSettings>>;

#[cfg(test)]
mod tests {

    #[test]
    fn privileged_project_paths_reader_rejects_duplicates_and_non_strings() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"projectPaths":["one"],"projectPaths":["two"]}"#).unwrap();
        assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());

        std::fs::write(&path, r#"{"projectPaths":["one",7]}"#).unwrap();
        assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    }

    #[test]
    fn privileged_project_paths_reader_rejects_a_dangling_link_when_supported() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        let missing = temp.path().join("missing.json");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&missing, &path).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&missing, &path).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if linked {
            assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
        }
    }

    #[test]
    fn privileged_project_paths_reader_is_bounded_and_exact() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"projectPaths":["one","two"]}"#).unwrap();
        assert_eq!(
            super::read_pty_input_project_paths_strict_from_path(&path).unwrap(),
            Some(vec!["one".to_string(), "two".to_string()])
        );

        std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    }

    #[test]
    fn validate_agent_commands_allows_plain_gemini() {
        let settings = settings_with_agents(&[("Gemini", "gemini")]);
        assert!(super::validate_agent_commands(&settings).is_ok());
    }

    #[test]
    fn validate_agent_commands_rejects_gemini_resume_latest() {
        let settings = settings_with_agents(&[("Gemini", "gemini --resume latest")]);
        let err = super::validate_agent_commands(&settings).unwrap_err();
        assert!(err.contains("Gemini commands must not include --resume"));
    }

    #[test]
    fn codex_home_template_accepts_each_token_alone_and_as_leading_segment() {
        for token in [
            "%AC_REPLICA_ROOT%",
            "%AC_WORKSPACE_ROOT%",
            "%AC_MATRIX_ROOT%",
        ] {
            super::validate_codex_home_template_value(token, "ctx")
                .unwrap_or_else(|e| panic!("{token} alone should be accepted: {e}"));
            super::validate_codex_home_template_value(&format!("{token}\\.codex"), "ctx")
                .unwrap_or_else(|e| panic!("{token}\\.codex should be accepted: {e}"));
            super::validate_codex_home_template_value(&format!("{token}/.codex"), "ctx")
                .unwrap_or_else(|e| panic!("{token}/.codex should be accepted: {e}"));
        }
    }

    #[test]
    fn codex_home_template_rejects_token_not_at_leading_segment() {
        let err = super::validate_codex_home_template_value(r"prefix%AC_MATRIX_ROOT%\x", "ctx")
            .unwrap_err();
        assert!(
            err.contains("complete path segment") && err.contains("%AC_MATRIX_ROOT%"),
            "{err}"
        );
    }

    #[test]
    fn codex_home_template_rejects_legacy_ac_root_as_unknown_marker() {
        // Save-time break: the old token is now an unknown placeholder, named in the error.
        let err = super::validate_codex_home_template_value(r"%AC_ROOT%\x", "ctx").unwrap_err();
        assert!(err.contains("%AC_ROOT%"), "{err}");
    }

    #[test]
    fn codex_home_template_keys_on_leading_token_not_list_order() {
        // F3: the leading segment is a valid token; another token appearing later must
        // NOT trigger a rejection that names the wrong token.
        super::validate_codex_home_template_value(r"%AC_MATRIX_ROOT%\sub", "ctx")
            .expect("matrix leading segment is valid");
        super::validate_codex_home_template_value(r"%AC_MATRIX_ROOT%\%AC_REPLICA_ROOT%\x", "ctx")
            .expect("leading token valid; remainder tokens are known at shape level");
    }

    use super::{
        merge_protected_coding_agent_settings, repair_coding_agent_profiles_config,
        validate_agent_commands, validate_api_server_settings, validate_resource_settings,
        AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, MainSidebarSide,
        ProfileCellConfig, ProfileSlotConfig, ResourceWatchdogAction,
        TelegramNetworkPollErrorLogging, TelegramPollFailureLogLevel, TelegramPollRecoveryLogLevel,
    };
    use std::collections::BTreeMap;

    fn settings_with_agents(commands: &[(&str, &str)]) -> AppSettings {
        AppSettings {
            agents: commands
                .iter()
                .enumerate()
                .map(|(idx, (label, command))| AgentConfig {
                    id: format!("agent-{idx}"),
                    label: (*label).to_string(),
                    command: (*command).to_string(),
                    color: "#000000".to_string(),
                    envs: Vec::new(),
                    isolated_home: false,
                    instructions_filename: None,
                    config_seed: None,
                    context_regex: None,
                    backend: Default::default(),
                })
                .collect(),
            ..AppSettings::default()
        }
    }

    #[test]
    fn validate_config_seed_dest_accepts_dotfile_names() {
        for ok in [".claude", ".claude-amp", ".codex", ".opencode", "config"] {
            assert!(
                super::validate_config_seed_dest(ok).is_ok(),
                "should accept {ok:?}"
            );
        }
        // Leading/trailing whitespace is trimmed before validation.
        assert!(super::validate_config_seed_dest("  .claude  ").is_ok());
    }

    #[test]
    fn validate_config_seed_dest_rejects_unsafe_names() {
        let bad = [
            "",                  // empty
            "   ",               // whitespace only
            ".config/opencode",  // forward separator (nested)
            ".config\\x",        // backslash separator
            "..",                // parent
            "a..b",              // contains ..
            "C:foo",             // drive-relative colon
            "x:y",               // ADS colon
            "%AC_REPLICA_ROOT%", // placeholder marker
            "$HOME",             // shell marker
            ".claude.",          // trailing dot (trim does NOT strip dots)
            "CON",               // reserved device
            "nul",               // reserved device (case-insensitive)
            "COM1",              // reserved device
            "lpt9.claude",       // reserved device before first dot
        ];
        for name in bad {
            assert!(
                super::validate_config_seed_dest(name).is_err(),
                "should reject {name:?}"
            );
        }
    }

    #[test]
    fn config_seed_is_active_requires_enabled_and_nonempty_dest() {
        use super::ConfigSeedConfig;
        assert!(ConfigSeedConfig {
            enabled: true,
            dest: ".claude".to_string()
        }
        .is_active());
        assert!(!ConfigSeedConfig {
            enabled: false,
            dest: ".claude".to_string()
        }
        .is_active());
        assert!(!ConfigSeedConfig {
            enabled: true,
            dest: "   ".to_string()
        }
        .is_active());
    }

    #[test]
    fn config_seed_serde_round_trips_camel_case_and_omits_when_absent() {
        use super::AgentConfig;
        // Present -> serializes as nested `configSeed { enabled, dest }`.
        let mut agent = AgentConfig {
            id: "claude".to_string(),
            label: "Claude".to_string(),
            command: "claude".to_string(),
            color: "#fff".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: Some(super::ConfigSeedConfig {
                enabled: true,
                dest: ".claude".to_string(),
            }),
            context_regex: None,
            backend: Default::default(),
        };
        let json = serde_json::to_string(&agent).unwrap();
        assert!(json.contains("\"configSeed\""), "{json}");
        assert!(json.contains("\"dest\":\".claude\""), "{json}");
        let back: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.config_seed.as_ref().map(|c| c.dest.as_str()),
            Some(".claude")
        );

        // Absent -> key omitted (skip_serializing_if), and old files deserialize to None.
        agent.config_seed = None;
        let json = serde_json::to_string(&agent).unwrap();
        assert!(!json.contains("configSeed"), "{json}");
        let back: AgentConfig =
            serde_json::from_str(r##"{"id":"x","label":"X","command":"claude","color":"#000"}"##)
                .unwrap();
        assert!(back.config_seed.is_none());
    }

    #[test]
    fn agent_backend_config_defaults_to_local_process_and_omits_default() {
        use super::AgentConfig;

        let agent: AgentConfig =
            serde_json::from_str(r##"{"id":"x","label":"X","command":"codex","color":"#000"}"##)
                .unwrap();

        assert_eq!(
            crate::pty::backend::SessionBackendKind::from(&agent.backend),
            crate::pty::backend::SessionBackendKind::LocalProcess
        );
        let json = serde_json::to_string(&agent).unwrap();
        assert!(!json.contains("backend"), "{json}");

        let mut local_with_image = agent;
        local_with_image.backend.image = Some("hand-edited:latest".to_string());
        let json = serde_json::to_string(&local_with_image).unwrap();
        assert!(!json.contains("backend"), "{json}");
    }

    #[test]
    fn agent_backend_config_serializes_container_image() {
        use super::AgentConfig;

        let mut settings = settings_with_agents(&[("Claude", "claude")]);
        settings.agents[0].backend.kind =
            crate::pty::backend::SessionBackendKind::ContainerTransport;
        settings.agents[0].backend.image = Some("agentscommander/ac-claude:latest".to_string());
        super::validate_and_repair_settings(&mut settings).unwrap();

        let json = serde_json::to_string(&settings.agents[0]).unwrap();
        assert!(json.contains("\"backend\""), "{json}");
        assert!(json.contains("\"kind\":\"containerTransport\""), "{json}");
        assert!(
            json.contains("\"image\":\"agentscommander/ac-claude:latest\""),
            "{json}"
        );
        let back: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.backend.image.as_deref(),
            Some("agentscommander/ac-claude:latest")
        );
    }

    #[test]
    fn validate_and_repair_normalizes_backend_image_edges() {
        let mut blank_container = settings_with_agents(&[("Claude", "claude")]);
        blank_container.agents[0].backend.kind =
            crate::pty::backend::SessionBackendKind::ContainerTransport;
        blank_container.agents[0].backend.image = Some("   ".to_string());
        super::validate_and_repair_settings(&mut blank_container).unwrap();
        assert!(blank_container.agents[0].backend.image.is_none());

        let mut local_with_image = settings_with_agents(&[("Codex", "codex")]);
        local_with_image.agents[0].backend.image = Some(" custom:latest ".to_string());
        super::validate_and_repair_settings(&mut local_with_image).unwrap();
        assert!(local_with_image.agents[0].backend.image.is_none());
        let json = serde_json::to_string(&local_with_image.agents[0]).unwrap();
        assert!(!json.contains("backend"), "{json}");
    }

    #[test]
    fn validate_and_repair_rejects_leading_dash_container_image() {
        let mut settings = settings_with_agents(&[("Claude", "claude")]);
        settings.agents[0].backend.kind =
            crate::pty::backend::SessionBackendKind::ContainerTransport;
        settings.agents[0].backend.image = Some(" --privileged".to_string());

        let err = super::validate_and_repair_settings(&mut settings).unwrap_err();
        assert!(err.contains("must not start with"), "{err}");
    }

    #[test]
    fn screenshot_hotkey_defaults_when_absent() {
        // #714 an old settings file without the key deserializes to "Ctrl+Q".
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("screenshotCaptureHotkey");
        let parsed: AppSettings = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.screenshot_capture_hotkey, "Ctrl+Q");
    }

    #[test]
    fn screenshot_hotkey_round_trips_camel_case() {
        let s = AppSettings {
            screenshot_capture_hotkey: "Control+P".to_string(),
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains("\"screenshotCaptureHotkey\":\"Control+P\""),
            "{json}"
        );
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.screenshot_capture_hotkey, "Control+P");
    }

    #[test]
    fn validate_and_repair_rejects_invalid_screenshot_hotkey() {
        let mut s = AppSettings {
            screenshot_capture_hotkey: "Ctrl+Shift+Q".to_string(),
            ..AppSettings::default()
        };
        assert!(super::validate_and_repair_settings(&mut s).is_err());

        s.screenshot_capture_hotkey = "Ctrl+Q".to_string();
        assert!(super::validate_and_repair_settings(&mut s).is_ok());
    }

    #[test]
    fn v1_profiles_migrate_to_v2_command_cells() {
        let json = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex --base",
                "color": "#000000"
            }],
            "codingAgentProfiles": {
                "schemaVersion": 1,
                "letters": { "A": { "name": "Baseline" } },
                "agentDefaults": { "dev-rust": "B" },
                "matrix": {
                    "codex": {
                        "B": {
                            "enabled": true,
                            "argv": ["--model", "gpt 5"],
                            "env": { "OPENAI_API_BASE": "https://example.test" },
                            "notes": "legacy"
                        }
                    }
                }
            }
        }"##;

        let (settings, migrated) = super::parse_settings_json(json, "test").unwrap();

        assert!(migrated);
        assert_eq!(settings.coding_agent_profiles.schema_version, 2);
        assert_eq!(
            settings.coding_agent_profiles.profile_slots["A"].label,
            "Baseline"
        );
        assert_eq!(
            settings
                .coding_agent_profiles
                .default_profile_by_agent
                .get("dev-rust")
                .map(String::as_str),
            Some("B")
        );
        let cell = &settings.coding_agent_profiles.profiles_by_agent["codex"]["B"];
        assert!(cell.enabled);
        assert_eq!(cell.command, "codex --base --model \"gpt 5\"");
        let out = serde_json::to_string(&settings).unwrap();
        assert!(out.contains("profileSlots"));
        assert!(out.contains("profilesByAgent"));
        assert!(out.contains("defaultProfileByAgent"));
        assert!(!out.contains("\"letters\""));
        assert!(!out.contains("\"matrix\""));
        assert!(!out.contains("\"argv\""));
    }

    #[test]
    fn load_settings_persists_v1_to_v2_migration_and_backup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let original = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "rootToken": "existing-token",
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000"
            }],
            "codingAgentProfiles": {
                "schemaVersion": 1,
                "letters": { "A": { "name": "Baseline" } },
                "matrix": {
                    "codex": {
                        "A": {
                            "enabled": true,
                            "argv": ["--model", "gpt'5"],
                            "env": {},
                            "notes": "legacy"
                        }
                    }
                }
            }
        }"##;
        std::fs::write(&path, original).unwrap();

        let settings = super::load_settings_from_path(&path);

        assert_eq!(settings.root_token.as_deref(), Some("existing-token"));
        assert_eq!(settings.coding_agent_profiles.schema_version, 2);
        let cell = &settings.coding_agent_profiles.profiles_by_agent["codex"]["A"];
        assert_eq!(cell.command, "codex --model \"gpt'5\"");

        let backup_path = temp.path().join("settings.pre-384-v1.json");
        assert_eq!(std::fs::read_to_string(backup_path).unwrap(), original);

        let saved_raw = std::fs::read_to_string(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
        assert_eq!(saved["codingAgentProfiles"]["schemaVersion"], 2);
        assert!(saved["codingAgentProfiles"].get("profileSlots").is_some());
        assert!(saved["codingAgentProfiles"]
            .get("profilesByAgent")
            .is_some());
        assert!(saved["codingAgentProfiles"].get("matrix").is_none());
        assert!(saved["codingAgentProfiles"].get("letters").is_none());
    }

    #[test]
    fn v1_profile_migration_prefers_argv_over_args_and_preserves_disabled_on_parse_error() {
        let json = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "\"unterminated",
                "color": "#000000"
            }],
            "codingAgentProfiles": {
                "schemaVersion": 1,
                "matrix": {
                    "codex": {
                        "A": {
                            "argv": ["--from-argv"],
                            "args": ["--from-args"],
                            "env": {},
                            "notes": "repair me"
                        }
                    }
                }
            }
        }"##;

        let (settings, migrated) = super::parse_settings_json(json, "test").unwrap();
        let cell = &settings.coding_agent_profiles.profiles_by_agent["codex"]["A"];

        assert!(migrated);
        assert!(!cell.enabled);
        assert_eq!(cell.command, "--from-argv");
        assert_eq!(cell.notes, "repair me");
    }

    #[test]
    fn legacy_env_source_and_isolated_home_alias_serialize_as_v2_names() {
        let json = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000",
                "isolateCodexHome": true,
                "envs": [{
                    "key": "OPENAI_API_BASE",
                    "value": "https://example.test",
                    "source": "agentsCommander",
                    "enabled": true
                }]
            }]
        }"##;

        let (settings, _migrated) = super::parse_settings_json(json, "test").unwrap();
        let out = serde_json::to_string(&settings).unwrap();

        assert!(settings.agents[0].isolated_home);
        assert_eq!(
            settings.agents[0].envs[0].source,
            CodingAgentEnvSource::System
        );
        assert!(out.contains("\"isolatedHome\":true"));
        assert!(out.contains("\"source\":\"system\""));
        assert!(!out.contains("isolateCodexHome"));
        assert!(!out.contains("agentsCommander"));
    }

    #[test]
    fn validate_agent_commands_allows_plain_claude() {
        let settings = settings_with_agents(&[("Claude", "claude")]);
        assert!(validate_agent_commands(&settings).is_ok());
    }

    #[test]
    fn repair_profiles_adds_a_letter_and_a_cells_for_agents() {
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        settings.coding_agent_profiles.profile_slots.clear();
        settings.coding_agent_profiles.profile_slots.insert(
            "AA".to_string(),
            ProfileSlotConfig {
                label: "bad".into(),
            },
        );

        let changed = repair_coding_agent_profiles_config(
            &mut settings.coding_agent_profiles,
            &settings.agents,
        );

        assert!(changed);
        assert!(settings
            .coding_agent_profiles
            .profile_slots
            .contains_key("A"));
        assert!(!settings
            .coding_agent_profiles
            .profile_slots
            .contains_key("AA"));
        assert!(settings.coding_agent_profiles.profiles_by_agent["agent-0"].contains_key("A"));
    }

    #[test]
    fn stale_full_settings_update_preserves_profiles_envs_and_isolation() {
        let mut current = settings_with_agents(&[("Codex", "codex")]);
        current.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        current.agents[0].isolated_home = true;
        current
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --current".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        let mut stale = current.clone();
        stale.agents[0].envs.clear();
        stale.agents[0].isolated_home = false;
        stale.coding_agent_profiles.profiles_by_agent.clear();
        stale.sidebar_style = "command-center".to_string();

        let merged = merge_protected_coding_agent_settings(&current, stale);

        assert_eq!(merged.sidebar_style, "command-center");
        assert_eq!(merged.agents[0].envs[0].value, "current");
        assert!(merged.agents[0].isolated_home);
        assert!(merged.coding_agent_profiles.profiles_by_agent["agent-0"].contains_key("B"));
    }

    #[test]
    fn validate_agent_commands_rejects_relative_codex_home_env() {
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        settings.agents[0].envs = vec![CodingAgentEnv {
            key: "CODEX_HOME".to_string(),
            value: "relative/path".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];

        let err = validate_agent_commands(&settings).unwrap_err();

        assert!(err.contains("CODEX_HOME must be an absolute path"), "{err}");
    }

    #[test]
    fn validate_agent_commands_rejects_claude_continue() {
        let settings = settings_with_agents(&[("Claude", "claude --continue")]);
        let err = validate_agent_commands(&settings).unwrap_err();
        assert!(err.contains("Claude commands must not include --continue or -c"));
    }

    #[test]
    fn validate_rejects_banned_continue_flag_in_cell_params() {
        // #597 - the cell holds params only; the provider token (claude) lives in
        // the base. The ban must still catch `--continue` typed in the cell, which
        // only works if validation runs on the COMPOSED command.
        let mut settings = settings_with_agents(&[("Claude", "claude")]);
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "--continue".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
        let err = validate_agent_commands(&settings).unwrap_err();
        assert!(
            err.contains("must not include --continue"),
            "composed claude + --continue cell must be rejected: {err}"
        );
    }

    #[test]
    fn validate_allows_continue_flag_when_base_is_not_claude() {
        // #597 - the --continue ban is claude-specific. With a codex base the
        // composed `codex --continue` is allowed, proving validation keys off the
        // composed provider token, not the raw cell text.
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "--continue".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
        assert!(validate_agent_commands(&settings).is_ok());
    }

    #[test]
    fn validate_agent_commands_allows_all_explicit_pi_session_controls_with_provider_overlap() {
        let selectors = [
            "-c",
            "-r",
            "--continue",
            "--continue=true",
            "--resume",
            "--resume=id",
            "--session",
            "--session=id",
            "--session-id",
            "--session-id=id",
            "--fork",
            "--fork=id",
            "--no-session",
            "--no-session=true",
        ];
        for provider in ["claude", "codex", "gemini"] {
            for selector in selectors {
                let command = format!("pi --provider {provider} {selector}");
                let settings = settings_with_agents(&[("Pi", command.as_str())]);
                assert!(
                    validate_agent_commands(&settings).is_ok(),
                    "provider={provider:?} selector={selector:?}"
                );
            }
        }
    }

    #[test]
    fn validate_agent_commands_allows_explicit_pi_controls_in_composed_profile_cells() {
        let selectors = [
            "-c",
            "-r",
            "--continue",
            "--resume",
            "--session",
            "--session-id",
            "--fork",
            "--no-session",
        ];
        for provider in ["claude", "codex", "gemini"] {
            for selector in selectors {
                let base = format!("pi --provider {provider}");
                let mut settings = settings_with_agents(&[("Pi", base.as_str())]);
                settings
                    .coding_agent_profiles
                    .profiles_by_agent
                    .entry("agent-0".to_string())
                    .or_default()
                    .insert(
                        "A".to_string(),
                        ProfileCellConfig {
                            enabled: true,
                            command: format!("--model {provider} {selector}"),
                            env: BTreeMap::new(),
                            notes: String::new(),
                        },
                    );
                assert!(
                    validate_agent_commands(&settings).is_ok(),
                    "provider={provider:?} selector={selector:?}"
                );
            }
        }
    }

    #[test]
    fn validate_agent_commands_allows_plain_codex() {
        let settings = settings_with_agents(&[("Codex", "codex")]);
        assert!(validate_agent_commands(&settings).is_ok());
    }

    #[test]
    fn validate_agent_commands_allows_codex_search() {
        let settings = settings_with_agents(&[("Codex", "codex --search")]);
        assert!(validate_agent_commands(&settings).is_ok());
    }

    #[test]
    fn validate_agent_commands_allows_explicit_codex_help() {
        let settings = settings_with_agents(&[("Codex", "codex help")]);
        assert!(validate_agent_commands(&settings).is_ok());
    }

    #[test]
    fn validate_agent_commands_rejects_codex_resume_last() {
        let settings = settings_with_agents(&[("Codex", "codex resume --last")]);
        let err = validate_agent_commands(&settings).unwrap_err();
        assert!(err.contains("Codex commands must not include resume or --last"));
    }

    #[test]
    fn validate_agent_commands_rejects_cmd_wrapper_codex_resume_last() {
        let settings = settings_with_agents(&[("Codex", "cmd /C codex resume --last")]);
        let err = validate_agent_commands(&settings).unwrap_err();
        assert!(err.contains("Codex commands must not include resume or --last"));
    }

    #[test]
    fn validate_agent_commands_allows_codex_config_value_with_resume_text() {
        let settings =
            settings_with_agents(&[("Codex", "codex -c instruction=\"resume later\" --search")]);
        assert!(validate_agent_commands(&settings).is_ok());
    }

    // #529 - instructions filename validation surfaces at settings save (G8/Q6).

    #[test]
    fn validate_agent_commands_rejects_unsafe_instructions_filename() {
        for bad in [
            "../x.md",
            "a/b.md",
            "a\\b.md",
            "C:x.md",
            "CON.md",
            "last_ac_context.md",
        ] {
            let mut settings = settings_with_agents(&[("Codex", "codex")]);
            settings.agents[0].instructions_filename = Some(bad.to_string());
            let err = validate_agent_commands(&settings).unwrap_err();
            assert!(
                err.contains("instructions filename is invalid"),
                "{bad:?} should be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn validate_agent_commands_allows_safe_or_empty_instructions_filename() {
        // Valid names plus empty/whitespace (which means "use the default").
        for ok in ["AGENTS.md", "CLAUDE.md", "MyTeam.md", "  ", ""] {
            let mut settings = settings_with_agents(&[("Codex", "codex")]);
            settings.agents[0].instructions_filename = Some(ok.to_string());
            assert!(
                validate_agent_commands(&settings).is_ok(),
                "{ok:?} should be allowed"
            );
        }
        // Absent (None) is allowed too.
        let settings = settings_with_agents(&[("Codex", "codex")]);
        assert!(validate_agent_commands(&settings).is_ok());
    }

    #[test]
    fn instructions_filename_serde_round_trips_and_omits_when_absent() {
        // Absent -> omitted (skip_serializing_if) and deserializes back to None.
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("instructionsFilename"));
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agents[0].instructions_filename, None);

        // Present -> serialized under the camelCase key and round-trips.
        settings.agents[0].instructions_filename = Some("Squad.md".to_string());
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"instructionsFilename\":\"Squad.md\""));
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.agents[0].instructions_filename.as_deref(),
            Some("Squad.md")
        );
    }

    #[test]
    fn team_idle_beep_enabled_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(s.team_idle_beep_enabled);
        s.team_idle_beep_enabled = false;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"teamIdleBeepEnabled\":false"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(!back.team_idle_beep_enabled);
    }

    #[test]
    fn team_idle_beep_enabled_defaults_true_when_missing_from_json() {
        // Old settings.json without the new field must deserialize to true (default_true).
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(s.team_idle_beep_enabled);
    }

    #[test]
    fn telegram_network_poll_error_logging_round_trips_through_serde() {
        let s = AppSettings {
            telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging {
                first_failure_level: TelegramPollFailureLogLevel::Debug,
                transient_repeat_level: TelegramPollFailureLogLevel::Warn,
                sustained_level: TelegramPollFailureLogLevel::Error,
                sustained_after_seconds: 12,
                sustained_repeat_seconds: 34,
                recovery_level: TelegramPollRecoveryLogLevel::Warn,
            },
            ..AppSettings::default()
        };

        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"telegramNetworkPollErrorLogging\""));
        assert!(json.contains("\"sustainedAfterSeconds\":12"));

        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            back.telegram_network_poll_error_logging
                .sustained_after_seconds,
            12
        );
        assert_eq!(
            back.telegram_network_poll_error_logging.recovery_level,
            TelegramPollRecoveryLogLevel::Warn
        );
    }

    #[test]
    fn telegram_network_poll_error_logging_defaults_when_missing_from_json() {
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert_eq!(
            s.telegram_network_poll_error_logging,
            TelegramNetworkPollErrorLogging::default()
        );
    }

    #[test]
    fn telegram_network_poll_error_logging_field_defaults_are_backfilled() {
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "telegramNetworkPollErrorLogging": {
                "sustainedAfterSeconds": 5
            }
        }"#;

        let s: AppSettings = serde_json::from_str(json).expect("deserialize partial policy");
        assert_eq!(
            s.telegram_network_poll_error_logging
                .sustained_after_seconds,
            5
        );
        assert_eq!(
            s.telegram_network_poll_error_logging.first_failure_level,
            TelegramPollFailureLogLevel::Warn
        );
        assert_eq!(
            s.telegram_network_poll_error_logging.transient_repeat_level,
            TelegramPollFailureLogLevel::Debug
        );
        assert_eq!(
            s.telegram_network_poll_error_logging.recovery_level,
            TelegramPollRecoveryLogLevel::Info
        );
    }

    #[test]
    fn sounds_enabled_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(s.sounds_enabled);
        s.sounds_enabled = false;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"soundsEnabled\":false"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(!back.sounds_enabled);
    }

    #[test]
    fn sounds_enabled_defaults_true_when_missing_from_json() {
        // Old settings.json with only team_idle_beep_enabled (and no soundsEnabled
        // field) must deserialize with sounds_enabled = true so existing users
        // remain audible until they explicitly mute.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "teamIdleBeepEnabled": false,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(s.sounds_enabled);
        // Existing per-feature toggle is honored as-is.
        assert!(!s.team_idle_beep_enabled);
    }

    #[test]
    fn rail_collapse_fields_round_trip_through_serde() {
        let mut s = AppSettings::default();
        assert!(s.rail_collapsed_projects.is_empty());
        assert!(!s.rail_favorites_collapsed);

        s.rail_collapsed_projects = vec!["c:/foo/bar".to_string(), "d:/baz".to_string()];
        s.rail_favorites_collapsed = true;

        let json = serde_json::to_string(&s).expect("serialize");
        // (#965) camelCase guard. A broken `rename_all` would otherwise surface only as a
        // silently dead frontend, since the TS side reads `railCollapsedProjects` /
        // `railFavoritesCollapsed`.
        assert!(
            json.contains("\"railCollapsedProjects\":[\"c:/foo/bar\",\"d:/baz\"]"),
            "{json}"
        );
        assert!(json.contains("\"railFavoritesCollapsed\":true"), "{json}");

        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.rail_collapsed_projects, s.rail_collapsed_projects);
        assert!(back.rail_favorites_collapsed);
    }

    #[test]
    fn rail_collapse_fields_default_when_missing_from_json() {
        // A legacy settings.json carries neither key. Build one by round-tripping a
        // default and deleting them, so every other field stays present and the test
        // isolates exactly the `#[serde(default)]` behavior.
        let mut value = serde_json::to_value(AppSettings::default()).expect("to_value");
        let obj = value.as_object_mut().expect("settings object");
        obj.remove("railCollapsedProjects");
        obj.remove("railFavoritesCollapsed");

        let s: AppSettings = serde_json::from_value(value).expect("deserialize legacy");

        assert!(s.rail_collapsed_projects.is_empty());
        assert!(!s.rail_favorites_collapsed);
    }

    #[test]
    fn theme_light_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.theme_light);
        s.theme_light = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"themeLight\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.theme_light);
    }

    #[test]
    fn theme_light_defaults_false_when_missing_from_json() {
        // Settings without themeLight now open dark by default.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(!s.theme_light);
    }

    #[test]
    fn theme_light_explicit_false_survives_round_trip() {
        // Once a user explicitly disables light mode, the value must survive
        // serialize/deserialize without being altered by the serde default.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "themeLight": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize");
        assert!(!s.theme_light);
        let out = serde_json::to_string(&s).expect("serialize");
        let back: AppSettings = serde_json::from_str(&out).expect("re-deserialize");
        assert!(!back.theme_light);
    }

    #[test]
    fn spec_board_enabled_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.spec_board_enabled);
        let default_json = serde_json::to_string(&s).expect("serialize default");
        assert!(default_json.contains("\"specBoardEnabled\":false"));

        s.spec_board_enabled = true;

        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"specBoardEnabled\":true"));

        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.spec_board_enabled);
    }

    #[test]
    fn resource_monitor_settings_round_trip_through_serde() {
        let mut s = AppSettings::default();
        assert!(s.resource_monitor_enabled);
        assert_eq!(s.max_concurrent_agent_processes, 32);
        assert_eq!(s.resource_watchdog_action, ResourceWatchdogAction::Warn);
        assert_eq!(s.agent_group_warn_private_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(s.agent_group_kill_private_bytes, 12 * 1024 * 1024 * 1024);
        assert_eq!(s.agent_process_kill_private_bytes, 12 * 1024 * 1024 * 1024);
        assert!(s.resource_keep_last_snapshot);
        assert!(s.resource_backoff_polling);

        s.resource_monitor_enabled = false;
        s.max_concurrent_agent_processes = 5;
        s.resource_watchdog_action = ResourceWatchdogAction::KillGroup;
        s.agent_group_warn_private_bytes = 128;
        s.agent_group_kill_private_bytes = 256;
        s.agent_process_kill_private_bytes = 512;
        s.resource_keep_last_snapshot = false;
        s.resource_backoff_polling = false;

        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"resourceMonitorEnabled\":false"));
        assert!(json.contains("\"resourceWatchdogAction\":\"killGroup\""));

        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(!back.resource_monitor_enabled);
        assert_eq!(back.max_concurrent_agent_processes, 5);
        assert_eq!(
            back.resource_watchdog_action,
            ResourceWatchdogAction::KillGroup
        );
        assert_eq!(back.agent_group_warn_private_bytes, 128);
        assert_eq!(back.agent_group_kill_private_bytes, 256);
        assert_eq!(back.agent_process_kill_private_bytes, 512);
        assert!(!back.resource_keep_last_snapshot);
        assert!(!back.resource_backoff_polling);
    }

    #[test]
    fn coordinator_clock_settings_default_when_keys_absent() {
        // #552: an old settings.json (no coordinator-* keys) must deserialize
        // cleanly to the documented defaults, no migration.
        // Serialize a default, strip ONLY these coordinator keys, deserialize back.
        let mut value =
            serde_json::to_value(AppSettings::default()).expect("serialize default to value");
        let obj = value
            .as_object_mut()
            .expect("settings serializes to an object");
        obj.remove("coordinatorIdleBadgeYellowMinutes");
        obj.remove("coordinatorIdleBadgeRedMinutes");
        obj.remove("coordinatorAutoCloseEnabled");
        obj.remove("coordinatorAutoCloseMinutes");
        obj.remove("coordinatorAutoCloseSkipTelegramAssigned");

        let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
        assert!(back.coordinator_auto_close_enabled);
        assert_eq!(back.coordinator_auto_close_minutes, 60);
        assert!(!back.coordinator_auto_close_skip_telegram_assigned);
        assert_eq!(back.coordinator_idle_badge_yellow_minutes, 30);
        assert_eq!(back.coordinator_idle_badge_red_minutes, 60);
    }

    #[test]
    fn coordinator_auto_close_skip_telegram_assigned_round_trips() {
        let s = AppSettings {
            coordinator_auto_close_skip_telegram_assigned: true,
            ..AppSettings::default()
        };

        let json = serde_json::to_value(&s).expect("serialize settings");
        assert_eq!(
            json.get("coordinatorAutoCloseSkipTelegramAssigned"),
            Some(&serde_json::Value::Bool(true))
        );

        let back: AppSettings = serde_json::from_value(json).expect("deserialize settings");
        assert!(back.coordinator_auto_close_skip_telegram_assigned);
    }

    #[test]
    fn resolve_auto_self_clear_precedence_table() {
        // #640 §5 resolution table. Master-first kill switch, then per-agent
        // override, then the class-aware default computed at the call site.
        let mut s = AppSettings::default();

        // master ON, no override, class default ON (coordinator/Root) -> ON.
        assert!(super::resolve_auto_self_clear(&s, "tech-lead", true));
        // master ON, no override, class default OFF (specialist) -> OFF.
        assert!(!super::resolve_auto_self_clear(&s, "dev-rust", false));

        // explicit per-agent opt-in overrides a specialist's OFF class default.
        s.auto_self_clear_by_agent
            .insert("dev-rust".to_string(), true);
        assert!(super::resolve_auto_self_clear(&s, "dev-rust", false));
        // explicit per-agent opt-out overrides a coordinator's ON class default.
        s.auto_self_clear_by_agent
            .insert("tech-lead".to_string(), false);
        assert!(!super::resolve_auto_self_clear(&s, "tech-lead", true));

        // master OFF is an absolute kill switch: off for everyone, and it wins
        // even over a per-agent opt-in.
        s.auto_self_clear_enabled = false;
        assert!(!super::resolve_auto_self_clear(&s, "tech-lead", true));
        assert!(!super::resolve_auto_self_clear(&s, "dev-rust", true));
    }

    #[test]
    fn resolve_auto_self_clear_empty_name_uses_class_default() {
        // #640 L1: the Root derives to an empty name (no `_agent_`/`__agent_`
        // prefix); with no "" key it falls through to the class default. Same
        // path covers any name-derivation failure.
        let s = AppSettings::default();
        assert!(super::resolve_auto_self_clear(&s, "", true));
        assert!(!super::resolve_auto_self_clear(&s, "", false));
    }

    #[test]
    fn auto_self_clear_defaults_when_keys_absent() {
        // #640: an old settings.json (no auto-self-clear keys) must deserialize
        // to the documented defaults: master ON + empty per-agent map.
        let mut value =
            serde_json::to_value(AppSettings::default()).expect("serialize default to value");
        let obj = value
            .as_object_mut()
            .expect("settings serializes to an object");
        obj.remove("autoSelfClearEnabled");
        obj.remove("autoSelfClearByAgent");

        let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
        assert!(back.auto_self_clear_enabled);
        assert!(back.auto_self_clear_by_agent.is_empty());
    }

    #[test]
    fn auto_self_clear_round_trips_through_serde() {
        let mut s = AppSettings {
            auto_self_clear_enabled: false,
            ..AppSettings::default()
        };
        s.auto_self_clear_by_agent
            .insert("dev-rust".to_string(), true);
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"autoSelfClearEnabled\":false"));
        assert!(json.contains("\"autoSelfClearByAgent\":{\"dev-rust\":true}"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(!back.auto_self_clear_enabled);
        assert_eq!(back.auto_self_clear_by_agent.get("dev-rust"), Some(&true));
    }

    #[test]
    fn validate_resource_settings_rejects_invalid_limits() {
        let s = AppSettings {
            max_concurrent_agent_processes: 0,
            ..AppSettings::default()
        };
        assert!(validate_resource_settings(&s)
            .unwrap_err()
            .contains("maxConcurrentAgentProcesses"));

        let s = AppSettings {
            agent_group_warn_private_bytes: 10,
            agent_group_kill_private_bytes: 9,
            ..AppSettings::default()
        };
        assert!(validate_resource_settings(&s)
            .unwrap_err()
            .contains("agentGroupWarnPrivateBytes"));

        let s = AppSettings {
            agent_group_warn_private_bytes: 1,
            agent_group_kill_private_bytes: 0,
            ..AppSettings::default()
        };
        assert!(validate_resource_settings(&s)
            .unwrap_err()
            .contains("agentGroupKillPrivateBytes"));

        let s = AppSettings {
            agent_group_warn_private_bytes: 1,
            agent_group_kill_private_bytes: 10,
            agent_process_kill_private_bytes: 0,
            ..AppSettings::default()
        };
        assert!(validate_resource_settings(&s)
            .unwrap_err()
            .contains("agentProcessKillPrivateBytes"));
    }

    #[test]
    fn validate_resource_settings_accepts_max_concurrent_above_legacy_cap() {
        // #565: the old 1..=16 upper ceiling is gone. Any value >= 1 is accepted;
        // the user is responsible for sizing it to their machine.
        for value in [17u32, 32, 64, 256, 10_000] {
            let s = AppSettings {
                max_concurrent_agent_processes: value,
                ..AppSettings::default()
            };
            assert!(
                validate_resource_settings(&s).is_ok(),
                "expected max_concurrent_agent_processes={value} to validate"
            );
        }
    }

    #[test]
    fn validate_api_server_settings_rejects_invalid_bind_and_port() {
        let mut empty_bind = AppSettings {
            api_server_bind: "   ".to_string(),
            ..AppSettings::default()
        };
        assert!(validate_api_server_settings(&mut empty_bind)
            .unwrap_err()
            .contains("apiServerBind"));

        let mut zero_port = AppSettings {
            api_server_port: 0,
            ..AppSettings::default()
        };
        assert!(validate_api_server_settings(&mut zero_port)
            .unwrap_err()
            .contains("apiServerPort"));

        let mut hostname = AppSettings {
            api_server_bind: "localhost".to_string(),
            ..AppSettings::default()
        };
        assert!(validate_api_server_settings(&mut hostname)
            .unwrap_err()
            .contains("apiServerBind"));
    }

    #[test]
    fn validate_api_server_settings_accepts_ip_binds_and_trims() {
        for bind in ["127.0.0.1", "0.0.0.0", "::1", "::"] {
            let mut settings = AppSettings {
                api_server_bind: format!(" {bind} "),
                ..AppSettings::default()
            };
            validate_api_server_settings(&mut settings).unwrap();
            assert_eq!(settings.api_server_bind, bind);
        }
    }

    #[test]
    fn coord_sort_by_activity_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.coord_sort_by_activity);
        s.coord_sort_by_activity = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"coordSortByActivity\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.coord_sort_by_activity);
    }

    #[test]
    fn log_level_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(s.log_level.is_none());
        s.log_level = Some("info,agentscommander_lib::config::teams=debug".to_string());
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"logLevel\":\"info,agentscommander_lib::config::teams=debug\""));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            back.log_level,
            Some("info,agentscommander_lib::config::teams=debug".to_string())
        );
    }

    #[test]
    fn log_level_defaults_to_none_when_missing_from_json() {
        // Old settings.json without the new field must deserialize to None.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(s.log_level.is_none());
    }

    #[test]
    fn read_log_level_only_returns_value_when_present() {
        let dir = std::env::temp_dir().join(format!("rlol-present-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"logLevel":"info,agentscommander_lib::config::teams=debug","other":"x"}"#,
        )
        .unwrap();
        assert_eq!(
            super::read_log_level_from_path(&path),
            Some("info,agentscommander_lib::config::teams=debug".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_log_level_only_returns_none_when_log_level_missing() {
        let dir = std::env::temp_dir().join(format!("rlol-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"other":"value"}"#).unwrap();
        assert_eq!(super::read_log_level_from_path(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_log_level_only_returns_none_when_settings_missing() {
        let path =
            std::env::temp_dir().join(format!("rlol-no-such-file-{}.json", std::process::id()));
        // Intentionally do not create the file.
        assert_eq!(super::read_log_level_from_path(&path), None);
    }

    #[test]
    fn read_log_level_only_returns_none_when_json_malformed() {
        let dir = std::env::temp_dir().join(format!("rlol-malformed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ invalid json no closing brace").unwrap();
        assert_eq!(super::read_log_level_from_path(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_log_level_only_returns_some_empty_string_when_log_level_is_empty() {
        // Asserts read_log_level_only returns Some("") (not None) when logLevel is the
        // empty string — the helper preserves the user's intent (the field is set, just
        // empty). Downstream filter machinery handles the rest, with semantics distinct
        // from the malformed-string case: empty-string → parse_filters("") produces 0
        // directives → env_filter's hidden {None, Error} default applies → Error-only logs
        // flow globally; malformed-string → 1 non-matching directive → all
        // agentscommander* logs suppressed. The helper is symmetric on both inputs
        // (returns Some(value)); the observable difference is at env_filter::Builder::build.
        let dir = std::env::temp_dir().join(format!("rlol-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"logLevel":"","other":"value"}"#).unwrap();
        assert_eq!(super::read_log_level_from_path(&path), Some(String::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── #778: disk-authoritative project_paths (Design S) ────────────────────

    fn settings_with_project_paths(paths: &[&str]) -> AppSettings {
        AppSettings {
            project_paths: paths.iter().map(|p| p.to_string()).collect(),
            project_path: paths.first().map(|p| p.to_string()),
            ..AppSettings::default()
        }
    }

    fn settings_with_project_and_archived_paths(paths: &[&str], archived: &[&str]) -> AppSettings {
        AppSettings {
            project_paths: paths.iter().map(|p| p.to_string()).collect(),
            project_path: paths.first().map(|p| p.to_string()),
            archived_project_paths: archived.iter().map(|p| p.to_string()).collect(),
            ..AppSettings::default()
        }
    }

    /// #1077: create a real AC project dir (`<parent>/<name>/.ac`) and return its
    /// display-canonical path, so the six-field decoder validates it and the
    /// SELECTED runtime path equals the stored string. Used by preserve/reconcile
    /// tests that assert the returned settings mirror the preserved disk list.
    fn real_ac_project_path(parent: &std::path::Path, name: &str) -> String {
        let dir = parent.join(name);
        std::fs::create_dir_all(dir.join(".ac")).unwrap();
        let canon = std::fs::canonicalize(&dir).unwrap();
        crate::config::projects::display_canonical(&canon.to_string_lossy())
    }

    #[test]
    fn read_project_paths_from_disk_returns_list_and_head() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        super::save_settings_to_path(&settings_with_project_paths(&["C:/a", "C:/b"]), &path)
            .unwrap();
        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        assert_eq!(
            lists.project_paths,
            vec!["C:/a".to_string(), "C:/b".to_string()]
        );
        assert_eq!(lists.project_path.as_deref(), Some("C:/a"));
    }

    #[test]
    fn read_project_paths_from_disk_returns_none_when_file_absent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("does-not-exist.json");
        // NotFound means there is no disk truth to substitute.
        assert_eq!(super::read_project_paths_from_disk(&path).unwrap(), None);
    }

    #[test]
    fn read_project_paths_from_disk_returns_none_when_key_absent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"themeLight":true}"#).unwrap();
        assert_eq!(super::read_project_paths_from_disk(&path).unwrap(), None);
    }

    #[test]
    fn read_project_paths_from_disk_returns_none_when_key_null() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"projectPaths":null}"#).unwrap();
        assert_eq!(super::read_project_paths_from_disk(&path).unwrap(), None);
    }

    #[test]
    fn read_project_paths_from_disk_aborts_when_key_wrong_type() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"projectPaths":"C:/a"}"#).unwrap();
        assert!(super::read_project_paths_from_disk(&path).is_err());
    }

    #[test]
    fn read_project_paths_from_disk_accepts_legit_empty_list() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"projectPaths":[]}"#).unwrap();
        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        assert!(lists.project_paths.is_empty());
        assert_eq!(lists.project_path, None);
    }

    #[test]
    fn read_project_paths_from_disk_returns_archived_list() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        super::save_settings_to_path(
            &settings_with_project_and_archived_paths(&["C:/a"], &["C:/old"]),
            &path,
        )
        .unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

        assert_eq!(
            lists.archived_project_paths,
            Some(vec!["C:/old".to_string()])
        );
    }

    #[test]
    fn read_project_paths_from_disk_returns_archived_none_when_key_absent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"projectPaths":["C:/a"],"projectPath":"C:/a"}"#).unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

        assert_eq!(lists.project_paths, vec!["C:/a".to_string()]);
        assert_eq!(lists.project_path.as_deref(), Some("C:/a"));
        assert_eq!(lists.archived_project_paths, None);
    }

    #[test]
    fn read_project_paths_from_disk_returns_archived_none_when_key_null() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"projectPaths":["C:/a"],"archivedProjectPaths":null}"#,
        )
        .unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

        assert_eq!(lists.archived_project_paths, None);
    }

    #[test]
    fn read_project_paths_from_disk_returns_archived_some_empty_when_key_is_empty_list() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"projectPaths":["C:/a"],"archivedProjectPaths":[]}"#,
        )
        .unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

        assert_eq!(lists.archived_project_paths, Some(vec![]));
    }

    #[test]
    fn read_project_paths_from_disk_aborts_when_archived_key_wrong_type() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"projectPaths":["C:/a"],"archivedProjectPaths":"C:/old"}"#,
        )
        .unwrap();

        assert!(super::read_project_paths_from_disk(&path).is_err());
    }

    #[test]
    fn read_project_paths_from_disk_aborts_when_archived_list_has_non_string_element() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"projectPaths":["C:/a"],"archivedProjectPaths":["C:/old",123]}"#,
        )
        .unwrap();

        assert!(super::read_project_paths_from_disk(&path).is_err());
    }

    #[test]
    fn read_project_paths_from_disk_drops_non_string_project_path_elements() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"projectPaths":["C:/a",123],"archivedProjectPaths":["C:/old"]}"#,
        )
        .unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

        assert_eq!(lists.project_paths, vec!["C:/a".to_string()]);
        assert_eq!(
            lists.archived_project_paths,
            Some(vec!["C:/old".to_string()])
        );
    }

    #[test]
    fn read_project_paths_from_disk_aborts_on_unparseable_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        // G2: unparseable is NOT NotFound, so the reader aborts (Err), never disk=[].
        assert!(super::read_project_paths_from_disk(&path).is_err());
    }

    #[test]
    fn read_project_paths_from_disk_aborts_on_non_notfound_read_error() {
        // G2: a directory at the path yields a non-NotFound read error; the reader
        // must abort (Err), not degrade to empty (which would silently drop a project).
        let temp = tempfile::tempdir().unwrap();
        let dir_as_path = temp.path().join("iam-a-dir");
        std::fs::create_dir_all(&dir_as_path).unwrap();
        assert!(super::read_project_paths_from_disk(&dir_as_path).is_err());
    }

    #[test]
    fn save_settings_preserve_keeps_disk_project_paths_on_passthrough() {
        // Marquee preserve: disk has [A, X] (X = an append this in-memory candidate
        // never saw); a whole-object save that changed only an unrelated field must
        // leave project_paths = [A, X] on disk (fail-safe) AND persist the field.
        // #1077: real AC dirs so the selected runtime paths equal the preserved list.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let a = real_ac_project_path(temp.path(), "A");
        let x = real_ac_project_path(temp.path(), "X");
        super::save_settings_to_path(&settings_with_project_paths(&[&a, &x]), &path).unwrap();

        let mut candidate = settings_with_project_paths(&[&a]); // stale, missing X
        candidate.sidebar_style = "deep-space".to_string(); // unrelated GUI field
        let written =
            super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let reloaded: AppSettings = serde_json::from_str(&contents).unwrap();
        assert_eq!(reloaded.project_paths, vec![a.clone(), x.clone()]); // X preserved, not clobbered
        assert_eq!(reloaded.project_path.as_deref(), Some(a.as_str()));
        assert_eq!(reloaded.sidebar_style, "deep-space"); // unrelated field persisted
                                                          // #1077: the returned settings carry the SELECTED (validated) projection,
                                                          // which equals the preserved disk list because both dirs are real.
        assert_eq!(written.project_paths, vec![a.clone(), x.clone()]);
        assert_eq!(written.project_path.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn refresh_project_paths_from_path_keeps_live_archived_list_when_key_absent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"projectPaths":["A"],"projectPath":"A"}"#).unwrap();
        let mut settings = settings_with_project_and_archived_paths(&["stale"], &["Archived"]);

        super::refresh_project_paths_from_path(&mut settings, &path).unwrap();

        assert_eq!(settings.project_paths, vec!["A".to_string()]);
        assert_eq!(settings.project_path.as_deref(), Some("A"));
        assert_eq!(
            settings.archived_project_paths,
            vec!["Archived".to_string()]
        );
    }

    #[test]
    fn refresh_project_paths_from_path_clears_archived_list_when_disk_says_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"projectPaths":["A"],"projectPath":"A","archivedProjectPaths":[]}"#,
        )
        .unwrap();
        let mut settings = settings_with_project_and_archived_paths(&["stale"], &["Archived"]);

        super::refresh_project_paths_from_path(&mut settings, &path).unwrap();

        assert_eq!(settings.project_paths, vec!["A".to_string()]);
        assert_eq!(settings.project_path.as_deref(), Some("A"));
        assert!(settings.archived_project_paths.is_empty());
    }

    #[test]
    fn save_settings_preserves_disk_archived_list() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let a = real_ac_project_path(temp.path(), "A");
        let archived_a = real_ac_project_path(temp.path(), "ArchivedA");
        super::save_settings_to_path(
            &settings_with_project_and_archived_paths(&[&a], &[&archived_a]),
            &path,
        )
        .unwrap();
        let candidate = settings_with_project_and_archived_paths(&[&a], &["ArchivedB"]);

        let written =
            super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

        let reloaded: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reloaded.archived_project_paths, vec![archived_a.clone()]);
        assert_eq!(written.archived_project_paths, vec![archived_a.clone()]);
    }

    #[test]
    fn save_settings_returns_caller_archived_list_when_disk_key_absent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let a = real_ac_project_path(temp.path(), "A");
        let archived = real_ac_project_path(temp.path(), "Archived");
        // Seed a whole, valid AppSettings object but with NO archivedProjectPaths
        // key (the preserve writer's disk gate requires a valid whole object).
        let mut seed = serde_json::to_value(settings_with_project_paths(&[&a])).unwrap();
        seed.as_object_mut().unwrap().remove("archivedProjectPaths");
        std::fs::write(&path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

        let candidate = settings_with_project_and_archived_paths(&[&a], &[&archived]);
        let written =
            super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

        let reloaded: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written.project_paths, vec![a.clone()]);
        assert_eq!(written.project_path.as_deref(), Some(a.as_str()));
        // Disk had no archived key → materialize the caller's live archived list.
        assert_eq!(written.archived_project_paths, vec![archived.clone()]);
        assert_eq!(reloaded.archived_project_paths, vec![archived.clone()]);
    }

    #[test]
    fn save_settings_with_project_paths_writes_verbatim() {
        // The explicit writer persists the in-memory list as-is (deliberate mutation).
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        super::save_settings_to_path(&settings_with_project_paths(&["A"]), &path).unwrap();
        super::save_settings_to_path(&settings_with_project_paths(&["A", "Y"]), &path).unwrap();
        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        assert_eq!(lists.project_paths, vec!["A".to_string(), "Y".to_string()]);
    }

    #[test]
    fn save_settings_with_project_paths_writes_archived_verbatim() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        super::save_settings_with_project_paths_to_path(
            &settings_with_project_and_archived_paths(&["A"], &["Archived"]),
            &path,
        )
        .unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

        assert_eq!(
            lists.archived_project_paths,
            Some(vec!["Archived".to_string()])
        );
    }

    #[test]
    fn resync_reconciles_missing_and_conflict_without_duplication() {
        // Grinch Defect 1: a decoded state carrying a MISSING and a CONFLICT
        // record, mutated across register/remove, must never duplicate/resurrect
        // those records, and remove must actually remove.
        use crate::config::projects::{
            IssueKind, ProjectPathPersistenceState, ProjectSource, RawStringField, RepairKind,
            ResolvedPair,
        };
        let pair = |idx: usize, raw: &str, selected: Option<&str>, issue: Option<IssueKind>| {
            ResolvedPair {
                source: ProjectSource::ProjectPaths,
                index: Some(idx),
                raw_absolute: RawStringField::string(raw),
                raw_relative: RawStringField::absent(),
                absolute_side: super::absent_side(),
                relative_side: super::absent_side(),
                selected: selected.map(String::from),
                selected_canonical_raw: selected.map(String::from),
                selected_identity: None,
                issue,
                repair: RepairKind::None,
            }
        };
        let state = ProjectPathPersistenceState {
            pairs: vec![
                pair(0, "A", Some("A"), None),
                pair(1, "M", None, Some(IssueKind::Missing)),
                pair(2, "C", None, Some(IssueKind::Conflict)),
            ],
            selected_head: Some("A".to_string()),
            active_registration_count: 3,
            archived_registration_count: 0,
            active_companion_present: true,
            archived_companion_present: false,
            has_genuine_singular: false,
            active_reconcile_eligible: false,
            archived_reconcile_eligible: false,
            structural_issues: Vec::new(),
            runtime_authoritative: false,
        };
        let mut s = AppSettings {
            project_path_state: std::sync::Arc::new(state),
            ..AppSettings::default()
        };
        // Raw runtime (3-field refresh) carries all three stored absolutes.
        s.project_paths = vec!["A".to_string(), "M".to_string(), "C".to_string()];

        // Register D.
        s.project_paths.push("D".to_string());
        super::resync_project_state_from_runtime(&mut s);
        {
            let st = &s.project_path_state;
            assert_eq!(
                st.active_registration_count, 4,
                "no duplication on register"
            );
            let keys: Vec<String> = st
                .active_pairs()
                .iter()
                .map(|p| p.raw_absolute.value.clone().unwrap())
                .collect();
            assert_eq!(
                keys,
                vec!["A", "M", "C", "D"],
                "order preserved, no duplicates"
            );
            assert_eq!(st.active_pairs()[1].issue, Some(IssueKind::Missing));
            assert_eq!(st.active_pairs()[2].issue, Some(IssueKind::Conflict));
            assert!(st.active_pairs()[3].issue.is_none());
        }

        // Remove M ("Remove from list" on the missing entry).
        s.project_paths = vec!["A".to_string(), "C".to_string(), "D".to_string()];
        super::resync_project_state_from_runtime(&mut s);
        {
            let st = &s.project_path_state;
            assert_eq!(
                st.active_registration_count, 3,
                "the missing record must actually be removed, not resurrected"
            );
            let keys: Vec<String> = st
                .active_pairs()
                .iter()
                .map(|p| p.raw_absolute.value.clone().unwrap())
                .collect();
            assert_eq!(keys, vec!["A", "C", "D"]);
            assert_eq!(
                st.active_pairs()[1].issue,
                Some(IssueKind::Conflict),
                "unrelated conflict preserved through the removal"
            );
        }
    }

    #[test]
    fn resync_persists_mutation_against_decoded_state() {
        // Regression: a mutator that changes only the runtime lists against a
        // DECODED (non-synthesized) hidden state must resync so the reconcile
        // write records the new project instead of copying the stale disk list.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let a = real_ac_project_path(temp.path(), "A");
        let b = real_ac_project_path(temp.path(), "B");
        super::save_settings_to_path(&settings_with_project_paths(&[&a]), &path).unwrap();

        // A decoded hidden state as the loader produces (runtime_authoritative=false).
        let value = serde_json::json!({ "projectPaths": [a.clone()], "projectPath": a.clone() });
        let state = super::decode_project_state(
            value.as_object().unwrap(),
            None,
            &crate::config::projects::FsCandidateResolver,
        );
        assert!(!state.runtime_authoritative);
        let mut settings = settings_with_project_paths(&[&a]);
        settings.project_path_state = std::sync::Arc::new(state);

        // Register B into the runtime list, then resync + reconcile-write.
        settings.project_paths.push(b.clone());
        super::resync_project_state_from_runtime(&mut settings);
        super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        assert_eq!(lists.project_paths, vec![a, b]);
    }

    /// #1171, 9.2.17 - **the regression a plain `BTreeMap<String, WatcherConfig>` would let
    /// through, and whose failure mode is the whole settings file.**
    ///
    /// Three hand-written mistakes, one per entry: a capitalized enum variant, a scalar where
    /// a list belongs, and a quoted number. Without the per-entry wrapper, ONE of them makes
    /// `serde_json::from_value::<AppSettings>` fail, `load_settings_from_path` replaces
    /// everything with `AppSettings::default()`, and AgentsCommander starts with no agents
    /// configured and one log line - after which every save is refused by the #1077 gate.
    #[test]
    fn one_malformed_watcher_does_not_take_the_settings_file_down() {
        let contents = serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": ["-NoLogo"],
            "agents": [
                { "id": "a1", "label": "Claude", "command": "claude", "color": "#fff" },
                { "id": "a2", "label": "Codex", "command": "codex", "color": "#000" }
            ],
            "watchers": {
                "bad-mode": { "mode": "State", "pattern": "x" },
                "bad-commands": { "mode": "occurrence", "pattern": "x", "commands": "claude" },
                "bad-window": { "mode": "occurrence", "pattern": "x", "dedupeWindowMs": "2000" },
                "good": { "mode": "state", "pattern": "Permission required" }
            }
        })
        .to_string();

        let (settings, _) = super::parse_settings_json(&contents, "test").expect(
            "a malformed watcher must never fail the whole file: that is what the wrapper is for",
        );

        // Every OTHER setting survived.
        assert_eq!(settings.agents.len(), 2);
        assert_eq!(settings.default_shell, "powershell.exe");

        // Every entry is still there, and exactly one of them resolved.
        assert_eq!(settings.watchers.len(), 4);
        let valid: Vec<&str> = settings
            .watchers
            .iter()
            .filter(|(_, entry)| entry.valid().is_some())
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(valid, vec!["good"]);

        // ...and the three bad ones are kept VERBATIM, so a save writes back the user's bytes
        // instead of deleting what it could not read.
        let round_tripped = serde_json::to_value(&settings.watchers).expect("serializes");
        assert_eq!(
            round_tripped["bad-mode"],
            serde_json::json!({ "mode": "State", "pattern": "x" })
        );
        assert_eq!(
            round_tripped["bad-commands"],
            serde_json::json!({ "mode": "occurrence", "pattern": "x", "commands": "claude" })
        );
        assert_eq!(
            round_tripped["bad-window"],
            serde_json::json!({ "mode": "occurrence", "pattern": "x", "dedupeWindowMs": "2000" })
        );
    }

    /// #1171, 9.2.18 - a user who configures nothing never sees either new key appear.
    #[test]
    fn a_settings_file_with_no_watchers_round_trips_without_the_new_keys() {
        let contents = serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": []
        })
        .to_string();

        let (settings, _) = super::parse_settings_json(&contents, "test").expect("parses");
        assert!(settings.watchers.is_empty());
        assert!(settings.watchers_geometry.is_none());

        let written = serde_json::to_value(&settings).expect("serializes");
        let root = written.as_object().expect("object");
        assert!(
            !root.contains_key("watchers"),
            "an empty map must not appear"
        );
        assert!(
            !root.contains_key("watchersGeometry"),
            "this is why watchersGeometry carries skip_serializing_if and mainGeometry does not"
        );
    }

    /// #1171, 9.2.19 - a configured watcher round-trips value for value, INCLUDING the
    /// absent-against-`[]` distinction for `commands`, which is the one place where the two
    /// are opposites: absent reaches every agent, `[]` reaches none.
    #[test]
    fn watchers_round_trip_through_save_and_load_including_absent_versus_empty() {
        let contents = serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "watchers": {
                "all-agents": { "mode": "state", "pattern": "  Permission" },
                "nobody": { "mode": "occurrence", "pattern": "Read", "commands": [] },
                "claude-only": {
                    "enabled": false,
                    "mode": "occurrence",
                    "pattern": "Read \\((.+)\\)",
                    "commands": ["claude"],
                    "dedupe": "capture",
                    "dedupeWindowMs": 5000,
                    "capturedAgainst": "claude 2.1.212"
                }
            }
        })
        .to_string();

        let (settings, _) = super::parse_settings_json(&contents, "test").expect("parses");

        let all = settings.watchers["all-agents"].valid().expect("valid");
        assert!(all.enabled, "enabled defaults to true");
        assert_eq!(all.mode, super::WatcherMode::State);
        assert_eq!(all.pattern, "  Permission");
        assert!(all.commands.is_none(), "absent means every agent");
        assert_eq!(all.dedupe, super::WatcherDedupe::Row);
        assert_eq!(all.dedupe_window_ms, 2000);

        let nobody = settings.watchers["nobody"].valid().expect("valid");
        assert_eq!(
            nobody.commands.as_deref(),
            Some(&[] as &[String]),
            "`[]` must survive as `[]` and never collapse into absent"
        );

        let claude = settings.watchers["claude-only"].valid().expect("valid");
        assert!(!claude.enabled);
        assert_eq!(claude.mode, super::WatcherMode::Occurrence);
        assert_eq!(claude.dedupe, super::WatcherDedupe::Capture);
        assert_eq!(claude.dedupe_window_ms, 5000);
        assert_eq!(claude.captured_against.as_deref(), Some("claude 2.1.212"));

        // Byte-stable: what comes back out is what went in, key for key.
        let written = serde_json::to_value(&settings.watchers).expect("serializes");
        let original: serde_json::Value = serde_json::from_str(&contents).expect("parses");
        let mut expected = original["watchers"].clone();
        // The two defaulted fields the input omitted are written explicitly, since neither
        // carries `skip_serializing_if`: they are settings with a value, not absent ones.
        expected["all-agents"]["enabled"] = serde_json::json!(true);
        expected["all-agents"]["dedupe"] = serde_json::json!("row");
        expected["all-agents"]["dedupeWindowMs"] = serde_json::json!(2000);
        expected["nobody"]["enabled"] = serde_json::json!(true);
        expected["nobody"]["dedupe"] = serde_json::json!("row");
        expected["nobody"]["dedupeWindowMs"] = serde_json::json!(2000);
        assert_eq!(written, expected);
    }

    #[test]
    fn save_settings_preserve_twice_succeeds_no_sharing_violation() {
        // G4a: the preserve-read closes its handle before the #774 rename, so two
        // preserve-saves in a row both succeed on Windows (a lingering read handle
        // would trip MoveFileEx with os 32).
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        super::save_settings_to_path(&settings_with_project_paths(&["A"]), &path).unwrap();
        super::save_settings_to_path_preserving_project_paths(
            &settings_with_project_paths(&["A"]),
            &path,
        )
        .unwrap();
        super::save_settings_to_path_preserving_project_paths(
            &settings_with_project_paths(&["A"]),
            &path,
        )
        .unwrap();
        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        assert_eq!(lists.project_paths, vec!["A".to_string()]);
    }

    #[test]
    fn remove_project_disk_composition_preserves_unseen_entry() {
        // remove_project command body against a tempdir: disk [A, X] (X unseen by the
        // sidebar), reconcile from disk, remove A, write verbatim => disk ends [X].
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        super::save_settings_to_path(&settings_with_project_paths(&["A", "X"]), &path).unwrap();

        let mut s = settings_with_project_paths(&["A"]); // stale in-memory
        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        s.project_paths = lists.project_paths;
        s.project_path = lists.project_path; // reconciled to [A, X]
        assert!(crate::config::projects::remove_project_path(&mut s, "A"));
        super::save_settings_to_path(&s, &path).unwrap(); // verbatim

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        assert_eq!(lists.project_paths, vec!["X".to_string()]); // A gone, X preserved
        assert_eq!(lists.project_path.as_deref(), Some("X"));
    }

    #[test]
    fn add_reconciles_from_disk_before_upsert() {
        // Add command body: in-memory stale [A], disk [A, X]; reconcile then append Y
        // and write verbatim => disk ends [A, X, Y] (the CLI append X is not lost).
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        super::save_settings_to_path(&settings_with_project_paths(&["A", "X"]), &path).unwrap();

        let mut s = settings_with_project_paths(&["A"]); // stale
        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        s.project_paths = lists.project_paths;
        s.project_path = lists.project_path; // [A, X]
        s.project_paths.push("Y".to_string()); // stand-in for register upsert
        s.project_path = s.project_paths.first().cloned();
        super::save_settings_to_path(&s, &path).unwrap();

        let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
        assert_eq!(
            lists.project_paths,
            vec!["A".to_string(), "X".to_string(), "Y".to_string()]
        );
    }

    #[test]
    fn main_sidebar_side_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert_eq!(s.main_sidebar_side, MainSidebarSide::Right);
        s.main_sidebar_side = MainSidebarSide::Left;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"mainSidebarSide\":\"left\""));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.main_sidebar_side, MainSidebarSide::Left);
    }

    #[test]
    fn main_sidebar_side_defaults_to_right_when_missing_from_json() {
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert_eq!(s.main_sidebar_side, MainSidebarSide::Right);
    }

    #[test]
    fn main_resource_monitor_attached_round_trips_and_defaults_false() {
        // #587 - round-trips through serde as camelCase.
        let mut s = AppSettings::default();
        assert!(!s.main_resource_monitor_attached);
        s.main_resource_monitor_attached = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"mainResourceMonitorAttached\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.main_resource_monitor_attached);

        // Absent from an older settings.json => serde default false.
        let json_without = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
        let from_old: AppSettings =
            serde_json::from_str(json_without).expect("deserialize old json");
        assert!(!from_old.main_resource_monitor_attached);
    }

    // ── Issue #248 — legacy startOnlyCoordinators → restoreCoordinatorWakeState ──
    //
    // The minimal JSON below carries the three fields without serde defaults
    // (`defaultShell`, `defaultShellArgs`, `agents`) plus whichever issue-#248
    // field is under test. All other AppSettings fields use their serde defaults.

    #[test]
    fn issue_248_migration_legacy_true_translates_to_new_true() {
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": true
        }"#;
        let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
        // Pre-migration: legacy field is parsed, new field is at its default.
        assert_eq!(s.legacy_start_only_coordinators, Some(true));
        assert!(!s.restore_coordinator_wake_state);

        super::apply_issue_248_migration(&mut s);

        assert!(s.restore_coordinator_wake_state);
        assert!(s.legacy_start_only_coordinators.is_none());

        // Round-trip — the legacy field must NOT reappear on next save.
        let out = serde_json::to_string(&s).expect("serialize");
        assert!(!out.contains("startOnlyCoordinators"));
        assert!(out.contains("\"restoreCoordinatorWakeState\":true"));
    }

    #[test]
    fn issue_248_migration_legacy_false_translates_to_new_false() {
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": false
        }"#;
        let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
        super::apply_issue_248_migration(&mut s);
        assert!(!s.restore_coordinator_wake_state);
        assert!(s.legacy_start_only_coordinators.is_none());
    }

    #[test]
    fn issue_248_no_legacy_field_keeps_new_field_value() {
        // Fresh install or post-migrated settings.json — no legacy field.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "restoreCoordinatorWakeState": true
        }"#;
        let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
        super::apply_issue_248_migration(&mut s);
        assert!(s.restore_coordinator_wake_state); // untouched
        assert!(s.legacy_start_only_coordinators.is_none());
    }

    #[test]
    fn issue_248_migration_conflict_keeps_new_value_and_drops_legacy() {
        // Grinch Z4 — both keys present, conflicting values. The user (or a
        // third-party tool) wrote restoreCoordinatorWakeState=true AFTER an
        // older startOnlyCoordinators=false was already on disk. The new value
        // wins; the legacy key is silently dropped from the next save. The
        // helper emits a `warn!` log line for triage — not asserted here (log
        // capture is not wired in the existing test suite), just exercised.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": false,
            "restoreCoordinatorWakeState": true
        }"#;
        let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
        assert_eq!(s.legacy_start_only_coordinators, Some(false));
        assert!(s.restore_coordinator_wake_state);

        super::apply_issue_248_migration(&mut s);

        assert!(s.restore_coordinator_wake_state); // preserved
        assert!(s.legacy_start_only_coordinators.is_none()); // dropped
    }

    #[test]
    fn coord_sort_by_activity_defaults_when_missing_from_json() {
        // Old settings.json without the new field must deserialize to false.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(!s.coord_sort_by_activity);
    }

    #[test]
    fn spec_board_enabled_defaults_false_when_missing_from_json() {
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(!s.spec_board_enabled);
    }

    // ---- #548: per-(agent, letter) profile label overrides ----

    #[test]
    fn profile_labels_override_survives_serde_round_trip() {
        let mut settings = AppSettings::default();
        settings
            .coding_agent_profiles
            .profile_labels_by_agent
            .insert(
                "codex".to_string(),
                BTreeMap::from([("B".to_string(), "turbo".to_string())]),
            );

        let json = serde_json::to_string(&settings).unwrap();
        let round_tripped: AppSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(
            round_tripped.coding_agent_profiles.profile_labels_by_agent["codex"]["B"],
            "turbo"
        );
    }

    #[test]
    fn profile_labels_default_to_empty_and_always_serialize() {
        // A pre-#548 v2 codingAgentProfiles object with NO profileLabelsByAgent key
        // must load as an empty map (proves #[serde(default)]).
        let json = r##"{
            "schemaVersion": 2,
            "profileSlots": { "A": { "label": "" } },
            "defaultProfileByAgent": {},
            "profilesByAgent": {}
        }"##;

        let profiles: super::CodingAgentProfilesConfig = serde_json::from_str(json).unwrap();
        assert!(profiles.profile_labels_by_agent.is_empty());

        // Re-serializing always emits the key (no skip_serializing_if), parity with
        // profilesByAgent, so the on-disk key is stable.
        let out = serde_json::to_string(&profiles).unwrap();
        assert!(out.contains("profileLabelsByAgent"));
    }

    #[test]
    fn repair_prunes_invalid_letter_label_overrides_and_keeps_valid() {
        let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0
                                                                        // Pre-seed the A cell so the config is otherwise repair-clean; the only
                                                                        // change repair makes is dropping the invalid-letter overrides.
        settings.coding_agent_profiles.profiles_by_agent.insert(
            "agent-0".to_string(),
            BTreeMap::from([("A".to_string(), super::empty_profile_cell())]),
        );
        settings
            .coding_agent_profiles
            .profile_labels_by_agent
            .insert(
                "agent-0".to_string(),
                BTreeMap::from([
                    ("B".to_string(), "turbo".to_string()),
                    ("1".to_string(), "bad-digit".to_string()),
                    ("ab".to_string(), "bad-two-char".to_string()),
                    ("b".to_string(), "bad-lowercase".to_string()),
                ]),
            );

        let changed = repair_coding_agent_profiles_config(
            &mut settings.coding_agent_profiles,
            &settings.agents,
        );

        assert!(changed);
        let labels = &settings.coding_agent_profiles.profile_labels_by_agent["agent-0"];
        assert_eq!(labels.get("B").map(String::as_str), Some("turbo"));
        assert!(!labels.contains_key("1"));
        assert!(!labels.contains_key("ab"));
        assert!(!labels.contains_key("b"));
    }

    #[test]
    fn repair_keeps_orphan_agent_id_label_override() {
        // An override under an agent id NOT present in settings.agents must survive
        // repair: no agent-id prune (plan 3.7); it is inert dead storage the
        // resolver never reads.
        let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0
        settings
            .coding_agent_profiles
            .profile_labels_by_agent
            .insert(
                "ghost-agent".to_string(),
                BTreeMap::from([("B".to_string(), "turbo".to_string())]),
            );

        repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

        assert_eq!(
            settings.coding_agent_profiles.profile_labels_by_agent["ghost-agent"]["B"],
            "turbo"
        );
    }

    #[test]
    fn repair_with_valid_label_map_does_not_flip_changed() {
        let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0
                                                                        // Make the config otherwise repair-clean: agent-0 already holds its A cell.
        settings.coding_agent_profiles.profiles_by_agent.insert(
            "agent-0".to_string(),
            BTreeMap::from([("A".to_string(), super::empty_profile_cell())]),
        );
        // A label map holding only valid letters must not trigger a disk rewrite.
        settings
            .coding_agent_profiles
            .profile_labels_by_agent
            .insert(
                "agent-0".to_string(),
                BTreeMap::from([("B".to_string(), "turbo".to_string())]),
            );

        let changed = repair_coding_agent_profiles_config(
            &mut settings.coding_agent_profiles,
            &settings.agents,
        );

        assert!(!changed);
        assert_eq!(
            settings.coding_agent_profiles.profile_labels_by_agent["agent-0"]["B"],
            "turbo"
        );
    }

    #[test]
    fn resource_monitor_settings_default_when_missing_from_json() {
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(s.resource_monitor_enabled);
        assert_eq!(s.max_concurrent_agent_processes, 32);
        assert_eq!(s.resource_watchdog_action, ResourceWatchdogAction::Warn);
        assert!(validate_resource_settings(&s).is_ok());
    }

    // ---- #1032: the per-agent context regex ---------------------------------------

    /// Criterion 6. Every settings file that exists today predates this field, so the one
    /// thing this field must never do is make an existing config fail to load.
    #[test]
    fn settings_without_context_regex_deserialize_unchanged() {
        let json = r##"{ "id": "claude", "label": "Claude", "command": "claude",
                         "color": "#10b981" }"##;
        let agent: super::AgentConfig = serde_json::from_str(json).expect("deserializes");
        assert_eq!(agent.label, "Claude");
        assert_eq!(agent.context_regex, None, "absent must mean off, not empty");
    }

    /// The field is an agent-config field, where an absent key is the correct way to say
    /// "off" - unlike the IPC payload, where `percent` must serialize as an explicit null.
    #[test]
    fn context_regex_round_trips_as_camel_case() {
        let json = r##"{ "id": "claude", "label": "Claude", "command": "claude",
                         "color": "#10b981",
                         "contextRegex": "^ {2}Context [\u2591\u2588]+ (\\d{1,3})%" }"##;
        let agent: super::AgentConfig = serde_json::from_str(json).expect("deserializes");
        assert_eq!(
            agent.context_regex.as_deref(),
            Some("^ {2}Context [\u{2591}\u{2588}]+ (\\d{1,3})%")
        );

        let back = serde_json::to_string(&agent).expect("serializes");
        assert!(
            back.contains("\"contextRegex\""),
            "the frontend contract is camelCase: {back}"
        );

        let cleared = super::AgentConfig {
            context_regex: None,
            ..agent
        };
        let back = serde_json::to_string(&cleared).expect("serializes");
        assert!(
            !back.contains("contextRegex"),
            "None must omit the key here, so an untouched config stays untouched: {back}"
        );
    }
}
