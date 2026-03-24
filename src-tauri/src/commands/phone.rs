use crate::config::dark_factory;
use crate::phone::manager;
use crate::phone::types::{AgentInfo, PhoneMessage};

#[tauri::command]
pub async fn phone_send_message(
    from: String,
    to: String,
    body: String,
    team: String,
) -> Result<String, String> {
    let config = dark_factory::load_dark_factory();
    manager::send_message(&from, &to, &body, &team, &config)
}

#[tauri::command]
pub async fn phone_get_inbox(agent_name: String) -> Result<Vec<PhoneMessage>, String> {
    manager::get_inbox(&agent_name)
}

#[tauri::command]
pub async fn phone_list_agents() -> Result<Vec<AgentInfo>, String> {
    let config = dark_factory::load_dark_factory();
    Ok(manager::list_agents(&config))
}

#[tauri::command]
pub async fn phone_ack_messages(
    agent_name: String,
    message_ids: Vec<String>,
) -> Result<(), String> {
    manager::ack_messages(&agent_name, &message_ids)
}
