//! #2774 Wayland portal capture tests (no D-Bus: the pick result is injected).

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedSender};

use super::super::portal::{cached_result, HostRecord, Presses};
use super::super::wayland_shortcut::press_may_cancel;
use super::*;
use crate::screenshot::{
    OverlayWindowSpec, ScreenshotEvents, ScreenshotHotkeyOwner, ScreenshotHotkeyState,
    ShortcutRegistrar,
};

type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

#[derive(Clone)]
struct WaylandHost {
    capture: ScreenshotCaptureState,
    hotkey: ScreenshotHotkeyState,
    display: DisplayEnvSnapshot,
    log: Arc<Mutex<Vec<String>>>,
    messages: Arc<Mutex<Vec<String>>>,
    spawned: Arc<Mutex<Vec<BoxFuture>>>,
}

impl WaylandHost {
    fn new(display: DisplayEnvSnapshot, state: ScreenshotCaptureLifecycle) -> Self {
        Self {
            capture: Arc::new(tokio::sync::Mutex::new(state)),
            hotkey: Arc::new(Mutex::new(ScreenshotHotkeyRuntime::default())),
            display,
            log: Arc::default(),
            messages: Arc::default(),
            spawned: Arc::default(),
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn push(&self, entry: &str) {
        self.log.lock().unwrap().push(entry.to_string());
    }

    async fn drain(&self) {
        let futures: Vec<BoxFuture> = self.spawned.lock().unwrap().drain(..).collect();
        for future in futures {
            future.await;
        }
    }

    async fn state_id(&self) -> Option<Uuid> {
        match &*self.capture.lock().await {
            ScreenshotCaptureLifecycle::Idle => None,
            ScreenshotCaptureLifecycle::Starting(s) => Some(s.id),
            other => panic!("unexpected state {}", lifecycle_name(other)),
        }
    }
}

fn lifecycle_name(state: &ScreenshotCaptureLifecycle) -> &'static str {
    match state {
        ScreenshotCaptureLifecycle::Idle => "Idle",
        ScreenshotCaptureLifecycle::Starting(_) => "Starting",
        ScreenshotCaptureLifecycle::Active(_) => "Active",
        ScreenshotCaptureLifecycle::Finishing(_) => "Finishing",
    }
}

fn wayland() -> DisplayEnvSnapshot {
    DisplayEnvSnapshot {
        xdg_session_type: Some("wayland".to_string()),
        wayland_display: Some("wayland-1".to_string()),
        wayland_socket: None,
        default_socket_present: true,
    }
}

fn x11() -> DisplayEnvSnapshot {
    DisplayEnvSnapshot {
        xdg_session_type: Some("x11".to_string()),
        wayland_display: None,
        wayland_socket: None,
        default_socket_present: false,
    }
}

impl ScreenshotHotkeyOwner for WaylandHost {
    fn hotkey_state(&self) -> ScreenshotHotkeyState {
        Arc::clone(&self.hotkey)
    }
}

impl ScreenshotEvents for WaylandHost {
    fn capture_saved(&self, _result: ScreenshotCaptureResult) {
        self.push("saved");
    }

    fn capture_failed(&self, event: ScreenshotCaptureEvent) {
        self.messages.lock().unwrap().push(event.message);
        self.push("failed");
    }

    fn hotkey_status_changed(&self, status: ScreenshotHotkeyStatus) {
        let error = status.error.unwrap_or_default();
        self.push(&format!("status:{}:{error}", status.registered));
    }
}

impl ScreenshotHost for WaylandHost {
    fn capture_state(&self) -> ScreenshotCaptureState {
        Arc::clone(&self.capture)
    }

    fn selection(&self) -> crate::session::selection::SelectionCoordinator {
        panic!("wayland test must not access selection")
    }

    fn sessions(&self) -> Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>> {
        panic!("wayland test must not access sessions")
    }

    fn available_monitors(&self) -> Vec<MonitorGeometry> {
        panic!("wayland test must not enumerate monitors")
    }

    fn with_shortcuts<T, F: FnOnce(&dyn ShortcutRegistrar) -> T>(&self, _callback: F) -> T {
        panic!("wayland test must not access shortcuts")
    }

    fn display_snapshot(&self) -> DisplayEnvSnapshot {
        self.display.clone()
    }

    fn open_overlay(&self, _spec: &OverlayWindowSpec) -> Result<(), String> {
        panic!("wayland test must not create overlays")
    }

    fn destroy_capture_overlays(&self, _capture_id: Uuid) -> usize {
        0
    }

    fn copy_path(&self, _path: String) -> Result<(), String> {
        self.push("clipboard");
        Ok(())
    }

    fn focus_capture_failure(&self) {
        self.push("focus");
    }

    fn spawn_background(&self, future: BoxFuture) {
        self.spawned.lock().unwrap().push(future);
    }
}

fn starting(root: &Path) -> ScreenshotCaptureStarting {
    ScreenshotCaptureStarting {
        id: Uuid::new_v4(),
        target_session_id: Uuid::new_v4(),
        target_session_name: "shot".to_string(),
        target_directory: root.to_path_buf(),
        created_at: Utc::now(),
    }
}

fn png(path: &Path, w: u32, h: u32) {
    image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]))
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

/// A Wayland host in `Starting` for a fresh capture id.
fn host_starting(root: &Path) -> (WaylandHost, Uuid) {
    let s = starting(root);
    let id = s.id;
    let host = WaylandHost::new(wayland(), ScreenshotCaptureLifecycle::Starting(s));
    (host, id)
}

fn screenshot_files(root: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(root.join("screenshots")) {
        Ok(dir) => dir.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn cancel_returns_to_idle_silently() {
    let (host, id) = host_starting(Path::new("unused"));
    let out = finish_portal_pick(&host, id, Ok(None), 1024).await;
    assert!(matches!(out, Ok(BeginOutcome::Cancelled)));
    assert_eq!(host.state_id().await, None);
    assert!(host.log().is_empty());
}

#[tokio::test]
async fn portal_error_returns_to_idle_and_err() {
    let (host, id) = host_starting(Path::new("unused"));
    let out = finish_portal_pick(&host, id, Err("x".to_string()), 1024).await;
    let Err(message) = out else {
        panic!("expected Err")
    };
    assert!(message.contains('x'), "{message}");
    assert_eq!(host.state_id().await, None);
}

#[tokio::test]
async fn valid_png_is_saved() {
    let (_temp, root) = super::screenshot_replica_fixture();
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("portal.png");
    png(&source, 4, 4);
    let before = std::fs::read(&source).unwrap();
    let (host, id) = host_starting(&root);

    let out = finish_portal_pick(&host, id, Ok(Some(source.clone())), 1024 * 1024).await;

    assert!(matches!(out, Ok(BeginOutcome::Started)));
    let files = screenshot_files(&root);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].extension().unwrap(), "png");
    assert_eq!(host.log(), ["clipboard", "saved"]);
    assert_eq!(host.state_id().await, None);
    assert_eq!(std::fs::read(&source).unwrap(), before);
}

#[tokio::test]
async fn superseded_pick_saves_nothing() {
    let (_temp, root) = super::screenshot_replica_fixture();
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("portal.png");
    png(&source, 4, 4);
    let (host, other_id) = host_starting(&root);

    let out = finish_portal_pick(&host, Uuid::new_v4(), Ok(Some(source)), 1024 * 1024).await;

    assert!(matches!(out, Ok(BeginOutcome::Busy(None))));
    assert!(!root.join("screenshots").exists());
    assert_eq!(host.state_id().await, Some(other_id));
}

#[tokio::test]
async fn unreadable_sources_fail_and_focus() {
    let (_temp, root) = super::screenshot_replica_fixture();
    let source_dir = tempfile::tempdir().unwrap();
    let dir = source_dir.path();
    let as_dir = dir.join("a-directory");
    std::fs::create_dir(&as_dir).unwrap();
    let garbage = dir.join("garbage.png");
    std::fs::write(&garbage, b"not an image").unwrap();
    let thin = dir.join("thin.png");
    png(&thin, 1, 5);
    let big = dir.join("big.png");
    png(&big, 4, 4);
    // User-granted exception (#2774 P02 only): a symlink to a valid PNG.
    let link = dir.join("link.png");
    std::os::unix::fs::symlink(&big, &link).unwrap();

    let cases: [(PathBuf, u64, &str); 5] = [
        (link, 1024 * 1024, "not a regular file"),
        (as_dir, 1024 * 1024, "not a regular file"),
        (garbage, 1024 * 1024, "not a readable image"),
        (thin, 1024 * 1024, "Selection is too small"),
        (big, 10, "too large"),
    ];
    for (source, max_bytes, expected) in cases {
        let (host, id) = host_starting(&root);
        let out = finish_portal_pick(&host, id, Ok(Some(source)), max_bytes).await;
        assert!(matches!(out, Ok(BeginOutcome::Started)), "{expected}");
        assert!(host
            .log()
            .ends_with(&["failed".to_string(), "focus".to_string()]));
        let messages = host.messages.lock().unwrap().clone();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains(expected), "{messages:?} vs {expected}");
        assert_eq!(host.state_id().await, None);
        assert!(screenshot_files(&root).is_empty(), "{expected}");
    }
}

#[test]
fn press_may_cancel_rule() {
    let id = Uuid::new_v4();
    assert_eq!(press_may_cancel(true, Some(id)), None);
    assert_eq!(press_may_cancel(false, Some(id)), Some(id));
    assert_eq!(press_may_cancel(true, None), None);
}

#[tokio::test]
async fn repeat_press_on_wayland_never_cancels() {
    let (host, id) = host_starting(Path::new("unused"));
    begin_capture_from_hotkey(host.clone());
    host.drain().await;
    assert_eq!(host.state_id().await, Some(id));
    assert!(host.log().is_empty());
}

#[tokio::test]
async fn repeat_press_on_x11_still_cancels() {
    let s = starting(Path::new("unused"));
    let host = WaylandHost::new(x11(), ScreenshotCaptureLifecycle::Starting(s));
    begin_capture_from_hotkey(host.clone());
    host.drain().await;
    assert_eq!(host.state_id().await, None);
}

#[test]
fn host_record_is_reused_only_for_the_same_owner() {
    let ok = HostRecord {
        owner: ":1.5".to_string(),
        result: Ok(()),
    };
    let failed = HostRecord {
        owner: ":1.5".to_string(),
        result: Err("x".to_string()),
    };
    assert_eq!(cached_result(None, ":1.5"), None);
    assert_eq!(
        cached_result(Some(&ok), ":1.5"),
        Some(Ok(":1.5".to_string()))
    );
    assert_eq!(cached_result(Some(&ok), ":1.9"), None);
    assert_eq!(
        cached_result(Some(&failed), ":1.5"),
        Some(Err("x".to_string()))
    );
    assert_eq!(cached_result(Some(&failed), ":1.9"), None);
}

// ── #2774 P03 shortcut registration ────────────────────────────────────────

#[derive(Clone, Default)]
struct FakeBackend {
    results: Arc<Mutex<VecDeque<Result<Presses, String>>>>,
    binds: Arc<AtomicUsize>,
    bind_entered: Arc<tokio::sync::Notify>,
    bind_gate: Option<tokio::sync::watch::Receiver<bool>>,
    outcomes: Arc<Mutex<VecDeque<hyprland_bind::BindOutcome>>>,
    applies: Arc<Mutex<Vec<(bool, char)>>>,
    apply_entered: Arc<tokio::sync::Notify>,
    apply_gate: Option<tokio::sync::watch::Receiver<bool>>,
}

impl FakeBackend {
    /// Script one `Ok` bind; the returned sender feeds (or, dropped, ends) its presses.
    fn script_ok(&self) -> UnboundedSender<()> {
        let (tx, rx) = unbounded();
        self.results.lock().unwrap().push_back(Ok(rx));
        tx
    }

    fn script_err(&self, e: &str) {
        self.results.lock().unwrap().push_back(Err(e.to_string()));
    }

    fn binds(&self) -> usize {
        self.binds.load(Ordering::SeqCst)
    }

    fn applies(&self) -> Vec<(bool, char)> {
        self.applies.lock().unwrap().clone()
    }
}

impl WaylandBackend for FakeBackend {
    async fn pick_region(&self) -> Result<Option<PathBuf>, String> {
        panic!("registration test must not pick a region")
    }

    async fn bind_shortcut(&self, _ids: &ShortcutIds, _trigger: &str) -> Result<Presses, String> {
        self.bind_entered.notify_one();
        if let Some(gate) = &self.bind_gate {
            let mut gate = gate.clone();
            let _ = gate.wait_for(|open| *open).await;
        }
        self.binds.fetch_add(1, Ordering::SeqCst);
        let next = self.results.lock().unwrap().pop_front();
        next.expect("unscripted bind_shortcut")
    }

    /// Unscripted -> `NotHyprland` (P03 behavior).
    async fn apply_hyprland(
        &self,
        enabled: bool,
        key: char,
        _ids: &ShortcutIds,
    ) -> hyprland_bind::BindOutcome {
        self.apply_entered.notify_one();
        if let Some(gate) = &self.apply_gate {
            let mut gate = gate.clone();
            let _ = gate.wait_for(|open| *open).await;
        }
        self.applies.lock().unwrap().push((enabled, key));
        let next = self.outcomes.lock().unwrap().pop_front();
        next.unwrap_or(hyprland_bind::BindOutcome::NotHyprland)
    }
}

fn idle_wayland_host() -> WaylandHost {
    WaylandHost::new(wayland(), ScreenshotCaptureLifecycle::Idle)
}

async fn register(host: &WaylandHost, backend: &FakeBackend, generation: u64) {
    register_enabled(host, backend, generation, false).await;
}

async fn register_enabled(host: &WaylandHost, backend: &FakeBackend, r#gen: u64, on: bool) {
    let token = "1a2b3c4d".to_string();
    let (h, b) = (host.clone(), backend.clone());
    register_wayland(h, b, r#gen, token, "Ctrl+Q".into(), 'Q', on).await;
}

async fn binding(host: &WaylandHost) -> u64 {
    let portal = host.hotkey.lock().unwrap().wayland.clone();
    let binding = portal.lock().await.binding;
    binding
}

fn status(host: &WaylandHost) -> ScreenshotHotkeyStatus {
    host.hotkey.lock().unwrap().status.clone()
}

/// `ScreenshotHotkeyStatus` has no `PartialEq`; compare its fields.
fn fields(st: ScreenshotHotkeyStatus) -> (String, bool, Option<String>) {
    (st.configured, st.registered, st.error)
}

fn ended_entry() -> String {
    format!("status:false:{PORTAL_SHORTCUT_ENDED}")
}

#[tokio::test]
async fn bind_ok_sets_registered_and_emits_once() {
    let (host, backend) = (idle_wayland_host(), FakeBackend::default());
    let _tx = backend.script_ok();
    let generation = begin_wayland_registration(&host);
    register(&host, &backend, generation).await;
    let st = status(&host);
    assert_eq!(
        (st.configured.as_str(), st.registered, st.error),
        ("Ctrl+Q", true, None)
    );
    assert_eq!(host.log(), ["status:true:"]);
    assert_ne!(binding(&host).await, 0);
    assert_eq!(host.spawned.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn bind_err_reports_and_retries_next_time() {
    let (host, backend) = (idle_wayland_host(), FakeBackend::default());
    backend.script_err("no portal");
    let generation = begin_wayland_registration(&host);
    register(&host, &backend, generation).await;
    let error = status(&host).error.expect("an error status");
    assert!(error.starts_with(WAYLAND_UNSUPPORTED), "{error}");
    assert!(error.contains("no portal"), "{error}");
    assert_eq!(binding(&host).await, 0);
    backend.script_err("no portal");
    let generation = begin_wayland_registration(&host);
    register(&host, &backend, generation).await;
    assert_eq!(backend.binds(), 2);
}

#[tokio::test]
async fn bind_happens_once() {
    let (host, backend) = (idle_wayland_host(), FakeBackend::default());
    let _tx = backend.script_ok();
    for _ in 0..2 {
        let generation = begin_wayland_registration(&host);
        register(&host, &backend, generation).await;
    }
    assert_eq!(backend.binds(), 1);
    assert_eq!(host.log(), ["status:true:", "status:true:"]);
}

#[tokio::test]
async fn stale_request_writes_nothing() {
    let (host, backend) = (idle_wayland_host(), FakeBackend::default());
    let _tx = backend.script_ok();
    let g1 = begin_wayland_registration(&host);
    let g2 = begin_wayland_registration(&host);
    let before = fields(status(&host));
    register(&host, &backend, g1).await;
    assert_eq!(backend.binds(), 0);
    assert_eq!(fields(status(&host)), before);
    assert!(host.log().is_empty());
    register(&host, &backend, g2).await;
    assert!(status(&host).registered);
}

#[tokio::test]
async fn older_task_holding_the_lock_loses() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let host = idle_wayland_host();
        let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
        let backend = FakeBackend {
            bind_gate: Some(gate_rx),
            ..FakeBackend::default()
        };
        let _tx = backend.script_ok();
        let _tx2 = backend.script_ok();
        let g1 = begin_wayland_registration(&host);
        let (h, b) = (host.clone(), backend.clone());
        let first = tokio::spawn(async move { register_enabled(&h, &b, g1, true).await });
        backend.bind_entered.notified().await;
        let g2 = begin_wayland_registration(&host);
        let (h, b) = (host.clone(), backend.clone());
        let second = tokio::spawn(async move { register(&h, &b, g2).await });
        tokio::task::yield_now().await;
        gate_tx.send(true).unwrap();
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(backend.binds(), 1);
        assert_eq!(host.log(), ["status:true:"]);
        assert_eq!(backend.applies(), [(false, 'Q')]);
    })
    .await
    .expect("ordering test hung");
}

#[tokio::test]
async fn older_task_in_apply_loses() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let host = idle_wayland_host();
        let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
        let backend = FakeBackend {
            apply_gate: Some(gate_rx),
            ..FakeBackend::default()
        };
        let _tx = backend.script_ok();
        let g1 = begin_wayland_registration(&host);
        let (h, b) = (host.clone(), backend.clone());
        let first = tokio::spawn(async move { register_enabled(&h, &b, g1, true).await });
        backend.apply_entered.notified().await;
        let g2 = begin_wayland_registration(&host);
        let (h, b) = (host.clone(), backend.clone());
        let second = tokio::spawn(async move { register(&h, &b, g2).await });
        tokio::task::yield_now().await;
        gate_tx.send(true).unwrap();
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(backend.applies(), [(true, 'Q'), (false, 'Q')]);
        assert_eq!(backend.binds(), 1);
        assert_eq!(host.log(), ["status:true:"]);
    })
    .await
    .expect("ordering test hung");
}

async fn register_with(outcome: hyprland_bind::BindOutcome) -> ScreenshotHotkeyStatus {
    let (host, backend) = (idle_wayland_host(), FakeBackend::default());
    let _tx = backend.script_ok();
    backend.outcomes.lock().unwrap().push_back(outcome);
    let generation = begin_wayland_registration(&host);
    register(&host, &backend, generation).await;
    status(&host)
}

#[tokio::test]
async fn manual_outcome_shows_the_lua_line() {
    let outcome = hyprland_bind::BindOutcome::Manual { reason: None };
    let error = register_with(outcome).await.error.expect("an error status");
    let ids = shortcut_ids("1a2b3c4d");
    let lua = hyprland_bind::lua_line('Q', &bind_ids(&ids));
    assert!(error.contains(&lua), "{error}");
}

#[tokio::test]
async fn applied_outcome_registers() {
    let st = register_with(hyprland_bind::BindOutcome::Applied).await;
    assert!(st.registered);
    assert_eq!(st.error, None);
}

#[tokio::test]
async fn stream_end_reports_and_rebinds() {
    let (host, backend) = (idle_wayland_host(), FakeBackend::default());
    let tx = backend.script_ok();
    let generation = begin_wayland_registration(&host);
    register(&host, &backend, generation).await;
    drop(tx);
    host.drain().await;
    assert_eq!(status(&host).error.as_deref(), Some(PORTAL_SHORTCUT_ENDED));
    assert_eq!(binding(&host).await, 0);
    assert_eq!(host.log().last(), Some(&ended_entry()));
    let _tx = backend.script_ok();
    let generation = begin_wayland_registration(&host);
    register(&host, &backend, generation).await;
    assert_eq!(backend.binds(), 2);
}

#[tokio::test]
async fn stream_end_of_old_binding_is_ignored() {
    let host = idle_wayland_host();
    let portal = host.hotkey.lock().unwrap().wayland.clone();
    portal.lock().await.binding = 8;
    let before = fields(status(&host));
    let (tx, rx) = unbounded::<()>();
    drop(tx);
    forward_presses(host.clone(), portal.clone(), rx, 7, |_h: WaylandHost| {}).await;
    assert_eq!(portal.lock().await.binding, 8);
    assert_eq!(fields(status(&host)), before);
    assert!(host.log().is_empty());
}

#[tokio::test]
async fn presses_are_forwarded() {
    let host = idle_wayland_host();
    let portal = host.hotkey.lock().unwrap().wayland.clone();
    portal.lock().await.binding = 3;
    let (tx, rx) = unbounded();
    tx.unbounded_send(()).unwrap();
    tx.unbounded_send(()).unwrap();
    drop(tx);
    let on_press = |h: WaylandHost| h.push("press");
    forward_presses(host.clone(), portal, rx, 3, on_press).await;
    assert_eq!(
        host.log(),
        ["press".to_string(), "press".to_string(), ended_entry()]
    );
}

#[test]
fn instance_token_is_stable_and_distinct() {
    let is_hex8 = |t: &str| t.len() == 8 && t.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    let a = instance_token(Some(Path::new("/home/u/.agentscommander")));
    assert!(is_hex8(&a), "{a}");
    assert_eq!(
        a,
        instance_token(Some(Path::new("/home/u/.agentscommander")))
    );
    assert_ne!(
        a,
        instance_token(Some(Path::new("/home/u/.agentscommander-dev")))
    );
    assert!(is_hex8(&instance_token(None)));
    assert_eq!(shortcut_ids("1a2b3c4d").shortcut_id, "screenshot-1a2b3c4d");
    let global = shortcut_ids("1a2b3c4d").global;
    assert_eq!(global, "agentscommander:screenshot-1a2b3c4d");
}

#[tokio::test]
async fn store_ignores_a_stale_generation() {
    let host = idle_wayland_host();
    let generation = begin_wayland_registration(&host);
    let _newer = begin_wayland_registration(&host);
    let before = fields(status(&host));
    assert!(!store_wayland_status(&host, generation, "Ctrl+Q", Ok(())));
    assert_eq!(fields(status(&host)), before);
    assert!(host.log().is_empty());
}
