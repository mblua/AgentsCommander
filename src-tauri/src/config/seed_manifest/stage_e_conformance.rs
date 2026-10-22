use super::*;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const HARD_WORKING_SET_LIMIT: u64 = 512 * 1024 * 1024;

// ---- small local fixtures (kept independent of `mod tests`) ----

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid test timestamp")
        .with_timezone(&Utc)
}

fn setup_project() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).expect("create project .ac");
    (temp, project)
}

fn canonical_path(project: &Path) -> PathBuf {
    project.join(".ac").join(SEED_MANIFEST_FILENAME)
}

fn config_scope(agent: &str, dest: &str) -> String {
    format!("config:.ac/wg-1-dev-team/__agent_{agent}/{dest}")
}

fn temp_leftovers(project: &Path) -> usize {
    let dir = project.join(".ac");
    std::fs::read_dir(&dir)
        .expect("read .ac")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .map(is_exact_temp_name)
                .unwrap_or(false)
        })
        .count()
}

// ---- working-set sampling (plan section 7.4 additional-working-set) ----

#[cfg(windows)]
fn working_set_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS>() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    if ok == 0 {
        0
    } else {
        counters.WorkingSetSize as u64
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn working_set_bytes() -> u64 {
    // Resident pages from /proc/self/statm (field 1) times the page size.
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).map(str::to_string))
        .and_then(|pages| pages.parse::<u64>().ok())
        .map(|pages| pages.saturating_mul(4096))
        .unwrap_or(0)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn working_set_bytes() -> u64 {
    0
}

/// Background sampler that tracks the peak working set during an operation.
struct WorkingSetSampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl WorkingSetSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(working_set_bytes()));
        let (stop_t, peak_t) = (Arc::clone(&stop), Arc::clone(&peak));
        let handle = std::thread::spawn(move || {
            while !stop_t.load(Ordering::Relaxed) {
                peak_t.fetch_max(working_set_bytes(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(2));
            }
            peak_t.fetch_max(working_set_bytes(), Ordering::Relaxed);
        });
        Self {
            stop,
            peak,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.peak.load(Ordering::Relaxed)
    }
}

// -- plan section 10.1 item 4: adversarial parser-memory bound --
#[test]
#[ignore = "release-mode adversarial parser-memory measurement (plan 7.4/10.1 item 4)"]
fn bench_adversarial_parser_memory_stays_bounded() {
    // Near the 128 MiB cap in release; a smaller shape in debug so a manual
    // debug run is tolerable. Each shape is generated, parsed, and dropped in
    // isolation with its own working-set sample.
    let target: usize = if cfg!(debug_assertions) {
        16 * 1024 * 1024
    } else {
        120 * 1024 * 1024
    };

    // (label, bytes, expect_ok)
    let mut cases: Vec<(&str, String, bool)> = Vec::new();

    // Valid near-cap manifest at the row cap with padded paths.
    cases.push(("valid-near-cap", valid_near_cap_manifest(target), true));
    // Adversarial invalid shapes: all must return a typed error, never abort.
    cases.push(("many-tiny-tables", many_tiny_tables(target), false));
    cases.push(("huge-escaped-string", huge_escaped_string(target), false));
    cases.push(("deep-array", deep_array(target), false));
    cases.push(("duplicate-headers", duplicate_headers(target), false));
    cases.push(("early-syntax-error", early_syntax_error(target), false));
    cases.push(("late-syntax-error", late_syntax_error(target), false));

    for (label, bytes, expect_ok) in cases {
        let input_len = bytes.len();
        let baseline = working_set_bytes();
        let sampler = WorkingSetSampler::start();
        let result = parse_manifest_bytes(bytes.as_bytes());
        drop(bytes);
        let peak = sampler.finish();
        let delta = peak.saturating_sub(baseline);
        if expect_ok {
            assert!(result.is_ok(), "{label}: valid near-cap input must parse");
        } else {
            assert!(
                result.is_err(),
                "{label}: adversarial input must return a typed error, not parse"
            );
        }
        eprintln!(
            "[stage-e adversarial] shape={label} input_bytes={input_len} \
             ok={expect_ok} working_set_delta_bytes={delta}"
        );
        if !cfg!(debug_assertions) {
            assert!(
                delta <= HARD_WORKING_SET_LIMIT,
                "{label}: parser used {delta} bytes additional working set, over 512 MiB"
            );
        }
    }
}

fn manifest_prefix() -> String {
    let mut s = String::new();
    s.push_str(MANAGED_HEADER);
    s.push('\n');
    s.push_str("schema_version = 1\n");
    s.push_str("coverage_version = 2\n");
    s.push_str(
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \
            \"coding_agent_catalog\"]\n",
    );
    s
}

fn valid_near_cap_manifest(target: usize) -> String {
    // Rows in one config scope; pad the path to grow bytes without exceeding
    // the 250k row cap. Stay comfortably under both caps.
    let mut out = manifest_prefix();
    let rows = 200_000usize;
    let pad = (target / rows).clamp(8, 4096);
    let filler = "d".repeat(pad);
    for i in 0..rows {
        out.push_str("\n[[files]]\n");
        out.push_str(&format!(
            "path = \".ac/wg-1-dev-team/__agent_alpha/.claude/{filler}-{i:07}\"\n"
        ));
        out.push_str("path_encoding = \"utf8\"\n");
        out.push_str("kind = \"replica_config_file\"\n");
        out.push_str("scope = \"config:.ac/wg-1-dev-team/__agent_alpha/.claude\"\n");
        out.push_str("source = \"workspace_base\"\n");
        out.push_str("last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n");
        if out.len() >= target {
            break;
        }
    }
    out
}

fn many_tiny_tables(target: usize) -> String {
    // Valid-shaped tables but with a duplicate identity that is rejected.
    let mut out = manifest_prefix();
    while out.len() < target {
        out.push_str("\n[[files]]\n");
        out.push_str("path = \".ac/wg-1-dev-team/__agent_alpha/.claude/same\"\n");
        out.push_str("path_encoding = \"utf8\"\n");
        out.push_str("kind = \"replica_config_file\"\n");
        out.push_str("scope = \"config:.ac/wg-1-dev-team/__agent_alpha/.claude\"\n");
        out.push_str("source = \"workspace_base\"\n");
        out.push_str("last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n");
    }
    out
}

fn huge_escaped_string(target: usize) -> String {
    let mut out = manifest_prefix();
    out.push_str("\n[[files]]\npath = \"");
    // A giant escaped-control-character string; the path decoder rejects it.
    while out.len() < target {
        out.push_str("\\u0000\\t\\\\");
    }
    out.push_str("\"\n");
    out
}

fn deep_array(target: usize) -> String {
    let mut out = manifest_prefix();
    out.push_str("\ncoverage = [");
    let depth = target / 2;
    for _ in 0..depth {
        out.push('[');
    }
    out
}

fn duplicate_headers(target: usize) -> String {
    let mut out = manifest_prefix();
    // Duplicate scalar top-level keys are a hard parse error.
    while out.len() < target {
        out.push_str("schema_version = 1\n");
    }
    out
}

fn early_syntax_error(target: usize) -> String {
    let mut out = String::from("!!! not toml at all\n");
    out.push_str(&manifest_prefix());
    while out.len() < target {
        out.push_str("padding = padding = padding\n");
    }
    out
}

fn late_syntax_error(target: usize) -> String {
    let mut out = valid_near_cap_manifest(target);
    out.push_str("\n[[files]\nbroken = \n");
    out
}
