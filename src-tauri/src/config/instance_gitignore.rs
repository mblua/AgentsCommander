use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const FIXED_RULES: [&str; 12] = [
    "/.agentscommander-injected-messages.json",
    "/app-outbox-path.txt",
    "/app.log",
    "/daemon.pid",
    "/injected-messages.default.toml",
    "/injected-messages.toml",
    "/injected-messages.toml.bak-*",
    "/master-token.txt",
    "/sessions.json",
    "/settings.json",
    "/update-check.json",
    "/web-token.txt",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptResult {
    Done,
    RetryClassification,
}

/// Ensure the running instance's root `.gitignore` contains the complete
/// AgentsCommander runtime-file policy.
pub(crate) fn ensure_instance_gitignore() -> Result<(), String> {
    let config_dir = super::config_dir().ok_or_else(|| {
        "cannot seed the instance .gitignore because the config directory is unavailable"
            .to_string()
    })?;
    std::fs::create_dir_all(&config_dir).map_err(|error| {
        format!(
            "failed to create instance config directory {}: {error}",
            config_dir.display()
        )
    })?;

    ensure_instance_gitignore_at(&config_dir, &super::agent_local_dir_name())
}

fn ensure_instance_gitignore_at(config_dir: &Path, agent_local_dir: &str) -> Result<(), String> {
    let rules = required_rules(agent_local_dir)?;
    let path = config_dir.join(".gitignore");
    let mut retried = false;

    loop {
        let result = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                validate_regular_metadata(&path, &metadata, "named target")?;
                ensure_existing_file(&path, &rules)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_fresh_file(&path, &rules)?
            }
            Err(error) => {
                return Err(format!(
                    "failed to inspect instance .gitignore at {}: {error}",
                    path.display()
                ));
            }
        };

        match result {
            AttemptResult::Done => return Ok(()),
            AttemptResult::RetryClassification if !retried => retried = true,
            AttemptResult::RetryClassification => {
                return Err(format!(
                    "instance .gitignore target changed repeatedly at {}",
                    path.display()
                ));
            }
        }
    }
}

fn escape_gitignore_path_segment(segment: &str) -> Result<String, String> {
    let mut escaped = String::with_capacity(segment.len());
    for character in segment.chars() {
        match character {
            '\r' | '\n' => {
                return Err("agent local directory name contains a line break".to_string());
            }
            '/' => {
                return Err("agent local directory name contains a path separator".to_string());
            }
            '\0' => return Err("agent local directory name contains NUL".to_string()),
            '\\' | '*' | '?' | '[' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    Ok(escaped)
}

fn required_rules(agent_local_dir: &str) -> Result<[String; 14], String> {
    let escaped_agent_local_dir = escape_gitignore_path_segment(agent_local_dir)?;

    Ok([
        format!(
            "/{}/{}/config.json",
            super::root_agent::ROOT_AGENT_DIR_NAME,
            escaped_agent_local_dir
        ),
        format!("/{}/config.json", super::root_agent::ROOT_AGENT_DIR_NAME),
        FIXED_RULES[0].to_string(),
        FIXED_RULES[1].to_string(),
        FIXED_RULES[2].to_string(),
        FIXED_RULES[3].to_string(),
        FIXED_RULES[4].to_string(),
        FIXED_RULES[5].to_string(),
        FIXED_RULES[6].to_string(),
        FIXED_RULES[7].to_string(),
        FIXED_RULES[8].to_string(),
        FIXED_RULES[9].to_string(),
        FIXED_RULES[10].to_string(),
        FIXED_RULES[11].to_string(),
    ])
}

fn create_fresh_file(path: &Path, rules: &[String; 14]) -> Result<AttemptResult, String> {
    let mut file = match open_new_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(AttemptResult::RetryClassification);
        }
        Err(error) => {
            return Err(format!(
                "failed to create instance .gitignore at {}: {error}",
                path.display()
            ));
        }
    };

    validate_opened_regular_file(path, &file, "newly created target")?;
    acquire_nonblocking_lock(path, &file)?;
    validate_opened_regular_file(path, &file, "locked newly created target")?;

    let bytes = fresh_file_bytes(rules);
    file.write_all(&bytes).map_err(|error| {
        format!(
            "failed to write new instance .gitignore at {}: {error}",
            path.display()
        )
    })?;
    Ok(AttemptResult::Done)
}

fn ensure_existing_file(path: &Path, rules: &[String; 14]) -> Result<AttemptResult, String> {
    let mut read_file = match open_read_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AttemptResult::RetryClassification);
        }
        Err(error) => {
            return Err(format!(
                "failed to open instance .gitignore for reading at {}: {error}",
                path.display()
            ));
        }
    };
    validate_opened_regular_file(path, &read_file, "read target")?;
    let bytes = read_all(path, &mut read_file)?;
    validate_opened_regular_file(path, &read_file, "read target after inspection")?;
    if missing_rule_indexes(&bytes, rules).is_empty() {
        return Ok(AttemptResult::Done);
    }
    drop(read_file);

    let mut append_file = match open_append_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AttemptResult::RetryClassification);
        }
        Err(error) => {
            return Err(format!(
                "failed to open incomplete instance .gitignore for append at {}: {error}",
                path.display()
            ));
        }
    };
    validate_opened_regular_file(path, &append_file, "append target")?;
    acquire_nonblocking_lock(path, &append_file)?;
    validate_opened_regular_file(path, &append_file, "locked append target")?;

    let locked_bytes = read_all(path, &mut append_file)?;
    validate_opened_regular_file(path, &append_file, "append target after locked read")?;
    let missing = missing_rule_indexes(&locked_bytes, rules);
    if missing.is_empty() {
        return Ok(AttemptResult::Done);
    }

    let suffix = append_buffer(&locked_bytes, rules, &missing);
    append_file.write_all(&suffix).map_err(|error| {
        format!(
            "failed to append instance .gitignore rules at {}: {error}",
            path.display()
        )
    })?;
    Ok(AttemptResult::Done)
}

fn fresh_file_bytes(rules: &[String; 14]) -> Vec<u8> {
    let capacity = rules.iter().map(|rule| rule.len() + 1).sum();
    let mut bytes = Vec::with_capacity(capacity);
    for rule in rules {
        bytes.extend_from_slice(rule.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

fn missing_rule_indexes(bytes: &[u8], rules: &[String; 14]) -> Vec<usize> {
    rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| (!contains_exact_line(bytes, rule.as_bytes())).then_some(index))
        .collect()
}

fn contains_exact_line(bytes: &[u8], rule: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        let logical_line = if line.last() == Some(&b'\r') {
            &line[..line.len() - 1]
        } else {
            line
        };
        logical_line == rule
    })
}

fn append_buffer(existing: &[u8], rules: &[String; 14], missing: &[usize]) -> Vec<u8> {
    let capacity = missing
        .iter()
        .map(|index| rules[*index].len() + 1)
        .sum::<usize>()
        + usize::from(!existing.is_empty() && !existing.ends_with(b"\n"));
    let mut suffix = Vec::with_capacity(capacity);
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        suffix.push(b'\n');
    }
    for index in missing {
        suffix.extend_from_slice(rules[*index].as_bytes());
        suffix.push(b'\n');
    }
    suffix
}

fn read_all(path: &Path, file: &mut File) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!(
            "failed to seek instance .gitignore at {}: {error}",
            path.display()
        )
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        if is_lock_contention_error(&error) {
            format!(
                "instance .gitignore lock contention while reading at {}: {error}",
                path.display()
            )
        } else {
            format!(
                "failed to read instance .gitignore at {}: {error}",
                path.display()
            )
        }
    })?;
    Ok(bytes)
}

fn acquire_nonblocking_lock(path: &Path, file: &File) -> Result<(), String> {
    file.try_lock().map_err(|error| {
        let error: io::Error = error.into();
        if is_lock_contention_error(&error) {
            format!("instance .gitignore lock contention at {}", path.display())
        } else {
            format!(
                "failed to lock instance .gitignore at {}: {error}",
                path.display()
            )
        }
    })
}

fn is_lock_contention_error(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
        error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn validate_regular_metadata(
    path: &Path,
    metadata: &Metadata,
    context: &str,
) -> Result<(), String> {
    if metadata.is_file() && !is_link_or_reparse(metadata) {
        Ok(())
    } else {
        Err(format!(
            "instance .gitignore {context} at {} is not a regular, non-reparse file",
            path.display()
        ))
    }
}

fn validate_opened_regular_file(path: &Path, file: &File, context: &str) -> Result<(), String> {
    let opened_metadata = file.metadata().map_err(|error| {
        format!(
            "failed to inspect opened instance .gitignore at {}: {error}",
            path.display()
        )
    })?;
    validate_regular_metadata(path, &opened_metadata, context)?;

    let leaf_metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "instance .gitignore target changed during validation at {}: {error}",
            path.display()
        )
    })?;
    validate_regular_metadata(path, &leaf_metadata, "revalidated named target")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened_metadata.dev() != leaf_metadata.dev()
            || opened_metadata.ino() != leaf_metadata.ino()
        {
            return Err(format!(
                "instance .gitignore target changed during validation at {}",
                path.display()
            ));
        }
    }

    Ok(())
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn apply_safe_open_flags(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = options;
    }
}

fn open_new_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

fn open_read_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

fn open_append_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).append(true);
    apply_safe_open_flags(&mut options);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::process::{Command, Output};
    use std::time::{Duration, Instant};

    const TEST_AGENT_LOCAL_DIR: &str = ".agentscommander_amp-office";

    fn git(repo: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("run git")
    }

    fn assert_git_success(repo: &Path, args: &[&str]) -> Output {
        let output = git(repo, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn assert_git_ignore_status(repo: &Path, relative_path: &str, expected_code: i32) {
        let output = git(
            repo,
            &["check-ignore", "--no-index", "--quiet", "--", relative_path],
        );
        assert_eq!(
            output.status.code(),
            Some(expected_code),
            "unexpected ignore status for {relative_path}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn fresh_file_has_exact_fourteen_rules_and_dynamic_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure fresh file");

        let actual = std::fs::read(temp.path().join(".gitignore")).expect("read .gitignore");
        let expected = concat!(
            "/ac-root-agent/.agentscommander_amp-office/config.json\n",
            "/ac-root-agent/config.json\n",
            "/.agentscommander-injected-messages.json\n",
            "/app-outbox-path.txt\n",
            "/app.log\n",
            "/daemon.pid\n",
            "/injected-messages.default.toml\n",
            "/injected-messages.toml\n",
            "/injected-messages.toml.bak-*\n",
            "/master-token.txt\n",
            "/sessions.json\n",
            "/settings.json\n",
            "/update-check.json\n",
            "/web-token.txt\n",
        );
        assert_eq!(actual, expected.as_bytes());
        assert!(actual.ends_with(b"\n"));

        let lines: Vec<&[u8]> = actual[..actual.len() - 1]
            .split(|byte| *byte == b'\n')
            .collect();
        assert_eq!(lines.len(), 14);
        assert_eq!(lines.iter().copied().collect::<HashSet<_>>().len(), 14);
        assert!(!actual
            .windows(b"<agent-local-dir>".len())
            .any(|window| window == b"<agent-local-dir>"));
        assert!(lines.iter().all(|line| !line.starts_with(b"!")));
        assert!(lines.iter().all(|line| *line != b"/*"));
    }

    #[test]
    fn injected_name_with_line_break_is_rejected_without_creating_a_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = ensure_instance_gitignore_at(temp.path(), ".valid\n/app.log");
        assert!(result.is_err());
        assert!(!temp.path().join(".gitignore").exists());
    }

    #[test]
    fn partial_file_preserves_prefix_and_appends_only_missing_rules() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        let seed = format!(
            "# user comment\r\n/custom/**\r\n{}\r\n{}\r\n{}\r\n# /daemon.pid\r\n!/sessions.json\r\n{}\r\n!/update-check.json\r\n/web-token.txt # note\r\nlast-user-rule",
            rules[0], rules[3], rules[3], rules[8]
        )
        .into_bytes();
        std::fs::write(&path, &seed).expect("seed partial file");

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
            .expect("repair partial file");
        let actual = std::fs::read(&path).expect("read repaired file");
        assert!(actual.starts_with(&seed));

        let mut expected = seed.clone();
        expected.push(b'\n');
        for (index, rule) in rules.iter().enumerate() {
            if !matches!(index, 0 | 3 | 8) {
                expected.extend_from_slice(rule.as_bytes());
                expected.push(b'\n');
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn complete_file_is_byte_stable_across_repeated_ensure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        std::fs::write(&path, b"# retained user rule\n/custom\n").expect("seed file");

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("first ensure");
        let after_first = std::fs::read(&path).expect("read first result");
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("second ensure");
        let after_second = std::fs::read(&path).expect("read second result");

        assert_eq!(after_second, after_first);
    }

    #[test]
    fn byte_scan_preserves_invalid_utf8_and_recognizes_crlf_rules() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        let mut seed = b"\xff\xfe user bytes\r\n".to_vec();
        for rule in &rules {
            seed.extend_from_slice(rule.as_bytes());
            seed.extend_from_slice(b"\r\n");
        }
        std::fs::write(&path, &seed).expect("seed invalid UTF-8 file");

        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure CRLF file");
        assert_eq!(std::fs::read(path).expect("read file"), seed);
    }

    #[test]
    fn directory_target_is_rejected_and_untouched() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join(".gitignore");
        std::fs::create_dir(&target).expect("create directory target");
        let sentinel = target.join("sentinel.txt");
        std::fs::write(&sentinel, b"untouched").expect("write sentinel");

        let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        assert!(result.is_err());
        assert!(target.is_dir());
        assert_eq!(
            std::fs::read(sentinel).expect("read sentinel"),
            b"untouched"
        );
    }

    #[test]
    fn symlink_target_is_rejected_without_touching_referent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join("config");
        std::fs::create_dir(&config_dir).expect("create config dir");
        let referent = temp.path().join("external-sentinel.txt");
        std::fs::write(&referent, b"external sentinel").expect("write referent");
        let link = config_dir.join(".gitignore");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&referent, &link).expect("create symlink");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&referent, &link).is_err() {
            return;
        }
        #[cfg(not(any(unix, windows)))]
        return;

        let result = ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR);
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&referent).expect("read referent"),
            b"external sentinel"
        );
    }

    #[test]
    fn read_only_complete_file_needs_no_write_but_partial_file_fails_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
            .expect("create complete file");
        let complete = std::fs::read(&path).expect("read complete file");

        let original_permissions = std::fs::metadata(&path)
            .expect("complete metadata")
            .permissions();
        let mut read_only_permissions = original_permissions.clone();
        read_only_permissions.set_readonly(true);
        std::fs::set_permissions(&path, read_only_permissions)
            .expect("make complete file read-only");
        let complete_result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        let complete_after = std::fs::read(&path).expect("read complete read-only file");
        std::fs::set_permissions(&path, original_permissions)
            .expect("restore complete permissions");
        assert!(complete_result.is_ok());
        assert_eq!(complete_after, complete);

        let partial = b"# partial user file\n".to_vec();
        std::fs::write(&path, &partial).expect("seed partial file");
        let partial_permissions = std::fs::metadata(&path)
            .expect("partial metadata")
            .permissions();
        let mut partial_read_only = partial_permissions.clone();
        partial_read_only.set_readonly(true);
        std::fs::set_permissions(&path, partial_read_only).expect("make partial file read-only");
        let can_bypass_read_only = OpenOptions::new().append(true).open(&path).is_ok();
        let partial_result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        let partial_after = std::fs::read(&path).expect("read partial read-only file");
        std::fs::set_permissions(&path, partial_permissions).expect("restore partial permissions");

        if can_bypass_read_only {
            eprintln!(
                "skipping permission-denial assertion because this process can write a read-only file"
            );
            return;
        }
        assert!(partial_result.is_err());
        assert_eq!(partial_after, partial);
    }

    #[test]
    fn locked_partial_file_fails_fast_and_remains_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let partial = b"# partial\n".to_vec();
        std::fs::write(&path, &partial).expect("seed partial file");
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open lock holder");
        held.try_lock().expect("hold target lock");

        let started = Instant::now();
        let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
        let elapsed = started.elapsed();
        assert!(
            result
                .as_ref()
                .is_err_and(|error| error.contains("lock contention")),
            "unexpected lock result: {result:?}"
        );
        assert!(elapsed < Duration::from_secs(5));

        drop(held);
        assert_eq!(
            std::fs::read(&path).expect("read file after releasing lock"),
            partial
        );
        ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
            .expect("repair after releasing lock");
        let repaired = std::fs::read(path).expect("read repaired file");
        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        assert!(missing_rule_indexes(&repaired, &rules).is_empty());
    }

    #[test]
    fn git_fixture_ignores_exactly_required_paths_without_untracking() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);

        let parent_gitignore = b"/.parent-ignore-sentinel\n";
        let info_exclude = b"/.exclude-sentinel\n";
        std::fs::write(repo.join(".gitignore"), parent_gitignore).expect("seed parent .gitignore");
        std::fs::write(repo.join(".git/info/exclude"), info_exclude).expect("seed info exclude");

        let config_dir = repo.join("instance");
        let required_paths = [
            "ac-root-agent/.agentscommander_amp-office/config.json",
            "ac-root-agent/config.json",
            ".agentscommander-injected-messages.json",
            "app-outbox-path.txt",
            "app.log",
            "daemon.pid",
            "injected-messages.default.toml",
            "injected-messages.toml",
            "injected-messages.toml.bak-20260801T221533Z",
            "master-token.txt",
            "sessions.json",
            "settings.json",
            "update-check.json",
            "web-token.txt",
        ];
        for relative in required_paths {
            let path = config_dir.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, b"fixture").expect("write required fixture");
        }

        assert_git_success(repo, &["add", "--", "instance/app.log"]);
        assert_git_success(
            repo,
            &["ls-files", "--error-unmatch", "--", "instance/app.log"],
        );

        ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
            .expect("ensure fixture .gitignore");

        for relative in required_paths {
            let repo_relative = format!("instance/{relative}");
            let output = git(
                repo,
                &[
                    "check-ignore",
                    "--no-index",
                    "--quiet",
                    "--",
                    &repo_relative,
                ],
            );
            assert!(
                output.status.success(),
                "required path was not ignored: {repo_relative}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let control_paths = [
            "app.log.1",
            "update-check.json.tmp",
            "api-audit.log",
            "cache/entry.bin",
            "state.sqlite",
            "ac-root-agent/unrelated/config.json",
            "injected-messages.toml.bak",
            "injected-messages.json",
            "agentscommander-injected-messages.json",
            "sub/injected-messages.toml",
        ];
        for relative in control_paths {
            let path = config_dir.join(relative);
            std::fs::create_dir_all(path.parent().expect("control parent"))
                .expect("create control parent");
            std::fs::write(path, b"control").expect("write control fixture");
            let repo_relative = format!("instance/{relative}");
            let output = git(
                repo,
                &[
                    "check-ignore",
                    "--no-index",
                    "--quiet",
                    "--",
                    &repo_relative,
                ],
            );
            assert_eq!(
                output.status.code(),
                Some(1),
                "unrequested control was ignored: {repo_relative}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        assert_git_success(
            repo,
            &["ls-files", "--error-unmatch", "--", "instance/app.log"],
        );
        assert_eq!(
            std::fs::read(repo.join(".gitignore")).expect("read parent .gitignore"),
            parent_gitignore
        );
        assert_eq!(
            std::fs::read(repo.join(".git/info/exclude")).expect("read info exclude"),
            info_exclude
        );
    }

    #[test]
    fn literal_gitignore_segment_encoding_is_canonical() {
        for (input, expected) in [("\\", "\\\\"), ("*", "\\*"), ("?", "\\?"), ("[", "\\[")] {
            assert_eq!(
                escape_gitignore_path_segment(input).expect("encode metacharacter"),
                expected
            );
        }
        assert_eq!(
            escape_gitignore_path_segment(r"\*?[]-^#! spaced._(), café 東京")
                .expect("encode combined segment"),
            r"\\\*\?\[]-^#! spaced._(), café 東京"
        );

        for invalid in [
            ".agents\rinjected",
            ".agents\ninjected",
            ".agents/injected",
            ".agents\0injected",
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let result = ensure_instance_gitignore_at(temp.path(), invalid);
            assert!(result.is_err(), "invalid segment was accepted: {invalid:?}");
            assert!(
                !temp.path().join(".gitignore").exists(),
                "invalid segment touched .gitignore: {invalid:?}"
            );
        }

        let agent_local_dir = r".agents\*?[]-^#! café";
        let escaped_agent_local_dir =
            escape_gitignore_path_segment(agent_local_dir).expect("encode agent local dir");
        let rules = required_rules(agent_local_dir).expect("construct canonical rules");
        assert_eq!(rules.len(), 14);
        assert_eq!(
            rules[0],
            format!(
                "/{}/{escaped_agent_local_dir}/config.json",
                super::super::root_agent::ROOT_AGENT_DIR_NAME
            )
        );
        assert_eq!(
            &rules[1..],
            &[
                "/ac-root-agent/config.json",
                "/.agentscommander-injected-messages.json",
                "/app-outbox-path.txt",
                "/app.log",
                "/daemon.pid",
                "/injected-messages.default.toml",
                "/injected-messages.toml",
                "/injected-messages.toml.bak-*",
                "/master-token.txt",
                "/sessions.json",
                "/settings.json",
                "/update-check.json",
                "/web-token.txt",
            ]
        );
        assert_eq!(
            rules
                .iter()
                .filter(|rule| rule.contains(&escaped_agent_local_dir))
                .count(),
            1
        );
        let raw_companion = format!(
            "/{}/{agent_local_dir}/config.json",
            super::super::root_agent::ROOT_AGENT_DIR_NAME
        );
        assert!(!rules.contains(&raw_companion));
    }

    #[test]
    fn git_fixture_treats_bracketed_agent_name_as_literal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);
        let config_dir = repo.join("instance");
        std::fs::create_dir(&config_dir).expect("create config dir");

        let agent_local_dir = ".agents[1]";
        ensure_instance_gitignore_at(&config_dir, agent_local_dir)
            .expect("ensure bracketed fixture");
        let rules = required_rules(agent_local_dir).expect("canonical bracketed rules");
        assert_eq!(rules[0], r"/ac-root-agent/.agents\[1]/config.json");
        let generated = std::fs::read(config_dir.join(".gitignore")).expect("read .gitignore");
        assert_eq!(generated, fresh_file_bytes(&rules));
        assert_eq!(
            generated
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            14
        );

        let literal = "instance/ac-root-agent/.agents[1]/config.json";
        let sibling = "instance/ac-root-agent/.agents1/config.json";
        for relative in [literal, sibling] {
            let path = repo.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, b"fixture").expect("write fixture");
        }
        assert_git_ignore_status(repo, literal, 0);
        assert_git_ignore_status(repo, sibling, 1);
    }

    #[cfg(unix)]
    #[test]
    fn git_fixture_treats_unix_metacharacter_agent_names_as_literal() {
        let cases = [
            (".agents*literal", ".agentsZZliteral"),
            (".agents?literal", ".agentsXliteral"),
            (".agents\\literal", ".agentsliteral"),
        ];

        for (agent_local_dir, unintended_sibling) in cases {
            let temp = tempfile::tempdir().expect("tempdir");
            let repo = temp.path();
            assert_git_success(repo, &["init", "--quiet"]);
            let config_dir = repo.join("instance");
            std::fs::create_dir(&config_dir).expect("create config dir");

            ensure_instance_gitignore_at(&config_dir, agent_local_dir)
                .expect("ensure metacharacter fixture");
            let rules = required_rules(agent_local_dir).expect("canonical metacharacter rules");
            let generated = std::fs::read(config_dir.join(".gitignore")).expect("read .gitignore");
            assert_eq!(generated, fresh_file_bytes(&rules));
            assert_eq!(
                generated
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                    .count(),
                14
            );
            let raw_companion = format!(
                "/{}/{agent_local_dir}/config.json",
                super::super::root_agent::ROOT_AGENT_DIR_NAME
            );
            assert!(!rules.contains(&raw_companion));

            let literal = format!(
                "instance/{}/{agent_local_dir}/config.json",
                super::super::root_agent::ROOT_AGENT_DIR_NAME
            );
            let sibling = format!(
                "instance/{}/{unintended_sibling}/config.json",
                super::super::root_agent::ROOT_AGENT_DIR_NAME
            );
            for relative in [&literal, &sibling] {
                let path = repo.join(relative);
                std::fs::create_dir_all(path.parent().expect("fixture parent"))
                    .expect("create fixture parent");
                std::fs::write(path, b"fixture").expect("write fixture");
            }
            assert_git_ignore_status(repo, &literal, 0);
            assert_git_ignore_status(repo, &sibling, 1);
        }
    }

    #[test]
    fn escaped_canonical_line_controls_detection_and_repair() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".gitignore");
        let agent_local_dir = ".agents[1]";
        let rules = required_rules(agent_local_dir).expect("canonical rules");
        let canonical_first = &rules[0];
        let raw_first = format!(
            "/{}/{agent_local_dir}/config.json",
            super::super::root_agent::ROOT_AGENT_DIR_NAME
        );

        assert!(!contains_exact_line(
            format!("{raw_first}\n").as_bytes(),
            canonical_first.as_bytes()
        ));
        assert!(contains_exact_line(
            format!("{canonical_first}\n").as_bytes(),
            canonical_first.as_bytes()
        ));

        let mut seed = Vec::new();
        seed.extend_from_slice(raw_first.as_bytes());
        seed.push(b'\n');
        for rule in &rules[1..] {
            seed.extend_from_slice(rule.as_bytes());
            seed.push(b'\n');
        }
        assert_eq!(missing_rule_indexes(&seed, &rules), vec![0]);
        std::fs::write(&path, &seed).expect("seed raw predecessor");

        ensure_instance_gitignore_at(temp.path(), agent_local_dir).expect("repair raw predecessor");
        let repaired = std::fs::read(&path).expect("read repaired file");
        let mut expected = seed.clone();
        expected.extend_from_slice(canonical_first.as_bytes());
        expected.push(b'\n');
        assert_eq!(repaired, expected);
        assert_eq!(missing_rule_indexes(&repaired, &rules), Vec::<usize>::new());

        ensure_instance_gitignore_at(temp.path(), agent_local_dir).expect("repeat repaired ensure");
        assert_eq!(std::fs::read(path).expect("read repeated result"), repaired);
    }

    #[test]
    fn instance_gitignore_covers_every_injected_messages_artifact() {
        use super::super::injected_messages::{
            INJECTED_MESSAGES_FILENAME, INJECTED_MESSAGES_REFERENCE_FILENAME,
            INJECTED_MESSAGES_STATE_FILENAME,
        };

        let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
        for expected in [
            format!("/{INJECTED_MESSAGES_FILENAME}"),
            format!("/{INJECTED_MESSAGES_REFERENCE_FILENAME}"),
            format!("/{INJECTED_MESSAGES_STATE_FILENAME}"),
            format!("/{INJECTED_MESSAGES_FILENAME}.bak-*"),
        ] {
            assert!(
                rules.contains(&expected),
                "injected-messages artifact is not covered by the instance policy: {expected}"
            );
        }
    }

    #[test]
    fn instance_gitignore_ignores_injected_messages_artifacts_narrowly() {
        use super::super::injected_messages::{
            INJECTED_MESSAGES_FILENAME, INJECTED_MESSAGES_REFERENCE_FILENAME,
            INJECTED_MESSAGES_STATE_FILENAME,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);
        let config_dir = repo.join("instance");
        std::fs::create_dir(&config_dir).expect("create config dir");

        ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
            .expect("ensure injected-messages fixture");

        let stamped_backup = format!("{INJECTED_MESSAGES_FILENAME}.bak-20260801T221533Z");
        let unstamped_backup = format!("{INJECTED_MESSAGES_FILENAME}.bak");
        let nested = format!("sub/{INJECTED_MESSAGES_FILENAME}");
        let cases = [
            (INJECTED_MESSAGES_STATE_FILENAME, 0),
            (INJECTED_MESSAGES_REFERENCE_FILENAME, 0),
            (INJECTED_MESSAGES_FILENAME, 0),
            (stamped_backup.as_str(), 0),
            (unstamped_backup.as_str(), 1),
            ("injected-messages.json", 1),
            ("agentscommander-injected-messages.json", 1),
            (nested.as_str(), 1),
        ];

        for (relative, expected_code) in cases {
            let path = config_dir.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, b"fixture").expect("write fixture");
            assert_git_ignore_status(repo, &format!("instance/{relative}"), expected_code);
        }
    }
}
