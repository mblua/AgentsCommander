use super::*;
use serde_json::json;

fn overlay_of(
    base: &mut Value,
    overlay: Value,
    derived: &[DerivedIdClosure],
) -> LocalSettingsOverlay {
    let Value::Object(map) = overlay else {
        panic!("overlay fixture must be an object");
    };
    LocalSettingsOverlay::from_overlay_object(base, map, &[], &[], derived)
}

fn plain(base: &mut Value, overlay: Value) -> LocalSettingsOverlay {
    overlay_of(base, overlay, &[])
}

const TEST_CLOSURES: &[DerivedIdClosure] = &[DerivedIdClosure {
    source_key: "agents",
    id_field: "id",
    derived_prefix: &["codingAgentProfiles", "profilesByAgent"],
}];

// L1
#[test]
fn nested_override_replaces_only_the_final_key() {
    let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
    assert_eq!(base["watchers"]["a"]["enabled"], json!(true));
    assert_eq!(base["watchers"]["a"]["intervalMs"], json!(50));
    assert!(!state.is_empty());
}

// L2
#[test]
fn an_array_replaces_the_base_array_whole() {
    let mut base = json!({"agents": [{"id": "a"}, {"id": "b"}]});
    plain(&mut base, json!({"agents": [{"id": "c"}]}));
    assert_eq!(base["agents"], json!([{"id": "c"}]));

    let mut base = json!({"agents": [{"id": "a"}]});
    plain(&mut base, json!({"agents": []}));
    assert_eq!(base["agents"], json!([]));
}

// L3
#[test]
fn an_overlay_key_absent_from_the_base_is_inserted() {
    let mut base = json!({"logLevel": "info"});
    plain(&mut base, json!({"newKey": 1}));
    assert_eq!(base["newKey"], json!(1));
    assert_eq!(base["logLevel"], json!("info"));
}

// L4
#[test]
fn an_overlay_object_over_a_base_scalar_replaces_it_wholesale() {
    let mut base = json!({"watchers": 7});
    let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
    assert_eq!(base["watchers"], json!({"a": {"intervalMs": 50}}));
    assert_eq!(state.owned_paths(), &[vec!["watchers".to_string()]]);
    assert_eq!(state.base_values, vec![Some(json!(7))]);
}

// L5
#[test]
fn the_restore_plan_for_a_nested_override_is_one_leaf_entry() {
    let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
    assert_eq!(
        state.owned_paths(),
        &[vec![
            "watchers".to_string(),
            "a".to_string(),
            "intervalMs".to_string()
        ]]
    );
    assert_eq!(state.base_values, vec![Some(json!(500))]);
}

// L6
#[test]
fn an_overlay_subtree_absent_from_the_base_captures_none() {
    let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
    assert_eq!(
        state.owned_paths(),
        &[vec!["watchers".to_string(), "z".to_string()]]
    );
    assert_eq!(state.base_values, vec![None]);
}

// L7
#[test]
fn restore_base_puts_the_base_leaf_back_and_leaves_siblings_alone() {
    let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
    let Value::Object(mut out) = base.clone() else {
        unreachable!()
    };
    out.insert("sibling".to_string(), json!(2));
    state.restore_base(&mut out);
    assert_eq!(out["watchers"]["a"]["intervalMs"], json!(500));
    assert_eq!(out["watchers"]["a"]["enabled"], json!(true));
    assert_eq!(out["sibling"], json!(2));
}

// L8
#[test]
fn restore_base_removes_only_the_owned_subtree_for_a_none_entry() {
    let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
    let Value::Object(mut out) = base.clone() else {
        unreachable!()
    };
    state.restore_base(&mut out);
    assert!(out["watchers"].get("z").is_none());
    assert_eq!(out["watchers"]["a"]["intervalMs"], json!(500));
}

// L9
#[test]
fn restore_base_over_a_missing_or_scalar_intermediate_is_a_no_op() {
    let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));

    let Value::Object(mut missing) = json!({"other": 1}) else {
        unreachable!()
    };
    state.restore_base(&mut missing);
    assert_eq!(Value::Object(missing), json!({"other": 1}));

    let Value::Object(mut scalar) = json!({"watchers": 5}) else {
        unreachable!()
    };
    state.restore_base(&mut scalar);
    assert_eq!(Value::Object(scalar), json!({"watchers": 5}));
}

// L10
#[test]
fn reapply_from_reproduces_the_effective_value_including_a_removed_path() {
    let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
    let Value::Object(effective) = base.clone() else {
        unreachable!()
    };
    let mut out = effective.clone();
    state.restore_base(&mut out);
    assert!(out["watchers"].get("z").is_none());
    state.reapply_from(&effective, &mut out);
    assert_eq!(out["watchers"]["z"], json!({"intervalMs": 9}));
}

// L11
#[test]
fn reapply_from_creates_a_missing_intermediate_object() {
    let mut base = json!({"watchers": {}});
    let state = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
    assert_eq!(
        state.owned_paths(),
        &[vec!["watchers".to_string(), "z".to_string()]]
    );
    let Value::Object(effective) = base.clone() else {
        unreachable!()
    };
    let mut out: Map<String, Value> = Map::new();
    state.reapply_from(&effective, &mut out);
    assert_eq!(out["watchers"]["z"], json!({"intervalMs": 9}));
}

// L12
#[test]
fn an_empty_overlay_object_owns_nothing() {
    let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
    let state = plain(&mut base, json!({}));
    assert!(state.is_empty());
    assert!(state.owned_paths().is_empty());
    assert_eq!(base, json!({"watchers": {"a": {"intervalMs": 500}}}));

    let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {}}));
    assert!(state.owned_paths().is_empty());
    assert_eq!(base, json!({"watchers": {"a": {"intervalMs": 500}}}));
}

// L13
#[test]
fn a_disk_ineligible_key_is_dropped_and_owns_nothing() {
    let mut base = json!({"rootToken": "base-token", "logLevel": "info"});
    let Value::Object(overlay) = json!({"rootToken": "override", "logLevel": "debug"}) else {
        unreachable!()
    };
    let state =
        LocalSettingsOverlay::from_overlay_object(&mut base, overlay, &["rootToken"], &[], &[]);
    assert_eq!(base["rootToken"], json!("base-token"));
    assert_eq!(base["logLevel"], json!("debug"));
    assert_eq!(state.dropped_keys(), &["rootToken".to_string()]);
    let records = state.diagnostics("settings.local.json");
    let dropped: Vec<&OverlayDiagnostic> = records
        .iter()
        .filter(|record| matches!(record, OverlayDiagnostic::IneligibleKeyDropped { .. }))
        .collect();
    assert_eq!(dropped.len(), 1);
    match dropped[0] {
        OverlayDiagnostic::IneligibleKeyDropped { key, rule, .. } => {
            assert_eq!(key, "rootToken");
            assert_eq!(*rule, RULE_DISK_AUTHORITATIVE);
        }
        _ => unreachable!(),
    }
}

// L14
#[test]
fn a_legacy_ineligible_key_is_dropped_with_the_legacy_rule() {
    let mut base = json!({"sidebarZoom": 1.5});
    let Value::Object(overlay) = json!({"sidebarZoom": 2.0}) else {
        unreachable!()
    };
    let state =
        LocalSettingsOverlay::from_overlay_object(&mut base, overlay, &[], &["sidebarZoom"], &[]);
    assert_eq!(base["sidebarZoom"], json!(1.5));
    assert!(state.is_empty());
    match &state.diagnostics("settings.local.json")[0] {
        OverlayDiagnostic::IneligibleKeyDropped { key, rule, .. } => {
            assert_eq!(key, "sidebarZoom");
            assert_eq!(*rule, RULE_LEGACY_MIGRATION_SOURCE);
        }
        other => panic!("unexpected record: {other:?}"),
    }
}

// L15
#[test]
fn invalid_json_is_rejected_and_leaves_the_base_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let settings = temp.path().join("settings.json");
    std::fs::write(
        temp.path().join("settings.50.personal.no-git.json"),
        "{ not json",
    )
    .unwrap();
    let mut base = json!({"logLevel": "info"});
    let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
    assert!(matches!(
        state.rejection(),
        Some(OverlayRejection::InvalidJson(_))
    ));
    assert!(state.is_empty());
    assert_eq!(base, json!({"logLevel": "info"}));
    let records = state.diagnostics("settings.local.json");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].level(), OverlayDiagnosticLevel::Error);
}

// L16
#[test]
fn a_top_level_non_object_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let settings = temp.path().join("settings.json");
    std::fs::write(
        temp.path().join("settings.50.personal.no-git.json"),
        "[1, 2]",
    )
    .unwrap();
    let mut base = json!({"logLevel": "info"});
    let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
    assert_eq!(state.rejection(), Some(&OverlayRejection::NotAnObject));
    assert!(state.is_empty());
    assert_eq!(base, json!({"logLevel": "info"}));
    assert_eq!(state.diagnostics("x").len(), 1);
}

// L17
#[test]
fn an_unreadable_overlay_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let settings = temp.path().join("settings.json");
    std::fs::create_dir(temp.path().join("settings.50.personal.no-git.json")).unwrap();
    let mut base = json!({"logLevel": "info"});
    let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
    assert!(matches!(
        state.rejection(),
        Some(OverlayRejection::Unreadable(_))
    ));
    assert_eq!(base, json!({"logLevel": "info"}));
}

// L18
#[test]
fn an_absent_overlay_file_produces_no_diagnostic_at_all() {
    let temp = tempfile::tempdir().unwrap();
    let settings = temp.path().join("settings.json");
    let mut base = json!({"logLevel": "info"});
    let state = LocalSettingsOverlay::load_and_merge(&settings, &mut base, &[], &[], &[]);
    assert!(state.is_empty());
    assert_eq!(state.rejection(), None);
    assert!(state.diagnostics("settings.local.json").is_empty());
    assert!(state.owned_paths().is_empty());
    assert_eq!(base, json!({"logLevel": "info"}));
}

// L19
#[test]
fn an_applied_overlay_records_one_info_record_naming_every_owned_path() {
    let mut base = json!({"watchers": {"a": {"enabled": true, "intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
    let records = state.diagnostics("C:/x/settings.local.json");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].level(), OverlayDiagnosticLevel::Info);
    let rendered = records[0].render();
    assert!(rendered.contains("C:/x/settings.local.json"), "{rendered}");
    assert!(rendered.contains("watchers.a.intervalMs"), "{rendered}");
}

// L20
#[test]
fn the_derived_id_closure_introduces_exactly_the_new_ids() {
    let mut base = json!({"agents": [{"id": "a"}, {"id": "b"}]});
    let state = overlay_of(
        &mut base,
        json!({"agents": [{"id": "b"}, {"id": "c"}]}),
        TEST_CLOSURES,
    );
    let derived: Vec<&Vec<String>> = state
        .owned_paths()
        .iter()
        .filter(|path| path.first().map(String::as_str) == Some("codingAgentProfiles"))
        .collect();
    assert_eq!(
        derived,
        vec![&vec![
            "codingAgentProfiles".to_string(),
            "profilesByAgent".to_string(),
            "c".to_string()
        ]]
    );

    let mut base = json!({"agents": [{"id": "a"}]});
    let state = overlay_of(&mut base, json!({"logLevel": "debug"}), TEST_CLOSURES);
    assert_eq!(state.owned_paths(), &[vec!["logLevel".to_string()]]);
}

// L21
#[test]
fn the_closure_captures_the_base_value_and_never_duplicates_an_owned_path() {
    let mut base = json!({
        "agents": [{"id": "a"}],
        "codingAgentProfiles": {"profilesByAgent": {"stale": {"A": {"command": "x"}}}}
    });
    let state = overlay_of(
        &mut base,
        json!({"agents": [{"id": "stale"}, {"id": "fresh"}]}),
        TEST_CLOSURES,
    );
    let entries: Vec<(Vec<String>, Option<Value>)> = state
        .paths
        .iter()
        .cloned()
        .zip(state.base_values.iter().cloned())
        .filter(|(path, _)| path.first().map(String::as_str) == Some("codingAgentProfiles"))
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].1, Some(json!({"A": {"command": "x"}})));
    assert_eq!(entries[1].1, None);

    // The operator already owns the exact derived path: no duplicate entry.
    let mut base = json!({"agents": [{"id": "a"}]});
    let state = overlay_of(
        &mut base,
        json!({
            "agents": [{"id": "fresh"}],
            "codingAgentProfiles": {"profilesByAgent": {"fresh": {"A": {}}}}
        }),
        TEST_CLOSURES,
    );
    let matching = state
        .owned_paths()
        .iter()
        .filter(|path| {
            **path
                == vec![
                    "codingAgentProfiles".to_string(),
                    "profilesByAgent".to_string(),
                    "fresh".to_string(),
                ]
        })
        .count();
    assert_eq!(matching, 1);
}

// L22
#[test]
fn an_ineligible_source_key_never_triggers_the_closure() {
    let mut base = json!({"agents": [{"id": "a"}]});
    let Value::Object(overlay) = json!({"agents": [{"id": "c"}]}) else {
        unreachable!()
    };
    let state = LocalSettingsOverlay::from_overlay_object(
        &mut base,
        overlay,
        &["agents"],
        &[],
        TEST_CLOSURES,
    );
    assert!(state.owned_paths().is_empty());
    assert_eq!(base["agents"], json!([{"id": "a"}]));
}

// L23
#[test]
fn markdown_override_path_appends_local_before_the_md_extension() {
    assert_eq!(
        markdown_override_path(Path::new("/x/Context.coordinator.md")),
        Some(PathBuf::from("/x/Context.coordinator.local.md"))
    );
    assert_eq!(
        markdown_override_path(Path::new("/x/a.b.md")),
        Some(PathBuf::from("/x/a.b.local.md"))
    );
    assert_eq!(markdown_override_path(Path::new("/x/Role.txt")), None);
    assert_eq!(markdown_override_path(Path::new("/x/noext")), None);
}

// L24
#[test]
fn read_markdown_override_maps_every_shape() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("Context.coordinator.md");
    std::fs::write(&base, "base bytes").unwrap();
    assert_eq!(read_markdown_override(&base), MarkdownOverride::Absent);

    let local = temp.path().join("Context.coordinator.local.md");
    std::fs::write(&local, "local bytes").unwrap();
    assert_eq!(
        read_markdown_override(&base),
        MarkdownOverride::Present("local bytes".to_string())
    );

    std::fs::remove_file(&local).unwrap();
    std::fs::create_dir(&local).unwrap();
    match read_markdown_override(&base) {
        MarkdownOverride::Rejected { reason, .. } => assert!(!reason.is_empty()),
        other => panic!("expected Rejected, got {other:?}"),
    }
    std::fs::remove_dir(&local).unwrap();

    std::fs::write(&local, [0x66u8, 0xff, 0x66]).unwrap();
    assert!(matches!(
        read_markdown_override(&base),
        MarkdownOverride::Rejected { .. }
    ));
}

// L25
#[test]
fn diagnostic_levels_are_error_for_three_variants_and_info_for_applied() {
    assert_eq!(
        OverlayDiagnostic::Rejected {
            source: "s".to_string(),
            rejection: OverlayRejection::NotAnObject,
        }
        .level(),
        OverlayDiagnosticLevel::Error
    );
    assert_eq!(
        OverlayDiagnostic::IneligibleKeyDropped {
            source: "s".to_string(),
            key: "rootToken".to_string(),
            rule: RULE_DISK_AUTHORITATIVE,
        }
        .level(),
        OverlayDiagnosticLevel::Error
    );
    assert_eq!(
        OverlayDiagnostic::MarkdownRejected {
            source: "s".to_string(),
            reason: "r".to_string(),
        }
        .level(),
        OverlayDiagnosticLevel::Error
    );
    assert_eq!(
        OverlayDiagnostic::Applied {
            source: "s".to_string(),
            owned: vec!["logLevel".to_string()],
        }
        .level(),
        OverlayDiagnosticLevel::Info
    );
}

// L26
#[test]
fn a_rejected_markdown_override_yields_an_error_diagnostic_naming_path_and_reason() {
    assert_eq!(MarkdownOverride::Absent.diagnostic(), None);
    assert_eq!(
        MarkdownOverride::Present("x".to_string()).diagnostic(),
        None
    );

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("Context.coordinator.md");
    std::fs::write(&base, "base bytes").unwrap();
    let local = temp.path().join("Context.coordinator.local.md");
    std::fs::create_dir(&local).unwrap();

    let value = read_markdown_override(&base);
    let diagnostic = value.diagnostic().expect("a rejection has a diagnostic");
    assert!(matches!(
        diagnostic,
        OverlayDiagnostic::MarkdownRejected { .. }
    ));
    assert_eq!(diagnostic.level(), OverlayDiagnosticLevel::Error);
    let rendered = diagnostic.render();
    assert!(
        rendered.contains(&local.display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("regular file"), "{rendered}");
}

// L27
#[test]
fn owns_top_level_is_true_for_the_first_segment_of_every_owned_path() {
    let mut base = json!({"mainZoom": 1.5});
    let state = plain(&mut base, json!({"mainZoom": 1.0}));
    assert!(state.owns_top_level("mainZoom"));
    assert!(!state.owns_top_level("mainGeometry"));

    let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
    let state = plain(&mut base, json!({"watchers": {"a": {"intervalMs": 50}}}));
    assert!(state.owns_top_level("watchers"));
    assert!(!state.owns_top_level("a"));
    assert!(!state.owns_top_level("intervalMs"));

    let mut base = json!({"agents": [{"id": "a"}]});
    let state = overlay_of(&mut base, json!({"agents": [{"id": "c"}]}), TEST_CLOSURES);
    assert!(state.owns_top_level("codingAgentProfiles"));
    assert!(state.owns_top_level("agents"));

    let default = LocalSettingsOverlay::default();
    for key in ["mainZoom", "watchers", "agents", "codingAgentProfiles"] {
        assert!(!default.owns_top_level(key));
    }
}

// L28
#[test]
fn a_dropped_key_is_not_owned() {
    let mut base = json!({"rootToken": "base"});
    let Value::Object(overlay) = json!({"rootToken": "override"}) else {
        unreachable!()
    };
    let state =
        LocalSettingsOverlay::from_overlay_object(&mut base, overlay, &["rootToken"], &[], &[]);
    assert!(!state.owns_top_level("rootToken"));
}

// L29
#[test]
fn a_non_object_array_or_non_array_source_key_yields_no_closure_entries() {
    let mut base = json!({"agents": [{"id": "a"}]});
    let state = overlay_of(&mut base, json!({"agents": [1, 2, "x"]}), TEST_CLOSURES);
    assert_eq!(state.owned_paths(), &[vec!["agents".to_string()]]);

    let mut base = json!({"agents": [{"id": "a"}]});
    let state = overlay_of(&mut base, json!({"agents": "not-an-array"}), TEST_CLOSURES);
    assert_eq!(state.owned_paths(), &[vec!["agents".to_string()]]);
}

// L30
#[test]
fn reapply_from_removes_an_owned_path_that_is_absent_from_effective() {
    let mut base = json!({"k": {"x": 1}, "sibling": 2});
    let state = plain(&mut base, json!({"k": null}));
    assert_eq!(state.owned_paths(), &[vec!["k".to_string()]]);
    assert_eq!(state.base_values, vec![Some(json!({"x": 1}))]);

    // `out` omits `k`, the shape serde produces for a `skip_serializing_if`
    // field whose value is `None`. `restore_base` puts the base object back.
    let Value::Object(mut out) = json!({"sibling": 2}) else {
        unreachable!()
    };
    state.restore_base(&mut out);
    assert_eq!(out["k"], json!({"x": 1}));

    // `effective` also omits `k`, so the live value is absence and the path
    // must be REMOVED, not left holding the base value.
    let Value::Object(effective) = json!({"sibling": 2}) else {
        unreachable!()
    };
    state.reapply_from(&effective, &mut out);
    assert!(out.get("k").is_none());
    assert_eq!(out["sibling"], json!(2));

    // A nested owned path is removed the same way.
    let mut base = json!({"watchers": {"a": {"commands": ["x"], "pattern": "p"}}});
    let nested = plain(&mut base, json!({"watchers": {"a": {"commands": null}}}));
    assert_eq!(
        nested.owned_paths(),
        &[vec![
            "watchers".to_string(),
            "a".to_string(),
            "commands".to_string()
        ]]
    );
    let Value::Object(mut out) = json!({"watchers": {"a": {"pattern": "p"}}}) else {
        unreachable!()
    };
    nested.restore_base(&mut out);
    assert_eq!(out["watchers"]["a"]["commands"], json!(["x"]));
    let Value::Object(effective) = json!({"watchers": {"a": {"pattern": "p"}}}) else {
        unreachable!()
    };
    nested.reapply_from(&effective, &mut out);
    assert!(out["watchers"]["a"].get("commands").is_none());
    assert_eq!(out["watchers"]["a"]["pattern"], json!("p"));

    // Removal creates nothing when `out`'s intermediate is missing or is not
    // an object, and does not panic.
    let Value::Object(mut missing) = json!({"other": 1}) else {
        unreachable!()
    };
    nested.reapply_from(&effective, &mut missing);
    assert_eq!(Value::Object(missing), json!({"other": 1}));
    let Value::Object(mut scalar) = json!({"watchers": 5}) else {
        unreachable!()
    };
    nested.reapply_from(&effective, &mut scalar);
    assert_eq!(Value::Object(scalar), json!({"watchers": 5}));

    // The second no-op shape: `restore_base` already removed the path
    // (captured base `None`) and `effective` does not carry it either.
    let mut base = json!({"watchers": {"a": {"intervalMs": 500}}});
    let absent = plain(&mut base, json!({"watchers": {"z": {"intervalMs": 9}}}));
    let Value::Object(mut out) = json!({"watchers": {"a": {"intervalMs": 500}}}) else {
        unreachable!()
    };
    absent.restore_base(&mut out);
    let Value::Object(effective) = json!({"watchers": {"a": {"intervalMs": 500}}}) else {
        unreachable!()
    };
    absent.reapply_from(&effective, &mut out);
    assert!(out["watchers"].get("z").is_none());

    // And the value is still COPIED when `effective` does carry it, so the
    // removal rule cannot be satisfied by removing unconditionally.
    let Value::Object(effective) =
        json!({"watchers": {"a": {"intervalMs": 500}, "z": {"intervalMs": 9}}})
    else {
        unreachable!()
    };
    absent.reapply_from(&effective, &mut out);
    assert_eq!(out["watchers"]["z"], json!({"intervalMs": 9}));
}

#[test]
fn into_undecodable_empties_the_plan_and_records_the_rejection() {
    let mut base = json!({"mainZoom": 1.5});
    let state = plain(&mut base, json!({"mainZoom": 1.0}));
    assert!(!state.is_empty());
    let state = state.into_undecodable("bad type".to_string());
    assert!(state.is_empty());
    assert!(state.owned_paths().is_empty());
    assert!(matches!(
        state.rejection(),
        Some(OverlayRejection::MergedValueUndecodable(_))
    ));
    let records = state.diagnostics("settings.local.json");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].level(), OverlayDiagnosticLevel::Error);
    assert!(records[0].render().contains("bad type"));
}

/// `into_undecodable` PRESERVES the ineligible-key drops, which is correct on
/// `parse_settings_json`'s path (it renders once, at the end) and is exactly
/// why `default_settings_with_overlay`'s decode-error arm must not render this
/// value: it already rendered those drops before its early return, so
/// re-rendering the whole list would emit each drop twice. A default overlay
/// carries the new record alone, which is what that arm uses.
#[test]
fn into_undecodable_preserves_drops_and_a_default_overlay_carries_one_record() {
    let mut base = json!({"rootToken": "base", "mainZoom": 1.5});
    let Value::Object(overlay) = json!({"rootToken": "x", "mainZoom": 1.0}) else {
        unreachable!()
    };
    let state =
        LocalSettingsOverlay::from_overlay_object(&mut base, overlay, &["rootToken"], &[], &[]);
    assert_eq!(state.dropped_keys(), &["rootToken".to_string()]);
    let carried = state.into_undecodable("bad type".to_string());
    assert_eq!(
        carried.diagnostics("settings.local.json").len(),
        2,
        "the drop survives alongside the new rejection"
    );

    let fresh = LocalSettingsOverlay::default().into_undecodable("bad type".to_string());
    let records = fresh.diagnostics("settings.local.json");
    assert_eq!(records.len(), 1);
    assert!(matches!(
        records[0],
        OverlayDiagnostic::Rejected {
            rejection: OverlayRejection::MergedValueUndecodable(_),
            ..
        }
    ));
}

/// #2470 - the overlay name is typed twice, here and in the registry. The
/// tie is test-only because this module has no outgoing arc to spend.
#[test]
fn overlay_file_name_matches_the_registry() {
    assert_eq!(
        SETTINGS_LOCAL_FILE_NAME,
        crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME
    );
}
