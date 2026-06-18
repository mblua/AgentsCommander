use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::telegram::types::TelegramBotConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub id: String,
    pub label: String,
    pub command: String,
    pub color: String,
    /// If true, run `git pull` before launching the agent
    #[serde(default)]
    pub git_pull_before: bool,
    /// If true, auto-generate .claude/settings.local.json with claudeMdExcludes on agent creation
    #[serde(default)]
    pub exclude_global_claude_md: bool,
    /// Base environment rows applied to every launch of this coding agent.
    #[serde(default)]
    pub envs: Vec<CodingAgentEnv>,
    /// When true for Codex, AC provides an isolated CODEX_HOME at spawn time.
    #[serde(default, alias = "isolateCodexHome")]
    pub isolated_home: bool,
    /// #529 - filename AC writes into the agent root at launch (content = AC
    /// context + Role.md). `None`/empty falls back to the command-derived default
    /// (Claude -> CLAUDE.md, Gemini -> GEMINI.md, else AGENTS.md). Serialized as
    /// `instructionsFilename`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
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
    /// Enable the embedded web server for remote browser access
    #[serde(default)]
    pub web_server_enabled: bool,
    /// Port for the web server
    #[serde(default = "default_web_port")]
    pub web_server_port: u16,
    /// Bind address: "127.0.0.1" (local only) or "0.0.0.0" (all interfaces)
    #[serde(default = "default_web_bind")]
    pub web_server_bind: String,
    /// Currently loaded project path (legacy single-project, kept for backward compat)
    #[serde(default)]
    pub project_path: Option<String>,
    /// Currently loaded project paths (multi-project support)
    #[serde(default)]
    pub project_paths: Vec<String>,
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
    /// When true, AC writes the RTK PreToolUse rewriter hook into every managed
    /// agent dir's `.claude/settings.local.json` (matrices + workgroup replicas).
    /// Toggled from Settings/General. See issue #120.
    #[serde(default)]
    pub inject_rtk_hook: bool,
    /// When true, the startup banner offering to enable `inject_rtk_hook` is
    /// suppressed for the lifetime of this settings file. Set by the `[Don't
    /// ask again]` button on the banner. See issue #120.
    #[serde(default)]
    pub rtk_prompt_dismissed: bool,
    /// When true, AC may surface the startup banner offering to enable
    /// `inject_rtk_hook`, subject to the other gates: `rtk` on PATH,
    /// `inject_rtk_hook == false`, and `rtk_prompt_dismissed == false`.
    /// Defaults to `false` so the banner is opt-in and never appears unless the
    /// user enables it in Settings → General → RTK. See issue #426.
    #[serde(default)]
    pub inform_when_rtk_installed: bool,
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
    /// Defaults to `true` so existing settings files without the field stay on
    /// the legacy light-mode behavior. Issue #289.
    #[serde(default = "default_true")]
    pub theme_light: bool,
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
}

fn default_true() -> bool {
    true
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
    16
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
            web_server_enabled: false,
            web_server_port: default_web_port(),
            web_server_bind: default_web_bind(),
            project_path: None,
            project_paths: vec![],
            sidebar_style: default_sidebar_style(),
            root_token: None,
            onboarding_dismissed: false,
            coord_sort_by_activity: false,
            always_show_selected_workgroup: true,
            log_level: None,
            inject_rtk_hook: false,
            rtk_prompt_dismissed: false,
            inform_when_rtk_installed: false,
            auto_generate_task_title: true,
            agent_templates_path: None,
            theme_light: true,
            spec_board_enabled: false,
            resource_monitor_enabled: default_resource_monitor_enabled(),
            max_concurrent_agent_processes: default_max_concurrent_agent_processes(),
            resource_watchdog_action: default_resource_watchdog_action(),
            agent_group_warn_private_bytes: default_agent_group_warn_private_bytes(),
            agent_group_kill_private_bytes: default_agent_group_kill_private_bytes(),
            agent_process_kill_private_bytes: default_agent_process_kill_private_bytes(),
            resource_keep_last_snapshot: default_resource_keep_last_snapshot(),
            resource_backoff_polling: default_resource_backoff_polling(),
        }
    }
}

/// Pure decision for the boot-time RTK banner/sweep mode (issue #120 §18,
/// extended by issue #426). Maps the three persisted settings flags plus the
/// runtime `rtk`-on-PATH probe to one of the four `RtkStartupMode` strings the
/// frontend understands. Side-effect-free so the full truth table is unit
/// testable without booting Tauri, spawning the setup task, probing PATH, or
/// touching disk.
///
/// `inform_when_rtk_installed` is the issue #426 opt-in gate: the
/// `prompt-enable` banner only appears when the user has explicitly turned it
/// on. `rtk_prompt_dismissed` is retained as a secondary suppression gate (the
/// banner's `[Don't ask again]` button) and still wins when set.
pub fn compute_rtk_startup_mode(
    rtk_present: bool,
    inject_enabled: bool,
    prompt_dismissed: bool,
    inform_when_rtk_installed: bool,
) -> &'static str {
    match (
        rtk_present,
        inject_enabled,
        prompt_dismissed,
        inform_when_rtk_installed,
    ) {
        // Opt-in banner: rtk present, not yet injected, not dismissed, and the
        // user asked to be informed. All four must hold (issue #426).
        (true, false, false, true) => "prompt-enable",
        // Already enabled: recover/refresh the hook. `inform` and `dismissed`
        // are irrelevant once injection is on.
        (true, true, _, _) => "active",
        // Enabled but the binary vanished from PATH: auto-disable and clean up.
        (false, true, _, _) => "auto-disabled",
        // Everything else (inform=false, or dismissed=true, or rtk absent and
        // not injected) is a no-op.
        _ => "silent",
    }
}

fn command_token_basename(token: &str) -> String {
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
    let settings: AppSettings = serde_json::from_value(value)
        .map_err(|e| format!("Failed to deserialize settings from {source}: {e}"))?;
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

    for (_agent_id, cells) in profiles.profiles_by_agent.iter_mut() {
        let original_cells_len = cells.len();
        cells.retain(|letter, _| is_valid_profile_letter(letter));
        changed |= cells.len() != original_cells_len;
    }

    // #548: prune invalid (non A..Z) letters from each agent's label-override map,
    // mirroring the cell-letter prune above. Never creates override entries and never
    // prunes by agent id (an override for a not-yet-loaded agent is harmless and may
    // precede its agent in load order, per plan 3.7). `retain` keeps every valid key,
    // so a clean config leaves `changed` false and is not rewritten on load.
    for (_agent_id, labels) in profiles.profile_labels_by_agent.iter_mut() {
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
    if trimmed.contains("%AC_ROOT%") {
        if !(trimmed == "%AC_ROOT%"
            || trimmed.starts_with("%AC_ROOT%/")
            || trimmed.starts_with("%AC_ROOT%\\"))
        {
            return Err(format!(
                "{context}: CODEX_HOME template must start with %AC_ROOT% as a complete path segment"
            ));
        }
        reject_unknown_codex_home_template_markers(trimmed, context)?;
        return Ok(());
    }
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
        if rest.starts_with("%AC_ROOT%") {
            rest = &rest["%AC_ROOT%".len()..];
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

fn validate_env_rows(rows: &[CodingAgentEnv], context: &str) -> Result<(), String> {
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

pub fn validate_and_repair_settings(settings: &mut AppSettings) -> Result<(), String> {
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    validate_agent_commands(settings)?;
    validate_resource_settings(settings)
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
                validate_agent_command_text(
                    &format!("Coding agent profile '{}:{}'", agent_id, letter),
                    &cell.command,
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

fn validate_agent_command_text(context: &str, command: &str) -> Result<(), String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command)
        .map_err(|e| format!("{context}: invalid command: {e}. command={command:?}"))?;
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
    if !(1..=16).contains(&settings.max_concurrent_agent_processes) {
        return Err("maxConcurrentAgentProcesses must be between 1 and 16".to_string());
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
                    log::info!("Loaded settings from {:?}", path);
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
            if let Err(e) = save_settings_to_path(&settings, path) {
                log::error!(
                    "Failed to persist settings (root_token gen and/or settings migration): {}",
                    e
                );
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
                    log::info!("[cli] Loaded settings from {:?}", path);
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

/// Save settings to the app config directory (see config_dir()).
/// Atomic write (tmp + rename) per G.14 — mirrors `sessions_persistence::save_sessions`.
/// Splitter-drag debouncing raises save frequency in 0.8.0; atomic writes ensure a crash
/// mid-write cannot corrupt the existing settings.json.
pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("settings.json");
    save_settings_to_path(settings, &path)
}

fn save_settings_to_path(settings: &AppSettings, path: &Path) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("Settings path {} has no parent", path.display()))?;
    // Ensure directory exists
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create settings directory: {}", e))?;

    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    let tmp_path = dir.join("settings.json.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("Failed to write temp settings file: {}", e))?;
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("Failed to rename settings file: {}", e))?;

    log::info!("Saved settings to {:?}", path);
    Ok(())
}

pub type SettingsState = Arc<RwLock<AppSettings>>;

#[cfg(test)]
mod tests {

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

    use super::{
        merge_protected_coding_agent_settings, repair_coding_agent_profiles_config,
        validate_agent_commands, validate_resource_settings, AgentConfig, AppSettings,
        CodingAgentEnv, CodingAgentEnvSource, MainSidebarSide, ProfileCellConfig,
        ProfileSlotConfig, ResourceWatchdogAction, TelegramNetworkPollErrorLogging,
        TelegramPollFailureLogLevel, TelegramPollRecoveryLogLevel,
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
                    git_pull_before: false,
                    exclude_global_claude_md: false,
                    envs: Vec::new(),
                    isolated_home: false,
                    instructions_filename: None,
                })
                .collect(),
            ..AppSettings::default()
        }
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
    fn theme_light_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(s.theme_light);
        s.theme_light = false;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"themeLight\":false"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(!back.theme_light);
    }

    #[test]
    fn theme_light_defaults_true_when_missing_from_json() {
        // Old settings.json without the new field must deserialize with
        // theme_light = true so existing users stay on the legacy light-mode
        // behavior until they explicitly switch to dark.
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
        assert!(s.theme_light);
    }

    #[test]
    fn theme_light_explicit_false_survives_round_trip() {
        // Once a user explicitly disables light mode, the value must survive
        // serialize/deserialize without reverting to the `default_true` default.
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
        assert_eq!(s.max_concurrent_agent_processes, 16);
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
    fn validate_resource_settings_rejects_invalid_limits() {
        let s = AppSettings {
            max_concurrent_agent_processes: 0,
            ..AppSettings::default()
        };
        assert!(validate_resource_settings(&s)
            .unwrap_err()
            .contains("maxConcurrentAgentProcesses"));

        let s = AppSettings {
            max_concurrent_agent_processes: 17,
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
    fn inject_rtk_hook_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.inject_rtk_hook);
        s.inject_rtk_hook = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"injectRtkHook\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.inject_rtk_hook);
    }

    #[test]
    fn rtk_prompt_dismissed_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.rtk_prompt_dismissed);
        s.rtk_prompt_dismissed = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"rtkPromptDismissed\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.rtk_prompt_dismissed);
    }

    #[test]
    fn inform_when_rtk_installed_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.inform_when_rtk_installed);
        s.inform_when_rtk_installed = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"informWhenRtkInstalled\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.inform_when_rtk_installed);
    }

    #[test]
    fn inform_when_rtk_installed_defaults_false_when_missing_from_json() {
        // Old settings.json without the #426 field must deserialize to false so
        // the startup banner stays opt-in (never appears after an upgrade).
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": []
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert!(!s.inform_when_rtk_installed);
    }

    #[test]
    fn compute_rtk_startup_mode_prompt_enable_requires_all_gates() {
        // Opt-in banner: only when rtk present, injection off, not dismissed,
        // AND the user opted in (#426).
        assert_eq!(
            super::compute_rtk_startup_mode(true, false, false, true),
            "prompt-enable"
        );
    }

    #[test]
    fn compute_rtk_startup_mode_silent_when_inform_off() {
        // Core #426 regression guard: rtk present, not injected, not dismissed,
        // but inform=false => no banner (silent). This is the new default.
        assert_eq!(
            super::compute_rtk_startup_mode(true, false, false, false),
            "silent"
        );
    }

    #[test]
    fn compute_rtk_startup_mode_dismissed_suppresses_even_when_inform_on() {
        // rtk_prompt_dismissed remains a secondary gate (#426): a dismissed
        // prompt stays suppressed even if inform was later enabled.
        assert_eq!(
            super::compute_rtk_startup_mode(true, false, true, true),
            "silent"
        );
    }

    #[test]
    fn compute_rtk_startup_mode_active_when_injected_regardless_of_inform() {
        assert_eq!(
            super::compute_rtk_startup_mode(true, true, false, false),
            "active"
        );
        assert_eq!(
            super::compute_rtk_startup_mode(true, true, true, true),
            "active"
        );
    }

    #[test]
    fn compute_rtk_startup_mode_auto_disabled_when_missing_but_injected() {
        assert_eq!(
            super::compute_rtk_startup_mode(false, true, false, false),
            "auto-disabled"
        );
    }

    #[test]
    fn compute_rtk_startup_mode_silent_when_missing_and_not_injected() {
        assert_eq!(
            super::compute_rtk_startup_mode(false, false, false, true),
            "silent"
        );
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

        repair_coding_agent_profiles_config(
            &mut settings.coding_agent_profiles,
            &settings.agents,
        );

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
        assert_eq!(s.max_concurrent_agent_processes, 16);
        assert_eq!(s.resource_watchdog_action, ResourceWatchdogAction::Warn);
        assert!(validate_resource_settings(&s).is_ok());
    }
}
