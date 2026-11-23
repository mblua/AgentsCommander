use super::*;

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use super::super::ExitEvents;
use crate::pty::manager::PtyManager;
use crate::session::context_alerts::ContextAlertMonitor;

#[derive(Clone, Default)]
struct PausedHost {
    elapsed: Arc<Mutex<Option<Duration>>>,
}

impl ExitEvents for PausedHost {
    fn terminal_attached(&self, _session_id: String) {
        unreachable!("final persist uses only block_on")
    }
}

impl ExitHost for PausedHost {
    fn quit_window_gone(&self, _label: &str) {
        unreachable!("final persist uses only block_on")
    }
    fn optional_pty(&self) -> Option<Arc<Mutex<PtyManager>>> {
        unreachable!("final persist uses only block_on")
    }
    fn required_pty(&self) -> Arc<Mutex<PtyManager>> {
        unreachable!("final persist uses only block_on")
    }
    fn context_alert_monitor(&self) -> Option<Arc<ContextAlertMonitor>> {
        unreachable!("final persist uses only block_on")
    }
    fn destroy_main_dependents(&self) {
        unreachable!("final persist uses only block_on")
    }
    fn overlay_destroyed(&self, _label: String) {
        unreachable!("final persist uses only block_on")
    }
    fn spawn_background<F: Future<Output = ()> + Send + 'static>(&self, _future: F) {
        unreachable!("final persist uses only block_on")
    }
    fn block_on<F: Future>(&self, future: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(async {
            let start = tokio::time::Instant::now();
            let out = future.await;
            *self.elapsed.lock().unwrap() = Some(start.elapsed());
            out
        })
    }
}

fn new_lines(before: usize) -> Vec<String> {
    crate::logging::test_tee_snapshot()[before..].to_vec()
}

#[test]
fn run_final_persist_times_out_and_logs_step() {
    crate::logging::test_install_logger();
    let before = crate::logging::test_tee_snapshot().len();
    let host = PausedHost::default();
    let outcome = run_final_persist(&host, std::future::pending::<()>(), Duration::from_secs(37));
    assert_eq!(outcome, FinalPersistOutcome::TimedOut);
    assert_eq!(*host.elapsed.lock().unwrap(), Some(Duration::from_secs(37)));
    let errors: Vec<String> = new_lines(before)
        .into_iter()
        .filter(|l| l.starts_with("ERROR ") && l.contains("within 37s"))
        .filter(|l| l.contains("step=persist_current_state"))
        .collect();
    assert_eq!(errors.len(), 1, "want one timeout error line: {errors:?}");
}

#[test]
fn run_final_persist_completes_normally() {
    crate::logging::test_install_logger();
    let before = crate::logging::test_tee_snapshot().len();
    let host = PausedHost::default();
    let flag = Arc::new(AtomicBool::new(false));
    let flag_in = flag.clone();
    let persist = async move { flag_in.store(true, SeqCst) };
    let outcome = run_final_persist(&host, persist, Duration::from_secs(41));
    assert_eq!(outcome, FinalPersistOutcome::Persisted);
    assert!(flag.load(SeqCst));
    assert_eq!(*host.elapsed.lock().unwrap(), Some(Duration::ZERO));
    let lines = new_lines(before);
    let msg = "Session state persisted, process exiting";
    let info = lines
        .iter()
        .any(|l| l.starts_with("INFO ") && l.contains(msg));
    assert!(info, "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("within 41s")), "{lines:?}");
}
