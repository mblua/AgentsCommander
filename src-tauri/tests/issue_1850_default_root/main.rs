//! #1868 R15 acceptance: an unsuffixed product executable selects
//! `$HOME/.agentscommander` and never reads, copies, moves, imports, deletes
//! or adopts a legacy root.
//!
//! Black-box integration test. It copies the built product binary
//! (`CARGO_BIN_EXE_agentscommander`) into a temp directory, runs real CLI
//! verbs against it and observes the filesystem and stdout. It deliberately
//! imports nothing from `agentscommander_lib`, so the target also builds and
//! runs in the release profile (where the pre-existing
//! `cli_workgroup_team.rs` defect blocks whole-suite release integration
//! builds).
//!
//! Only `std`, `tempfile`, `dirs` and `serde_json` are named, and `sha256_hex`
//! below is a test-local implementation: the phase plan freezes
//! `src-tauri/Cargo.toml` and `Cargo.lock`, so no other crate may be named.
//!
//! Modes (see `admission`):
//! - non-Windows: the seven cases run against a fresh per-case `TempDir` HOME.
//! - Windows with the disposable-profile admission: the seven cases run
//!   against `dirs::home_dir()`, the known-folder profile of a disposable
//!   GitHub-hosted VM.
//! - Windows without admission: the copy-and-run mechanism control plus
//!   refusal safety only; no profile root is created, opened or adopted.

mod product;
mod profile;

use product::{
    copy_product_binary, inject_injected_home, print_retained, run_child,
    strip_inherited_config_env,
};
use profile::{
    census_unchanged, profile_census, profile_entry_absent, read_json, refusal_census_self_control,
    regular_file, regular_file_nonempty, scan_canonical_for_legacy, session_row, snapshot,
    write_json, write_legacy_root,
};

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// Excludes this crate's in-flight copy write descriptor on a freshly copied
/// product binary from overlapping another thread's fork/exec of its copy.
///
/// The suite runs its tests on parallel threads and `Command::spawn` forks. A
/// fork performed while another thread still holds the write descriptor from
/// `fs::copy` lets the child inherit that descriptor, and exec'ing a binary
/// any process holds open for writing fails with `ETXTBSY`. One process-local
/// mutex covering every write of the copy and every spawn of it closes that
/// window. The guard is released before the child is waited for, so child runs
/// still overlap.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn spawn_lock() -> MutexGuard<'static, ()> {
    // A test that panics elsewhere must not disable the guard for the rest.
    SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const CANONICAL: &str = ".agentscommander";
const LEGACY_HOME_NEW: &str = ".agentscommander-new";
const LEGACY_HOME_NEW_DEV: &str = ".agentscommander-new-dev";
const PROFILE_ENTRIES: [&str; 3] = [CANONICAL, LEGACY_HOME_NEW, LEGACY_HOME_NEW_DEV];

const TOKEN: &str = "00000000-0000-4000-8000-000000001850";

const CANONICAL_TOKEN: &str = "ISSUE1850-CANONICAL-TOKEN";
const LEGACY_ADJACENT_TOKEN: &str = "ISSUE1850-LEGACY-ADJACENT-TOKEN";
const LEGACY_NEW_TOKEN: &str = "ISSUE1850-LEGACY-NEW-TOKEN";
const LEGACY_NEW_DEV_TOKEN: &str = "ISSUE1850-LEGACY-NEWDEV-TOKEN";
const LEGACY_TOKENS: [&str; 3] = [
    LEGACY_ADJACENT_TOKEN,
    LEGACY_NEW_TOKEN,
    LEGACY_NEW_DEV_TOKEN,
];

const CANONICAL_SESSION: &str = "ISSUE1850-CANONICAL-SESSION";
const LEGACY_ADJACENT_SESSION: &str = "ISSUE1850-LEGACY-ADJACENT";
const LEGACY_NEW_SESSION: &str = "ISSUE1850-LEGACY-NEW";
const LEGACY_NEW_DEV_SESSION: &str = "ISSUE1850-LEGACY-NEWDEV";
const LEGACY_SESSIONS: [&str; 3] = [
    LEGACY_ADJACENT_SESSION,
    LEGACY_NEW_SESSION,
    LEGACY_NEW_DEV_SESSION,
];

const LEGACY_PAYLOAD: &[u8] = b"\x00\xff legacy opaque payload 1850 \x01\xfe";
const LEGACY_LOG: &[u8] = b"legacy app.log line 1850\n";
const CANONICAL_SENTINEL_NAME: &str = "canonical-sentinel.bin";
const CANONICAL_SENTINEL: &[u8] = b"\x7f canonical sentinel 1850 \x00\x80";
const INVALID_SETTINGS: &[u8] = b"{ \"rootToken\": \"broken-1850\", \"onboardingDismissed\": tru";
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    RealProfile,
    InjectedHome,
    RefusalOnly,
}

/// Admission gate from the phase plan §5. `windows` is injected so the table
/// test exercises the Windows rows on any host, and `known_home`/`user_profile`
/// are values rather than environment reads so the function is pure.
fn admission(
    windows: bool,
    marker: Option<&str>,
    gha: Option<&str>,
    runner: Option<&str>,
    known_home: Option<&Path>,
    user_profile: Option<&Path>,
) -> Result<Mode, String> {
    if !windows {
        return Ok(Mode::InjectedHome);
    }
    match marker {
        None => Ok(Mode::RefusalOnly),
        Some("1") => {
            if gha != Some("true") {
                return Err(format!("GITHUB_ACTIONS={gha:?}, expected \"true\""));
            }
            if runner != Some("github-hosted") {
                return Err(format!(
                    "RUNNER_ENVIRONMENT={runner:?}, expected \"github-hosted\""
                ));
            }
            let home = known_home.ok_or("dirs::home_dir() returned None")?;
            if home.as_os_str().is_empty() || !home.is_absolute() {
                return Err(format!(
                    "known home {} is not an absolute non-empty path",
                    home.display()
                ));
            }
            let profile = user_profile.ok_or("USERPROFILE is not set")?;
            let same = match (fs::canonicalize(home), fs::canonicalize(profile)) {
                (Ok(lhs), Ok(rhs)) => lhs == rhs,
                _ => home == profile,
            };
            if !same {
                return Err(format!(
                    "known home {} != USERPROFILE {} after canonicalisation",
                    home.display(),
                    profile.display()
                ));
            }
            Ok(Mode::RealProfile)
        }
        Some(other) => Err(format!(
            "AC_ISSUE1850_DISPOSABLE_PROFILE={other:?}, expected \"1\""
        )),
    }
}

type AdmissionRow<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a Path>,
    Option<&'a Path>,
);

fn absolute_home() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\profiles\runneradmin")
    } else {
        PathBuf::from("/profiles/runneradmin")
    }
}

fn absolute_other_home() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\profiles\other")
    } else {
        PathBuf::from("/profiles/other")
    }
}

/// Pure table test of the admission gate. The `Err` rows are the positive
/// control that a present marker with broken admission refuses rather than
/// degrading to `RefusalOnly`.
#[test]
fn issue_1850_admission_gate_table() {
    let home = absolute_home();
    let other = absolute_other_home();
    let relative = Path::new("relative/profile");

    // Non-Windows hosts always inject a child-local HOME; the marker can never
    // promote them to the real-profile route.
    assert_eq!(
        admission(false, None, None, None, Some(&home), None).unwrap(),
        Mode::InjectedHome
    );
    assert_eq!(
        admission(false, Some("1"), None, None, None, None).unwrap(),
        Mode::InjectedHome
    );

    // Windows without the admission marker proves refusal safety only.
    assert_eq!(
        admission(true, None, None, None, Some(&home), Some(&home)).unwrap(),
        Mode::RefusalOnly
    );

    // Fully admitted disposable profile.
    assert_eq!(
        admission(
            true,
            Some("1"),
            Some("true"),
            Some("github-hosted"),
            Some(&home),
            Some(&home)
        )
        .unwrap(),
        Mode::RealProfile
    );

    let broken: [AdmissionRow<'_>; 8] = [
        (
            "gha missing",
            None,
            Some("github-hosted"),
            Some(&home),
            Some(&home),
        ),
        (
            "gha false",
            Some("false"),
            Some("github-hosted"),
            Some(&home),
            Some(&home),
        ),
        (
            "runner self-hosted",
            Some("true"),
            Some("self-hosted"),
            Some(&home),
            Some(&home),
        ),
        (
            "runner missing",
            Some("true"),
            None,
            Some(&home),
            Some(&home),
        ),
        (
            "known home missing",
            Some("true"),
            Some("github-hosted"),
            None,
            Some(&home),
        ),
        (
            "known home relative",
            Some("true"),
            Some("github-hosted"),
            Some(relative),
            Some(relative),
        ),
        (
            "user profile missing",
            Some("true"),
            Some("github-hosted"),
            Some(&home),
            None,
        ),
        (
            "user profile different",
            Some("true"),
            Some("github-hosted"),
            Some(&home),
            Some(&other),
        ),
    ];
    for (label, gha, runner, known, profile) in broken {
        assert!(
            admission(true, Some("1"), gha, runner, known, profile).is_err(),
            "broken admission row {label:?} must refuse"
        );
    }

    // Marker values other than the literal "1" refuse too.
    assert!(admission(
        true,
        Some("0"),
        Some("true"),
        Some("github-hosted"),
        Some(&home),
        Some(&home)
    )
    .is_err());
    assert!(admission(
        true,
        Some("true"),
        Some("true"),
        Some("github-hosted"),
        Some(&home),
        Some(&home)
    )
    .is_err());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalFixture {
    Absent,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Copy)]
struct Case {
    name: &'static str,
    adjacent_legacy: bool,
    home_new: bool,
    home_new_dev: bool,
    canonical: CanonicalFixture,
}

const CASES: [Case; 7] = [
    Case {
        name: "adjacent-legacy-alone",
        adjacent_legacy: true,
        home_new: false,
        home_new_dev: false,
        canonical: CanonicalFixture::Absent,
    },
    Case {
        name: "home-new-alone",
        adjacent_legacy: false,
        home_new: true,
        home_new_dev: false,
        canonical: CanonicalFixture::Absent,
    },
    Case {
        name: "home-new-dev-alone",
        adjacent_legacy: false,
        home_new: false,
        home_new_dev: true,
        canonical: CanonicalFixture::Absent,
    },
    Case {
        name: "all-legacy-conflicting",
        adjacent_legacy: true,
        home_new: true,
        home_new_dev: true,
        canonical: CanonicalFixture::Absent,
    },
    Case {
        name: "canonical-valid-settings",
        adjacent_legacy: true,
        home_new: true,
        home_new_dev: true,
        canonical: CanonicalFixture::Valid,
    },
    Case {
        name: "canonical-invalid-settings",
        adjacent_legacy: false,
        home_new: true,
        home_new_dev: false,
        canonical: CanonicalFixture::Invalid,
    },
    Case {
        name: "canonical-absent-without-marker",
        adjacent_legacy: false,
        home_new: false,
        home_new_dev: false,
        canonical: CanonicalFixture::Absent,
    },
];

/// Driver. Runs the copy-and-run mechanism control in every mode, then either
/// the seven injected-HOME cases, the seven real-profile cases, or the
/// refusal-only safety route.
#[test]
fn issue_1850_default_root_acceptance() {
    let marker = std::env::var("AC_ISSUE1850_DISPOSABLE_PROFILE").ok();
    let gha = std::env::var("GITHUB_ACTIONS").ok();
    let runner = std::env::var("RUNNER_ENVIRONMENT").ok();
    let known_home = dirs::home_dir();
    let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);

    let mode = admission(
        cfg!(windows),
        marker.as_deref(),
        gha.as_deref(),
        runner.as_deref(),
        known_home.as_deref(),
        user_profile.as_deref(),
    )
    .unwrap_or_else(|reason| panic!("issue 1850 admission refused: {reason}"));

    let mode_label = match mode {
        Mode::RealProfile => "real-profile",
        Mode::InjectedHome => "injected-home",
        Mode::RefusalOnly => "refusal-only",
    };
    let inherited_home_env = if cfg!(windows) {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    };
    println!(
        "ISSUE1850_ADMISSION mode={mode_label} os={} job={} head={} home={} inherited_home_env={}",
        std::env::consts::OS,
        std::env::var("GITHUB_JOB").unwrap_or_else(|_| "<unset>".to_string()),
        std::env::var("GITHUB_SHA").unwrap_or_else(|_| "<unset>".to_string()),
        known_home
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        inherited_home_env.as_deref().unwrap_or("<unset>"),
    );

    refusal_census_self_control();

    if mode == Mode::RefusalOnly {
        let profile = user_profile
            .as_deref()
            .expect("refusal-only admission requires USERPROFILE");
        let pre = profile_census(profile).expect("refusal-only pre-census");
        mechanism_control();
        let post = profile_census(profile).expect("refusal-only post-census");
        census_unchanged(&pre, &post).expect("refusal-only census must be unchanged");
        let occupied = post.iter().filter(|(_, kind)| kind.is_some()).count();
        if occupied == 0 {
            println!("ISSUE1850_REFUSAL_ONLY");
        } else {
            let entries = post
                .iter()
                .map(|(name, kind)| format!("{name}={}", kind.unwrap_or("absent")))
                .collect::<Vec<_>>()
                .join(",");
            println!("ISSUE1850_REFUSAL_OCCUPIED occupied={occupied} entries={entries}");
        }
        return;
    }

    mechanism_control();

    let mut passed = 0usize;
    for case in &CASES {
        match run_case(case, mode, known_home.as_deref()) {
            Ok(()) => {
                passed += 1;
                println!("ISSUE1850_CASE_OK case={}", case.name);
            }
            Err(error) => panic!(
                "issue 1850 case {} failed after {passed} passing case(s): {error}",
                case.name
            ),
        }
    }
    assert_eq!(passed, CASES.len());

    if mode == Mode::RealProfile {
        println!("ISSUE1850_WINDOWS_PROFILE_PROOF_OK cases={passed}");
    } else {
        println!("ISSUE1850_DEFAULT_ROOT_PROOF_OK cases={passed}");
    }
}

/// Copy-and-run mechanism control: the copied unsuffixed product binary must
/// load and resolve the override root on this host before any profile-touching
/// case. This is the check that would have caught the Windows
/// `0xC0000139` loader failure in an ordinary run.
fn mechanism_control() {
    let temp = tempfile::TempDir::new().expect("mechanism TempDir");
    let case_root = temp.path();
    let bin_dir = case_root.join("bin");
    let cli_root = case_root.join("cli-root");
    let override_dir = case_root.join("override");
    fs::create_dir(&bin_dir).expect("create mechanism bin");
    fs::create_dir(&cli_root).expect("create mechanism cli-root");
    fs::create_dir(&override_dir).expect("create mechanism override");
    let binary = copy_product_binary(&bin_dir).expect("copy mechanism product binary");

    let mut command = Command::new(&binary);
    strip_inherited_config_env(&mut command);
    command
        .arg("list-peers")
        .arg("--token")
        .arg(TOKEN)
        .arg("--root")
        .arg(&cli_root)
        .current_dir(&cli_root)
        .env("AGENTSCOMMANDER_CONFIG_DIR", &override_dir);
    let run = run_child(&mut command, "mechanism control").expect("spawn mechanism control");

    print_retained("mechanism control", &run);
    assert_eq!(
        run.status,
        Some(0),
        "mechanism control child failed: status={:?} stderr={:?}",
        run.status,
        run.stderr
    );
    regular_file_nonempty(&override_dir.join(".gitignore"), "mechanism .gitignore")
        .expect("mechanism .gitignore");
    regular_file(&override_dir.join("app.log"), "mechanism app.log").expect("mechanism app.log");
    assert!(
        snapshot(&bin_dir.join(CANONICAL))
            .expect("mechanism adjacent snapshot")
            .is_none(),
        "mechanism control must not create an executable-adjacent .agentscommander"
    );
    println!("ISSUE1850_MECHANISM_OK");
}

/// Per-case harness: owns the case `TempDir`, chooses the case HOME, and keeps
/// both directories on failure so diagnostics survive.
fn run_case(case: &Case, mode: Mode, known_home: Option<&Path>) -> Result<(), String> {
    let case_temp =
        tempfile::TempDir::new().map_err(|error| format!("case TempDir failed: {error}"))?;
    let case_root = case_temp.path().to_path_buf();
    let home_temp = if mode == Mode::InjectedHome {
        Some(tempfile::TempDir::new().map_err(|error| format!("home TempDir failed: {error}"))?)
    } else {
        None
    };
    let home = match &home_temp {
        Some(temp) => temp.path().to_path_buf(),
        None => known_home
            .ok_or_else(|| "real-profile admission requires dirs::home_dir()".to_string())?
            .to_path_buf(),
    };

    match run_case_inner(case, mode, &case_root, &home) {
        Ok(()) => Ok(()),
        Err(error) => {
            let kept_case = case_temp.keep();
            let retained_home = match home_temp {
                Some(temp) => temp.keep().display().to_string(),
                None => home.display().to_string(),
            };
            Err(format!(
                "{error}\nretained case_root={} home={retained_home}",
                kept_case.display()
            ))
        }
    }
}

fn run_case_inner(case: &Case, mode: Mode, case_root: &Path, home: &Path) -> Result<(), String> {
    let bin_dir = case_root.join("bin");
    let cli_root = case_root.join("cli-root");
    fs::create_dir(&bin_dir)
        .map_err(|error| format!("create {} failed: {error}", bin_dir.display()))?;
    fs::create_dir(&cli_root)
        .map_err(|error| format!("create {} failed: {error}", cli_root.display()))?;
    let binary = copy_product_binary(&bin_dir)?;

    // Fail-closed pre-check: every profile entry must be absent before anything
    // is created, opened or adopted.
    for entry in PROFILE_ENTRIES {
        profile_entry_absent(home, entry)?;
    }

    let adjacent = bin_dir.join(CANONICAL);
    if case.adjacent_legacy {
        write_legacy_root(
            &adjacent,
            LEGACY_ADJACENT_TOKEN,
            LEGACY_ADJACENT_SESSION,
            &cli_root,
        )?;
    }
    let home_new = home.join(LEGACY_HOME_NEW);
    if case.home_new {
        write_legacy_root(&home_new, LEGACY_NEW_TOKEN, LEGACY_NEW_SESSION, &cli_root)?;
    }
    let home_new_dev = home.join(LEGACY_HOME_NEW_DEV);
    if case.home_new_dev {
        write_legacy_root(
            &home_new_dev,
            LEGACY_NEW_DEV_TOKEN,
            LEGACY_NEW_DEV_SESSION,
            &cli_root,
        )?;
    }

    let canonical = home.join(CANONICAL);
    match case.canonical {
        CanonicalFixture::Absent => {}
        CanonicalFixture::Valid => {
            fs::create_dir(&canonical)
                .map_err(|error| format!("create {} failed: {error}", canonical.display()))?;
            let settings = serde_json::json!({
                "defaultShell": "cmd",
                "defaultShellArgs": [],
                "agents": [],
                "rootToken": CANONICAL_TOKEN,
                "onboardingDismissed": true,
            });
            write_json(&canonical.join("settings.json"), &settings)?;
            write_json(
                &canonical.join("sessions.json"),
                &serde_json::json!([session_row(
                    "canonical-session-id",
                    CANONICAL_SESSION,
                    &cli_root
                )]),
            )?;
            fs::write(canonical.join(CANONICAL_SENTINEL_NAME), CANONICAL_SENTINEL)
                .map_err(|error| format!("write canonical sentinel failed: {error}"))?;
        }
        CanonicalFixture::Invalid => {
            fs::create_dir(&canonical)
                .map_err(|error| format!("create {} failed: {error}", canonical.display()))?;
            fs::write(canonical.join("settings.json"), INVALID_SETTINGS)
                .map_err(|error| format!("write invalid settings failed: {error}"))?;
        }
    }

    // The canonical root is seeded only in the valid/invalid cases and, when
    // seeded, deliberately does not carry `.gitignore` or `app.log`; that is
    // what makes their later presence a child-created proof.
    if canonical.exists() {
        for forbidden in ["app.log", ".gitignore"] {
            if snapshot(&canonical.join(forbidden))?.is_some() {
                return Err(format!(
                    "{}: canonical root was seeded with {forbidden}, \
                     which would hollow out the creation oracle",
                    case.name
                ));
            }
        }
    }

    // Roots this case created under the case HOME. The canonical root always
    // joins the list: its absence was established above, so whatever ends up
    // there is test-owned output.
    let mut owned_home_roots: Vec<PathBuf> = Vec::new();
    if case.home_new {
        owned_home_roots.push(home_new.clone());
    }
    if case.home_new_dev {
        owned_home_roots.push(home_new_dev.clone());
    }
    owned_home_roots.push(canonical.clone());

    println!(
        "ISSUE1850_CASE_FIXTURES case={} adjacent_legacy={} home_new={} home_new_dev={} \
         canonical={:?} case_root={} home={} canonical_pre_gitignore=absent \
         canonical_pre_applog=absent",
        case.name,
        if case.adjacent_legacy {
            "seeded"
        } else {
            "absent"
        },
        if case.home_new { "seeded" } else { "absent" },
        if case.home_new_dev {
            "seeded"
        } else {
            "absent"
        },
        case.canonical,
        case_root.display(),
        home.display()
    );

    // Step 4 snapshot.
    let adjacent_before = snapshot(&adjacent)?;
    let home_new_before = snapshot(&home_new)?;
    let home_new_dev_before = snapshot(&home_new_dev)?;
    let invalid_settings_before = if case.canonical == CanonicalFixture::Invalid {
        Some(
            fs::read(canonical.join("settings.json"))
                .map_err(|error| format!("read invalid settings failed: {error}"))?,
        )
    } else {
        None
    };

    // Child A: list-peers, real startup path (preflight + logger), cwd inside
    // the case.
    let mut child_a = Command::new(&binary);
    strip_inherited_config_env(&mut child_a);
    child_a
        .arg("list-peers")
        .arg("--token")
        .arg(TOKEN)
        .arg("--root")
        .arg(&cli_root)
        .current_dir(&cli_root);
    inject_injected_home(&mut child_a, mode, home);
    let run_a = run_child(&mut child_a, &format!("{} child A", case.name))?;
    print_retained(&format!("{} child A", case.name), &run_a);
    if case.canonical == CanonicalFixture::Invalid {
        if run_a.success
            || run_a.status != Some(1)
            || !run_a
                .stderr
                .contains("settings.json exists but could not be parsed")
        {
            return Err(format!(
                "{}: child A did not refuse invalid settings status={:?} stderr={:?}",
                case.name, run_a.status, run_a.stderr
            ));
        }
        if run_a
            .stdout
            .char_indices()
            .filter(|(_, character)| *character == '[' || *character == '{')
            .any(|(offset, _)| {
                matches!(
                    serde_json::Deserializer::from_str(&run_a.stdout[offset..])
                        .into_iter::<serde_json::Value>()
                        .next(),
                    Some(Ok(
                        serde_json::Value::Array(_) | serde_json::Value::Object(_)
                    ))
                )
            })
        {
            return Err(format!(
                "{}: child A emitted successful peer JSON: {:?}",
                case.name, run_a.stdout
            ));
        }
    } else if !run_a.success {
        return Err(format!(
            "{}: child A list-peers failed status={:?} stderr={:?}",
            case.name, run_a.status, run_a.stderr
        ));
    }

    // Child B: list-sessions, reads <selected root>/sessions.json.
    let mut child_b = Command::new(&binary);
    strip_inherited_config_env(&mut child_b);
    child_b.arg("list-sessions").current_dir(&cli_root);
    inject_injected_home(&mut child_b, mode, home);
    let run_b = run_child(&mut child_b, &format!("{} child B", case.name))?;
    print_retained(&format!("{} child B", case.name), &run_b);
    if !run_b.success {
        return Err(format!(
            "{}: child B list-sessions failed status={:?} stderr={:?}",
            case.name, run_b.status, run_b.stderr
        ));
    }
    let child_b_json: serde_json::Value = serde_json::from_str(run_b.stdout.trim())
        .map_err(|error| format!("{}: child B stdout is not JSON: {error}", case.name))?;
    if !child_b_json.is_array() {
        return Err(format!(
            "{}: child B stdout is not a JSON array: {:?}",
            case.name, run_b.stdout
        ));
    }

    // Selection oracle: app.log exists (possibly 0 bytes), .gitignore exists
    // and is non-empty, and neither was in the step-4 snapshot.
    regular_file(
        &canonical.join("app.log"),
        &format!("{}: app.log", case.name),
    )?;
    regular_file_nonempty(
        &canonical.join(".gitignore"),
        &format!("{}: .gitignore", case.name),
    )?;

    // Preservation: legacy roots and executable-adjacent entries are
    // byte-identical to their snapshots, or still absent when not seeded.
    if snapshot(&home_new)? != home_new_before {
        return Err(format!(
            "{}: legacy HOME root {} changed",
            case.name,
            home_new.display()
        ));
    }
    if snapshot(&home_new_dev)? != home_new_dev_before {
        return Err(format!(
            "{}: legacy HOME root {} changed",
            case.name,
            home_new_dev.display()
        ));
    }
    if snapshot(&adjacent)? != adjacent_before {
        return Err(format!(
            "{}: executable-adjacent root {} changed",
            case.name,
            adjacent.display()
        ));
    }

    // No legacy payload, token or marker may appear under the canonical root.
    scan_canonical_for_legacy(&canonical, case.name)?;

    // Legacy session names must never reach child B's stdout.
    for legacy in LEGACY_SESSIONS {
        if run_b.stdout.contains(legacy) {
            return Err(format!(
                "{}: child B exposed legacy session {legacy}: {:?}",
                case.name, run_b.stdout
            ));
        }
    }

    let settings_path = canonical.join("settings.30.instance.no-git.json");
    if case.canonical != CanonicalFixture::Invalid {
        // Settings identity: whenever a settings file appeared, it parses.
        // The invalid fixture is the explicit exception, asserted below.
        if settings_path.exists() {
            let settings = read_json(&settings_path, case.name)?;
            if case.canonical == CanonicalFixture::Absent {
                if let Some(token) = settings.get("rootToken").and_then(|value| value.as_str()) {
                    if LEGACY_TOKENS.contains(&token) {
                        return Err(format!(
                            "{}: fresh canonical settings carried a legacy rootToken {token:?}",
                            case.name
                        ));
                    }
                }
                if settings.get("legacyMarker").is_some() {
                    return Err(format!(
                        "{}: fresh canonical settings carried a legacyMarker key",
                        case.name
                    ));
                }
                if settings
                    .get("onboardingDismissed")
                    .and_then(|value| value.as_bool())
                    == Some(true)
                {
                    return Err(format!(
                        "{}: fresh canonical settings carried a legacy onboarding preference",
                        case.name
                    ));
                }
            }
        }
    }

    match case.canonical {
        CanonicalFixture::Absent => {}
        CanonicalFixture::Valid => {
            if !run_b.stdout.contains(CANONICAL_SESSION) {
                return Err(format!(
                    "{}: seeded canonical session never appeared in child B stdout: {:?}",
                    case.name, run_b.stdout
                ));
            }
            let settings = read_json(&settings_path, case.name)?;
            if settings.get("rootToken").and_then(|value| value.as_str()) != Some(CANONICAL_TOKEN) {
                return Err(format!(
                    "{}: canonical settings lost rootToken: {settings}",
                    case.name
                ));
            }
            if settings
                .get("onboardingDismissed")
                .and_then(|value| value.as_bool())
                != Some(true)
            {
                return Err(format!(
                    "{}: canonical settings lost the seeded preference: {settings}",
                    case.name
                ));
            }
            let sentinel = fs::read(canonical.join(CANONICAL_SENTINEL_NAME)).map_err(|error| {
                format!("{}: read canonical sentinel failed: {error}", case.name)
            })?;
            if sentinel != CANONICAL_SENTINEL {
                return Err(format!("{}: canonical sentinel bytes changed", case.name));
            }
        }
        CanonicalFixture::Invalid => {
            let after = fs::read(&settings_path).map_err(|error| {
                format!(
                    "{}: read invalid settings {} failed: {error}",
                    case.name,
                    settings_path.display()
                )
            })?;
            let before = invalid_settings_before.expect("invalid settings snapshot");
            if after != before {
                return Err(format!(
                    "{}: invalid canonical settings bytes were rewritten",
                    case.name
                ));
            }
        }
    }

    // Cleanup of this case's owned roots only, then prove the profile is empty
    // again before the next case.
    for root in &owned_home_roots {
        if root.exists() {
            fs::remove_dir_all(root).map_err(|error| {
                format!("{}: remove {} failed: {error}", case.name, root.display())
            })?;
        }
    }
    for entry in PROFILE_ENTRIES {
        profile_entry_absent(home, entry)?;
    }
    Ok(())
}
