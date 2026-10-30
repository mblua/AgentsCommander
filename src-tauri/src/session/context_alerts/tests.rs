use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

mod batching;
mod lifecycle;
mod resolution;

#[derive(Default)]
struct FakeContextAlertServices {
    sessions: Mutex<HashMap<Uuid, Session>>,
    registrations: Mutex<std::collections::HashSet<Uuid>>,
    blocked: AtomicBool,
    state_checks: AtomicUsize,
}

impl ContextAlertServices for FakeContextAlertServices {
    fn public_session(&self, id: Uuid) -> BoxFuture<'static, Option<Session>> {
        let session = self.sessions.lock().unwrap().get(&id).cloned();
        Box::pin(async move { session })
    }

    fn registered(&self, id: Uuid) -> bool {
        self.registrations.lock().unwrap().contains(&id)
    }

    fn retire_sample_registration(&self, id: Uuid) {
        self.registrations.lock().unwrap().remove(&id);
    }

    fn validate_delivery_state(
        &self,
        id: Uuid,
        target: &InternalSystemTarget,
    ) -> Result<(), String> {
        self.state_checks.fetch_add(1, Ordering::SeqCst);
        if self.blocked.load(Ordering::SeqCst) {
            return Err(format!("purge-room blocks '{}'", target.fqn()));
        }
        if !self.registered(id) {
            return Err("Sampled session is no longer registered".to_string());
        }
        Ok(())
    }

    fn deliver(
        &self,
        _target: InternalSystemTarget,
        _notice: InternalSystemNotice,
        _cancellation: CancellationToken,
        _guard: InternalNoticeGuard,
    ) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

struct ManualClock {
    now: Mutex<Instant>,
    waiters: Mutex<Vec<(Instant, tokio::sync::oneshot::Sender<()>)>>,
    waiters_changed: tokio::sync::Notify,
}

impl ManualClock {
    fn new(now: Instant) -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(now),
            waiters: Mutex::new(Vec::new()),
            waiters_changed: tokio::sync::Notify::new(),
        })
    }

    fn advance(&self, duration: Duration) {
        let now = {
            let mut now = self.now.lock().unwrap();
            *now += duration;
            *now
        };
        let mut waiters = self.waiters.lock().unwrap();
        let mut pending = Vec::new();
        for (deadline, sender) in waiters.drain(..) {
            if deadline <= now {
                let _ = sender.send(());
            } else {
                pending.push((deadline, sender));
            }
        }
        *waiters = pending;
    }

    fn has_live_waiter_within(&self, duration: Duration) -> bool {
        let latest = *self.now.lock().unwrap() + duration;
        self.waiters
            .lock()
            .unwrap()
            .iter()
            .any(|(deadline, sender)| !sender.is_closed() && *deadline <= latest)
    }

    async fn wait_for_live_waiter_within(&self, duration: Duration) {
        wait_until_notified(
            &self.waiters_changed,
            "manual clock waiter",
            || self.has_live_waiter_within(duration),
            || {
                let live_waiters = self
                    .waiters
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(_, sender)| !sender.is_closed())
                    .count();
                format!("live_waiters={live_waiters} window={duration:?}")
            },
        )
        .await;
    }
}

impl AlertClock for ManualClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()> {
        let now = *self.now.lock().unwrap();
        if deadline <= now {
            return Box::pin(async {});
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.waiters.lock().unwrap().push((deadline, sender));
        self.waiters_changed.notify_one();
        Box::pin(async move {
            let _ = receiver.await;
        })
    }
}

struct ScriptedRuntime {
    _temp: tempfile::TempDir,
    target: InternalSystemTarget,
    policies: Mutex<HashMap<Uuid, ResolvedPolicy>>,
    live: AtomicBool,
    live_checks: AtomicUsize,
    resolve_calls: AtomicUsize,
    prepare_calls: Mutex<Vec<AttemptSnapshot>>,
    prepare_live_checks: Mutex<Vec<(u64, u32, usize)>>,
    deliveries: Mutex<Vec<AttemptSnapshot>>,
    delivery_results: Mutex<VecDeque<Result<(), String>>>,
    blocked_delivery: Mutex<Option<tokio::sync::oneshot::Receiver<Result<(), String>>>>,
    blocked_resolution: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    blocked_live_check: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    cancel_during_live_check: Mutex<Option<CancellationToken>>,
    panic_next_delivery: AtomicBool,
    retired: Mutex<Vec<Uuid>>,
    changed: tokio::sync::Notify,
}

impl ScriptedRuntime {
    fn new() -> Arc<Self> {
        let temp = tempfile::tempdir().unwrap();
        let replica = temp
            .path()
            .join("project")
            .join(".ac")
            .join("wg-1-team")
            .join("__agent_coordinator");
        std::fs::create_dir_all(&replica).unwrap();
        let target = InternalSystemTarget::for_context_alert(
            "project:wg-1-team/coordinator".to_string(),
            replica,
        )
        .unwrap();
        Arc::new(Self {
            _temp: temp,
            target,
            policies: Mutex::new(HashMap::new()),
            live: AtomicBool::new(true),
            live_checks: AtomicUsize::new(0),
            resolve_calls: AtomicUsize::new(0),
            prepare_calls: Mutex::new(Vec::new()),
            prepare_live_checks: Mutex::new(Vec::new()),
            deliveries: Mutex::new(Vec::new()),
            delivery_results: Mutex::new(VecDeque::new()),
            blocked_delivery: Mutex::new(None),
            blocked_resolution: Mutex::new(None),
            blocked_live_check: Mutex::new(None),
            cancel_during_live_check: Mutex::new(None),
            panic_next_delivery: AtomicBool::new(false),
            retired: Mutex::new(Vec::new()),
            changed: tokio::sync::Notify::new(),
        })
    }

    fn set_policy(&self, id: Uuid, thresholds: &[u8]) {
        self.policies.lock().unwrap().insert(
            id,
            ResolvedPolicy {
                fingerprint: fingerprint(id),
                thresholds: thresholds.to_vec(),
            },
        );
    }

    fn delivery_count(&self) -> usize {
        self.deliveries.lock().unwrap().len()
    }

    async fn wait_for_delivery_count(&self, expected: usize) {
        wait_until_notified(
            &self.changed,
            "scripted delivery count",
            || self.delivery_count() >= expected,
            || {
                format!(
                    "expected_at_least={expected} actual={}",
                    self.delivery_count()
                )
            },
        )
        .await;
    }

    async fn wait_for_resolve_calls(&self, expected: usize) {
        wait_until_notified(
            &self.changed,
            "scripted policy resolutions",
            || self.resolve_calls.load(Ordering::SeqCst) >= expected,
            || {
                format!(
                    "expected_at_least={expected} actual={}",
                    self.resolve_calls.load(Ordering::SeqCst)
                )
            },
        )
        .await;
    }

    async fn wait_for_live_checks(&self, expected: usize) {
        wait_until_notified(
            &self.changed,
            "scripted liveness checks",
            || self.live_checks.load(Ordering::SeqCst) >= expected,
            || {
                format!(
                    "expected_at_least={expected} actual={}",
                    self.live_checks.load(Ordering::SeqCst)
                )
            },
        )
        .await;
    }

    async fn wait_for_retirement(&self, session_id: Uuid) {
        wait_until_notified(
            &self.changed,
            "scripted sample retirement",
            || self.retired.lock().unwrap().contains(&session_id),
            || {
                format!(
                    "session_id={session_id} retired={:?}",
                    self.retired.lock().unwrap().as_slice()
                )
            },
        )
        .await;
    }

    fn block_next_delivery(&self) -> tokio::sync::oneshot::Sender<Result<(), String>> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *self.blocked_delivery.lock().unwrap() = Some(receiver);
        sender
    }

    fn block_next_resolution(&self) -> tokio::sync::oneshot::Sender<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *self.blocked_resolution.lock().unwrap() = Some(receiver);
        sender
    }

    fn block_next_live_check(&self) -> tokio::sync::oneshot::Sender<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *self.blocked_live_check.lock().unwrap() = Some(receiver);
        sender
    }
}

impl ContextAlertRuntime for ScriptedRuntime {
    fn resolve_member_policy(
        &self,
        session_id: Uuid,
    ) -> BoxFuture<'static, MemberPolicyResolution> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        let resolution = self
            .policies
            .lock()
            .unwrap()
            .get(&session_id)
            .cloned()
            .map(MemberPolicyResolution::Eligible)
            .unwrap_or_else(|| {
                MemberPolicyResolution::PermanentIneligible("missing policy".to_string())
            });
        let blocked = self.blocked_resolution.lock().unwrap().take();
        Box::pin(async move {
            if let Some(blocked) = blocked {
                let _ = blocked.await;
            }
            resolution
        })
    }

    fn check_session_live(&self, _session_id: Uuid) -> BoxFuture<'static, bool> {
        self.live_checks.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        let live = self.live.load(Ordering::SeqCst);
        let blocked = self.blocked_live_check.lock().unwrap().take();
        let cancel = self.cancel_during_live_check.lock().unwrap().take();
        Box::pin(async move {
            if let Some(blocked) = blocked {
                let _ = blocked.await;
            }
            if let Some(cancel) = cancel {
                cancel.cancel();
            }
            live
        })
    }

    fn retire_sample_registration(&self, session_id: Uuid) {
        self.retired.lock().unwrap().push(session_id);
        self.changed.notify_one();
    }

    fn prepare_attempt(
        &self,
        snapshot: AttemptSnapshot,
        cancellation: CancellationToken,
    ) -> BoxFuture<'static, AttemptPreparation> {
        self.prepare_calls.lock().unwrap().push(snapshot.clone());
        self.prepare_live_checks.lock().unwrap().push((
            snapshot.generation,
            snapshot.failure_count,
            self.live_checks.load(Ordering::SeqCst),
        ));
        let policy = self
            .policies
            .lock()
            .unwrap()
            .get(&snapshot.session_id)
            .cloned();
        let target = self.target.clone();
        Box::pin(async move {
            let Some(policy) = policy else {
                return AttemptPreparation::Cancel {
                    reason: "missing policy".to_string(),
                    current_policy: None,
                };
            };
            let thresholds: Vec<u8> = snapshot
                .thresholds
                .iter()
                .copied()
                .filter(|threshold| policy.thresholds.contains(threshold))
                .collect();
            if thresholds.is_empty() {
                return AttemptPreparation::Cancel {
                    reason: "removed".to_string(),
                    current_policy: Some(policy),
                };
            }
            let notice = InternalSystemNotice::for_context_alert(
                snapshot.fingerprint.member.clone(),
                snapshot.fingerprint.workgroup.clone(),
                snapshot.observed,
                thresholds,
            )
            .unwrap();
            AttemptPreparation::Ready(ReadyAttempt {
                snapshot,
                current_policy: policy.thresholds,
                target,
                notice,
                guard: Arc::new(|| Ok(())),
                cancellation,
            })
        })
    }

    fn deliver_attempt(&self, attempt: ReadyAttempt) -> BoxFuture<'static, Result<(), String>> {
        self.deliveries
            .lock()
            .unwrap()
            .push(attempt.snapshot.clone());
        self.changed.notify_one();
        if self.panic_next_delivery.swap(false, Ordering::SeqCst) {
            return Box::pin(async move {
                panic!("scripted delivery panic");
            });
        }
        if let Some(receiver) = self.blocked_delivery.lock().unwrap().take() {
            return Box::pin(async move {
                receiver
                    .await
                    .unwrap_or_else(|_| Err("blocked delivery sender dropped".to_string()))
            });
        }
        let result = self
            .delivery_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()));
        Box::pin(async move { result })
    }
}

async fn wait_until_notified(
    notify: &tokio::sync::Notify,
    label: &str,
    mut predicate: impl FnMut() -> bool,
    diagnostics: impl Fn() -> String,
) {
    let wait = async {
        loop {
            let notified = notify.notified();
            if predicate() {
                return;
            }
            notified.await;
        }
    };
    if tokio::time::timeout(Duration::from_secs(60), wait)
        .await
        .is_err()
    {
        panic!("timed out waiting for {label}: {}", diagnostics());
    }
}

struct IdentityFixture {
    _temp: tempfile::TempDir,
    session: Session,
    team_config: PathBuf,
    ac_root: PathBuf,
    workgroup: PathBuf,
    replica: PathBuf,
    member_matrix: PathBuf,
}

async fn identity_fixture() -> IdentityFixture {
    identity_fixture_named("project-a", "wg-2-dev-team").await
}

async fn identity_fixture_named(project: &str, workgroup_name: &str) -> IdentityFixture {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join(project).join(".ac");
    let workgroup = ac_root.join(workgroup_name);
    let replica = workgroup.join("__agent_member");
    let coordinator_replica = workgroup.join("__agent_coordinator");
    let nested = replica.join("repo-one").join("src");
    let member_matrix = ac_root.join("_agent_member");
    let coordinator_matrix = ac_root.join("_agent_coordinator");
    let team = parse_team_from_workgroup_name(workgroup_name).unwrap();
    let team_dir = ac_root.join(format!("_team_{}", team));
    for path in [
        &nested,
        &coordinator_replica,
        &member_matrix,
        &coordinator_matrix,
        &team_dir,
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_member","context":[],"repos":[]}"#,
    )
    .unwrap();
    std::fs::write(
        coordinator_replica.join("config.json"),
        r#"{"identity":"../../_agent_coordinator","context":[],"repos":[]}"#,
    )
    .unwrap();
    let team_config = team_dir.join("config.json");
    std::fs::write(
        &team_config,
        concat!(
            r#"{"agents":["_agent_member","_agent_coordinator"],"#,
            r#""coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50,75]}"#
        ),
    )
    .unwrap();
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "claude".to_string(),
            Vec::new(),
            nested.to_string_lossy().to_string(),
            Some("claude-profile".to_string()),
            Some("Claude".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    IdentityFixture {
        _temp: temp,
        session,
        team_config,
        ac_root,
        workgroup,
        replica,
        member_matrix,
    }
}

fn fingerprint(id: Uuid) -> MemberIdentityFingerprint {
    MemberIdentityFingerprint {
        session_id: id,
        agent_id: "claude".to_string(),
        working_directory: "cwd".to_string(),
        project_dir: PathBuf::from("project"),
        ac_root: PathBuf::from("workspace"),
        workgroup_dir: PathBuf::from("workgroup"),
        replica_dir: PathBuf::from("replica"),
        matrix_dir: PathBuf::from("matrix"),
        project: "project".to_string(),
        team: "team".to_string(),
        workgroup: "wg-1-team".to_string(),
        member: "member".to_string(),
    }
}

fn eligible(id: Uuid, thresholds: &[u8]) -> MemberPolicyResolution {
    MemberPolicyResolution::Eligible(ResolvedPolicy {
        fingerprint: fingerprint(id),
        thresholds: thresholds.to_vec(),
    })
}
