use super::*;
// Each fixture owns its temporary directory. Production never scans debris.
fn p22_stages(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut stages: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "tmp") && path.is_file()
        })
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    stages.sort_by(|left, right| left.0.cmp(&right.0));
    stages
}

// A fresh positive-control fixture per call; no state is shared between tests.
fn calibrate_source_native_b(after: &[u8]) -> (usize, usize) {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let parent = crate::path_identity::retain_directory(root.path()).unwrap();
    let target = parent.identity().canonical_path.join("native-target.json");
    let calls = Cell::new(0usize);
    let pre = Cell::new(0usize);
    let post = Cell::new(0usize);
    let stage = std::cell::RefCell::new(None);
    let observe = || {
        let ordinal = calls.get() + 1;
        calls.set(ordinal);
        let stages = p22_stages(root.path());
        if !target.exists() {
            if stages.len() == 1 && stages[0].1 == after {
                pre.set(ordinal);
                *stage.borrow_mut() = Some((
                    stages[0].0.clone(),
                    parent.verify_regular_file(&stages[0].0).unwrap(),
                ));
            }
        } else if post.get() == 0 {
            assert_eq!(std::fs::read(&target).unwrap(), after);
            assert!(stages.is_empty());
            post.set(ordinal);
            let staged = stage.borrow();
            let (path, identity) = staged.as_ref().unwrap();
            assert!(!path.exists());
            assert!(crate::path_identity::same_object(
                identity,
                &parent.verify_regular_file(&target).unwrap()
            ));
        }
        Ok(())
    };
    let published = publish_source_coordination_bytes_with_validation(
        &target,
        &PhysicalState::Absent,
        after,
        &parent,
        &observe,
    );
    assert_eq!(published.result, Ok(()));
    assert!(!published.guard_rejected);
    let landed = parent.verify_regular_file(&target).unwrap();
    assert!(crate::path_identity::same_object(
        published.published_identity.as_ref().unwrap(),
        &landed
    ));
    assert_eq!(std::fs::read(&target).unwrap(), after);
    assert!(p22_stages(root.path()).is_empty());
    assert!(pre.get() > 0);
    assert_eq!(post.get(), pre.get() + 1);
    (pre.get(), post.get())
}

#[test]
fn source_native_b_collision_and_foreign_identical_preserve_target_and_cleanup_owned_stage() {
    let after: &[u8] = br#"{"native":true}"#;
    let (pre_move, post_move) = calibrate_source_native_b(after);
    for foreign in [br#"{"foreign":true}"#.as_slice(), after] {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let sentinel = root.path().join("foreign-sentinel.tmp");
        let parent = crate::path_identity::retain_directory(root.path()).unwrap();
        let target = parent.identity().canonical_path.join("native-target.json");
        let calls = Cell::new(0usize);
        let phases = std::cell::RefCell::new(Vec::new());
        let owned = std::cell::RefCell::new(None);
        let foreign_identity = std::cell::RefCell::new(None);
        let sentinel_identity = std::cell::RefCell::new(None);
        let interfere = || {
            let ordinal = calls.get() + 1;
            calls.set(ordinal);
            if ordinal == pre_move {
                phases.borrow_mut().push("preMove");
                assert!(!target.exists());
                let stages = p22_stages(root.path());
                assert_eq!(stages.len(), 1);
                assert_eq!(stages[0].1, after);
                let identity = parent.verify_regular_file(&stages[0].0).unwrap();
                *owned.borrow_mut() = Some((stages[0].0.clone(), identity.clone()));
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .unwrap();
                file.write_all(foreign).unwrap();
                file.sync_all().unwrap();
                drop(file);
                let foreign_object = parent.verify_regular_file(&target).unwrap();
                assert!(!crate::path_identity::same_object(
                    &identity,
                    &foreign_object
                ));
                *foreign_identity.borrow_mut() = Some(foreign_object);
            } else if ordinal == post_move {
                phases.borrow_mut().push("postMove");
                assert_eq!(std::fs::read(&target).unwrap(), foreign);
                assert_eq!(
                    parent.verify_regular_file(&target).unwrap(),
                    *foreign_identity.borrow().as_ref().unwrap()
                );
                let stage = owned.borrow();
                let (path, identity) = stage.as_ref().unwrap();
                assert_eq!(std::fs::read(path).unwrap(), after);
                assert_eq!(parent.verify_regular_file(path).unwrap(), *identity);
            } else if ordinal == post_move + 1 {
                phases.borrow_mut().push("cleanupAuthority");
                assert!(owned.borrow().as_ref().unwrap().0.exists());
            } else if ordinal == post_move + 2 {
                phases.borrow_mut().push("cleanupOwnedIdentity");
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&sentinel)
                    .unwrap();
                file.write_all(b"foreign sentinel").unwrap();
                file.sync_all().unwrap();
                drop(file);
                *sentinel_identity.borrow_mut() =
                    Some(parent.verify_regular_file(&sentinel).unwrap());
            } else {
                assert!(ordinal < pre_move, "unexpected callback after cleanup");
                assert!(!target.exists());
            }
            Ok(())
        };
        let published = publish_source_coordination_bytes_with_validation(
            &target,
            &PhysicalState::Absent,
            after,
            &parent,
            &interfere,
        );
        assert_eq!(published.result, Err(PreparedPairError::Conflict));
        assert!(!published.guard_rejected);
        assert!(published.published_identity.is_none());
        assert_eq!(
            *phases.borrow(),
            [
                "preMove",
                "postMove",
                "cleanupAuthority",
                "cleanupOwnedIdentity"
            ]
        );
        assert_eq!(calls.get(), post_move + 2);
        assert_eq!(std::fs::read(&target).unwrap(), foreign);
        assert_eq!(
            parent.verify_regular_file(&target).unwrap(),
            *foreign_identity.borrow().as_ref().unwrap()
        );
        assert!(!owned.borrow().as_ref().unwrap().0.exists());
        assert_eq!(
            parent.verify_regular_file(&sentinel).unwrap(),
            *sentinel_identity.borrow().as_ref().unwrap()
        );
        assert_eq!(
            p22_stages(root.path()),
            vec![(sentinel, b"foreign sentinel".to_vec())]
        );
    }
}

#[test]
fn source_native_b_postmove_guard_refusal_preserves_after_without_ownership_or_cleanup() {
    let after: &[u8] = br#"{"native":true}"#;
    let (pre_move, post_move) = calibrate_source_native_b(after);
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let sentinel = root.path().join("foreign-sentinel.tmp");
    let parent = crate::path_identity::retain_directory(root.path()).unwrap();
    let target = parent.identity().canonical_path.join("native-target.json");
    let calls = Cell::new(0usize);
    let phases = std::cell::RefCell::new(Vec::new());
    let owned = std::cell::RefCell::new(None);
    let snapshot = std::cell::RefCell::new(None);
    let refuse = || {
        let ordinal = calls.get() + 1;
        calls.set(ordinal);
        if ordinal == pre_move {
            phases.borrow_mut().push("preMove");
            assert!(!target.exists());
            let stages = p22_stages(root.path());
            assert_eq!(stages.len(), 1);
            assert_eq!(stages[0].1, after);
            *owned.borrow_mut() = Some((
                stages[0].0.clone(),
                parent.verify_regular_file(&stages[0].0).unwrap(),
            ));
        } else if ordinal == post_move {
            phases.borrow_mut().push("postMoveRefusal");
            let landed = parent.verify_regular_file(&target).unwrap();
            let stage = owned.borrow();
            let (path, identity) = stage.as_ref().unwrap();
            assert!(!path.exists());
            assert!(crate::path_identity::same_object(identity, &landed));
            assert_eq!(std::fs::read(&target).unwrap(), after);
            assert!(p22_stages(root.path()).is_empty());
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sentinel)
                .unwrap();
            file.write_all(b"foreign sentinel").unwrap();
            file.sync_all().unwrap();
            drop(file);
            *snapshot.borrow_mut() = Some((
                landed,
                std::fs::read(&target).unwrap(),
                p22_stages(root.path()),
                parent.verify_regular_file(&sentinel).unwrap(),
            ));
            return Err(PreparedPairError::Conflict);
        } else {
            assert!(
                ordinal < pre_move,
                "unexpected callback after authority refusal"
            );
            assert!(!target.exists());
        }
        Ok(())
    };
    let published = publish_source_coordination_bytes_with_validation(
        &target,
        &PhysicalState::Absent,
        after,
        &parent,
        &refuse,
    );
    assert_eq!(published.result, Err(PreparedPairError::Conflict));
    assert!(published.guard_rejected);
    assert!(published.published_identity.is_none());
    assert_eq!(*phases.borrow(), ["preMove", "postMoveRefusal"]);
    assert_eq!(calls.get(), post_move);
    assert_eq!(
        (
            parent.verify_regular_file(&target).unwrap(),
            std::fs::read(&target).unwrap(),
            p22_stages(root.path()),
            parent.verify_regular_file(&sentinel).unwrap()
        ),
        snapshot.into_inner().unwrap()
    );
    assert!(!owned.borrow().as_ref().unwrap().0.exists());
}

#[test]
fn source_native_b_premove_guard_refusal_retains_owned_stage_without_effect() {
    let after: &[u8] = br#"{"native":true}"#;
    let (pre_move, post_move) = calibrate_source_native_b(after);
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let sentinel = root.path().join("foreign-sentinel.tmp");
    let parent = crate::path_identity::retain_directory(root.path()).unwrap();
    let target = parent.identity().canonical_path.join("native-target.json");
    let calls = Cell::new(0usize);
    let phases = std::cell::RefCell::new(Vec::new());
    let snapshot = std::cell::RefCell::new(None);
    let refuse = || {
        let ordinal = calls.get() + 1;
        calls.set(ordinal);
        if ordinal == pre_move {
            phases.borrow_mut().push("preMoveRefusal");
            assert!(!target.exists());
            let stages = p22_stages(root.path());
            assert_eq!(stages.len(), 1);
            assert_eq!(stages[0].1, after);
            let identity = parent.verify_regular_file(&stages[0].0).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sentinel)
                .unwrap();
            file.write_all(b"foreign sentinel").unwrap();
            file.sync_all().unwrap();
            drop(file);
            *snapshot.borrow_mut() = Some((
                stages[0].0.clone(),
                identity,
                p22_stages(root.path()),
                parent.verify_regular_file(&sentinel).unwrap(),
            ));
            return Err(PreparedPairError::Conflict);
        }
        assert!(
            ordinal < pre_move,
            "unexpected callback after authority refusal"
        );
        assert!(!target.exists());
        Ok(())
    };
    let published = publish_source_coordination_bytes_with_validation(
        &target,
        &PhysicalState::Absent,
        after,
        &parent,
        &refuse,
    );
    assert_eq!(published.result, Err(PreparedPairError::Conflict));
    assert!(published.guard_rejected);
    assert!(published.published_identity.is_none());
    assert_eq!(*phases.borrow(), ["preMoveRefusal"]);
    assert_eq!(calls.get(), pre_move);
    assert!(pre_move < post_move);
    assert!(!target.exists());
    let (stage, identity, stages, sentinel_object) = snapshot.into_inner().unwrap();
    assert_eq!(parent.verify_regular_file(&stage).unwrap(), identity);
    assert_eq!(std::fs::read(&stage).unwrap(), after);
    assert_eq!(p22_stages(root.path()), stages);
    assert_eq!(
        parent.verify_regular_file(&sentinel).unwrap(),
        sentinel_object
    );
    // Retained diagnostic stage is NOT cleanup or product GC proof.
    // Production never scans this debris; fixture TempDir Drop is fixture-only.
}

fn p22_filled_stage(target: &Path, bytes: &[u8]) -> PathBuf {
    let guard = PairTargetGuard::new(&|| Ok(()));
    let (stage, mut file) = acquire_fresh_config_stage(target, &guard).unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    file.sync_all().unwrap();
    drop(file);
    stage
}

fn p22_next_writer(mode: usize, target: &Path) {
    match mode {
        0 => {
            update_config_json_object(target, true, |map| {
                map.insert("value".into(), json!(2));
                Ok(())
            })
            .unwrap();
        }
        1 => write_file_atomic(target, b"{\"value\":2}").unwrap(),
        2 => publish_pair_side(target, json!({"value":2}).as_object().unwrap()).unwrap(),
        3 => publish_prepared_pair_image(
            target,
            &read_config_pair_physical(target).unwrap(),
            &PhysicalState::from_bytes(b"{\"value\":2}".to_vec()),
        )
        .unwrap(),
        _ => unreachable!(),
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(target).unwrap()).unwrap(),
        json!({"value":2})
    );
}

#[test]
fn p22_all_four_next_writers_leave_detached_legacy_debris_unchanged() {
    for mode in 0..4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let debris = temp_config_path(&target);
        std::fs::write(&debris, b"detached stale bytes").unwrap();
        std::fs::write(&target, b"{\"value\":1}").unwrap();
        p22_next_writer(mode, &target);
        assert_eq!(std::fs::read(&debris).unwrap(), b"detached stale bytes");
        assert_eq!(
            p22_stages(root.path()),
            vec![(debris, b"detached stale bytes".to_vec())]
        );
    }
}

#[test]
fn p22_all_four_next_writers_preserve_refused_absent_publish_hardlink_alias() {
    for mode in 0..4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let validate = || {
            if target.exists() {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let first = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
        assert_eq!(
            publish_prepared_pair_image_guarded(&target, &PhysicalState::Absent, &first, &guard),
            Err(PreparedPairError::Conflict)
        );
        let stages = p22_stages(root.path());
        assert_eq!(stages.len(), 1);
        let alias = stages[0].0.clone();
        assert_eq!(std::fs::read(&target).unwrap(), stages[0].1);
        p22_next_writer(mode, &target);
        assert_eq!(std::fs::read(&alias).unwrap(), b"{\"value\":1}");
        assert_eq!(p22_stages(root.path()), stages);
    }
}

#[test]
fn p22_atomic_candidate_collisions_regular_directory_hardlink_and_race() {
    for kind in 0..4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let occupied = fresh_config_stage_path(&target, 0);
        let linked = root.path().join("linked-original");
        match kind {
            0 => std::fs::write(&occupied, b"occupied").unwrap(),
            1 => std::fs::create_dir(&occupied).unwrap(),
            2 => {
                std::fs::write(&linked, b"occupied").unwrap();
                std::fs::hard_link(&linked, &occupied).unwrap();
            }
            3 => {}
            _ => unreachable!(),
        }
        let mut calls = 0;
        let mut next = || {
            let value = calls;
            calls += 1;
            if kind == 3 && value == 0 {
                // Competitor creates the candidate before our atomic open.
                std::fs::write(&occupied, b"occupied").unwrap();
            }
            Ok(value)
        };
        let guard = PairTargetGuard::new(&|| Ok(()));
        match acquire_fresh_config_stage_with_sequence(&target, &guard, &mut next) {
            Ok((stage, mut file)) => {
                assert_eq!(stage, fresh_config_stage_path(&target, 1));
                assert_eq!(calls, 2);
                file.write_all(b"owned").unwrap();
                drop(file);
            }
            Err(PreparedPairError::Io(reason)) if kind == 1 => {
                // Some hosts report a directory as a non-AlreadyExists
                // open error; that precise error must surface immediately.
                assert!(reason.contains(&occupied.display().to_string()));
                assert_eq!(calls, 1);
            }
            other => panic!("unexpected collision result: {other:?}"),
        }
        if kind == 1 {
            assert!(occupied.is_dir());
        } else {
            assert_eq!(std::fs::read(&occupied).unwrap(), b"occupied");
        }
        if kind == 2 {
            assert_eq!(std::fs::read(linked).unwrap(), b"occupied");
        }
    }
}

#[test]
fn p22_exact_64_attempt_exhaustion_never_cleans_unacquired_candidates() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("config.json");
    for sequence in 0..MAX_STAGING_ATTEMPTS as u64 {
        std::fs::write(fresh_config_stage_path(&target, sequence), b"occupied").unwrap();
    }
    let original = p22_stages(root.path());
    let mut calls = 0;
    let mut next = || {
        let value = calls;
        calls += 1;
        Ok(value)
    };
    let guard = PairTargetGuard::new(&|| Ok(()));
    let result = publish_prepared_pair_image_with_io(
        &target,
        &PhysicalState::Absent,
        &PhysicalState::from_bytes(b"{}".to_vec()),
        &guard,
        &|_, _| panic!("no cleanup diagnostic without acquisition"),
        &mut next,
        &|_, _| panic!("no write without acquisition"),
    );
    assert!(matches!(result, Err(PreparedPairError::Io(reason)) if reason.contains("64 attempts")));
    assert_eq!(calls, 64);
    assert_eq!(p22_stages(root.path()), original);
    assert!(!target.exists());
}

#[test]
fn p22_sequence_overflow_never_wraps_or_creates_a_candidate() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let sequence = AtomicU64::new(u64::MAX - 1);
    assert_eq!(next_staging_sequence(&sequence), Ok(u64::MAX - 1));
    assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("config.json");
    let guard = PairTargetGuard::new(&|| Ok(()));
    assert!(matches!(
        acquire_fresh_config_stage_with_sequence(&target, &guard, &mut || {
            next_staging_sequence(&sequence)
        }),
        Err(PreparedPairError::Io(_))
    ));
    assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn p22_allocator_refusal_before_each_candidate_is_sticky() {
    for cut in 1..=4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let occupied = fresh_config_stage_path(&target, 0);
        std::fs::write(&occupied, b"occupied").unwrap();
        let calls = Cell::new(0);
        let validate = || {
            calls.set(calls.get() + 1);
            if calls.get() == cut {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let mut sequence = 0;
        let mut next = || {
            let value = sequence;
            sequence += 1;
            Ok(value)
        };
        assert!(matches!(
            acquire_fresh_config_stage_with_sequence(&target, &guard, &mut next),
            Err(PreparedPairError::Conflict)
        ));
        assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
        assert_eq!(calls.get(), cut);
        assert_eq!(
            p22_stages(root.path()),
            vec![(occupied, b"occupied".to_vec())]
        );
    }
}

#[test]
fn p22_fresh_staging_long_paths_all_four_writers() {
    let root = tempfile::tempdir().unwrap();
    let mut directory = root.path().to_path_buf();
    while directory.as_os_str().to_string_lossy().len() <= 300 {
        directory.push("long-path-component-0123456789");
    }
    std::fs::create_dir_all(&directory).unwrap();
    for mode in 0..4 {
        let target = directory.join(format!("config-{mode}.json"));
        std::fs::write(&target, b"{\"value\":1}").unwrap();
        p22_next_writer(mode, &target);
    }
    assert!(p22_stages(&directory).is_empty());
}

#[test]
fn p22_noncollision_acquisition_error_is_immediate_and_precise() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("missing-parent").join("config.json");
    let guard = PairTargetGuard::new(&|| Ok(()));
    let mut calls = 0;
    let mut next = || {
        calls += 1;
        Ok(0)
    };
    let result = publish_prepared_pair_image_with_io(
        &target,
        &PhysicalState::Absent,
        &PhysicalState::from_bytes(b"{}".to_vec()),
        &guard,
        &|_, _| panic!("no cleanup without acquisition"),
        &mut next,
        &|_, _| panic!("no write without acquisition"),
    );
    assert!(matches!(result, Err(PreparedPairError::Io(reason))
        if reason.contains(&fresh_config_stage_path(&target, 0).display().to_string())));
    assert_eq!(calls, 1);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn p22_observed_directory_witness_change_after_acquisition_preserves_stage() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("parent");
    std::fs::create_dir(&directory).unwrap();
    let witness = directory.join("fixture-directory-witness");
    std::fs::write(&witness, b"original witness").unwrap();
    let target = directory.join("config.json");
    std::fs::write(&target, b"{\"value\":0}").unwrap();
    let before = read_config_pair_physical(&target).unwrap();
    let validate = || {
        if std::fs::read(&witness).unwrap() == b"original witness" {
            Ok(())
        } else {
            Err(PreparedPairError::Conflict)
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let mut next = || next_staging_sequence(&STAGING_SEQUENCE);
    let write = |file: &mut std::fs::File, bytes: &[u8]| {
        file.write_all(bytes).unwrap();
        // A fixture marker models an observed directory witness. It is
        // neither production identity proof nor a hostile-host claim.
        std::fs::write(&witness, b"changed witness").unwrap();
        Ok(())
    };
    assert_eq!(
        publish_prepared_pair_image_with_io(
            &target,
            &before,
            &PhysicalState::from_bytes(b"{\"value\":1}".to_vec()),
            &guard,
            &|_, _| panic!("sticky refusal skips cleanup"),
            &mut next,
            &write
        ),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":0}");
    let stages = p22_stages(&directory);
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].1, b"{\"value\":1}");
    assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
    assert_eq!(p22_stages(&directory), stages);
}

use serde_json::json;

const T1: &str = "2026-10-08T01:00:00Z";
const T2: &str = "2026-10-08T02:00:00Z";
const T3: &str = "2026-10-08T03:00:00Z";

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(absent_state: bool) -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
        };
        fixture.write(
            &fixture.d(),
            json!({"decision":0,"cleanup":0,
            "tooling":{"lastAgentMessageAt":T1},"opaque":null}),
        );
        if !absent_state {
            fixture.write(&fixture.s(), json!({"cleanup":0,
                "tooling":{"lastAgentMessageAt":T1,"codingAgents":{"legacy":{"opaque":true}}},"opaque":[1,2]}));
        }
        fixture
    }
    fn d(&self) -> PathBuf {
        self.root.path().join("config.json")
    }
    fn s(&self) -> PathBuf {
        self.root.path().join("config.state.no-git.json")
    }
    fn r(&self) -> PathBuf {
        self.root
            .path()
            .join("config-identity-reservation.state.no-git.json")
    }
    fn write(&self, path: &Path, value: Value) {
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn tuple(&self) -> [PhysicalState; 2] {
        read_pair_tuple(&self.d(), &self.s()).unwrap()
    }
    fn packet(&self) -> PairReservation {
        read_pair_reservation(&self.r()).unwrap().unwrap().0
    }
    fn request(&self) -> PairReservationRequest {
        PairReservationRequest {
            operation_id: "operation-1".into(),
            source_physical_key: "source-key".into(),
            mappings_digest: "a".repeat(64),
            before_protected: protected_config_pair_revision(&self.tuple()).unwrap(),
        }
    }
    fn reserve(
        &self,
        request: &PairReservationRequest,
    ) -> Result<PairReservation, PreparedPairError> {
        let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
            d.remove("cleanup");
            s.insert("cleanup".into(), json!(1));
            let old = d
                .get_mut("tooling")
                .and_then(Value::as_object_mut)
                .unwrap()
                .remove("lastAgentMessageAt");
            let tooling = s
                .entry("tooling")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            if let Some(value) = old {
                tooling.entry("lastAgentMessageAt").or_insert(value);
            }
            Ok(())
        };
        prepare_and_reserve_config_pair(
            &self.d(),
            &self.s(),
            &self.r(),
            request,
            Some(&cleanup),
            |d, s| {
                d.insert("decision".into(), json!(1));
                s.insert("caller".into(), json!(1));
                Ok(())
            },
        )
    }
    fn activity(&self, input: &str) -> Result<bool, String> {
        let landed = Cell::new(false);
        update_config_pair_guarded(
            &self.d(),
            &self.s(),
            &self.r(),
            &[],
            None,
            Some(&|_, _| panic!("pending activity cannot cleanup")),
            &|_| panic!("no ordinary stages"),
            |_, _| panic!("pending activity uses the exact leaf primitive"),
            &|_| panic!("pending activity cannot stamp"),
            Some((input, &landed)),
        )?;
        Ok(landed.get())
    }
    fn interrupted_activity(&self, input: &str, stop: &str) -> Result<bool, PreparedPairError> {
        let _writer = ConfigWriterActive::enter("pair_activity_fixture").unwrap();
        let _guard = lock_local_config_writes();
        let _d = acquire_config_file_write_lock(&self.d(), CONFIG_LOCK_TIMEOUT).unwrap();
        let _s = acquire_config_file_write_lock(&self.s(), CONFIG_LOCK_TIMEOUT).unwrap();
        let _r = acquire_config_file_write_lock(&self.r(), CONFIG_LOCK_TIMEOUT).unwrap();
        let (mut packet, mut physical) = read_pair_reservation(&self.r())?.unwrap();
        land_pair_activity(
            &self.d(),
            &self.s(),
            &self.r(),
            &mut packet,
            &mut physical,
            input,
            &|stage| {
                if stage == stop {
                    Err(PreparedPairError::Io("lost activity response".into()))
                } else {
                    Ok(())
                }
            },
        )
    }
    fn complete_projection(&self) -> Value {
        let packet = self.packet();
        let hashes = packet.protected_revision_digests().unwrap();
        let target = json!({"targetId":packet.physical_target_identity,
            "beforeProtectedRevision":{"kind":"bytes","sha256":hashes[0]},
            "afterProtectedRevision":{"kind":"bytes","sha256":hashes[1]},
            "planDigest":packet.plan_digest});
        let revision = json!({"kind":"bytes","sha256":"b".repeat(64)});
        json!({"schemaVersion":2,"physicalSourceKey":"source-key","operationId":"operation-1",
            "phase":"complete","afterSourceRevision":revision,"packetsPrepared":true,
            "sourcePublished":true,"refsVerified":true,"reloaded":true,"packets":[target],
            "receipt":{"operationId":"operation-1","sourceRevision":revision}})
    }
}

#[test]
fn pair_activity_atomic_packet_floor_and_locks_before_presence() {
    let f = Fixture::new(false);
    let request = f.request();
    let before = f.tuple();
    let packet = prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |d, _| {
        assert!(!f.r().exists(), "no reservation before complete plan");
        for path in [f.d(), f.s(), f.r()] {
            assert!(acquire_config_file_write_lock(&path, Duration::ZERO).is_err());
        }
        assert!(write_file_atomic(&f.d(), b"{}")
            .unwrap_err()
            .contains("Nested config write"));
        d.insert("decision".into(), json!(1));
        Ok(())
    })
    .unwrap();
    assert_eq!(f.tuple(), before);
    assert_eq!(f.packet(), packet);
    assert_eq!(packet.timestamp_floor.as_deref(), Some(T1));
    assert_eq!(packet.plan_images.stages.len(), 5);
    assert!(packet.activity_intent.is_none());
}

#[test]
fn pair_activity_occupied_legacy_stage_allows_packet_without_target_write() {
    let f = Fixture::new(false);
    let before = f.tuple();
    let debris = temp_config_path(&f.r());
    std::fs::create_dir(&debris).unwrap();
    let _packet = f.reserve(&f.request()).unwrap();
    assert!(f.r().is_file());
    assert!(debris.is_dir());
    assert_eq!(f.tuple(), before);
    for path in [f.d(), f.s(), f.r()] {
        drop(acquire_config_file_write_lock(&path, Duration::ZERO).unwrap());
    }
}

#[test]
fn single_editor_pair_activity_same_operation_original_and_incompatible_refusal() {
    let f = Fixture::new(false);
    let request = f.request();
    let original = f.reserve(&request).unwrap();
    let bytes = std::fs::read(f.r()).unwrap();
    assert_eq!(f.reserve(&request).unwrap(), original);
    assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
    assert_eq!(
        verify_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", &original.plan_digest),
        Err(PreparedPairError::Pending)
    );
    let mut foreign = request.clone();
    foreign.operation_id = "operation-2".into();
    assert_eq!(f.reserve(&foreign), Err(PreparedPairError::Conflict));
    let divergent =
        prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |d, _| {
            d.insert("decision".into(), json!(9));
            Ok(())
        });
    assert_eq!(divergent, Err(PreparedPairError::Conflict));
    assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert_eq!(
        verify_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", &original.plan_digest)
            .unwrap(),
        f.tuple()
    );
}

#[test]
fn pair_activity_generic_noop_zero_write_and_history_selection_unknown_rejected() {
    let f = Fixture::new(false);
    f.reserve(&f.request()).unwrap();
    let before = f.tuple();
    let packet = std::fs::read(f.r()).unwrap();
    update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        None,
        Some(&|_, _| panic!("no cleanup")),
        &|_| panic!("no publish"),
        |_, _| Ok(()),
        &|_| panic!("no stamp"),
        None,
    )
    .unwrap();
    for key in ["codingAgents", "configurationRef", "unknown"] {
        let error = update_config_pair_guarded(
            &f.d(),
            &f.s(),
            &f.r(),
            &[],
            None,
            None,
            &|_| {},
            |d, s| {
                if key == "configurationRef" {
                    d.insert(key.into(), json!({"changed":true}));
                } else {
                    s.insert(key.into(), json!({"changed":true}));
                }
                Ok(())
            },
            &|_| panic!("no stamp"),
            None,
        )
        .unwrap_err();
        assert!(error.contains("targetTransitionPending"), "{error}");
    }
    assert_eq!(f.tuple(), before);
    assert_eq!(std::fs::read(f.r()).unwrap(), packet);
    assert_eq!(
        execute_unreserved_config_pair(&f.d(), &f.s(), &f.r(), &f.packet().plan_images),
        Err(PreparedPairError::Pending)
    );
}

#[test]
fn pair_activity_two_edges_and_older_edge_keep_digest_and_maximum() {
    let f = Fixture::new(false);
    let original = f.reserve(&f.request()).unwrap();
    let d = read_config_pair_physical(&f.d()).unwrap();
    assert_eq!(f.activity(T2), Ok(true));
    assert_eq!(f.activity(T3), Ok(true));
    assert_eq!(f.activity(T1), Ok(false));
    let packet = f.packet();
    assert_eq!(packet.timestamp_floor.as_deref(), Some(T3));
    assert_eq!(packet.plan_digest, original.plan_digest);
    assert_eq!(
        packet.protected_revision_digests().unwrap(),
        original.protected_revision_digests().unwrap()
    );
    assert_eq!(read_config_pair_physical(&f.d()).unwrap(), d);
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
}

#[test]
fn pair_activity_every_t0_t4_cut_recovers_with_monotonic_stamp() {
    for absent in [false, true] {
        for cut in 0..=4 {
            let f = Fixture::new(absent);
            f.reserve(&f.request()).unwrap();
            if cut != 0 {
                let result = execute_reserved_config_pair_with_stage(
                    &f.d(),
                    &f.s(),
                    &f.r(),
                    "operation-1",
                    &|stage| {
                        if stage == cut {
                            Err(PreparedPairError::Io("lost progress".into()))
                        } else {
                            Ok(())
                        }
                    },
                );
                assert_eq!(result, Err(PreparedPairError::Io("lost progress".into())));
            }
            assert_eq!(f.activity(T2), Ok(true));
            assert_eq!(f.activity(T3), Ok(true));
            assert_eq!(f.activity(T1), Ok(false));
            // Re-read durable packet: no in-memory plan or progress authority.
            let packet = f.packet();
            assert_eq!(packet.timestamp_floor.as_deref(), Some(T3));
            execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
            assert_eq!(f.packet().recognize(&f.tuple()).unwrap(), 4);
            assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
            assert_eq!(f.tuple()[0].map().unwrap()["decision"], json!(1));
            assert_eq!(f.tuple()[0].map().unwrap()["opaque"], Value::Null);
            if !absent {
                assert_eq!(f.tuple()[1].map().unwrap()["opaque"], json!([1, 2]));
            }
        }
    }
}

#[test]
fn pair_activity_intent_state_floor_crashes_never_report_false_success() {
    for stop in [
        "activity_intent_durable",
        "activity_state_durable",
        "activity_floor_durable",
    ] {
        let f = Fixture::new(false);
        f.reserve(&f.request()).unwrap();
        assert_eq!(
            f.interrupted_activity(T3, stop),
            Err(PreparedPairError::Io("lost activity response".into()))
        );
        let packet = f.packet();
        if stop == "activity_intent_durable" {
            assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T1));
            assert!(packet.activity_intent.is_some());
            // A generic no-op cannot consolidate a pending activity.
            let bytes = std::fs::read(f.r()).unwrap();
            let tuple = f.tuple();
            update_config_pair_guarded(
                &f.d(),
                &f.s(),
                &f.r(),
                &[],
                None,
                None,
                &|_| panic!("no publish"),
                |_, _| Ok(()),
                &|_| panic!("no stamp"),
                None,
            )
            .unwrap();
            assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
            assert_eq!(f.tuple(), tuple);
        } else {
            assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
        }
        assert_eq!(f.activity(T2), Ok(false));
        assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
        assert_eq!(f.packet().timestamp_floor.as_deref(), Some(T3));
        assert!(f.packet().activity_intent.is_none());
    }
}

#[test]
fn pair_activity_absent_overlay_structural_presence_and_legacy_floor() {
    let f = Fixture::new(true);
    f.reserve(&f.request()).unwrap();
    assert_eq!(f.activity(T2), Ok(true));
    let state = f.tuple()[1].map().unwrap();
    assert_eq!(
        state,
        json!({"tooling":{"lastAgentMessageAt":T2}})
            .as_object()
            .unwrap()
            .clone()
    );
    assert!(protected_matches(&PhysicalState::Absent, &f.tuple()[1], true).unwrap());
    let empty = PhysicalState::from_bytes(b"{}".to_vec());
    assert_ne!(
        protected_revision(&f.tuple()[1], true).unwrap(),
        protected_revision(&empty, true).unwrap()
    );
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T2));
    assert_eq!(f.tuple()[0].map().unwrap()["opaque"], Value::Null);
}

#[test]
fn pair_activity_external_unknown_deleted_malformed_or_lower_timestamp_conflicts() {
    for kind in [
        "unknown",
        "deleted",
        "malformed",
        "lower",
        "tooling",
        "stateGone",
    ] {
        let f = Fixture::new(false);
        f.reserve(&f.request()).unwrap();
        f.activity(T3).unwrap();
        let mut map = f.tuple()[1].map().unwrap();
        match kind {
            "unknown" => {
                map.insert("outside".into(), json!(7));
            }
            "deleted" => {
                map.get_mut("tooling")
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("lastAgentMessageAt");
            }
            "malformed" => {
                map.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!("bad");
            }
            "lower" => {
                map.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T1);
            }
            "tooling" => {
                map.insert("tooling".into(), Value::Null);
            }
            _ => {}
        }
        if kind == "stateGone" {
            std::fs::remove_file(f.s()).unwrap();
        } else {
            f.write(&f.s(), Value::Object(map));
        }
        let tuple = f.tuple();
        let packet = std::fs::read(f.r()).unwrap();
        assert!(f.activity(T2).is_err());
        assert_eq!(
            execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1"),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(f.tuple(), tuple);
        assert_eq!(std::fs::read(f.r()).unwrap(), packet);
    }
}

#[test]
fn pair_activity_corrupt_packet_wrong_target_and_before_cas_preserve_evidence() {
    let f = Fixture::new(false);
    let mut wrong = f.request();
    wrong.before_protected[0] = ProtectedRevision::Absent;
    assert_eq!(f.reserve(&wrong), Err(PreparedPairError::Conflict));
    assert!(!f.r().exists());
    f.reserve(&f.request()).unwrap();
    let other = Fixture::new(false);
    std::fs::write(other.r(), std::fs::read(f.r()).unwrap()).unwrap();
    assert!(other.activity(T2).is_err());
    let tuple = f.tuple();
    f.write(&f.r(), json!({"version":1,"operationId":"operation-1"}));
    let bytes = std::fs::read(f.r()).unwrap();
    assert!(f.activity(T2).is_err());
    assert_eq!(f.tuple(), tuple);
    assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
}

#[test]
fn pair_activity_initial_null_parse_error_incompatible_tooling_zero_write() {
    for bytes in [
        b"null".as_slice(),
        b"[]",
        b"not json",
        br#"{"tooling":null}"#,
        br#"{"tooling":{"lastAgentMessageAt":null}}"#,
        br#"{"tooling":{"lastAgentMessageAt":"bad"}}"#,
    ] {
        let f = Fixture::new(false);
        let request = f.request();
        std::fs::write(f.s(), bytes).unwrap();
        let d = std::fs::read(f.d()).unwrap();
        assert!(f.reserve(&request).is_err());
        assert!(!f.r().exists());
        assert_eq!(std::fs::read(f.s()).unwrap(), bytes);
        assert_eq!(std::fs::read(f.d()).unwrap(), d);
    }
}

#[test]
fn pair_activity_collapsed_absent_stages_choose_greatest_and_keep_overlay() {
    let f = Fixture {
        root: tempfile::tempdir().unwrap(),
    };
    let request = f.request();
    let packet =
        prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |_, _| Ok(()))
            .unwrap();
    assert_eq!(packet.recognize(&f.tuple()).unwrap(), 4);
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert!(!f.d().exists());
    assert!(!f.s().exists());
    assert_eq!(f.activity(T2), Ok(true));
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert!(!f.d().exists());
    assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T2));
    assert_eq!(f.packet().recognize(&f.tuple()).unwrap(), 4);
}

#[test]
fn pair_activity_intent_unknown_timestamp_and_exact_generic_delta() {
    let f = Fixture::new(false);
    f.reserve(&f.request()).unwrap();
    let tuple = f.tuple();
    update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        None,
        None,
        &|_| panic!("no cleanup stages"),
        |_, s| {
            s.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T2);
            Ok(())
        },
        &|_| panic!("no split stamp"),
        None,
    )
    .unwrap();
    assert_eq!(f.tuple()[0], tuple[0]);
    assert_eq!(f.packet().timestamp_floor.as_deref(), Some(T2));
    let before = f.tuple();
    let packet = std::fs::read(f.r()).unwrap();
    let error = update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        None,
        None,
        &|_| {},
        |_, s| {
            s.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T1);
            Ok(())
        },
        &|_| panic!("no stamp"),
        None,
    )
    .unwrap_err();
    assert!(error.contains("targetTransitionPending"));
    assert_eq!(f.tuple(), before);
    assert_eq!(std::fs::read(f.r()).unwrap(), packet);
    assert!(f
        .interrupted_activity(T3, "activity_intent_durable")
        .is_err());
    let mut state = f.tuple()[1].map().unwrap();
    state.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!("2026-10-08T02:30:00Z");
    f.write(&f.s(), Value::Object(state));
    let tuple = f.tuple();
    let packet = std::fs::read(f.r()).unwrap();
    assert!(f.activity(T3).is_err());
    assert_eq!(f.tuple(), tuple);
    assert_eq!(std::fs::read(f.r()).unwrap(), packet);
}

#[test]
fn pair_activity_preflight_precedes_state_sidecars_and_holds_decisions_lease() {
    let f = Fixture::new(false);
    let parent = f.root.path().canonicalize().unwrap();
    let state_lock = config_lock_path(&parent, &f.s()).unwrap();
    let reservation_lock = config_lock_path(&parent, &f.r()).unwrap();
    let called = Cell::new(false);
    let preflight = || {
        assert!(!state_lock.exists());
        assert!(!reservation_lock.exists());
        assert!(acquire_config_file_write_lock(&f.d(), Duration::from_millis(20)).is_err());
        called.set(true);
        Err("ignore preflight refused".to_string())
    };
    let before = f.tuple();
    let error = update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        Some(&preflight),
        None,
        &|_| panic!("no publish"),
        |_, _| panic!("no mutation"),
        &|_| Ok(()),
        None,
    )
    .unwrap_err();
    assert_eq!(error, "ignore preflight refused");
    assert!(called.get());
    assert_eq!(f.tuple(), before);
    assert!(!f.r().exists());
    assert!(!state_lock.exists());
    assert!(!reservation_lock.exists());
    std::fs::write(f.r(), b"{invalid").unwrap();
    called.set(false);
    assert!(update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        Some(&preflight),
        None,
        &|_| panic!("no publish"),
        |_, _| panic!("no mutation"),
        &|_| Ok(()),
        None
    )
    .is_err());
    assert!(!called.get());
    assert_eq!(std::fs::read(f.r()).unwrap(), b"{invalid");
    assert!(!state_lock.exists());
    assert!(!reservation_lock.exists());
}

#[test]
fn single_editor_pair_activity_release_requires_local_receipt_source_and_final_target() {
    let f = Fixture::new(false);
    f.reserve(&f.request()).unwrap();
    let target = f.packet().physical_target_identity;
    let ledger = f.complete_projection();
    let revision = &ledger["afterSourceRevision"];
    let proof =
        PairCompleteProof::from_single_editor_complete(&ledger, "operation-1", &target, revision)
            .unwrap();
    assert_eq!(
        release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof),
        Err(PreparedPairError::Conflict)
    );
    assert!(PairCompleteProof::from_single_editor_complete(
        &ledger,
        "operation-1",
        &target,
        &json!({"kind":"absent"})
    )
    .is_err());
    for key in ["phase", "sourcePublished", "receipt"] {
        let mut invalid = ledger.clone();
        invalid[key] = Value::Null;
        assert!(PairCompleteProof::from_single_editor_complete(
            &invalid,
            "operation-1",
            &target,
            revision
        )
        .is_err());
    }
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    f.activity(T3).unwrap();
    let before = f.tuple();
    assert_eq!(
        release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof).unwrap(),
        before
    );
    assert!(!f.r().exists());
    assert!(config_lock_path(f.root.path(), &f.r()).unwrap().exists());
    assert_eq!(
        release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof).unwrap(),
        before
    );
    assert_eq!(timestamp_value(&before[1]).unwrap().as_deref(), Some(T3));
}
// P22: deterministic failure at each reached validation boundary.
fn p22_prepare(
    fixture: &Fixture,
    request: &PairReservationRequest,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PairReservation, PreparedPairError> {
    let cleanup = |decisions: &mut Map<String, Value>, state: &mut Map<String, Value>| {
        decisions.insert("cleanup".into(), json!(1));
        state.insert("cleanup".into(), json!(1));
        Ok(())
    };
    prepare_and_reserve_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        request,
        Some(&cleanup),
        |decisions, state| {
            decisions.insert("decision".into(), json!(2));
            state.insert("caller".into(), json!(2));
            Ok(())
        },
        validate,
    )
}

type P22Artifacts = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<(String, Vec<u8>)>);

fn p22_artifacts(fixture: &Fixture) -> P22Artifacts {
    let mut stages = std::fs::read_dir(fixture.root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "tmp"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    stages.sort();
    (
        std::fs::read(fixture.d()).unwrap(),
        std::fs::read(fixture.s()).unwrap(),
        std::fs::read(fixture.r()).ok(),
        stages,
    )
}

fn p22_case(mode: usize, stop: Option<usize>) -> (usize, Result<(), PreparedPairError>) {
    let fixture = Fixture::new(false);
    let original_request = fixture.request();
    let mut proof = None;
    if mode != 0 {
        p22_prepare(&fixture, &original_request, &|| Ok(())).unwrap();
    }
    if [2, 4, 9, 10].contains(&mode) {
        assert!(fixture
            .interrupted_activity(T2, "activity_intent_durable")
            .is_err());
    }
    if mode == 5 {
        let mut state: Value =
            serde_json::from_slice(&std::fs::read(fixture.s()).unwrap()).unwrap();
        state["tooling"]["lastAgentMessageAt"] = json!(T3);
        fixture.write(&fixture.s(), state);
    }
    if [6, 7, 8].contains(&mode) {
        execute_reserved_config_pair(&fixture.d(), &fixture.s(), &fixture.r(), "operation-1")
            .unwrap();
        if mode == 7 {
            assert!(fixture
                .interrupted_activity(T3, "activity_intent_durable")
                .is_err());
        }
        proof = Some(
            PairCompleteProof::from_single_editor_complete(
                &fixture.complete_projection(),
                "operation-1",
                &fixture.packet().physical_target_identity,
                &json!({"kind":"bytes", "sha256":"b".repeat(64)}),
            )
            .unwrap(),
        );
        if mode == 8 {
            release_reserved_config_pair(
                &fixture.d(),
                &fixture.s(),
                &fixture.r(),
                proof.as_ref().unwrap(),
            )
            .unwrap();
        }
    }
    let calls = Cell::new(0);
    let at_failure = std::cell::RefCell::new(None);
    let validate = || {
        calls.set(calls.get() + 1);
        if stop == Some(calls.get()) {
            *at_failure.borrow_mut() = Some(p22_artifacts(&fixture));
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let result = match mode {
        0..=3 | 10 => p22_prepare(&fixture, &original_request, &validate).map(|_| ()),
        4..=5 => execute_reserved_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            "operation-1",
            &validate,
        ),
        6 | 9 => verify_reserved_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            "operation-1",
            &fixture.packet().plan_digest,
            &validate,
        )
        .map(|_| ()),
        7..=8 => release_reserved_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            proof.as_ref().unwrap(),
            &validate,
        )
        .map(|_| ()),
        _ => unreachable!(),
    };
    if let Some(expected) = at_failure.borrow().as_ref() {
        assert_eq!(
            &p22_artifacts(&fixture),
            expected,
            "no target, packet or staging mutation after failed validation"
        );
        assert_eq!(result, Err(PreparedPairError::Conflict));
        assert_eq!(Some(calls.get()), stop, "failed guard remains failed");
    }
    (calls.get(), result)
}

#[test]
fn single_editor_pair_activity_every_reached_reservation_execution_verify_release_cut() {
    let mut total_cuts = 0;
    for mode in 0..11 {
        let (count, result) = p22_case(mode, None);
        if mode == 9 {
            assert_eq!(result, Err(PreparedPairError::Pending));
        } else {
            result.unwrap();
        }
        assert!(count > 0);
        for cut in 1..=count {
            assert_eq!(
                p22_case(mode, Some(cut)).1,
                Err(PreparedPairError::Conflict)
            );
            total_cuts += 1;
        }
    }
    assert!(
        total_cuts >= 11,
        "all eleven independently prepared paths ran"
    );
}

#[test]
fn p22_validation_after_mutation_before_first_packet_preserves_original_tuple() {
    let fixture = Fixture::new(false);
    let request = fixture.request();
    let before = fixture.tuple();
    let mutated = Cell::new(false);
    let result = prepare_and_reserve_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        &request,
        None,
        |decisions, _| {
            decisions.insert("decision".into(), json!(2));
            mutated.set(true);
            Ok(())
        },
        &|| {
            if mutated.get() {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(result, Err(PreparedPairError::Conflict));
    assert_eq!(fixture.tuple(), before);
    assert!(!fixture.r().exists());
}

// Insert inside pair_activity_tests; existing source_guard cut fixture is retained.
#[test]
fn source_guard_postpublish_failure_preserves_exact_after_without_false_rollback() {
    for absent in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("source-guard-postpublish.json");
        if !absent {
            std::fs::write(&target, b"{\"value\":0}").unwrap();
        }
        let before = read_config_pair_physical(&target).unwrap();
        let after = b"{\"value\":1}";
        let rejected = Cell::new(false);
        let evidence = std::cell::RefCell::new(None);
        let result = publish_coordination_bytes_with_validation(&target, &before, after, &|| {
            if std::fs::read(&target).ok().as_deref() == Some(after.as_slice()) {
                rejected.set(true);
                *evidence.borrow_mut() =
                    Some((std::fs::read(&target).unwrap(), p22_stages(root.path())));
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(PreparedPairError::Conflict));
        assert!(
            rejected.get(),
            "must reach a validation boundary after publication"
        );
        assert_eq!(
            (std::fs::read(&target).unwrap(), p22_stages(root.path())),
            evidence.borrow_mut().take().unwrap()
        );
        assert_eq!(std::fs::read(&target).unwrap(), after);
        // This fixture proves after-evidence retention at the adapter.
        // The driver must separately classify pending/conflict and forbid
        // complete until durable refs/reload/acks/receipt all succeed.
    }
}

#[test]
fn source_guard_callback_adapter_noop_absent_replace_and_every_failure_cut() {
    for mode in 0..3 {
        let setup = |root: &Path| {
            let target = root.join("source-guard.json");
            if mode != 1 {
                std::fs::write(&target, b"{\"value\":0}").unwrap();
            }
            let before = read_config_pair_physical(&target).unwrap();
            let after = if mode == 0 {
                b"{\"value\":0}".as_slice()
            } else {
                b"{\"value\":1}".as_slice()
            };
            (target, before, after)
        };
        let root = tempfile::tempdir().unwrap();
        let (target, before, after) = setup(root.path());
        let calls = Cell::new(0);
        publish_coordination_bytes_with_validation(&target, &before, after, &|| {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
        assert!(calls.get() > 0);
        assert_eq!(std::fs::read(&target).unwrap(), after);
        assert!(p22_stages(root.path()).is_empty());
        for cut in 1..=calls.get() {
            let root = tempfile::tempdir().unwrap();
            let (target, before, after) = setup(root.path());
            let reached = Cell::new(0);
            let observed = std::cell::RefCell::new(None);
            let result =
                publish_coordination_bytes_with_validation(&target, &before, after, &|| {
                    reached.set(reached.get() + 1);
                    if reached.get() == cut {
                        *observed.borrow_mut() =
                            Some((std::fs::read(&target).ok(), p22_stages(root.path())));
                        Err(PreparedPairError::Conflict)
                    } else {
                        Ok(())
                    }
                });
            assert_eq!(result, Err(PreparedPairError::Conflict));
            assert_eq!(reached.get(), cut);
            assert_eq!(
                (std::fs::read(&target).ok(), p22_stages(root.path())),
                observed.borrow_mut().take().unwrap(),
                "a rejected callback must forbid later publication and cleanup",
            );
        }
    }
}

#[test]
fn p22_publisher_failure_cuts_preserve_staging_and_no_false_rollback() {
    for absent in [false, true] {
        let setup = |root: &Path| {
            let target = root.join("pair-side.json");
            if !absent {
                std::fs::write(&target, b"{\"value\":0}").unwrap();
            }
            let before = read_config_pair_physical(&target).unwrap();
            (target, before)
        };
        let root = tempfile::tempdir().unwrap();
        let (target, before) = setup(root.path());
        let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
        let count = Cell::new(0);
        let allow = || {
            count.set(count.get() + 1);
            Ok(())
        };
        publish_prepared_pair_image_guarded(
            &target,
            &before,
            &after,
            &PairTargetGuard::new(&allow),
        )
        .unwrap();
        for cut in 1..=count.get() {
            let root = tempfile::tempdir().unwrap();
            let (target, before) = setup(root.path());
            let calls = Cell::new(0);
            let observed = std::cell::RefCell::new(None);
            let reject = || {
                calls.set(calls.get() + 1);
                if calls.get() == cut {
                    *observed.borrow_mut() =
                        Some((std::fs::read(&target).ok(), p22_stages(root.path())));
                    Err(PreparedPairError::Conflict)
                } else {
                    Ok(())
                }
            };
            let guard = PairTargetGuard::new(&reject);
            assert_eq!(
                publish_prepared_pair_image_guarded(&target, &before, &after, &guard),
                Err(PreparedPairError::Conflict)
            );
            assert_eq!(
                (std::fs::read(&target).ok(), p22_stages(root.path())),
                observed.borrow_mut().take().unwrap()
            );
            assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
            assert_eq!(calls.get(), cut);
        }
    }
}

#[test]
fn p22_cleanup_rejection_retains_diagnostic_stage_and_observed_drift() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    std::fs::write(&target, b"{\"value\":0}").unwrap();
    let before = read_config_pair_physical(&target).unwrap();
    let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
    let drifted = Cell::new(false);
    let validate = || {
        if drifted.get() {
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let mut next = || next_staging_sequence(&STAGING_SEQUENCE);
    let write = |file: &mut std::fs::File, bytes: &[u8]| {
        file.write_all(bytes).unwrap();
        std::fs::write(&target, b"{\"value\":2}").unwrap();
        drifted.set(true);
        Ok(())
    };
    assert_eq!(
        publish_prepared_pair_image_with_io(
            &target,
            &before,
            &after,
            &guard,
            &|_, _| panic!("validation failure is already sticky"),
            &mut next,
            &write
        ),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":2}");
    assert_eq!(p22_stages(root.path()).len(), 1);
    assert_eq!(p22_stages(root.path())[0].1, b"{\"value\":1}");
}

#[test]
fn p22_allowing_guard_ordinary_adapter_packet_and_floor_parity() {
    // Same physical target avoids comparing path-dependent packet identities.
    let fixture = Fixture::new(false);
    let request = fixture.request();
    let first = p22_prepare(&fixture, &request, &|| Ok(())).unwrap();
    let observed = p22_prepare(&fixture, &request, &|| Ok(())).unwrap();
    assert_eq!(first, observed);
    execute_reserved_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        "operation-1",
        &|| Ok(()),
    )
    .unwrap();
    let guarded_after = fixture.tuple();
    execute_reserved_config_pair(&fixture.d(), &fixture.s(), &fixture.r(), "operation-1").unwrap();
    assert_eq!(fixture.tuple(), guarded_after);
    assert_eq!(fixture.packet().timestamp_floor.as_deref(), Some(T1));
}

#[test]
fn p22_ordinary_and_guarded_reservation_same_packet_and_tuples() {
    let fixture = Fixture::new(false);
    let request = fixture.request();
    let ordinary = prepare_and_reserve_config_pair(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        &request,
        None,
        |decisions, _| {
            decisions.insert("decision".into(), json!(1));
            Ok(())
        },
    )
    .unwrap();
    let packet_bytes = std::fs::read(fixture.r()).unwrap();
    let before = fixture.tuple();
    let guarded = prepare_and_reserve_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        &request,
        None,
        |decisions, _| {
            decisions.insert("decision".into(), json!(1));
            Ok(())
        },
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(ordinary, guarded);
    assert_eq!(std::fs::read(fixture.r()).unwrap(), packet_bytes);
    assert_eq!(fixture.tuple(), before);
}

#[test]
fn p22_primary_io_diagnostic_and_cleanup_refusal_both_retained() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    std::fs::write(&target, b"{\"value\":0}").unwrap();
    let before = read_config_pair_physical(&target).unwrap();
    let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
    let io_failed = Cell::new(false);
    let validations = Cell::new(0);
    let validate = || {
        validations.set(validations.get() + 1);
        if io_failed.get() {
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let diagnostics = std::cell::RefCell::new(Vec::new());
    let owned = std::cell::RefCell::new(None);
    let mut next = || next_staging_sequence(&STAGING_SEQUENCE);
    let write = |_: &mut std::fs::File, _: &[u8]| {
        let stages = p22_stages(root.path());
        assert_eq!(stages.len(), 1);
        *owned.borrow_mut() = Some(stages[0].0.clone());
        io_failed.set(true);
        Err(PreparedPairError::Io(
            "injected write failure after exclusive acquisition".into(),
        ))
    };
    let report = |primary: &PreparedPairError, refusal: &PreparedPairError| {
        assert!(matches!(primary, PreparedPairError::Io(_)));
        assert_eq!(refusal, &PreparedPairError::Conflict);
        diagnostics
            .borrow_mut()
            .push((primary.to_string(), refusal.to_string()));
    };
    assert_eq!(
        publish_prepared_pair_image_with_io(
            &target, &before, &after, &guard, &report, &mut next, &write
        ),
        Err(PreparedPairError::Conflict)
    );
    let calls = validations.get();
    assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
    assert_eq!(validations.get(), calls);
    let observed = diagnostics.into_inner();
    assert_eq!(observed.len(), 1);
    assert!(observed[0].0.starts_with("preparedPairIo:"));
    assert!(observed[0].1.starts_with("preparedPairConflict:"));
    assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":0}");
    assert_eq!(std::fs::read(owned.into_inner().unwrap()).unwrap(), b"");
}

#[cfg(not(windows))]
fn p22_os_publish_attempt(stage: &Path, target: &Path) -> Result<(), std::io::Error> {
    std::fs::rename(stage, target)
}

#[cfg(windows)]
fn p22_os_publish_attempt(stage: &Path, target: &Path) -> Result<(), std::io::Error> {
    replace_temp_config_once(&publish_path_wide(target), &publish_path_wide(stage))
}

#[test]
fn p22_transient_error_then_guard_refusal_has_no_second_os_attempt() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    std::fs::write(&target, b"before").unwrap();
    let stage = p22_filled_stage(&target, b"after");
    let attempts = Cell::new(0);
    let validations = Cell::new(0);
    let delays = std::cell::RefCell::new(Vec::new());
    let validate = || {
        validations.set(validations.get() + 1);
        if attempts.get() > 0 {
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let mut attempt = || {
        attempts.set(attempts.get() + 1);
        if attempts.get() == 1 {
            Err(std::io::Error::from_raw_os_error(
                TRANSIENT_PUBLISH_OS_ERRORS[1],
            ))
        } else {
            p22_os_publish_attempt(&stage, &target)
        }
    };
    let backoff = |duration| delays.borrow_mut().push(duration);
    assert_eq!(
        publish_temp_config_retry_with_attempt(&stage, &target, &guard, &mut attempt, &backoff,),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(attempts.get(), 1, "second OS primitive forbidden");
    assert_eq!(validations.get(), 3);
    assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
    assert_eq!(validations.get(), 3);
    assert_eq!(
        delays.into_inner(),
        vec![std::time::Duration::from_millis(PUBLISH_BACKOFFS_MS[0])]
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"before");
    assert_eq!(std::fs::read(&stage).unwrap(), b"after");
}

#[test]
fn p22_accepting_retry_and_ordinary_adapter_have_exact_success_parity() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    for ordinary in [false, true] {
        std::fs::write(&target, b"before").unwrap();
        let stage = p22_filled_stage(&target, b"after");
        let attempts = Cell::new(0);
        let delays = std::cell::RefCell::new(Vec::new());
        let mut attempt = || {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err(std::io::Error::from_raw_os_error(
                    TRANSIENT_PUBLISH_OS_ERRORS[1],
                ))
            } else {
                p22_os_publish_attempt(&stage, &target)
            }
        };
        let backoff = |duration| delays.borrow_mut().push(duration);
        if ordinary {
            assert_eq!(
                publish_temp_config_with_attempt(&stage, &target, &mut attempt, &backoff,),
                Ok(())
            );
        } else {
            assert_eq!(
                publish_temp_config_retry_with_attempt(
                    &stage,
                    &target,
                    &PairTargetGuard::new(&|| Ok(())),
                    &mut attempt,
                    &backoff,
                ),
                Ok(())
            );
        }
        assert_eq!(attempts.get(), 2);
        assert_eq!(
            delays.into_inner(),
            vec![std::time::Duration::from_millis(PUBLISH_BACKOFFS_MS[0])]
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"after");
        assert!(!stage.exists());
    }
    // Ordinary production adapter still chooses the same host OS primitive.
    std::fs::write(&target, b"before").unwrap();
    let stage = p22_filled_stage(&target, b"after");
    assert_eq!(publish_temp_config(&stage, &target), Ok(()));
    assert_eq!(std::fs::read(&target).unwrap(), b"after");
    assert!(!stage.exists());
}
