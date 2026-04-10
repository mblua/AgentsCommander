use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

const WATCHER_INTERVAL: Duration = Duration::from_secs(5);

type Callback = Arc<dyn Fn(Uuid) + Send + Sync>;

pub struct CompletionTracker {
    state: Arc<Mutex<HashMap<Uuid, SessionCompletionState>>>,
    phrase: Arc<Mutex<String>>,
    hung_timeout: Arc<Mutex<Duration>>,
    on_completed: Callback,
    on_hung: Callback,
}

struct SessionCompletionState {
    phrase_detected: bool,
    idle_since: Option<Instant>,
    status: CompletionStatus,
    hung_notified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompletionStatus {
    Working,
    Completed,
    Hung,
}

impl CompletionTracker {
    pub fn new(
        phrase: String,
        hung_timeout_secs: u64,
        on_completed: impl Fn(Uuid) + Send + Sync + 'static,
        on_hung: impl Fn(Uuid) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            phrase: Arc::new(Mutex::new(phrase)),
            hung_timeout: Arc::new(Mutex::new(Duration::from_secs(hung_timeout_secs))),
            on_completed: Arc::new(on_completed),
            on_hung: Arc::new(on_hung),
        })
    }

    /// Check if text contains the completion phrase. Called from PTY read loop.
    pub fn scan_phrase(&self, text: &str) -> bool {
        let phrase = self.phrase.lock().unwrap();
        !phrase.is_empty() && text.contains(phrase.as_str())
    }

    /// Called from PTY read loop when output contains the completion phrase.
    pub fn record_phrase_detected(&self, session_id: Uuid) {
        let mut state = self.state.lock().unwrap();
        let entry = state.entry(session_id).or_default();
        entry.phrase_detected = true;
        log::info!("[completion] phrase detected for {}", &session_id.to_string()[..8]);
    }

    /// Called when a message is injected into a session (resets all tracking).
    pub fn reset(&self, session_id: Uuid) {
        let mut state = self.state.lock().unwrap();
        if let Some(s) = state.get_mut(&session_id) {
            s.phrase_detected = false;
            s.idle_since = None;
            s.status = CompletionStatus::Working;
            s.hung_notified = false;
            log::info!("[completion] reset for {}", &session_id.to_string()[..8]);
        }
    }

    /// Called when idle detector fires session_idle.
    pub fn mark_idle(&self, session_id: Uuid) {
        let mut state = self.state.lock().unwrap();
        let entry = state.entry(session_id).or_default();
        if entry.idle_since.is_none() {
            entry.idle_since = Some(Instant::now());
        }
    }

    /// Called when idle detector fires session_busy.
    /// Resets idle_since and hung_notified, but does NOT reset Completed status.
    /// Only reset() on message injection transitions back to Working.
    pub fn mark_busy(&self, session_id: Uuid) {
        let mut state = self.state.lock().unwrap();
        if let Some(s) = state.get_mut(&session_id) {
            s.idle_since = None;
            s.hung_notified = false;
        }
    }

    /// Remove session from tracking.
    pub fn remove_session(&self, session_id: Uuid) {
        self.state.lock().unwrap().remove(&session_id);
    }

    /// Query current status for a session.
    pub fn get_status(&self, session_id: Uuid) -> CompletionStatus {
        self.state.lock().unwrap()
            .get(&session_id)
            .map(|s| s.status)
            .unwrap_or(CompletionStatus::Working)
    }

    /// Update phrase and timeout from settings (hot reload).
    pub fn update_config(&self, phrase: String, hung_timeout_secs: u64) {
        *self.phrase.lock().unwrap() = phrase;
        *self.hung_timeout.lock().unwrap() = Duration::from_secs(hung_timeout_secs);
    }

    /// Start background watcher thread.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let tracker = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(WATCHER_INTERVAL);

                if shutdown.is_cancelled() {
                    log::info!("[CompletionTracker] Shutdown signal received, stopping");
                    break;
                }

                let hung_timeout = *tracker.hung_timeout.lock().unwrap();

                // Collect events under lock, fire callbacks after unlocking
                let mut completed_ids = Vec::new();
                let mut hung_ids = Vec::new();

                {
                    let now = Instant::now();
                    let mut state = tracker.state.lock().unwrap();

                    for (&session_id, s) in state.iter_mut() {
                        if s.status == CompletionStatus::Working && s.phrase_detected {
                            if let Some(idle_since) = s.idle_since {
                                let _ = idle_since; // phrase + idle = completed
                                s.status = CompletionStatus::Completed;
                                completed_ids.push(session_id);
                            }
                        }

                        if s.status == CompletionStatus::Working && !s.phrase_detected {
                            if let Some(idle_since) = s.idle_since {
                                if let Some(elapsed) = now.checked_duration_since(idle_since) {
                                    if elapsed > hung_timeout && !s.hung_notified {
                                        s.status = CompletionStatus::Hung;
                                        s.hung_notified = true;
                                        hung_ids.push(session_id);
                                    }
                                }
                            }
                        }
                    }
                }

                // Fire callbacks outside the lock
                for id in completed_ids {
                    log::info!("[completion] session {} completed", &id.to_string()[..8]);
                    (tracker.on_completed)(id);
                }
                for id in hung_ids {
                    log::warn!("[completion] session {} appears hung", &id.to_string()[..8]);
                    (tracker.on_hung)(id);
                }
            }
        });
    }
}

impl Default for SessionCompletionState {
    fn default() -> Self {
        Self {
            phrase_detected: false,
            idle_since: None,
            status: CompletionStatus::Working,
            hung_notified: false,
        }
    }
}
