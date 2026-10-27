use super::*;
use crate::config::settings::AppSettings;
use std::path::Path;

// -- symlink test helpers (Windows needs SeCreateSymbolicLinkPrivilege) --

#[cfg(unix)]
fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(windows)]
fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}
#[cfg(unix)]
fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(windows)]
fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, contents).expect("write file");
}

fn seed_agency_cache(config: &Path, body: &str) {
    write(
        &agency_templates_dir(config)
            .join("testing")
            .join("accessibility-auditor")
            .join("Role.md"),
        body,
    );
    let manifest = AgencyTemplatesManifest {
        repo: "https://github.com/msitarzewski/agency-agents".into(),
        reference: "main".into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        template_count: 1,
    };
    write(
        &agency_manifest_path(config),
        &serde_json::to_string_pretty(&manifest).expect("manifest json"),
    );
}

#[test]
fn missing_agency_cache_lists_no_agency_templates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(agency_template_metas_from_cache(tmp.path()).is_empty());
    let status = agency_templates_status(tmp.path());
    assert!(!status.available);
    assert_eq!(status.reason.as_deref(), Some("missing"));
}

#[test]
fn valid_agency_cache_lists_metadata() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_agency_cache(
        tmp.path(),
        "---\nname: Accessibility Auditor\ndescription: Finds a11y issues\ncolor: teal\n---\n\n# \
        Body\n",
    );
    let list = agency_template_metas_from_cache(tmp.path());
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "agency:testing-accessibility-auditor");
    assert_eq!(list[0].source, "agency");
    assert_eq!(list[0].name, "Accessibility Auditor");
    assert_eq!(list[0].description, "Finds a11y issues");
    assert_eq!(list[0].category, "Testing");
    assert_eq!(list[0].color.as_deref(), Some("teal"));
    assert!(!list[0].has_skills);
}

#[test]
fn invalid_agency_manifest_returns_unavailable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_agency_cache(tmp.path(), "# Body\n");
    write(&agency_manifest_path(tmp.path()), "{not json");
    assert!(agency_template_metas_from_cache(tmp.path()).is_empty());
    let status = agency_templates_status(tmp.path());
    assert!(!status.available);
    assert_eq!(status.reason.as_deref(), Some("manifestMalformed"));
}

#[test]
fn malformed_agency_commit_returns_unavailable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_agency_cache(tmp.path(), "# Body\n");
    let manifest = AgencyTemplatesManifest {
        repo: "https://github.com/msitarzewski/agency-agents".into(),
        reference: "main".into(),
        commit: "not-a-sha".into(),
        template_count: 1,
    };
    write(
        &agency_manifest_path(tmp.path()),
        &serde_json::to_string(&manifest).expect("manifest json"),
    );
    let status = agency_templates_status(tmp.path());
    assert!(!status.available);
    assert_eq!(status.reason.as_deref(), Some("invalidCommit"));
}

#[test]
fn resolve_unknown_agency_id_errs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let r = resolve_role_template("agency:nope", &AppSettings::default(), tmp.path());
    assert!(r.is_err(), "unknown agency id must error");
}

#[test]
fn resolve_cached_agency_id_ok() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_agency_cache(tmp.path(), "---\nname: A\n---\n\n# Cached Body\n");
    let r = resolve_role_template(
        "agency:testing-accessibility-auditor",
        &AppSettings::default(),
        tmp.path(),
    )
    .expect("known agency id must resolve");
    assert!(!r.body.trim().is_empty(), "resolved body must be non-empty");
    assert!(r.body.contains("Cached Body"));
    assert!(r.skills_src.is_none(), "agency templates ship no skills");
}

#[test]
fn duplicate_agency_ids_reject_cache() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        &agency_templates_dir(tmp.path())
            .join("testing")
            .join("accessibility-auditor")
            .join("Role.md"),
        "# A\n",
    );
    write(
        &agency_templates_dir(tmp.path())
            .join("testing")
            .join("accessibility auditor")
            .join("Role.md"),
        "# B\n",
    );
    let manifest = AgencyTemplatesManifest {
        repo: "https://github.com/msitarzewski/agency-agents".into(),
        reference: "main".into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        template_count: 2,
    };
    write(
        &agency_manifest_path(tmp.path()),
        &serde_json::to_string(&manifest).expect("manifest json"),
    );
    assert!(load_cached_agency_templates(tmp.path()).is_err());
}

#[test]
fn reserved_agency_body_delimiters_reject_validation() {
    let err =
        validate_role_template_body("agency:x", "# Body\n<!-- ac:role-profile\n").unwrap_err();
    assert!(err.contains("reserved"));
}

// -- 6: local resolution with/without skills --

#[test]
fn resolve_local_id_ok_with_and_without_skills() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().join("agent-templates");
    write(
        &base.join("tmpl-a").join("Role.md"),
        "# A\n\nRole A body.\n",
    );
    write(
        &base.join("tmpl-b").join("Role.md"),
        "# B\n\nRole B body.\n",
    );
    write(&base.join("tmpl-b").join("skills").join("x.md"), "skill\n");

    let s = AppSettings::default();
    let a = resolve_role_template("local:tmpl-a", &s, tmp.path()).expect("tmpl-a resolves");
    assert!(a.skills_src.is_none(), "tmpl-a has no skills/");
    let b = resolve_role_template("local:tmpl-b", &s, tmp.path()).expect("tmpl-b resolves");
    assert!(b.skills_src.is_some(), "tmpl-b ships skills/");
}

// -- 7: path traversal --

#[test]
fn resolve_local_id_traversal_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("agent-templates")).expect("mk dir");
    let r = resolve_role_template("local:../../evil", &AppSettings::default(), tmp.path());
    assert!(r.is_err(), "a traversal id must not resolve to any entry");
}

// -- 8-9: frontmatter stripping --

#[test]
fn resolve_local_id_strips_frontmatter() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        &tmp.path()
            .join("agent-templates")
            .join("fm")
            .join("Role.md"),
        "---\nname: FM\ndescription: D\n---\n\n# Body\n\nReal content.\n",
    );
    let r = resolve_role_template("local:fm", &AppSettings::default(), tmp.path())
        .expect("fm resolves");
    assert!(
        !r.body.starts_with("---"),
        "frontmatter must be stripped from the body: {:?}",
        r.body
    );
    assert!(r.body.contains("Real content."));
}

#[test]
fn resolve_local_id_no_frontmatter_body_unchanged() {
    // A body that opens with a Markdown `---` horizontal rule but has NO
    // real frontmatter must keep that rule.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        &tmp.path()
            .join("agent-templates")
            .join("hr")
            .join("Role.md"),
        "# Heading\n\n---\n\nText after a horizontal rule.\n",
    );
    let r = resolve_role_template("local:hr", &AppSettings::default(), tmp.path())
        .expect("hr resolves");
    assert!(
        r.body.contains("---"),
        "a markdown horizontal rule must not be mis-stripped: {:?}",
        r.body
    );
}

#[test]
fn strip_yaml_frontmatter_wrapper_delegates_to_session_context_helper() {
    let with_frontmatter = "---\nname: Wrapper\ndescription: D\n---\n\n# Body\n\nContent.\n";
    let without_frontmatter = "# Heading\n\n---\n\nText after a horizontal rule.\n";
    assert_eq!(
        strip_yaml_frontmatter_for_role_template(with_frontmatter),
        crate::config::session_context::strip_yaml_frontmatter(with_frontmatter)
    );
    assert_eq!(
        strip_yaml_frontmatter_for_role_template(without_frontmatter),
        crate::config::session_context::strip_yaml_frontmatter(without_frontmatter)
    );
}

// -- 10-11: link safety --

#[test]
fn resolve_local_id_symlinked_role_md_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let secret = tmp.path().join("secret.txt");
    write(&secret, "TOP SECRET\n");
    let tmpl = tmp.path().join("agent-templates").join("evil");
    std::fs::create_dir_all(&tmpl).expect("mk template dir");
    // Symlink the template's Role.md at an outside file.
    let Ok(()) = try_symlink_file(&secret, &tmpl.join("Role.md")) else {
        return; // no symlink privilege — skip, not fail.
    };
    let r = resolve_role_template("local:evil", &AppSettings::default(), tmp.path());
    assert!(r.is_err(), "a symlinked Role.md must be rejected");
    if let Ok(t) = r {
        assert!(!t.body.contains("TOP SECRET"), "outside file leaked");
    }
}

#[test]
fn resolve_local_id_symlinked_skills_ignored() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tmp.path().join("outside-skills");
    write(&outside.join("leak.md"), "leaked\n");
    let tmpl = tmp.path().join("agent-templates").join("ls");
    write(&tmpl.join("Role.md"), "# LS\n\nBody.\n");
    let Ok(()) = try_symlink_dir(&outside, &tmpl.join("skills")) else {
        return; // no symlink privilege — skip, not fail.
    };
    let r = resolve_role_template("local:ls", &AppSettings::default(), tmp.path())
        .expect("ls resolves (symlinked skills just ignored)");
    assert!(
        r.skills_src.is_none(),
        "a symlinked skills/ must be treated as absent"
    );
}

// -- 12: path resolution variants --

#[test]
fn resolve_local_templates_dir_variants() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = tmp.path();

    // unset ⇒ Default
    let unset = AppSettings::default();
    assert!(matches!(
        resolve_local_templates_dir(&unset, config),
        LocalTemplatesDir::Default(_)
    ));

    // relative override pointing at an existing dir ⇒ Custom(config/<rel>)
    std::fs::create_dir_all(config.join("rel-templates")).expect("mk rel dir");
    let rel = AppSettings {
        agent_templates_path: Some("rel-templates".into()),
        ..AppSettings::default()
    };
    match resolve_local_templates_dir(&rel, config) {
        LocalTemplatesDir::Custom(p) => assert_eq!(p, config.join("rel-templates")),
        other => panic!("expected Custom, got {:?}", other),
    }

    // absolute existing dir ⇒ Custom
    let abs_dir = tmp.path().join("abs-templates");
    std::fs::create_dir_all(&abs_dir).expect("mk abs dir");
    let abs = AppSettings {
        agent_templates_path: Some(abs_dir.to_string_lossy().to_string()),
        ..AppSettings::default()
    };
    assert!(matches!(
        resolve_local_templates_dir(&abs, config),
        LocalTemplatesDir::Custom(_)
    ));

    // non-existent override ⇒ InvalidCustom
    let bad = AppSettings {
        agent_templates_path: Some("does-not-exist-xyz".into()),
        ..AppSettings::default()
    };
    assert!(matches!(
        resolve_local_templates_dir(&bad, config),
        LocalTemplatesDir::InvalidCustom { .. }
    ));
}

// -- 13-14: default dir seeding --

#[test]
fn ensure_default_templates_dir_creates_dir_and_readme() {
    let tmp = tempfile::tempdir().expect("tempdir");
    ensure_default_templates_dir(tmp.path()).expect("seed default dir");
    assert!(tmp.path().join("agent-templates").is_dir());
    assert!(tmp
        .path()
        .join("agent-templates")
        .join("README.md")
        .is_file());
}

#[test]
fn ensure_default_templates_dir_preserves_existing_readme() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let readme = tmp.path().join("agent-templates").join("README.md");
    write(&readme, "SENTINEL — user edited\n");
    ensure_default_templates_dir(tmp.path()).expect("seed default dir");
    let after = std::fs::read_to_string(&readme).expect("read readme");
    assert_eq!(
        after, "SENTINEL — user edited\n",
        "README must not be overwritten"
    );
}

// -- 15: combined collection --

#[test]
fn collect_role_templates_includes_cached_agency_and_local() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_agency_cache(tmp.path(), "# Agency\n\nBody.\n");
    write(
        &tmp.path()
            .join("agent-templates")
            .join("mine")
            .join("Role.md"),
        "# Mine\n\nLocal body.\n",
    );
    let all = collect_role_templates(&AppSettings::default(), tmp.path());
    assert!(
        all.iter().any(|t| t.source == "agency"),
        "seeded agency templates must be present"
    );
    let local = all
        .iter()
        .find(|t| t.id == "local:mine")
        .expect("the local template must be listed");
    assert_eq!(local.source, "local");
    assert_eq!(local.category, "Local");
}

// -- 16: local sort order --

#[test]
fn collect_local_templates_sorted_by_name() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().join("t");
    for folder in ["zeta", "alpha", "Mike"] {
        write(&base.join(folder).join("Role.md"), "# x\n\nbody\n");
    }
    let list = collect_local_templates(&base);
    let names: Vec<String> = list.iter().map(|t| t.name.to_lowercase()).collect();
    assert_eq!(names, vec!["alpha", "mike", "zeta"], "must be name-sorted");
}

// -- 17: BOM --

#[test]
fn parse_template_frontmatter_strips_bom() {
    let (name, desc) =
        parse_template_frontmatter("\u{FEFF}---\nname: Foo\ndescription: Bar\n---\n\nbody\n");
    assert_eq!(name.as_deref(), Some("Foo"));
    assert_eq!(desc.as_deref(), Some("Bar"));
}

// -- 18-20: recursive copy --

#[test]
fn copy_dir_recursive_copies_nested_tree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("src");
    write(&src.join("a.md"), "a\n");
    write(&src.join("nested").join("b.md"), "b\n");
    let dst = tmp.path().join("dst");
    let failures = copy_dir_recursive(&src, &dst);
    assert!(failures.is_empty(), "clean copy must report no failures");
    assert!(dst.join("a.md").is_file());
    assert!(dst.join("nested").join("b.md").is_file());
}

#[test]
fn copy_dir_recursive_skips_symlink_entries() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tmp.path().join("outside");
    write(&outside.join("leak.md"), "leak\n");
    let src = tmp.path().join("src");
    write(&src.join("real.md"), "real\n");
    let Ok(()) = try_symlink_dir(&outside, &src.join("linked")) else {
        return; // no symlink privilege — skip, not fail.
    };
    let dst = tmp.path().join("dst");
    let _ = copy_dir_recursive(&src, &dst);
    assert!(dst.join("real.md").is_file(), "real files copied");
    assert!(
        !dst.join("linked").exists(),
        "a symlink entry must never be copied"
    );
}

#[test]
fn copy_dir_recursive_skips_symlink_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let real = tmp.path().join("real");
    write(&real.join("f.md"), "f\n");
    let link = tmp.path().join("link");
    let Ok(()) = try_symlink_dir(&real, &link) else {
        return; // no symlink privilege — skip, not fail.
    };
    let dst = tmp.path().join("dst");
    let failures = copy_dir_recursive(&link, &dst);
    assert!(
        !failures.is_empty(),
        "a symlinked source root must be rejected"
    );
    assert!(
        !dst.join("f.md").exists(),
        "nothing must be copied from a symlinked root"
    );
}

#[test]
fn manifest_codecs_preserve_pretty_bytes_and_raw_read_contract() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manifest = AgencyTemplatesManifest {
        repo: "https://example.test/雪\n\"quoted\"".into(),
        reference: r"réf\branch".into(),
        commit: "not-a-sha".into(),
        template_count: 7,
    };
    let expected = r#"{
  "repo": "https://example.test/雪\n\"quoted\"",
  "ref": "réf\\branch",
  "commit": "not-a-sha",
  "templateCount": 7
}"#;
    write_staged_agency_manifest(tmp.path(), &manifest).expect("staged write");
    let path = tmp.path().join(AGENCY_MANIFEST_FILE);
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    write_normalized_agency_manifest(tmp.path(), &manifest).expect("normalized write");
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    for read in [read_staged_agency_manifest, read_cached_agency_manifest] {
        let actual = read(tmp.path()).expect("raw reader accepts invalid commit");
        assert_eq!(actual.repo, manifest.repo);
        assert_eq!(actual.reference, manifest.reference);
        assert_eq!(actual.commit, manifest.commit);
        assert_eq!(actual.template_count, manifest.template_count);
    }
}

#[test]
fn manifest_raw_read_errors_keep_context_and_local_error_text() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for root in [tmp.path().join("missing"), tmp.path().to_path_buf()] {
        let error = std::fs::read_to_string(root.join(AGENCY_MANIFEST_FILE)).unwrap_err();
        assert_eq!(
            read_staged_agency_manifest(&root).unwrap_err(),
            format!("Failed to read staged manifest: {error}")
        );
        assert_eq!(
            read_cached_agency_manifest(&root).unwrap_err(),
            format!("Failed to read cached manifest: {error}")
        );
    }
    let path = tmp.path().join(AGENCY_MANIFEST_FILE);
    std::fs::write(&path, [255]).unwrap();
    let error = std::fs::read_to_string(&path).unwrap_err();
    assert_eq!(
        read_staged_agency_manifest(tmp.path()).unwrap_err(),
        format!("Failed to read staged manifest: {error}")
    );
    assert_eq!(
        read_cached_agency_manifest(tmp.path()).unwrap_err(),
        format!("Failed to read cached manifest: {error}")
    );
    std::fs::write(&path, "{bad").unwrap();
    let error = serde_json::from_str::<AgencyTemplatesManifest>("{bad").unwrap_err();
    assert_eq!(
        read_staged_agency_manifest(tmp.path()).unwrap_err(),
        format!("Failed to parse staged manifest: {error}")
    );
    assert_eq!(
        read_cached_agency_manifest(tmp.path()).unwrap_err(),
        format!("Failed to parse cached manifest: {error}")
    );
}

#[test]
fn manifest_writers_preserve_contextual_directory_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    seed_agency_cache(tmp.path(), "# Body\n");
    let manifest = read_agency_manifest(tmp.path()).unwrap().unwrap();
    let root = tmp.path().join("write-errors");
    std::fs::create_dir_all(root.join(AGENCY_MANIFEST_FILE)).unwrap();
    let encoded = serde_json::to_string_pretty(&manifest).unwrap();
    let error = std::fs::write(root.join(AGENCY_MANIFEST_FILE), encoded).unwrap_err();
    assert_eq!(
        write_staged_agency_manifest(&root, &manifest).unwrap_err(),
        format!("Failed to write staged manifest: {error}")
    );
    assert_eq!(
        write_normalized_agency_manifest(&root, &manifest).unwrap_err(),
        format!("Failed to write Agency manifest: {error}")
    );
}

#[test]
fn manifest_validated_reader_keeps_metadata_and_commit_rejections() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(read_agency_manifest(tmp.path()).unwrap().is_none());
    let root = agency_templates_dir(tmp.path());
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(
        read_agency_manifest(tmp.path()).unwrap_err(),
        "manifestMissing"
    );
    let path = root.join(AGENCY_MANIFEST_FILE);
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        read_agency_manifest(tmp.path()).unwrap_err(),
        "manifestMissing"
    );
    std::fs::remove_dir(&path).unwrap();
    seed_agency_cache(tmp.path(), "# Body\n");
    let mut manifest = read_staged_agency_manifest(&root).unwrap();
    manifest.commit = "not-a-sha".into();
    write_staged_agency_manifest(&root, &manifest).unwrap();
    assert_eq!(
        read_cached_agency_manifest(&root).unwrap().commit,
        "not-a-sha"
    );
    assert_eq!(
        read_agency_manifest(tmp.path()).unwrap_err(),
        "invalidCommit"
    );
    assert_eq!(
        agency_templates_status(tmp.path()).reason.as_deref(),
        Some("invalidCommit")
    );
    std::fs::write(&path, [255]).unwrap();
    assert_eq!(
        read_agency_manifest(tmp.path()).unwrap_err(),
        "manifestMalformed"
    );
}
