use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Local};

use super::*;

// ---- fixtures --------------------------------------------------------

struct RoomFixture {
    _temp: tempfile::TempDir,
    room: PathBuf,
    coordinator_replica: PathBuf,
}

fn replica_config(agent: &str) -> String {
    format!(r#"{{"identity":"../../_agent_{agent}","context":[],"repos":[]}}"#)
}

const TEAM_CONFIG: &str = concat!(
    r#"{"agents":["_agent_member","_agent_coordinator","_agent_other"],"#,
    r#""coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50,75]}"#,
);

/// A room with three replicas, one of them the configured orchestrator.
/// Deliberately not a helper shared with the context-alert tests: importing
/// theirs would add an arc the phase's cycle condition forbids.
fn room_fixture() -> RoomFixture {
    let temp = tempfile::tempdir().expect("temp dir");
    let ac_root = temp.path().join("project-a").join(".ac");
    let room = ac_root.join("wg-2-dev-team");
    let coordinator = room.join("__agent_coordinator");
    for path in [
        &coordinator,
        &room.join("__agent_member"),
        &room.join("__agent_other"),
        &ac_root.join("_agent_coordinator"),
        &ac_root.join("_agent_member"),
        &ac_root.join("_agent_other"),
        &ac_root.join("_team_dev-team"),
    ] {
        std::fs::create_dir_all(path).expect("fixture dir");
    }
    for (dir, agent) in [
        ("__agent_coordinator", "coordinator"),
        ("__agent_member", "member"),
        ("__agent_other", "other"),
    ] {
        std::fs::write(room.join(dir).join("config.json"), replica_config(agent))
            .expect("replica config");
    }
    std::fs::write(
        ac_root.join("_team_dev-team").join("config.json"),
        TEAM_CONFIG,
    )
    .expect("team config");
    RoomFixture {
        _temp: temp,
        room,
        coordinator_replica: coordinator,
    }
}

/// Creates the link the way the platform needs it, and FAILS rather than
/// skipping, so the coverage cannot silently evaporate on a machine where
/// the link cannot be created.
fn link_directory(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("run mklink");
        assert!(
            output.status.success(),
            "mklink /J failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).expect("symlink");
    }
}

fn dials(ci: bool, staleness: bool) -> RemoteNotifyDials {
    RemoteNotifyDials { ci, staleness }
}

/// A real `DateTime<Local>`, built from a fixed instant rather than the wall
/// clock: the wall clock must not be read anywhere in this file.
fn observed_at() -> DateTime<Local> {
    DateTime::parse_from_rfc3339("2026-09-15T23:18:43-03:00")
        .expect("fixture timestamp")
        .with_timezone(&Local)
}

/// Shaped exactly as Phase A's transitions are: an empty base branch and no
/// commit count for the two CI kinds, both set for a stale branch.
fn transition(kind: TransitionKind, room: &Path, repo: &str) -> RemoteTransition {
    let stale = matches!(kind, TransitionKind::BranchStale);
    RemoteTransition {
        repo_path: repo.to_string(),
        room_dir: room.to_string_lossy().to_string(),
        nwo: "mblua/AgentsCommander".to_string(),
        branch: "feature/2083-2064-remote-alerts".to_string(),
        head_sha: "abcdef1".repeat(5) + "abcde",
        kind,
        behind_by: stale.then_some(4),
        base_branch: if stale {
            "main".to_string()
        } else {
            String::new()
        },
        observed_at: observed_at(),
        last_confirmed_at: Some(observed_at()),
    }
}

fn production_target(room: &Path) -> InternalSystemTarget {
    resolve_orchestrator_blocking(room, &Mutex::new(HashSet::new())).expect("fixture target")
}

#[derive(Clone, Debug)]
struct Delivery {
    target_fqn: String,
    kind: RemoteNoticeKind,
    at: String,
    blind_gap: Option<(String, String)>,
}

/// The variant's fields, never the rendered line: `line()` is private to
/// `phone::mailbox`, so a test that cannot call it must not pretend to.
fn delivery_of(target: &InternalSystemTarget, notice: &InternalSystemNotice) -> Delivery {
    match notice {
        InternalSystemNotice::RemoteActivity {
            kind,
            at,
            blind_gap,
            ..
        } => Delivery {
            target_fqn: target.fqn().to_string(),
            kind: *kind,
            at: at.clone(),
            blind_gap: blind_gap.clone(),
        },
        InternalSystemNotice::ContextAlert { .. } => panic!("a remote activity notice"),
    }
}

type ScriptedAttempt = (
    Result<(), String>,
    Option<tokio::sync::oneshot::Receiver<()>>,
);

struct RecordingPorts {
    dials: Mutex<RemoteNotifyDials>,
    target: Mutex<Option<InternalSystemTarget>>,
    resolutions: AtomicUsize,
    deliveries: Mutex<Vec<Delivery>>,
    originals: Mutex<Vec<(InternalSystemTarget, InternalSystemNotice)>>,
    attempts: Mutex<std::collections::VecDeque<ScriptedAttempt>>,
    delivery_started: tokio::sync::Notify,
    completed: Arc<AtomicUsize>,
    dial_reads: AtomicUsize,
}

impl RecordingPorts {
    fn new(dials: RemoteNotifyDials, target: Option<InternalSystemTarget>) -> Self {
        Self {
            dials: Mutex::new(dials),
            target: Mutex::new(target),
            resolutions: AtomicUsize::new(0),
            deliveries: Mutex::new(Vec::new()),
            originals: Mutex::new(Vec::new()),
            attempts: Mutex::new(std::collections::VecDeque::new()),
            delivery_started: tokio::sync::Notify::new(),
            completed: Arc::new(AtomicUsize::new(0)),
            dial_reads: AtomicUsize::new(0),
        }
    }

    fn set_dials(&self, dials: RemoteNotifyDials) {
        *self.dials.lock().unwrap_or_else(|error| error.into_inner()) = dials;
    }

    fn resolutions(&self) -> usize {
        self.resolutions.load(Ordering::SeqCst)
    }

    fn deliveries(&self) -> Vec<Delivery> {
        self.deliveries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl RemoteAlertPorts for RecordingPorts {
    fn resolve_orchestrator(
        &self,
        _room_dir: String,
    ) -> BoxFuture<'static, Option<InternalSystemTarget>> {
        self.resolutions.fetch_add(1, Ordering::SeqCst);
        let target = self
            .target
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        Box::pin(async move { target })
    }

    fn deliver(
        &self,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
    ) -> BoxFuture<'static, Result<(), String>> {
        self.deliveries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(delivery_of(&target, &notice));
        self.originals.lock().unwrap().push((target, notice));
        let (result, gate) = self
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((Ok(()), None));
        self.delivery_started.notify_one();
        let completed = self.completed.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.await.expect("release delivery completion");
            }
            if result.is_ok() {
                completed.fetch_add(1, Ordering::SeqCst);
            }
            result
        })
    }

    fn notify_dials(&self) -> BoxFuture<'static, RemoteNotifyDials> {
        self.dial_reads.fetch_add(1, Ordering::SeqCst);
        let dials = *self.dials.lock().unwrap_or_else(|error| error.into_inner());
        Box::pin(async move { dials })
    }
}

/// Resolves through the production path, so a containment drop is observed at
/// `handle` and not only at the helper.
#[derive(Default)]
struct ResolvingPorts {
    deliveries: Mutex<Vec<Delivery>>,
}

impl ResolvingPorts {
    fn deliveries(&self) -> Vec<Delivery> {
        self.deliveries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl RemoteAlertPorts for ResolvingPorts {
    fn resolve_orchestrator(
        &self,
        room_dir: String,
    ) -> BoxFuture<'static, Option<InternalSystemTarget>> {
        Box::pin(async move {
            tauri::async_runtime::spawn_blocking(move || {
                resolve_orchestrator_blocking(Path::new(&room_dir), &Mutex::new(HashSet::new()))
            })
            .await
            .ok()
            .flatten()
        })
    }

    fn deliver(
        &self,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
    ) -> BoxFuture<'static, Result<(), String>> {
        self.deliveries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(delivery_of(&target, &notice));
        Box::pin(async move { Ok(()) })
    }

    fn notify_dials(&self) -> BoxFuture<'static, RemoteNotifyDials> {
        Box::pin(async move { dials(true, true) })
    }
}

// ---- 13..20: policy ---------------------------------------------------

#[tokio::test]
async fn notify_dial_off_drops_before_resolution() {
    let ports = Arc::new(RecordingPorts::new(dials(false, true), None));
    let mut state = RemoteAlertState::new(ports.clone());
    let now = Instant::now();
    state
        .handle(
            transition(TransitionKind::CiStarted, Path::new("room"), "repo"),
            now,
        )
        .await;
    assert_eq!(
        ports.resolutions(),
        0,
        "a disabled CI axis resolves nothing"
    );

    ports.set_dials(dials(true, false));
    state
        .handle(
            transition(TransitionKind::BranchStale, Path::new("room"), "repo"),
            now,
        )
        .await;
    assert_eq!(
        ports.resolutions(),
        0,
        "a disabled staleness axis resolves nothing"
    );
    assert!(ports.deliveries().is_empty());
}

#[tokio::test]
async fn ci_notice_from_a_sweeper_transition_is_delivered() {
    let fixture = room_fixture();
    let ports = Arc::new(RecordingPorts::new(
        dials(true, false),
        Some(production_target(&fixture.room)),
    ));
    let mut state = RemoteAlertState::new(ports.clone());
    let now = Instant::now();
    for kind in [TransitionKind::CiStarted, TransitionKind::CiFinished] {
        state
            .handle(transition(kind, &fixture.room, "repo-a"), now)
            .await;
    }

    let delivered = ports.deliveries();
    assert_eq!(delivered.len(), 2, "the CI axis must not ship dark");
    assert_eq!(delivered[0].kind, RemoteNoticeKind::CiStarted);
    assert_eq!(delivered[1].kind, RemoteNoticeKind::CiFinished);
    assert_eq!(
        delivered[0].target_fqn,
        "project-a:wg-2-dev-team/coordinator"
    );
}

#[tokio::test]
async fn only_the_configured_orchestrator_is_ever_targeted() {
    let fixture = room_fixture();
    let ports = Arc::new(ResolvingPorts::default());
    let mut state = RemoteAlertState::new(ports.clone());
    state
        .handle(
            transition(TransitionKind::CiStarted, &fixture.room, "repo-a"),
            Instant::now(),
        )
        .await;

    let delivered = ports.deliveries();
    assert_eq!(delivered.len(), 1, "three replicas, exactly one target");
    assert_eq!(
        delivered[0].target_fqn,
        "project-a:wg-2-dev-team/coordinator"
    );
    assert!(
        delivered
            .iter()
            .all(|delivery| !delivery.target_fqn.contains("member")
                && !delivery.target_fqn.contains("other")),
        "no code path enumerates the non-orchestrator replicas"
    );
}

#[test]
fn resolved_replica_outside_the_room_is_dropped() {
    let temp = tempfile::tempdir().expect("temp dir");
    let ac_root = temp.path().join("project-a").join(".ac");
    let room = ac_root.join("wg-2-dev-team");
    let outside = temp.path().join("elsewhere").join("__agent_coordinator");
    for path in [
        &outside,
        &temp.path().join("_agent_coordinator"),
        &ac_root.join("_agent_coordinator"),
        &ac_root.join("_team_dev-team"),
        &room,
    ] {
        std::fs::create_dir_all(path).expect("fixture dir");
    }
    std::fs::write(outside.join("config.json"), replica_config("coordinator"))
        .expect("replica config");
    std::fs::write(
        ac_root.join("_team_dev-team").join("config.json"),
        r#"{"agents":["_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[]}"#,
    )
    .expect("team config");

    // A real directory outside the room is never inside it.
    assert!(!replica_within_room(&room, &outside));

    // The resolver DOES find the orchestrator through the link; containment
    // is what drops it.
    link_directory(&room.join("__agent_coordinator"), &outside);
    assert!(
        crate::config::teams::resolve_wg_coordinator_replica(&ac_root, &room).is_some(),
        "the fixture must resolve, or this test proves nothing"
    );
    assert!(
        resolve_orchestrator_blocking(&room, &Mutex::new(HashSet::new())).is_none(),
        "a notice addressed through a replica outside the room is dropped"
    );
}

#[tokio::test]
async fn cap_allows_twelve_then_suppresses_silently() {
    let fixture = room_fixture();
    let ports = Arc::new(RecordingPorts::new(
        dials(true, true),
        Some(production_target(&fixture.room)),
    ));
    let mut state = RemoteAlertState::new(ports.clone());
    let now = Instant::now();
    for _ in 0..12 {
        state
            .handle(
                transition(TransitionKind::CiStarted, &fixture.room, "repo-a"),
                now,
            )
            .await;
    }
    for _ in 0..3 {
        state
            .handle(
                transition(TransitionKind::BranchStale, &fixture.room, "repo-a"),
                now,
            )
            .await;
    }

    let delivered = ports.deliveries();
    assert_eq!(delivered.len(), 12);
    assert!(
        delivered
            .iter()
            .all(|delivery| delivery.kind == RemoteNoticeKind::CiStarted),
        "the first twelve transitions are the ones delivered"
    );
}

#[tokio::test]
async fn cap_window_is_rolling_and_per_room_repo_pair() {
    let fixture = room_fixture();
    let ports = Arc::new(RecordingPorts::new(
        dials(true, true),
        Some(production_target(&fixture.room)),
    ));
    let mut state = RemoteAlertState::new(ports.clone());
    let now = Instant::now();
    let room = fixture.room.clone();

    for _ in 0..12 {
        state
            .handle(transition(TransitionKind::CiStarted, &room, "repo-a"), now)
            .await;
    }
    state
        .handle(transition(TransitionKind::CiStarted, &room, "repo-b"), now)
        .await;
    assert_eq!(
        ports.deliveries().len(),
        13,
        "the window is keyed by the (room, repo) pair, not by the room"
    );

    state
        .handle(
            transition(TransitionKind::CiStarted, &room, "repo-a"),
            now + Duration::from_secs(3599),
        )
        .await;
    assert_eq!(
        ports.deliveries().len(),
        13,
        "inside the window the cap still holds"
    );

    state
        .handle(
            transition(TransitionKind::CiStarted, &room, "repo-a"),
            now + Duration::from_secs(3601),
        )
        .await;
    assert_eq!(
        ports.deliveries().len(),
        14,
        "the window is rolling, not a fixed bucket"
    );
}

#[tokio::test]
async fn blind_gap_threshold_is_six_hundred_seconds() {
    let fixture = room_fixture();
    let ports = Arc::new(RecordingPorts::new(
        dials(true, true),
        Some(production_target(&fixture.room)),
    ));
    let mut state = RemoteAlertState::new(ports.clone());
    let now = Instant::now();

    let mut short = transition(TransitionKind::CiStarted, &fixture.room, "repo-a");
    short.last_confirmed_at = Some(observed_at() - chrono::Duration::seconds(599));
    let mut long = transition(TransitionKind::CiStarted, &fixture.room, "repo-a");
    long.last_confirmed_at = Some(observed_at() - chrono::Duration::seconds(601));
    let mut first_ever = transition(TransitionKind::CiStarted, &fixture.room, "repo-a");
    first_ever.last_confirmed_at = None;
    for candidate in [short, long, first_ever] {
        state.handle(candidate, now).await;
    }

    let delivered = ports.deliveries();
    assert_eq!(delivered.len(), 3);
    assert_eq!(delivered[0].blind_gap, None, "599 s is not a blind gap");
    assert_eq!(
        delivered[1].blind_gap.as_ref().map(|(gap, _)| gap.as_str()),
        Some("10 minutes"),
        "601 s is"
    );
    assert_eq!(
        delivered[2].blind_gap, None,
        "no previous confirmed state appends nothing"
    );
}

#[test]
fn blind_gap_threshold_needs_two_lost_rounds_at_the_default_interval() {
    let i = crate::config::settings::AppSettings::default().branch_staleness_interval_secs as i64;
    assert!(
        BLIND_GAP_THRESHOLD_SECS > 2 * i,
        "#2149: one lost round measures two intervals and must not render the clause"
    );
    assert!(
        BLIND_GAP_THRESHOLD_SECS <= 3 * i,
        "#2149: two lost rounds must render the clause"
    );
}

#[tokio::test]
async fn timestamp_format_is_local_with_a_numeric_offset() {
    let fixture = room_fixture();
    let ports = Arc::new(RecordingPorts::new(
        dials(true, false),
        Some(production_target(&fixture.room)),
    ));
    let mut state = RemoteAlertState::new(ports.clone());
    state
        .handle(
            transition(TransitionKind::CiStarted, &fixture.room, "repo-a"),
            Instant::now(),
        )
        .await;

    let at = ports.deliveries()[0].at.clone();
    assert!(
        is_offset_timestamp(&at),
        "the observation time must be local with a numeric offset: {at}"
    );
}

/// `^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}[+-]\d{2}:\d{2}$`, spelled out rather
/// than matched with a regex: a `to_rfc3339()` of UTC would still look like a
/// timestamp to the eye, and this is the only assertion that catches it.
fn is_offset_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 25 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && bytes[10] == b' '
        && digits(11..13)
        && bytes[13] == b':'
        && digits(14..16)
        && bytes[16] == b':'
        && digits(17..19)
        && matches!(bytes[19], b'+' | b'-')
        && digits(20..22)
        && bytes[22] == b':'
        && digits(23..25)
}

#[test]
fn gap_is_rendered_in_human_units() {
    assert_eq!(human_gap(2820), "47 minutes");
    assert_eq!(human_gap(7500), "2 hours 5 minutes");
    assert_eq!(human_gap(7200), "2 hours");
    assert!(!human_gap(2820).contains("2820"));
}

// ---- 21..23: lifecycle and containment --------------------------------

#[tokio::test]
async fn channel_close_ends_the_monitor() {
    let ports = Arc::new(RecordingPorts::new(dials(true, false), None));
    let (sender, receiver) = mpsc::channel(4);
    let handle =
        tauri::async_runtime::spawn(run(ports.clone(), receiver, CancellationToken::new()));
    sender
        .send(transition(
            TransitionKind::CiStarted,
            Path::new("room"),
            "repo",
        ))
        .await
        .expect("queued before the close");
    drop(sender);

    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the monitor ends when the sweeper's channel closes")
        .expect("the monitor task must not panic");
}

#[tokio::test]
async fn shutdown_ends_the_monitor() {
    let ports = Arc::new(RecordingPorts::new(dials(true, true), None));
    let (_sender, receiver) = mpsc::channel(4);
    let cancellation = CancellationToken::new();
    let handle = tauri::async_runtime::spawn(run(ports.clone(), receiver, cancellation.clone()));
    cancellation.cancel();

    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the monitor ends on shutdown")
        .expect("the monitor task must not panic");
    assert_eq!(ports.resolutions(), 0, "nothing is resolved after shutdown");
}

#[tokio::test]
async fn containment_canonicalizes_both_comparands() {
    let fixture = room_fixture();
    // A `..` hop on the way to the room, and a `.`/`..` detour inside the
    // replica path: the same directories, so neither reads as an escape.
    let hop = fixture._temp.path().join("hop");
    std::fs::create_dir_all(&hop).expect("hop dir");
    let indirect_room = hop
        .join("..")
        .join("project-a")
        .join(".ac")
        .join("wg-2-dev-team");
    let indirect_replica = fixture
        .coordinator_replica
        .join(".")
        .join("..")
        .join("__agent_coordinator");
    assert!(replica_within_room(
        &indirect_room,
        &fixture.coordinator_replica
    ));
    assert!(replica_within_room(&fixture.room, &indirect_replica));

    // And the real resolver still resolves and delivers through that spelling.
    let ports = Arc::new(ResolvingPorts::default());
    let mut state = RemoteAlertState::new(ports.clone());
    state
        .handle(
            transition(TransitionKind::CiStarted, &indirect_room, "repo-a"),
            Instant::now(),
        )
        .await;
    let delivered = ports.deliveries();
    assert_eq!(
        delivered.len(),
        1,
        "a non-canonical spelling is the same room"
    );
    assert_eq!(
        delivered[0].target_fqn,
        "project-a:wg-2-dev-team/coordinator"
    );

    // A directory that is not a child of the room is still outside it.
    let outside = fixture._temp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).expect("outside dir");
    assert!(!replica_within_room(&fixture.room, &outside));

    #[cfg(windows)]
    {
        // Windows resolves a differently-cased spelling to the same directory.
        // Only the room's parents are shouted: the room's own name carries the
        // `wg-` entity prefix, and the entity parser is case-sensitive, so
        // shouting that component would test the parser and not containment.
        let shouted = PathBuf::from(
            fixture
                .room
                .parent()
                .expect("room parent")
                .to_string_lossy()
                .to_uppercase(),
        )
        .join(fixture.room.file_name().expect("room name"));
        let shouted_ac_root = shouted.parent().expect("ac root");
        let resolved =
            crate::config::teams::resolve_wg_coordinator_replica(shouted_ac_root, &shouted)
                .expect("the resolver reads a case variant");
        assert!(replica_within_room(&shouted, &fixture.coordinator_replica));
        assert!(
            replica_within_room(&shouted, &resolved.replica_dir),
            "a case variant resolves to a replica this room contains"
        );
        // The delivery half IS asserted: the notice is addressed by the
        // canonical spelling, so a settings file or a discovery scan that
        // spells the room with different case does not lose every notice.
        let ports = Arc::new(ResolvingPorts::default());
        let mut state = RemoteAlertState::new(ports.clone());
        state
            .handle(
                transition(TransitionKind::CiStarted, &shouted, "repo-a"),
                Instant::now(),
            )
            .await;
        let delivered = ports.deliveries();
        assert_eq!(
            delivered.len(),
            1,
            "a differently-cased path is the same room and must deliver"
        );
        assert_eq!(
            delivered[0].target_fqn, "project-a:wg-2-dev-team/coordinator",
            "the FQN is spelled from the canonical directory names"
        );
    }
}

#[tokio::test]
async fn containment_rejects_a_link_or_reparse_replica() {
    let temp = tempfile::tempdir().expect("temp dir");
    let ac_root = temp.path().join("project-a").join(".ac");
    let room = ac_root.join("wg-2-dev-team");
    let real_inside = room.join("__agent_real");
    for path in [
        &real_inside,
        &ac_root.join("_agent_coordinator"),
        &ac_root.join("_team_dev-team"),
    ] {
        std::fs::create_dir_all(path).expect("fixture dir");
    }
    std::fs::write(
        real_inside.join("config.json"),
        replica_config("coordinator"),
    )
    .expect("replica config");
    std::fs::write(
        ac_root.join("_team_dev-team").join("config.json"),
        r#"{"agents":["_agent_coordinator"],"coordinator":"_agent_coordinator","repos":[]}"#,
    )
    .expect("team config");

    // First as a real directory: the resolver finds it, so the drop below is
    // caused by the link and not by an unresolvable fixture.
    std::fs::create_dir_all(room.join("__agent_coordinator")).expect("real replica");
    std::fs::write(
        room.join("__agent_coordinator").join("config.json"),
        replica_config("coordinator"),
    )
    .expect("replica config");
    assert!(
        resolve_orchestrator_blocking(&room, &Mutex::new(HashSet::new())).is_some(),
        "the fixture must resolve before the link replaces it"
    );
    std::fs::remove_dir_all(room.join("__agent_coordinator")).expect("drop the real replica");

    // Now the configured coordinator replica is a LINK to a real directory
    // INSIDE the room, so only the link check can reject it: the parent check
    // would pass.
    link_directory(&room.join("__agent_coordinator"), &real_inside);
    assert!(!replica_within_room(
        &room,
        &room.join("__agent_coordinator")
    ));

    let ports = Arc::new(ResolvingPorts::default());
    let mut state = RemoteAlertState::new(ports.clone());
    state
        .handle(
            transition(TransitionKind::CiStarted, &room, "repo-a"),
            Instant::now(),
        )
        .await;
    assert!(
        ports.deliveries().is_empty(),
        "a linked replica is never targeted"
    );
}
/// #2129 - T2: the unresolved sentinel is qualified, and neither the bare
/// sentinel nor the branch name is what the notice carries.
#[test]
fn issue_2129_branch_stale_qualifies_the_unresolved_sentinel() {
    let mut transition = transition(TransitionKind::BranchStale, Path::new("room"), "repo");
    transition.branch = "main".to_string();
    transition.base_branch = "the default branch".to_string();
    let notice = notice_for(&transition).expect("a branch-stale notice");
    match notice {
        InternalSystemNotice::RemoteActivity { base_branch, .. } => {
            assert_eq!(base_branch, "the default branch on GitHub");
            assert_ne!(base_branch, "the default branch");
            assert_ne!(base_branch, "main");
        }
        InternalSystemNotice::ContextAlert { .. } => panic!("a remote activity notice"),
    }
}
/// #2129 - T1: a resolved base is named and qualified.
#[test]
fn issue_2129_branch_stale_qualifies_a_resolved_base() {
    let transition = transition(TransitionKind::BranchStale, Path::new("room"), "repo");
    let notice = notice_for(&transition).expect("a branch-stale notice");
    match notice {
        InternalSystemNotice::RemoteActivity { base_branch, .. } => {
            assert_eq!(base_branch, "main on GitHub");
        }
        InternalSystemNotice::ContextAlert { .. } => panic!("a remote activity notice"),
    }
}

/// #2129 - T3: a base equal to the branch is named by relation, so the
/// branch name is never printed twice. #2131 normally suppresses this
/// notice first; the branch is defence in depth.
#[test]
fn issue_2129_branch_stale_names_a_base_equal_to_the_branch_by_relation() {
    let mut transition = transition(TransitionKind::BranchStale, Path::new("room"), "repo");
    transition.branch = "main".to_string();
    transition.base_branch = "main".to_string();
    let notice = notice_for(&transition).expect("a branch-stale notice");
    match notice {
        InternalSystemNotice::RemoteActivity { base_branch, .. } => {
            assert_eq!(base_branch, "its counterpart on GitHub");
            assert!(!base_branch.contains("main"));
        }
        InternalSystemNotice::ContextAlert { .. } => panic!("a remote activity notice"),
    }
}

/// #2129 - T4: the qualifier never touches a CI kind, whose base branch
/// must stay empty.
#[test]
fn issue_2129_ci_kinds_keep_an_empty_base_branch() {
    for kind in [TransitionKind::CiStarted, TransitionKind::CiFinished] {
        let transition = transition(kind, Path::new("room"), "repo");
        let notice = notice_for(&transition).expect("a CI notice");
        match notice {
            InternalSystemNotice::RemoteActivity { base_branch, .. } => {
                assert!(
                    base_branch.is_empty(),
                    "a CI notice carries no base: {kind:?}"
                );
            }
            InternalSystemNotice::ContextAlert { .. } => panic!("a remote activity notice"),
        }
    }
}

mod serial_hold;
