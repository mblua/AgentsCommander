use super::*;

fn parsed_struct_visibility(prefix: &str) -> Visibility {
    let source = if prefix.is_empty() {
        "struct Item;".to_owned()
    } else {
        format!("{prefix} struct Item;")
    };
    let item = syn::parse_str::<Item>(&source)
        .unwrap_or_else(|error| panic!("visibility fixture did not parse: {source}: {error}"));
    let Item::Struct(item_struct) = item else {
        panic!("visibility fixture did not produce a struct")
    };
    item_struct.vis
}

#[test]
fn visibility_matrix_distinguishes_canonical_shorthand_from_path_spelling() {
    require_visibility(
        &parsed_struct_visibility("pub(crate)"),
        RequiredVisibility::Crate,
        "fixture item",
    )
    .expect("canonical pub(crate) should pass");
    for form in [
        "",
        "pub(self)",
        "pub(in self)",
        "pub(super)",
        "pub",
        "pub(in crate)",
        "pub(in super)",
        "pub(in crate::telegram)",
    ] {
        let message = require_visibility(
            &parsed_struct_visibility(form),
            RequiredVisibility::Crate,
            "fixture item",
        )
        .expect_err("noncanonical item visibility must fail")
        .to_string();
        assert!(message.contains("fixture item"));
        assert!(message.contains("observed visibility"));
        if form == "pub(in crate)" {
            assert!(message.contains("graph-neutral"));
            assert!(message.contains("in_token.is_none()"));
        }
        if form == "pub(in crate::telegram)" {
            assert!(message.contains("detector 1.1.0"));
            assert!(message.contains("output -> telegram"));
        }
    }

    require_visibility(
        &parsed_struct_visibility("pub(super)"),
        RequiredVisibility::Super,
        "fixture module/facade",
    )
    .expect("canonical pub(super) should pass");
    for form in [
        "",
        "pub(self)",
        "pub(in self)",
        "pub(crate)",
        "pub",
        "pub(in super)",
        "pub(in crate)",
        "pub(in crate::telegram)",
    ] {
        require_visibility(
            &parsed_struct_visibility(form),
            RequiredVisibility::Super,
            "fixture module/facade",
        )
        .expect_err("noncanonical module/facade visibility must fail");
    }

    require_visibility(
        &parsed_struct_visibility(""),
        RequiredVisibility::Private,
        "fixture private field/helper",
    )
    .expect("inherited visibility should be private");
    require_visibility(
        &parsed_struct_visibility("pub(crate)"),
        RequiredVisibility::Private,
        "fixture private field/helper",
    )
    .expect_err("public helper must not satisfy private contract");
}

#[test]
fn moved_symbol_map_preserves_duplicates_locations_and_rejects_local_substitutes() {
    let duplicate = Fixture::with_telegram("duplicate-symbol-bodies");
    duplicate.write("src/telegram.rs", include_str!("sources/source-7.txt"));
    let duplicate_index = duplicate
        .index_as(CRATE_ID)
        .expect("duplicate fixture should index");
    let duplicate_rows = collect_moved_symbol_occurrences(&duplicate_index, CRATE_ID)
        .remove(&format!("struct {OUTPUT_MODULE}::BridgeLogger"))
        .expect("duplicate BridgeLogger key should exist");
    assert_eq!(duplicate_rows.len(), 2);
    assert_ne!(duplicate_rows[0].body_id, duplicate_rows[1].body_id);

    let misplaced = Fixture::with_telegram("misplaced-inherent-method");
    misplaced.write("src/telegram.rs", include_str!("sources/source-8.txt"));
    let misplaced_index = misplaced
        .index_as(CRATE_ID)
        .expect("misplaced fixture should index");
    let misplaced_map = collect_moved_symbol_occurrences(&misplaced_index, CRATE_ID);
    let misplaced_log = misplaced_map
        .get(&format!("method {OUTPUT_MODULE}::BridgeLogger::log"))
        .expect("out-of-place method should resolve through its destination self type");
    assert_eq!(misplaced_log.len(), 2);
    assert!(misplaced_log
        .iter()
        .any(|row| row.body_id.contains("::output#")));
    assert!(misplaced_log
        .iter()
        .any(|row| row.body_id.contains("::bridge#")));

    let local = Fixture::with_telegram("local-item-not-definition");
    local.write(
        "src/telegram.rs",
        "pub mod claude_watcher {}\npub mod output { fn wrapper() { struct BridgeLogger; } }\n",
    );
    let local_index = local
        .index_as(CRATE_ID)
        .expect("local-item fixture should index");
    assert!(
        !collect_moved_symbol_occurrences(&local_index, CRATE_ID)
            .contains_key(&format!("struct {OUTPUT_MODULE}::BridgeLogger")),
        "function-local item must not satisfy module-scope destination presence"
    );
}

#[test]
fn module_tree_resolves_both_conventional_source_forms() {
    for (label, telegram_source) in [
        ("name-rs", "src/telegram.rs"),
        ("name-mod-rs", "src/telegram/mod.rs"),
    ] {
        let fixture = Fixture::with_telegram(label);
        fixture.write(telegram_source, "pub mod claude_watcher {}\n");
        let index = fixture.index().expect("conventional source should resolve");
        assert!(index.declared_modules.contains("fixture::telegram"));
        assert!(index
            .declared_modules
            .contains("fixture::telegram::claude_watcher"));
    }
}

#[test]
fn module_tree_rejects_ambiguous_and_missing_conventional_sources() {
    let ambiguous = Fixture::with_telegram("ambiguous");
    ambiguous.write("src/telegram.rs", "");
    ambiguous.write("src/telegram/mod.rs", "");
    let ambiguous_error = ambiguous
        .index()
        .expect_err("two candidates must fail")
        .to_string();
    assert!(ambiguous_error.contains("ambiguous source"));
    assert!(ambiguous_error.contains("telegram.rs"));
    assert!(
        ambiguous_error.contains("telegram\\mod.rs") || ambiguous_error.contains("telegram/mod.rs")
    );

    let missing = Fixture::with_telegram("missing");
    let missing_error = missing
        .index()
        .expect_err("zero candidates must fail")
        .to_string();
    assert!(missing_error.contains("missing source"));
    assert!(missing_error.contains("telegram.rs"));
    assert!(missing_error.contains(FOCUSED_RERUN));
}

#[test]
fn nested_inline_modules_share_a_source_without_losing_body_identity() {
    let fixture = Fixture::with_telegram("nested-inline");
    fixture.write("src/telegram.rs", include_str!("sources/source-9.txt"));
    let index = fixture.index().expect("inline tree should index");
    let target_bodies = index
        .bodies
        .iter()
        .filter(|body| {
            body.module_id
                .starts_with("fixture::telegram::claude_watcher")
        })
        .collect::<Vec<_>>();
    assert_eq!(target_bodies.len(), 2);
    assert_eq!(target_bodies[0].source, target_bodies[1].source);
    assert_ne!(target_bodies[0].body_id, target_bodies[1].body_id);
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("inline target should analyze");
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/telegram.rs".to_owned(),
        module: "fixture::telegram::bridge".to_owned(),
    }));
}

#[test]
fn cfg_disjoint_inline_variants_keep_distinct_scopes_and_scan_the_second_body() {
    let fixture = Fixture::with_telegram("cfg-inline-variants");
    fixture.write("src/telegram.rs", include_str!("sources/source-10.txt"));
    let index = fixture
        .index()
        .expect("cfg-disjoint inline variants should index");
    let bodies = index
        .bodies
        .iter()
        .filter(|body| body.module_id == "fixture::telegram::claude_watcher")
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 2);
    assert_ne!(bodies[0].body_id, bodies[1].body_id);
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("both inline variants should scan");
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/telegram.rs".to_owned(),
        module: "fixture::telegram::bridge".to_owned(),
    }));
}

#[test]
fn cfg_disjoint_out_of_line_variants_scan_every_selected_file() {
    let fixture = Fixture::with_telegram("cfg-out-of-line-variants");
    fixture.write("src/telegram.rs", include_str!("sources/source-11.txt"));
    fixture.write("src/claude_a.rs", "fn first() {}\n");
    fixture.write(
        "src/claude_b.rs",
        "use crate::telegram::bridge::Forbidden;\n",
    );
    let index = fixture
        .index()
        .expect("cfg-disjoint out-of-line variants should index");
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("both out-of-line variants should scan");
    assert_eq!(
        report.sources,
        ["src/claude_a.rs".to_owned(), "src/claude_b.rs".to_owned()]
            .into_iter()
            .collect()
    );
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/claude_b.rs".to_owned(),
        module: "fixture::telegram::bridge".to_owned(),
    }));
}

#[test]
fn module_tree_fails_on_source_set_shrink_and_zero_target() {
    let fixture = Fixture::with_telegram("source-set-shrink");
    fixture.write("src/telegram.rs", "pub mod claude_watcher {}\n");
    let index = fixture.index().expect("fixture tree should index");
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("target root should exist");
    let expected = [
        "src/telegram.rs".to_owned(),
        "src/telegram/claude_watcher/output.rs".to_owned(),
    ]
    .into_iter()
    .collect();
    let shrink = require_exact_source_set(&report, &expected, "fixture::telegram::claude_watcher")
        .expect_err("missing expected descendant must fail")
        .to_string();
    assert!(shrink.contains("target source-set equality"));
    assert!(shrink.contains("output.rs"));

    let zero = analyze_module_index(&index, "fixture", "fixture::absent")
        .expect_err("absent target must fail")
        .to_string();
    assert!(zero.contains("zero source files"));
    assert!(zero.contains("fixture::absent"));
}

#[test]
fn module_tree_rejects_escape_duplicate_owner_and_active_source_cycle() {
    let escape = Fixture::new("escape");
    escape.write(
        "src/lib.rs",
        "#[path = \"../../outside.rs\"] mod escaped;\n",
    );
    escape.write_outside_manifest("outside.rs", "");
    let escape_error = escape
        .index()
        .expect_err("manifest escape must fail")
        .to_string();
    assert!(escape_error.contains("outside canonical manifest root"));
    assert!(escape_error.contains("outside.rs"));

    let duplicate = Fixture::new("duplicate-owner");
    duplicate.write(
        "src/lib.rs",
        "#[path = \"shared.rs\"] mod first;\n#[path = \"shared.rs\"] mod second;\n",
    );
    duplicate.write("src/shared.rs", "");
    let duplicate_error = duplicate
        .index()
        .expect_err("two module IDs cannot own one source")
        .to_string();
    assert!(duplicate_error.contains("out-of-line source ownership"));
    assert!(duplicate_error.contains("fixture::first"));
    assert!(duplicate_error.contains("fixture::second"));

    let cycle = Fixture::new("active-cycle");
    cycle.write("src/lib.rs", "mod child;\n");
    cycle.write("src/child.rs", "#[path = \"lib.rs\"] mod back;\n");
    let cycle_error = cycle
        .index()
        .expect_err("active source cycle must fail")
        .to_string();
    assert!(cycle_error.contains("active module source cycle"));
    assert!(cycle_error.contains("src/lib.rs"));
}

#[test]
fn raw_module_ids_normalize_and_exact_cfg_test_bodies_are_excluded() {
    let fixture = Fixture::new("raw-modules-and-cfg-test");
    fixture.write("src/lib.rs", "mod r#telegram;\n");
    fixture.write("src/telegram.rs", include_str!("sources/source-12.txt"));
    let index = fixture
        .index_as(CRATE_ID)
        .expect("raw target tree should index");
    assert!(index.declared_modules.contains(TARGET_MODULE));
    assert!(!index
        .declared_modules
        .contains("agentscommander_lib::telegram::ignored"));
    let report = analyze_module_index(&index, CRATE_ID, TARGET_MODULE)
        .expect("raw target body should analyze");
    assert!(report
        .dependencies
        .iter()
        .all(|row| row.module != "agentscommander_lib::telegram::bridge"));
}

#[test]
fn invalid_direct_path_attributes_fail_with_named_diagnostics() {
    let non_string = Fixture::new("non-string-path");
    non_string.write("src/lib.rs", "#[path = 1] mod child;\n");
    let non_string_message = non_string
        .index()
        .expect_err("non-string path must fail")
        .to_string();
    assert!(non_string_message.contains("non-string path value"));
    assert!(non_string_message.contains("module child"));

    let duplicate = Fixture::new("duplicate-direct-path");
    duplicate.write(
        "src/lib.rs",
        "#[path = \"first.rs\"] #[path = \"second.rs\"] mod child;\n",
    );
    duplicate.write("src/first.rs", "");
    duplicate.write("src/second.rs", "");
    let duplicate_message = duplicate
        .index()
        .expect_err("duplicate direct path attributes must fail")
        .to_string();
    assert!(duplicate_message.contains("duplicate direct path attributes"));
}

#[test]
fn failure_messages_include_contract_diffs_rerun_scope_and_detector() {
    let fixture = Fixture::new("drop-reporting-contract");
    let fixture_root = fixture.root.clone();
    assert!(
        fixture_root.is_absolute(),
        "simulated cleanup fixture path must be absolute: {}",
        fixture_root.display()
    );
    let simulated_error = std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "simulated Windows sharing violation (os error 32)",
    );
    let simulated_error_text = simulated_error.to_string();
    let drop_diagnostic = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed_fixture_root = fixture_root.clone();
    let fixture = fixture.with_drop_cleanup(
        move |observed_root| {
            assert_eq!(observed_root, observed_fixture_root.as_path());
            Err(simulated_error)
        },
        RecordingDiagnostic {
            bytes: std::rc::Rc::clone(&drop_diagnostic),
        },
    );
    let drop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(fixture)));
    let cleanup_result = fs::remove_dir_all(&fixture_root);
    assert!(
        drop_result.is_ok(),
        "Fixture::drop must not panic while reporting a simulated cleanup failure"
    );
    assert!(
        cleanup_result.is_ok(),
        "could not remove {} after the injected Fixture::drop failure: {cleanup_result:?}",
        fixture_root.display()
    );

    let writer_fixture = Fixture::new("drop-writer-error-contract");
    let writer_fixture_root = writer_fixture.root.clone();
    let observed_writer_fixture_root = writer_fixture_root.clone();
    let write_calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let writer_fixture = writer_fixture.with_drop_cleanup(
        move |observed_root| {
            assert_eq!(observed_root, observed_writer_fixture_root.as_path());
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated Windows sharing violation (os error 32)",
            ))
        },
        FailingDiagnostic {
            write_calls: std::rc::Rc::clone(&write_calls),
        },
    );
    let writer_drop_result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(writer_fixture)));
    let writer_cleanup_result = fs::remove_dir_all(&writer_fixture_root);
    assert!(
        writer_drop_result.is_ok(),
        "Fixture::drop must not panic when its diagnostic writer returns an error"
    );
    assert!(
        write_calls.get() > 0,
        "Fixture::drop did not exercise the injected failing diagnostic writer"
    );
    assert!(
        writer_cleanup_result.is_ok(),
        "could not remove {} after the injected writer failure: {writer_cleanup_result:?}",
        writer_fixture_root.display()
    );

    let drop_diagnostic = drop_diagnostic.borrow().clone();
    let drop_diagnostic = String::from_utf8(drop_diagnostic)
        .expect("fixture cleanup fallback diagnostic should be UTF-8");
    assert_eq!(
        drop_diagnostic,
        format!(
            "fixture cleanup fallback could not remove isolated fixture {}: \
             {simulated_error_text}\n",
            fixture_root.display()
        ),
        "Drop fallback must report the absolute fixture path and concrete cleanup error"
    );

    let report = GuardReport {
        sources: [TARGET_ROOT_SOURCE.to_owned()].into_iter().collect(),
        dependencies: [DependencyObservation {
            source: TARGET_ROOT_SOURCE.to_owned(),
            module: "agentscommander_lib::telegram::bridge".to_owned(),
        }]
        .into_iter()
        .collect(),
    };
    let message = require_exact_production_report(&report)
        .expect_err("incomplete report must fail")
        .to_string();
    for marker in [
        "CONTRACT:",
        "missing expected source rows",
        "unexpected observed dependency rows",
        TARGET_ROOT_SOURCE,
        "agentscommander_lib::telegram::bridge",
        FOCUSED_RERUN,
        "SCOPE:",
        AUTHORITATIVE_TOPOLOGY,
    ] {
        assert!(
            message.contains(marker),
            "missing diagnostic marker {marker}"
        );
    }
}
