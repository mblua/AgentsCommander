use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) const PAUSE_DIR_ENV: &str = "AC_2713_MIGRATION_PAUSE_DIR";
pub(crate) const PAUSE_STAGE_ENV: &str = "AC_2713_MIGRATION_PAUSE_STAGE";
pub(crate) const READY_FILE: &str = "paused.ready";
pub(crate) const RELEASE_FILE: &str = "paused.release";
const BOUND: Duration = Duration::from_secs(60);

type Slot = (mpsc::Sender<()>, mpsc::Receiver<()>);

fn registry() -> &'static Mutex<HashMap<(String, PathBuf), Slot>> {
    static REGISTRY: OnceLock<Mutex<HashMap<(String, PathBuf), Slot>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One armed pause: `reached` fires when the engine stops at the stage, and
/// it resumes on `release`.
pub(crate) struct Armed {
    pub(crate) reached: mpsc::Receiver<()>,
    pub(crate) release: mpsc::Sender<()>,
}

pub(crate) fn arm(stage: &str, dir: &Path) -> Armed {
    let (reached_tx, reached) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    registry().lock().expect("pause registry").insert(
        (stage.to_string(), dir.to_path_buf()),
        (reached_tx, release_rx),
    );
    Armed { reached, release }
}

pub(crate) fn hook(stage: &str, dir: &Path) {
    let slot = registry()
        .lock()
        .expect("pause registry")
        .remove(&(stage.to_string(), dir.to_path_buf()));
    if let Some((reached, release)) = slot {
        let _ = reached.send(());
        release
            .recv_timeout(BOUND)
            .expect("in-process pause released within its 60 s bound");
        return;
    }
    let (Some(rendezvous), Some(armed_stage)) = (
        std::env::var_os(PAUSE_DIR_ENV),
        std::env::var_os(PAUSE_STAGE_ENV),
    ) else {
        return;
    };
    if armed_stage != stage {
        return;
    }
    let rendezvous = PathBuf::from(rendezvous);
    std::fs::write(rendezvous.join(READY_FILE), b"ready").expect("announce pause");
    let started = Instant::now();
    while !rendezvous.join(RELEASE_FILE).exists() {
        assert!(
            started.elapsed() < BOUND,
            "cross-process pause exceeded 60 s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
