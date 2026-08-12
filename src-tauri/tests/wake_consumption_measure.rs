//! (#1001 PR1) On-demand measurement harness for the wake-consumption oracle.
//!
//! WHY THIS EXISTS (plan section 16.1): two output-stream oracles have already
//! died the same way - the echo/repaint of a pasted body IS output, produced
//! WITHOUT consuming the turn, so no output-only signal is provable on paper.
//! This harness therefore does not confirm a signal; it FALSIFIES candidates
//! against an echo-immune, out-of-band ground truth.
//!
//! GROUND TRUTH (echo-immune, by construction, plan 16.1): the wake body tells
//! the agent to run a tool that appends a unique marker line to a file in a
//! harness-controlled dir. The harness polls the FILESYSTEM, never the PTY
//! stream. The file gains a line iff the agent actually executed the turn; the
//! echo of the instruction cannot create it. A per-attempt marker measures
//! duplicate submission (F7/G4) directly as marker-count.
//!
//! CANDIDATE SIGNALS measured against that GT, per agent, on Windows/ConPTY:
//!   1. bare `waiting_for_input` flip  (IdleDetector idle_set, via purge_readiness)
//!   2. post-submit activity timestamp gate  (has_printable_activity_since, sec 14.1)
//!   3. screen-state: body no longer in the input box AND transcript grew
//!      (vt100 screen via get_screen_snapshot, sec 16.1 candidate #3)
//!
//! For each: false-positive (signal=consumed, GT=not -> masks the bug) and
//! false-negative (signal=not, GT=consumed -> needless redeliver, couples G4).
//!
//! AGENT-AGNOSTIC: the agent command is read from env so the SAME instrument
//! runs whichever agent is installed, with ZERO code change (tech-lead guidance):
//!   AC_WAKE_HARNESS_SHELL  (default "claude")
//!   AC_WAKE_HARNESS_ARGS   (default "--dangerously-skip-permissions"; space split)
//!   AC_WAKE_HARNESS_AGENT  (report label; default = shell)
//!   AC_WAKE_HARNESS_TRIALS (default "5")
//!   AC_WAKE_HARNESS_SIGNAL_WINDOW_MS (default "6000")
//!   AC_WAKE_HARNESS_GT_TIMEOUT_MS    (default "60000")
//!   AC_WAKE_HARNESS_INJECT_MODE      (ready | first_idle | immediate | pi_logical_clear; default ready)
//!   AC_WAKE_HARNESS_REDELIVER_MODE   (immediate | settled; default immediate)
//!   AC_WAKE_HARNESS_SETTLE_HOLD_MS   (sustained paste-ready hold; default "3500")
//! Never fabricated: if the agent binary is absent, the harness prints a SKIP
//! line and returns (it does not assert), so a run on a box without that agent
//! is an honest no-op, not a fake pass.
//!
//! RUN (Windows, agent installed + authenticated):
//!   cargo test --test wake_consumption_measure -- --ignored --nocapture
//! It prints a per-signal FP/FN table, the F7 lingering answer, and the
//! live-path fresh-idle drop rate.

#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentscommander_lib::commands::pty::get_screen_snapshot;
use agentscommander_lib::commands::session::{
    create_session_inner, destroy_session_inner, CreateSelectionIntent,
};
use agentscommander_lib::config::settings::{AppSettings, SettingsState};
use agentscommander_lib::pty::backend::PtyViewport;
use agentscommander_lib::pty::git_watcher::GitWatcher;
use agentscommander_lib::pty::idle_detector::IdleDetector;
use agentscommander_lib::pty::inject::inject_text_into_session;
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::resource_monitor::ResourceMonitorState;
use agentscommander_lib::session::manager::SessionManager;
use agentscommander_lib::session::selection::SelectionCoordinator;
use agentscommander_lib::shutdown::ShutdownSignal;
use agentscommander_lib::telegram::manager::{
    OutputSenderMap, TelegramBridgeManager, TelegramBridgeState,
};
use agentscommander_lib::voice::tracker::{VoiceTracker, VoiceTrackingState};
use agentscommander_lib::web::auth::WebAccessToken;
use agentscommander_lib::web::broadcast::WsBroadcaster;
use agentscommander_lib::{
    AppOutbox, ConfigSeedLockState, DetachedSessionsState, MasterToken, RestoreInProgress,
    SpecBoardState, WebServerHandle,
};
use tauri::Manager;
use uuid::Uuid;

// ─────────────────────────── env-driven config ───────────────────────────

struct HarnessConfig {
    shell: String,
    args: Vec<String>,
    agent_label: String,
    trials: usize,
    signal_window: Duration,
    gt_timeout: Duration,
    inject_mode: String,
    immediate_delay: Duration,
    redeliver_mode: String,
    settle_hold: Duration,
    live_settle: String,
    live_warmup: String,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl HarnessConfig {
    fn from_env() -> Self {
        let shell = env_or("AC_WAKE_HARNESS_SHELL", "claude");
        let args_raw = env_or("AC_WAKE_HARNESS_ARGS", "--dangerously-skip-permissions");
        let args = args_raw
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let agent_label = env_or("AC_WAKE_HARNESS_AGENT", &shell);
        let trials = env_or("AC_WAKE_HARNESS_TRIALS", "5")
            .parse()
            .unwrap_or(5usize);
        let signal_window = Duration::from_millis(
            env_or("AC_WAKE_HARNESS_SIGNAL_WINDOW_MS", "6000")
                .parse()
                .unwrap_or(6000),
        );
        let gt_timeout = Duration::from_millis(
            env_or("AC_WAKE_HARNESS_GT_TIMEOUT_MS", "60000")
                .parse()
                .unwrap_or(60000),
        );
        let inject_mode = env_or("AC_WAKE_HARNESS_INJECT_MODE", "ready");
        let redeliver_mode = env_or("AC_WAKE_HARNESS_REDELIVER_MODE", "immediate");
        let live_settle = env_or("AC_WAKE_HARNESS_LIVE_SETTLE", "on");
        let live_warmup = env_or("AC_WAKE_HARNESS_LIVE_WARMUP", "on");
        let settle_hold = Duration::from_millis(
            env_or("AC_WAKE_HARNESS_SETTLE_HOLD_MS", "3500")
                .parse()
                .unwrap_or(3500),
        );
        let immediate_delay = Duration::from_millis(
            env_or("AC_WAKE_HARNESS_IMMEDIATE_DELAY_MS", "1500")
                .parse()
                .unwrap_or(1500),
        );
        Self {
            shell,
            args,
            agent_label,
            trials,
            signal_window,
            gt_timeout,
            inject_mode,
            immediate_delay,
            redeliver_mode,
            settle_hold,
            live_settle,
            live_warmup,
        }
    }
}

/// Resolve the agent binary on PATH (or as an absolute path). Returns None when
/// it is not installed, so the harness can SKIP honestly rather than fake a run.
fn agent_available(shell: &str) -> bool {
    if Path::new(shell).is_absolute() {
        return Path::new(shell).exists();
    }
    let exts = ["", ".exe", ".cmd", ".bat"];
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(';') {
            for ext in exts {
                if Path::new(dir).join(format!("{shell}{ext}")).exists() {
                    return true;
                }
            }
        }
    }
    false
}

// ───────────────────────────── app context ───────────────────────────────

struct HarnessCtx {
    app: tauri::App,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    idle: Arc<IdleDetector>,
    output_senders: OutputSenderMap,
    _shutdown: ShutdownSignal,
    _temp: tempfile::TempDir,
}

/// Build a minimal but REAL app context (mirrors tests/pty_lifecycle_regression
/// make_test_app) and START the idle watcher so the `waiting_for_input`/idle_set
/// transition (candidate signal 1) actually fires. We hold the `Arc<IdleDetector>`
/// so signals 1 and 2 read the real detector directly.
fn make_ctx(repo_root: &Path) -> HarnessCtx {
    let temp = tempfile::TempDir::new().expect("temp");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(Arc::clone(&output_senders)),
    ));
    // No-op callbacks: signal 1 is read from the detector's idle_set via
    // purge_readiness, so we do not need to mirror into SessionManager here.
    let idle: Arc<IdleDetector> = IdleDetector::new(|_| {}, |_| {});
    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new({
        // #1077: AppSettings has a crate-private hidden field; build from Default.
        let mut s = AppSettings::default();
        s.default_shell = "powershell.exe".to_string();
        s.default_shell_args = vec!["-NoLogo".to_string()];
        s.project_paths = vec![repo_root.to_string_lossy().to_string()];
        s
    }));
    let git_app = Box::leak(Box::new(
        tauri::Builder::default()
            .any_thread()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("git handle app"),
    ));
    let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), git_app.handle().clone());
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
        Arc::clone(&output_senders),
        Arc::clone(&idle),
        Arc::clone(&git_watcher),
        None,
        None,
    )));

    let detached: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));
    let spec_board: SpecBoardState = Arc::new(tokio::sync::RwLock::new(
        agentscommander_lib::commands::spec_board::SpecBoardManager::new(),
    ));
    let config_seed_lock: ConfigSeedLockState = Arc::new(tokio::sync::Mutex::new(()));
    let shutdown = ShutdownSignal::new();
    let selection_coordinator =
        SelectionCoordinator::new(Arc::clone(&session_mgr), shutdown.token().clone());

    let app = tauri::Builder::default()
        .any_thread()
        .manage(MasterToken::new("wake-consume-master".into()))
        .manage(AppOutbox::new(
            repo_root.join(".app-outbox").to_string_lossy().to_string(),
        ))
        .manage(settings)
        .manage(Arc::clone(&session_mgr))
        .manage(selection_coordinator.clone())
        .manage(tg_mgr)
        .manage(detached)
        .manage(voice)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(shutdown.clone())
        .manage(Arc::new(WebAccessToken::new("wake-consume-web".into())))
        .manage(WsBroadcaster::new())
        .manage(WebServerHandle::default())
        .manage(spec_board)
        .manage(config_seed_lock)
        .manage(Arc::clone(&git_watcher))
        .manage(Arc::new(ResourceMonitorState::new()))
        .manage(Arc::clone(&pty_mgr))
        .manage(Arc::clone(&idle))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build harness app");

    selection_coordinator
        .start(app.handle().clone())
        .expect("start selection coordinator");
    let bootstrap = selection_coordinator.clone();
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(async move {
            bootstrap
                .submit_restore_first()
                .await
                .expect("open selection coordinator")
                .finish();
        });
    })
    .join()
    .expect("join selection bootstrap");

    idle.start(shutdown.clone());

    HarnessCtx {
        app,
        session_mgr,
        pty_mgr,
        idle,
        output_senders,
        _shutdown: shutdown,
        _temp: temp,
    }
}

// ───────────────────────────── signals + GT ──────────────────────────────

/// Signal 1: is the session currently in the idle set (waiting_for_input == true
/// equivalent)? Read from the real detector, watcher-driven.
fn watcher_idle(idle: &Arc<IdleDetector>, id: Uuid) -> bool {
    idle.purge_readiness(&[id])
        .first()
        .map(|r| r.watcher_idle)
        .unwrap_or(false)
}

/// Strip the common SGR/CSI escapes so a plain-text screen can be scanned for the
/// body token. Deliberately simple: the screen signal is per-agent-approximate by
/// construction (plan 16.1), and this only needs contiguous ASCII tokens.
fn strip_ansi(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC: skip a CSI/OSC-ish run until a letter/terminator.
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                // ESC + one/two byte sequence: drop next char.
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn screen_text(app: &tauri::App, id: Uuid) -> String {
    let state = app.state::<Arc<Mutex<PtyManager>>>();
    match get_screen_snapshot(state, id.to_string()) {
        Ok(Some(snap)) => strip_ansi(&snap.data),
        _ => String::new(),
    }
}

fn stable_screen_snapshot(app: &tauri::App, id: Uuid) -> Result<(u64, String), String> {
    let state = app.state::<Arc<Mutex<PtyManager>>>();
    get_screen_snapshot(state, id.to_string())
        .map_err(|error| format!("screen snapshot failed: {error}"))?
        .map(|snapshot| (snapshot.sequence, strip_ansi(&snapshot.data)))
        .ok_or_else(|| format!("screen snapshot unavailable for session {id}"))
}

fn nonblank_lines(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

/// Wait until the session is SUSTAINED paste-ready (watcher idle AND rendered
/// content, held continuously for `hold`), mirroring B's live-path settle.
/// Returns true if it settled, false if `deadline` passed first.
async fn wait_for_settle(ctx: &HarnessCtx, id: Uuid, hold: Duration, deadline: Instant) -> bool {
    let mut ready_since: Option<Instant> = None;
    while Instant::now() < deadline {
        let ready = watcher_idle(&ctx.idle, id) && nonblank_lines(&screen_text(&ctx.app, id)) > 0;
        if ready {
            let since = *ready_since.get_or_insert_with(Instant::now);
            if Instant::now().duration_since(since) >= hold {
                return true;
            }
        } else {
            ready_since = None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// Signal 3: the body token is no longer visible in the input-box region (last
/// few non-blank lines) AND the transcript grew vs the pre-inject snapshot.
fn screen_consumed(pre_lines: usize, post: &str, body_token: &str) -> bool {
    let last_region: String = post
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .join("\n");
    let body_in_box = last_region.contains(body_token);
    let grew = nonblank_lines(post) > pre_lines;
    !body_in_box && grew
}

/// Ground truth: count marker lines the agent actually wrote to its side-effect
/// file. Echo-immune: only a real tool execution creates them.
fn gt_marker_count(file: &Path) -> usize {
    std::fs::read_to_string(file)
        .map(|s| s.lines().filter(|l| l.contains("AC_WAKE_MARK")).count())
        .unwrap_or(0)
}

/// The wake body: instruct the agent to append a per-attempt marker line via a
/// shell tool. Same string whichever agent runs it (agent-agnostic).
fn wake_body(file: &Path, trial: usize, attempt: usize) -> String {
    // PowerShell Add-Content is available on every Windows box the app targets.
    let path = file.to_string_lossy().replace('\'', "''");
    format!(
        "Run exactly this one shell command now, then stop and do nothing else:\n\
         powershell -NoProfile -Command \"Add-Content -LiteralPath '{path}' -Value 'AC_WAKE_MARK {trial}-{attempt}'\"\n"
    )
}

// ───────────────────────────── per-trial record ──────────────────────────

#[derive(Default, Clone, Copy)]
struct SignalTally {
    fp: usize,  // signal said consumed, GT said not
    fn_: usize, // signal said not-consumed, GT said consumed
    total: usize,
}

impl SignalTally {
    fn observe(&mut self, signal_consumed: bool, gt_consumed: bool) {
        self.total += 1;
        match (signal_consumed, gt_consumed) {
            (true, false) => self.fp += 1,
            (false, true) => self.fn_ += 1,
            _ => {}
        }
    }
}

// ─────────────────────────────── the harness ─────────────────────────────

/// (#1001 PR2 P2) Faithfully replicate B's live settle decision, so the post-fix
/// number reflects the SHIPPED gate, not a proxy. Option-a first routes on
/// `alive_age` (`live_wake_route`): a STARTING candidate takes the sustained-idle
/// settle; an ESTABLISHED one runs `live_settle_action`'s activity_age loop (busy
/// fast-path, long-idle inject, fresh-idle wait, resize wait, cap).
async fn settle_like_b(ctx: &HarnessCtx, id: Uuid, max_wait: Duration) {
    // (#1001 PR2 P2 option-a) Mirror prod: classify starting vs established by
    // alive_age. Kept in sync with mailbox.rs STARTUP_SETTLE_THRESHOLD (pub(crate),
    // unreachable from this integration-test crate, so hardcoded like FRESH_IDLE_GUARD).
    const STARTUP_THRESHOLD: Duration = Duration::from_secs(20);
    if ctx
        .idle
        .alive_age(id)
        .is_some_and(|a| a < STARTUP_THRESHOLD)
    {
        // Starting: route to the sustained-idle settle, mirroring prod's #611 path
        // (cold-spawn params: 90s cap, 2s hold). FIDELITY CAVEAT (grinch F2): this
        // wait_for_settle proxy is STRICTER than prod's idle-only settle_until_ready
        // (it also requires rendered content), so it injects LATER and is LESS likely
        // to drop. It can therefore HIDE a drop the earlier-injecting shipped gate
        // would take; it does NOT prove the shipped Starting gate drop-free. The
        // Starting gate's real validation is #611's production track record - it is
        // the same idle-only sustained settle already shipped for cold-spawn - not
        // this harness arm.
        let deadline = Instant::now() + Duration::from_secs(90);
        wait_for_settle(ctx, id, Duration::from_millis(2000), deadline).await;
        return;
    }
    // Established: the real-time activity_age loop (unchanged).
    let start = Instant::now();
    let poll = Duration::from_millis(500);
    loop {
        let Some(r) = ctx.idle.purge_readiness(&[id]).into_iter().next() else {
            return;
        };
        if start.elapsed() >= max_wait {
            return;
        }
        if let Some(rz) = r.last_resize_age {
            if rz < r.resize_grace {
                tokio::time::sleep(poll).await;
                continue;
            }
        }
        let settle = r.idle_threshold + Duration::from_millis(1000); // FRESH_IDLE_GUARD
        match r.activity_age {
            None => return,
            Some(a) if a < r.idle_threshold => return, // busy fast-path
            Some(a) if a >= settle => return,          // long-idle
            Some(_) => tokio::time::sleep(poll).await, // fresh-idle window
        }
    }
}

/// (#1001 PR2 P2 option-a) DERIVE the "starting" threshold from evidence. Spawns
/// the agent and records `alive_age` at the instant it FIRST holds sustained
/// paste-ready - `wait_for_settle`'s definition (watcher idle AND rendered
/// content held for `settle_hold`), the same paste-ready notion B's live settle
/// targets. `wait_for_settle` returns AFTER the hold, so first-ready alive_age =
/// alive_age - settle_hold (up to a 200ms poll of slack). Prints per-trial and
/// min/mean/max so the startup threshold is set to max + margin, not a guess. No
/// production code path runs here; this only measures the boot timing that feeds
/// the `STARTUP_SETTLE_THRESHOLD` constant.
async fn run_startup_probe(cfg: &HarnessConfig, ctx: &HarnessCtx) {
    println!(
        "\n=== startup-probe (agent='{}', settle_hold={:?}) ===",
        cfg.agent_label, cfg.settle_hold
    );
    let mut first_ready: Vec<Duration> = Vec::new();
    let mut raw_ready: Vec<Duration> = Vec::new();
    for trial in 0..cfg.trials {
        let trial_dir = ctx._temp.path().join(format!("probe{trial}"));
        std::fs::create_dir_all(&trial_dir).expect("trial dir");

        let info = match create_session_inner(
            ctx.app.handle(),
            &ctx.session_mgr,
            &ctx.pty_mgr,
            cfg.shell.clone(),
            cfg.args.clone(),
            trial_dir.to_string_lossy().to_string(),
            Some(format!("wake-probe-{trial}")),
            None,
            Some(cfg.agent_label.clone()),
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        {
            Ok(info) => info,
            Err(e) => {
                println!("trial {trial}: spawn failed: {e}");
                continue;
            }
        };
        let id = Uuid::parse_str(&info.id).expect("uuid");

        let deadline = Instant::now() + Duration::from_secs(60);
        if wait_for_settle(ctx, id, cfg.settle_hold, deadline).await {
            // alive_age is measured from registered_at (set at PTY spawn), so it
            // is the authoritative "alive since"; subtract the hold to recover the
            // instant the session BECAME ready.
            let raw = ctx.idle.alive_age(id).unwrap_or_default();
            let fr = raw.checked_sub(cfg.settle_hold).unwrap_or(raw);
            raw_ready.push(raw);
            first_ready.push(fr);
            println!(
                "trial {trial}: first sustained-ready alive_age={:?} (raw held-ready alive_age={:?}, hold={:?})",
                fr, raw, cfg.settle_hold
            );
        } else {
            println!("trial {trial}: never reached sustained paste-ready within 60s");
        }
        let _ = destroy_session_inner(ctx.app.handle(), id).await;
    }

    println!(
        "\n--- STARTUP-PROBE RESULT (agent='{}') ---",
        cfg.agent_label
    );
    if first_ready.is_empty() {
        println!("no samples (no session reached sustained paste-ready)");
    } else {
        let stat = |v: &[Duration]| {
            let min = v.iter().min().copied().unwrap_or_default();
            let max = v.iter().max().copied().unwrap_or_default();
            let mean = v.iter().sum::<Duration>() / v.len() as u32;
            (min, mean, max)
        };
        let (fmin, fmean, fmax) = stat(&first_ready);
        let (rmin, rmean, rmax) = stat(&raw_ready);
        println!(
            "first-ready alive_age: n={} min={:?} mean={:?} max={:?}",
            first_ready.len(),
            fmin,
            fmean,
            fmax
        );
        println!(
            "held-ready alive_age:  n={} min={:?} mean={:?} max={:?}",
            raw_ready.len(),
            rmin,
            rmean,
            rmax
        );
        println!(
            "suggested STARTUP_SETTLE_THRESHOLD >= max first-ready ({:?}) + margin",
            fmax
        );
    }
    println!("=== end ===\n");
}

/// (#1001 PR2 P2, the grinch-P2 gate) Live-path drop baseline. Reuses an
/// ALREADY-LIVE session: spawn, run a settled warm-up turn (so it is used, not a
/// fresh cold-spawn), let it return to fresh-idle after that turn, then wake it
/// AGAIN in the `[idle_threshold, idle_threshold + guard]` window and measure the
/// drop via the echo-immune GT. `AC_WAKE_HARNESS_LIVE_SETTLE`:
///  - "off": PRE-fix behaviour - fire wake #2 at fresh-idle, no settle.
///  - "on":  POST-fix - apply the fix's settle (wait activity_age >=
///    idle_threshold + guard) before wake #2.
///
/// The live Inject path (`deliver_wake` is pub(crate), `settle_live_before_inject`
/// is private) is unreachable from an integration test, so this replicates the
/// fix's exact timing gate on real `activity_age` rather than routing through it.
/// The measured quantity - does a fresh-idle live wake drop, with vs without the
/// settle - is identical.
async fn run_live_reuse(cfg: &HarnessConfig, ctx: &HarnessCtx) {
    println!(
        "\n=== live-reuse baseline (agent='{}', live_settle='{}') ===",
        cfg.agent_label, cfg.live_settle
    );
    let mut measured = 0usize;
    let mut dropped = 0usize;
    let mut warmup_failed = 0usize;

    for trial in 0..cfg.trials {
        let trial_dir = ctx._temp.path().join(format!("reuse{trial}"));
        std::fs::create_dir_all(&trial_dir).expect("trial dir");
        let gt_file = trial_dir.join("marks.txt");

        let info = match create_session_inner(
            ctx.app.handle(),
            &ctx.session_mgr,
            &ctx.pty_mgr,
            cfg.shell.clone(),
            cfg.args.clone(),
            trial_dir.to_string_lossy().to_string(),
            Some(format!("wake-reuse-{trial}")),
            None,
            Some(cfg.agent_label.clone()),
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        {
            Ok(info) => info,
            Err(e) => {
                println!("trial {trial}: spawn failed: {e}");
                continue;
            }
        };
        let id = Uuid::parse_str(&info.id).expect("uuid");

        // Warm-up turn (wake #1), settled so it reliably runs: makes this an
        // already-live, already-used session rather than a fresh cold-spawn. With
        // AC_WAKE_HARNESS_LIVE_WARMUP=off it is skipped, so wake #2 lands in the
        // session's STARTUP fresh-idle (an existing-but-still-starting candidate,
        // the not-paste-ready case B actually protects).
        let warm_count = if cfg.live_warmup == "on" {
            let boot = Instant::now() + Duration::from_secs(45);
            wait_for_settle(ctx, id, Duration::from_millis(3500), boot).await;
            let _ = inject_text_into_session(ctx.app.handle(), id, &wake_body(&gt_file, trial, 0))
                .await;
            let warm_deadline = Instant::now() + cfg.gt_timeout;
            while gt_marker_count(&gt_file) < 1 && Instant::now() < warm_deadline {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if gt_marker_count(&gt_file) < 1 {
                warmup_failed += 1;
                println!("trial {trial}: warm-up turn did not run; skipping");
                let _ = destroy_session_inner(ctx.app.handle(), id).await;
                continue;
            }
            gt_marker_count(&gt_file)
        } else {
            0
        };

        // Return to FRESH-idle after the warm-up turn (watcher_idle just true).
        let fi_deadline = Instant::now() + Duration::from_secs(30);
        while !watcher_idle(&ctx.idle, id) && Instant::now() < fi_deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // POST-fix applies the settle; PRE-fix ("off") injects here, in the
        // fresh-idle danger window.
        if cfg.live_settle == "on" {
            settle_like_b(ctx, id, Duration::from_secs(10)).await;
        }

        // Measured wake #2.
        let _ =
            inject_text_into_session(ctx.app.handle(), id, &wake_body(&gt_file, trial, 1)).await;
        let m_deadline = Instant::now() + cfg.gt_timeout;
        while gt_marker_count(&gt_file) <= warm_count && Instant::now() < m_deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let consumed = gt_marker_count(&gt_file) > warm_count;
        measured += 1;
        if !consumed {
            dropped += 1;
        }
        println!(
            "trial {trial}: live-reuse wake#2 (settle={}) consumed={consumed}",
            cfg.live_settle
        );
        let _ = destroy_session_inner(ctx.app.handle(), id).await;
    }

    let pct = if measured == 0 {
        0.0
    } else {
        dropped as f64 / measured as f64 * 100.0
    };
    println!(
        "\n--- LIVE-REUSE RESULT (agent='{}', live_settle='{}') ---",
        cfg.agent_label, cfg.live_settle
    );
    println!(
        "fresh-idle live-path wake#2 drop rate: {}/{} ({:.0}%); warm-up-failed skipped: {}",
        dropped, measured, pct, warmup_failed
    );
    println!("=== end ===\n");
}

const PI_CAPTURE_MAX_BYTES: usize = 4 * 1024 * 1024;

struct PiConfigDirGuard {
    path: PathBuf,
    owned: bool,
}

impl PiConfigDirGuard {
    fn cleanup(&mut self) -> Result<(), String> {
        if !self.owned {
            return Ok(());
        }
        std::fs::remove_dir_all(&self.path).map_err(|error| {
            format!(
                "failed to remove harness-owned PI_CODING_AGENT_DIR '{}': {error}",
                self.path.display()
            )
        })?;
        self.owned = false;
        Ok(())
    }
}

impl Drop for PiConfigDirGuard {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn prepare_pi_logical_clear_mode(
    cfg: &HarnessConfig,
) -> Result<(PiConfigDirGuard, String), String> {
    if !cfg.shell.eq_ignore_ascii_case("pi") {
        return Err(
            "pi_logical_clear requires AC_WAKE_HARNESS_SHELL to be the bare PATH launcher 'pi'"
                .to_string(),
        );
    }
    if cfg.trials == 0 {
        return Err("pi_logical_clear requires AC_WAKE_HARNESS_TRIALS > 0".to_string());
    }
    if cfg.settle_hold.is_zero() {
        return Err("pi_logical_clear requires AC_WAKE_HARNESS_SETTLE_HOLD_MS > 0".to_string());
    }
    if cfg.gt_timeout < Duration::from_secs(5) {
        return Err("pi_logical_clear requires AC_WAKE_HARNESS_GT_TIMEOUT_MS >= 5000".to_string());
    }

    const REQUIRED_ARGS: [&str; 7] = [
        "--no-session",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-context-files",
        "--no-approve",
        "--offline",
    ];
    if cfg.args.len() != REQUIRED_ARGS.len()
        || REQUIRED_ARGS.iter().any(|required| {
            cfg.args
                .iter()
                .filter(|arg| arg.as_str() == *required)
                .count()
                != 1
        })
    {
        return Err(format!(
            "pi_logical_clear requires exactly these isolation args once each and no others: {:?}; got {:?}",
            REQUIRED_ARGS, cfg.args
        ));
    }
    if std::env::var("PI_OFFLINE").as_deref() != Ok("1") {
        return Err("pi_logical_clear requires PI_OFFLINE=1".to_string());
    }
    let config_value = std::env::var("PI_CODING_AGENT_DIR")
        .map_err(|_| "pi_logical_clear requires PI_CODING_AGENT_DIR".to_string())?;
    if config_value.is_empty() {
        return Err("PI_CODING_AGENT_DIR must be nonempty".to_string());
    }
    let config_dir = PathBuf::from(&config_value);
    if !config_dir.is_absolute() {
        return Err(format!(
            "PI_CODING_AGENT_DIR must be absolute: '{}'",
            config_dir.display()
        ));
    }
    match std::fs::symlink_metadata(&config_dir) {
        Ok(_) => {
            return Err(format!(
                "PI_CODING_AGENT_DIR must not exist at harness entry: '{}'",
                config_dir.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "could not verify PI_CODING_AGENT_DIR '{}': {error}",
                config_dir.display()
            ));
        }
    }

    let version_output = std::process::Command::new("cmd.exe")
        .args(["/D", "/S", "/C", cfg.shell.as_str(), "--version"])
        .output()
        .map_err(|error| format!("Pi version probe failed to start: {error}"))?;
    let stdout = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();
    let stderr = String::from_utf8_lossy(&version_output.stderr)
        .trim()
        .to_string();
    let version_text = [stdout, stderr]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let version_pattern = regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
        .map_err(|error| format!("internal version regex failed: {error}"))?;
    if !version_output.status.success()
        || version_text.lines().count() != 1
        || !version_pattern.is_match(&version_text)
    {
        return Err(format!(
            "Pi version probe must succeed with one semantic-version line; status={} output={version_text:?}",
            version_output.status
        ));
    }
    println!("Pi version: {version_text}");

    std::fs::create_dir(&config_dir).map_err(|error| {
        format!(
            "failed to create harness-owned PI_CODING_AGENT_DIR '{}': {error}",
            config_dir.display()
        )
    })?;
    Ok((
        PiConfigDirGuard {
            path: config_dir,
            owned: true,
        },
        version_text,
    ))
}

struct CaptureEpoch {
    session_id: Uuid,
    senders: OutputSenderMap,
    bytes: Arc<Mutex<Vec<u8>>>,
    overflowed: Arc<AtomicBool>,
    collector: tokio::task::JoinHandle<()>,
}

struct CaptureSnapshot {
    bytes: Vec<u8>,
    overflowed: bool,
}

fn start_capture_epoch(ctx: &HarnessCtx, session_id: Uuid) -> Result<CaptureEpoch, String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    {
        let mut senders = ctx
            .output_senders
            .lock()
            .map_err(|_| "output sender map lock poisoned".to_string())?;
        if senders.contains_key(&session_id) {
            return Err(format!(
                "capture epoch already registered for session {session_id}"
            ));
        }
        senders.insert(session_id, sender);
    }
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let overflowed = Arc::new(AtomicBool::new(false));
    let bytes_for_collector = Arc::clone(&bytes);
    let overflow_for_collector = Arc::clone(&overflowed);
    let collector = tokio::spawn(async move {
        while let Some(chunk) = receiver.recv().await {
            if overflow_for_collector.load(Ordering::SeqCst) {
                continue;
            }
            let mut retained = bytes_for_collector
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if retained.len().saturating_add(chunk.len()) > PI_CAPTURE_MAX_BYTES {
                overflow_for_collector.store(true, Ordering::SeqCst);
                continue;
            }
            retained.extend_from_slice(&chunk);
        }
    });
    Ok(CaptureEpoch {
        session_id,
        senders: Arc::clone(&ctx.output_senders),
        bytes,
        overflowed,
        collector,
    })
}

impl CaptureEpoch {
    fn contains(&self, marker: &str) -> bool {
        let bytes = self.bytes.lock().unwrap_or_else(|error| error.into_inner());
        String::from_utf8_lossy(&bytes).contains(marker)
    }

    async fn stop(mut self) -> Result<CaptureSnapshot, String> {
        self.senders
            .lock()
            .map_err(|_| "output sender map lock poisoned".to_string())?
            .remove(&self.session_id);
        match tokio::time::timeout(Duration::from_secs(5), &mut self.collector).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(format!("capture collector failed: {error}")),
            Err(_) => {
                self.collector.abort();
                let _ = self.collector.await;
                return Err("capture collector cleanup timed out and was aborted".to_string());
            }
        }
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        Ok(CaptureSnapshot {
            bytes,
            overflowed: self.overflowed.load(Ordering::SeqCst),
        })
    }
}

async fn stop_capture_slot(slot: &mut Option<CaptureEpoch>) -> Result<CaptureSnapshot, String> {
    let capture = slot
        .take()
        .ok_or_else(|| "capture epoch was not active".to_string())?;
    capture.stop().await
}

async fn wait_for_capture_marker(
    capture: &CaptureEpoch,
    marker: &str,
    forbidden: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if capture.overflowed.load(Ordering::SeqCst) {
            return Err(format!(
                "capture exceeded the {} byte cap",
                PI_CAPTURE_MAX_BYTES
            ));
        }
        if capture.contains(forbidden) {
            return Err(format!("forbidden output marker observed: {forbidden}"));
        }
        if capture.contains(marker) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("timed out waiting for output marker {marker:?}"))
}

async fn wait_for_newer_screen_marker(
    ctx: &HarnessCtx,
    session_id: Uuid,
    baseline_sequence: u64,
    marker: &str,
    timeout: Duration,
) -> Result<(u64, String), String> {
    let deadline = Instant::now() + timeout;
    let mut last_snapshot: Option<(u64, String)> = None;
    while Instant::now() < deadline {
        if let Ok((sequence, text)) = stable_screen_snapshot(&ctx.app, session_id) {
            if sequence > baseline_sequence && text.contains(marker) {
                return Ok((sequence, text));
            }
            last_snapshot = Some((sequence, text));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "stable screen did not advance past sequence {baseline_sequence} with marker {marker:?}; last_snapshot={last_snapshot:?}"
    ))
}

async fn run_pi_trial_body(
    cfg: &HarnessConfig,
    ctx: &HarnessCtx,
    session_id: Uuid,
    capture: &mut Option<CaptureEpoch>,
) -> Result<(), String> {
    let settle_deadline = Instant::now() + cfg.gt_timeout;
    if !wait_for_settle(ctx, session_id, cfg.settle_hold, settle_deadline).await {
        return Err("initial Pi session did not reach sustained paste-ready state".to_string());
    }
    let (new_baseline_sequence, new_baseline) = stable_screen_snapshot(&ctx.app, session_id)?;
    if new_baseline.matches("New session started").count() != 0
        || new_baseline.matches("Keyboard Shortcuts").count() != 0
        || new_baseline.contains("No API key found")
    {
        return Err(format!(
            "Pi baseline contains a control marker or model-call marker: {new_baseline:?}"
        ));
    }

    *capture = Some(start_capture_epoch(ctx, session_id)?);
    inject_text_into_session(ctx.app.handle(), session_id, "/new")
        .await
        .map_err(|error| format!("/new production injection failed: {error}"))?;
    let active = capture
        .as_ref()
        .ok_or_else(|| "capture epoch disappeared before /new observation".to_string())?;
    wait_for_capture_marker(
        active,
        "New session started",
        "No API key found",
        cfg.gt_timeout,
    )
    .await?;

    let post_new_settle_deadline = Instant::now() + cfg.gt_timeout;
    if !wait_for_settle(ctx, session_id, cfg.settle_hold, post_new_settle_deadline).await {
        return Err("Pi did not reach a fresh sustained-idle hold after /new".to_string());
    }
    let (_, post_new_screen) = wait_for_newer_screen_marker(
        ctx,
        session_id,
        new_baseline_sequence,
        "New session started",
        cfg.gt_timeout,
    )
    .await?;
    if post_new_screen.matches("New session started").count() != 1 {
        return Err(format!(
            "expected exactly one stable New session started marker: {post_new_screen:?}"
        ));
    }
    if post_new_screen.contains("No API key found") {
        return Err("model-call marker appeared after /new".to_string());
    }
    let new_capture = stop_capture_slot(capture).await?;
    let new_raw = String::from_utf8_lossy(&new_capture.bytes);
    if new_capture.overflowed
        || !new_raw.contains("New session started")
        || new_raw.contains("No API key found")
    {
        return Err(format!(
            "invalid clean /new capture: overflow={} output={new_raw:?}",
            new_capture.overflowed
        ));
    }

    *capture = Some(start_capture_epoch(ctx, session_id)?);
    let (hotkeys_baseline_sequence, hotkeys_baseline) =
        stable_screen_snapshot(&ctx.app, session_id)?;
    if hotkeys_baseline.contains("Keyboard Shortcuts") {
        return Err("/hotkeys baseline already contained Keyboard Shortcuts".to_string());
    }
    inject_text_into_session(ctx.app.handle(), session_id, "/hotkeys")
        .await
        .map_err(|error| format!("/hotkeys production injection failed: {error}"))?;
    let active = capture
        .as_ref()
        .ok_or_else(|| "capture epoch disappeared before /hotkeys observation".to_string())?;
    wait_for_capture_marker(
        active,
        "Keyboard Shortcuts",
        "No API key found",
        cfg.gt_timeout,
    )
    .await?;
    let (_, hotkeys_screen) = wait_for_newer_screen_marker(
        ctx,
        session_id,
        hotkeys_baseline_sequence,
        "Keyboard Shortcuts",
        cfg.gt_timeout,
    )
    .await?;
    if hotkeys_screen.contains("No API key found") {
        return Err("model-call marker appeared after /hotkeys".to_string());
    }
    let hotkeys_capture = stop_capture_slot(capture).await?;
    let hotkeys_raw = String::from_utf8_lossy(&hotkeys_capture.bytes);
    if hotkeys_capture.overflowed
        || !hotkeys_raw.contains("Keyboard Shortcuts")
        || hotkeys_raw.contains("No API key found")
    {
        return Err(format!(
            "invalid clean /hotkeys capture: overflow={} output={hotkeys_raw:?}",
            hotkeys_capture.overflowed
        ));
    }
    Ok(())
}

async fn cleanup_pi_trial(
    ctx: &HarnessCtx,
    session_id: Uuid,
    capture: &mut Option<CaptureEpoch>,
) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(active) = capture.take() {
        if let Err(error) = active.stop().await {
            errors.push(error);
        }
    }

    let destroy = tokio::time::timeout(
        Duration::from_secs(15),
        destroy_session_inner(ctx.app.handle(), session_id),
    )
    .await;
    let needs_fallback = match destroy {
        Ok(Ok(())) => false,
        Ok(Err(error)) => {
            errors.push(format!("destroy_session_inner failed: {error}"));
            true
        }
        Err(_) => {
            errors.push("destroy_session_inner timed out after 15s".to_string());
            true
        }
    };
    if needs_fallback {
        let kill_result = ctx
            .pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())
            .and_then(|manager| manager.kill(session_id).map_err(|error| error.to_string()));
        if let Err(error) = kill_result {
            errors.push(format!("PtyManager kill fallback failed: {error}"));
        }
        match tokio::time::timeout(
            Duration::from_secs(5),
            destroy_session_inner(ctx.app.handle(), session_id),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(format!(
                "SessionManager removal fallback through destroy failed: {error}"
            )),
            Err(_) => errors.push("SessionManager removal fallback timed out".to_string()),
        }
    }

    let pty_live = ctx
        .pty_mgr
        .lock()
        .map(|manager| manager.has_session(session_id))
        .unwrap_or(true);
    if pty_live {
        errors.push("PTY route remained live after cleanup".to_string());
    }
    let manager = {
        let guard = ctx.session_mgr.read().await;
        guard.clone()
    };
    if manager.get_session(session_id).await.is_some() {
        errors.push("SessionManager record remained after cleanup".to_string());
    }
    errors
}

async fn run_pi_logical_clear(cfg: &HarnessConfig, ctx: &HarnessCtx) -> Result<(), String> {
    let mut completed = 0usize;
    let mut passed = 0usize;
    let mut failures = Vec::new();

    for trial in 0..cfg.trials {
        completed += 1;
        let trial_dir = ctx._temp.path().join(format!("pi-clear-{trial}"));
        if let Err(error) = std::fs::create_dir(&trial_dir) {
            failures.push(format!(
                "trial {trial}: working-directory creation failed: {error}"
            ));
            continue;
        }
        let spawned = create_session_inner(
            ctx.app.handle(),
            &ctx.session_mgr,
            &ctx.pty_mgr,
            cfg.shell.clone(),
            cfg.args.clone(),
            trial_dir.to_string_lossy().to_string(),
            Some(format!("pi-clear-{trial}")),
            None,
            Some(cfg.agent_label.clone()),
            true,
            Vec::new(),
            true,
            None,
            None,
            Some(PtyViewport::from_fit(120, 120)),
            CreateSelectionIntent::User,
        )
        .await;
        let info = match spawned {
            Ok(info) => info,
            Err(error) => {
                failures.push(format!("trial {trial}: spawn failed: {error}"));
                continue;
            }
        };
        let session_id = match Uuid::parse_str(&info.id) {
            Ok(id) => id,
            Err(error) => {
                failures.push(format!(
                    "trial {trial}: invalid spawned session id: {error}"
                ));
                continue;
            }
        };
        let mut capture = None;
        let body_result = run_pi_trial_body(cfg, ctx, session_id, &mut capture).await;
        let cleanup_errors = cleanup_pi_trial(ctx, session_id, &mut capture).await;
        if let Err(error) = &body_result {
            failures.push(format!("trial {trial}: {error}"));
        }
        for error in cleanup_errors {
            failures.push(format!("trial {trial} cleanup: {error}"));
        }
        if body_result.is_ok()
            && failures
                .iter()
                .all(|failure| !failure.starts_with(&format!("trial {trial}")))
        {
            passed += 1;
            println!("trial {trial}: PASS");
        } else {
            println!("trial {trial}: FAIL");
        }
    }

    let failed = completed.saturating_sub(passed);
    println!(
        "pi_logical_clear summary: configured={} completed={} passed={} failed={}",
        cfg.trials, completed, passed, failed
    );
    for failure in &failures {
        println!("FAIL: {failure}");
    }
    if completed != cfg.trials || completed == 0 || !failures.is_empty() || passed != completed {
        return Err(format!(
            "pi_logical_clear failed: configured={} completed={} passed={} failed={} detail_count={}",
            cfg.trials,
            completed,
            passed,
            failed,
            failures.len()
        ));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "on-demand real-agent measurement; needs an installed+authed coding agent on Windows/ConPTY"]
async fn measure_wake_consumption_signals() {
    let cfg = HarnessConfig::from_env();
    println!("\n=== #1001 wake-consumption measurement harness ===");
    println!(
        "agent='{}' shell='{}' args={:?} trials={} inject_mode='{}' redeliver_mode='{}' signal_window={:?} gt_timeout={:?}",
        cfg.agent_label, cfg.shell, cfg.args, cfg.trials, cfg.inject_mode, cfg.redeliver_mode, cfg.signal_window, cfg.gt_timeout
    );

    if !agent_available(&cfg.shell) {
        println!(
            "SKIP: agent shell '{}' not found on PATH. Set AC_WAKE_HARNESS_SHELL/ARGS to a real \
             coding agent and re-run. No numbers produced (not fabricated).",
            cfg.shell
        );
        return;
    }

    if cfg.inject_mode == "pi_logical_clear" {
        let (mut config_guard, _version) = prepare_pi_logical_clear_mode(&cfg)
            .unwrap_or_else(|error| panic!("pi_logical_clear precondition failed: {error}"));
        let repo_root = std::env::current_dir().expect("cwd");
        let ctx = make_ctx(&repo_root);
        let run_result = run_pi_logical_clear(&cfg, &ctx).await;
        let config_cleanup = config_guard.cleanup();
        match (run_result, config_cleanup) {
            (Ok(()), Ok(())) => return,
            (Err(run_error), Ok(())) => panic!("{run_error}"),
            (Ok(()), Err(cleanup_error)) => panic!("{cleanup_error}"),
            (Err(run_error), Err(cleanup_error)) => {
                panic!("{run_error}; global cleanup: {cleanup_error}")
            }
        }
    }

    let repo_root = std::env::current_dir().expect("cwd");
    let ctx = make_ctx(&repo_root);

    if cfg.inject_mode == "startup_probe" {
        run_startup_probe(&cfg, &ctx).await;
        return;
    }
    if cfg.inject_mode == "live_reuse" {
        run_live_reuse(&cfg, &ctx).await;
        return;
    }

    let mut bare = SignalTally::default();
    let mut ts_gate = SignalTally::default();
    let mut screen = SignalTally::default();
    let mut cold_drops = 0usize; // GT-not-consumed on first attempt (the raw bug)
    let mut redeliver_measured = 0usize; // drops where we redelivered
    let mut redeliver_recovered = 0usize; // GT marker appeared after redeliver
    let mut redeliver_duplicated = 0usize; // >1 markers: body double-submitted (F7/G4)

    for trial in 0..cfg.trials {
        let trial_dir = ctx._temp.path().join(format!("trial{trial}"));
        std::fs::create_dir_all(&trial_dir).expect("trial dir");
        let gt_file = trial_dir.join("marks.txt");

        // Cold-spawn a fresh agent session at the trial dir.
        let info = match create_session_inner(
            ctx.app.handle(),
            &ctx.session_mgr,
            &ctx.pty_mgr,
            cfg.shell.clone(),
            cfg.args.clone(),
            trial_dir.to_string_lossy().to_string(),
            Some(format!("wake-harness-{trial}")),
            None,
            Some(cfg.agent_label.clone()),
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        {
            Ok(info) => info,
            Err(e) => {
                println!("trial {trial}: spawn failed: {e}");
                continue;
            }
        };
        let id = Uuid::parse_str(&info.id).expect("uuid");

        // Boot/inject timing per mode. The #1001 drop lives in the fresh-idle
        // window BEFORE the TUI is paste-ready, so "ready" (settle first) tends
        // to consume cleanly while "first_idle"/"immediate" reproduce the race.
        let boot_deadline = Instant::now() + Duration::from_secs(45);
        match cfg.inject_mode.as_str() {
            "immediate" => {
                // Fixed short delay, then inject regardless of readiness: the
                // most aggressive reproduction of the not-paste-ready race.
                tokio::time::sleep(cfg.immediate_delay).await;
            }
            "first_idle" => {
                // Inject the instant the watcher first reports idle, with no
                // paste-ready settle (the fresh-idle danger window B targets).
                while !watcher_idle(&ctx.idle, id) && Instant::now() < boot_deadline {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            _ => {
                // "ready" (default): sustained idle AND rendered content.
                while !(watcher_idle(&ctx.idle, id)
                    && nonblank_lines(&screen_text(&ctx.app, id)) > 0)
                    && Instant::now() < boot_deadline
                {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
        let idle_at_inject = watcher_idle(&ctx.idle, id);
        let pre_lines = nonblank_lines(&screen_text(&ctx.app, id));

        // ── inject attempt 1 (production inject: body + double-Enter) ──
        let body1 = wake_body(&gt_file, trial, 1);
        let body_token = format!("AC_WAKE_MARK {trial}-1"); // the box would echo the instruction, incl. this
        let _ = inject_text_into_session(ctx.app.handle(), id, &body1).await;
        let submit_completed_at = Instant::now();

        // ── poll the three signals over the signal window ──
        let mut s1 = false;
        let mut s2 = false;
        let mut s3 = false;
        let win_deadline = submit_completed_at + cfg.signal_window;
        while Instant::now() < win_deadline {
            if idle_at_inject && !watcher_idle(&ctx.idle, id) {
                s1 = true; // bare flip: idle -> busy after inject
            }
            if idle_at_inject
                && ctx
                    .idle
                    .has_printable_activity_since(id, submit_completed_at)
            {
                s2 = true;
            }
            if idle_at_inject {
                let post = screen_text(&ctx.app, id);
                if screen_consumed(pre_lines, &post, &body_token) {
                    s3 = true;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        // ── ground truth: did the agent actually run the tool? ──
        let gt_deadline = Instant::now() + cfg.gt_timeout;
        while gt_marker_count(&gt_file) < 1 && Instant::now() < gt_deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let gt1 = gt_marker_count(&gt_file) >= 1;
        if !gt1 {
            cold_drops += 1;
        }

        bare.observe(s1, gt1);
        ts_gate.observe(s2, gt1);
        screen.observe(s3, gt1);
        println!(
            "trial {trial}: idle_at_inject={idle_at_inject} GT_consumed={gt1} | \
             bare_flip={s1} ts_gate={s2} screen={s3}"
        );

        // ── F7/G4 + settled-redeliver: on a drop, redeliver and measure whether
        // the turn RECOVERS (a GT marker appears) and whether it DUPLICATES (>1
        // markers = a lingering body double-submitted). `immediate` (default)
        // re-injects at once (the baseline dev-rust already measured); `settled`
        // first waits for SUSTAINED paste-ready (mirroring B's settle), to learn
        // whether a settled redeliver is inherently safe or still needs the
        // Ctrl-U clear. Only meaningful when attempt 1 dropped. ──
        if !gt1 {
            redeliver_measured += 1;
            if cfg.redeliver_mode == "settled" {
                let settle_deadline = Instant::now() + Duration::from_secs(30);
                if !wait_for_settle(&ctx, id, cfg.settle_hold, settle_deadline).await {
                    println!("trial {trial}: redeliver(settled) gave up waiting for paste-ready");
                }
            }
            let body2 = wake_body(&gt_file, trial, 2);
            let _ = inject_text_into_session(ctx.app.handle(), id, &body2).await;
            let deadline = Instant::now() + cfg.gt_timeout;
            while gt_marker_count(&gt_file) < 1 && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let count = gt_marker_count(&gt_file);
            if count >= 1 {
                redeliver_recovered += 1;
            }
            if count > 1 {
                redeliver_duplicated += 1;
            }
            println!(
                "trial {trial}: redeliver({}) -> markers={count} recovered={} duplicated={}",
                cfg.redeliver_mode,
                count >= 1,
                count > 1
            );
        }

        let _ = destroy_session_inner(ctx.app.handle(), id).await;
    }

    // ───────────────────────────── report ────────────────────────────────
    println!("\n--- RESULTS (agent='{}') ---", cfg.agent_label);
    let rate = |n: usize, d: usize| if d == 0 { 0.0 } else { n as f64 / d as f64 };
    for (name, t) in [
        ("bare waiting_for_input flip", bare),
        ("post-submit activity gate (14.1)", ts_gate),
        ("screen-state (16.1 #3)", screen),
    ] {
        println!(
            "signal '{name}': trials={} FP={} ({:.0}%) FN={} ({:.0}%)",
            t.total,
            t.fp,
            rate(t.fp, t.total) * 100.0,
            t.fn_,
            rate(t.fn_, t.total) * 100.0,
        );
    }
    println!(
        "cold-spawn first-attempt drop rate (raw #1001 bug): {}/{}",
        cold_drops, cfg.trials
    );
    println!(
        "redeliver mode '{}': recovered {}/{} drops, duplicated {}/{} drops",
        cfg.redeliver_mode,
        redeliver_recovered,
        redeliver_measured,
        redeliver_duplicated,
        redeliver_measured
    );
    println!("=== end ===\n");
}
