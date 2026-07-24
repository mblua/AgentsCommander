use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedCandidate {
    pub path: PathBuf,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildPath {
    pub value: OsString,
    pub skipped: Vec<SkippedCandidate>,
}

pub fn has_explicit_linux_path(
    configured_env: &[(String, String)],
    env_remove_keys: &[String],
) -> bool {
    configured_env.iter().any(|(key, _)| key == "PATH")
        || env_remove_keys.iter().any(|key| key == "PATH")
}

pub fn build_child_path<F>(
    inherited: Option<&OsStr>,
    candidates: &[PathBuf],
    is_directory: F,
) -> ChildPath
where
    F: Fn(&Path) -> bool,
{
    let base_value = match inherited {
        Some(value) if !value.is_empty() => value.to_os_string(),
        _ => OsString::from("/usr/local/bin:/usr/bin:/bin"),
    };
    let base: Vec<PathBuf> = std::env::split_paths(&base_value).collect();
    let mut retained = Vec::new();
    let mut skipped = Vec::new();

    for candidate in candidates {
        if !candidate.is_absolute() {
            skipped.push(SkippedCandidate {
                path: candidate.clone(),
                reason: "candidate is not absolute",
            });
            continue;
        }
        if !is_directory(candidate) {
            continue;
        }
        if std::env::join_paths(std::iter::once(candidate)).is_err() {
            skipped.push(SkippedCandidate {
                path: candidate.clone(),
                reason: "candidate cannot be represented in PATH",
            });
            continue;
        }
        if retained.iter().any(|existing| existing == candidate)
            || base.iter().any(|existing| existing == candidate)
        {
            continue;
        }
        retained.push(candidate.clone());
    }

    let value = std::env::join_paths(retained.iter().chain(base.iter())).unwrap_or(base_value);
    ChildPath { value, skipped }
}

#[cfg(target_os = "linux")]
pub fn local_codex_child_path() -> ChildPath {
    let inherited = std::env::var_os("PATH");
    let candidates = match dirs::home_dir() {
        Some(home) => vec![
            home.join(".local/bin"),
            home.join("bin"),
            home.join(".cargo/bin"),
        ],
        None => {
            log::warn!(
                "[pty] HOME could not be resolved; local Codex child PATH keeps its inherited baseline"
            );
            Vec::new()
        }
    };
    build_child_path(inherited.as_deref(), &candidates, Path::is_dir)
}

#[cfg(test)]
mod tests {
    use super::{build_child_path, has_explicit_linux_path};
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    #[cfg(unix)]
    #[test]
    fn candidates_prepend_in_order_and_deduplicate_exactly() {
        let candidates = vec![
            PathBuf::from("/home/u/.local/bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/home/u/bin"),
            PathBuf::from("/home/u/.local/bin"),
        ];
        let result = build_child_path(Some(OsStr::new("/usr/bin:/bin")), &candidates, |_| true);
        assert_eq!(
            result.value,
            OsStr::new("/home/u/.local/bin:/home/u/bin:/usr/bin:/bin")
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_or_empty_inherited_path_uses_safe_baseline() {
        for inherited in [None, Some(OsStr::new(""))] {
            let result = build_child_path(inherited, &[], |_| false);
            assert_eq!(result.value, OsStr::new("/usr/local/bin:/usr/bin:/bin"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn missing_candidates_are_silent() {
        let result = build_child_path(
            Some(OsStr::new("/usr/bin")),
            &[PathBuf::from("/missing")],
            |_| false,
        );
        assert_eq!(result.value, OsStr::new("/usr/bin"));
        assert!(result.skipped.is_empty());
    }

    #[test]
    fn relative_candidate_is_skipped_even_when_reported_as_a_directory() {
        let result = build_child_path(
            Some(OsStr::new("/usr/bin")),
            &[PathBuf::from("relative/bin")],
            |_| true,
        );
        assert_eq!(result.value, OsStr::new("/usr/bin"));
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].reason, "candidate is not absolute");
    }

    #[test]
    fn explicit_path_detection_is_exact_and_covers_configure_plus_remove() {
        assert!(has_explicit_linux_path(
            &[("PATH".to_string(), String::new())],
            &[]
        ));
        assert!(has_explicit_linux_path(&[], &["PATH".to_string()]));
        assert!(has_explicit_linux_path(
            &[("PATH".to_string(), "/custom".to_string())],
            &["PATH".to_string()]
        ));
        assert!(!has_explicit_linux_path(
            &[("Path".to_string(), "/custom".to_string())],
            &["path".to_string()]
        ));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_components_round_trip_as_raw_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let inherited =
            std::ffi::OsString::from_vec(vec![b'/', b'b', b'i', b'n', b':', b'/', b'x', 0xff]);
        let candidate = PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'/', b'u', b's', b'e', b'r', 0xfe,
        ]));
        let result = build_child_path(Some(inherited.as_os_str()), &[candidate], |_: &Path| true);
        assert_eq!(
            result.value.as_os_str().as_bytes(),
            b"/user\xfe:/bin:/x\xff"
        );
    }

    #[cfg(unix)]
    #[test]
    fn colon_candidate_is_skipped_without_losing_neighbors() {
        let candidates = vec![
            PathBuf::from("/one"),
            PathBuf::from("/bad:entry"),
            PathBuf::from("/two"),
        ];
        let result = build_child_path(Some(OsStr::new("/usr/bin")), &candidates, |_| true);
        assert_eq!(result.value, OsStr::new("/one:/two:/usr/bin"));
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].path, PathBuf::from("/bad:entry"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn marker_codex_is_found_and_observed_through_a_real_pty() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let parent_path_before = std::env::var_os("PATH");
        let temp = tempfile::tempdir().expect("tempdir");
        let candidate = temp.path().join("home/.local/bin");
        std::fs::create_dir_all(&candidate).expect("create candidate");
        let marker = candidate.join("codex");
        std::fs::write(
            &marker,
            "#!/bin/sh\nprintf '%s\\n' 'AC_PATH_MARKER_OK'\nprintf '%s\\n' \"$PATH\"\n",
        )
        .expect("write marker");
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o755))
            .expect("chmod marker");

        let child_path = build_child_path(
            Some(OsStr::new("/usr/bin:/bin")),
            std::slice::from_ref(&candidate),
            Path::is_dir,
        );
        assert_eq!(
            child_path.value,
            std::env::join_paths([
                candidate.as_path(),
                Path::new("/usr/bin"),
                Path::new("/bin")
            ])
            .expect("expected PATH")
        );

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");
        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let mut command = CommandBuilder::new("codex");
        command.env("PATH", &child_path.value);
        let mut child = pair
            .slave
            .spawn_command(command)
            .expect("spawn marker Codex");
        drop(pair.slave);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("poll marker Codex").is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("marker Codex did not exit before deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let result = reader.read_to_end(&mut output).map(|_| output);
            let _ = sender.send(result);
        });
        let output = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("PTY read deadline")
            .expect("read PTY output");
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("AC_PATH_MARKER_OK"));
        assert!(output.contains(candidate.to_string_lossy().as_ref()));
        assert_eq!(std::env::var_os("PATH"), parent_path_before);
    }
}
