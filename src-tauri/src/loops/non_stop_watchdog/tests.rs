use super::*;
use tauri::Listener;

fn report(project: &str, disparity: bool, tolerance_seconds: u32) -> NonStopReport {
    NonStopReport {
        project_path: project.to_string(),
        group_name: "Alert me!".to_string(),
        disparity,
        working: if disparity { 1 } else { 2 },
        total: 2,
        not_working_workgroups: if disparity {
            vec!["wg-2".to_string()]
        } else {
            vec![]
        },
        tolerance_seconds,
        telegram_enabled: false,
        telegram_bot_id: None,
        sound_enabled: false,
        sound_seconds: 3,
    }
}

fn report_with_sound(
    project: &str,
    disparity: bool,
    tolerance_seconds: u32,
    sound_seconds: u32,
) -> NonStopReport {
    NonStopReport {
        sound_enabled: true,
        sound_seconds,
        ..report(project, disparity, tolerance_seconds)
    }
}

fn bot(id: &str) -> TelegramBotConfig {
    TelegramBotConfig {
        id: id.to_string(),
        label: format!("label-{id}"),
        token: format!("token-{id}"),
        chat_id: 100,
        color: "#000".to_string(),
    }
}

async fn disparity_since(state: &NonStopWatchdogState, project: &str) -> Option<Instant> {
    state
        .inner
        .lock()
        .await
        .get(project)
        .unwrap()
        .disparity_since
}

async fn is_fired(state: &NonStopWatchdogState, project: &str) -> bool {
    state.inner.lock().await.get(project).unwrap().fired
}

/// Force an armed episode's `disparity_since` (and `last_seen`) into the past
/// so tolerance/staleness logic can be exercised without sleeping.
async fn backdate_disparity(state: &NonStopWatchdogState, project: &str, ago: Duration) {
    let mut map = state.inner.lock().await;
    let ep = map.get_mut(project).unwrap();
    ep.disparity_since = Some(Instant::now() - ago);
}

async fn backdate_last_seen(state: &NonStopWatchdogState, project: &str, ago: Duration) {
    let mut map = state.inner.lock().await;
    map.get_mut(project).unwrap().last_seen = Instant::now() - ago;
}

async fn fireable(state: &NonStopWatchdogState) -> Vec<NonStopReport> {
    let mut map = state.inner.lock().await;
    collect_fireable(&mut map, Instant::now())
}

#[tokio::test]
async fn ingest_stamps_onset_once() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;
    let first = disparity_since(&state, "p").await;
    assert!(first.is_some());

    state.ingest(vec![report("p", true, 30)]).await;
    let second = disparity_since(&state, "p").await;
    assert_eq!(first, second, "a continuing disparity keeps the same onset");
}

#[tokio::test]
async fn ingest_clears_on_recovery() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;
    state.ingest(vec![report("p", false, 30)]).await;
    assert_eq!(disparity_since(&state, "p").await, None);
    assert!(!is_fired(&state, "p").await);
}

#[tokio::test]
async fn ingest_drops_absent_projects() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;
    // A snapshot that omits "p" disarms it.
    state.ingest(vec![]).await;
    assert!(state.inner.lock().await.get("p").is_none());
}

#[tokio::test]
async fn tick_fires_after_tolerance_and_not_before() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;

    // Not yet past tolerance.
    assert!(fireable(&state).await.is_empty());

    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    let fired = fireable(&state).await;
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].project_path, "p");

    // Single-fire latch: a second collection yields nothing.
    assert!(fireable(&state).await.is_empty());
}

#[tokio::test]
async fn tick_refires_after_rearm() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    assert_eq!(fireable(&state).await.len(), 1);

    // Recover, then a fresh disparity re-arms and fires again.
    state.ingest(vec![report("p", false, 30)]).await;
    state.ingest(vec![report("p", true, 30)]).await;
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    assert_eq!(fireable(&state).await.len(), 1);
}

#[tokio::test]
async fn tick_disarms_after_staleness_ceiling() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    // Frontend went silent well past the ceiling.
    backdate_last_seen(
        &state,
        "p",
        REPORT_STALENESS_CEILING + Duration::from_secs(1),
    )
    .await;

    // One collection disarms instead of firing.
    assert!(fireable(&state).await.is_empty());
    assert_eq!(disparity_since(&state, "p").await, None);
    assert!(!is_fired(&state, "p").await);
}

#[tokio::test]
async fn fresh_last_seen_does_not_disarm() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    // last_seen is fresh (just ingested): a live minimized episode still fires.
    let fired = fireable(&state).await;
    assert_eq!(fired.len(), 1);
}

#[tokio::test]
async fn ingest_returns_recovered_paths() {
    let state = NonStopWatchdogState::new();

    let recovered = state.ingest(vec![report("p", true, 30)]).await;
    assert!(recovered.is_empty());

    let recovered = state.ingest(vec![report("p", false, 30)]).await;
    assert_eq!(recovered, vec!["p"]);

    // Re-arm, then drop the project from the snapshot entirely.
    state.ingest(vec![report("p", true, 30)]).await;
    let recovered = state.ingest(vec![]).await;
    assert_eq!(recovered, vec!["p"]);
}

#[tokio::test]
async fn fire_skips_when_disparity_cleared() {
    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await;
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    // The tick decided to fire (episode marked fired)...
    assert_eq!(fireable(&state).await.len(), 1);
    // ...but a clear arrives before actuation.
    state.ingest(vec![report("p", false, 30)]).await;

    let map = state.inner.lock().await;
    assert!(
        fresh_if_still_disparate(&map, "p").is_none(),
        "TOCTOU guard: no send/beep once the disparity cleared"
    );
}

#[test]
fn alarm_event_name_matches_the_frontend_contract() {
    assert_eq!(NON_STOP_ALARM_EVENT, "non_stop_alarm");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fire_emits_start_once_per_episode() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap_or_else(|_| panic!("build app"));
    let (tx, rx) = std::sync::mpsc::channel();
    app.listen_any(NON_STOP_ALARM_EVENT, move |event| {
        let _ = tx.send(event.payload().to_string());
    });

    let state = NonStopWatchdogState::new();
    state
        .ingest(vec![report_with_sound("p", true, 30, 30)])
        .await;
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    let to_fire = fireable(&state).await;
    assert_eq!(to_fire.len(), 1);
    fire(app.handle(), &state, &to_fire[0]).await;

    let payload: serde_json::Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
    assert_eq!(payload["action"], "start");
    assert_eq!(payload["projectPath"], "p");
    assert_eq!(payload["groupName"], "Alert me!");
    assert_eq!(payload["seconds"], 30);
    let mut keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["action", "groupName", "projectPath", "seconds"]);

    // Second, separate drive inside the same test: pins the clamp(1, 60) ceiling.
    let state = NonStopWatchdogState::new();
    state
        .ingest(vec![report_with_sound("q", true, 30, 600)])
        .await;
    backdate_disparity(&state, "q", Duration::from_secs(31)).await;
    let to_fire = fireable(&state).await;
    assert_eq!(to_fire.len(), 1);
    fire(app.handle(), &state, &to_fire[0]).await;

    let payload: serde_json::Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
    assert_eq!(payload["projectPath"], "q");
    assert_eq!(payload["seconds"], 60);

    // Single-fire latch: a second collection yields nothing, so no second event.
    assert!(fireable(&state).await.is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

    // Third, separate drive: pins the clamp(1, 60) floor.
    let state = NonStopWatchdogState::new();
    state
        .ingest(vec![report_with_sound("r", true, 30, 0)])
        .await;
    backdate_disparity(&state, "r", Duration::from_secs(31)).await;
    let to_fire = fireable(&state).await;
    assert_eq!(to_fire.len(), 1);
    fire(app.handle(), &state, &to_fire[0]).await;

    let payload: serde_json::Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
    assert_eq!(payload["projectPath"], "r");
    assert_eq!(payload["seconds"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fire_emits_nothing_when_disparity_cleared() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap_or_else(|_| panic!("build app"));
    let (tx, rx) = std::sync::mpsc::channel();
    app.listen_any(NON_STOP_ALARM_EVENT, move |event| {
        let _ = tx.send(event.payload().to_string());
    });

    let state = NonStopWatchdogState::new();
    state
        .ingest(vec![report_with_sound("p", true, 30, 30)])
        .await;
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    let to_fire = fireable(&state).await;
    assert_eq!(to_fire.len(), 1);
    // The clear arrives between the tick's fire-decision and actuation (G5).
    // Armed with the Sound measure ON so only the TOCTOU guard can suppress it:
    // a `sound_enabled: false` clear would be hidden by the sound gate instead.
    state
        .ingest(vec![report_with_sound("p", false, 30, 30)])
        .await;
    fire(app.handle(), &state, &to_fire[0]).await;

    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "TOCTOU: no alarm once the disparity cleared"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fire_emits_nothing_when_sound_disabled() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap_or_else(|_| panic!("build app"));
    let (tx, rx) = std::sync::mpsc::channel();
    app.listen_any(NON_STOP_ALARM_EVENT, move |event| {
        let _ = tx.send(event.payload().to_string());
    });

    let state = NonStopWatchdogState::new();
    state.ingest(vec![report("p", true, 30)]).await; // sound_enabled: false
    backdate_disparity(&state, "p", Duration::from_secs(31)).await;
    let to_fire = fireable(&state).await;
    assert_eq!(to_fire.len(), 1);
    fire(app.handle(), &state, &to_fire[0]).await;

    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "no alarm when the Sound measure is off"
    );
}

#[test]
fn resolve_bot_prefers_requested_then_first_then_none() {
    let bots = vec![bot("a"), bot("b")];
    assert_eq!(resolve_bot(&bots, Some("b")).unwrap().id, "b");
    // Unknown requested id falls back to the first configured bot.
    assert_eq!(resolve_bot(&bots, Some("zzz")).unwrap().id, "a");
    // No request -> first bot.
    assert_eq!(resolve_bot(&bots, None).unwrap().id, "a");
    // No bots -> none.
    assert!(resolve_bot(&[], Some("a")).is_none());
    assert!(resolve_bot(&[], None).is_none());
}

#[test]
fn project_folder_name_extracts_leaf() {
    assert_eq!(project_folder_name("C:/repos/my-project"), "my-project");
    assert_eq!(project_folder_name("plain"), "plain");
}

#[derive(Clone)]
struct RecordingWatchdogHost {
    state: NonStopWatchdogState,
    settings: Option<SettingsState>,
    network: Option<OutboundNetwork>,
    calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl RecordingWatchdogHost {
    fn new(state: &NonStopWatchdogState, bots: Option<Vec<TelegramBotConfig>>) -> Self {
        Self {
            state: state.clone(),
            settings: bots.map(|telegram_bots| {
                crate::config::settings::new_settings_state(crate::config::settings::AppSettings {
                    telegram_bots,
                    ..Default::default()
                })
            }),
            network: None,
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn record(&self, name: &'static str) {
        assert!(self.state.inner.try_lock().is_ok(), "episode lock released");
        self.calls.lock().unwrap().push(name);
    }
}

impl NonStopAlarmEvents for RecordingWatchdogHost {
    fn alarm(&self, payload: NonStopAlarmPayload) {
        assert_eq!(payload.action, "start");
        assert_eq!(payload.project_path, "p");
        assert_eq!(payload.seconds, 3);
        self.record("alarm");
    }
}

impl WatchdogHost for RecordingWatchdogHost {
    fn settings_owner(&self) -> &SettingsState {
        self.record("settings");
        self.settings.as_ref().expect("missing settings at use")
    }

    fn outbound_network(&self) -> &OutboundNetwork {
        self.record("network");
        assert!(self.settings.as_ref().unwrap().try_write().is_ok());
        self.network.as_ref().expect("missing network at use")
    }

    fn spawn_background(&self, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {
        self.record("spawn");
        assert!(self.settings.as_ref().unwrap().try_write().is_ok());
        // Recording submission must not poll a real Telegram operation.
        drop(future);
    }
}

#[tokio::test]
async fn issue_3260_watchdog_early_paths_do_not_lookup_settings_or_network() {
    let state = NonStopWatchdogState::new();
    let host = RecordingWatchdogHost::new(&state, None);
    let mut r = report_with_sound("p", true, 0, 3);
    state.ingest(vec![r.clone()]).await;
    tick(&host, &state).await;
    tick(&host, &state).await;
    assert_eq!(*host.calls.lock().unwrap(), ["alarm"], "single-fire latch");

    host.calls.lock().unwrap().clear();
    r.telegram_enabled = true;
    state.ingest(vec![report("p", false, 0)]).await;
    fire(&host, &state, &r).await;
    assert!(
        host.calls.lock().unwrap().is_empty(),
        "recovered report rechecked"
    );
    state.ingest(vec![r]).await;
    backdate_last_seen(
        &state,
        "p",
        REPORT_STALENESS_CEILING + Duration::from_secs(1),
    )
    .await;
    tick(&host, &state).await;
    assert!(
        host.calls.lock().unwrap().is_empty(),
        "stale episode disarmed"
    );
}

#[tokio::test]
async fn issue_3260_watchdog_missing_settings_panics_only_at_use_after_alarm() {
    use futures::FutureExt;
    let state = NonStopWatchdogState::new();
    let host = RecordingWatchdogHost::new(&state, None);
    let mut r = report_with_sound("p", true, 0, 3);
    r.telegram_enabled = true;
    state.ingest(vec![r.clone()]).await;
    assert!(std::panic::AssertUnwindSafe(fire(&host, &state, &r))
        .catch_unwind()
        .await
        .is_err());
    assert_eq!(*host.calls.lock().unwrap(), ["alarm", "settings"]);
}

#[tokio::test]
async fn issue_3260_watchdog_no_bot_skips_network_and_spawn() {
    let state = NonStopWatchdogState::new();
    let host = RecordingWatchdogHost::new(&state, Some(Vec::new()));
    let mut r = report("p", true, 0);
    r.telegram_enabled = true;
    state.ingest(vec![r.clone()]).await;
    fire(&host, &state, &r).await;
    assert_eq!(*host.calls.lock().unwrap(), ["settings"]);
}

#[tokio::test]
async fn issue_3260_watchdog_resolves_bot_releases_settings_then_network_and_spawn() {
    use futures::FutureExt;
    let state = NonStopWatchdogState::new();
    let mut host = RecordingWatchdogHost::new(&state, Some(vec![bot("a"), bot("b")]));
    let mut r = report_with_sound("p", true, 0, 3);
    r.telegram_enabled = true;
    r.telegram_bot_id = Some("b".into());
    state.ingest(vec![r.clone()]).await;
    assert!(std::panic::AssertUnwindSafe(fire(&host, &state, &r))
        .catch_unwind()
        .await
        .is_err());
    assert_eq!(
        *host.calls.lock().unwrap(),
        ["alarm", "settings", "network"]
    );

    host.calls.lock().unwrap().clear();
    host.network = Some(OutboundNetwork::new_for_tests(1));
    fire(&host, &state, &r).await;
    assert_eq!(
        *host.calls.lock().unwrap(),
        ["alarm", "settings", "network", "spawn"]
    );
    assert!(host
        .network
        .as_ref()
        .unwrap()
        .acquired_labels_for_tests()
        .is_empty());
}
