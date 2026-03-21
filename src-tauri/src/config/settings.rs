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
    pub args: Vec<String>,
    pub color: String,
    /// If true, run `git pull` before launching the agent
    pub git_pull_before: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_shell: String,
    pub default_shell_args: Vec<String>,
    /// Base directories to scan for repos
    pub repo_paths: Vec<String>,
    /// Available coding agents
    pub agents: Vec<AgentConfig>,
    /// Configured Telegram bots for bridge
    #[serde(default)]
    pub telegram_bots: Vec<TelegramBotConfig>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_shell: "powershell.exe".to_string(),
            default_shell_args: vec!["-NoLogo".to_string()],
            repo_paths: vec![
                r"C:\Users\maria\0_repos".to_string(),
                r"C:\Users\maria\0_repos_phi".to_string(),
            ],
            agents: vec![],
            telegram_bots: vec![],
        }
    }
}

/// Returns the settings directory: ~/.summongate/
fn settings_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".summongate"))
}

/// Returns the settings file path: ~/.summongate/settings.json
fn settings_path() -> Option<PathBuf> {
    settings_dir().map(|d| d.join("settings.json"))
}

/// Load settings from ~/.summongate/settings.json, falling back to defaults
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

/// Save settings to ~/.summongate/settings.json
pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let dir = settings_dir().ok_or("Could not determine home directory")?;
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
