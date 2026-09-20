use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::capture::registry::CaptureRegistry;
use crate::errors::AppError;
use crate::network::OutboundNetwork;
use crate::pty::manager::PtyManager;
use crate::session::profile::CodingAgentKind;
use crate::capture::key::Cut;
use crate::telegram::bridge::{self, BridgeHandle, ReaderDest, ReaderTask};
use crate::telegram::types::{BridgeInfo, BridgeStatus, TelegramBotConfig};

/// Who is asking for a session's transcript reader (#2232 phase 4 section 5).
///
/// A demand is a `(session_id, consumer)` pair. Adding one **never** restarts a
/// running reader; releasing the last one cancels it and closes the session's
/// `CaptureRegistry` entry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReaderConsumer {
    /// A Telegram bot is attached to the session.
    Bot,
    /// The room wants a reader because Co-managed is effective for the session.
    Room,
}

/// A live reader task and the demands keeping it alive.
///
/// The reader lives here and **not** in [`BridgeHandle::tasks`]: a room can ask
/// for a reader without a bot, and a bot can attach over a reader that is
/// already running.
pub struct ReaderEntry {
    /// Stable for the lifetime of the reader task. A demand added over a live
    /// reader leaves this value alone, which is what pins "adding a demand
    /// never restarts the reader".
    pub reader_id: u64,
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
    dest: watch::Sender<Option<ReaderDest>>,
    reanchor: watch::Sender<u64>,
    frontier: Arc<Mutex<Option<Cut>>>,
    demands: BTreeSet<ReaderConsumer>,
}

impl ReaderEntry {
    /// Wrap a freshly spawned reader, with no demand yet: the caller records
    /// the demand that caused it through [`TelegramBridgeManager::reader_install`].
    pub fn new(reader_id: u64, spawned: ReaderTask) -> Self {
        Self {
            reader_id,
            cancel: spawned.cancel,
            tasks: spawned.tasks,
            dest: spawned.dest,
            reanchor: spawned.reanchor,
            frontier: spawned.frontier,
            demands: BTreeSet::new(),
        }
    }
}

/// Shared map of session_id → mpsc sender. The PTY read loop checks this
/// to clone bytes to an active bridge. Uses std::sync::Mutex because the
/// PTY read loop runs on a std::thread, not tokio.
pub type OutputSenderMap = Arc<Mutex<HashMap<Uuid, tokio::sync::mpsc::Sender<Vec<u8>>>>>;

pub struct TelegramBridgeManager {
    bridges: HashMap<Uuid, BridgeHandle>,
    bot_assignments: HashMap<String, Uuid>,
    output_senders: OutputSenderMap,
    /// #2232 phase 4: the per-session readers and their demands. Separate from
    /// `bridges`, because a reader outlives every bot attached to it.
    readers: HashMap<Uuid, ReaderEntry>,
    next_reader_id: u64,
    /// The single registry created in `lib.rs` (phase 3 section 5.1). The
    /// manager closes a session's entry when the last demand is released.
    captures: Arc<CaptureRegistry>,
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
    /// Every caller outside `lib.rs` gets a private registry: only the single
    /// app-wide one, created in `lib.rs` and stored in Tauri state (phase 3
    /// section 5.1), is shared, and it arrives through
    /// [`Self::with_captures`]. Keeping this signature stable is what holds
    /// phase 4's diff to its ten paths.
    pub fn new(output_senders: OutputSenderMap) -> Self {
        Self::with_captures(output_senders, Arc::new(CaptureRegistry::new()))
    }

    pub fn with_captures(output_senders: OutputSenderMap, captures: Arc<CaptureRegistry>) -> Self {
        Self {
            bridges: HashMap::new(),
            bot_assignments: HashMap::new(),
            output_senders,
            readers: HashMap::new(),
            next_reader_id: 1,
            captures,
            #[cfg(test)]
            detach_counts: HashMap::new(),
        }
    }

    // ── Reader demands (#2232 phase 4 section 5) ──────────────────────────

    /// The shared capture registry, so the supervisor can `open` a session's
    /// endpoints before spawning its reader.
    pub fn captures(&self) -> &Arc<CaptureRegistry> {
        &self.captures
    }

    /// True when a reader task already exists for `session_id`.
    pub fn reader_is_running(&self, session_id: Uuid) -> bool {
        self.readers.contains_key(&session_id)
    }

    /// The reader's stable identity, or `None` when no reader is running.
    pub fn reader_id(&self, session_id: Uuid) -> Option<u64> {
        self.readers.get(&session_id).map(|entry| entry.reader_id)
    }

    /// The demands currently keeping `session_id`'s reader alive.
    pub fn reader_demands(&self, session_id: Uuid) -> BTreeSet<ReaderConsumer> {
        self.readers
            .get(&session_id)
            .map(|entry| entry.demands.clone())
            .unwrap_or_default()
    }

    /// Hand out the next reader identity. Callers spawn the task, then pass the
    /// resulting [`ReaderEntry`] to [`Self::reader_install`], all while holding
    /// this manager's lock, so no second demand can race the spawn.
    pub fn next_reader_id(&mut self) -> u64 {
        let id = self.next_reader_id;
        self.next_reader_id += 1;
        id
    }

    /// Install a freshly spawned reader together with the demand that caused it.
    pub fn reader_install(
        &mut self,
        session_id: Uuid,
        entry: ReaderEntry,
        consumer: ReaderConsumer,
    ) {
        debug_assert!(
            !self.readers.contains_key(&session_id),
            "callers check reader_is_running under this lock before spawning"
        );
        if let Some(stale) = self.readers.insert(session_id, entry) {
            // Defensive: never leak a task. Adding a demand must not restart a
            // running reader, so reaching here is a caller bug.
            stale.cancel.cancel();
        }
        if let Some(entry) = self.readers.get_mut(&session_id) {
            entry.demands.insert(consumer);
        }
    }

    /// Register `consumer` against a **running** reader.
    ///
    /// Idempotent, and the reader is never restarted: the new consumer simply
    /// joins it. For [`ReaderConsumer::Bot`] the destination is switched, which
    /// is what runs the three §6 transitions inside the reader task. Returns
    /// `false` when no reader is running for `session_id`.
    pub fn reader_demand_add(
        &mut self,
        session_id: Uuid,
        consumer: ReaderConsumer,
        dest: Option<ReaderDest>,
    ) -> bool {
        let Some(entry) = self.readers.get_mut(&session_id) else {
            return false;
        };
        let joined = entry.demands.insert(consumer);
        if joined {
            // The new consumer joins a reader that was already running, so the
            // records it already produced are not candidates for it: record a
            // cut at the reader's current frontier (section 5, phase 3
            // section 7). Nothing is restarted.
            if let Some(slot) = self.captures.slot(&session_id.to_string()) {
                if let Some(cut) = entry.frontier.lock().ok().and_then(|f| f.clone()) {
                    slot.set_cut(cut);
                }
            }
        }
        if consumer == ReaderConsumer::Bot {
            let _ = entry.dest.send(dest);
        }
        true
    }

    /// Release `consumer`'s demand.
    ///
    /// Releasing the **bot** demand while a room demand remains keeps the
    /// reader running and stops Telegram sends. Releasing the **last** demand
    /// cancels the reader, drops its state and closes the session's
    /// `CaptureRegistry` entry; the returned shutdown must be awaited outside
    /// `TelegramBridgeState` (section 9).
    #[must_use = "the reader shutdown must be consumed after releasing TelegramBridgeState"]
    pub fn reader_demand_release(
        &mut self,
        session_id: Uuid,
        consumer: ReaderConsumer,
    ) -> Option<BridgeShutdown> {
        let Some(entry) = self.readers.get_mut(&session_id) else {
            return None;
        };
        entry.demands.remove(&consumer);
        if consumer == ReaderConsumer::Bot {
            let _ = entry.dest.send(None);
        }
        if !entry.demands.is_empty() {
            return None;
        }
        let entry = self.readers.remove(&session_id)?;
        entry.cancel.cancel();
        self.captures.close(&session_id.to_string());
        Some(BridgeShutdown {
            session_id,
            tasks: entry.tasks,
        })
    }

    /// Release **every** demand for `session_id`. Destroy and shutdown do this;
    /// a persistence rollback releases only the bot demand (section 5).
    #[must_use = "the reader shutdown must be consumed after releasing TelegramBridgeState"]
    pub fn reader_release_all(&mut self, session_id: Uuid) -> Option<BridgeShutdown> {
        let entry = self.readers.remove(&session_id)?;
        entry.cancel.cancel();
        self.captures.close(&session_id.to_string());
        Some(BridgeShutdown {
            session_id,
            tasks: entry.tasks,
        })
    }

    /// Signal the reader to re-anchor its transcript **file** (section 5.2).
    ///
    /// The demand set is untouched: there is no release, no re-raise and no
    /// `CaptureRegistry::close`, because AC preserves the session UUID across a
    /// restart. The cut's sequence part is superseded and the slot is cleared to
    /// `Empty` with a fresh `seq`, so a candidate captured before the restart
    /// can never be consumed after it.
    pub fn reader_reanchor(&self, session_id: Uuid) -> bool {
        let Some(entry) = self.readers.get(&session_id) else {
            return false;
        };
        let key = session_id.to_string();
        if let Some(slot) = self.captures.slot(&key) {
            slot.supersede_cut_sequence();
            slot.clear();
        }
        entry.reanchor.send_modify(|seq| *seq = seq.wrapping_add(1));
        true
    }

    // The 8-argument signature is the frozen plan spec (#1549 §5.4): the PTY-bridge
    // chain threads `agent_kind` as a loose parameter by design (no struct grouping).
    #[allow(clippy::too_many_arguments)]
    pub fn attach<R: tauri::Runtime>(
        &mut self,
        session_id: Uuid,
        bot: &TelegramBotConfig,
        pty_mgr: Arc<Mutex<PtyManager>>,
        network: OutboundNetwork,
        app_handle: tauri::AppHandle<R>,
        reader_mode: bool,
        agent_kind: Option<CodingAgentKind>,
    ) -> Result<BridgeInfo, AppError> {
        // #2232 phase 4 section 5: attaching is **idempotent**. A persisted bot
        // re-attaching and a local auto-attach no longer treat "a bridge
        // already exists" as an error when it is the same bot on the same
        // session; they simply re-register the same demand.
        if let Some(existing) = self.bridges.get(&session_id) {
            if existing.info.bot_id == bot.id {
                return Ok(existing.info.clone());
            }
            return Err(AppError::Telegram(format!(
                "Session {} already has a bridge attached",
                session_id
            )));
        }

        // Exclusivity: one bot can only be attached to one session
        if let Some(existing) = self.bot_assignments.get(&bot.id) {
            return Err(AppError::Telegram(format!(
                "Bot '{}' already attached to session {}",
                bot.label, existing
            )));
        }

        let info = BridgeInfo {
            bot_id: bot.id.clone(),
            bot_label: bot.label.clone(),
            session_id: session_id.to_string(),
            status: BridgeStatus::Active,
            color: bot.color.clone(),
        };

        let handle = bridge::spawn_bridge(
            bot.token.clone(),
            bot.chat_id,
            session_id,
            info.clone(),
            pty_mgr,
            network,
            app_handle,
            reader_mode,
            agent_kind,
        );

        // Only register output sender for PTY mode.
        // In reader mode, the watcher reads directly from file — no PTY byte feed needed.
        if !reader_mode {
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
        // #2232 phase 4 section 5: shutdown releases **both** demands, so every
        // reader is cancelled and its capture entry dropped.
        for (session_id, entry) in self.readers.drain() {
            entry.cancel.cancel();
            self.captures.close(&session_id.to_string());
            shutdowns.push(BridgeShutdown {
                session_id,
                tasks: entry.tasks,
            });
        }
        if !shutdowns.is_empty() {
            log::info!(
                "[telegram] Cancelled {} active bridges and readers for shutdown",
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
