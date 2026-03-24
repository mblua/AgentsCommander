use crate::config::dark_factory::{self, DarkFactoryConfig};

#[tauri::command]
pub async fn get_dark_factory() -> Result<DarkFactoryConfig, String> {
    Ok(dark_factory::load_dark_factory())
}

#[tauri::command]
pub async fn save_dark_factory(config: DarkFactoryConfig) -> Result<(), String> {
    dark_factory::save_dark_factory(&config)?;
    dark_factory::sync_agent_configs(&config)?;
    Ok(())
}
