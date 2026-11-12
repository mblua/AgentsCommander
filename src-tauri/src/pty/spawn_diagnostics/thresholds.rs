use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_STALL_TIMEOUT_MS: u64 = 5_000;
const MAX_STALL_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
/// Output below this many bytes is not a painted screen; it is the shell saying hello.
/// Measured on the real spawn shape (`cmd.exe /C codex ...`): `ConPTY`'s handshake and
/// screen setup plus the npm shim's `title %COMSPEC%` put **71 bytes** on the PTY within
/// ~45ms while the agent itself has written nothing. A floor of 64 was therefore crossed
/// by noise alone, which made every hung coding agent read as "painted" and silently
/// disabled the stall detector this module exists for. A healthy codex TUI crosses 256
/// bytes in ~350ms; a hanging one takes seconds.
pub(crate) const DEFAULT_PAINT_FLOOR_BYTES: u64 = 256;
const MAX_PAINT_FLOOR_BYTES: u64 = 64 * 1024;
const DEFAULT_HEAD_BYTES: u64 = 512;
const MAX_HEAD_BYTES: u64 = 64 * 1024;
const DEFAULT_CONCURRENCY_WINDOW_MS: u64 = 10_000;
const MAX_CONCURRENCY_WINDOW_MS: u64 = 60 * 60 * 1_000;
/// How long an AC stop request owns the exit that follows it. A resource-monitor kill
/// can be blocked (a Quarantined result leaves the child running); a child that dies
/// half an hour later died on its own, whatever we asked for back then.
const DEFAULT_STOP_ATTRIBUTION_MS: u64 = 60_000;
const MAX_STOP_ATTRIBUTION_MS: u64 = 60 * 60 * 1_000;

/// Everything the diagnostics need to be told, resolved once from the environment and
/// then carried by each record, so no code path (and no test) reads a process-global.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Deadline for the startup report. Zero disables it.
    pub stall_timeout: Duration,
    pub paint_floor: u64,
    pub head_bytes: usize,
    pub concurrency_window: Duration,
    pub stop_attribution: Duration,
    /// How long after a stop request `ConPTY`'s teardown repaint can still arrive. Only
    /// the startup TIMING stamps are suppressed during it; bytes keep counting, and the
    /// stall verdict is byte-based, so a stop that fails to kill the child cannot fake a
    /// stall on a session that is painting normally.
    pub teardown_suppression: Duration,
    pub startup_poll: Duration,
    pub steady_poll: Duration,
}

impl Thresholds {
    pub const DEFAULT: Self = Self {
        stall_timeout: Duration::from_millis(DEFAULT_STALL_TIMEOUT_MS),
        paint_floor: DEFAULT_PAINT_FLOOR_BYTES,
        head_bytes: DEFAULT_HEAD_BYTES as usize,
        concurrency_window: Duration::from_millis(DEFAULT_CONCURRENCY_WINDOW_MS),
        stop_attribution: Duration::from_millis(DEFAULT_STOP_ATTRIBUTION_MS),
        teardown_suppression: Duration::from_secs(2),
        startup_poll: Duration::from_millis(250),
        steady_poll: Duration::from_secs(2),
    };
}

impl Thresholds {
    /// Read once per process. Every value is clamped: a fat-fingered
    /// `AC_SPAWN_HEAD_BYTES=1000000000` must not buy a 1 GB buffer per session.
    pub fn from_env() -> Self {
        static VALUE: OnceLock<Thresholds> = OnceLock::new();
        *VALUE.get_or_init(|| Thresholds {
            stall_timeout: Duration::from_millis(env_u64(
                "AC_SPAWN_STALL_TIMEOUT_MS",
                DEFAULT_STALL_TIMEOUT_MS,
                MAX_STALL_TIMEOUT_MS,
            )),
            paint_floor: env_u64(
                "AC_SPAWN_PAINT_FLOOR_BYTES",
                DEFAULT_PAINT_FLOOR_BYTES,
                MAX_PAINT_FLOOR_BYTES,
            ),
            head_bytes: env_u64("AC_SPAWN_HEAD_BYTES", DEFAULT_HEAD_BYTES, MAX_HEAD_BYTES) as usize,
            concurrency_window: Duration::from_millis(env_u64(
                "AC_SPAWN_CONCURRENCY_WINDOW_MS",
                DEFAULT_CONCURRENCY_WINDOW_MS,
                MAX_CONCURRENCY_WINDOW_MS,
            )),
            stop_attribution: Duration::from_millis(env_u64(
                "AC_SPAWN_STOP_ATTRIBUTION_MS",
                DEFAULT_STOP_ATTRIBUTION_MS,
                MAX_STOP_ATTRIBUTION_MS,
            )),
            ..Thresholds::DEFAULT
        })
    }
}

pub(crate) fn env_u64(key: &str, default: u64, max: u64) -> u64 {
    let Ok(raw) = crate::config::environment::spawn_diagnostic_value(key) else {
        return default;
    };
    match raw.trim().parse::<u64>() {
        Ok(value) if value <= max => value,
        Ok(value) => {
            log::warn!(
                target: "agentscommander_lib::pty::spawn_diagnostics",
                "[pty] {key}={value} exceeds the {max} ceiling; clamping to {max}");
            max
        }
        Err(_) => {
            log::warn!(
                target: "agentscommander_lib::pty::spawn_diagnostics",
                "[pty] ignoring non-numeric {key}='{raw}'; using {default}");
            default
        }
    }
}
