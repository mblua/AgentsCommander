use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::capture::key::{normalise_path, Cut};
use crate::capture::record::CapturedRecord;
use crate::capture::sink::CaptureSlot;
use crate::config::settings::TelegramNetworkPollErrorLogging;
use crate::network::OutboundNetwork;
use crate::pty::manager::PtyManager;
use crate::session::profile::CodingAgentKind;
use crate::telegram::output::{flush_buffer, BridgeLogger, DiagLogger, TelegramErrKind};
use crate::telegram::types::BridgeInfo;

mod filter;
mod poll;

use filter::filter_for_agent;
use filter::AgentFilter;
use filter::RowTracker;
#[cfg(test)]
use filter::{AgyFilter, ClaudeCodeFilter};
#[cfg(test)]
use poll::new_poll_backoff;
use poll::poll_task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollNetworkLogAction {
    Failure {
        level: log::Level,
        suppressed_since_last_emit: u32,
        sustained: bool,
    },
    Suppressed {
        level: log::Level,
    },
    Recovery {
        level: log::Level,
        outage_seconds: u64,
        suppressed_total: u32,
        sustained: bool,
    },
    None,
}

struct PollNetworkErrorState {
    first_failure_at: Option<Instant>,
    last_emit_at: Option<Instant>,
    suppressed_since_last_emit: u32,
    suppressed_total: u32,
    emitted_any_failure: bool,
    emitted_sustained: bool,
}

impl PollNetworkErrorState {
    fn new() -> Self {
        Self {
            first_failure_at: None,
            last_emit_at: None,
            suppressed_since_last_emit: 0,
            suppressed_total: 0,
            emitted_any_failure: false,
            emitted_sustained: false,
        }
    }

    fn record_network_failure(
        &mut self,
        policy: &TelegramNetworkPollErrorLogging,
    ) -> PollNetworkLogAction {
        let now = Instant::now();

        let Some(first) = self.first_failure_at else {
            self.first_failure_at = Some(now);
            self.last_emit_at = Some(now);
            self.emitted_any_failure = true;

            let sustained = policy.sustained_after_seconds == 0;
            if sustained {
                self.emitted_sustained = true;
            }

            return PollNetworkLogAction::Failure {
                level: if sustained {
                    policy.sustained_level.as_log_level()
                } else {
                    policy.first_failure_level.as_log_level()
                },
                suppressed_since_last_emit: 0,
                sustained,
            };
        };

        let sustained = now.saturating_duration_since(first)
            >= Duration::from_secs(policy.sustained_after_seconds);
        let repeat_ready = self
            .last_emit_at
            .map(|last| {
                now.saturating_duration_since(last)
                    >= Duration::from_secs(policy.sustained_repeat_seconds)
            })
            .unwrap_or(true);

        if sustained && repeat_ready {
            let suppressed = self.suppressed_since_last_emit;
            self.suppressed_since_last_emit = 0;
            self.last_emit_at = Some(now);
            self.emitted_any_failure = true;
            self.emitted_sustained = true;
            PollNetworkLogAction::Failure {
                level: policy.sustained_level.as_log_level(),
                suppressed_since_last_emit: suppressed,
                sustained: true,
            }
        } else {
            self.suppressed_since_last_emit = self.suppressed_since_last_emit.saturating_add(1);
            self.suppressed_total = self.suppressed_total.saturating_add(1);
            PollNetworkLogAction::Suppressed {
                level: policy.transient_repeat_level.as_log_level(),
            }
        }
    }

    fn record_success(&mut self, policy: &TelegramNetworkPollErrorLogging) -> PollNetworkLogAction {
        let Some(first) = self.first_failure_at else {
            return PollNetworkLogAction::None;
        };

        let outage_seconds = Instant::now().saturating_duration_since(first).as_secs();
        let should_log_recovery = self.emitted_any_failure;
        let suppressed_total = self.suppressed_total;
        let sustained = self.emitted_sustained;
        *self = Self::new();

        if should_log_recovery {
            PollNetworkLogAction::Recovery {
                level: policy.recovery_level.as_log_level(),
                outage_seconds,
                suppressed_total,
                sustained,
            }
        } else {
            PollNetworkLogAction::None
        }
    }

    fn reset_without_recovery(&mut self) {
        *self = Self::new();
    }
}

// ── Bridge spawn ─────────────────────────────────────────────

/// Which session-reader pipeline to spawn for this bridge. `None` (the bridge
/// signature) falls back to the PTY 6-phase pipeline (legacy noisy mode).
#[derive(Debug, Clone)]
pub enum SessionReaderKind {
    /// Watch Claude Code's append-only JSONL session log at the resolved projects dir.
    ///
    /// `transcript_id` is the transcript id AC minted at spawn (#2454), when
    /// the effective argv names one; the reader pins its first attach to
    /// `<id>.jsonl`. `None` keeps the attach by newest mtime.
    Claude {
        project_dir: PathBuf,
        transcript_id: Option<String>,
    },
    /// Watch Codex CLI's append-only `rollout-*.jsonl` under `~/.codex/sessions/`,
    /// filtering candidates by `session_meta.cwd` match.
    Codex {
        search_root: PathBuf,
        cwd: String,
        attach_time: chrono::DateTime<chrono::Utc>,
    },
}

pub struct BridgeHandle {
    pub info: BridgeInfo,
    pub cancel: CancellationToken,
    pub output_sender: mpsc::Sender<Vec<u8>>,
    pub tasks: Vec<JoinHandle<()>>,
}

/// The Telegram destination a reader sends to (#2232 phase 4 section 6).
///
/// This is the Claude watcher's own target type under the supervisor's name.
/// The watchers must stay outside the 88-member SCC (section 10), so the arc
/// runs supervisor → watcher only and never the reverse; `codex_watcher`
/// declares its own equivalent, which this module converts to at spawn time.
pub use super::claude_watcher::BotTarget as ReaderDest;

/// Everything a freshly spawned reader hands back to the supervisor.
pub struct ReaderTask {
    pub cancel: CancellationToken,
    pub tasks: Vec<JoinHandle<()>>,
    pub dest: ReaderDestSender,
    /// The reader's last observation, kept as a ready-made [`Cut`] so a demand
    /// raised over the running reader can record one (section 5, phase 3
    /// section 7) without stopping or re-reading anything.
    pub frontier: Arc<Mutex<Option<Cut>>>,
}

/// The supervisor's send side of a reader's destination watch (#2232 phase 4).
///
/// Each watcher declares its own `BotTarget` (section 4.1/§10), so the
/// supervisor keeps one sender per watcher flavour rather than sharing a type
/// across them. The demand registry only ever attaches (`Some`) or detaches
/// (`None`); this type makes both transitions available without the registry
/// naming either watcher's target type.
#[derive(Debug)]
pub enum ReaderDestSender {
    Claude(tokio::sync::watch::Sender<Option<super::claude_watcher::BotTarget>>),
    Codex(tokio::sync::watch::Sender<Option<super::codex_watcher::BotTarget>>),
}

impl ReaderDestSender {
    /// Publish a destination transition to the reader task. A send error means
    /// the reader is already gone, which the demand release accounts for.
    pub fn send(&self, dest: Option<ReaderDest>) {
        match self {
            Self::Claude(tx) => {
                let _ = tx.send(dest);
            }
            Self::Codex(tx) => {
                let _ = tx.send(dest.map(codex_bot_target));
            }
        }
    }

    /// The destination the reader currently holds. The idempotency check of
    /// section 5 compares against this before sending, so repeating the same
    /// Bot demand produces no watch update at all.
    pub fn current(&self) -> Option<ReaderDest> {
        match self {
            Self::Claude(tx) => tx.borrow().clone(),
            Self::Codex(tx) => tx.borrow().clone().map(|target| ReaderDest {
                token: target.token,
                chat_id: target.chat_id,
            }),
        }
    }

    /// Test-only: observe future transitions through one uniform receiver.
    ///
    /// The Codex tier forwards through a small watcher task so a test never
    /// needs to name `codex_watcher::BotTarget`; production only sends and
    /// reads the current value.
    #[cfg(test)]
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<ReaderDest>> {
        match self {
            Self::Claude(tx) => tx.subscribe(),
            Self::Codex(tx) => {
                let (out_tx, out_rx) = tokio::sync::watch::channel(self.current());
                let mut inner = tx.subscribe();
                tauri::async_runtime::spawn(async move {
                    while inner.changed().await.is_ok() {
                        let next = inner.borrow_and_update().clone().map(|target| ReaderDest {
                            token: target.token,
                            chat_id: target.chat_id,
                        });
                        if out_tx.send(next).is_err() {
                            break;
                        }
                    }
                });
                out_rx
            }
        }
    }
}

/// Convert the supervisor's destination into the Codex watcher's own target
/// type. Both carry exactly the same two credentials; the split exists only to
/// keep the watchers from naming each other (section 10).
fn codex_bot_target(dest: ReaderDest) -> super::codex_watcher::BotTarget {
    super::codex_watcher::BotTarget {
        token: dest.token,
        chat_id: dest.chat_id,
    }
}

/// Drain the reader's capture channel into the session's slot.
///
/// This is the last link of the phase-3 chain: without it the reader emits
/// records that reach nothing, which is the "merge green, production emits
/// nothing" failure phase 4 exists to close (acceptance criterion 8).
fn spawn_capture_pump(
    slot: CaptureSlot,
    mut rx: mpsc::UnboundedReceiver<Arc<CapturedRecord>>,
    cancel: CancellationToken,
    frontier: Arc<Mutex<Option<Cut>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                next = rx.recv() => {
                    let Some(record) = next else { break };
                    if let Ok(mut guard) = frontier.lock() {
                        *guard = Some(Cut {
                            path: normalise_path(&record.observed_path),
                            epoch: record.epoch,
                            len: record.observed_len,
                            reader_seq: Some(record.reader_seq),
                        });
                    }
                    let epoch = record.epoch;
                    slot.offer(record, epoch);
                }
            }
        }
    })
}

/// Spawn the transcript reader for one session, outside any bridge.
///
/// This is the phase-4 supervisor entry point: the reader no longer lives in
/// [`BridgeHandle::tasks`], so a room can ask for one without a bot (`dest ==
/// None`) and a bot can attach over one that is already running (by sending a
/// new destination through the returned `watch`). `sink` is the live
/// `CaptureRegistry` sender — passing `Some(tx)` here is what makes phases 1
/// and 3 reachable from production (acceptance criterion 8).
pub fn spawn_reader<R: tauri::Runtime>(
    kind: SessionReaderKind,
    session_id: Uuid,
    dest: Option<ReaderDest>,
    network: OutboundNetwork,
    app_handle: tauri::AppHandle<R>,
    sink: Option<mpsc::UnboundedSender<Arc<CapturedRecord>>>,
    capture: Option<(CaptureSlot, mpsc::UnboundedReceiver<Arc<CapturedRecord>>)>,
) -> ReaderTask {
    let cancel = CancellationToken::new();
    let frontier: Arc<Mutex<Option<Cut>>> = Arc::new(Mutex::new(None));
    let session_id_str = session_id.to_string();

    // Each watcher gets its own live destination `watch`, not a snapshot taken
    // at spawn: a bot attaching over a running reader must reach the task
    // through the channel (section 4.1). Codex receives its own `BotTarget`
    // flavour of the same value.
    let (task, dest) = match kind {
        SessionReaderKind::Claude {
            project_dir,
            transcript_id,
        } => {
            let (dest_tx, dest_rx) = tokio::sync::watch::channel(dest);
            let task = super::claude_watcher::spawn_watch_task(
                project_dir,
                transcript_id,
                network,
                dest_rx,
                session_id_str,
                cancel.clone(),
                app_handle,
                sink,
            );
            (task, ReaderDestSender::Claude(dest_tx))
        }
        SessionReaderKind::Codex {
            search_root,
            cwd,
            attach_time,
        } => {
            let (dest_tx, dest_rx) = tokio::sync::watch::channel(dest.map(codex_bot_target));
            let task = super::codex_watcher::spawn_watch_task(
                search_root,
                cwd,
                attach_time,
                network,
                dest_rx,
                session_id_str,
                cancel.clone(),
                app_handle,
                sink,
            );
            (task, ReaderDestSender::Codex(dest_tx))
        }
    };

    let mut tasks = vec![task];
    if let Some((slot, rx)) = capture {
        // The cut is superseded at reader start, before the first record is
        // processed (phase 3 section 7).
        slot.supersede_cut_sequence();
        tasks.push(spawn_capture_pump(
            slot,
            rx,
            cancel.clone(),
            Arc::clone(&frontier),
        ));
    }

    ReaderTask {
        cancel,
        tasks,
        dest,
        frontier,
    }
}

/// Spawn the bot-side bridge tasks for one session.
///
/// `reader_mode` is `true` when the session has a transcript reader: the PTY
/// 6-phase pipeline is then skipped, and the reader itself is owned by the
/// supervisor, not by this handle (#2232 phase 4).
#[allow(clippy::too_many_arguments)]
pub fn spawn_bridge<R: tauri::Runtime>(
    bot_token: String,
    chat_id: i64,
    session_id: Uuid,
    info: BridgeInfo,
    pty_mgr: Arc<Mutex<PtyManager>>,
    network: OutboundNetwork,
    app_handle: tauri::AppHandle<R>,
    reader_mode: bool,
    agent_kind: Option<CodingAgentKind>,
) -> BridgeHandle {
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(256);
    let mut tasks = Vec::new();

    let session_id_str = session_id.to_string();

    if reader_mode {
        drop(rx);
    } else {
        tasks.push(tokio::spawn(output_task(
            rx,
            network.clone(),
            bot_token.clone(),
            chat_id,
            session_id_str.clone(),
            cancel.clone(),
            app_handle.clone(),
            agent_kind,
        )));
    }

    // Poll task: Telegram getUpdates -> write to PTY stdin (runs in BOTH modes)
    tasks.push(tokio::spawn(poll_task(
        bot_token,
        chat_id,
        session_id,
        session_id_str,
        pty_mgr,
        network,
        cancel.clone(),
        app_handle,
    )));

    BridgeHandle {
        info,
        cancel,
        output_sender: tx,
        tasks,
    }
}

// ── Output task (PTY -> Telegram) ────────────────────────────
//
// Pipeline phases:
//   Phase 1: RAW BYTES   - PTY stdout chunks (Vec<u8>)
//   Phase 2: VT100 PARSE - vt100::Parser renders to virtual screen
//   Phase 3: STABILIZE   - RowTracker: emit only rows stable for 800ms+
//   Phase 4: FILTER      - AgentFilter: remove TUI chrome (agent-specific)
//   Phase 5: BUFFER      - Accumulate + dedup consecutive lines
//   Phase 6: SEND        - Chunk at 4000 chars, rate-limit, send to Telegram

const VT_ROWS: u16 = 50;
const VT_COLS: u16 = 220;
const STABILIZATION_MS: u64 = 800;
const TICK_MS: u64 = 200;
const FLUSH_DELAY_MS: u64 = 500;

// The 8-argument signature is the frozen plan spec (#1549 §5.4): the PTY-bridge
// chain threads `agent_kind` as a loose parameter by design (no struct grouping).
#[allow(clippy::too_many_arguments)]
async fn output_task<R: tauri::Runtime>(
    mut rx: mpsc::Receiver<Vec<u8>>,
    network: OutboundNetwork,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    agent_kind: Option<CodingAgentKind>,
) {
    let mut logger = BridgeLogger::new(&session_id);
    let mut diag = DiagLogger::new();
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

    // Phase 2: Virtual terminal parser
    let mut vt = vt100::Parser::new(VT_ROWS, VT_COLS, 0);

    // Phase 3: Row stabilization tracker
    let mut tracker = RowTracker::new(VT_ROWS, STABILIZATION_MS);

    // Phase 4: Agent-specific filter, selected by the session's agent kind.
    // INIT log line below shows `filter=antigravity` for agy sessions and
    // `filter=claude-code` for everything else (live evidence hook).
    let filter: Box<dyn AgentFilter> = filter_for_agent(agent_kind);

    // Tick interval for harvesting stabilized rows
    let mut tick = tokio::time::interval(Duration::from_millis(TICK_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    logger.log(
        "INIT",
        &session_id,
        &format!(
            "output_task started: filter={} stabilization={}ms tick={}ms",
            filter.name(),
            STABILIZATION_MS,
            TICK_MS,
        ),
    );

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            // Periodic tick: harvest stabilized rows, flush buffer if ready
            _ = tick.tick() => {
                let stable_lines = tracker.harvest_stable(filter.as_ref());

                if !stable_lines.is_empty() {
                    let raw_text = stable_lines.join("\n");
                    diag.log_raw(&raw_text);
                    logger.log("STABLE", &session_id, &raw_text);

                    for line in &stable_lines {
                        buffer.push_str(line);
                        buffer.push('\n');
                    }
                    last_buffer_add = Instant::now();
                }

                // Flush buffer if enough time has passed since last addition
                if !buffer.is_empty() {
                    let since_last = last_buffer_add.elapsed();
                    let buf_len = buffer.trim().len();
                    if since_last >= flush_delay || buf_len > 2000 {
                        flush_buffer(
                            &mut buffer, &network, &token, chat_id,
                            &session_id, &app, &mut logger, &mut diag,
                            false,
                        ).await;
                    }
                }
            }

            // Phase 1: Receive raw PTY bytes
            maybe_data = rx.recv() => {
                match maybe_data {
                    Some(data) => {
                        // Phase 2: Process through virtual terminal
                        vt.process(&data);

                        // Phase 3: Update row tracker from screen state
                        tracker.update_from_screen(vt.screen());
                    }
                    None => break,
                }
            }
        }
    }

    // Final harvest + flush
    // Give a moment for any remaining rows to stabilize
    if tracker.has_pending() {
        tokio::time::sleep(Duration::from_millis(STABILIZATION_MS + 100)).await;
        let stable_lines = tracker.harvest_stable(filter.as_ref());
        if !stable_lines.is_empty() {
            for line in &stable_lines {
                buffer.push_str(line);
                buffer.push('\n');
            }
        }
    }
    if !buffer.is_empty() {
        flush_buffer(
            &mut buffer,
            &network,
            &token,
            chat_id,
            &session_id,
            &app,
            &mut logger,
            &mut diag,
            false,
        )
        .await;
    }
}

#[cfg(test)]
mod tests;
