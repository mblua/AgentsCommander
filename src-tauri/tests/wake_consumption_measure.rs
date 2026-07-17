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
//!   AC_WAKE_HARNESS_INJECT_MODE      (ready | first_idle | immediate; default ready)
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
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentscommander_lib::commands::pty::get_screen_snapshot;
use agentscommander_lib::commands::session::{
    create_session_inner, destroy_session_inner, CreateSelectionIntent,
};
use agentscommander_lib::config::settings::{AppSettings, SettingsState};
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
    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(AppSettings {
        default_shell: "powershell.exe".to_string(),
        default_shell_args: vec!["-NoLogo".to_string()],
        project_paths: vec![repo_root.to_string_lossy().to_string()],
        ..AppSettings::default()
    }));
    let git_app = Box::leak(Box::new(
        tauri::Builder::default()
            .any_thread()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("git handle app"),
    ));
    let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), git_app.handle().clone());
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
        output_senders,
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
