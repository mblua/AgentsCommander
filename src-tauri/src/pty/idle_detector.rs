use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

const IDLE_THRESHOLD: Duration = Duration::from_millis(2500);
const CHECK_INTERVAL: Duration = Duration::from_millis(500);
/// Grace period after a resize: PTY output during this window is prompt
/// repaint noise, not real agent activity. Suppresses false busy→idle cycles.
/// Must be >= IDLE_THRESHOLD to prevent resize repaint from triggering a
/// false busy→idle transition that sets pendingReview.
const RESIZE_GRACE: Duration = Duration::from_millis(3000);

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
        log::info!("[idle] RESIZE recorded for {}", &session_id.to_string()[..8]);
        self.resize_grace.lock().unwrap().insert(session_id, Instant::now());
    }

    /// Record PTY activity (with byte count for diagnostics).
    pub fn record_activity_with_bytes(&self, session_id: Uuid, byte_count: usize) {
        let sid = &session_id.to_string()[..8];
        // Suppress activity caused by resize prompt repaint
        if let Some(&last_resize) = self.resize_grace.lock().unwrap().get(&session_id) {
            let elapsed = last_resize.elapsed();
            if elapsed < RESIZE_GRACE {
                log::info!("[idle] SUPPRESSED {} ({} bytes, {}ms after resize)", sid, byte_count, elapsed.as_millis());
                return;
            }
        }
        let was_idle = {
            self.activity.lock().unwrap().insert(session_id, Instant::now());
            self.idle_set.lock().unwrap().remove(&session_id)
        };
        if was_idle {
            log::info!("[idle] BUSY {} ({} bytes, was idle → now busy)", sid, byte_count);
            (self.on_busy)(session_id);
        }
    }

    /// Record PTY activity for a session (backwards-compatible wrapper).
    pub fn record_activity(&self, session_id: Uuid) {
        self.record_activity_with_bytes(session_id, 0);
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
                        log::info!("[idle] IDLE {} ({}ms since last activity)", &session_id.to_string()[..8], now.duration_since(last_seen).as_millis());
                        (detector.on_idle)(session_id);
                    }
                }
            }
        });
    }
}
