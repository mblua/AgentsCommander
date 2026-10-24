use super::*;

#[test]
fn structured_spelling_corpus_resolves_every_supported_bridge_path() {
    let cases = [
        (
            "01-direct",
            "#[cfg(any())] use crate::telegram::bridge::BridgeLogger;",
        ),
        (
            "02-grouped",
            "#[cfg(any())] use crate::telegram::{bridge::BridgeLogger};",
        ),
        (
            "03-nested-grouped",
            "#[cfg(any())] use crate::{telegram::{bridge::{BridgeLogger}}};",
        ),
        (
            "04-raw-bridge",
            "#[cfg(any())] use crate::telegram::r#bridge::BridgeLogger;",
        ),
        (
            "05-comment-between-segments",
            "#[cfg(any())] use crate::telegram::/* comment */bridge::BridgeLogger;",
        ),
        (
            "06-sibling-super",
            "#[cfg(any())] use super::bridge::BridgeLogger;",
        ),
        (
            "07-absolute-crate",
            "#[cfg(any())] use ::agentscommander_lib::telegram::bridge::BridgeLogger;",
        ),
        (
            "08-fixed-point-alias",
            "#[cfg(any())] use tg::bridge::BridgeLogger; #[cfg(any())] use crate::telegram as tg;",
        ),
        (
            "09-public-rename",
            "#[cfg(any())] pub use crate::telegram::bridge::{BridgeLogger as Logger};",
        ),
        (
            "10-type-and-expression",
            include_str!("sources/10-type-and-expression.txt"),
        ),
        (
            "13-use-tree-self-chain",
            include_str!("sources/13-use-tree-self-chain.txt"),
        ),
        (
            "14-sibling-grouped-rename",
            "#[cfg(any())] use super::{bridge as b}; #[cfg(any())] use b::BridgeLogger;",
        ),
        (
            "15-nested-repeated-super",
            "mod nested { #[cfg(any())] use super::super::bridge::BridgeLogger; }",
        ),
        (
            "16-current-crate-extern-alias",
            include_str!("sources/16-current-crate-extern-alias.txt"),
        ),
        (
            "17-anonymous-import",
            "#[cfg(any())] use crate::telegram::bridge::BridgeLogger as _;",
        ),
        (
            "18-ufcs",
            "#[cfg(any())] fn probe() { let _ = <crate::telegram::bridge::BridgeLogger>::new; }",
        ),
        (
            "19-raw-grouped-alias",
            "#[cfg(any())] use crate::telegram::r#bridge::{BridgeLogger as BL};",
        ),
        (
            "20-explicit-macro-path",
            "#[cfg(any())] fn probe() { crate::telegram::bridge::forbidden_macro!(); }",
        ),
    ];

    for (label, body) in cases {
        let report = inline_spelling_report(label, body)
            .unwrap_or_else(|error| panic!("spelling {label} did not resolve: {error}"));
        assert!(
            report.dependencies.contains(&DependencyObservation {
                source: "src/telegram.rs".to_owned(),
                module: "agentscommander_lib::telegram::bridge".to_owned(),
            }),
            "spelling {label} survived the structured guard: {report:?}"
        );
    }
}

#[test]
fn cfg_disabled_out_of_line_spelling_rows_are_scanned() {
    for (label, probe) in [
        (
            "11-out-of-line-grouped",
            "use crate::telegram::{bridge::BridgeLogger};\n",
        ),
        (
            "12-out-of-line-raw-grouped",
            "use crate::telegram::{r#bridge::{BridgeLogger}};\n",
        ),
    ] {
        let report = out_of_line_spelling_report(label, probe)
            .unwrap_or_else(|error| panic!("out-of-line spelling {label} failed: {error}"));
        assert!(report.dependencies.contains(&DependencyObservation {
            source: "src/telegram/claude_watcher_layering_probe.rs".to_owned(),
            module: "agentscommander_lib::telegram::bridge".to_owned(),
        }));
    }
}

#[test]
fn recursively_nested_cfg_attr_path_variants_scan_every_alternative() {
    let fixture = Fixture::with_telegram("21-nested-cfg-attr-raw-child");
    fixture.write(
        "src/telegram.rs",
        "pub mod bridge { pub struct BridgeLogger; }\npub mod claude_watcher;\n",
    );
    fixture.write(
        "src/telegram/claude_watcher.rs",
        include_str!("sources/21-nested-cfg-attr-raw-child.txt"),
    );
    fixture.write(
        "src/telegram/claude_watcher/child.rs",
        "fn conventional() {}\n",
    );
    fixture.write(
        "src/telegram/probe_a.rs",
        "use crate::telegram::{bridge as b}; use b::BridgeLogger;\n",
    );
    fixture.write("src/telegram/probe_b.rs", "fn second_variant() {}\n");

    let index = fixture
        .index_as(CRATE_ID)
        .expect("nested cfg_attr path alternatives should all index");
    let report = analyze_module_index(&index, CRATE_ID, TARGET_MODULE)
        .expect("all conditional target descendants should scan");
    assert!(report
        .sources
        .contains("src/telegram/claude_watcher/child.rs"));
    assert!(report.sources.contains("src/telegram/probe_a.rs"));
    assert!(report.sources.contains("src/telegram/probe_b.rs"));
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/telegram/probe_a.rs".to_owned(),
        module: "agentscommander_lib::telegram::bridge".to_owned(),
    }));
}

#[test]
fn path_attribute_matrix_handles_rebases_and_rejects_invalid_combinations() {
    let direct = Fixture::new("direct-path-outside-inline");
    direct.write(
        "src/lib.rs",
        "#[path = \"alternate/telegram.rs\"] mod telegram;\n",
    );
    direct.write("src/alternate/telegram.rs", "pub mod claude_watcher {}\n");
    assert!(direct
        .index()
        .expect("direct path should resolve")
        .declared_modules
        .contains("fixture::telegram::claude_watcher"));

    for (label, outer_source, child_source) in [
        (
            "inline-path-lib-owner",
            "#[path = \"rebased\"] mod outer { mod child; }\n",
            "src/rebased/child.rs",
        ),
        (
            "inline-non-mod-owner",
            "mod outer;\n",
            "src/outer/inline/child.rs",
        ),
        (
            "inline-mod-owner",
            "mod outer;\n",
            "src/outer/inline/child.rs",
        ),
    ] {
        let fixture = Fixture::new(label);
        fixture.write("src/lib.rs", outer_source);
        if label == "inline-non-mod-owner" {
            fixture.write("src/outer.rs", "mod inline { mod child; }\n");
        }
        if label == "inline-mod-owner" {
            fixture.write("src/outer/mod.rs", "mod inline { mod child; }\n");
        }
        fixture.write(child_source, "fn discovered() {}\n");
        fixture
            .index()
            .unwrap_or_else(|error| panic!("inline path fixture {label} did not resolve: {error}"));
    }

    let combined = Fixture::new("direct-plus-conditional");
    combined.write(
        "src/lib.rs",
        "#[path = \"direct.rs\"] #[cfg_attr(any(), path = \"conditional.rs\")] mod child;\n",
    );
    combined.write("src/direct.rs", "");
    combined.write("src/conditional.rs", "");
    let combined_error = combined
        .index()
        .expect_err("direct and conditional path combination must fail")
        .to_string();
    assert!(combined_error.contains("combines a direct path"));

    let missing = Fixture::new("missing-conditional-variant");
    missing.write(
        "src/lib.rs",
        "#[cfg_attr(any(), path = \"missing.rs\")] mod child;\n",
    );
    missing.write("src/child.rs", "");
    let missing_error = missing
        .index()
        .expect_err("every conditional path variant must exist")
        .to_string();
    assert!(missing_error.contains("conditional path variant"));
    assert!(missing_error.contains("missing.rs"));
}

#[test]
fn inert_text_and_macro_tokens_do_not_create_bridge_dependencies() {
    let rows = [
        ("line-comment", "// crate::telegram::bridge::BridgeLogger"),
        (
            "block-comment",
            "/* crate::telegram::bridge::BridgeLogger */",
        ),
        (
            "normal-string",
            "#[cfg(any())] const TEXT: &str = \"crate::telegram::bridge::BridgeLogger\";",
        ),
        (
            "raw-string",
            "#[cfg(any())] const TEXT: &str = r#\"crate::telegram::bridge::BridgeLogger\"#;",
        ),
        (
            "stringify-tokens",
            include_str!("sources/stringify-tokens.txt"),
        ),
    ];
    for (label, body) in rows {
        let report = inline_spelling_report(label, body)
            .unwrap_or_else(|error| panic!("negative row {label} failed to analyze: {error}"));
        assert!(
            report
                .dependencies
                .iter()
                .all(|dependency| dependency.module != "agentscommander_lib::telegram::bridge"),
            "negative row {label} created a false bridge dependency: {report:?}"
        );
    }
}

#[test]
fn alias_conflicts_cycles_and_globs_fail_closed() {
    for (label, body, marker) in [
        (
            "alias-conflict",
            include_str!("sources/alias-conflict.txt"),
            "lexical alias conflict",
        ),
        (
            "alias-cycle",
            "#[cfg(any())] use b as a; #[cfg(any())] use a as b;",
            "alias fixed point",
        ),
        (
            "glob-import",
            "#[cfg(any())] use crate::telegram::bridge::*;",
            "glob import",
        ),
    ] {
        let message = inline_spelling_report(label, body)
            .expect_err("unsupported alias shape must fail closed")
            .to_string();
        assert!(message.contains(marker), "row {label}: {message}");
        assert!(message.contains(FOCUSED_RERUN));
    }
}
