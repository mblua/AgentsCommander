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
//! - non-Windows: the eight cases run against a fresh per-case `TempDir` HOME.
//! - Windows with the disposable-profile admission: the eight cases run
//!   against `dirs::home_dir()`, the known-folder profile of a disposable
//!   GitHub-hosted VM.
//! - Windows without admission: the copy-and-run mechanism control plus
//!   refusal safety only; no profile root is created, opened or adopted.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CANONICAL: &str = ".agentscommander";
const LEGACY_HOME_NEW: &str = ".agentscommander-new";
const LEGACY_HOME_NEW_DEV: &str = ".agentscommander-new-dev";
const PROFILE_ENTRIES: [&str; 3] = [CANONICAL, LEGACY_HOME_NEW, LEGACY_HOME_NEW_DEV];
const PORTABLE_MARKER: &str = "portable.txt";

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
const PORTABLE_MARKER_BYTES: &[u8] = b"portable marker beside an unsuffixed executable\n";
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
    portable: bool,
}

const CASES: [Case; 8] = [
    Case {
        name: "adjacent-legacy-alone",
        adjacent_legacy: true,
        home_new: false,
        home_new_dev: false,
        canonical: CanonicalFixture::Absent,
        portable: false,
    },
    Case {
        name: "home-new-alone",
        adjacent_legacy: false,
        home_new: true,
        home_new_dev: false,
        canonical: CanonicalFixture::Absent,
        portable: false,
    },
    Case {
        name: "home-new-dev-alone",
        adjacent_legacy: false,
        home_new: false,
        home_new_dev: true,
        canonical: CanonicalFixture::Absent,
        portable: false,
    },
    Case {
        name: "all-legacy-conflicting",
        adjacent_legacy: true,
        home_new: true,
        home_new_dev: true,
        canonical: CanonicalFixture::Absent,
        portable: false,
    },
    Case {
        name: "canonical-valid-settings",
        adjacent_legacy: true,
        home_new: true,
        home_new_dev: true,
        canonical: CanonicalFixture::Valid,
        portable: false,
    },
    Case {
        name: "canonical-invalid-settings",
        adjacent_legacy: false,
        home_new: true,
        home_new_dev: false,
        canonical: CanonicalFixture::Invalid,
        portable: false,
    },
    Case {
        name: "canonical-absent-with-marker",
        adjacent_legacy: false,
        home_new: false,
        home_new_dev: false,
        canonical: CanonicalFixture::Absent,
        portable: true,
    },
    Case {
        name: "canonical-absent-without-marker",
        adjacent_legacy: false,
        home_new: false,
        home_new_dev: false,
        canonical: CanonicalFixture::Absent,
        portable: false,
    },
];

/// Driver. Runs the copy-and-run mechanism control in every mode, then either
/// the eight injected-HOME cases, the eight real-profile cases, or the
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

    if mode == Mode::RefusalOnly {
        let profile = user_profile
            .as_deref()
            .expect("refusal-only admission requires USERPROFILE");
        for entry in PROFILE_ENTRIES {
            profile_entry_absent(profile, entry).expect("refusal-only pre-check");
        }
        mechanism_control();
        for entry in PROFILE_ENTRIES {
            profile_entry_absent(profile, entry).expect("refusal-only post-check");
        }
        println!("ISSUE1850_REFUSAL_ONLY");
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

    let marker = bin_dir.join(PORTABLE_MARKER);
    if case.portable {
        fs::write(&marker, PORTABLE_MARKER_BYTES)
            .map_err(|error| format!("write {} failed: {error}", marker.display()))?;
    }

    // The canonical root is seeded only in the valid/invalid cases and, when
    // seeded, deliberately does not carry `.gitignore` or `app.log`; that is
    // what makes their later presence a child-created proof.
    if canonical.exists() {
        for forbidden in ["app.log", ".gitignore"] {
            if snapshot(&canonical.join(forbidden))?.is_some() {
                return Err(format!(
                    "{}: canonical root was seeded with {forbidden}, which would hollow out the creation oracle",
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
        "ISSUE1850_CASE_FIXTURES case={} adjacent_legacy={} home_new={} home_new_dev={} canonical={:?} portable={} case_root={} home={} canonical_pre_gitignore=absent canonical_pre_applog=absent",
        case.name,
        if case.adjacent_legacy { "seeded" } else { "absent" },
        if case.home_new { "seeded" } else { "absent" },
        if case.home_new_dev { "seeded" } else { "absent" },
        case.canonical,
        if case.portable { "present" } else { "absent" },
        case_root.display(),
        home.display()
    );

    // Step 4 snapshot.
    let adjacent_before = snapshot(&adjacent)?;
    let home_new_before = snapshot(&home_new)?;
    let home_new_dev_before = snapshot(&home_new_dev)?;
    let marker_before = snapshot(&marker)?;
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
    if !run_a.success {
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
    if snapshot(&marker)? != marker_before {
        return Err(format!(
            "{}: executable-adjacent {} changed",
            case.name,
            marker.display()
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

    let settings_path = canonical.join("settings.json");
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

fn inject_injected_home(command: &mut Command, mode: Mode, home: &Path) {
    if mode == Mode::InjectedHome {
        command.env("HOME", home);
    }
}

fn strip_inherited_config_env(command: &mut Command) {
    command
        .env_remove("AGENTSCOMMANDER_CONFIG_DIR")
        .env_remove("AGENTSCOMMANDER_TEST_CONFIG_DIR")
        .env_remove("AC_MACHINE_OUTPUT");
}

fn copy_product_binary(bin_dir: &Path) -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "agentscommander.exe"
    } else {
        "agentscommander"
    };
    let source = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let destination = bin_dir.join(name);
    fs::copy(source, &destination).map_err(|error| {
        format!(
            "copy {} -> {} failed: {error}",
            source.display(),
            destination.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&destination)
            .map_err(|error| format!("copied binary metadata failed: {error}"))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(&destination, permissions)
            .map_err(|error| format!("set executable mode failed: {error}"))?;
    }
    Ok(destination)
}

fn profile_entry_absent(home: &Path, name: &str) -> Result<(), String> {
    let path = home.join(name);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "symlink_metadata {} failed with {:?}: {error}",
            path.display(),
            error.kind()
        )),
        Ok(metadata) => Err(format!(
            "profile entry {} already exists ({:?}); refusing to create, open, clean or adopt it",
            path.display(),
            metadata.file_type()
        )),
    }
}

fn regular_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "{label}: symlink_metadata {} failed: {error}",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!("{label}: {} is not a regular file", path.display()));
    }
    Ok(())
}

fn regular_file_nonempty(path: &Path, label: &str) -> Result<(), String> {
    regular_file(path, label)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "{label}: symlink_metadata {} failed: {error}",
            path.display()
        )
    })?;
    if metadata.len() == 0 {
        return Err(format!("{label}: {} is empty", path.display()));
    }
    Ok(())
}

fn write_legacy_root(
    root: &Path,
    token: &str,
    session_name: &str,
    working_directory: &Path,
) -> Result<(), String> {
    // Non-recursive: a pre-existing path is a collision, not state to adopt.
    fs::create_dir(root)
        .map_err(|error| format!("create_dir {} failed: {error}", root.display()))?;
    let settings = serde_json::json!({
        "defaultShell": "cmd",
        "defaultShellArgs": [],
        "agents": [],
        "rootToken": token,
        "legacyMarker": token,
    });
    write_json(&root.join("settings.json"), &settings)?;
    write_json(
        &root.join("sessions.json"),
        &serde_json::json!([session_row(
            &format!("{token}-session-id"),
            session_name,
            working_directory
        )]),
    )?;
    for (name, bytes) in [("app.log", LEGACY_LOG), ("payload.bin", LEGACY_PAYLOAD)] {
        fs::write(root.join(name), bytes)
            .map_err(|error| format!("write {} failed: {error}", root.join(name).display()))?;
    }
    let nested = root.join("instances").join("legacy");
    fs::create_dir_all(&nested)
        .map_err(|error| format!("create {} failed: {error}", nested.display()))?;
    fs::write(nested.join("web-token.txt"), token.as_bytes())
        .map_err(|error| format!("write nested legacy token failed: {error}"))?;
    Ok(())
}

fn session_row(id: &str, name: &str, working_directory: &Path) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "shell": "cmd",
        "shellArgs": [],
        "workingDirectory": working_directory.to_string_lossy(),
    })
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("serialize {} failed: {error}", path.display()))?;
    fs::write(path, text).map_err(|error| format!("write {} failed: {error}", path.display()))
}

fn read_json(path: &Path, case: &str) -> Result<serde_json::Value, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("{case}: read {} failed: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{case}: {} is not valid JSON: {error}", path.display()))
}

/// Sorted recursive inventory of relative path, entry type and SHA-256 of file
/// bytes. Directories and symlinks carry an empty digest; files carry the hex
/// digest of their bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InventoryEntry {
    relative: String,
    kind: &'static str,
    digest: String,
}

fn snapshot(root: &Path) -> Result<Option<Vec<InventoryEntry>>, String> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "symlink_metadata {} failed: {error}",
            root.display()
        )),
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                let bytes = fs::read(root)
                    .map_err(|error| format!("read {} failed: {error}", root.display()))?;
                return Ok(Some(vec![InventoryEntry {
                    relative: root
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| ".".to_string()),
                    kind: if metadata.file_type().is_file() {
                        "file"
                    } else {
                        "other"
                    },
                    digest: if metadata.file_type().is_file() {
                        sha256_hex(&bytes)
                    } else {
                        String::new()
                    },
                }]));
            }
            let mut entries = Vec::new();
            inventory_into(root, root, &mut entries)?;
            entries.sort_by(|lhs, rhs| lhs.relative.cmp(&rhs.relative));
            Ok(Some(entries))
        }
    }
}

fn inventory_into(root: &Path, dir: &Path, out: &mut Vec<InventoryEntry>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("read_dir {} failed: {error}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("read entry in {} failed: {error}", dir.display()))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("strip prefix failed: {error}"))?
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("symlink_metadata {} failed: {error}", path.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            out.push(InventoryEntry {
                relative,
                kind: "symlink",
                digest: String::new(),
            });
        } else if file_type.is_dir() {
            out.push(InventoryEntry {
                relative: relative.clone(),
                kind: "dir",
                digest: String::new(),
            });
            inventory_into(root, &path, out)?;
        } else if file_type.is_file() {
            let bytes = fs::read(&path)
                .map_err(|error| format!("read {} failed: {error}", path.display()))?;
            out.push(InventoryEntry {
                relative,
                kind: "file",
                digest: sha256_hex(&bytes),
            });
        } else {
            out.push(InventoryEntry {
                relative,
                kind: "other",
                digest: String::new(),
            });
        }
    }
    Ok(())
}

fn scan_canonical_for_legacy(root: &Path, case: &str) -> Result<(), String> {
    fn walk(root: &Path, dir: &Path, case: &str) -> Result<(), String> {
        let entries = fs::read_dir(dir)
            .map_err(|error| format!("{case}: read_dir {} failed: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("{case}: read entry in {} failed: {error}", dir.display())
            })?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|error| format!("{case}: strip prefix failed: {error}"))?
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!(
                    "{case}: symlink_metadata {} failed: {error}",
                    path.display()
                )
            })?;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                if relative == "instances/legacy" {
                    return Err(format!(
                        "{case}: legacy instances directory appeared at {relative}"
                    ));
                }
                walk(root, &path, case)?;
            } else if file_type.is_file() {
                let leaf = relative.rsplit('/').next().unwrap_or(relative.as_str());
                if leaf == "payload.bin" {
                    return Err(format!(
                        "{case}: legacy payload entry appeared at {relative}"
                    ));
                }
                let bytes = fs::read(&path)
                    .map_err(|error| format!("{case}: read {} failed: {error}", path.display()))?;
                if bytes == LEGACY_PAYLOAD {
                    return Err(format!(
                        "{case}: legacy payload bytes appeared at {relative}"
                    ));
                }
                let text = String::from_utf8_lossy(&bytes);
                for token in LEGACY_TOKENS {
                    if text.contains(token) {
                        return Err(format!(
                            "{case}: legacy token {token} leaked into {relative}"
                        ));
                    }
                }
                if text.contains("legacyMarker") {
                    return Err(format!("{case}: legacyMarker leaked into {relative}"));
                }
            }
        }
        Ok(())
    }
    walk(root, root, case)
}

struct ChildRun {
    status: Option<i32>,
    success: bool,
    stdout: String,
    stderr: String,
}

fn run_child(command: &mut Command, label: &str) -> Result<ChildRun, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn child for {label} failed: {error}"))?;
    let deadline = Instant::now() + CHILD_TIMEOUT;
    let mut status_poll_error = None;
    let terminal_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => break None,
            Err(error) => {
                status_poll_error = Some(error);
                break None;
            }
        }
    };
    if terminal_status.is_none() {
        let kill_error = child.kill().err();
        let reap_deadline = Instant::now() + REAP_TIMEOUT;
        let reaped = loop {
            match child.try_wait() {
                Ok(Some(_)) => break true,
                Ok(None) if Instant::now() < reap_deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) | Err(_) => break false,
            }
        };
        if !reaped {
            return Err(format!(
                "{label}: child did not reap within 5 seconds; kill_error={kill_error:?} poll_error={status_poll_error:?}"
            ));
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("{label}: timed-out child final reap failed: {error}"))?;
        return Err(format!(
            "{label}: child did not reach a clean terminal status; kill_error={kill_error:?} poll_error={status_poll_error:?} status={:?} stdout={:?} stderr={:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("{label}: child output collection failed: {error}"))?;
    Ok(ChildRun {
        status: output.status.code(),
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn print_retained(label: &str, run: &ChildRun) {
    println!("[child {label}] status={:?}", run.status);
    for line in run.stdout.lines() {
        println!("[child {label} stdout] {line}");
    }
    for line in run.stderr.lines() {
        println!("[child {label} stderr] {line}");
    }
}

/// Test-local SHA-256 (FIPS 180-4). The frozen manifest forbids adding a
/// dependency for the inventory digests, and the plan names only `std`,
/// `tempfile`, `dirs` and `serde_json`.
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (bytes.len() as u64) * 8;
    let mut message = bytes.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|word| format!("{word:08x}")).collect()
}
