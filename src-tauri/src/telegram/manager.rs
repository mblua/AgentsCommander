use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::manager::PtyManager;
use crate::telegram::bridge::{self, BridgeHandle};
use crate::telegram::types::{BridgeInfo, BridgeStatus, TelegramBotConfig};

/// Shared map of session_id → mpsc sender. The PTY read loop checks this
/// to clone bytes to an active bridge. Uses std::sync::Mutex because the
/// PTY read loop runs on a std::thread, not tokio.
pub type OutputSenderMap =
    Arc<Mutex<HashMap<Uuid, tokio::sync::mpsc::Sender<Vec<u8>>>>>;

/// Shared map of session_id → typing flag. When true, the user is actively
/// typing in the terminal and bridge output should be suppressed until Enter.
pub type TypingFlagMap = Arc<Mutex<HashMap<Uuid, Arc<AtomicBool>>>>;

pub struct TelegramBridgeManager {
    bridges: HashMap<Uuid, BridgeHandle>,
    bot_assignments: HashMap<String, Uuid>,
    output_senders: OutputSenderMap,
    typing_flags: TypingFlagMap,
}

pub type TelegramBridgeState = Arc<tokio::sync::Mutex<TelegramBridgeManager>>;

impl TelegramBridgeManager {
    pub fn new(output_senders: OutputSenderMap, typing_flags: TypingFlagMap) -> Self {
        Self {
            bridges: HashMap::new(),
            bot_assignments: HashMap::new(),
            output_senders,
            typing_flags,
        }
    }

    pub fn attach(
        &mut self,
        session_id: Uuid,
        bot: &TelegramBotConfig,
        pty_mgr: Arc<Mutex<PtyManager>>,
        app_handle: tauri::AppHandle,
    ) -> Result<BridgeInfo, AppError> {
        // Exclusivity: one bot can only be attached to one session
        if let Some(existing) = self.bot_assignments.get(&bot.id) {
            return Err(AppError::Telegram(format!(
                "Bot '{}' already attached to session {}",
                bot.label, existing
            )));
        }

        // One session can only have one bridge
        if self.bridges.contains_key(&session_id) {
            return Err(AppError::Telegram(format!(
                "Session {} already has a bridge attached",
                session_id
            )));
        }

        let info = BridgeInfo {
            bot_id: bot.id.clone(),
            bot_label: bot.label.clone(),
            session_id: session_id.to_string(),
            status: BridgeStatus::Active,
            color: bot.color.clone(),
        };

        let typing_flag = Arc::new(AtomicBool::new(false));

        let handle = bridge::spawn_bridge(
            bot.token.clone(),
            bot.chat_id,
            session_id,
            info.clone(),
            pty_mgr,
            app_handle,
            typing_flag.clone(),
        );

        // Register output sender so PTY read loop feeds it
        if let Ok(mut senders) = self.output_senders.lock() {
            senders.insert(session_id, handle.output_sender.clone());
        }

        // Register typing flag so pty_write can signal typing state
        if let Ok(mut flags) = self.typing_flags.lock() {
            flags.insert(session_id, typing_flag);
        }

        self.bot_assignments.insert(bot.id.clone(), session_id);
        self.bridges.insert(session_id, handle);

        Ok(info)
    }

    pub fn detach(&mut self, session_id: Uuid) -> Result<(), AppError> {
        let handle = self.bridges.remove(&session_id).ok_or_else(|| {
            AppError::Telegram(format!(
                "No bridge attached to session {}",
                session_id
            ))
        })?;

        handle.cancel.cancel();

        if let Ok(mut senders) = self.output_senders.lock() {
            senders.remove(&session_id);
        }

        if let Ok(mut flags) = self.typing_flags.lock() {
            flags.remove(&session_id);
        }

        self.bot_assignments.retain(|_, sid| *sid != session_id);

        Ok(())
    }

    pub fn list_bridges(&self) -> Vec<BridgeInfo> {
        self.bridges.values().map(|h| h.info.clone()).collect()
    }

    pub fn get_bridge(&self, session_id: Uuid) -> Option<BridgeInfo> {
        self.bridges.get(&session_id).map(|h| h.info.clone())
    }

    pub fn has_bridge(&self, session_id: Uuid) -> bool {
        self.bridges.contains_key(&session_id)
    }
}
