use super::*;

// Stage E (#1064) Git-visibility conformance (plan section 10.5 items 3-4,
// section 7.3). AC's managed `.ac/.gitignore` ignores the manifest, but a
// parent rule that excludes the `.ac/` directory still wins: Git does not
// descend into an excluded directory. AC documents but does not override
// this.
#[test]
fn stage_e_parent_gitignore_excluding_ac_hides_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");
    std::fs::write(ac_root.join("seed-manifest.toml"), b"fixture").expect("write manifest");
    // A parent repository rule excludes the whole `.ac/` directory.
    std::fs::write(project.join(".gitignore"), b"/.ac/\n").expect("write parent .gitignore");

    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init");
    assert!(init.success(), "git init must succeed");

    let verbose = std::process::Command::new("git")
        .args([
            "check-ignore",
            "-v",
            "--no-index",
            "--",
            ".ac/seed-manifest.toml",
        ])
        .current_dir(&project)
        .output()
        .expect("git check-ignore");
    assert!(
        verbose.status.success(),
        "the manifest must be ignored when the parent excludes .ac/: {}",
        String::from_utf8_lossy(&verbose.stderr)
    );
    let line = String::from_utf8(verbose.stdout).expect("utf8");
    assert!(
        line.contains(".gitignore") && line.contains("/.ac/"),
        "check-ignore -v must name the parent .ac/ rule as the winner, got {line:?}"
    );
}

#[test]
fn stage_e_later_user_rule_and_excludes_win_over_managed_rules() {
    // (a) A later user negation in the SAME .ac/.gitignore re-includes the manifest.
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(&ac_root).expect("create .ac");
        ensure_ac_root_gitignore_with_names(&ac_root, &floor_names()).expect("ensure .gitignore");
        // Append a later user negation (never reordered by AC).
        let mut content =
            std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
        content.push_str("\n# user rule\n!/seed-manifest.toml\n");
        std::fs::write(ac_root.join(".gitignore"), content).expect("append user rule");
        std::fs::write(ac_root.join("seed-manifest.toml"), b"x").expect("manifest");

        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .status()
            .expect("git init");
        assert!(init.success());
        let verbose = std::process::Command::new("git")
            .args([
                "check-ignore",
                "-v",
                "--non-matching",
                "--no-index",
                "--",
                ".ac/seed-manifest.toml",
            ])
            .current_dir(&project)
            .output()
            .expect("check-ignore");
        assert!(
            verbose.status.success(),
            "check-ignore -v --non-matching must name the winning rule"
        );
        let line = String::from_utf8(verbose.stdout).expect("utf8");
        assert!(
            line.contains(".ac/.gitignore") && line.contains("!/seed-manifest.toml"),
            "a later same-file user negation must re-include the manifest, got {line:?}"
        );
        let ignored = std::process::Command::new("git")
            .args([
                "check-ignore",
                "--quiet",
                "--no-index",
                "--",
                ".ac/seed-manifest.toml",
            ])
            .current_dir(&project)
            .status()
            .expect("check-ignore");
        assert!(
            !ignored.success(),
            "the manifest must not be ignored after the user negation, got exit {:?}",
            ignored.code()
        );
    }

    // (b) A repository exclude of the parent .ac/ dir hides the manifest.
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(&ac_root).expect("create .ac");
        ensure_ac_root_gitignore_with_names(&ac_root, &floor_names()).expect("ensure .gitignore");
        std::fs::write(ac_root.join("seed-manifest.toml"), b"x").expect("manifest");
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .status()
            .expect("git init");
        assert!(init.success());
        std::fs::write(
            project.join(".git").join("info").join("exclude"),
            b"/.ac/\n",
        )
        .expect("write repo exclude");
        let verbose = std::process::Command::new("git")
            .args([
                "check-ignore",
                "-v",
                "--no-index",
                "--",
                ".ac/seed-manifest.toml",
            ])
            .current_dir(&project)
            .output()
            .expect("check-ignore");
        assert!(
            verbose.status.success(),
            "a repository exclude of .ac/ must hide the manifest"
        );
        let line = String::from_utf8(verbose.stdout).expect("utf8");
        assert!(
            line.contains("info") && line.contains("exclude"),
            "check-ignore -v must name the repository exclude, got {line:?}"
        );
    }

    // (c) A global excludes file excluding the parent .ac/ dir hides it.
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(&ac_root).expect("create .ac");
        ensure_ac_root_gitignore_with_names(&ac_root, &floor_names()).expect("ensure .gitignore");
        std::fs::write(ac_root.join("seed-manifest.toml"), b"x").expect("manifest");
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .status()
            .expect("git init");
        assert!(init.success());
        let global = project.join("global-excludes");
        std::fs::write(&global, b"/.ac/\n").expect("write global excludes");
        let override_arg = format!(
            "core.excludesFile={}",
            global.to_string_lossy().replace('\\', "/")
        );
        let verbose = std::process::Command::new("git")
            .arg("-c")
            .arg(&override_arg)
            .args([
                "check-ignore",
                "-v",
                "--no-index",
                "--",
                ".ac/seed-manifest.toml",
            ])
            .current_dir(&project)
            .output()
            .expect("check-ignore");
        assert!(
            verbose.status.success(),
            "a global exclude of .ac/ must hide the manifest"
        );
        let line = String::from_utf8(verbose.stdout).expect("utf8");
        assert!(
            line.contains("global-excludes"),
            "check-ignore -v must name the global excludes file, got {line:?}"
        );
    }
}

#[test]
fn ensure_ac_root_gitignore_appends_team_config_lock_block_preserving_bytes_and_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    let original = b"# User-authored rules\r\n!important.txt\r\n\r\n# AgentsCommander: exclude room cloned repos from parent git tracking.\r\n# Without this, parent repo operations (checkout, reset) corrupt child clones.\r\nwg-*/\r\n"
        .to_vec();
    std::fs::write(&gitignore_path, &original).expect("write .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let updated = std::fs::read(&gitignore_path).expect("read updated .gitignore");
    assert!(
        updated.starts_with(&original),
        "workspace .gitignore must preserve the original bytes as an exact prefix"
    );
    let updated_text = std::str::from_utf8(&updated).expect("updated .gitignore is UTF-8");
    let block =
        "# AgentsCommander: exclude team-config coordination files.\n/.team-config-write.lock";
    assert_eq!(
        updated_text.matches(block).count(),
        1,
        "workspace .gitignore must append the exact team-config lock block once"
    );
    assert_eq!(
        updated_text
            .lines()
            .filter(|line| *line == "/.team-config-write.lock")
            .count(),
        1,
        "workspace .gitignore must contain the anchored team-config lock pattern once"
    );

    let once_updated = updated;
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore again");
    let twice_updated = std::fs::read(&gitignore_path).expect("read .gitignore again");
    assert_eq!(
        twice_updated, once_updated,
        "a repeated ensure must leave .gitignore byte-identical"
    );
}

/// #1938: the local-config and project-settings write-lock sidecars get
/// their own exact, narrow rules on a fresh workspace, and the config files
/// themselves stay trackable.
#[test]
fn ensure_ac_root_gitignore_writes_config_write_lock_blocks_on_create() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    let config_block =
        "# AgentsCommander: exclude local config write-lock sidecars.\n**/.config.json.lock";
    assert_eq!(
        content.matches(config_block).count(),
        1,
        "workspace .gitignore must contain the exact config lock block once"
    );
    let project_settings_block =
        "# AgentsCommander: exclude project-settings write-lock sidecars.\n/.project-settings.json.lock";
    assert_eq!(
        content.matches(project_settings_block).count(),
        1,
        "workspace .gitignore must contain the exact project-settings lock block once"
    );
    assert_eq!(
        content
            .lines()
            .filter(|line| *line == "**/.config.json.lock")
            .count(),
        1,
        "workspace .gitignore must contain the depth-insensitive config lock line once"
    );
    assert_eq!(
        content
            .lines()
            .filter(|line| *line == "/.project-settings.json.lock")
            .count(),
        1,
        "workspace .gitignore must contain the anchored project-settings lock line once"
    );
    assert!(
        !content.lines().any(|line| {
            matches!(
                line.trim(),
                "config.json"
                    | "**/config.json"
                    | ".config.json.lock"
                    | ".project-settings.json.lock"
            )
        }),
        "only the exact lock lines may be ignored; config files stay trackable"
    );
}

/// #1938: appending to an existing user-authored .gitignore preserves the
/// original bytes, adds each exact lock block once, and a second ensure is
/// byte-identical.
#[test]
fn ensure_ac_root_gitignore_appends_config_write_lock_blocks_preserving_bytes_and_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    let original = b"# User-authored rules\r\n!important.txt\r\n\r\n".to_vec();
    std::fs::write(&gitignore_path, &original).expect("write .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let updated = std::fs::read(&gitignore_path).expect("read updated .gitignore");
    assert!(
        updated.starts_with(&original),
        "workspace .gitignore must preserve the original bytes as an exact prefix"
    );
    let updated_text = std::str::from_utf8(&updated).expect("updated .gitignore is UTF-8");
    assert_eq!(
        updated_text
            .lines()
            .filter(|line| *line == "**/.config.json.lock")
            .count(),
        1,
        "workspace .gitignore must contain the config lock line once"
    );
    assert_eq!(
        updated_text
            .lines()
            .filter(|line| *line == "/.project-settings.json.lock")
            .count(),
        1,
        "workspace .gitignore must contain the project-settings lock line once"
    );
    assert_eq!(
        updated_text
            .matches(
                "# AgentsCommander: exclude local config write-lock sidecars.\n**/.config.json.lock"
            )
            .count(),
        1,
        "workspace .gitignore must append the exact config lock block once"
    );
    assert_eq!(
        updated_text
            .matches(
                "# AgentsCommander: exclude project-settings write-lock sidecars.\n/.project-settings.json.lock"
            )
            .count(),
        1,
        "workspace .gitignore must append the exact project-settings lock block once"
    );
    assert_eq!(
        updated_text
            .lines()
            .filter(|line| *line == "/.settings.50.personal.no-git.json.lock")
            .count(),
        1,
        "workspace .gitignore must contain the project settings lock line once"
    );
    assert_eq!(
        updated_text
            .matches(
                "# AgentsCommander: exclude the project settings write-lock sidecar.\n/.settings.50.personal.no-git.json.lock"
            )
            .count(),
        1,
        "workspace .gitignore must append the exact project settings lock block once"
    );

    let once_updated = updated;
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore again");
    let twice_updated = std::fs::read(&gitignore_path).expect("read .gitignore again");
    assert_eq!(
        twice_updated, once_updated,
        "a repeated ensure must leave .gitignore byte-identical"
    );
}

/// #1938: the two new rules match exactly what they must - the config lock
/// sidecars - at every depth for `**/.config.json.lock` and only at the root
/// for `/.project-settings.json.lock`; `config.json` itself stays trackable.
#[test]
fn ensure_ac_root_gitignore_config_write_lock_rules_are_narrow() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let init_status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init must execute");
    assert!(init_status.success(), "git init must succeed");

    let empty_excludes = project.join("empty-global-excludes");
    std::fs::write(&empty_excludes, []).expect("create empty global excludes file");
    let excludes_override = format!(
        "core.excludesFile={}",
        empty_excludes.to_string_lossy().replace('\\', "/")
    );

    for relative in [
        ".ac/.config.json.lock",
        ".ac/_agent_a/.config.json.lock",
        ".ac/.project-settings.json.lock",
        ".ac/nested/.project-settings.json.lock",
        ".ac/config.json",
        ".ac/_agent_a/config.json",
    ] {
        let path = project.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture dir");
        }
        std::fs::write(&path, []).expect("create fixture file");
    }

    let check_ignore = |relative: &str| -> Option<String> {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg(&excludes_override)
            .args(["check-ignore", "-v", "--no-index", "--", relative])
            .current_dir(&project)
            .output()
            .expect("git check-ignore must execute");
        if output.status.success() {
            Some(String::from_utf8(output.stdout).expect("check-ignore output is UTF-8"))
        } else {
            None
        }
    };

    let root_config_lock = check_ignore(".ac/.config.json.lock")
        .expect("the root config lock sidecar must be ignored");
    assert!(
        root_config_lock.contains("**/.config.json.lock"),
        "config lock rule must be the depth-insensitive one: {root_config_lock}"
    );
    let nested_config_lock = check_ignore(".ac/_agent_a/.config.json.lock")
        .expect("a nested config lock sidecar must be ignored");
    assert!(
        nested_config_lock.contains("**/.config.json.lock"),
        "config lock rule must reach nested directories: {nested_config_lock}"
    );

    let root_project_settings_lock = check_ignore(".ac/.project-settings.json.lock")
        .expect("the root project-settings lock sidecar must be ignored");
    assert!(
        root_project_settings_lock.contains("/.project-settings.json.lock"),
        "project-settings lock rule must be the anchored one: {root_project_settings_lock}"
    );
    assert!(
        check_ignore(".ac/nested/.project-settings.json.lock").is_none(),
        "the project-settings lock rule must stay root-anchored"
    );

    assert!(
        check_ignore(".ac/config.json").is_none(),
        "matrix config.json must remain trackable"
    );
    assert!(
        check_ignore(".ac/_agent_a/config.json").is_none(),
        "nested replica config.json must remain trackable"
    );
}

#[test]
fn ensure_ac_root_gitignore_team_config_lock_rule_is_root_anchored() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let init_status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init must execute");
    assert!(init_status.success(), "git init must succeed");

    let empty_excludes = project.join("empty-global-excludes");
    std::fs::write(&empty_excludes, []).expect("create empty global excludes file");
    std::fs::write(ac_root.join(".team-config-write.lock"), [])
        .expect("create root team-config lock");
    std::fs::create_dir_all(ac_root.join("nested")).expect("create nested directory");
    std::fs::write(ac_root.join("nested").join(".team-config-write.lock"), [])
        .expect("create nested team-config lock");

    let excludes_override = format!(
        "core.excludesFile={}",
        empty_excludes.to_string_lossy().replace('\\', "/")
    );
    let root_output = std::process::Command::new("git")
        .arg("-c")
        .arg(&excludes_override)
        .args([
            "check-ignore",
            "-v",
            "--no-index",
            "--",
            ".ac/.team-config-write.lock",
        ])
        .current_dir(&project)
        .output()
        .expect("root git check-ignore must execute");
    assert!(
        root_output.status.success(),
        "root git check-ignore must match: {}",
        String::from_utf8_lossy(&root_output.stderr)
    );

    let root_stdout = String::from_utf8(root_output.stdout).expect("root output is UTF-8");
    let root_line = root_stdout.trim_end_matches(&['\r', '\n'][..]);
    let (source_and_pattern, target) = root_line
        .split_once('\t')
        .expect("verbose git check-ignore output contains a tab");
    let mut source_fields = source_and_pattern.rsplitn(3, ':');
    let pattern = source_fields.next().expect("matched pattern");
    let line_number = source_fields.next().expect("matched line number");
    let source = source_fields.next().expect("matched source");
    assert_eq!(
        source, ".ac/.gitignore",
        "match source must be .ac/.gitignore"
    );
    assert!(
        line_number.parse::<usize>().is_ok(),
        "match line number must be numeric"
    );
    assert_eq!(
        pattern, "/.team-config-write.lock",
        "match pattern must be the anchored team-config lock rule"
    );
    assert_eq!(
        target, ".ac/.team-config-write.lock",
        "match target must be the root team-config lock"
    );

    let nested_output = std::process::Command::new("git")
        .arg("-c")
        .arg(&excludes_override)
        .args([
            "check-ignore",
            "-v",
            "--no-index",
            "--",
            ".ac/nested/.team-config-write.lock",
        ])
        .current_dir(&project)
        .output()
        .expect("nested git check-ignore must execute");
    assert_eq!(
        nested_output.status.code(),
        Some(1),
        "nested git check-ignore must report no match: {}",
        String::from_utf8_lossy(&nested_output.stderr)
    );
    assert!(
        nested_output.stdout.is_empty(),
        "nested git check-ignore must emit no stdout"
    );
}
