use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

const WATCHER_INTERVAL: Duration = Duration::from_secs(5);

type CompletedCallback = Arc<dyn Fn(Uuid, String) + Send + Sync>;
type HungCallback = Arc<dyn Fn(Uuid, String, u64) + Send + Sync>;

pub struct CompletionTracker {
    state: Arc<Mutex<HashMap<Uuid, SessionCompletionState>>>,
    phrase: String,
    hung_timeout: Duration,
    on_completed: CompletedCallback,
    on_hung: HungCallback,
}

struct SessionCompletionState {
    name: String,
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
        on_completed: impl Fn(Uuid, String) + Send + Sync + 'static,
        on_hung: impl Fn(Uuid, String, u64) + Send + Sync + 'static,
    ) -> Arc<Self> {
        log::info!(
            "[completion] initialized: phrase={:?}, hung_timeout={}s",
            phrase, hung_timeout_secs
        );
        Arc::new(Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            phrase,
            hung_timeout: Duration::from_secs(hung_timeout_secs),
            on_completed: Arc::new(on_completed),
            on_hung: Arc::new(on_hung),
        })
    }

    /// Register a Claude session for completion tracking.
    /// Only registered sessions are monitored for phrase detection and hung state.
    pub fn register_session(&self, session_id: Uuid, name: String) {
        let mut state = self.state.lock().unwrap();
        state.entry(session_id).or_insert_with(|| SessionCompletionState {
            name,
            phrase_detected: false,
            idle_since: None,
            status: CompletionStatus::Working,
            hung_notified: false,
        });
        log::info!("[completion] registered session {}", &session_id.to_string()[..8]);
    }

    /// Check if text contains the completion phrase. Called from PTY read loop.
    pub fn scan_phrase(&self, text: &str) -> bool {
        !self.phrase.is_empty() && text.contains(self.phrase.as_str())
    }

    /// Called from PTY read loop when output contains the completion phrase.
    /// Only acts on registered sessions.
    pub fn record_phrase_detected(&self, session_id: Uuid) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.get_mut(&session_id) {
            entry.phrase_detected = true;
            log::info!("[completion] phrase detected for {}", &session_id.to_string()[..8]);
        }
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
    /// Only acts on registered (Claude) sessions.
    pub fn mark_idle(&self, session_id: Uuid) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.get_mut(&session_id) {
            if entry.idle_since.is_none() {
                entry.idle_since = Some(Instant::now());
            }
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

                // Skip if hung timeout is disabled (0)
                if tracker.hung_timeout.is_zero() {
                    continue;
                }

                // Collect events under lock, fire callbacks after unlocking
                let mut completed: Vec<(Uuid, String)> = Vec::new();
                let mut hung: Vec<(Uuid, String, u64)> = Vec::new();

                {
                    let now = Instant::now();
                    let mut state = tracker.state.lock().unwrap();

                    for (&session_id, s) in state.iter_mut() {
                        if s.status == CompletionStatus::Working && s.phrase_detected && s.idle_since.is_some() {
                            s.status = CompletionStatus::Completed;
                            completed.push((session_id, s.name.clone()));
                        }

                        if s.status == CompletionStatus::Working && !s.phrase_detected {
                            if let Some(idle_since) = s.idle_since {
                                if let Some(elapsed) = now.checked_duration_since(idle_since) {
                                    if elapsed > tracker.hung_timeout && !s.hung_notified {
                                        let idle_minutes = elapsed.as_secs() / 60;
                                        s.status = CompletionStatus::Hung;
                                        s.hung_notified = true;
                                        hung.push((session_id, s.name.clone(), idle_minutes));
                                    }
                                }
                            }
                        }
                    }
                }

                // Fire callbacks outside the lock
                for (id, name) in completed {
                    log::info!("[completion] session {} ({}) completed", &id.to_string()[..8], name);
                    (tracker.on_completed)(id, name);
                }
                for (id, name, idle_minutes) in hung {
                    log::warn!("[completion] session {} ({}) appears hung (idle={}min, timeout={}s)", &id.to_string()[..8], name, idle_minutes, tracker.hung_timeout.as_secs());
                    (tracker.on_hung)(id, name, idle_minutes);
                }
            }
        });
    }
}

impl Default for SessionCompletionState {
    fn default() -> Self {
        Self {
            name: String::new(),
            phrase_detected: false,
            idle_since: None,
            status: CompletionStatus::Working,
            hung_notified: false,
        }
    }
}
