use super::*;

mod agency_migration;
mod config_merge;
mod frozen_snapshots;
mod provisioning;
mod skill_snapshots;
mod tracked_state;

#[cfg(unix)]
fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

// ---------------------------------------------------------------------------------------
// #909: the frozen snapshot, and the two pins that keep it honest.
// ---------------------------------------------------------------------------------------

/// Look a default up by `dir_name`. Never index `DEFAULT_ROOT_SKILLS` positionally: the four
/// pre-existing assertions do, and that is four too many.
fn default_root_skill(dir_name: &str) -> &'static DefaultRootSkill {
    DEFAULT_ROOT_SKILLS
        .iter()
        .find(|skill| skill.dir_name == dir_name)
        .expect("default skill must be in the table")
}

// ---------------------------------------------------------------------------------------
// #909: the migration. Every enum assertion is paired with a content assertion. The enum
// proves which branch ran; it does not prove no bytes were written. The pair is the proof.
// ---------------------------------------------------------------------------------------

struct AgencyFixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    skills_root: PathBuf,
    path: PathBuf,
}

#[test]
fn issue_3398_root_and_skill_entry_types() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("directory");
    let file = temp.path().join("file");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("sentinel"), b"directory sentinel").unwrap();
    std::fs::write(&file, b"file sentinel").unwrap();
    let validators = [
        (
            validate_root_agent_root_path as fn(&Path) -> Result<(), String>,
            "directory",
        ),
        (validate_root_agent_skills_root, "skills path"),
        (validate_default_skill_directory, "default skill path"),
    ];
    for (validate, label) in validators {
        assert_eq!(validate(&directory), Ok(()), "{label}: directory");
        assert_eq!(
            validate(&file),
            Err(format!(
                "Root agent {label} {} exists but is not a regular directory",
                file.display()
            )),
            "{label}: regular file"
        );
    }
    assert_eq!(
        validate_default_skill_file(&file, &std::fs::symlink_metadata(&file).unwrap()),
        Ok(())
    );
    assert_eq!(
        validate_default_skill_file(&directory, &std::fs::symlink_metadata(&directory).unwrap()),
        Err(format!(
            "Root agent default skill file {} exists but is not a regular file",
            directory.display()
        ))
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"file sentinel");
    assert_eq!(
        std::fs::read(directory.join("sentinel")).unwrap(),
        b"directory sentinel"
    );
}

#[cfg(windows)]
#[test]
fn issue_3398_directory_links_refused() {
    use std::os::windows::fs::MetadataExt;
    // Probe runs select one independent fixture; ordinary runs exercise all four.
    let selected = std::env::var("AC_3398_JUNCTION_SCENARIO").ok();
    if let Some(value) = selected.as_deref() {
        assert!(["root", "skills", "leaf", "provision"].contains(&value));
    }
    for scenario in ["root", "skills", "leaf", "provision"] {
        if selected.as_deref().is_some_and(|value| value != scenario) {
            continue;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let skills = root.join("skills");
        let target = temp.path().join("target");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"target sentinel").unwrap();
        let skill = default_root_skill("self-handoff");
        let link = match scenario {
            "root" => temp.path().join("root-link"),
            "skills" => root.join("skills-link"),
            _ => skills.join(skill.dir_name),
        };
        crate::config::teams::create_test_junction(&link, &target);
        let metadata = std::fs::symlink_metadata(&link).unwrap();
        assert_ne!(
            metadata.file_attributes() & 0x400,
            0,
            "{scenario}: reparse point"
        );
        let (result, label) = match scenario {
            "root" => (validate_root_agent_root_path(&link), "directory"),
            "skills" => (validate_root_agent_skills_root(&link), "skills path"),
            "leaf" => (
                validate_default_skill_directory(&link),
                "default skill path",
            ),
            _ => (
                super::provisioning::ensure_default_root_skill_file(&root, &skills, skill),
                "default skill path",
            ),
        };
        assert_eq!(
            result,
            Err(format!(
                "Root agent {label} {} exists but is not a regular directory",
                link.display()
            )),
            "{scenario}: junction refusal"
        );
        assert!(
            !target.join(skill.file_name).exists(),
            "{scenario}: no skill created"
        );
        assert_eq!(
            std::fs::read(target.join("sentinel")).unwrap(),
            b"target sentinel",
            "{scenario}: target preserved"
        );
        std::fs::remove_dir(&link).unwrap();
    }
}

#[test]
fn issue_3398_publish_race_and_errors() {
    use std::io::{Error, ErrorKind};
    let temp = tempfile::tempdir().unwrap();
    let stage = temp.path().join("stage");
    let target = temp.path().join("winner");
    let directory = temp.path().join("directory");
    let missing = temp.path().join("missing");
    std::fs::write(&stage, b"stage bytes").unwrap();
    std::fs::write(&target, b"winner bytes").unwrap();
    std::fs::create_dir(&directory).unwrap();
    let already_exists = |_: &Path, _: &Path| Err(Error::from(ErrorKind::AlreadyExists));
    assert_eq!(
        publish_missing_default_skill_file_with(&stage, &target, already_exists),
        Ok(false),
        "injected regular target"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"winner bytes");
    assert_eq!(
        publish_missing_default_skill_file_with(&stage, &directory, already_exists),
        Err(format!(
            "Root agent default skill file {} exists but is not a regular file",
            directory.display()
        ))
    );
    let inspect_error = std::fs::symlink_metadata(&missing).unwrap_err();
    assert_eq!(
        publish_missing_default_skill_file_with(&stage, &missing, already_exists),
        Err(format!(
            "Root agent default skill file {} appeared during publish but could not be \
             inspected: {}",
            missing.display(),
            inspect_error
        ))
    );
    let denied = Error::new(ErrorKind::PermissionDenied, "issue 3398 denied");
    let expected = format!(
        "Failed to publish root agent default skill file {} from {}: {}",
        target.display(),
        stage.display(),
        denied
    );
    assert_eq!(
        publish_missing_default_skill_file_with(&stage, &target, |_, _| Err(denied)),
        Err(expected)
    );
    assert_eq!(
        publish_missing_default_skill_file(&stage, &target),
        Ok(false),
        "production create-only callback"
    );
    assert_eq!(std::fs::read(&stage).unwrap(), b"stage bytes");
    assert_eq!(std::fs::read(&target).unwrap(), b"winner bytes");
}

#[test]
fn issue_3398_temp_collision_exhaustion() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("SKILL.md");
    let collision = temp.path().join("collision");
    let fresh = temp.path().join("fresh");
    std::fs::write(&collision, b"collision sentinel").unwrap();
    let selected = std::env::var("AC_3398_COLLISION_SCENARIO").ok();
    if let Some(value) = selected.as_deref() {
        assert!(["exhaustion", "companion"].contains(&value));
    }
    if selected.as_deref() != Some("companion") {
        let mut attempts = 0;
        let result = create_default_skill_temp_file_with(&target, |_| {
            attempts += 1;
            collision.clone()
        });
        assert_eq!(
            result.err(),
            Some(format!(
                "Failed to create temporary root agent skill file for {} after 16 attempts",
                target.display()
            )),
            "collision exhaustion"
        );
        assert_eq!(attempts, 16, "exhaustion attempts");
        assert_eq!(std::fs::read(&collision).unwrap(), b"collision sentinel");
    }
    if selected.as_deref() != Some("exhaustion") {
        let mut attempts = 0;
        let (path, stage) = create_default_skill_temp_file_with(&target, |_| {
            attempts += 1;
            if attempts == 1 {
                collision.clone()
            } else {
                fresh.clone()
            }
        })
        .expect("collision then fresh stage must succeed");
        assert_eq!(attempts, 2, "collision then success attempts");
        assert_eq!(path, fresh);
        assert_eq!(std::fs::read(&fresh).unwrap(), b"");
        assert_eq!(std::fs::read(&collision).unwrap(), b"collision sentinel");
        drop(stage);
        std::fs::remove_file(&fresh).unwrap();
    }
}

#[test]
fn issue_3398_bounded_skill_read() {
    use super::skill_files::{read_bounded, BoundedRead};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let skills = root.join("skills");
    let leaf = skills.join("leaf");
    std::fs::create_dir_all(&leaf).unwrap();
    let path = leaf.join("SKILL.md");
    std::fs::write(&path, b"text").unwrap();
    assert!(matches!(read_bounded(&path, 4).unwrap(), BoundedRead::Text(s) if s == "text"));
    std::fs::write(&path, b"text+").unwrap();
    assert!(matches!(
        read_bounded(&path, 4).unwrap(),
        BoundedRead::TooLarge
    ));
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(matches!(
        read_bounded(&path, 4).unwrap(),
        BoundedRead::NotUtf8
    ));
    let missing = leaf.join("missing");
    let error = std::fs::File::open(&missing).unwrap_err();
    assert_eq!(
        read_bounded(&missing, 4).err(),
        Some(format!("Failed to open {}: {}", missing.display(), error))
    );
    let snapshots = ["old snapshot"];
    let bound = max_migratable_len("new content", &snapshots);
    let bytes = format!("{}{}MY NOTES", snapshots[0], " ".repeat(bound as usize));
    std::fs::write(&path, bytes.as_bytes()).unwrap();
    assert_eq!(
        migrate_default_skill_file(&root, &skills, &path, "new content", &snapshots).unwrap(),
        SkillMigration::Skipped(SkipReason::TooLarge)
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes());
}

#[test]
fn issue_3398_role_create_no_clobber() {
    use super::provisioning::{create_missing_role, CreateMissingRole};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Role.md");
    assert!(matches!(
        create_missing_role(&path, "intended role").unwrap(),
        CreateMissingRole::Created
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"intended role");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    std::fs::write(&path, b"custom role").unwrap();
    assert!(matches!(
        create_missing_role(&path, "replacement role").unwrap(),
        CreateMissingRole::AlreadyExists
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"custom role");
    let entries: Vec<_> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries, vec![path], "no leftover role stages");
}
