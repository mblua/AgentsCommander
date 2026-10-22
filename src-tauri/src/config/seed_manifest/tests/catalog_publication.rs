use super::*;

// -----------------------------------------------------------------------
// #1968 - the narrow read-only catalog-publication query.
// -----------------------------------------------------------------------

fn catalog_publication_row() -> Result<PublishedManifestRow, SeedManifestError> {
    PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/coding-agents/agents.json".to_string(),
        )?,
        timestamp("2026-09-12T13:30:00.000Z"),
    )
}

fn ac_dir_entries(project: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(project.join(".ac"))
        .expect("read .ac")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    names
}

#[test]
fn managed_catalog_has_catalog_publication_missing_manifest_is_false_and_creates_nothing() {
    let (_temp, project) = setup_project();
    let entries_before = ac_dir_entries(&project);
    assert!(!has_catalog_publication(&project).expect("query missing manifest"));
    assert!(
        !canonical_path(&project).exists(),
        "the query never creates"
    );
    assert_eq!(entries_before, ac_dir_entries(&project));
}

#[test]
fn managed_catalog_has_catalog_publication_matches_only_the_exact_canonical_row() {
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    guard.release();
    assert!(
        !has_catalog_publication(&project).expect("context-only manifest"),
        "a context row is not a catalog publication"
    );

    let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
    let entries_before = ac_dir_entries(&project);
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            catalog_publication_row().unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    assert!(has_catalog_publication(&project).expect("catalog row"));

    // Repeated reads change neither bytes nor the directory entry set.
    let bytes_after = std::fs::read(canonical_path(&project)).unwrap();
    let entries_after = ac_dir_entries(&project);
    for _ in 0..3 {
        assert!(has_catalog_publication(&project).expect("repeat query"));
    }
    assert_ne!(bytes_before, bytes_after, "the row was recorded");
    assert_eq!(
        std::fs::read(canonical_path(&project)).unwrap(),
        bytes_after
    );
    assert_eq!(entries_before, entries_after);
}

#[test]
fn managed_catalog_has_catalog_publication_malformed_future_and_directory_are_err() {
    fn inventory(
        root: &std::path::Path,
    ) -> Vec<(
        std::path::PathBuf,
        bool,
        u64,
        bool,
        std::time::SystemTime,
        Vec<u8>,
    )> {
        #[cfg(test)]
        type TreeInventoryRows = Vec<(
            std::path::PathBuf,
            bool,
            u64,
            bool,
            std::time::SystemTime,
            Vec<u8>,
        )>;
        fn visit(root: &std::path::Path, path: &std::path::Path, rows: &mut TreeInventoryRows) {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            rows.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                metadata.is_dir(),
                metadata.len(),
                metadata.permissions().readonly(),
                metadata.modified().unwrap(),
                if metadata.is_file() {
                    std::fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
            ));
            if metadata.is_dir() {
                let mut entries: Vec<_> = std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect();
                entries.sort();
                for entry in entries {
                    visit(root, &entry, rows);
                }
            }
        }
        let mut rows = Vec::new();
        visit(root, root, &mut rows);
        rows
    }
    let (_temp, project) = setup_project();
    std::fs::write(canonical_path(&project), b"not = [valid toml").unwrap();
    assert!(has_catalog_publication(&project).is_err());

    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        "schema_version = 99\ncoverage_version = 99\ncoverage = []\nfiles = []\n",
    )
    .unwrap();
    assert!(has_catalog_publication(&project).is_err());

    let (_temp, project) = setup_project();
    std::fs::create_dir(canonical_path(&project)).unwrap();
    std::fs::write(canonical_path(&project).join("keep.txt"), b"keep").unwrap();
    let unrelated = project.join(".ac").join("unrelated-target.toml");
    std::fs::write(
        &unrelated,
        b"# unrelated target
",
    )
    .unwrap();
    let before = inventory(&project);
    assert!(has_catalog_publication(&project).is_err());
    for _ in 0..3 {
        let error = has_catalog_publication(&project).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&canonical_path(&project).display().to_string()),
            "{error}"
        );
        assert_eq!(inventory(&project), before);
    }
    assert_eq!(
        std::fs::read(canonical_path(&project).join("keep.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        std::fs::read(unrelated).unwrap(),
        b"# unrelated target
"
    );
}

#[test]
fn managed_catalog_has_catalog_publication_reads_under_a_held_writer_lock() {
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        catalog_publication_row().unwrap(),
    );
    // The writer still holds the sidecar lock; the query takes NO lock, so
    // it must see the committed bytes and never block once the guard drops.
    let started = Instant::now();
    assert!(has_catalog_publication(&project).expect("read under held writer"));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the read must not wait on the writer lock"
    );
    let bytes = std::fs::read(canonical_path(&project)).unwrap();
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), bytes);
    assert!(has_catalog_publication(&project).expect("read after restart"));
}

/// Raw coverage-v2 manifest text carrying exactly one `[[files]]` row, used
/// to feed the real parser combinations the row constructors reject.
fn raw_manifest_with_row(
    path: &str,
    encoding: &str,
    kind: &str,
    scope: &str,
    source: &str,
) -> String {
    format!(
        concat!(
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \
                \"coding_agent_catalog\"]\n",
            "\n",
            "[[files]]\n",
            "path = \"{path}\"\n",
            "path_encoding = \"{encoding}\"\n",
            "kind = \"{kind}\"\n",
            "scope = \"{scope}\"\n",
            "source = \"{source}\"\n",
            "last_seeded_at = \"2026-09-12T13:30:00.000Z\"\n",
        ),
        path = path,
        encoding = encoding,
        kind = kind,
        scope = scope,
        source = source,
    )
}

#[cfg(windows)]
fn symlink_file_at(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn symlink_file_at(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[test]
fn managed_catalog_has_catalog_publication_wrong_rows_are_err_or_false_without_writes() {
    let cases: [(&str, &str, &str, &str, &str, bool); 5] = [
        // Correct control: exact path, utf8, catalog kind, canonical scope, builtin.
        (
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
            true,
        ),
        // Wrong path.
        (
            ".ac/coding-agents/agents-list.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
            false,
        ),
        // Wrong scope.
        (
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:other",
            "builtin",
            false,
        ),
        // Wrong source.
        (
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "user",
            false,
        ),
        // Wrong encoding.
        (
            ".ac/coding-agents/agents.json",
            "unix_bytes_hex",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
            false,
        ),
    ];
    for (path, encoding, kind, scope, source, expect_true) in cases {
        let (_temp, project) = setup_project();
        std::fs::write(
            canonical_path(&project),
            raw_manifest_with_row(path, encoding, kind, scope, source),
        )
        .unwrap();
        let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
        let entries_before = ac_dir_entries(&project);
        let result = has_catalog_publication(&project);
        if expect_true {
            assert!(
                result.expect("the correct control parses"),
                "{path} {scope} {source}"
            );
        } else {
            assert!(
                result.is_err(),
                "expected Err for {path} {scope} {source}: {result:?}"
            );
        }
        assert_eq!(
            std::fs::read(canonical_path(&project)).unwrap(),
            bytes_before
        );
        assert_eq!(ac_dir_entries(&project), entries_before);
    }

    // Wrong kind: a well-formed context row parses, but it never matches
    // the catalog identity, so the honest answer is `false` (no proof).
    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        raw_manifest_with_row(
            ".ac/Context.AgentsCommander.md",
            "utf8",
            "project_context_template",
            "context:agentscommander",
            "builtin",
        ),
    )
    .unwrap();
    let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
    let entries_before = ac_dir_entries(&project);
    assert!(!has_catalog_publication(&project).expect("the context row parses"));
    assert_eq!(
        std::fs::read(canonical_path(&project)).unwrap(),
        bytes_before
    );
    assert_eq!(ac_dir_entries(&project), entries_before);
}

#[test]
fn managed_catalog_has_catalog_publication_link_is_err_without_writes() {
    let (_temp, project) = setup_project();
    let target = project.join(".ac").join("real-manifest-target.toml");
    std::fs::write(&target, b"# fixture target\n").unwrap();
    symlink_file_at(&target, &canonical_path(&project))
        .expect("real manifest symlink fixture (not a silent skip)");
    let entries_before = ac_dir_entries(&project);
    let target_before = std::fs::read(&target).unwrap();
    assert!(has_catalog_publication(&project).is_err());
    assert!(std::fs::symlink_metadata(canonical_path(&project))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(ac_dir_entries(&project), entries_before);
    assert_eq!(std::fs::read(&target).unwrap(), target_before);
}

#[cfg(windows)]
#[test]
fn managed_catalog_has_catalog_publication_share_denied_read_is_err_without_writes() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_WRITE};

    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        raw_manifest_with_row(
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
        ),
    )
    .unwrap();
    let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
    let entries_before = ac_dir_entries(&project);
    // A real handle that denies READ sharing: the query's own read open
    // fails with a real sharing violation (an OS error, not an injection).
    let blocker = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(canonical_path(&project))
        .unwrap();
    let result = has_catalog_publication(&project);
    assert!(result.is_err(), "{result:?}");
    assert_eq!(ac_dir_entries(&project), entries_before);
    drop(blocker);
    assert_eq!(
        std::fs::read(canonical_path(&project)).unwrap(),
        bytes_before
    );
}
