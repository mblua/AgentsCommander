use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

const IDLE_THRESHOLD: Duration = Duration::from_millis(700);
const CHECK_INTERVAL: Duration = Duration::from_millis(200);
/// Grace period after a resize: PTY output during this window is prompt
/// repaint noise, not real agent activity. Suppresses false busy→idle cycles.
const RESIZE_GRACE: Duration = Duration::from_millis(500);

type Callback = Arc<dyn Fn(Uuid) + Send + Sync>;

pub struct IdleDetector {
    activity: Arc<Mutex<HashMap<Uuid, Instant>>>,
    idle_set: Arc<Mutex<HashSet<Uuid>>>,
    resize_grace: Arc<Mutex<HashMap<Uuid, Instant>>>,
    on_idle: Callback,
    on_busy: Callback,
}

impl IdleDetector {
    pub fn new(
        on_idle: impl Fn(Uuid) + Send + Sync + 'static,
        on_busy: impl Fn(Uuid) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            activity: Arc::new(Mutex::new(HashMap::new())),
            idle_set: Arc::new(Mutex::new(HashSet::new())),
            resize_grace: Arc::new(Mutex::new(HashMap::new())),
            on_idle: Arc::new(on_idle),
            on_busy: Arc::new(on_busy),
        })
    }

    /// Mark that a resize just happened for this session.
    /// PTY output within RESIZE_GRACE will be ignored (prompt repaint noise).
    pub fn record_resize(&self, session_id: Uuid) {
        self.resize_grace.lock().unwrap().insert(session_id, Instant::now());
    }

    /// Record PTY activity for a session. If the session was idle,
    /// fires on_busy and removes it from idle_set.
    /// Ignores activity within the resize grace period.
    pub fn record_activity(&self, session_id: Uuid) {
        // Suppress activity caused by resize prompt repaint
        if let Some(&last_resize) = self.resize_grace.lock().unwrap().get(&session_id) {
            if last_resize.elapsed() < RESIZE_GRACE {
                return;
            }
        }
        self.activity.lock().unwrap().insert(session_id, Instant::now());
        let was_idle = self.idle_set.lock().unwrap().remove(&session_id);
        if was_idle {
            (self.on_busy)(session_id);
        }
    }

    /// Remove a session from tracking (called on session destroy).
    pub fn remove_session(&self, session_id: Uuid) {
        self.activity.lock().unwrap().remove(&session_id);
        self.idle_set.lock().unwrap().remove(&session_id);
        self.resize_grace.lock().unwrap().remove(&session_id);
    }

    /// Start the watcher thread that polls for idle transitions.
    pub fn start(self: &Arc<Self>) {
        let detector = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(CHECK_INTERVAL);

                let now = Instant::now();
                let activity = detector.activity.lock().unwrap();
                let mut idle_set = detector.idle_set.lock().unwrap();

                for (&session_id, &last_seen) in activity.iter() {
                    if now.duration_since(last_seen) > IDLE_THRESHOLD
                        && !idle_set.contains(&session_id)
                    {
                        idle_set.insert(session_id);
                        (detector.on_idle)(session_id);
                    }
                }
            }
        });
    }
}
