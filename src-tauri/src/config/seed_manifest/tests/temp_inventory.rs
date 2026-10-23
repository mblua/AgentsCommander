use super::*;

#[test]
fn v1_coverage_declaration_is_exhaustive() {
    // The declaration must name every coverage boundary exactly
    // once. The exhaustive match makes adding a `V1CoverageBoundary` variant
    // without listing it here (or vice versa) a compile/count failure, so the
    // Stage F coverage checklist cannot silently drop a boundary.
    assert_eq!(V1_COVERAGE_BOUNDARIES.len(), 17);
    for boundary in V1_COVERAGE_BOUNDARIES {
        // Exhaustive match: a new variant forces an update here.
        match boundary {
            V1CoverageBoundary::DirectCreateAcProjectFreshRoot
            | V1CoverageBoundary::ContextCreate
            | V1CoverageBoundary::ContextUpdate
            | V1CoverageBoundary::ContextSelfHeal
            | V1CoverageBoundary::ContextOverwrite
            | V1CoverageBoundary::CoordinatorStatelessV2ToV4
            | V1CoverageBoundary::CoordinatorStatelessV3ToV4
            | V1CoverageBoundary::CoordinatorSeededV3ToV4
            | V1CoverageBoundary::ConfigExactPublish
            | V1CoverageBoundary::ConfigOverBoundPublish
            | V1CoverageBoundary::ConfigFailedRestore
            | V1CoverageBoundary::LifecycleReplicaRemoval
            | V1CoverageBoundary::LifecycleWorkgroupRemoval
            | V1CoverageBoundary::LifecycleTeamDeletion
            | V1CoverageBoundary::LifecycleAgentMatrixDeletion
            | V1CoverageBoundary::PendingInclusiveDeleteProtection
            | V1CoverageBoundary::CatalogSeed => {}
        }
    }
    // No duplicates: every declared boundary is distinct.
    for (index, boundary) in V1_COVERAGE_BOUNDARIES.iter().enumerate() {
        assert!(
            !V1_COVERAGE_BOUNDARIES[index + 1..].contains(boundary),
            "duplicate coverage boundary {boundary:?}"
        );
    }
}

#[test]
fn exact_temp_name_requires_lowercase_hyphenated_uuid() {
    assert!(is_exact_temp_name(
        ".seed-manifest.00000000-0000-0000-0000-000000000000.tmp"
    ));
    assert!(!is_exact_temp_name(
        ".seed-manifest.00000000000000000000000000000000.tmp"
    ));
    assert!(!is_exact_temp_name(
        ".seed-manifest.00000000-0000-0000-0000-00000000000A.tmp"
    ));
    assert!(!is_exact_temp_name(".seed-manifest.not-a-uuid.tmp"));
}

#[test]
fn prior_temp_inventory_is_hard_bounded_and_preserves_uninspected_entries() {
    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    let candidate_count = MAX_TEMP_INVENTORY_ENTRIES + 5;
    for index in 0..candidate_count {
        let name = format!(
            "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
            Uuid::from_u128(u128::try_from(index + 1).unwrap()).hyphenated()
        );
        std::fs::write(ac.join(name), b"candidate").unwrap();
    }
    for index in 0..(MAX_TEMP_DIAGNOSTIC_SAMPLES + 5) {
        std::fs::write(
            ac.join(format!(".seed-manifest.malformed-{index}.tmp")),
            b"lookalike",
        )
        .unwrap();
    }

    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let inventory = guard.inventory_prior_temps();
    assert!(inventory.scan_truncated);
    assert!(inventory.exact_count > 0);
    assert!(
        inventory
            .exact_count
            .saturating_add(inventory.malformed_count)
            <= u64::try_from(MAX_TEMP_INVENTORY_ENTRIES).unwrap()
    );
    assert!(inventory.exact_names.len() <= MAX_TEMP_INVENTORY_ENTRIES);
    assert!(inventory.exact_samples.len() <= MAX_TEMP_DIAGNOSTIC_SAMPLES);
    assert!(inventory.malformed_samples.len() <= MAX_TEMP_DIAGNOSTIC_SAMPLES);
    guard.release();
    assert_eq!(exact_temp_paths(&project).len(), candidate_count);
}

#[test]
fn prior_temp_inventory_counts_and_samples_malformed_names_exactly() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    for index in 0..3_u128 {
        let name = format!(
            "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
            Uuid::from_u128(index + 1).hyphenated()
        );
        std::fs::write(ac.join(name), b"candidate").unwrap();
    }
    let malformed_count = MAX_TEMP_DIAGNOSTIC_SAMPLES + 5;
    for index in 0..malformed_count {
        std::fs::write(
            ac.join(format!(".seed-manifest.malformed-{index}.tmp")),
            b"lookalike",
        )
        .unwrap();
    }

    let diagnostic_count = Arc::new(AtomicUsize::new(0));
    let hooks = TestFilesystemHooks {
        on_prior_temp_diagnostic: Some(Arc::new({
            let diagnostic_count = Arc::clone(&diagnostic_count);
            move |inventory| {
                diagnostic_count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(inventory.exact_count, 3);
                assert_eq!(
                    inventory.malformed_count,
                    u64::try_from(malformed_count).unwrap()
                );
                assert_eq!(inventory.exact_samples.len(), 3);
                assert_eq!(
                    inventory.malformed_samples.len(),
                    MAX_TEMP_DIAGNOSTIC_SAMPLES
                );
            }
        })),
        ..TestFilesystemHooks::default()
    };
    let guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    assert_eq!(diagnostic_count.load(Ordering::SeqCst), 1);

    let inventory = guard.inventory_prior_temps();
    assert!(!inventory.scan_truncated);
    assert!(inventory.scan_error.is_none());
    assert_eq!(inventory.entry_errors, 0);
    assert_eq!(inventory.exact_count, 3);
    assert_eq!(
        inventory.malformed_count,
        u64::try_from(malformed_count).unwrap()
    );
    assert_eq!(inventory.malformed_native_count, 0);
    assert_eq!(inventory.exact_names.len(), 3);
    assert_eq!(inventory.exact_samples.len(), 3);
    assert_eq!(
        inventory.malformed_samples.len(),
        MAX_TEMP_DIAGNOSTIC_SAMPLES
    );
    assert!(inventory
        .malformed_samples
        .iter()
        .all(|sample| sample.starts_with(SEED_MANIFEST_TEMP_PREFIX)
            && sample.ends_with(SEED_MANIFEST_TEMP_SUFFIX)));
    guard.release();
}

#[cfg(any(unix, windows))]
#[test]
fn native_non_unicode_temp_shape_is_counted_without_lossy_materialization() {
    let (_temp, project) = setup_project();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();

    #[cfg(unix)]
    let native_name = {
        use std::os::unix::ffi::OsStringExt;

        let mut bytes = SEED_MANIFEST_TEMP_PREFIX.as_bytes().to_vec();
        bytes.push(0xff);
        bytes.extend_from_slice(SEED_MANIFEST_TEMP_SUFFIX.as_bytes());
        std::ffi::OsString::from_vec(bytes)
    };
    #[cfg(windows)]
    let native_name = {
        use std::os::windows::ffi::OsStringExt;

        let mut units = SEED_MANIFEST_TEMP_PREFIX.encode_utf16().collect::<Vec<_>>();
        units.push(0xd800);
        units.extend(SEED_MANIFEST_TEMP_SUFFIX.encode_utf16());
        std::ffi::OsString::from_wide(&units)
    };

    assert!(native_name.to_str().is_none());
    std::fs::write(project.join(".ac").join(&native_name), b"lookalike").unwrap();
    let inventory = guard.inventory_prior_temps();
    assert!(!inventory.scan_truncated);
    assert_eq!(inventory.malformed_count, 1);
    assert_eq!(inventory.malformed_native_count, 1);
    assert_eq!(
        inventory.malformed_samples,
        vec!["<native-non-unicode-temp-name>".to_string()]
    );
    assert!(!inventory
        .malformed_samples
        .iter()
        .any(|sample| sample.contains('\u{fffd}')));
    guard.release();
}
