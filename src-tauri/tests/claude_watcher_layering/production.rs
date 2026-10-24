use super::*;

fn production_index() -> ModuleIndex {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    build_module_index(&manifest, &manifest.join("src/lib.rs"), CRATE_ID)
        .expect("production module index should build")
}

#[test]
fn production_moved_symbols_and_all_seven_methods_exist_once_at_the_destination() {
    let index = production_index();
    verify_exact_moved_symbols(&index, CRATE_ID)
        .unwrap_or_else(|error| panic!("moved-symbol proof failed: {error}"));
}

#[test]
fn production_interface_is_exactly_telegram_internal_and_canonical() {
    let index = production_index();
    verify_exact_interface(&index, CRATE_ID)
        .unwrap_or_else(|error| panic!("exact interface proof failed: {error}"));
}

#[test]
fn production_output_module_owns_the_seam_alone() {
    let index = production_index();
    let report = analyze_module_index(&index, CRATE_ID, OUTPUT_MODULE)
        .unwrap_or_else(|error| panic!("output guard analysis failed:\n{error}"));
    require_exact_output_report(&report)
        .unwrap_or_else(|error| panic!("output guard failed:\n{error}"));
}

#[test]
fn production_claude_and_codex_reach_output_without_bridge() {
    let index = production_index();
    // Both watchers keep their `use super::*;` inside a `#[cfg(test)] mod`, which
    // `index_body` skips, so no glob reaches `build_scope`. Lifting one to the top
    // level would abort this analysis with "glob import forbidden".
    for module in [
        "agentscommander_lib::telegram::claude_watcher",
        "agentscommander_lib::telegram::codex_watcher",
    ] {
        let report = analyze_module_index(&index, CRATE_ID, module)
            .unwrap_or_else(|error| panic!("{module} guard analysis failed:\n{error}"));
        assert!(
            report
                .dependencies
                .iter()
                .any(|row| row.module == OUTPUT_MODULE),
            "{module} must depend on {OUTPUT_MODULE} directly; observed {:?}",
            report.dependencies
        );
        // #2232 phase 4 section 10: the supervisor drives the watchers, never
        // the reverse. Phase 4 moved the reader out of `BridgeHandle.tasks`
        // into `telegram::manager`'s demand registry and gave it a destination
        // `watch` and a capture sink, so the three names a watcher must still
        // never carry are pinned together here. `telegram::bridge` and
        // `telegram::manager` are both SCC members; an arc from either watcher
        // into them, or into any `commands::` module, pulls that watcher into
        // the 88-member cycle.
        for forbidden in [
            "agentscommander_lib::telegram::bridge",
            "agentscommander_lib::telegram::manager",
        ] {
            assert!(
                report
                    .dependencies
                    .iter()
                    .all(|row| row.module != forbidden),
                "{module} must not depend on {forbidden}; observed {:?}",
                report.dependencies
            );
        }
        // `commands::co_managed` and `commands::telegram` are the SCC-side
        // command modules of this epic; `commands::codex_resolver`, which the
        // Codex watcher already named before this phase, is a leaf outside the
        // 88-member cycle, so it is not forbidden here.
        for forbidden in [
            "agentscommander_lib::commands::telegram",
            "agentscommander_lib::commands::session",
            "agentscommander_lib::commands::co_managed",
        ] {
            assert!(
                report
                    .dependencies
                    .iter()
                    .all(|row| row.module != forbidden),
                "{module} must not reach into {forbidden}; observed {:?}",
                report.dependencies
            );
        }
    }
}

#[test]
fn production_every_telegram_source_is_reachable_from_the_module_tree() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let index = production_index();
    let indexed = index
        .bodies
        .iter()
        .map(|body| body.relative_source.clone())
        .chain(index.test_only_sources.iter().cloned())
        .collect::<BTreeSet<_>>();

    let mut on_disk = BTreeSet::new();
    let mut pending = vec![(manifest.join("src/telegram"), "src/telegram".to_owned())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap_or_else(|error| {
            panic!(
                "telegram source directory {} should read: {error}",
                directory.display()
            )
        }) {
            let entry = entry.expect("telegram directory entry should read");
            let name = entry.file_name().to_string_lossy().into_owned();
            let rendered = format!("{prefix}/{name}");
            if entry.path().is_dir() {
                pending.push((entry.path(), rendered));
            } else if name.ends_with(".rs") {
                on_disk.insert(rendered);
            }
        }
    }
    assert!(!on_disk.is_empty(), "telegram subtree must contain sources");

    let unreachable = on_disk.difference(&indexed).cloned().collect::<Vec<_>>();
    assert!(
        unreachable.is_empty(),
        "every .rs under src/telegram/ must be declared by some mod; unreachable: {unreachable:?}"
    );
}

#[test]
fn production_guard_observes_the_exact_initial_dependency_set() {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crate_root = manifest_root.join("src/lib.rs");
    let first_index = build_module_index(&manifest_root, &crate_root, CRATE_ID)
        .expect("production module tree should build");
    let second_index = build_module_index(&manifest_root, &crate_root, CRATE_ID)
        .expect("second production module tree should build");
    let first = analyze_module_index(&first_index, CRATE_ID, TARGET_MODULE)
        .unwrap_or_else(|error| panic!("production guard analysis failed:\n{error}"));
    let second = analyze_module_index(&second_index, CRATE_ID, TARGET_MODULE)
        .unwrap_or_else(|error| panic!("second production guard analysis failed:\n{error}"));

    assert_eq!(first, second, "guard observations must be deterministic");
    require_exact_production_report(&first)
        .unwrap_or_else(|error| panic!("production guard failed:\n{error}"));
}

#[test]
fn zero_target_sources_fail_closed() {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let error = analyze_guard(
        &manifest_root,
        &manifest_root.join("src/lib.rs"),
        CRATE_ID,
        TARGET_MODULE,
        &[],
        &production_modules(),
    )
    .expect_err("zero target sources must not pass vacuously");

    let message = error.to_string();
    assert!(message.contains("target source discovery"));
    assert!(message.contains("zero source files"));
    assert!(message.contains(TARGET_MODULE));
    assert!(message.contains(FOCUSED_RERUN));
}
