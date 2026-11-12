//! Per-trial signal-measurement steps extracted from `measure_wake_consumption_signals`.

use std::path::Path;
use std::time::{Duration, Instant};

use agentscommander_lib::pty::inject::inject_text_into_session;
use uuid::Uuid;

use super::{
    gt_marker_count, nonblank_lines, screen_consumed, screen_text, wait_for_settle, wake_body,
    watcher_idle, HarnessConfig, HarnessCtx, SignalTally,
};

pub(super) async fn wait_for_inject_point(cfg: &HarnessConfig, ctx: &HarnessCtx, id: Uuid) {
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
            while !(watcher_idle(&ctx.idle, id) && nonblank_lines(&screen_text(&ctx.app, id)) > 0)
                && Instant::now() < boot_deadline
            {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

pub(super) async fn poll_signals(
    cfg: &HarnessConfig,
    ctx: &HarnessCtx,
    id: Uuid,
    idle_at_inject: bool,
    pre_lines: usize,
    body_token: &str,
    submit_completed_at: Instant,
) -> (bool, bool, bool) {
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
            if screen_consumed(pre_lines, &post, body_token) {
                s3 = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    (s1, s2, s3)
}

pub(super) async fn redeliver_after_drop(
    cfg: &HarnessConfig,
    ctx: &HarnessCtx,
    id: Uuid,
    gt_file: &Path,
    trial: usize,
) -> usize {
    if cfg.redeliver_mode == "settled" {
        let settle_deadline = Instant::now() + Duration::from_secs(30);
        if !wait_for_settle(ctx, id, cfg.settle_hold, settle_deadline).await {
            println!("trial {trial}: redeliver(settled) gave up waiting for paste-ready");
        }
    }
    let body2 = wake_body(gt_file, trial, 2);
    let _ = inject_text_into_session(ctx.app.handle(), id, &body2).await;
    let deadline = Instant::now() + cfg.gt_timeout;
    while gt_marker_count(gt_file) < 1 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let count = gt_marker_count(gt_file);
    println!(
        "trial {trial}: redeliver({}) -> markers={count} recovered={} duplicated={}",
        cfg.redeliver_mode,
        count >= 1,
        count > 1
    );
    count
}

pub(super) fn print_signal_results(
    cfg: &HarnessConfig,
    tallies: [(&str, SignalTally); 3],
    cold_drops: usize,
    redeliver_recovered: usize,
    redeliver_measured: usize,
    redeliver_duplicated: usize,
) {
    println!("\n--- RESULTS (agent='{}') ---", cfg.agent_label);
    let rate = |n: usize, d: usize| if d == 0 { 0.0 } else { n as f64 / d as f64 };
    for (name, t) in tallies {
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
