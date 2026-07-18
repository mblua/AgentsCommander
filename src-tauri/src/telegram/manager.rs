use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use crate::errors::AppError;
use crate::network::OutboundNetwork;
use crate::pty::manager::PtyManager;
use crate::telegram::bridge::{self, BridgeHandle, SessionReaderKind};
use crate::telegram::types::{BridgeInfo, BridgeStatus, TelegramBotConfig};

/// Shared map of session_id → mpsc sender. The PTY read loop checks this
/// to clone bytes to an active bridge. Uses std::sync::Mutex because the
/// PTY read loop runs on a std::thread, not tokio.
pub type OutputSenderMap = Arc<Mutex<HashMap<Uuid, tokio::sync::mpsc::Sender<Vec<u8>>>>>;

pub struct TelegramBridgeManager {
    bridges: HashMap<Uuid, BridgeHandle>,
    bot_assignments: HashMap<String, Uuid>,
    output_senders: OutputSenderMap,
    #[cfg(test)]
    detach_counts: HashMap<Uuid, usize>,
}

pub type TelegramBridgeState = Arc<tokio::sync::Mutex<TelegramBridgeManager>>;

#[must_use = "BridgeShutdown must be consumed with spawn_wait_or_abort() or abort_now() after releasing TelegramBridgeState"]
pub struct BridgeShutdown {
    session_id: Uuid,
    tasks: Vec<JoinHandle<()>>,
}

impl BridgeShutdown {
    pub fn spawn_wait_or_abort(self) {
        tauri::async_runtime::spawn(async move {
            self.wait_or_abort().await;
        });
    }

    pub fn abort_now(self) {
        for task in self.tasks {
            task.abort();
        }
    }

    async fn wait_or_abort(self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        for mut task in self.tasks {
            if Instant::now() >= deadline {
                task.abort();
                continue;
            }

            match timeout_at(deadline, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) if e.is_cancelled() => {}
                Ok(Err(e)) => {
                    log::warn!(
                        "[telegram] Bridge task for session {} ended with error: {}",
                        self.session_id,
                        e
                    );
                }
                Err(_) => {
                    task.abort();
                    log::warn!(
                        "[telegram] Bridge task for session {} did not stop within timeout",
                        self.session_id
                    );
                }
            }
        }
    }
}

impl TelegramBridgeManager {
    pub fn new(output_senders: OutputSenderMap) -> Self {
        Self {
            bridges: HashMap::new(),
            bot_assignments: HashMap::new(),
            output_senders,
            #[cfg(test)]
            detach_counts: HashMap::new(),
        }
    }

    pub fn attach<R: tauri::Runtime>(
        &mut self,
        session_id: Uuid,
        bot: &TelegramBotConfig,
        pty_mgr: Arc<Mutex<PtyManager>>,
        network: OutboundNetwork,
        app_handle: tauri::AppHandle<R>,
        reader: Option<SessionReaderKind>,
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

        let is_reader_mode = reader.is_some();

        let handle = bridge::spawn_bridge(
            bot.token.clone(),
            bot.chat_id,
            session_id,
            info.clone(),
            pty_mgr,
            network,
            app_handle,
            reader,
        );

        // Only register output sender for PTY mode.
        // In reader mode, the watcher reads directly from file — no PTY byte feed needed.
        if !is_reader_mode {
            if let Ok(mut senders) = self.output_senders.lock() {
                senders.insert(session_id, handle.output_sender.clone());
            }
        }

        self.bot_assignments.insert(bot.id.clone(), session_id);
        self.bridges.insert(session_id, handle);

        Ok(info)
    }

    pub fn detach(&mut self, session_id: Uuid) -> Result<BridgeShutdown, AppError> {
        let handle = self.bridges.remove(&session_id).ok_or_else(|| {
            AppError::Telegram(format!("No bridge attached to session {}", session_id))
        })?;

        handle.cancel.cancel();

        if let Ok(mut senders) = self.output_senders.lock() {
            senders.remove(&session_id);
        }

        self.bot_assignments.retain(|_, sid| *sid != session_id);
        #[cfg(test)]
        {
            *self.detach_counts.entry(session_id).or_default() += 1;
        }

        Ok(BridgeShutdown {
            session_id,
            tasks: handle.tasks,
        })
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

    #[cfg(test)]
    pub(crate) fn insert_test_bridge(&mut self, session_id: Uuid, bot_id: &str) {
        let (output_sender, _output_receiver) = tokio::sync::mpsc::channel(1);
        let info = BridgeInfo {
            bot_id: bot_id.to_string(),
            bot_label: bot_id.to_string(),
            session_id: session_id.to_string(),
            status: BridgeStatus::Active,
            color: "#000000".to_string(),
        };
        self.bot_assignments.insert(bot_id.to_string(), session_id);
        self.bridges.insert(
            session_id,
            BridgeHandle {
                info,
                cancel: tokio_util::sync::CancellationToken::new(),
                output_sender,
                tasks: Vec::new(),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn test_detach_count(&self, session_id: Uuid) -> usize {
        self.detach_counts
            .get(&session_id)
            .copied()
            .unwrap_or_default()
    }

    /// Cancel all active bridges. Called during app shutdown.
    pub fn cancel_all(&mut self) -> Vec<BridgeShutdown> {
        let mut shutdowns = Vec::new();
        for (session_id, handle) in self.bridges.drain() {
            handle.cancel.cancel();
            shutdowns.push(BridgeShutdown {
                session_id,
                tasks: handle.tasks,
            });
        }
        if let Ok(mut senders) = self.output_senders.lock() {
            senders.clear();
        }
        self.bot_assignments.clear();
        if !shutdowns.is_empty() {
            log::info!(
                "[telegram] Cancelled {} active bridges for shutdown",
                shutdowns.len()
            );
        }
        shutdowns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn cancel_all_drains_bridge_state_and_returns_shutdowns() {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let mut manager = TelegramBridgeManager::new(Arc::clone(&output_senders));
        let session_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let task = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        manager.bot_assignments.insert("bot-1".into(), session_id);
        output_senders
            .lock()
            .unwrap()
            .insert(session_id, tx.clone());
        manager.bridges.insert(
            session_id,
            BridgeHandle {
                info: BridgeInfo {
                    bot_id: "bot-1".into(),
                    bot_label: "Bot 1".into(),
                    session_id: session_id.to_string(),
                    status: BridgeStatus::Active,
                    color: "#229ED9".into(),
                },
                cancel: cancel.clone(),
                output_sender: tx,
                tasks: vec![task],
            },
        );

        let shutdowns = manager.cancel_all();

        assert!(cancel.is_cancelled());
        assert!(manager.bridges.is_empty());
        assert!(manager.bot_assignments.is_empty());
        assert!(output_senders.lock().unwrap().is_empty());
        assert_eq!(shutdowns.len(), 1);
        for shutdown in shutdowns {
            shutdown.abort_now();
        }
    }
}
