impl crate::web::commands::WsState {
    pub(crate) fn new(
        session_mgr: std::sync::Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>,
        pty_mgr: std::sync::Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>,
        settings: crate::config::settings::SettingsState,
        broadcaster: crate::web::broadcast::WsBroadcaster,
        app_handle: tauri::AppHandle,
    ) -> Self {
        Self {
            session_mgr,
            pty_mgr,
            settings,
            broadcaster,
            app_handle,
        }
    }
}
