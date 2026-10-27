use super::*;
use crate::filesystem::projects::entities::copies::LeaseObserver;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("source");
    let source = project.join(".ac/_agent_alpha");
    let destination = temp.path().join("destination/.ac");
    std::fs::create_dir_all(source.join("skills/nested")).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(source.join("Role.md"), b"role bytes\r\n").unwrap();
    std::fs::write(source.join("skills/nested/payload"), b"payload bytes").unwrap();
    (temp, project, source, destination)
}

#[test]
fn issue_3294_source_leases() {
    use MatrixCopyStage::{
        BeforeCleanup, BeforeVerify, Classify, ConfigWritten, Provenance, ProvenanceWritten,
        Reserved, Streaming,
    };
    for failure in [None, Some(Streaming), Some(Provenance), Some(BeforeCleanup)] {
        let (_temp, project, _source, destination) = fixture();
        let observer = LeaseObserver::default();
        let mut stages = Vec::new();
        let mut callback = |stage, _: &Path| {
            stages.push(stage);
            let current_created = matches!(stage, ConfigWritten | Provenance | ProvenanceWritten)
                || (stage == BeforeCleanup && failure != Some(Streaming));
            let expected = if current_created {
                vec![("Source", 0, true), ("Current", 1, stage != BeforeCleanup)]
            } else {
                vec![("Source", 0, true)]
            };
            assert_eq!(
                observer.states(),
                expected,
                "leases at {stage:?}, failure {failure:?}"
            );
            if Some(stage) == failure || (failure == Some(BeforeCleanup) && stage == Provenance) {
                return Err(format!("injected {stage:?}"));
            }
            Ok(())
        };
        let result = copy_agent_matrix_strict_with_hooks(
            &project,
            "alpha",
            &destination,
            "_agent_copy",
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                observer: Some(observer.clone()),
                ..Default::default()
            },
        );
        let expected_stages: &[MatrixCopyStage] = match failure {
            None => &[
                Reserved,
                Classify,
                Streaming,
                BeforeVerify,
                ConfigWritten,
                Provenance,
                ProvenanceWritten,
            ],
            Some(Streaming) => &[Reserved, Classify, Streaming, BeforeCleanup],
            _ => &[
                Reserved,
                Classify,
                Streaming,
                BeforeVerify,
                ConfigWritten,
                Provenance,
                BeforeCleanup,
            ],
        };
        for stage in expected_stages {
            assert!(
                stages.contains(stage),
                "missing {stage:?}, failure {failure:?}"
            );
        }
        let expected_dead = if failure == Some(Streaming) {
            vec![("Source", 0, false)]
        } else {
            vec![("Source", 0, false), ("Current", 1, false)]
        };
        assert_eq!(observer.states(), expected_dead);
        if failure.is_none() {
            cleanup_owned_copy_artifacts(&result.unwrap()).unwrap();
        } else {
            let error = result.unwrap_err();
            assert!(error.contains("injected"));
            if failure == Some(BeforeCleanup) {
                assert!(error.contains("cleanup hook: injected BeforeCleanup"));
            }
            assert!(!destination.join("_agent_copy").exists());
        }
    }
}

#[test]
fn issue_3294_parent_leases() {
    for fail_body in [false, true] {
        let (_temp, project, source, _destination) = fixture();
        let observer = LeaseObserver::default();
        let mut called = false;
        let result =
            disk::with_source_parent(&source, &project.join(".ac"), Some(&observer), || {
                called = true;
                assert_eq!(
                    observer.states(),
                    vec![("Parent", 0, true), ("Ac", 1, true)]
                );
                if fail_body {
                    Err("body error".to_string())
                } else {
                    Ok(())
                }
            });
        assert!(called);
        assert_eq!(result.is_err(), fail_body);
        assert_eq!(
            observer.states(),
            vec![("Parent", 0, false), ("Ac", 1, false)]
        );
    }
    let (_temp, project, source, _destination) = fixture();
    let observer = LeaseObserver::default();
    let mut called = false;
    let result = disk::with_source_parent(
        &source,
        &project.join("missing-ac"),
        Some(&observer),
        || {
            called = true;
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(!called);
    assert_eq!(observer.states(), vec![("Parent", 0, false)]);
}

#[test]
fn issue_3294_copy_no_clobber_positive_control() {
    let (_temp, project, _source, destination) = fixture();
    let root = destination.join("_agent_copy");
    let displaced = destination.join("displaced-role");
    let mut replaced = false;
    let mut changed = false;
    let mut callback = |stage, path: &Path| {
        if stage == MatrixCopyStage::Provenance {
            return Err("force cleanup".into());
        }
        if stage == MatrixCopyStage::BeforeCleanup {
            std::fs::rename(path.join("Role.md"), &displaced).unwrap();
            std::fs::write(path.join("Role.md"), b"external replacement").unwrap();
            replaced = true;
            std::fs::write(path.join("skills/nested/payload"), b"external modification").unwrap();
            changed = true;
        }
        Ok(())
    };
    let error = copy_agent_matrix_strict_with_hooks(
        &project,
        "alpha",
        &destination,
        "_agent_copy",
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(replaced && changed);
    assert!(error.contains("copy cleanup residues"), "{error}");
    assert!(error.contains("copy output replaced"), "{error}");
    assert!(error.contains("copy output modified"), "{error}");
    assert_eq!(
        std::fs::read(root.join("Role.md")).unwrap(),
        b"external replacement"
    );
    assert_eq!(
        std::fs::read(root.join("skills/nested/payload")).unwrap(),
        b"external modification"
    );
    assert_eq!(std::fs::read(&displaced).unwrap(), b"role bytes\r\n");
    for name in ["config.json", "memory", "plans", "inbox", "outbox"] {
        assert!(!root.join(name).exists(), "own output retained: {name}");
    }
}
