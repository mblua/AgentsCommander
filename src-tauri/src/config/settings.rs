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
    /// Keep sidebar window always on top
    #[serde(default)]
    pub sidebar_always_on_top: bool,
    /// Raise terminal window when sidebar is clicked
    #[serde(default = "default_true")]
    pub raise_terminal_on_click: bool,
}

fn default_true() -> bool {
    true
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
            sidebar_always_on_top: false,
            raise_terminal_on_click: true,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("settings.json"))
}

/// Migrate settings from old ~/.summongate/ to ~/.agentscommander/ if needed
fn migrate_from_summongate() {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };
    let old_path = home.join(".summongate").join("settings.json");
    let new_dir = home.join(".agentscommander");
    let new_path = new_dir.join("settings.json");

    if old_path.exists() && !new_path.exists() {
        log::info!("Migrating settings from {:?} to {:?}", old_path, new_path);
        if let Err(e) = std::fs::create_dir_all(&new_dir) {
            log::error!("Failed to create new settings dir: {}", e);
            return;
        }
        if let Err(e) = std::fs::copy(&old_path, &new_path) {
            log::error!("Failed to copy settings: {}", e);
        }
    }
}

/// Load settings from the app config directory (see config_dir()), falling back to defaults
pub fn load_settings() -> AppSettings {
    migrate_from_summongate();

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
