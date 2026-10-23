use super::*;
use std::collections::VecDeque;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;

mod write_probe;

fn exe_err() -> Result<PathBuf, Error> {
    Err(Error::new(ErrorKind::NotFound, "no current_exe"))
}

/// A SUFFIXED absolute executable: the only kind that still reaches the
/// adjacent write-probe table after #1868. Its complete stem names the
/// adjacent directory.
fn absolute_executable() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\bundle\agentscommander_portable.exe")
    } else {
        PathBuf::from("/opt/bundle/agentscommander_portable")
    }
}

fn expected_adjacent() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\bundle\.agentscommander_portable")
    } else {
        PathBuf::from("/opt/bundle/.agentscommander_portable")
    }
}

/// The canonical unsuffixed executable of a packaged install (#1868).
fn unsuffixed_executable() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\bundle\agentscommander.exe")
    } else {
        PathBuf::from("/opt/bundle/agentscommander")
    }
}

fn canonical_home(home: &Path) -> PathBuf {
    home.join(".agentscommander")
}

fn failed_write(
    platform: RetryPlatform,
    operation: ProbeOperation,
    path: PathBuf,
    error: io::Error,
    attempts: usize,
) -> WriteProbeFailure {
    let primary = ProbeFailure::from_retry(
        platform,
        operation,
        path.clone(),
        RetriedIoError { error, attempts },
    );
    WriteProbeFailure::new(primary, None, Some(path), false)
}

#[test]
fn packaged_absolute_executable_yields_portable_config_and_base() {
    // #1868: portable selection is a suffixed-executable behavior; the
    // fixture carries a suffix and its complete stem names the directory.
    let loc = resolve_instance_location(
        None,
        None,
        Ok(absolute_executable()),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Success,
    );
    let expected_base = if cfg!(windows) {
        PathBuf::from(r"C:\bundle")
    } else {
        PathBuf::from("/opt/bundle")
    };
    assert_eq!(loc.config_dir, Some(expected_adjacent()));
    assert_eq!(loc.instance_base.as_deref(), Some(expected_base.as_path()));
    assert_eq!(loc.local_dir_stem, "agentscommander_portable");
}

#[test]
fn renamed_executable_keeps_per_stem_config_and_base() {
    let exe = if cfg!(windows) {
        PathBuf::from(r"C:\tools\agentscommander_standalone.exe")
    } else {
        PathBuf::from("/tools/agentscommander_standalone")
    };
    let loc = resolve_instance_location(
        None,
        None,
        Ok(exe),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Success,
    );
    let expected_config = if cfg!(windows) {
        PathBuf::from(r"C:\tools\.agentscommander_standalone")
    } else {
        PathBuf::from("/tools/.agentscommander_standalone")
    };
    assert_eq!(loc.config_dir.as_deref(), Some(expected_config.as_path()));
    assert_eq!(loc.local_dir_stem, "agentscommander_standalone");
    assert!(loc.instance_base.is_some());
}

#[test]
fn absolute_debug_override_sets_config_verbatim_and_parent_base() {
    let override_dir = if cfg!(windows) {
        r"C:\bundle\.agentscommander"
    } else {
        "/opt/bundle/.agentscommander"
    };
    let exe = if cfg!(windows) {
        PathBuf::from(r"C:\somewhere\else\ac.exe")
    } else {
        PathBuf::from("/somewhere/else/ac")
    };
    let loc = resolve_instance_location(
        None,
        Some(override_dir.to_string()),
        Ok(exe),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::NotRun,
    );
    assert_eq!(loc.config_dir.as_deref(), Some(Path::new(override_dir)));
    let expected_base = if cfg!(windows) {
        PathBuf::from(r"C:\bundle")
    } else {
        PathBuf::from("/opt/bundle")
    };
    assert_eq!(loc.instance_base.as_deref(), Some(expected_base.as_path()));
    // The local dir stem still comes from the executable, not the override.
    assert_eq!(loc.local_dir_stem, "ac");
}

#[test]
fn relative_debug_override_sets_config_but_no_base() {
    let loc = resolve_instance_location(
        None,
        Some("relative/.acdir".to_string()),
        exe_err(),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::NotRun,
    );
    assert_eq!(
        loc.config_dir.as_deref(),
        Some(Path::new("relative/.acdir"))
    );
    assert_eq!(loc.instance_base, None);
    assert_eq!(loc.local_dir_stem, "agentscommander");
}

#[test]
fn blank_debug_override_is_ignored() {
    let exe = if cfg!(windows) {
        PathBuf::from(r"C:\bundle\ac_portable.exe")
    } else {
        PathBuf::from("/opt/bundle/ac_portable")
    };
    let loc = resolve_instance_location(
        None,
        Some("   ".to_string()),
        Ok(exe),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Success,
    );
    // Falls through to the portable executable-derived config (suffixed
    // fixture: the unsuffixed route selects HOME instead, see #1868).
    let expected_config = if cfg!(windows) {
        PathBuf::from(r"C:\bundle\.ac_portable")
    } else {
        PathBuf::from("/opt/bundle/.ac_portable")
    };
    assert_eq!(loc.config_dir.as_deref(), Some(expected_config.as_path()));
    assert!(loc.instance_base.is_some());
}

#[test]
fn relative_executable_keeps_relative_config_but_no_base() {
    // Suffixed fixture: a relative UNSUFFIXED executable selects HOME (#1868).
    let loc = resolve_instance_location(
        None,
        None,
        Ok(PathBuf::from("bin/agentscommander_portable")),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Success,
    );
    assert_eq!(
        loc.config_dir.as_deref(),
        Some(Path::new("bin/.agentscommander_portable"))
    );
    assert_eq!(
        loc.instance_base, None,
        "relative exe exposes no portable base"
    );
    assert_eq!(loc.local_dir_stem, "agentscommander_portable");
}

#[test]
fn missing_parent_takes_home_fallback() {
    // A bare root path has a stem-less/parent-less shape → home fallback.
    let root = if cfg!(windows) {
        PathBuf::from(r"C:\")
    } else {
        PathBuf::from("/")
    };
    let loc = resolve_instance_location(
        None,
        None,
        Ok(root),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::NotRun,
    );
    let expected = PathBuf::from("/home/u").join(profile::config_dir_name());
    assert_eq!(loc.config_dir.as_deref(), Some(expected.as_path()));
    assert_eq!(loc.instance_base, None);
}

#[test]
fn current_exe_failure_uses_home_fallback_and_default_stem() {
    let loc = resolve_instance_location(
        None,
        None,
        exe_err(),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::NotRun,
    );
    let expected = PathBuf::from("/home/u").join(profile::config_dir_name());
    assert_eq!(loc.config_dir.as_deref(), Some(expected.as_path()));
    assert_eq!(loc.instance_base, None);
    assert_eq!(loc.local_dir_stem, "agentscommander");
}

#[test]
fn current_exe_failure_and_no_home_yields_none_config() {
    let loc = resolve_instance_location(None, None, exe_err(), None, WriteProbeOutcome::NotRun);
    assert_eq!(loc.config_dir, None);
    assert_eq!(loc.instance_base, None);
}

#[test]
fn issue_1577_public_override_beats_debug_and_probe_failures() {
    let public = if cfg!(windows) {
        r"C:\public config"
    } else {
        "/public config"
    };
    let loc = resolve_instance_location(
        Some(public.to_string()),
        Some("debug-canary".to_string()),
        Ok(absolute_executable()),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Failed(failed_write(
            RetryPlatform::Other,
            ProbeOperation::CreateConfigurationDirectory,
            expected_adjacent(),
            Error::new(ErrorKind::PermissionDenied, "denied"),
            1,
        )),
    );

    assert_eq!(loc.config_dir.as_deref(), Some(Path::new(public)));
    assert!(loc.startup_error.is_none());
}

#[test]
fn issue_1577_blank_overrides_fall_through_to_adjacent_success() {
    let loc = resolve_instance_location(
        Some("   ".to_string()),
        Some("\t".to_string()),
        Ok(absolute_executable()),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Success,
    );

    assert_eq!(loc.config_dir, Some(expected_adjacent()));
    assert!(loc.startup_error.is_none());
}

#[test]
fn issue_1930_unmarked_conclusive_failure_refuses_and_never_selects_home() {
    let failure = failed_write(
        RetryPlatform::Other,
        ProbeOperation::CreateConfigurationDirectory,
        expected_adjacent(),
        Error::new(ErrorKind::PermissionDenied, "permission denied"),
        1,
    );
    for home in [Some("/home/u"), None] {
        let loc = resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            home.map(PathBuf::from),
            WriteProbeOutcome::Failed(failure.clone()),
        );
        assert_eq!(loc.config_dir, Some(expected_adjacent()));
        assert_eq!(
            loc.instance_base,
            expected_adjacent().parent().map(Path::to_path_buf)
        );
        assert_eq!(
            loc.startup_error,
            Some(ConfigStartupError::AdjacentDirectoryUnwritable {
                config_dir: expected_adjacent(),
                reason: failure.reason(),
            })
        );
    }
    // A bare suffixed name still has a parent (the empty path), so no suffixed
    // executable reaches HOME through a missing parent or stem.
    let loc = resolve_instance_location(
        None,
        None,
        Ok(PathBuf::from("agentscommander_bare")),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Failed(failure),
    );
    assert_eq!(loc.config_dir, Some(PathBuf::from(".agentscommander_bare")));
    assert_eq!(loc.instance_base, None);
    assert!(matches!(
        loc.startup_error,
        Some(ConfigStartupError::AdjacentDirectoryUnwritable { .. })
    ));
}

#[test]
fn issue_1577_unmarked_indeterminate_failure_never_relocates() {
    let failure = failed_write(
        RetryPlatform::Other,
        ProbeOperation::CreateProbeFile,
        expected_adjacent().join("probe.tmp"),
        Error::new(ErrorKind::AlreadyExists, "collision"),
        1,
    );
    let loc = resolve_instance_location(
        None,
        None,
        Ok(absolute_executable()),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Failed(failure.clone()),
    );

    assert_eq!(loc.config_dir, Some(expected_adjacent()));
    assert_eq!(
        loc.startup_error,
        Some(ConfigStartupError::AdjacentSelectionBlocked {
            config_dir: expected_adjacent(),
            reason: failure.reason(),
        })
    );
    assert!(!loc
        .startup_error
        .unwrap()
        .to_string()
        .contains(&PathBuf::from("/home/u").display().to_string()));
}

/// #2816 (C3) E9a - the three isolation mechanisms a test fixture relies
/// on, as resolver behaviour: public override, debug override, suffixed
/// executable, and the public override first.
#[test]
fn issue_2816_the_isolation_mechanisms_resolve_in_precedence_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let public = tmp.path().join("public");
    let debug = tmp.path().join("debug");
    let exe = tmp.path().join("bin").join("agentscommander_copy.exe");
    let text = |path: &PathBuf| Some(path.to_string_lossy().to_string());
    let resolve = |public_override, test_override| {
        resolve_instance_location(
            public_override,
            test_override,
            Ok(exe.clone()),
            Some(PathBuf::from("/home/u")),
            WriteProbeOutcome::Success,
        )
        .config_dir
    };

    assert_eq!(resolve(text(&public), None), Some(public.clone()));
    assert_eq!(resolve(None, text(&debug)), Some(debug.clone()));
    assert_eq!(
        resolve(None, None),
        Some(tmp.path().join("bin").join(".agentscommander_copy"))
    );
    assert_eq!(
        resolve(text(&public), text(&debug)),
        Some(public.clone()),
        "the public override wins over the debug override"
    );
}

#[test]
fn issue_1577_identical_inputs_produce_identical_complete_location() {
    let resolve = || {
        resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            Some(PathBuf::from("/home/u")),
            WriteProbeOutcome::Success,
        )
    };
    assert_eq!(resolve(), resolve());
}
#[test]
fn issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome() {
    let home = PathBuf::from("/home/u");
    let root = if cfg!(windows) {
        PathBuf::from(r"C:\")
    } else {
        PathBuf::from("/")
    };
    let hyphenated = if cfg!(windows) {
        PathBuf::from(r"C:\bundle\agentscommander-stage.exe")
    } else {
        PathBuf::from("/opt/bundle/agentscommander-stage")
    };
    // `None` stands for a failing `current_exe()`.
    let executables: Vec<(Option<PathBuf>, &str)> = vec![
        (Some(unsuffixed_executable()), "agentscommander"),
        (Some(hyphenated), "agentscommander-stage"),
        (
            Some(PathBuf::from("bin/agentscommander")),
            "agentscommander",
        ),
        (Some(PathBuf::from("ac")), "ac"),
        (Some(root), "agentscommander"),
        (None, "agentscommander"),
    ];
    let write_outcomes = || {
        [
            WriteProbeOutcome::NotRun,
            WriteProbeOutcome::Success,
            WriteProbeOutcome::Failed(failed_write(
                RetryPlatform::Other,
                ProbeOperation::CreateConfigurationDirectory,
                expected_adjacent(),
                Error::new(ErrorKind::PermissionDenied, "denied"),
                1,
            )),
            WriteProbeOutcome::Failed(failed_write(
                RetryPlatform::Other,
                ProbeOperation::CreateProbeFile,
                expected_adjacent().join("probe.tmp"),
                Error::new(ErrorKind::AlreadyExists, "collision"),
                1,
            )),
        ]
    };
    for (executable, stem) in &executables {
        for write in write_outcomes() {
            for home in [Some(home.clone()), None] {
                let current_exe_result = match executable {
                    Some(path) => Ok(path.clone()),
                    None => exe_err(),
                };
                let loc = resolve_instance_location(
                    None,
                    None,
                    current_exe_result,
                    home.clone(),
                    write.clone(),
                );
                let context = format!("executable={executable:?} write={write:?} home={home:?}");
                assert_eq!(
                    loc.config_dir,
                    home.as_deref().map(canonical_home),
                    "{context}"
                );
                assert_eq!(loc.instance_base, None, "{context}");
                assert!(loc.startup_error.is_none(), "{context}");
                assert_eq!(loc.local_dir_stem, *stem, "{context}");
            }
        }
    }
}

#[test]
fn issue_1850_overrides_keep_precedence_and_identity_over_canonical_home() {
    let public = if cfg!(windows) {
        r"C:\public config"
    } else {
        "/public config"
    };
    let debug = if cfg!(windows) {
        r"C:\debug\.override"
    } else {
        "/debug/.override"
    };
    let home = PathBuf::from("/home/u");

    let loc = resolve_instance_location(
        Some(public.to_string()),
        Some(debug.to_string()),
        Ok(unsuffixed_executable()),
        Some(home.clone()),
        WriteProbeOutcome::NotRun,
    );
    assert_eq!(loc.config_dir.as_deref(), Some(Path::new(public)));
    assert_eq!(
        loc.instance_base.as_deref(),
        Path::new(public).parent(),
        "absolute public override keeps its parent as the instance base"
    );
    assert_eq!(loc.local_dir_stem, "agentscommander");

    let loc = resolve_instance_location(
        Some(" ".to_string()),
        Some(debug.to_string()),
        Ok(unsuffixed_executable()),
        Some(home.clone()),
        WriteProbeOutcome::NotRun,
    );
    assert_eq!(loc.config_dir.as_deref(), Some(Path::new(debug)));
    assert_eq!(loc.instance_base.as_deref(), Path::new(debug).parent());

    let loc = resolve_instance_location(
        None,
        Some("relative/.override".to_string()),
        Ok(unsuffixed_executable()),
        Some(home.clone()),
        WriteProbeOutcome::NotRun,
    );
    assert_eq!(
        loc.config_dir.as_deref(),
        Some(Path::new("relative/.override"))
    );
    assert_eq!(loc.instance_base, None);

    let loc = resolve_instance_location(
        Some("   ".to_string()),
        Some("\t".to_string()),
        Ok(unsuffixed_executable()),
        Some(home.clone()),
        WriteProbeOutcome::Success,
    );
    assert_eq!(loc.config_dir, Some(canonical_home(&home)));
    assert_eq!(loc.instance_base, None);
    assert!(loc.startup_error.is_none());
}

fn never_write(_: &Path) -> WriteProbeOutcome {
    panic!("write probe must not run on this route");
}

#[test]
fn issue_1850_lazy_helper_never_probes_unsuffixed_or_overridden_routes() {
    let home = PathBuf::from("/home/u");
    let public = if cfg!(windows) {
        r"C:\public config"
    } else {
        "/public config"
    };

    for executable in [
        Ok(unsuffixed_executable()),
        Ok(PathBuf::from("bin/agentscommander")),
        exe_err(),
    ] {
        let loc = resolve_instance_location_with_probes(
            None,
            None,
            executable,
            Some(home.clone()),
            never_write,
        );
        assert_eq!(loc.config_dir, Some(canonical_home(&home)));
        assert_eq!(loc.instance_base, None);
        assert!(loc.startup_error.is_none());
    }

    let loc = resolve_instance_location_with_probes(
        Some(public.to_string()),
        None,
        Ok(absolute_executable()),
        Some(home.clone()),
        never_write,
    );
    assert_eq!(loc.config_dir.as_deref(), Some(Path::new(public)));

    let loc = resolve_instance_location_with_probes(
        Some("  ".to_string()),
        Some(public.to_string()),
        Ok(absolute_executable()),
        Some(home),
        never_write,
    );
    assert_eq!(loc.config_dir.as_deref(), Some(Path::new(public)));
}

#[test]
fn issue_1850_lazy_helper_probes_suffixed_routes_write_once() {
    struct Probes {
        calls: std::cell::RefCell<Vec<PathBuf>>,
    }
    impl Probes {
        fn run(&self, write: WriteProbeOutcome, home: Option<PathBuf>) -> InstanceLocation {
            self.calls.borrow_mut().clear();
            resolve_instance_location_with_probes(
                None,
                None,
                Ok(absolute_executable()),
                home,
                |path: &Path| {
                    self.calls.borrow_mut().push(path.to_path_buf());
                    write.clone()
                },
            )
        }
        fn calls(&self) -> Vec<PathBuf> {
            self.calls.borrow().clone()
        }
    }
    let probes = Probes {
        calls: std::cell::RefCell::new(Vec::new()),
    };
    let home = PathBuf::from("/home/u");
    let write_once = vec![expected_adjacent()];

    let loc = probes.run(WriteProbeOutcome::Success, Some(home.clone()));
    assert_eq!(probes.calls(), write_once);
    assert_eq!(loc.config_dir, Some(expected_adjacent()));
    assert!(loc.startup_error.is_none());

    let conclusive = failed_write(
        RetryPlatform::Other,
        ProbeOperation::CreateConfigurationDirectory,
        expected_adjacent(),
        Error::new(ErrorKind::PermissionDenied, "denied"),
        1,
    );
    let loc = probes.run(
        WriteProbeOutcome::Failed(conclusive.clone()),
        Some(home.clone()),
    );
    assert_eq!(probes.calls(), write_once);
    assert_eq!(loc.config_dir, Some(expected_adjacent()));
    assert_eq!(
        loc.startup_error,
        Some(ConfigStartupError::AdjacentDirectoryUnwritable {
            config_dir: expected_adjacent(),
            reason: conclusive.reason(),
        })
    );

    let indeterminate = failed_write(
        RetryPlatform::Other,
        ProbeOperation::CreateProbeFile,
        expected_adjacent().join("probe.tmp"),
        Error::new(ErrorKind::AlreadyExists, "collision"),
        1,
    );
    let loc = probes.run(WriteProbeOutcome::Failed(indeterminate.clone()), Some(home));
    assert_eq!(probes.calls(), write_once);
    assert_eq!(loc.config_dir, Some(expected_adjacent()));
    assert_eq!(
        loc.startup_error,
        Some(ConfigStartupError::AdjacentSelectionBlocked {
            config_dir: expected_adjacent(),
            reason: indeterminate.reason(),
        })
    );
}
