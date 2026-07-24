use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::sync::atomic::AtomicI32;
#[cfg(any(target_os = "linux", windows))]
use std::sync::atomic::AtomicU64;

#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use uuid::Uuid;

#[cfg(windows)]
use std::path::Path;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, PtyShutdownReport};
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyScreenSnapshot, SessionIoFanout};
use crate::pty::spawn_diagnostics::{self, ChildLiveness, ExitCause, SpawnRecordInit};
#[cfg(target_os = "linux")]
use crate::session::profile::CodingAgentKind;
use crate::telegram::manager::OutputSenderMap;

#[cfg(target_os = "linux")]
fn should_synthesize_local_codex_path(
    coding_agent: Option<CodingAgentKind>,
    configured_env: &[(String, String)],
    env_remove_keys: &[String],
) -> bool {
    coding_agent == Some(CodingAgentKind::Codex)
        && !crate::pty::child_path::has_explicit_linux_path(configured_env, env_remove_keys)
}

#[cfg(all(test, target_os = "linux"))]
mod linux_codex_path_tests {
    use super::should_synthesize_local_codex_path;
    use crate::session::profile::CodingAgentKind;

    #[test]
    fn synthesis_is_identity_gated_and_explicit_path_wins() {
        assert!(should_synthesize_local_codex_path(
            Some(CodingAgentKind::Codex),
            &[],
            &[]
        ));
        for identity in [
            None,
            Some(CodingAgentKind::Claude),
            Some(CodingAgentKind::Gemini),
            Some(CodingAgentKind::Pi),
        ] {
            assert!(!should_synthesize_local_codex_path(identity, &[], &[]));
        }
        assert!(!should_synthesize_local_codex_path(
            Some(CodingAgentKind::Codex),
            &[("PATH".to_string(), String::new())],
            &[]
        ));
        assert!(!should_synthesize_local_codex_path(
            Some(CodingAgentKind::Codex),
            &[],
            &["PATH".to_string()]
        ));
    }
}

struct PtyInstance {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    owner: LocalProcessOwner,
    /// #973 (C) - the size the ConPTY is actually at, so a resize that changes nothing is
    /// not sent to the child. Seeded from the size the PTY was opened at.
    size: Mutex<(u16, u16)>,
    /// #973 (B) - held closed until the child has rendered something. See `StartupGate`.
    startup_gate: Mutex<StartupGate>,
    /// #973 (B) - the same fact as the gate, as one relaxed atomic, so the PTY read loop
    /// can skip the lock on every chunk once the child is up. The gate is the truth; this
    /// is only a fast path, and a stale `false` costs one no-op call.
    rendered: Arc<AtomicBool>,
}

type PortableChild = Box<dyn portable_pty::Child + Send + Sync>;

struct LocalProcessOwner {
    generation: u64,
    root_pid: Option<u32>,
    child: Option<PortableChild>,
    job: Option<crate::pty::job::JobObject>,
    #[cfg(windows)]
    job_required: bool,
    #[cfg(unix)]
    process_group: Option<UnixProcessGroupOwner>,
    #[cfg(unix)]
    process_group_required: bool,
    resource_registration: Option<crate::resource_monitor::ResourceLaunchRegistration>,
    diagnostics: Vec<String>,
}

impl LocalProcessOwner {
    #[cfg(test)]
    fn new(
        child: PortableChild,
        resource_registration: Option<crate::resource_monitor::ResourceLaunchRegistration>,
    ) -> Self {
        Self::new_for_generation(0, child, resource_registration)
    }

    fn new_for_generation(
        generation: u64,
        child: PortableChild,
        resource_registration: Option<crate::resource_monitor::ResourceLaunchRegistration>,
    ) -> Self {
        let root_pid = child.process_id();
        Self {
            generation,
            root_pid,
            child: Some(child),
            job: None,
            #[cfg(windows)]
            job_required: true,
            #[cfg(unix)]
            process_group: None,
            #[cfg(unix)]
            process_group_required: true,
            resource_registration,
            diagnostics: Vec::new(),
        }
    }

    fn push_diagnostic(&mut self, diagnostic: String) {
        if !self.diagnostics.contains(&diagnostic) {
            self.diagnostics.push(diagnostic);
        }
    }
}

enum LocalAttemptState {
    Reserved,
    CancelRequested,
    Detached(LocalProcessOwner),
    Active(PtyInstance),
    TeardownInProgress,
    Terminal,
}

struct LocalAttemptIdentity {
    root_pid: AtomicU32,
    #[cfg(unix)]
    process_group: AtomicI32,
    #[cfg(target_os = "linux")]
    process_start_ticks: AtomicU64,
    #[cfg(windows)]
    job_retained: AtomicBool,
}

impl LocalAttemptIdentity {
    fn new() -> Self {
        Self {
            root_pid: AtomicU32::new(0),
            #[cfg(unix)]
            process_group: AtomicI32::new(0),
            #[cfg(target_os = "linux")]
            process_start_ticks: AtomicU64::new(0),
            #[cfg(windows)]
            job_retained: AtomicBool::new(false),
        }
    }

    fn update(&self, owner: &LocalProcessOwner) {
        self.root_pid
            .store(owner.root_pid.unwrap_or_default(), Ordering::Release);
        #[cfg(unix)]
        if let Some(group) = owner.process_group {
            self.process_group.store(group.leader, Ordering::Release);
            #[cfg(target_os = "linux")]
            self.process_start_ticks.store(
                group.start_time_ticks.unwrap_or_default(),
                Ordering::Release,
            );
        }
        #[cfg(windows)]
        self.job_retained
            .store(owner.job.is_some(), Ordering::Release);
    }

    fn diagnostic(&self, id: Uuid, generation: u64, operation: &str) -> String {
        let root_pid = self.root_pid.load(Ordering::Acquire);
        #[cfg(unix)]
        let group_owner = {
            let group = self.process_group.load(Ordering::Acquire);
            if group == 0 {
                "unverified".to_string()
            } else {
                #[cfg(target_os = "linux")]
                {
                    let start = self.process_start_ticks.load(Ordering::Acquire);
                    format!("{group}@start={start}")
                }
                #[cfg(all(unix, not(target_os = "linux")))]
                {
                    group.to_string()
                }
            }
        };
        #[cfg(windows)]
        let group_owner = if self.job_retained.load(Ordering::Acquire) {
            "job-object-retained".to_string()
        } else {
            "job-object-unverified".to_string()
        };
        format!(
            "session {id} generation {generation} retained local PTY ownership: root_pid={} group_owner={group_owner} syscall={operation}",
            if root_pid == 0 {
                "unavailable".to_string()
            } else {
                root_pid.to_string()
            }
        )
    }
}

struct LocalProcessAttempt {
    id: Uuid,
    generation: u64,
    cancelled: AtomicBool,
    identity: LocalAttemptIdentity,
    state: Mutex<LocalAttemptState>,
    state_changed: Condvar,
}

impl LocalProcessAttempt {
    fn new(id: Uuid, generation: u64) -> Self {
        Self {
            id,
            generation,
            cancelled: AtomicBool::new(false),
            identity: LocalAttemptIdentity::new(),
            state: Mutex::new(LocalAttemptState::Reserved),
            state_changed: Condvar::new(),
        }
    }

    fn diagnostic(&self, operation: &str) -> String {
        self.identity
            .diagnostic(self.id, self.generation, operation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalKillState {
    InProgress,
    Finished,
}

struct LocalKillTombstone {
    state: Mutex<LocalKillState>,
    state_changed: Condvar,
}

impl LocalKillTombstone {
    fn new() -> Self {
        Self {
            state: Mutex::new(LocalKillState::InProgress),
            state_changed: Condvar::new(),
        }
    }

    fn finish(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        *state = LocalKillState::Finished;
        self.state_changed.notify_all();
    }
}

struct LocalSessionOwnership {
    attempts: HashMap<u64, Arc<LocalProcessAttempt>>,
    kill: Option<Arc<LocalKillTombstone>>,
}

impl LocalSessionOwnership {
    fn new(attempt: Arc<LocalProcessAttempt>) -> Self {
        Self {
            attempts: HashMap::from([(attempt.generation, attempt)]),
            kill: None,
        }
    }
}

struct LocalOwnershipRegistry {
    next_generation: u64,
    sessions: HashMap<Uuid, LocalSessionOwnership>,
}

struct LocalOwnershipSet {
    registry: Mutex<LocalOwnershipRegistry>,
    diagnostic_index: Mutex<HashMap<Uuid, Vec<Arc<LocalProcessAttempt>>>>,
}

impl LocalOwnershipSet {
    fn new() -> Self {
        Self {
            registry: Mutex::new(LocalOwnershipRegistry {
                next_generation: 1,
                sessions: HashMap::new(),
            }),
            diagnostic_index: Mutex::new(HashMap::new()),
        }
    }
}

const LOCAL_OWNER_SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);
#[cfg(unix)]
const PROCESS_GROUP_TERM_GRACE: Duration = Duration::from_secs(3);

/// A Unix portable-pty child calls `setsid()` before `exec`, so its PID is also
/// the session and process-group leader. Linux additionally retains the
/// leader's `/proc` start identity, so a recycled numeric PID or PGID is never
/// accepted as the original process group.
#[cfg(unix)]
#[derive(Clone, Copy)]
struct UnixProcessGroupOwner {
    leader: libc::pid_t,
    #[cfg(target_os = "linux")]
    start_time_ticks: Option<u64>,
}

#[cfg(unix)]
impl UnixProcessGroupOwner {
    fn unverified_for_child_pid(pid: Option<u32>) -> Result<Self, AppError> {
        let pid = pid.ok_or_else(|| {
            AppError::PtyError("portable-pty spawned a Unix child without a process id".to_string())
        })?;
        let leader = libc::pid_t::try_from(pid).map_err(|_| {
            AppError::PtyError(format!(
                "portable-pty spawned Unix child pid {pid} outside pid_t range"
            ))
        })?;
        if leader <= 0 {
            return Err(AppError::PtyError(format!(
                "portable-pty spawned invalid Unix child pid {pid}"
            )));
        }
        Ok(Self {
            leader,
            #[cfg(target_os = "linux")]
            start_time_ticks: None,
        })
    }

    fn verify_identity(&mut self) -> Result<(), AppError> {
        #[cfg(target_os = "linux")]
        {
            let stat = read_linux_proc_stat(self.leader)?.ok_or_else(|| {
                AppError::PtyError(format!(
                    "spawned Unix child pid {} exited before process-group identity capture",
                    self.leader
                ))
            })?;
            if stat.process_group != self.leader || stat.session != self.leader {
                return Err(AppError::PtyError(format!(
                    "spawned Unix child pid {} did not own its expected process group and session",
                    self.leader
                )));
            }
            self.start_time_ticks = Some(stat.start_time_ticks);
        }

        #[cfg(all(unix, not(target_os = "linux")))]
        {
            // SAFETY: `leader` is the positive pid captured from portable-pty.
            let process_group = unsafe { libc::getpgid(self.leader) };
            if process_group != self.leader {
                return Err(AppError::PtyError(format!(
                    "spawned Unix child pid {} did not own process group {}",
                    self.leader, process_group
                )));
            }
        }
        Ok(())
    }

    fn exists(self) -> std::io::Result<bool> {
        #[cfg(target_os = "linux")]
        {
            match self.linux_identity_state()? {
                LinuxGroupIdentityState::OriginalLeader
                | LinuxGroupIdentityState::OriginalWithoutLeader => Ok(true),
                LinuxGroupIdentityState::GoneOrReused => Ok(false),
            }
        }

        #[cfg(all(unix, not(target_os = "linux")))]
        {
            numeric_process_group_exists(self.leader)
        }
    }

    fn signal(self, signal: libc::c_int, deadline: Instant) -> std::io::Result<bool> {
        #[cfg(target_os = "linux")]
        {
            signal_linux_group_members(self, signal, deadline)
        }

        #[cfg(all(unix, not(target_os = "linux")))]
        {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Unix process-group signal deadline expired",
                ));
            }
            if !self.exists()? {
                return Ok(false);
            }
            // SAFETY: the existence probe above establishes the retained
            // process group on platforms without Linux pidfds.
            if unsafe { libc::kill(-self.leader, signal) } == 0 {
                return Ok(true);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(false)
            } else {
                Err(error)
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_identity_state(self) -> std::io::Result<LinuxGroupIdentityState> {
        let current = read_linux_proc_stat(self.leader)?;
        let group_exists = numeric_process_group_exists(self.leader)?;
        let Some(expected_start) = self.start_time_ticks else {
            if current.is_none() && !group_exists {
                return Ok(LinuxGroupIdentityState::GoneOrReused);
            }
            return Err(std::io::Error::other(format!(
                "Linux process-group {} has no captured start identity",
                self.leader
            )));
        };
        Ok(classify_linux_group_identity(
            self.leader,
            expected_start,
            current,
            group_exists,
        ))
    }
}

#[cfg(unix)]
fn numeric_process_group_exists(leader: libc::pid_t) -> std::io::Result<bool> {
    if leader <= 0 {
        return Ok(false);
    }
    // SAFETY: signal 0 performs an existence/permission probe only.
    if unsafe { libc::kill(-leader, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error),
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinuxProcStat {
    pid: libc::pid_t,
    state: char,
    process_group: libc::pid_t,
    session: libc::pid_t,
    start_time_ticks: u64,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxGroupIdentityState {
    OriginalLeader,
    OriginalWithoutLeader,
    GoneOrReused,
}

#[cfg(target_os = "linux")]
fn classify_linux_group_identity(
    leader: libc::pid_t,
    expected_start: u64,
    current: Option<LinuxProcStat>,
    group_exists: bool,
) -> LinuxGroupIdentityState {
    match current {
        Some(stat)
            if stat.start_time_ticks == expected_start
                && stat.process_group == leader
                && stat.session == leader =>
        {
            LinuxGroupIdentityState::OriginalLeader
        }
        Some(_) => LinuxGroupIdentityState::GoneOrReused,
        None if group_exists => LinuxGroupIdentityState::OriginalWithoutLeader,
        None => LinuxGroupIdentityState::GoneOrReused,
    }
}

#[cfg(target_os = "linux")]
fn read_linux_proc_stat(pid: libc::pid_t) -> std::io::Result<Option<LinuxProcStat>> {
    if pid <= 0 {
        return Ok(None);
    }
    let path = format!("/proc/{pid}/stat");
    let value = match std::fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    parse_linux_proc_stat(&value).map(Some)
}

#[cfg(target_os = "linux")]
fn parse_linux_proc_stat(value: &str) -> std::io::Result<LinuxProcStat> {
    let open = value
        .find('(')
        .ok_or_else(|| std::io::Error::other("Linux process stat omitted command start"))?;
    let close = value
        .rfind(')')
        .ok_or_else(|| std::io::Error::other("Linux process stat omitted command end"))?;
    if close <= open {
        return Err(std::io::Error::other(
            "Linux process stat had an invalid command field",
        ));
    }
    let pid = value[..open]
        .trim()
        .parse::<libc::pid_t>()
        .map_err(|error| std::io::Error::other(format!("invalid Linux stat pid: {error}")))?;
    let fields = value[close + 1..].split_whitespace().collect::<Vec<_>>();
    let parse_pid_field = |index: usize, name: &str| -> std::io::Result<libc::pid_t> {
        fields
            .get(index)
            .ok_or_else(|| std::io::Error::other(format!("Linux stat omitted {name}")))?
            .parse::<libc::pid_t>()
            .map_err(|error| std::io::Error::other(format!("invalid Linux stat {name}: {error}")))
    };
    let start_time_ticks = fields
        .get(19)
        .ok_or_else(|| std::io::Error::other("Linux stat omitted start time"))?
        .parse::<u64>()
        .map_err(|error| {
            std::io::Error::other(format!("invalid Linux stat start time: {error}"))
        })?;
    Ok(LinuxProcStat {
        pid,
        state: fields
            .first()
            .and_then(|value| value.chars().next())
            .ok_or_else(|| std::io::Error::other("Linux stat omitted process state"))?,
        process_group: parse_pid_field(2, "process group")?,
        session: parse_pid_field(3, "session")?,
        start_time_ticks,
    })
}

#[cfg(target_os = "linux")]
struct LinuxPidfdTarget {
    stat: LinuxProcStat,
    pidfd: OwnedFd,
}

#[cfg(target_os = "linux")]
fn linux_group_members(
    owner: UnixProcessGroupOwner,
    deadline: Instant,
) -> std::io::Result<Vec<LinuxPidfdTarget>> {
    let mut members = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Linux process-group {} enumeration exceeded teardown deadline",
                    owner.leader
                ),
            ));
        }
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        let Some(before) = read_linux_proc_stat(pid)? else {
            continue;
        };
        if before.process_group != owner.leader || before.session != owner.leader {
            continue;
        }
        let Some(pidfd) = open_linux_pidfd(pid)? else {
            continue;
        };
        let Some(after) = read_linux_proc_stat(pid)? else {
            continue;
        };
        if before == after {
            members.push(LinuxPidfdTarget { stat: after, pidfd });
        }
    }
    Ok(members)
}

#[cfg(target_os = "linux")]
fn linux_group_has_live_members(
    owner: UnixProcessGroupOwner,
    deadline: Instant,
) -> std::io::Result<bool> {
    Ok(linux_group_members(owner, deadline)?
        .iter()
        .any(|member| !matches!(member.stat.state, 'Z' | 'X' | 'x')))
}

#[cfg(target_os = "linux")]
fn open_linux_pidfd(pid: libc::pid_t) -> std::io::Result<Option<OwnedFd>> {
    // SAFETY: pidfd_open receives a positive observed pid and flags zero.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw >= 0 {
        // SAFETY: a successful pidfd_open returns a new owned descriptor.
        return Ok(Some(unsafe { OwnedFd::from_raw_fd(raw as libc::c_int) }));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(target_os = "linux")]
fn send_linux_pidfd_signal(target: &LinuxPidfdTarget, signal: libc::c_int) -> std::io::Result<()> {
    // SAFETY: the pidfd is owned and valid, the signal is a standard Unix
    // signal, and the siginfo pointer is null as permitted by pidfd_send_signal.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            target.pidfd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "pidfd signal failed for pid {} start {}: {error}",
            target.stat.pid, target.stat.start_time_ticks
        )))
    }
}

#[cfg(target_os = "linux")]
fn with_original_linux_group<T>(
    state: LinuxGroupIdentityState,
    action: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<Option<T>> {
    match state {
        LinuxGroupIdentityState::OriginalLeader => action().map(Some),
        LinuxGroupIdentityState::OriginalWithoutLeader => Err(std::io::Error::other(
            "original leader identity is no longer observable; refusing numeric process-group signal",
        )),
        LinuxGroupIdentityState::GoneOrReused => Ok(None),
    }
}

#[cfg(target_os = "linux")]
fn signal_linux_group_members(
    owner: UnixProcessGroupOwner,
    signal: libc::c_int,
    deadline: Instant,
) -> std::io::Result<bool> {
    let Some(members) = with_original_linux_group(owner.linux_identity_state()?, || {
        linux_group_members(owner, deadline)
    })?
    else {
        return Ok(false);
    };
    with_original_linux_group(owner.linux_identity_state()?, || {
        if members.is_empty() {
            return Err(std::io::Error::other(format!(
                "Linux process group {} existed but exposed no identity-stable members",
                owner.leader
            )));
        }
        for member in &members {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "Linux process-group {} signaling exceeded teardown deadline",
                        owner.leader
                    ),
                ));
            }
            send_linux_pidfd_signal(member, signal)?;
        }
        Ok(())
    })
    .map(|result| result.is_some())
}

#[cfg(all(test, target_os = "linux"))]
mod linux_process_identity_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn reused_leader_identity_never_authorizes_a_signal() {
        let leader = 300;
        let expected_start = 41;
        let reused = LinuxProcStat {
            pid: leader,
            state: 'S',
            process_group: leader,
            session: leader,
            start_time_ticks: 42,
        };
        let state = classify_linux_group_identity(leader, expected_start, Some(reused), true);
        assert_eq!(state, LinuxGroupIdentityState::GoneOrReused);
        let signaled = AtomicBool::new(false);
        assert!(with_original_linux_group(state, || {
            signaled.store(true, Ordering::SeqCst);
            Ok(())
        })
        .expect("reused identity is a terminal no-op")
        .is_none());
        assert!(!signaled.load(Ordering::SeqCst));
    }

    #[test]
    fn leaderless_numeric_group_never_authorizes_a_signal() {
        let signaled = AtomicBool::new(false);
        let error =
            with_original_linux_group(LinuxGroupIdentityState::OriginalWithoutLeader, || {
                signaled.store(true, Ordering::SeqCst);
                Ok(())
            })
            .expect_err("a leaderless numeric group is not identity-qualified");
        assert!(error
            .to_string()
            .contains("original leader identity is no longer observable"));
        assert!(!signaled.load(Ordering::SeqCst));
    }

    #[test]
    fn naturally_absent_original_group_is_terminal() {
        assert_eq!(
            classify_linux_group_identity(300, 41, None, false),
            LinuxGroupIdentityState::GoneOrReused
        );
    }

    #[test]
    fn proc_stat_parser_handles_spaces_and_parentheses_in_command() {
        let parsed = parse_linux_proc_stat(
            "17 (name with ) parenthesis) S 1 17 17 0 -1 0 0 0 0 0 0 0 0 0 0 0 0 0 4242 0",
        )
        .expect("parse injected proc stat");
        assert_eq!(parsed.pid, 17);
        assert_eq!(parsed.state, 'S');
        assert_eq!(parsed.process_group, 17);
        assert_eq!(parsed.session, 17);
        assert_eq!(parsed.start_time_ticks, 4242);
    }
}

/// #973 (B) - a PTY resize that lands while a coding agent's TUI is starting up makes it
/// redraw its still-empty viewport and lose the wakeup for the content that becomes ready
/// right after. The terminal stays blank, the process stays alive, and any keypress paints
/// it. Measured on a bare ConPTY: a resize in that window blanks Codex 8 times in 10.
///
/// So AC does not resize a child that has not rendered anything yet. Resizes are held, the
/// LAST one is kept, and it is applied the moment the child paints - which is the earliest
/// moment it is safe to.
///
/// **Why the trigger is "has rendered" and not a timer.** The danger window is
/// [TUI is up, content is ready], and both ends move with machine load: in production
/// first-paint spans 324 ms to 2163 ms, while on an idle box the window sits at 180-300 ms.
/// Any fixed delay is a constant fitted to one machine, and on a slower one it lands
/// *inside* the window - a fix that reproduces the bug. A render, by contrast, cannot happen
/// inside the window, because the window is defined as the interval in which the child has
/// rendered nothing. The trigger is immune to load, terminal height and frame count.
///
/// **That argument only holds if the trigger actually tests rendering, so it does.** It asks the
/// real, stateful vt100 parser - the one `handle_output` already feeds every byte of every chunk -
/// whether the screen now holds a cell a human could see:
/// `SessionIoFanout::has_rendered_visible_content`.
///
/// It is worth saying what it must NOT be, because the first version of this gate got it wrong and
/// the mistake was invisible. It used the idle detector's text predicate
/// (`output_has_printable_activity`), which asks whether a printable byte survives a STATELESS,
/// per-chunk escape stripper. That is a different question, and it answers this one wrong twice:
/// a chunk that ends mid-CSI or mid-OSC hands the tail to the next call with no leading `ESC`
/// (`1049h`, `2J` and `cmd.exe` are all printable, and conhost really does split its writes), and
/// a three-byte charset designator like `ESC ( B` - which ncurses and half the TUI world emit on
/// the way up - outran a stripper that consumed `ESC` plus one char. Either one opens the gate on
/// a child that has painted nothing, which is precisely the bug the gate exists to prevent, and
/// it would have failed silently and intermittently inside an already intermittent bug. A parser
/// carries state across reads and knows what an escape is, so both are closed by construction
/// rather than by another special case.
///
/// **Why not #942's paint floor.** That floor is 256 bytes and the blank child sits at 345:
/// it would fire for a child that is hung.
///
/// **Why there is no timeout escape.** A child that never renders is showing nothing, so a
/// resize it never receives changes nothing a user can see - and the instant it does render,
/// the size it should have had is applied. A timeout would be the very constant this design
/// exists to avoid. Shells are unaffected: a prompt is printable, so their gate opens on the
/// first chunk.
enum StartupGate {
    /// The child has not rendered yet. Resizes are held here, last one wins.
    Holding(Option<(u16, u16)>),
    /// The child has rendered. Resizes go straight through, forever.
    Open,
}

/// #973 - decide what to do with a resize the view asked for, and do it. This is the whole
/// of B and C, and it is a free function over the instance so it can be tested against a
/// real ConPTY: the backend itself cannot be built in a unit test, because its `GitWatcher`
/// needs a Tauri `AppHandle`, and none of that has anything to do with resizing a PTY.
///
/// Returns whether the ConPTY was actually resized.
fn resize_instance(
    instance: &PtyInstance,
    id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<bool, AppError> {
    // #973 - refuse a degenerate size FIRST, ahead of the gate.
    //
    // The gate is last-wins (`StartupGate::on_resize`), so a `0x0` reaching it OVERWRITES the
    // real size the view is waiting to give the child. The hand-over then pops the `0x0`,
    // `send_size_to_conpty` refuses it, the pending slot is consumed, and nothing retries: the
    // ConPTY is wedged at the size it was opened at for good. On cold start that is a 120x30
    // child behind a 74x23 terminal. A size that must never reach the child must never be
    // allowed to displace one that must.
    //
    // Where a zero comes from, since we got this wrong once: NOT from xterm. `fit()` clamps to
    // its own MINIMUM_COLS = 2 / MINIMUM_ROWS = 1 (`@xterm/addon-fit`), and `CoreTerminal.resize`
    // rejects NaN and clamps again - the worst it can produce is NaN, which the frontend's
    // `Number.isInteger` check catches. The zero comes off the WIRE: `pty_resize` on the web
    // transport takes cols/rows straight from a JSON payload (`web/commands.rs`), and a client
    // that is not xterm, or is simply broken, can put a 0 in it.
    if cols == 0 || rows == 0 {
        log::warn!("[pty] refusing degenerate resize {id} to {cols}x{rows} (#973)");
        return Ok(false);
    }

    // B - do not resize a child that has not rendered anything yet. The view fires its fit
    // 300-500 ms after spawn, which is exactly when a coding agent's TUI is coming up, and a
    // resize there costs it its first content render. Hold the size; the read loop hands it
    // over the moment the child paints. See `StartupGate`.
    {
        let mut gate = instance
            .startup_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if gate.on_resize(cols, rows).is_none() {
            log::debug!(
                "[pty] resize {id} to {cols}x{rows} held: the child has not rendered yet (#973)"
            );
            return Ok(false);
        }
    }
    send_size_to_conpty(instance, id, cols, rows)
}

/// #973 (C) - do not tell the child about a resize that is not a resize.
///
/// ConPTY delivers even a same-size `ResizePseudoConsole` to the client as a real event, and
/// the view fires 5-20 identical resizes per attach (`TerminalView.tsx:85-95`, a double
/// `requestAnimationFrame` plus a `ResizeObserver`), so today every attach shakes the child
/// for nothing.
///
/// The cached size is written only AFTER the ConPTY has accepted the new one: if the resize
/// failed and we recorded it anyway, every retry would be skipped as a no-op and the PTY
/// would be wedged at the wrong size forever.
fn send_size_to_conpty(
    instance: &PtyInstance,
    id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<bool, AppError> {
    // #973 - refuse a degenerate size. Defence in depth: the request path is guarded ahead of
    // the gate in `resize_instance`, and this is the last line before `master.resize()`, which
    // is also what `hand_over_held_size` calls with a size that has been sitting in the gate.
    //
    // It has to be refused SOMEWHERE, because ConPTY does not refuse it: `master.resize(0x0)`
    // returns Ok and the child is left with no screen at all. A stale size is recoverable; a
    // zero-column terminal is not. The zero itself comes off the wire, not from xterm - see
    // `resize_instance`.
    if cols == 0 || rows == 0 {
        log::warn!("[pty] refusing degenerate resize {id} to {cols}x{rows} (#973)");
        return Ok(false);
    }

    if !instance.size_changed(cols, rows) {
        log::debug!("[pty] resize {id} skipped: already {cols}x{rows} (#973)");
        return Ok(false);
    }

    {
        let master = instance.master.lock().unwrap_or_else(|e| e.into_inner());
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::PtyError(e.to_string()))?;
    }
    instance.remember_size(cols, rows);
    Ok(true)
}

/// #973 (B) - the child has rendered something, so the startup window is behind us. Open the
/// gate for good and hand the ConPTY the size the view has been waiting to give it.
///
/// Returns the size the ConPTY ACTUALLY took, so the caller can bring the vt100 screen along
/// with it - and `None` if nothing was held, or the held size was refused, or it failed.
///
/// Free over the instance, like `resize_instance`, so the hand-over can be driven by a test
/// against a real ConPTY: `open_startup_gate`, its only caller, needs a `LocalProcessBackend`,
/// whose `GitWatcher` needs a Tauri `AppHandle`. A test that re-implemented these lines rather
/// than calling them would not be testing this code.
fn hand_over_held_size(instance: &PtyInstance, id: Uuid) -> Option<(u16, u16)> {
    // One lock covers both "take the held size" and "open the gate", so a resize landing at
    // this exact instant cannot be recorded into a gate that is already open and then dropped
    // on the floor, leaving the terminal stuck at the wrong size.
    let pending = {
        let mut gate = instance
            .startup_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        gate.open()
    };
    // Only a fast path for the read loop; the gate above is the truth. Publishing it late is
    // safe: a stale `false` costs one extra no-op call to this function.
    instance.rendered.store(true, Ordering::Relaxed);

    let (cols, rows) = pending?;

    match send_size_to_conpty(instance, id, cols, rows) {
        Ok(true) => {
            log::info!(
                "[pty] startup gate open for {id}: applied the held resize {cols}x{rows} (#973)"
            );
            Some((cols, rows))
        }
        Ok(false) => None,
        Err(e) => {
            // Non-critical: the child is up and painting, it is just the wrong size.
            log::warn!("[pty] held resize {id} to {cols}x{rows} failed: {e} (#973)");
            None
        }
    }
}

impl StartupGate {
    /// The view asked for a resize. Returns the size to hand the ConPTY, or `None` if the
    /// child is still starting up and it must be held.
    fn on_resize(&mut self, cols: u16, rows: u16) -> Option<(u16, u16)> {
        match self {
            StartupGate::Holding(pending) => {
                // Only the last size matters: the frontend fires 5-20 of these per attach.
                *pending = Some((cols, rows));
                None
            }
            StartupGate::Open => Some((cols, rows)),
        }
    }

    /// The child rendered. Open the gate for good and hand back the size that was held.
    fn open(&mut self) -> Option<(u16, u16)> {
        match std::mem::replace(self, StartupGate::Open) {
            StartupGate::Holding(pending) => pending,
            StartupGate::Open => None,
        }
    }
}

impl PtyInstance {
    /// #973 (C) - has the size actually moved?
    ///
    /// The frontend calls resize unconditionally (`TerminalView.tsx:85-95`, from a double
    /// `requestAnimationFrame` plus a `ResizeObserver`), so one attach fires 5-20 identical
    /// resizes, and ConPTY hands every one of them to the child as a real event.
    fn size_changed(&self, cols: u16, rows: u16) -> bool {
        Self::size_changed_in(&self.size, cols, rows)
    }

    /// Only after the ConPTY has actually accepted it: if `resize` failed, the cached size
    /// must stay stale so the next attempt is not skipped as a no-op and the PTY is not
    /// wedged at the wrong size forever.
    fn remember_size(&self, cols: u16, rows: u16) {
        Self::remember_size_in(&self.size, cols, rows);
    }

    // The two free functions above are the whole of C. Split out so they can be tested
    // without a live ConPTY: a `PtyInstance` owns real `MasterPty` handles.
    fn size_changed_in(size: &Mutex<(u16, u16)>, cols: u16, rows: u16) -> bool {
        *size.lock().unwrap_or_else(|e| e.into_inner()) != (cols, rows)
    }

    fn remember_size_in(size: &Mutex<(u16, u16)>, cols: u16, rows: u16) {
        *size.lock().unwrap_or_else(|e| e.into_inner()) = (cols, rows);
    }
}

#[cfg(windows)]
struct GitGuardEnv {
    path: String,
    pathext: String,
    real_git: String,
}

#[cfg(windows)]
static GIT_GUARD_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
const GIT_GUARD_PUBLISH_RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
    Duration::from_millis(1600),
];

#[cfg(windows)]
fn resolve_real_git_path() -> Option<String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = std::process::Command::new("where.exe");
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    cmd.arg("git.exe").creation_flags(CREATE_NO_WINDOW);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string())
}

#[cfg(windows)]
fn ensure_git_guard_wrapper() -> Result<std::path::PathBuf, AppError> {
    let config_dir = crate::config::config_dir()
        .ok_or_else(|| AppError::Other("Could not resolve app config directory".to_string()))?;
    let guard_dir = config_dir.join("git-guard");
    std::fs::create_dir_all(&guard_dir)
        .map_err(|e| AppError::Other(format!("Failed to create git-guard dir: {}", e)))?;

    let cmd_path = guard_dir.join("git.cmd");
    let ps1_path = guard_dir.join("git-guard.ps1");

    let cmd_content = "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0git-guard.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n";
    let ps1_content = r#"$ErrorActionPreference = 'Stop'
$realGit = $env:AC_REAL_GIT
if ([string]::IsNullOrWhiteSpace($realGit)) {
  Write-Error 'AgentsCommander git guard: AC_REAL_GIT is not set.'
  exit 1
}

$originalArgs = @($args)
$target = (Get-Location).Path

for ($i = 0; $i -lt $originalArgs.Count; $i++) {
  $arg = [string]$originalArgs[$i]
  if ($arg -eq '-C') {
    if ($i + 1 -ge $originalArgs.Count) {
      Write-Error 'AgentsCommander git guard: missing path after -C.'
      exit 1
    }

    $next = [string]$originalArgs[$i + 1]
    if ([System.IO.Path]::IsPathRooted($next)) {
      $target = [System.IO.Path]::GetFullPath($next)
    } else {
      $target = [System.IO.Path]::GetFullPath((Join-Path -Path $target -ChildPath $next))
    }
    $i++
    continue
  }

  if ($arg -eq '--git-dir' -or $arg -like '--git-dir=*' -or $arg -eq '--work-tree' -or $arg -like '--work-tree=*') {
    Write-Error 'AgentsCommander git guard: --git-dir and --work-tree are not allowed in agent sessions.'
    exit 1
  }
}

function Test-AllowedGitTarget([string]$path) {
  try {
    $current = [System.IO.Path]::GetFullPath($path)
  } catch {
    return $false
  }

  while ($true) {
    $name = [System.IO.Path]::GetFileName($current)
    if ($name -like 'repo-*') {
      return $true
    }

    $parent = Split-Path -Path $current -Parent
    if ([string]::IsNullOrWhiteSpace($parent) -or $parent -eq $current) {
      break
    }
    $current = $parent
  }

  return $false
}

if (-not (Test-AllowedGitTarget $target)) {
  Write-Error ('AgentsCommander git guard: git is only allowed inside repo-* directories. Target path: ' + $target)
  exit 1
}

& $realGit @originalArgs
exit $LASTEXITCODE
"#;

    write_git_guard_file_if_changed(&cmd_path, cmd_content)
        .map_err(|e| AppError::Other(format!("Failed to write git.cmd guard: {}", e)))?;
    write_git_guard_file_if_changed(&ps1_path, ps1_content)
        .map_err(|e| AppError::Other(format!("Failed to write git-guard.ps1: {}", e)))?;

    Ok(guard_dir)
}

#[cfg(windows)]
fn write_git_guard_file_if_changed(path: &Path, content: &str) -> Result<(), String> {
    let desired = content.as_bytes();
    match std::fs::read(path) {
        Ok(existing) if existing == desired => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read existing {}: {}", path.display(), e)),
    }

    write_git_guard_file_atomic(path, desired)
}

#[cfg(windows)]
fn write_git_guard_file_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("target {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("git-guard");
    let counter = GIT_GUARD_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{name}.{}.{counter}.tmp", std::process::id()));

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("create temp {}: {}", temp.display(), e))?;

    if let Err(e) = file.write_all(content) {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write temp {}: {}", temp.display(), e));
    }
    if let Err(e) = file.flush() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("flush temp {}: {}", temp.display(), e));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("sync temp {}: {}", temp.display(), e));
    }
    drop(file);

    publish_git_guard_temp_with_retry(&temp, path, content)
}

#[cfg(windows)]
fn publish_git_guard_temp_with_retry(
    temp: &Path,
    path: &Path,
    content: &[u8],
) -> Result<(), String> {
    let attempts = GIT_GUARD_PUBLISH_RETRY_DELAYS
        .iter()
        .copied()
        .map(Some)
        .chain(std::iter::once(None));

    for (attempt, delay) in attempts.enumerate() {
        if git_guard_file_matches(path, content) {
            let _ = std::fs::remove_file(temp);
            return Ok(());
        }

        match crate::config::root_agent::atomic_replace_existing(temp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if git_guard_file_matches(path, content) {
                    let _ = std::fs::remove_file(temp);
                    return Ok(());
                }

                let Some(delay) = delay else {
                    let _ = std::fs::remove_file(temp);
                    return Err(format!(
                        "publish {} from {} failed after {} attempts: {}",
                        path.display(),
                        temp.display(),
                        attempt + 1,
                        e
                    ));
                };
                std::thread::sleep(delay);
            }
        }
    }

    unreachable!("retry loop includes a final no-delay attempt")
}

#[cfg(windows)]
fn git_guard_file_matches(path: &Path, content: &[u8]) -> bool {
    match std::fs::read(path) {
        Ok(existing) => existing == content,
        Err(_) => false,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::{Arc, Barrier};
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    fn assert_no_temp_files(dir: &Path) {
        for entry in fs::read_dir(dir).expect("read temp dir") {
            let name = entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string();
            assert!(!name.ends_with(".tmp"), "unexpected temp file: {name}");
        }
    }

    #[allow(clippy::permissions_set_readonly_false)]
    #[test]
    fn git_guard_writer_skips_unchanged_readonly_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("git-guard.ps1");
        fs::write(&path, "same").expect("write fixture");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).expect("set readonly");

        let result = write_git_guard_file_if_changed(&path, "same");

        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).expect("clear readonly");

        result.expect("unchanged readonly file should be skipped");
        assert_eq!(fs::read_to_string(&path).expect("read file"), "same");
        assert_no_temp_files(dir.path());
    }

    #[test]
    fn git_guard_writer_replaces_changed_file_without_temp_leftover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("git.cmd");
        fs::write(&path, "old").expect("write fixture");

        write_git_guard_file_if_changed(&path, "new").expect("replace changed file");

        assert_eq!(fs::read_to_string(&path).expect("read file"), "new");
        assert_no_temp_files(dir.path());
    }

    #[test]
    fn git_guard_writer_retries_in_use_destination_until_released() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("git.cmd");
        fs::write(&path, "old").expect("write fixture");
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .expect("open destination without delete sharing");
        let barrier = Arc::new(Barrier::new(2));
        let writer_barrier = Arc::clone(&barrier);
        let writer_path = path.clone();

        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            write_git_guard_file_if_changed(&writer_path, "new")
        });

        barrier.wait();
        std::thread::sleep(Duration::from_millis(1000));
        drop(held);

        writer
            .join()
            .expect("writer thread")
            .expect("retry should publish after held handle is released");
        assert_eq!(fs::read_to_string(&path).expect("read file"), "new");
        assert_no_temp_files(dir.path());
    }
}

#[cfg(windows)]
fn build_git_guard_env() -> Result<Option<GitGuardEnv>, AppError> {
    let Some(real_git) = resolve_real_git_path() else {
        log::warn!("[pty] git.exe not found; skipping PATH git guard wrapper");
        return Ok(None);
    };

    let guard_dir = ensure_git_guard_wrapper()?;
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let mut path_entries: Vec<std::path::PathBuf> = vec![guard_dir];
    path_entries.extend(std::env::split_paths(&current_path));
    let path = std::env::join_paths(path_entries.iter())
        .map_err(|e| AppError::Other(format!("Failed to join PATH for git guard: {}", e)))?
        .to_string_lossy()
        .to_string();

    Ok(Some(GitGuardEnv {
        path,
        pathext: ".CMD;.BAT;.COM;.EXE".to_string(),
        real_git,
    }))
}

pub struct LocalProcessBackend<R: tauri::Runtime = tauri::Wry> {
    ownership: Arc<LocalOwnershipSet>,
    fanout: SessionIoFanout,
    git_watcher: Arc<GitWatcher<R>>,
}

#[cfg(all(test, windows))]
fn write_to_local_pty(
    ptys: &Mutex<HashMap<Uuid, PtyInstance>>,
    id: Uuid,
    data: &[u8],
) -> Result<(), AppError> {
    let writer = {
        let ptys = ptys.lock().unwrap_or_else(|error| error.into_inner());
        let instance = ptys
            .get(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        Arc::clone(&instance.writer)
    };

    // The global PTY map is released before a potentially blocking pipe
    // write. Teardown can remove and terminate the child to unblock it.
    let mut writer = writer.lock().unwrap_or_else(|error| error.into_inner());
    writer
        .write_all(data)
        .map_err(|error| AppError::PtyError(error.to_string()))?;
    writer
        .flush()
        .map_err(|error| AppError::PtyError(error.to_string()))
}

#[cfg(all(test, windows))]
fn remove_local_pty(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) -> Option<PtyInstance> {
    ptys.lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id)
}

enum RetainedLocalProcess {
    Instance(PtyInstance),
    Detached(LocalProcessOwner),
}

impl RetainedLocalProcess {
    fn owner_mut(&mut self) -> &mut LocalProcessOwner {
        match self {
            Self::Instance(instance) => &mut instance.owner,
            Self::Detached(owner) => owner,
        }
    }
}

fn poll_local_root(id: Uuid, owner: &mut LocalProcessOwner) {
    let Some(child) = owner.child.as_mut() else {
        return;
    };
    match probe_child_contained(child) {
        ChildLiveness::Exited { code, success } => {
            log::info!(
                "[pty] reaped session {} local PTY root pid {:?}: code={} success={}",
                id,
                owner.root_pid,
                code,
                success
            );
            owner.child = None;
        }
        ChildLiveness::Gone => {
            owner.child = None;
        }
        ChildLiveness::Alive => {}
        ChildLiveness::Unqueryable(error) => owner.push_diagnostic(format!(
            "session {id} root pid {:?} poll failed: {error}",
            owner.root_pid
        )),
    }
}

fn poll_local_root_for_shutdown(id: Uuid, owner: &mut LocalProcessOwner, _deadline: Instant) {
    #[cfg(target_os = "linux")]
    if owner.child.is_some() {
        if let Some(group) = owner.process_group {
            match group.linux_identity_state() {
                Ok(LinuxGroupIdentityState::OriginalLeader) => {
                    match linux_group_has_live_members(group, _deadline) {
                        Ok(true) => return,
                        Ok(false) => {}
                        Err(error) => {
                            owner.push_diagnostic(format!(
                                "session {id} process group {} live-member probe failed: {error}",
                                group.leader
                            ));
                            return;
                        }
                    }
                }
                Ok(
                    LinuxGroupIdentityState::OriginalWithoutLeader
                    | LinuxGroupIdentityState::GoneOrReused,
                ) => {}
                Err(error) => {
                    owner.push_diagnostic(format!(
                        "session {id} process group {} root-reap identity probe failed: {error}",
                        group.leader
                    ));
                    return;
                }
            }
        }
    }

    poll_local_root(id, owner);
}

fn local_process_tree_absent(id: Uuid, owner: &mut LocalProcessOwner, deadline: Instant) -> bool {
    poll_local_root_for_shutdown(id, owner, deadline);
    let root_absent = owner.child.is_none();

    #[cfg(unix)]
    let group_absent = match owner.process_group {
        Some(group) => match group.exists() {
            Ok(false) => {
                owner.process_group_required = false;
                true
            }
            Ok(true) => false,
            Err(error) => {
                owner.push_diagnostic(format!(
                    "session {id} process group {} probe failed: {error}",
                    group.leader
                ));
                false
            }
        },
        None if owner.process_group_required => {
            owner.push_diagnostic(format!(
                "session {id} root pid {:?} has no verified Unix process-group owner",
                owner.root_pid
            ));
            false
        }
        None => true,
    };

    #[cfg(windows)]
    let group_absent = match owner.job.as_ref() {
        Some(job) => match job.is_empty() {
            Ok(true) => {
                owner.job_required = false;
                true
            }
            Ok(false) => false,
            Err(error) => {
                owner.push_diagnostic(format!(
                    "session {id} root pid {:?} Job Object probe failed: {error}",
                    owner.root_pid
                ));
                false
            }
        },
        None if owner.job_required => {
            owner.push_diagnostic(format!(
                "session {id} root pid {:?} has no required Windows Job Object",
                owner.root_pid
            ));
            false
        }
        None => true,
    };

    root_absent && group_absent
}

fn request_child_kill(id: Uuid, owner: &mut LocalProcessOwner, phase: &str) {
    let Some(child) = owner.child.as_mut() else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| child.kill()));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => owner.push_diagnostic(format!(
            "session {id} root pid {:?} {phase} kill failed: {error}",
            owner.root_pid
        )),
        Err(_) => owner.push_diagnostic(format!(
            "session {id} root pid {:?} {phase} kill panicked inside portable-pty",
            owner.root_pid
        )),
    }
}

#[cfg(unix)]
fn request_unix_group_signal(
    id: Uuid,
    owner: &mut LocalProcessOwner,
    signal: libc::c_int,
    phase: &str,
    deadline: Instant,
) -> bool {
    let Some(group) = owner.process_group else {
        owner.push_diagnostic(format!(
            "session {id} root pid {:?} has no Unix group for {phase}",
            owner.root_pid
        ));
        return false;
    };
    match group.signal(signal, deadline) {
        Ok(true) => true,
        Ok(false) => {
            owner.process_group_required = false;
            true
        }
        Err(error) => {
            owner.push_diagnostic(format!(
                "session {id} process group {} {phase} signal failed: {error}",
                group.leader
            ));
            false
        }
    }
}

#[cfg(windows)]
fn request_windows_job_termination(id: Uuid, owner: &mut LocalProcessOwner, phase: &str) -> bool {
    let Some(job) = owner.job.as_ref() else {
        owner.push_diagnostic(format!(
            "session {id} root pid {:?} has no Windows Job Object for {phase}",
            owner.root_pid
        ));
        return false;
    };
    match job.terminate_checked() {
        Ok(()) => true,
        Err(error) => {
            owner.push_diagnostic(format!(
                "session {id} root pid {:?} Job Object {phase} failed: {error}",
                owner.root_pid
            ));
            false
        }
    }
}

fn poll_local_processes_until(entries: &mut [(Uuid, RetainedLocalProcess)], deadline: Instant) {
    loop {
        let mut all_absent = true;
        for (id, retained) in entries.iter_mut() {
            if Instant::now() >= deadline
                || !local_process_tree_absent(*id, retained.owner_mut(), deadline)
            {
                all_absent = false;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        if all_absent || Instant::now() >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(Duration::from_millis(10).min(remaining));
    }
}

fn reconcile_resource_registration(
    id: Uuid,
    owner: &mut LocalProcessOwner,
    deadline: Instant,
) -> bool {
    let Some(registration) = owner.resource_registration.as_mut() else {
        return true;
    };
    if Instant::now() >= deadline {
        owner.push_diagnostic(format!(
            "session {id} generation {} resource ownership reconciliation skipped because the absolute teardown deadline expired",
            owner.generation
        ));
        return false;
    }
    match registration.rollback_registered_until(deadline) {
        Ok(None) => {
            owner.resource_registration = None;
            true
        }
        Ok(Some(result))
            if result.state == crate::resource_monitor::types::ResourceGroupState::Terminated =>
        {
            owner.resource_registration = None;
            true
        }
        Ok(Some(result)) => {
            owner.push_diagnostic(format!(
                "session {id} resource ownership remained {:?}: {}",
                result.state, result.message
            ));
            false
        }
        Err(error) => {
            owner.push_diagnostic(format!(
                "session {id} generation {} resource ownership reconciliation failed: {error}",
                owner.generation
            ));
            false
        }
    }
}

fn retained_owner_diagnostic(id: Uuid, owner: &LocalProcessOwner, budget: Duration) -> String {
    #[cfg(unix)]
    let process_group = match owner.process_group {
        Some(group) => {
            #[cfg(target_os = "linux")]
            {
                format!("{}@start={:?}", group.leader, group.start_time_ticks)
            }
            #[cfg(all(unix, not(target_os = "linux")))]
            {
                group.leader.to_string()
            }
        }
        None if owner.process_group_required => "missing-required".to_string(),
        None => "absent".to_string(),
    };
    #[cfg(windows)]
    let process_group = if owner.job.is_some() {
        "job-object-retained".to_string()
    } else {
        "no-job-object".to_string()
    };

    let details = if owner.diagnostics.is_empty() {
        "no syscall diagnostic was available".to_string()
    } else {
        owner.diagnostics.join(" | ")
    };
    format!(
        "session {id} generation {} retained local PTY ownership after {}ms: root_pid={:?} root_handle={} group_owner={process_group}; {details}",
        owner.generation,
        budget.as_millis(),
        owner.root_pid,
        if owner.child.is_some() {
            "retained"
        } else {
            "reaped"
        }
    )
}

#[cfg(test)]
fn shutdown_local_processes(
    entries: &mut [(Uuid, RetainedLocalProcess)],
    budget: Duration,
) -> Vec<Option<String>> {
    let started = Instant::now();
    let deadline = started.checked_add(budget).unwrap_or(started);
    shutdown_local_processes_until(entries, budget, started, deadline)
}

fn shutdown_local_processes_until(
    entries: &mut [(Uuid, RetainedLocalProcess)],
    budget: Duration,
    _started: Instant,
    deadline: Instant,
) -> Vec<Option<String>> {
    #[cfg(unix)]
    {
        for (id, retained) in entries.iter_mut() {
            if Instant::now() >= deadline {
                break;
            }
            let owner = retained.owner_mut();
            if !request_unix_group_signal(*id, owner, libc::SIGTERM, "SIGTERM", deadline) {
                request_child_kill(*id, owner, "SIGTERM fallback");
            }
        }
        let term_deadline = (_started + PROCESS_GROUP_TERM_GRACE).min(deadline);
        poll_local_processes_until(entries, term_deadline);
        for (id, retained) in entries.iter_mut() {
            if Instant::now() >= deadline {
                break;
            }
            let owner = retained.owner_mut();
            if !local_process_tree_absent(*id, owner, deadline) {
                if !request_unix_group_signal(*id, owner, libc::SIGKILL, "SIGKILL", deadline) {
                    request_child_kill(*id, owner, "SIGKILL fallback");
                } else {
                    request_child_kill(*id, owner, "SIGKILL root fallback");
                }
            }
        }
    }

    #[cfg(windows)]
    {
        for (id, retained) in entries.iter_mut() {
            if Instant::now() >= deadline {
                break;
            }
            let owner = retained.owner_mut();
            if !request_windows_job_termination(*id, owner, "termination") {
                request_child_kill(*id, owner, "termination fallback");
            }
        }
    }

    poll_local_processes_until(entries, deadline);

    entries
        .iter_mut()
        .map(|(id, retained)| {
            let owner = retained.owner_mut();
            if Instant::now() < deadline
                && local_process_tree_absent(*id, owner, deadline)
                && Instant::now() < deadline
                && reconcile_resource_registration(*id, owner, deadline)
            {
                None
            } else {
                if Instant::now() >= deadline {
                    owner.push_diagnostic(format!(
                        "session {id} local PTY teardown deadline expired after {}ms",
                        budget.as_millis()
                    ));
                }
                Some(retained_owner_diagnostic(*id, owner, budget))
            }
        })
        .collect()
}

fn lock_local_owner_map_until<T>(
    owner: &Mutex<T>,
    deadline: Instant,
) -> Result<std::sync::MutexGuard<'_, T>, ()> {
    loop {
        match owner.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(Duration::from_millis(2).min(remaining));
            }
            Err(std::sync::TryLockError::WouldBlock) => return Err(()),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpawnFailurePoint {
    TakeWriter,
    CloneReader,
    PostSpawnCwdVerification,
    RouteRegistration,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpawnPausePoint {
    BeforeSpawn,
    AfterChildCreation,
    AfterRegistration,
    AfterPublication,
}

#[cfg(test)]
struct SpawnFailureInjection {
    point: SpawnFailurePoint,
    descendant_marker: std::path::PathBuf,
}

#[cfg(test)]
fn spawn_failure_injections() -> &'static Mutex<HashMap<Uuid, SpawnFailureInjection>> {
    static INJECTIONS: std::sync::OnceLock<Mutex<HashMap<Uuid, SpawnFailureInjection>>> =
        std::sync::OnceLock::new();
    INJECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
struct SpawnPauseInjection {
    point: SpawnPausePoint,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
fn spawn_pause_injections() -> &'static Mutex<HashMap<Uuid, SpawnPauseInjection>> {
    static INJECTIONS: std::sync::OnceLock<Mutex<HashMap<Uuid, SpawnPauseInjection>>> =
        std::sync::OnceLock::new();
    INJECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn duplicate_reservation_injections() -> &'static Mutex<std::collections::HashSet<Uuid>> {
    static INJECTIONS: std::sync::OnceLock<Mutex<std::collections::HashSet<Uuid>>> =
        std::sync::OnceLock::new();
    INJECTIONS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[cfg(test)]
fn reservation_failure_injections() -> &'static Mutex<std::collections::HashSet<Uuid>> {
    static INJECTIONS: std::sync::OnceLock<Mutex<std::collections::HashSet<Uuid>>> =
        std::sync::OnceLock::new();
    INJECTIONS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[cfg(test)]
fn concurrent_kill_wait_observers() -> &'static Mutex<HashMap<Uuid, std::sync::mpsc::SyncSender<()>>>
{
    static OBSERVERS: std::sync::OnceLock<Mutex<HashMap<Uuid, std::sync::mpsc::SyncSender<()>>>> =
        std::sync::OnceLock::new();
    OBSERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) fn inject_spawn_failure(
    id: Uuid,
    point: SpawnFailurePoint,
    descendant_marker: std::path::PathBuf,
) {
    spawn_failure_injections()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            id,
            SpawnFailureInjection {
                point,
                descendant_marker,
            },
        );
}

#[cfg(test)]
pub(crate) fn inject_spawn_pause(
    id: Uuid,
    point: SpawnPausePoint,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    spawn_pause_injections()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            id,
            SpawnPauseInjection {
                point,
                reached,
                release,
            },
        );
}

#[cfg(test)]
pub(crate) fn allow_duplicate_reservation_once(id: Uuid) {
    duplicate_reservation_injections()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(id);
}

#[cfg(test)]
pub(crate) fn inject_reservation_failure_once(id: Uuid) {
    reservation_failure_injections()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(id);
}

#[cfg(test)]
pub(crate) fn observe_next_concurrent_kill_wait(
    id: Uuid,
    entered: std::sync::mpsc::SyncSender<()>,
) {
    concurrent_kill_wait_observers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(id, entered);
}

#[cfg(test)]
fn notify_concurrent_kill_wait(id: Uuid) {
    let observer = concurrent_kill_wait_observers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id);
    if let Some(observer) = observer {
        let _ = observer.send(());
    }
}

#[cfg(test)]
fn take_duplicate_reservation_injection(id: Uuid) -> bool {
    duplicate_reservation_injections()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id)
}

#[cfg(test)]
fn take_reservation_failure_injection(id: Uuid) -> bool {
    reservation_failure_injections()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id)
}

#[cfg(test)]
fn pause_spawn_if_injected(id: Uuid, point: SpawnPausePoint) -> Result<(), AppError> {
    let injection = {
        let mut injections = spawn_pause_injections()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if injections
            .get(&id)
            .is_some_and(|injection| injection.point == point)
        {
            injections.remove(&id)
        } else {
            None
        }
    };
    let Some(injection) = injection else {
        return Ok(());
    };
    injection.reached.send(()).map_err(|error| {
        AppError::PtyError(format!("publish injected spawn pause {point:?}: {error}"))
    })?;
    injection.release.recv().map_err(|error| {
        AppError::PtyError(format!("release injected spawn pause {point:?}: {error}"))
    })
}

#[cfg(test)]
pub(crate) fn take_spawn_failure(id: Uuid, point: SpawnFailurePoint) -> Result<bool, AppError> {
    let injection = {
        let mut injections = spawn_failure_injections()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if injections
            .get(&id)
            .is_some_and(|entry| entry.point == point)
        {
            injections.remove(&id)
        } else {
            None
        }
    };
    let Some(injection) = injection else {
        return Ok(false);
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match std::fs::read_to_string(&injection.descendant_marker) {
            Ok(value) if value.split_whitespace().count() == 2 => return Ok(true),
            Ok(_) | Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(value) => {
                return Err(AppError::PtyError(format!(
                    "injected {point:?} boundary did not observe two process ids in {}: {value:?}",
                    injection.descendant_marker.display()
                )));
            }
            Err(error) => {
                return Err(AppError::PtyError(format!(
                    "injected {point:?} boundary did not observe descendant marker {}: {error}",
                    injection.descendant_marker.display()
                )));
            }
        }
    }
}

impl<R: tauri::Runtime> Clone for LocalProcessBackend<R> {
    fn clone(&self) -> Self {
        Self {
            ownership: Arc::clone(&self.ownership),
            fanout: self.fanout.clone(),
            git_watcher: Arc::clone(&self.git_watcher),
        }
    }
}

impl<R: tauri::Runtime> LocalProcessBackend<R> {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher<R>>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        Self {
            ownership: Arc::new(LocalOwnershipSet::new()),
            fanout: SessionIoFanout::new(output_senders, idle_detector, ws_broadcaster),
            git_watcher,
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_additional_detached_for_test(
        &self,
        mut spec: BackendSpawnSpec,
    ) -> Result<u64, AppError> {
        let id = spec.id;
        let attempt =
            {
                let mut registry =
                    self.ownership.registry.lock().map_err(|_| {
                        AppError::PtyError("local_owner_registry_poisoned".to_string())
                    })?;
                let generation = registry.next_generation;
                registry.next_generation = generation.checked_add(1).ok_or_else(|| {
                    AppError::PtyError("local PTY generation overflow".to_string())
                })?;
                let attempt = Arc::new(LocalProcessAttempt::new(id, generation));
                let session = registry.sessions.get_mut(&id).ok_or_else(|| {
                    AppError::PtyError("test session owner is absent".to_string())
                })?;
                if session.kill.is_some() {
                    return Err(AppError::PtyError(
                        "test session teardown is already active".to_string(),
                    ));
                }
                session.attempts.insert(generation, Arc::clone(&attempt));
                attempt
            };
        self.ownership
            .diagnostic_index
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(id)
            .or_default()
            .push(Arc::clone(&attempt));
        if let Some(registration) = spec.resource_registration.as_mut() {
            registration
                .bind_owner_generation(attempt.generation)
                .map_err(AppError::PtyError)?;
        }
        self.spawn_sync(spec, Arc::clone(&attempt))?;
        let mut state = attempt
            .state
            .lock()
            .map_err(|_| AppError::PtyError("local_attempt_state_poisoned".to_string()))?;
        let previous = std::mem::replace(&mut *state, LocalAttemptState::TeardownInProgress);
        let LocalAttemptState::Active(instance) = previous else {
            *state = previous;
            return Err(AppError::PtyError(
                "test generation did not publish an active owner".to_string(),
            ));
        };
        *state = LocalAttemptState::Detached(instance.owner);
        attempt.state_changed.notify_all();
        Ok(attempt.generation)
    }

    #[cfg(test)]
    pub(crate) fn owner_generation_count_for_test(&self, id: Uuid) -> usize {
        self.ownership
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sessions
            .get(&id)
            .map(|session| session.attempts.len())
            .unwrap_or_default()
    }

    fn reserve_attempt(&self, id: Uuid) -> Result<Arc<LocalProcessAttempt>, AppError> {
        #[cfg(test)]
        if take_reservation_failure_injection(id) {
            return Err(AppError::PtyError(
                "injected local owner reservation failure".to_string(),
            ));
        }
        #[cfg(test)]
        let allow_duplicate = take_duplicate_reservation_injection(id);
        #[cfg(not(test))]
        let allow_duplicate = false;
        let attempt = {
            let mut registry = self
                .ownership
                .registry
                .lock()
                .map_err(|_| AppError::PtyError("local_owner_registry_poisoned".to_string()))?;
            if let Some(session) = registry.sessions.get(&id) {
                if allow_duplicate {
                    // Test-only fault injection may reserve a second generation.
                } else {
                    let generations = session
                        .attempts
                        .keys()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(",");
                    return Err(AppError::PtyError(format!(
                    "local PTY session {id} already owns or is tearing down generation(s) [{generations}]"
                )));
                }
            }
            registry.sessions.try_reserve(1).map_err(|error| {
                AppError::PtyError(format!(
                    "failed to reserve local PTY session ownership: {error}"
                ))
            })?;
            let generation = registry.next_generation;
            registry.next_generation = generation
                .checked_add(1)
                .ok_or_else(|| AppError::PtyError("local PTY generation overflow".to_string()))?;
            let attempt = Arc::new(LocalProcessAttempt::new(id, generation));
            if let Some(session) = registry.sessions.get_mut(&id) {
                if session.kill.is_some() {
                    return Err(AppError::PtyError(format!(
                        "local PTY session {id} teardown is already in progress"
                    )));
                }
                session.attempts.insert(generation, Arc::clone(&attempt));
            } else {
                registry
                    .sessions
                    .insert(id, LocalSessionOwnership::new(Arc::clone(&attempt)));
            }
            attempt
        };
        self.ownership
            .diagnostic_index
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(id)
            .or_default()
            .push(Arc::clone(&attempt));
        Ok(attempt)
    }

    fn attempt_cancelled_error(attempt: &LocalProcessAttempt) -> AppError {
        AppError::PtyError(format!(
            "local PTY spawn cancelled for session {} generation {}",
            attempt.id, attempt.generation
        ))
    }

    fn install_spawned_owner(
        &self,
        attempt: &Arc<LocalProcessAttempt>,
        owner: LocalProcessOwner,
    ) -> Result<(), AppError> {
        attempt.identity.update(&owner);
        let mut state = attempt
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match &*state {
            LocalAttemptState::Reserved | LocalAttemptState::CancelRequested => {
                *state = LocalAttemptState::Detached(owner);
            }
            _ => {
                return Err(AppError::PtyError(format!(
                    "local PTY session {} generation {} lost its spawn reservation before owner registration",
                    attempt.id, attempt.generation
                )));
            }
        }
        let LocalAttemptState::Detached(owner) = &mut *state else {
            unreachable!("the spawned owner was stored under the same state lock")
        };
        #[cfg(unix)]
        let ownership_result = {
            let mut process_group =
                UnixProcessGroupOwner::unverified_for_child_pid(owner.root_pid)?;
            let result = process_group.verify_identity();
            owner.process_group = Some(process_group);
            result
        };
        #[cfg(windows)]
        let ownership_result = {
            owner.job = owner
                .root_pid
                .and_then(crate::pty::job::JobObject::for_child);
            if owner.job.is_some() {
                Ok(())
            } else {
                Err(AppError::PtyError(format!(
                    "spawned Windows child pid {:?} without an effective Job Object owner",
                    owner.root_pid
                )))
            }
        };
        attempt.identity.update(owner);
        attempt.state_changed.notify_all();
        ownership_result
    }

    fn with_detached_owner<T>(
        attempt: &Arc<LocalProcessAttempt>,
        operation: impl FnOnce(&mut LocalProcessOwner) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut state = attempt
            .state
            .lock()
            .map_err(|_| AppError::PtyError("local_attempt_state_poisoned".to_string()))?;
        let LocalAttemptState::Detached(owner) = &mut *state else {
            return Err(Self::attempt_cancelled_error(attempt));
        };
        let result = operation(owner);
        attempt.identity.update(owner);
        result
    }

    fn publish_active_attempt(
        attempt: &Arc<LocalProcessAttempt>,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        cols: u16,
        rows: u16,
        rendered: Arc<AtomicBool>,
    ) -> Result<(), AppError> {
        let mut state = attempt
            .state
            .lock()
            .map_err(|_| AppError::PtyError("local_attempt_state_poisoned".to_string()))?;
        if attempt.cancelled.load(Ordering::Acquire) {
            return Err(Self::attempt_cancelled_error(attempt));
        }
        let previous = std::mem::replace(&mut *state, LocalAttemptState::TeardownInProgress);
        let LocalAttemptState::Detached(owner) = previous else {
            *state = previous;
            return Err(Self::attempt_cancelled_error(attempt));
        };
        *state = LocalAttemptState::Active(PtyInstance {
            master: Arc::new(Mutex::new(master)),
            writer: Arc::new(Mutex::new(writer)),
            owner,
            size: Mutex::new((cols, rows)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered,
        });
        attempt.state_changed.notify_all();
        Ok(())
    }

    fn finish_unspawned_attempt(
        &self,
        attempt: &Arc<LocalProcessAttempt>,
        deadline: Instant,
    ) -> Result<(), String> {
        {
            let mut state = lock_local_owner_map_until(&attempt.state, deadline)
                .map_err(|()| attempt.diagnostic("Mutex::try_lock(attempt-state)"))?;
            match &*state {
                LocalAttemptState::Reserved | LocalAttemptState::CancelRequested => {
                    *state = LocalAttemptState::Terminal;
                    attempt.state_changed.notify_all();
                }
                LocalAttemptState::Terminal => {}
                _ => {
                    return Err(attempt.diagnostic(
                        "finish_unspawned_attempt observed an installed process owner",
                    ));
                }
            }
        }
        self.commit_terminal_attempt(attempt, deadline)
    }

    fn commit_terminal_attempt(
        &self,
        attempt: &Arc<LocalProcessAttempt>,
        deadline: Instant,
    ) -> Result<(), String> {
        let mut registry = lock_local_owner_map_until(&self.ownership.registry, deadline)
            .map_err(|()| attempt.diagnostic("Mutex::try_lock(owner-registry-commit)"))?;
        let mut remove_session = false;
        if let Some(session) = registry.sessions.get_mut(&attempt.id) {
            if session.kill.is_none() {
                let is_terminal = attempt
                    .state
                    .try_lock()
                    .map(|state| matches!(*state, LocalAttemptState::Terminal))
                    .unwrap_or(false);
                if is_terminal {
                    session.attempts.remove(&attempt.generation);
                    remove_session = session.attempts.is_empty();
                }
            }
        }
        if remove_session {
            registry.sessions.remove(&attempt.id);
        }
        drop(registry);
        if let Ok(mut index) = self.ownership.diagnostic_index.try_lock() {
            if let Some(attempts) = index.get_mut(&attempt.id) {
                attempts.retain(|candidate| {
                    candidate.generation != attempt.generation
                        || !matches!(
                            candidate.state.try_lock().as_deref(),
                            Ok(LocalAttemptState::Terminal)
                        )
                });
                if attempts.is_empty() {
                    index.remove(&attempt.id);
                }
            }
        }
        Ok(())
    }

    fn cleanup_attempt_until(
        &self,
        attempt: &Arc<LocalProcessAttempt>,
        budget: Duration,
        started: Instant,
        deadline: Instant,
    ) -> Result<bool, String> {
        attempt.cancelled.store(true, Ordering::Release);
        let retained = loop {
            let mut state = lock_local_owner_map_until(&attempt.state, deadline)
                .map_err(|()| attempt.diagnostic("Mutex::try_lock(attempt-state)"))?;
            let previous = std::mem::replace(&mut *state, LocalAttemptState::TeardownInProgress);
            match previous {
                LocalAttemptState::Reserved | LocalAttemptState::CancelRequested => {
                    *state = LocalAttemptState::CancelRequested;
                    attempt.state_changed.notify_all();
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(attempt.diagnostic(
                            "spawn worker did not resolve reservation before deadline",
                        ));
                    }
                    let remaining = deadline.saturating_duration_since(now);
                    let (next, _) = attempt
                        .state_changed
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(|error| error.into_inner());
                    drop(next);
                }
                LocalAttemptState::Detached(owner) => {
                    break RetainedLocalProcess::Detached(owner);
                }
                LocalAttemptState::Active(instance) => {
                    break RetainedLocalProcess::Instance(instance);
                }
                LocalAttemptState::TeardownInProgress => {
                    *state = LocalAttemptState::TeardownInProgress;
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(attempt.diagnostic(
                            "another teardown retained its in-progress tombstone through deadline",
                        ));
                    }
                    let remaining = deadline.saturating_duration_since(now);
                    let (next, _) = attempt
                        .state_changed
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(|error| error.into_inner());
                    drop(next);
                }
                LocalAttemptState::Terminal => {
                    *state = LocalAttemptState::Terminal;
                    return Ok(true);
                }
            }
        };

        let mut entries = vec![(attempt.id, retained)];
        let diagnostic = shutdown_local_processes_until(&mut entries, budget, started, deadline)
            .pop()
            .flatten();
        let (_, retained) = entries
            .pop()
            .expect("one attempt cleanup retains exactly one ownership value");
        let mut state = lock_local_owner_map_until(&attempt.state, deadline)
            .map_err(|()| attempt.diagnostic("Mutex::try_lock(attempt-state-commit)"))?;
        match diagnostic {
            None => {
                *state = LocalAttemptState::Terminal;
                attempt.state_changed.notify_all();
                Ok(true)
            }
            Some(diagnostic) => {
                let owner = match retained {
                    RetainedLocalProcess::Instance(instance) => instance.owner,
                    RetainedLocalProcess::Detached(owner) => owner,
                };
                attempt.identity.update(&owner);
                *state = LocalAttemptState::Detached(owner);
                attempt.state_changed.notify_all();
                Err(diagnostic)
            }
        }
    }

    fn cleanup_cancelled_attempt(&self, attempt: &Arc<LocalProcessAttempt>) {
        attempt.cancelled.store(true, Ordering::Release);
        let should_cleanup = match attempt.state.try_lock() {
            Ok(mut state) => match &*state {
                LocalAttemptState::Reserved => {
                    *state = LocalAttemptState::CancelRequested;
                    attempt.state_changed.notify_all();
                    false
                }
                LocalAttemptState::CancelRequested | LocalAttemptState::TeardownInProgress => false,
                LocalAttemptState::Terminal
                | LocalAttemptState::Detached(_)
                | LocalAttemptState::Active(_) => true,
            },
            Err(std::sync::TryLockError::Poisoned(error)) => {
                let state = error.into_inner();
                matches!(
                    &*state,
                    LocalAttemptState::Terminal
                        | LocalAttemptState::Detached(_)
                        | LocalAttemptState::Active(_)
                )
            }
            // The blocking worker or an existing teardown owns the state
            // transition. The generation-scoped cancellation flag makes that
            // owner perform the cleanup without blocking this future's Drop.
            Err(std::sync::TryLockError::WouldBlock) => false,
        };
        if !should_cleanup {
            return;
        }
        let budget = LOCAL_OWNER_SHUTDOWN_BUDGET;
        let started = Instant::now();
        let deadline = started.checked_add(budget).unwrap_or(started);
        match self.cleanup_attempt_until(attempt, budget, started, deadline) {
            Ok(true) => {
                if let Err(diagnostic) = self.commit_terminal_attempt(attempt, deadline) {
                    log::error!("[pty] {diagnostic}");
                }
            }
            Ok(false) => {}
            Err(diagnostic) => log::error!("[pty] {diagnostic}"),
        }
    }

    fn fail_spawn_attempt(
        &self,
        attempt: &Arc<LocalProcessAttempt>,
        primary: AppError,
    ) -> AppError {
        let budget = LOCAL_OWNER_SHUTDOWN_BUDGET;
        let started = Instant::now();
        let deadline = started.checked_add(budget).unwrap_or(started);
        match self.cleanup_attempt_until(attempt, budget, started, deadline) {
            Ok(true) => match self.commit_terminal_attempt(attempt, deadline) {
                Ok(()) => primary,
                Err(diagnostic) => AppError::PtyError(format!(
                    "{primary}; spawn rollback retained ownership tombstone: {diagnostic}"
                )),
            },
            Ok(false) => primary,
            Err(diagnostic) => AppError::PtyError(format!(
                "{primary}; spawn rollback retained ownership: {diagnostic}"
            )),
        }
    }

    fn snapshot_attempts(&self, id: Uuid) -> Vec<Arc<LocalProcessAttempt>> {
        self.ownership
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sessions
            .get(&id)
            .map(|session| session.attempts.values().cloned().collect())
            .unwrap_or_default()
    }

    fn diagnostic_attempts(&self, id: Option<Uuid>, operation: &str) -> Vec<String> {
        let index = self
            .ownership
            .diagnostic_index
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        index
            .iter()
            .filter(|(session_id, _)| id.is_none_or(|id| id == **session_id))
            .flat_map(|(_, attempts)| attempts.iter().map(|attempt| attempt.diagnostic(operation)))
            .collect()
    }

    fn wait_for_kill_tombstone(
        _id: Uuid,
        tombstone: &LocalKillTombstone,
        deadline: Instant,
    ) -> Result<(), ()> {
        let mut state = lock_local_owner_map_until(&tombstone.state, deadline)?;
        while *state == LocalKillState::InProgress {
            #[cfg(test)]
            notify_concurrent_kill_wait(_id);
            let now = Instant::now();
            if now >= deadline {
                return Err(());
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, timeout) = tombstone
                .state_changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state = next;
            if timeout.timed_out() && *state == LocalKillState::InProgress {
                return Err(());
            }
        }
        Ok(())
    }

    fn kill_session_until(
        &self,
        id: Uuid,
        budget: Duration,
        started: Instant,
        deadline: Instant,
        source: &str,
    ) -> Result<usize, Vec<String>> {
        let (tombstone, attempts) = loop {
            let mut registry = match lock_local_owner_map_until(&self.ownership.registry, deadline)
            {
                Ok(registry) => registry,
                Err(()) => {
                    let diagnostics =
                        self.diagnostic_attempts(Some(id), "Mutex::try_lock(owner-registry)");
                    return Err(if diagnostics.is_empty() {
                        vec![format!(
                            "session {id} retained local PTY ownership after {}ms: no generation was published before the owner-registry deadline",
                            budget.as_millis()
                        )]
                    } else {
                        diagnostics
                    });
                }
            };
            let Some(session) = registry.sessions.get_mut(&id) else {
                return Ok(0);
            };
            if let Some(existing) = session.kill.as_ref().cloned() {
                let finished = existing
                    .state
                    .try_lock()
                    .map(|state| *state == LocalKillState::Finished)
                    .unwrap_or(false);
                if finished {
                    session.kill = None;
                    session.attempts.retain(|_, attempt| {
                        !matches!(
                            attempt.state.try_lock().as_deref(),
                            Ok(LocalAttemptState::Terminal)
                        )
                    });
                    if session.attempts.is_empty() {
                        registry.sessions.remove(&id);
                    }
                    continue;
                }
                drop(registry);
                if Self::wait_for_kill_tombstone(id, &existing, deadline).is_err() {
                    return Err(self.diagnostic_attempts(
                        Some(id),
                        "concurrent kill tombstone remained in progress through deadline",
                    ));
                }
                continue;
            }
            let tombstone = Arc::new(LocalKillTombstone::new());
            session.kill = Some(Arc::clone(&tombstone));
            let attempts = session.attempts.values().cloned().collect::<Vec<_>>();
            break (tombstone, attempts);
        };

        let mut diagnostics = Vec::new();
        let mut terminal = 0;
        for attempt in &attempts {
            if Instant::now() >= deadline {
                diagnostics.push(attempt.diagnostic("absolute teardown deadline expired"));
                continue;
            }
            match self.cleanup_attempt_until(attempt, budget, started, deadline) {
                Ok(true) => terminal += 1,
                Ok(false) => {}
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }

        let commit = lock_local_owner_map_until(&self.ownership.registry, deadline);
        match commit {
            Ok(mut registry) => {
                let mut remove_session = false;
                if let Some(session) = registry.sessions.get_mut(&id) {
                    if session
                        .kill
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &tombstone))
                    {
                        session.attempts.retain(|_, attempt| {
                            !matches!(
                                attempt.state.try_lock().as_deref(),
                                Ok(LocalAttemptState::Terminal)
                            )
                        });
                        session.kill = None;
                        remove_session = session.attempts.is_empty();
                    }
                }
                if remove_session {
                    registry.sessions.remove(&id);
                }
                drop(registry);
                if let Ok(mut index) = self.ownership.diagnostic_index.try_lock() {
                    if let Some(indexed) = index.get_mut(&id) {
                        indexed.retain(|attempt| {
                            !matches!(
                                attempt.state.try_lock().as_deref(),
                                Ok(LocalAttemptState::Terminal)
                            )
                        });
                        if indexed.is_empty() {
                            index.remove(&id);
                        }
                    }
                }
            }
            Err(()) => {
                diagnostics.extend(self.diagnostic_attempts(
                    Some(id),
                    "Mutex::try_lock(owner-registry-outcome-commit)",
                ))
            }
        }
        tombstone.finish();

        self.fanout.remove_session(id);
        self.git_watcher.remove_session(id);
        spawn_diagnostics::forget(id);
        if diagnostics.is_empty() {
            log::debug!(
                "[pty] session {id} source={source} committed terminal ownership for {terminal} generation(s)"
            );
            Ok(terminal)
        } else {
            Err(diagnostics)
        }
    }

    fn shutdown_local_processes_until_deadline(
        &self,
        budget: Duration,
        started: Instant,
        deadline: Instant,
    ) -> PtyShutdownReport {
        let ids = match lock_local_owner_map_until(&self.ownership.registry, deadline) {
            Ok(registry) => registry.sessions.keys().copied().collect::<Vec<_>>(),
            Err(()) => {
                return PtyShutdownReport {
                    terminal: 0,
                    retained: self
                        .diagnostic_attempts(None, "Mutex::try_lock(owner-registry-bulk-shutdown)"),
                };
            }
        };
        let mut report = PtyShutdownReport::default();
        for id in ids {
            match self.kill_session_until(id, budget, started, deadline, "app-shutdown") {
                Ok(terminal) => report.terminal += terminal,
                Err(mut diagnostics) => {
                    for diagnostic in &diagnostics {
                        log::error!("[pty] {diagnostic}");
                    }
                    report.retained.append(&mut diagnostics);
                }
            }
        }
        report
    }

    fn shutdown_local_processes_with_budget(&self, budget: Duration) -> PtyShutdownReport {
        let started = Instant::now();
        let deadline = started.checked_add(budget).unwrap_or(started);
        self.shutdown_local_processes_until_deadline(budget, started, deadline)
    }

    fn spawn_sync(
        &self,
        spec: BackendSpawnSpec,
        attempt: Arc<LocalProcessAttempt>,
    ) -> Result<(), AppError> {
        let BackendSpawnSpec {
            id,
            agent_id,
            coding_agent,
            cmd,
            args,
            cwd,
            selected_cwd: _,
            cols,
            rows,
            container_image: _,
            configured_env,
            env_remove_keys,
            env_unset: _,
            extra_env,
            idle_tuning,
            output_target,
            mut resource_registration,
            logical_resource_slot: _,
            container_credential: _,
            container_repo_mounts: _,
        } = spec;
        // #942 - how many sessions were spawned in the window just before this one,
        // and how many of them were the same CLI. Concurrent startups against shared
        // agent state (the global ~/.codex) are a prime suspect for the intermittent
        // blank terminal, so every spawn record carries its own concurrency context.
        // Keyed on the CLI, never on the profile id: several profiles run the same
        // codex binary against the same ~/.codex. Counting only, no behavior change.
        let diag_thresholds = spawn_diagnostics::Thresholds::from_env();
        let spawn_window = spawn_diagnostics::note_spawn_attempt(coding_agent, diag_thresholds);
        let pty_system = native_pty_system();
        let spawn_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system
            .openpty(size)
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        let is_direct_exe = cmd.to_lowercase().ends_with(".exe")
            || std::path::Path::new(&cmd)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));

        let mut command = if cfg!(windows) && !is_direct_exe {
            let mut c = CommandBuilder::new("cmd.exe");
            c.arg("/C");
            c.arg(&cmd);
            for arg in &args {
                c.arg(arg);
            }
            c
        } else {
            let mut c = CommandBuilder::new(&cmd);
            for arg in &args {
                c.arg(arg);
            }
            c
        };
        command.cwd(&spawn_cwd);

        // #942 - the argv exactly as executed, cmd.exe wrapper included. Mirrors the
        // branch above instead of reshaping the CommandBuilder, so the spawn stays
        // byte-for-byte what it was.
        let exec_argv: Vec<String> = if cfg!(windows) && !is_direct_exe {
            let mut argv = vec!["cmd.exe".to_string(), "/C".to_string(), cmd.clone()];
            argv.extend(args.iter().cloned());
            argv
        } else {
            let mut argv = vec![cmd.clone()];
            argv.extend(args.iter().cloned());
            argv
        };

        // #942 - what the child will really see for CODEX_HOME: an explicit configured
        // value wins, then an explicit removal, then the AC environment the child
        // inherits. None here means this Codex shares the global ~/.codex with every
        // other one.
        let codex_home = configured_env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("CODEX_HOME"))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                if env_remove_keys
                    .iter()
                    .any(|key| key.eq_ignore_ascii_case("CODEX_HOME"))
                {
                    None
                } else {
                    std::env::var("CODEX_HOME").ok()
                }
            });

        for key in &env_remove_keys {
            command.env_remove(key);
        }
        for (key, value) in &configured_env {
            command.env(key, value);
        }
        if !configured_env.is_empty() || !env_remove_keys.is_empty() {
            log::info!(
                "[pty] Applied {} configured env vars and removed {} inherited env vars for session {}",
                configured_env.len(),
                env_remove_keys.len(),
                id
            );
        }
        #[cfg(target_os = "linux")]
        {
            if should_synthesize_local_codex_path(coding_agent, &configured_env, &env_remove_keys) {
                let child_path = crate::pty::child_path::local_codex_child_path();
                for skipped in &child_path.skipped {
                    log::warn!(
                        "[pty] Skipping local Codex PATH candidate {}: {}",
                        skipped.path.display(),
                        skipped.reason
                    );
                }
                command.env("PATH", &child_path.value);
            }
        }
        crate::pty::credentials::apply_credential_env_to_pty_command(&mut command, &extra_env);
        command.env("TERM", "xterm-256color");

        if !extra_env.is_empty() {
            log::info!(
                "[pty] Applied {} per-process credential environment variables for session {}",
                extra_env.len(),
                id
            );
        }

        if let Some(git_ceiling_dirs) =
            crate::config::session_context::git_ceiling_directories_for_session_root(&spawn_cwd)
        {
            command.env("GIT_CEILING_DIRECTORIES", &git_ceiling_dirs);
            log::info!(
                "[pty] Applied GIT_CEILING_DIRECTORIES for session cwd {}: {}",
                spawn_cwd,
                git_ceiling_dirs
            );

            #[cfg(windows)]
            if let Some(git_guard_env) = build_git_guard_env()? {
                command.env("PATH", &git_guard_env.path);
                command.env("PATHEXT", &git_guard_env.pathext);
                command.env("AC_REAL_GIT", &git_guard_env.real_git);
                log::info!(
                    "[pty] Enabled git guard wrapper for session cwd {}",
                    spawn_cwd
                );
            }
        }

        // #942 - time zero for time-to-first-output.
        let spawn_started = Instant::now();
        #[cfg(test)]
        if let Err(error) = pause_spawn_if_injected(id, SpawnPausePoint::BeforeSpawn) {
            let deadline = Instant::now() + LOCAL_OWNER_SHUTDOWN_BUDGET;
            let _ = self.finish_unspawned_attempt(&attempt, deadline);
            return Err(error);
        }
        if attempt.cancelled.load(Ordering::Acquire) {
            let deadline = Instant::now() + LOCAL_OWNER_SHUTDOWN_BUDGET;
            let _ = self.finish_unspawned_attempt(&attempt, deadline);
            return Err(Self::attempt_cancelled_error(&attempt));
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| AppError::PtyError(e.to_string()))?;
        let child_pid = child.process_id();
        log::info!(
            "[pty] Spawned session {} with child pid {:?}",
            id,
            child_pid
        );

        // Ownership begins immediately after the real child exists. Every
        // subsequent failure either proves the whole owner terminal or stores
        // it for a later bounded retry with an exact diagnostic.
        let owner = LocalProcessOwner::new_for_generation(
            attempt.generation,
            child,
            resource_registration.take(),
        );
        if let Err(error) = self.install_spawned_owner(&attempt, owner) {
            return Err(self.fail_spawn_attempt(&attempt, error));
        }
        #[cfg(test)]
        if let Err(error) = pause_spawn_if_injected(id, SpawnPausePoint::AfterChildCreation) {
            return Err(self.fail_spawn_attempt(&attempt, error));
        }
        if attempt.cancelled.load(Ordering::Acquire) {
            let primary = Self::attempt_cancelled_error(&attempt);
            return Err(self.fail_spawn_attempt(&attempt, primary));
        }

        let registration_result = Self::with_detached_owner(&attempt, |owner| {
            if let Some(registration) = owner.resource_registration.as_mut() {
                let pid = child_pid.ok_or_else(|| {
                    AppError::PtyError(
                        "Resource Monitor could not capture spawned child pid".to_string(),
                    )
                })?;
                registration
                    .register_root_pid(pid)
                    .map_err(AppError::PtyError)?;
            }
            Ok(())
        });
        if let Err(error) = registration_result {
            return Err(self.fail_spawn_attempt(&attempt, error));
        }
        #[cfg(test)]
        if let Err(error) = pause_spawn_if_injected(id, SpawnPausePoint::AfterRegistration) {
            return Err(self.fail_spawn_attempt(&attempt, error));
        }
        if attempt.cancelled.load(Ordering::Acquire) {
            let primary = Self::attempt_cancelled_error(&attempt);
            return Err(self.fail_spawn_attempt(&attempt, primary));
        }

        drop(pair.slave);

        #[cfg(test)]
        match take_spawn_failure(id, SpawnFailurePoint::TakeWriter) {
            Ok(true) => {
                return Err(self.fail_spawn_attempt(
                    &attempt,
                    AppError::PtyError("injected take_writer failure".to_string()),
                ));
            }
            Ok(false) => {}
            Err(error) => return Err(self.fail_spawn_attempt(&attempt, error)),
        }
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                return Err(
                    self.fail_spawn_attempt(&attempt, AppError::PtyError(error.to_string()))
                );
            }
        };

        #[cfg(test)]
        match take_spawn_failure(id, SpawnFailurePoint::CloneReader) {
            Ok(true) => {
                return Err(self.fail_spawn_attempt(
                    &attempt,
                    AppError::PtyError("injected reader clone failure".to_string()),
                ));
            }
            Ok(false) => {}
            Err(error) => return Err(self.fail_spawn_attempt(&attempt, error)),
        }
        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                return Err(
                    self.fail_spawn_attempt(&attempt, AppError::PtyError(error.to_string()))
                );
            }
        };

        // #973 (B) - the child has rendered nothing yet, so the gate starts closed.
        let rendered = Arc::new(AtomicBool::new(false));
        if let Err(error) = Self::publish_active_attempt(
            &attempt,
            pair.master,
            writer,
            cols,
            rows,
            Arc::clone(&rendered),
        ) {
            return Err(self.fail_spawn_attempt(&attempt, error));
        }
        #[cfg(test)]
        if let Err(error) = pause_spawn_if_injected(id, SpawnPausePoint::AfterPublication) {
            return Err(self.fail_spawn_attempt(&attempt, error));
        }
        if attempt.cancelled.load(Ordering::Acquire) {
            let primary = Self::attempt_cancelled_error(&attempt);
            return Err(self.fail_spawn_attempt(&attempt, primary));
        }
        self.fanout.register_session(id, idle_tuning, rows, cols);

        // #942 - app.log is what users paste into issues, so a secret must never be
        // echoed back out through the child output or the argv we log. Keyed on the env
        // NAME (token / key / secret / password / credential), across both AC's own
        // credential env and the configured rows where AC tells users to keep their API
        // keys. Deliberately not a sweep of every value: a Codex profile legitimately
        // carries `MODEL=gpt-5.6-sol`, and blanking that would shred the very argv this
        // instrumentation exists to record.
        let redact: Vec<String> = extra_env
            .iter()
            .chain(configured_env.iter())
            .filter(|(key, _)| spawn_diagnostics::is_secret_env_key(key))
            .map(|(_, value)| value.clone())
            .collect();

        // #942 - emits `[pty] spawn-record` (argv, cwd, CLI, profile, CODEX_HOME,
        // concurrency).
        let record = spawn_diagnostics::register(SpawnRecordInit {
            session_id: id,
            pid: child_pid,
            argv: exec_argv,
            cwd: spawn_cwd,
            cli: coding_agent,
            agent_profile_id: agent_id,
            codex_home,
            configured_env_count: configured_env.len(),
            removed_env_count: env_remove_keys.len(),
            redact,
            window: spawn_window,
            started: spawn_started,
            thresholds: diag_thresholds,
        });

        // #942 - the startup verdict at the deadline and exit attribution. The PTY
        // reader alone cannot carry either: ConPTY holds the pipe open past the death of
        // the child, so a child that dies on its own would stay invisible until the
        // session is torn down. The monitor polls the child handle instead and ends with
        // the session (detached on purpose; the handle is only useful to tests).
        let monitor_backend = self.clone();
        let _monitor = spawn_diagnostics::watch_child(Arc::clone(&record), move || {
            monitor_backend.probe_child(id)
        });

        let session_id_str = id.to_string();
        let fanout = self.fanout.clone();
        let gate_backend = self.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // #942 - time-to-first-output and the retained head bytes. Hot
                        // path: once the first byte is stamped and the head buffer is
                        // full this is two relaxed loads and one relaxed add.
                        record.note_output(&buf[..n]);
                        fanout.handle_output(
                            &output_target,
                            id,
                            &session_id_str,
                            buf[..n].to_vec(),
                        );
                        // #973 (B) - has the child PAINTED anything yet? Asked of the real vt100
                        // parser that `handle_output` has just fed this chunk to, which is why it
                        // is asked after it and not before.
                        //
                        // Once the child has painted, this is a single relaxed load per chunk and
                        // nothing else: no lock, no scan, no allocation. Before it has, it is one
                        // uncontended lock and a bounded scan of the grid - and it REPLACED a
                        // second `from_utf8_lossy` + `strip_ansi_csi` over the whole chunk, which
                        // `handle_output` was already paying for the idle detector on the very
                        // same bytes. One less chunk-sized allocation per chunk, not one more.
                        //
                        // Locks: the query takes `screen_parsers` and RELEASES it before
                        // `open_startup_gate` takes `ptys`. The two are sequential, never nested,
                        // so the order stays `ptys -> startup_gate -> size -> master`.
                        if !rendered.load(Ordering::Relaxed)
                            && fanout.has_rendered_visible_content(id)
                        {
                            gate_backend.open_startup_gate(id);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    /// #973 (B) - the child rendered. Hand the ConPTY the size the view has been waiting to
    /// give it, and bring the vt100 screen with it. Called from the PTY read loop, once per
    /// session in practice. The decision itself is `hand_over_held_size`.
    fn open_startup_gate(&self, id: Uuid) {
        let applied = {
            let mut applied = None;
            for attempt in self.snapshot_attempts(id) {
                let Ok(state) = attempt.state.lock() else {
                    continue;
                };
                if let LocalAttemptState::Active(instance) = &*state {
                    applied = hand_over_held_size(instance, id);
                    break;
                }
            }
            applied
        };

        // Outside the `ptys` guard: the screen and the broadcast take locks of their own, and
        // every terminal write, resize and kill in the app queues behind that one.
        //
        // Only for a size the ConPTY actually took. The vt100 screen models the CHILD's
        // screen, so it must not be moved to a size the child was never given.
        if let Some((cols, rows)) = applied {
            self.fanout.resize_screen_and_broadcast(id, cols, rows);
        }
    }

    /// #942 - liveness of the child of a session, without disturbing it. Never blocks
    /// (zero timeout) and never reports a child it could not query as running.
    fn probe_child(&self, id: Uuid) -> ChildLiveness {
        for attempt in self.snapshot_attempts(id) {
            let Ok(mut state) = attempt.state.lock() else {
                return ChildLiveness::Unqueryable(
                    "local attempt state lock is poisoned".to_string(),
                );
            };
            match &mut *state {
                LocalAttemptState::Active(instance) => {
                    return probe_owner_child(&mut instance.owner);
                }
                LocalAttemptState::Detached(owner) => return probe_owner_child(owner),
                LocalAttemptState::Reserved
                | LocalAttemptState::CancelRequested
                | LocalAttemptState::TeardownInProgress => {
                    return ChildLiveness::Unqueryable(format!(
                        "local PTY generation {} is not yet queryable",
                        attempt.generation
                    ));
                }
                LocalAttemptState::Terminal => {}
            }
        }
        ChildLiveness::Gone
    }
}

/// #942 - the body of `probe_child`, free over the map so #1032's liveness gate can be
/// driven by a test against a real ConPTY child.
///
/// The guard is a local and `ChildLiveness` is owned, so the `ptys` lock is released at the
/// return. That is what makes the gate below safe by construction rather than by care: the
/// `match` scrutinee holds no borrow of the map, so no temporary-lifetime extension can
/// carry the guard into an arm and nest `screen_parsers` inside `ptys`.
#[cfg(test)]
fn probe_child_in(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) -> ChildLiveness {
    let mut ptys = ptys.lock().unwrap_or_else(|e| e.into_inner());
    let Some(instance) = ptys.get_mut(&id) else {
        return ChildLiveness::Gone;
    };
    probe_owner_child(&mut instance.owner)
}

fn probe_owner_child(owner: &mut LocalProcessOwner) -> ChildLiveness {
    let Some(child) = owner.child.as_mut() else {
        return ChildLiveness::Gone;
    };
    let liveness = probe_child_contained(child);
    if matches!(liveness, ChildLiveness::Exited { .. } | ChildLiveness::Gone) {
        owner.child = None;
        #[cfg(unix)]
        if owner
            .process_group
            .is_some_and(|group| matches!(group.exists(), Ok(false)))
        {
            owner.process_group_required = false;
        }
    }
    liveness
}

/// #1032 - a local session's screen rows, gated on its child actually being alive.
///
/// Why a gate at all: a coding agent's statusline SURVIVES on the frozen grid after the
/// child dies. Verbatim, ~8s after a confirmed `code: 0`, on every exit path that matters -
/// a killed process cannot repaint, and Claude Code never uses the alternate screen, never
/// clears and never restores. PTY EOF never arrives either. Nothing signals the death from
/// any direction, so the frozen grid presents a perfectly well-formed row - glyph present,
/// `%` present, column 2, last match on the grid - that passes every defence the user's
/// pattern has. Liveness is not a regex problem, and asking the child is the only thing
/// that can tell a live 42% from a dead one.
///
/// Free over the map and the fanout, like `resize_instance` above, so the gate can be driven
/// by a test against a real ConPTY child: `LocalProcessBackend` itself cannot be built in a
/// unit test (its `GitWatcher` needs a Tauri `AppHandle`), and none of that has anything to
/// do with whether a child is alive.
pub(crate) fn context_liveness_from_child_liveness(
    liveness: &ChildLiveness,
) -> ContextSessionLiveness {
    match liveness {
        ChildLiveness::Alive => ContextSessionLiveness::Live,
        ChildLiveness::Unqueryable(_) => ContextSessionLiveness::Unavailable,
        ChildLiveness::Exited { .. } | ChildLiveness::Gone => ContextSessionLiveness::SessionOver,
    }
}

#[cfg(test)]
mod context_liveness_tests {
    use super::{context_liveness_from_child_liveness, ChildLiveness, ContextSessionLiveness};

    #[test]
    fn maps_every_contained_child_liveness_state() {
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Alive),
            ContextSessionLiveness::Live
        );
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Unqueryable("denied".into())),
            ContextSessionLiveness::Unavailable
        );
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Exited {
                code: 0,
                success: true,
            }),
            ContextSessionLiveness::SessionOver
        );
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Gone),
            ContextSessionLiveness::SessionOver
        );
    }
}

#[cfg(test)]
fn screen_rows_if_child_alive(
    ptys: &Mutex<HashMap<Uuid, PtyInstance>>,
    fanout: &SessionIoFanout,
    id: Uuid,
) -> ScreenRowsRead {
    match probe_child_in(ptys, id) {
        ChildLiveness::Alive => match fanout.get_screen_rows(id) {
            Some(rows) => ScreenRowsRead::Rows(rows),
            // The child is alive, so the session is NOT over. A missing or poisoned parser
            // here is a desync or a poisoned lock, never a statement about the session.
            None => ScreenRowsRead::Unavailable,
        },
        ChildLiveness::Exited { .. } | ChildLiveness::Gone => ScreenRowsRead::SessionOver,
        // "We could not ask" is NOT "the child is dead". #942 built a three-valued oracle
        // exactly so those two never merge, and this is the arm that keeps them apart: the
        // same running process reads Alive through a full-rights handle and Unqueryable
        // through one stripped of SYNCHRONIZE. Reporting that as over would deregister a
        // live session permanently, on a child that never died.
        ChildLiveness::Unqueryable(_) => ScreenRowsRead::Unavailable,
    }
}

/// #942 - probe a child while the caller holds the `ptys` guard, and CONTAIN any panic.
///
/// portable-pty locks a mutex of its own inside the child and unwraps it (`try_wait`,
/// `do_kill`, `process_id` and `as_raw_handle` all do). If that mutex is ever poisoned,
/// an unwind from the probe would escape while the `ptys` guard is held and poison the
/// PTY map itself, which every terminal write, resize and kill locks on: a diagnostics
/// probe would silently take the terminal subsystem down with it. Catching AT the call
/// means the guard above us is released normally and the map stays usable.
fn probe_child_contained(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> ChildLiveness {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe_child_liveness(child)))
        .unwrap_or_else(|_| {
            ChildLiveness::Unqueryable("portable-pty child lock is poisoned".to_string())
        })
}

/// #942 - ask Windows directly instead of going through `portable_pty::Child::try_wait`.
/// `WinChild::is_complete` cannot answer honestly: it swallows a failed
/// `GetExitCodeProcess` and returns "not exited" (so a handle whose query rights were
/// stripped by AV/EDR, a known scenario here, reads as ALIVE), and its `STILL_ACTIVE`
/// sentinel is 259, which is also a legal exit code. The process handle is signalled if
/// and only if the child is gone, whatever it exited with, and a handle we cannot wait
/// on fails loudly instead of pretending.
#[cfg(windows)]
fn probe_child_liveness(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> ChildLiveness {
    use windows_sys::Win32::Foundation::{GetLastError, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

    let Some(handle) = child.as_raw_handle() else {
        return ChildLiveness::Unqueryable("child exposes no process handle".to_string());
    };

    match unsafe { WaitForSingleObject(handle as _, 0) } {
        WAIT_TIMEOUT => ChildLiveness::Alive,
        WAIT_OBJECT_0 => {
            let mut code: u32 = 0;
            if unsafe { GetExitCodeProcess(handle as _, &mut code) } == 0 {
                let os_error = unsafe { GetLastError() };
                return ChildLiveness::Unqueryable(format!(
                    "GetExitCodeProcess failed (os error {os_error})"
                ));
            }
            ChildLiveness::Exited {
                code,
                success: code == 0,
            }
        }
        WAIT_FAILED => {
            let os_error = unsafe { GetLastError() };
            ChildLiveness::Unqueryable(format!("WaitForSingleObject failed (os error {os_error})"))
        }
        other => ChildLiveness::Unqueryable(format!("WaitForSingleObject returned {other}")),
    }
}

/// #942 - on Unix `try_wait` is `waitpid(WNOHANG)`: it reports a failed poll as `Err`,
/// so it can be trusted to tell "running" from "we could not ask".
#[cfg(not(windows))]
fn probe_child_liveness(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> ChildLiveness {
    match child.try_wait() {
        Ok(Some(status)) => ChildLiveness::from(&status),
        Ok(None) => ChildLiveness::Alive,
        Err(e) => ChildLiveness::Unqueryable(e.to_string()),
    }
}

type BlockingSpawnCleanup = dyn Fn(Arc<LocalProcessAttempt>) + Send + Sync + 'static;

struct BlockingSpawnCancelGuard {
    attempt: Arc<LocalProcessAttempt>,
    cleanup: Arc<BlockingSpawnCleanup>,
    disarmed: bool,
}

impl BlockingSpawnCancelGuard {
    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for BlockingSpawnCancelGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }

        self.attempt.cancelled.store(true, Ordering::Release);
        (self.cleanup)(Arc::clone(&self.attempt));
    }
}

impl<R: tauri::Runtime> PtyBackend for LocalProcessBackend<R> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        mut spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        let id = spec.id;
        let attempt = match self.reserve_attempt(id) {
            Ok(attempt) => attempt,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        if let Some(registration) = spec.resource_registration.as_mut() {
            if let Err(error) = registration.bind_owner_generation(attempt.generation) {
                let deadline = Instant::now() + LOCAL_OWNER_SHUTDOWN_BUDGET;
                let _ = self.finish_unspawned_attempt(&attempt, deadline);
                return Box::pin(async move { Err(AppError::PtyError(error)) });
            }
        }
        let spawn_backend = self.clone();
        let cleanup_backend = self.clone();
        Box::pin(async move {
            let cleanup: Arc<BlockingSpawnCleanup> = Arc::new(move |attempt| {
                cleanup_backend.cleanup_cancelled_attempt(&attempt);
            });
            let worker_attempt = Arc::clone(&attempt);
            let worker_cleanup = Arc::clone(&cleanup);
            let mut guard = BlockingSpawnCancelGuard {
                attempt: Arc::clone(&attempt),
                cleanup: Arc::clone(&cleanup),
                disarmed: false,
            };
            let handle = tokio::task::spawn_blocking(move || {
                let result = spawn_backend.spawn_sync(spec, Arc::clone(&worker_attempt));
                if worker_attempt.cancelled.load(Ordering::Acquire) {
                    (worker_cleanup)(Arc::clone(&worker_attempt));
                }
                result
            });
            let result = handle.await.map_err(|error| {
                AppError::Other(format!("local process spawn task failed: {error}"))
            })?;
            guard.disarm();
            result
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError> {
        let writer = self
            .snapshot_attempts(id)
            .into_iter()
            .find_map(|attempt| {
                let state = attempt.state.lock().ok()?;
                match &*state {
                    LocalAttemptState::Active(instance) => Some(Arc::clone(&instance.writer)),
                    _ => None,
                }
            })
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        let mut writer = writer.lock().unwrap_or_else(|error| error.into_inner());
        writer
            .write_all(data)
            .map_err(|error| AppError::PtyError(error.to_string()))?;
        writer
            .flush()
            .map_err(|error| AppError::PtyError(error.to_string()))
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.ownership
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sessions
            .get(&id)
            .is_some_and(|session| !session.attempts.is_empty() || session.kill.is_some())
    }

    fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        context_liveness_from_child_liveness(&self.probe_child(id))
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        // The idle grace sees every resize the view ASKED for, held or refused. Idle semantics
        // are #954's, not this fix's.
        self.fanout.record_resize(id);

        let sent = {
            let mut sent = None;
            for attempt in self.snapshot_attempts(id) {
                let state = attempt
                    .state
                    .lock()
                    .map_err(|_| AppError::PtyError("local_attempt_state_poisoned".to_string()))?;
                if let LocalAttemptState::Active(instance) = &*state {
                    sent = Some(resize_instance(instance, id, cols, rows)?);
                    break;
                }
            }
            sent.ok_or_else(|| AppError::SessionNotFound(id.to_string()))?
        };

        // #973 - the vt100 screen models the CHILD's screen, so it may only follow a size the
        // ConPTY actually took.
        //
        // A refused `0x0` would otherwise set the parser to zero rows (`output.rs`), and #955's
        // `get_screen_snapshot` reads `contents_formatted()` off that grid: an empty snapshot,
        // and a black tile on re-attach - the exact bug #955 shipped to kill.
        //
        // A HELD size is not the child's size either. The child is still emitting for the size
        // the PTY was opened at, and a screen moved ahead of it would parse that output against
        // a geometry the child is not using. `open_startup_gate` moves the screen at the instant
        // the ConPTY takes the size, which is the first moment the two agree.
        //
        // A DEDUPED size is already the screen's size: `register_session` seeds the parser with
        // the size the PTY was opened at, and from there the two only ever move together.
        if sent {
            self.fanout.resize_screen_and_broadcast(id, cols, rows);
        }

        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let budget = LOCAL_OWNER_SHUTDOWN_BUDGET;
        let started = Instant::now();
        let deadline = started.checked_add(budget).unwrap_or(started);
        let child_at_stop = self.probe_child(id);
        let record =
            spawn_diagnostics::mark_ac_stop(id, "session-kill", Some(child_at_stop.clone()));
        let terminal = self
            .kill_session_until(id, budget, started, deadline, "session-kill")
            .map_err(|diagnostics| AppError::PtyError(diagnostics.join(" | ")))?;

        if let Some(record) = record.as_ref() {
            let cause = record.attribute_exit(record.stop_snapshot());
            let _ = record.log_child_exit(cause, &ChildLiveness::Gone, "bounded-stop");
        } else {
            let cause = if matches!(child_at_stop, ChildLiveness::Exited { .. }) {
                ExitCause::ChildInitiated
            } else {
                ExitCause::AcRequested
            };
            log::info!(
                "[pty] child-exit session={} cause={} detail=bounded-stop child=gone",
                id,
                cause.as_log()
            );
        }
        log::debug!(
            "[pty] explicit kill committed {terminal} terminal generation(s) for session {id}"
        );
        Ok(())
    }

    /// #942 - record who asked for the stop and what the child looked like BEFORE any
    /// process is touched. The resource monitor kills a process tree without going through
    /// the PTY layer, so a caller that is about to do that publishes the witness here
    /// first; without it, a child that had already died on its own would be logged as our
    /// kill and its evidence dropped.
    fn publish_stop_witness(&self, id: Uuid, source: &str) {
        let child_at_stop = self.probe_child(id);
        spawn_diagnostics::mark_ac_stop(id, source, Some(child_at_stop));
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        // #942 - AC asked for this stop. Probe first (same reason as `kill`), then tag,
        // so a child that was already dead is never charged to us. This stop can also
        // FAIL (a Quarantined resource-monitor kill leaves the instance intact), which
        // is why the tag only owns exits that follow it inside the attribution window.
        let child_at_stop = self.probe_child(id);
        spawn_diagnostics::mark_ac_stop(id, "job-terminate", Some(child_at_stop));
        let mut terminated = false;
        for attempt in self.snapshot_attempts(id) {
            let Ok(state) = attempt.state.lock() else {
                continue;
            };
            let owner = match &*state {
                LocalAttemptState::Active(instance) => Some(&instance.owner),
                LocalAttemptState::Detached(owner) => Some(owner),
                _ => None,
            };
            if let Some(job) = owner.and_then(|owner| owner.job.as_ref()) {
                job.terminate();
                terminated = true;
            }
        }
        terminated
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        self.shutdown_local_processes_with_budget(LOCAL_OWNER_SHUTDOWN_BUDGET)
            .counts()
    }

    fn kill_all_jobs_with_budget(&self, budget: Duration) -> PtyShutdownReport {
        self.shutdown_local_processes_with_budget(budget)
    }

    fn kill_all_jobs_until(&self, deadline: Instant) -> PtyShutdownReport {
        let started = Instant::now();
        let budget = deadline.saturating_duration_since(started);
        self.shutdown_local_processes_until_deadline(budget, started, deadline)
    }

    fn ownership_diagnostics(&self, operation: &str) -> Vec<String> {
        self.diagnostic_attempts(None, operation)
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        self.fanout.get_screen_snapshot(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.fanout.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        match self.probe_child(id) {
            ChildLiveness::Alive => self
                .fanout
                .get_screen_rows(id)
                .map(ScreenRowsRead::Rows)
                .unwrap_or(ScreenRowsRead::Unavailable),
            ChildLiveness::Exited { .. } | ChildLiveness::Gone => ScreenRowsRead::SessionOver,
            ChildLiveness::Unqueryable(_) => ScreenRowsRead::Unavailable,
        }
    }

    fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        self.fanout
            .register_response_watcher(session_id, request_id, response_dir);
    }
}

#[cfg(test)]
mod probe_containment_tests {
    use super::*;
    use std::collections::HashMap;

    /// A child that panics exactly where portable-pty does when its inner mutex is
    /// poisoned: inside the accessor, with the caller holding the PTY map guard.
    #[derive(Debug)]
    struct PoisonedChild;

    impl portable_pty::ChildKiller for PoisonedChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(PoisonedChild)
        }
    }

    impl portable_pty::Child for PoisonedChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            panic!("called `Result::unwrap()` on a poisoned mutex");
        }

        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            panic!("called `Result::unwrap()` on a poisoned mutex");
        }

        fn process_id(&self) -> Option<u32> {
            None
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            panic!("called `Result::unwrap()` on a poisoned mutex");
        }
    }

    /// grinch D4: the probe runs while the global `ptys` guard is held. A panic inside
    /// portable-pty must not unwind through that guard, because a poisoned `ptys` map
    /// makes the next terminal write panic, with nothing in the log tying it back here.
    #[test]
    fn a_panicking_child_probe_never_poisons_the_pty_map() {
        let ptys: Mutex<HashMap<Uuid, Box<dyn portable_pty::Child + Send + Sync>>> =
            Mutex::new(HashMap::new());
        let id = Uuid::new_v4();
        ptys.lock().unwrap().insert(
            id,
            Box::new(PoisonedChild) as Box<dyn portable_pty::Child + Send + Sync>,
        );

        {
            // Exactly the shape of `probe_child`: guard held, probe called under it.
            let mut guard = ptys.lock().unwrap();
            let child = guard.get_mut(&id).expect("child");
            let liveness = probe_child_contained(child);
            assert!(
                matches!(liveness, ChildLiveness::Unqueryable(_)),
                "a probe that could not ask must never claim the child is alive"
            );
        }

        assert!(
            !ptys.is_poisoned(),
            "the ptys guard must be released normally, not unwound through"
        );
        assert!(
            ptys.lock().is_ok(),
            "the next terminal write still gets the lock"
        );
    }
}

#[cfg(test)]
mod bounded_owner_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct NeverExitsChild {
        kill_calls: Arc<AtomicUsize>,
    }

    impl portable_pty::ChildKiller for NeverExitsChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.kill_calls.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("injected child kill failure"))
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Self {
                kill_calls: Arc::clone(&self.kill_calls),
            })
        }
    }

    impl portable_pty::Child for NeverExitsChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(None)
        }

        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            panic!("bounded local PTY teardown must never call Child::wait")
        }

        fn process_id(&self) -> Option<u32> {
            Some(4242)
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    #[test]
    fn failed_kill_returns_within_budget_and_retains_exact_owner() {
        let id = Uuid::new_v4();
        let kill_calls = Arc::new(AtomicUsize::new(0));
        let owner = LocalProcessOwner::new(
            Box::new(NeverExitsChild {
                kill_calls: Arc::clone(&kill_calls),
            }),
            None,
        );
        #[cfg(unix)]
        let owner = {
            let mut owner = owner;
            owner.process_group_required = false;
            owner
        };
        let mut entries = vec![(id, RetainedLocalProcess::Detached(owner))];
        let budget = Duration::from_millis(30);
        let started = Instant::now();
        let outcomes = shutdown_local_processes(&mut entries, budget);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "failed kill exceeded its explicit budget"
        );
        assert!(kill_calls.load(Ordering::SeqCst) >= 1);
        let diagnostic = outcomes[0]
            .as_ref()
            .expect("failed kill must retain ownership");
        assert!(diagnostic.contains(&id.to_string()));
        assert!(diagnostic.contains("root_pid=Some(4242)"));
        assert!(diagnostic.contains("injected child kill failure"));
        assert!(entries[0].1.owner_mut().child.is_some());
    }

    #[test]
    fn busy_owner_map_access_returns_at_its_deadline() {
        let owner = Mutex::new(());
        let _held = owner.lock().expect("hold owner map");
        let budget = Duration::from_millis(30);
        let started = Instant::now();
        assert!(lock_local_owner_map_until(&owner, started + budget).is_err());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "owner-map contention exceeded its explicit deadline"
        );
    }
}

#[cfg(test)]
mod resize_dedup_tests {
    use super::PtyInstance;
    use std::sync::Mutex;

    /// A `PtyInstance` carries real ConPTY handles, so the tests below drive the one piece
    /// that #973 (C) actually changes: the size cache and the decision it drives.
    fn cache(cols: u16, rows: u16) -> Mutex<(u16, u16)> {
        Mutex::new((cols, rows))
    }

    /// #973 (C) - the frontend calls resize unconditionally from a double
    /// `requestAnimationFrame` and a `ResizeObserver`, so a single attach fires 5-20
    /// identical resizes and ConPTY hands every one of them to the child as a real event.
    /// A resize that changes nothing must not reach the child.
    #[test]
    fn a_resize_to_the_size_it_already_has_is_not_sent_to_the_child() {
        let size = cache(74, 23);
        assert!(
            !PtyInstance::size_changed_in(&size, 74, 23),
            "an identical resize must be skipped: today AC fires 5-20 of these per attach"
        );
    }

    /// The other half: a real resize must still get through, or the terminal would never
    /// follow the window.
    #[test]
    fn a_resize_that_moves_the_size_is_sent() {
        let size = cache(74, 23);
        assert!(
            PtyInstance::size_changed_in(&size, 74, 24),
            "one row is a real resize"
        );
        assert!(
            PtyInstance::size_changed_in(&size, 120, 30),
            "a real resize"
        );
    }

    /// The trap in the dedup: if the cache were updated BEFORE the ConPTY accepted the new
    /// size, a failed resize would leave the cache claiming a size the PTY never took, and
    /// every retry would then be skipped as a no-op - the terminal would be wedged at the
    /// wrong size forever. The cache is only written after `master.resize()` returns Ok.
    #[test]
    fn a_failed_resize_does_not_poison_the_cache_and_the_retry_still_fires() {
        let size = cache(74, 23);
        // resize to 100x40 "fails": remember_size is never reached, so the cache stays put
        assert!(PtyInstance::size_changed_in(&size, 100, 40));
        assert!(
            PtyInstance::size_changed_in(&size, 100, 40),
            "after a failed resize the retry must still be issued, not skipped as a no-op"
        );
        // now it succeeds
        PtyInstance::remember_size_in(&size, 100, 40);
        assert!(!PtyInstance::size_changed_in(&size, 100, 40));
    }
}

#[cfg(test)]
mod startup_gate_tests {
    use super::{
        hand_over_held_size, resize_instance, send_size_to_conpty, LocalProcessOwner, PtyInstance,
        StartupGate,
    };
    use portable_pty::{native_pty_system, MasterPty, PtySize};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    /// A real ConPTY, with no child. `LocalProcessBackend` itself cannot be built here (its
    /// `GitWatcher` needs a Tauri `AppHandle`), and it does not need to be: `PtyBackend::resize`
    /// is a four-line wrapper - lock, delegate to `resize_instance`, broadcast - and
    /// `resize_instance` is the whole of the decision.
    fn conpty(cols: u16, rows: u16) -> (PtyInstance, Arc<Mutex<Box<dyn MasterPty + Send>>>) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        drop(pair.slave);

        let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));
        let instance = PtyInstance {
            master: Arc::clone(&master),
            writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
            owner: LocalProcessOwner {
                generation: 0,
                root_pid: None,
                child: None,
                job: None,
                #[cfg(windows)]
                job_required: false,
                #[cfg(unix)]
                process_group: None,
                #[cfg(unix)]
                process_group_required: false,
                resource_registration: None,
                diagnostics: Vec::new(),
            },
            size: Mutex::new((cols, rows)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered: Arc::new(AtomicBool::new(false)),
        };
        (instance, master)
    }

    /// What the CHILD would see. This is the point of these tests: not what AC believes.
    fn size_the_child_sees(master: &Arc<Mutex<Box<dyn MasterPty + Send>>>) -> (u16, u16) {
        let s = master.lock().unwrap().get_size().expect("get_size");
        (s.cols, s.rows)
    }

    /// #973 - THE COLD-START CASE. This is the one the user hits every morning.
    ///
    /// `TerminalView` lives inside `<Show when={activeSessionId}>`, so when the app opens
    /// with no active session there is no terminal host to measure, and the frontend cannot
    /// supply a size. The PTY is opened at 120x30, the view then mounts, fits, and fires its
    /// resize burst 300-500 ms later - straight into the startup window of the first coding
    /// agent the user launches. **Option A cannot cover this. This is what covers it.**
    ///
    /// Red without the gate: the resize reaches the ConPTY immediately, and the child is
    /// resized while it is still coming up, which is what costs Codex its first content
    /// render (measured: 8 blanks in 10).
    #[test]
    fn a_session_opened_without_a_view_size_holds_the_burst_until_the_child_renders() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        // the view mounts and fits: 6 identical resizes inside a few milliseconds
        for _ in 0..6 {
            let sent = resize_instance(&instance, id, 74, 23).expect("resize");
            assert!(!sent, "nothing may reach a child that has not rendered yet");
        }

        assert_eq!(
            size_the_child_sees(&master),
            (120, 30),
            "the child has rendered nothing yet, so the ConPTY must NOT have been resized"
        );

        // the child paints
        let held = instance
            .startup_gate
            .lock()
            .unwrap()
            .open()
            .expect("the size the view asked for must have been kept");
        send_size_to_conpty(&instance, id, held.0, held.1).expect("apply");

        assert_eq!(
            size_the_child_sees(&master),
            (74, 23),
            "once the child has rendered, the size it should have had must be applied"
        );
    }

    /// The view fires 5-20 resizes per attach. Only the last is real; replaying all of them
    /// at the child would be churn for nothing.
    #[test]
    fn only_the_last_held_size_reaches_the_child() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        resize_instance(&instance, id, 74, 23).expect("resize");
        resize_instance(&instance, id, 90, 40).expect("resize");
        resize_instance(&instance, id, 74, 24).expect("resize");
        assert_eq!(size_the_child_sees(&master), (120, 30), "all of them held");

        let held = instance.startup_gate.lock().unwrap().open().expect("held");
        send_size_to_conpty(&instance, id, held.0, held.1).expect("apply");

        assert_eq!(
            size_the_child_sees(&master),
            (74, 24),
            "only the last size the view asked for should reach the child"
        );
    }

    /// Once the child is up, AC gets out of the way: every later resize - the user dragging
    /// the window - goes straight through, forever. A gate that never reopened would leave
    /// the terminal unable to follow its window.
    #[test]
    fn once_the_child_has_rendered_resizes_go_straight_through() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        instance.startup_gate.lock().unwrap().open(); // the child rendered, nothing held

        assert!(resize_instance(&instance, id, 74, 23).expect("resize"));
        assert_eq!(size_the_child_sees(&master), (74, 23));

        assert!(resize_instance(&instance, id, 100, 50).expect("resize"));
        assert_eq!(
            size_the_child_sees(&master),
            (100, 50),
            "the gate must not close again"
        );
    }

    /// #973 (C) - a resize that changes nothing must not be sent. ConPTY hands even a
    /// same-size resize to the child as a real event, and the view fires 5-20 of them.
    #[test]
    fn a_resize_that_changes_nothing_is_not_sent() {
        let (instance, id) = (conpty(74, 23).0, Uuid::new_v4());
        instance.startup_gate.lock().unwrap().open();

        assert!(
            !resize_instance(&instance, id, 74, 23).expect("resize"),
            "the ConPTY is already 74x23: nothing should be sent"
        );
        assert!(
            resize_instance(&instance, id, 74, 24).expect("resize"),
            "one row is a real resize and must be sent"
        );
    }

    /// #973 - a degenerate resize that lands WHILE THE GATE IS HOLDING must not evict the
    /// real size the view is waiting to give the child.
    ///
    /// This walks the path `pty_resize` actually walks - `resize_instance`, gate and all -
    /// which is the entire point of it. The guard used to sit in `send_size_to_conpty`, PAST
    /// the gate, so nothing checked a size on its way IN: the gate took the `0x0` last-wins
    /// over the real `74x23`, the hand-over popped it, `send_size_to_conpty` refused it, and
    /// the pending slot was consumed and gone. **Nothing retries.** The ConPTY then stays at
    /// the size it was OPENED at, for good - on cold start, a 120x30 child behind a 74x23
    /// terminal, until the user drags the window.
    ///
    /// Red before the guard moved ahead of the gate:
    /// `assertion failed: left: (120, 30)  right: (74, 23)`.
    #[test]
    fn a_degenerate_resize_cannot_evict_the_size_held_for_the_child() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        // the view mounts and fits, while the child is still starting up: held, as it should be
        assert!(
            !resize_instance(&instance, id, 74, 23).expect("resize"),
            "nothing may reach a child that has not rendered yet"
        );

        // ...and now a zero dimension arrives, still inside the startup window. It comes off the
        // wire: `web/commands.rs` takes cols/rows straight from a JSON payload. (Not from xterm -
        // `fit()` clamps to 2x1 and cannot produce a zero.)
        assert!(
            !resize_instance(&instance, id, 0, 0).expect("must not error"),
            "a zero dimension must never be sent to the ConPTY"
        );

        // the child paints. This is the read loop's hand-over itself, not a copy of it.
        hand_over_held_size(&instance, id);

        assert_eq!(
            size_the_child_sees(&master),
            (74, 23),
            "the real held size must survive a degenerate resize"
        );
    }

    /// #973 - the guard INSIDE `send_size_to_conpty`, which is defence in depth: the last line
    /// before `master.resize()`, and the one `hand_over_held_size` relies on when it applies a
    /// size that has been sitting in the gate. The guard that keeps a `0x0` off the request
    /// path is in `resize_instance`, ahead of the gate - see
    /// `a_degenerate_resize_cannot_evict_the_size_held_for_the_child`, which is the one that
    /// walks the path `pty_resize` walks. This test calls `send_size_to_conpty` DIRECTLY and
    /// makes no claim about the gate.
    ///
    /// It caught a real one. I had assumed portable-pty would reject `0x0`. **It does not**:
    /// `master.resize(0x0)` returns Ok and the ConPTY is genuinely set to zero, so before the
    /// guard this failed with `left: (0, 0)`.
    ///
    /// The cache matters as much as the refusal: if a size the ConPTY never took were
    /// cached, every retry would be skipped as a no-op and the terminal would be wedged at
    /// the wrong size forever.
    #[test]
    fn a_degenerate_resize_is_refused_and_does_not_poison_the_cache() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();
        instance.startup_gate.lock().unwrap().open();

        assert!(
            !send_size_to_conpty(&instance, id, 0, 0).expect("must not error"),
            "a zero dimension must never be sent to the ConPTY"
        );
        assert_eq!(
            size_the_child_sees(&master),
            (120, 30),
            "ConPTY accepts a 0x0 resize without complaint, so AC has to refuse it"
        );
        assert_eq!(
            *instance.size.lock().unwrap(),
            (120, 30),
            "and it must not be cached, or the next real resize is skipped as a no-op"
        );

        assert!(
            send_size_to_conpty(&instance, id, 74, 23).expect("resize"),
            "a real resize after a refused one must still go through"
        );
        assert_eq!(size_the_child_sees(&master), (74, 23));
    }

    /// The gate is a two-state machine. Pin it directly, including the ordering trap: the
    /// hand-over happens exactly once, and after it the gate is open for good.
    #[test]
    fn the_gate_hands_over_exactly_once() {
        let mut gate = StartupGate::Holding(None);
        assert_eq!(gate.on_resize(74, 23), None, "held");
        assert_eq!(gate.on_resize(74, 24), None, "held, replacing the first");
        assert_eq!(
            gate.open(),
            Some((74, 24)),
            "the last held size, handed over once"
        );
        assert_eq!(gate.open(), None, "a second open hands over nothing");
        assert_eq!(
            gate.on_resize(80, 25),
            Some((80, 25)),
            "the gate is open now: resizes go straight through"
        );
    }
}

#[cfg(test)]
mod context_gate_tests {
    use super::{
        probe_child_in, screen_rows_if_child_alive, ChildLiveness, LocalProcessOwner, PtyInstance,
        StartupGate,
    };
    use crate::pty::context_scrape::ScreenRowsRead;
    use crate::pty::idle_detector::IdleDetector;
    use crate::pty::output::{PtyOutputTarget, SessionIoFanout};
    use crate::session::profile::IdleTuning;
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    /// The statusline the agent painted, and the one that stays on the grid after it dies.
    const ROW: &str = "  Context \u{2591}\u{2591}\u{2588} 42%";

    /// A real ConPTY with a REAL CHILD on the far end, plus the real fanout.
    ///
    /// The child is the entire point. This gate's whole job is to answer a question the grid
    /// cannot - is the child alive? - so a fake child cannot be asked it wrongly, which means
    /// it cannot be asked at all. `startup_gate_tests::conpty` already opens the real ConPTY
    /// and builds the real `PtyInstance`; this fills in its `child: None`.
    fn conpty_with_child(cols: u16, rows: u16) -> (Mutex<HashMap<Uuid, PtyInstance>>, Uuid) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        // A child that sits and waits, the way an agent CLI does.
        let shell = if cfg!(windows) { "cmd.exe" } else { "sh" };
        let child = pair
            .slave
            .spawn_command(CommandBuilder::new(shell))
            .expect("spawn a real child");
        let owner = LocalProcessOwner::new(child, None);
        #[cfg(unix)]
        let owner = {
            let mut owner = owner;
            let mut process_group =
                super::UnixProcessGroupOwner::unverified_for_child_pid(owner.root_pid)
                    .expect("capture real child process group");
            process_group
                .verify_identity()
                .expect("verify real child process group");
            owner.process_group = Some(process_group);
            owner
        };
        drop(pair.slave);

        let writer = pair.master.take_writer().expect("take_writer");
        let instance = PtyInstance {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            owner,
            size: Mutex::new((cols, rows)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered: Arc::new(AtomicBool::new(false)),
        };

        let id = Uuid::new_v4();
        let mut map = HashMap::new();
        map.insert(id, instance);
        (Mutex::new(map), id)
    }

    fn fanout() -> SessionIoFanout {
        SessionIoFanout::new(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
        )
    }

    fn paint(fanout: &SessionIoFanout, id: Uuid) {
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        fanout.handle_output(
            &PtyOutputTarget::noop(),
            id,
            &id.to_string(),
            ROW.as_bytes().to_vec(),
        );
    }

    /// Kill the child and wait until it is REALLY gone - which is NOT what
    /// `portable_pty::Child::wait()` waits for, and finding that out cost this test a failing
    /// run.
    ///
    /// `TerminateProcess` is asynchronous, and `wait()` short-circuits on `try_wait()` ->
    /// `is_complete()` -> `GetExitCodeProcess`. The exit code is readable ~14 ms (measured
    /// here) BEFORE the kernel signals the process object, so `wait()` returns `Ok(1)` while
    /// `WaitForSingleObject(h, 0)` still answers WAIT_TIMEOUT. That is #942 arriving from the
    /// other side: `is_complete` is exactly the accessor AC's oracle refuses to trust, and
    /// the oracle asks the stronger question on purpose. Production never notices the gap -
    /// 14 ms against a 5 s tick - but a test that kills and reads in the same breath lands
    /// inside it every time.
    fn kill_and_await_real_exit(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) {
        {
            let mut map = ptys.lock().unwrap();
            let child = map
                .get_mut(&id)
                .expect("instance")
                .owner
                .child
                .as_mut()
                .expect("child");
            child.kill().expect("kill the child");
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !matches!(probe_child_in(ptys, id), ChildLiveness::Alive) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the oracle answered Alive for 10s after TerminateProcess: it cannot see deaths");
    }

    /// The live case: a real child, a real grid with a real statusline on it, and the gate
    /// hands the rows straight through.
    #[test]
    fn rows_are_readable_while_the_child_is_alive() {
        let (ptys, id) = conpty_with_child(120, 30);
        let fanout = fanout();
        paint(&fanout, id);

        match screen_rows_if_child_alive(&ptys, &fanout, id) {
            ScreenRowsRead::Rows(rows) => assert_eq!(rows[0], ROW),
            ScreenRowsRead::Unavailable => panic!("a live child with a live parser must read"),
            ScreenRowsRead::SessionOver => panic!("the child is alive; the session is not over"),
        }
    }

    /// The case the whole gate exists for, and it is RED if the probe call is deleted.
    ///
    /// The parser is never touched here, so the row stays on the frozen grid verbatim,
    /// exactly as it does in production: a killed process cannot repaint, and no EOF ever
    /// arrives to notice. Without the probe this reads a perfectly well-formed `42%` off a
    /// dead session forever, and no rule expressible in the user pattern can tell.
    #[test]
    fn session_over_once_the_child_actually_exits() {
        let (ptys, id) = conpty_with_child(120, 30);
        let fanout = fanout();
        paint(&fanout, id);

        kill_and_await_real_exit(&ptys, id);

        assert!(
            matches!(
                screen_rows_if_child_alive(&ptys, &fanout, id),
                ScreenRowsRead::SessionOver
            ),
            "the child is gone, so the session is over - whatever the frozen grid still says"
        );

        // ...and the row really is still sitting there. This is the fact that makes the probe
        // load-bearing rather than belt-and-braces.
        assert_eq!(
            fanout.get_screen_rows(id).expect("the parser is untouched")[0],
            ROW,
            "if this goes red the grid started self-correcting and the gate premise changed"
        );
    }

    /// No instance at all is the other definite answer: `Gone`, so the session is over.
    #[test]
    fn session_over_when_there_is_no_pty_instance() {
        let (ptys, _id) = conpty_with_child(120, 30);
        let fanout = fanout();
        assert!(matches!(
            screen_rows_if_child_alive(&ptys, &fanout, Uuid::new_v4()),
            ScreenRowsRead::SessionOver
        ));
    }

    /// A live child whose parser is missing is a DESYNC, not a dead session, and the caller
    /// must keep sampling it. This is the two-state collapse `ScreenRowsRead` exists to
    /// prevent, pinned at the one seam where the real oracle is involved.
    #[test]
    fn a_live_child_with_no_parser_is_unavailable_not_over() {
        let (ptys, id) = conpty_with_child(120, 30);
        let fanout = fanout();
        // deliberately never registered with the fanout

        assert!(
            matches!(
                screen_rows_if_child_alive(&ptys, &fanout, id),
                ScreenRowsRead::Unavailable
            ),
            "the child is alive, so nothing here may claim the session is over"
        );
    }
}

#[cfg(all(test, windows))]
mod blocked_writer_teardown_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn ptys_with_nonreading_child() -> (Arc<Mutex<HashMap<Uuid, PtyInstance>>>, Uuid) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open real ConPTY");
        let mut command = CommandBuilder::new("powershell.exe");
        command.arg("-NoProfile");
        command.arg("-NonInteractive");
        command.arg("-Command");
        command.arg("Start-Sleep -Seconds 30");
        let child = pair
            .slave
            .spawn_command(command)
            .expect("spawn non-reading child");
        drop(pair.slave);
        let writer = pair.master.take_writer().expect("take ConPTY writer");
        let id = Uuid::new_v4();
        let ptys = Arc::new(Mutex::new(HashMap::from([(
            id,
            PtyInstance {
                master: Arc::new(Mutex::new(pair.master)),
                writer: Arc::new(Mutex::new(writer)),
                owner: LocalProcessOwner::new(child, None),
                size: Mutex::new((120, 30)),
                startup_gate: Mutex::new(StartupGate::Holding(None)),
                rendered: Arc::new(AtomicBool::new(false)),
            },
        )])));
        (ptys, id)
    }

    // Real-ConPTY teardown evidence, kept out of the default parallel run.
    //
    // This spawns a genuine ConPTY child and relies on the OS pipe buffer filling so a
    // 65,536-byte write stays blocked long enough to be observed within a ~250 ms detection
    // window. That precondition (the `observed_block` assertion below) is parallel-load
    // sensitive: it holds deterministically when the test runs in isolation, but under the
    // full parallel `cargo test --lib` run the write can drain before detection, so the
    // precondition flakes. The teardown/unblock behavior actually under test is sound; only
    // the setup precondition is timing fragile, and it is a real platform boundary that
    // cannot be reliably simulated under automated parallel load.
    //
    // The frozen plan therefore routes real-ConPTY behavior to manual Windows evidence
    // (section 14 acceptance item 10: automated assertions everywhere, with ConPTY manual
    // evidence only where the real platform boundary cannot be simulated; section 15 Windows
    // manual ConPTY/API matrix). It stays `#[ignore]`d out of the default automated gate and
    // is exercised on demand, in isolation, as part of that manual matrix:
    //
    //   cargo test --lib real_blocked_conpty_writer_is_unblocked_by_session_teardown -- --ignored --test-threads=1
    #[test]
    #[ignore = "real ConPTY block precondition is parallel-load sensitive; run in isolation as manual Windows matrix evidence (plan section 14 item 10 / section 15)"]
    fn real_blocked_conpty_writer_is_unblocked_by_session_teardown() {
        let (ptys, id) = ptys_with_nonreading_child();
        let started = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = mpsc::channel();
        let writer_ptys = Arc::clone(&ptys);
        let writer_started = Arc::clone(&started);
        let writer_completed = Arc::clone(&completed);
        let writer = std::thread::spawn(move || {
            let chunk = vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES];
            let outcome = (0..4096).try_for_each(|_| {
                writer_started.fetch_add(1, Ordering::SeqCst);
                write_to_local_pty(&writer_ptys, id, &chunk).map_err(|error| error.to_string())?;
                writer_completed.fetch_add(1, Ordering::SeqCst);
                Ok::<(), String>(())
            });
            done_tx.send(outcome).expect("publish writer completion");
        });

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed_block = false;
        while Instant::now() < deadline {
            let before = (
                started.load(Ordering::SeqCst),
                completed.load(Ordering::SeqCst),
            );
            if before.0 > before.1 {
                std::thread::sleep(Duration::from_millis(250));
                let after = (
                    started.load(Ordering::SeqCst),
                    completed.load(Ordering::SeqCst),
                );
                if after == before {
                    observed_block = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            observed_block,
            "the real ConPTY child must leave a writer blocked before teardown"
        );

        let kill_started = Instant::now();
        let mut instance = remove_local_pty(&ptys, id).expect("remove blocked PTY instance");
        if let Some(mut child) = instance.owner.child.take() {
            child.kill().expect("terminate non-reading child");
        }
        drop(instance);
        assert!(
            kill_started.elapsed() < Duration::from_secs(2),
            "teardown must not wait on the blocked writer mutex"
        );
        let outcome = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("child teardown must unblock the real ConPTY write");
        assert!(outcome.is_err(), "the broken PTY write must report failure");
        writer.join().expect("join blocked writer thread");
        assert!(!ptys.lock().unwrap().contains_key(&id));
    }
}
