use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
    pub git_pull_before: bool,
    /// If true, auto-generate .claude/settings.local.json with claudeMdExcludes on agent creation
    #[serde(default)]
    pub exclude_global_claude_md: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_shell: String,
    pub default_shell_args: Vec<String>,
    /// Agent folders and parent folders to scan for potential agents
    pub repo_paths: Vec<String>,
    /// Available coding agents
    pub agents: Vec<AgentConfig>,
    /// Configured Telegram bots for bridge
    #[serde(default)]
    pub telegram_bots: Vec<TelegramBotConfig>,
    /// Keep sidebar window always on top
    #[serde(default)]
    pub sidebar_always_on_top: bool,
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
    /// Zoom level for the sidebar window (1.0 = 100%)
    #[serde(default = "default_zoom")]
    pub sidebar_zoom: f64,
    /// Zoom level for the terminal window (1.0 = 100%)
    #[serde(default = "default_zoom")]
    pub terminal_zoom: f64,
    /// Zoom level for the guide window (1.0 = 100%)
    #[serde(default = "default_zoom")]
    pub guide_zoom: f64,
    /// Zoom level for the dark factory window (1.0 = 100%)
    #[serde(default = "default_zoom")]
    pub darkfactory_zoom: f64,
    /// Saved geometry for the sidebar window
    #[serde(default)]
    pub sidebar_geometry: Option<WindowGeometry>,
    /// Saved geometry for the terminal window
    #[serde(default)]
    pub terminal_geometry: Option<WindowGeometry>,
    /// Enable the embedded web server for remote browser access
    #[serde(default)]
    pub web_server_enabled: bool,
    /// Port for the web server
    #[serde(default = "default_web_port")]
    pub web_server_port: u16,
    /// Bind address: "127.0.0.1" (local only) or "0.0.0.0" (all interfaces)
    #[serde(default = "default_web_bind")]
    pub web_server_bind: String,
    /// Currently loaded project path (persisted across restarts)
    #[serde(default)]
    pub project_path: Option<String>,
}

fn default_true() -> bool {
    true
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
    9876
}

fn default_web_bind() -> String {
    "127.0.0.1".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        let (default_shell, default_shell_args, repo_paths) = if cfg!(target_os = "windows") {
            (
                "powershell.exe".to_string(),
                vec!["-NoLogo".to_string()],
                vec![],
            )
        } else {
            (
                "/bin/bash".to_string(),
                vec![],
                vec![format!("{}/repos", dirs::home_dir().unwrap_or_default().display())],
            )
        };

        Self {
            default_shell,
            default_shell_args,
            repo_paths,
            agents: vec![],
            telegram_bots: vec![],
            sidebar_always_on_top: false,
            raise_terminal_on_click: true,
            voice_to_text_enabled: false,
            gemini_api_key: String::new(),
            gemini_model: default_gemini_model(),
            voice_auto_execute: true,
            voice_auto_execute_delay: default_voice_delay(),
            sidebar_zoom: default_zoom(),
            terminal_zoom: default_zoom(),
            guide_zoom: default_zoom(),
            darkfactory_zoom: default_zoom(),
            sidebar_geometry: None,
            terminal_geometry: None,
            web_server_enabled: false,
            web_server_port: default_web_port(),
            web_server_bind: default_web_bind(),
            project_path: None,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("settings.json"))
}

/// Load settings from the app config directory (see config_dir()), falling back to defaults
pub fn load_settings() -> AppSettings {

    let path = match settings_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory, using defaults");
            return AppSettings::default();
        }
    };

    if !path.exists() {
        log::info!("No settings file found at {:?}, using defaults", path);
        return AppSettings::default();
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<AppSettings>(&contents) {
            Ok(settings) => {
                log::info!("Loaded settings from {:?}", path);
                settings
            }
            Err(e) => {
                log::error!("Failed to parse settings file: {}", e);
                AppSettings::default()
            }
        },
        Err(e) => {
            log::error!("Failed to read settings file: {}", e);
            AppSettings::default()
        }
    }
}

/// Save settings to the app config directory (see config_dir())
pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("settings.json");

    // Ensure directory exists
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create settings directory: {}", e))?;

    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write settings file: {}", e))?;

    log::info!("Saved settings to {:?}", path);
    Ok(())
}

pub type SettingsState = Arc<RwLock<AppSettings>>;
