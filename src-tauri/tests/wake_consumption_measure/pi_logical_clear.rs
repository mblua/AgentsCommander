//! Isolated Pi logical-clear trials, output capture, and cleanup.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentscommander_lib::commands::pty::get_screen_snapshot;
use agentscommander_lib::commands::session::{
    create_session_inner, destroy_session_inner, CreateSelectionIntent,
};
use agentscommander_lib::pty::backend::PtyViewport;
use agentscommander_lib::pty::inject::inject_text_into_session;
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::telegram::manager::OutputSenderMap;
use tauri::Manager;
use uuid::Uuid;

use super::{strip_ansi, wait_for_settle, HarnessConfig, HarnessCtx};

fn stable_screen_snapshot(app: &tauri::App, id: Uuid) -> Result<(u64, String), String> {
    let state = app.state::<Arc<Mutex<PtyManager>>>();
    get_screen_snapshot(state, id.to_string())
        .map_err(|error| format!("screen snapshot failed: {error}"))?
        .map(|snapshot| (snapshot.sequence, strip_ansi(&snapshot.data)))
        .ok_or_else(|| format!("screen snapshot unavailable for session {id}"))
}

const PI_CAPTURE_MAX_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct PiConfigDirGuard {
    path: PathBuf,
    owned: bool,
}

impl PiConfigDirGuard {
    pub(crate) fn cleanup(&mut self) -> Result<(), String> {
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

pub(crate) fn prepare_pi_logical_clear_mode(
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
            "pi_logical_clear requires exactly these isolation args once each and no others: \
                {:?}; got {:?}",
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
            "Pi version probe must succeed with one semantic-version line; status={} \
                output={version_text:?}",
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
        "stable screen did not advance past sequence {baseline_sequence} with marker \
            {marker:?}; last_snapshot={last_snapshot:?}"
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

pub(crate) async fn run_pi_logical_clear(
    cfg: &HarnessConfig,
    ctx: &HarnessCtx,
) -> Result<(), String> {
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
            "pi_logical_clear failed: configured={} completed={} passed={} failed={} \
                detail_count={}",
            cfg.trials,
            completed,
            passed,
            failed,
            failures.len()
        ));
    }
    Ok(())
}
