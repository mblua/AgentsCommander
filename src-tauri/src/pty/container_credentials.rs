//! #930 - resolve, copy in, and tear down a coding agent's host credential file
//! for container runtime. The per-agent *descriptor* lives on
//! `CodingAgentProfile` (session/profile.rs); this module turns a descriptor +
//! the session's host mount root into a concrete copy job and executes it.
//! Credential contents are NEVER logged (mirror pty/credentials.rs rule).

use std::path::{Path, PathBuf};

use crate::session::profile::ContainerCredentialSource;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// A resolved copy job: absolute host source file and absolute replica dest file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerCredentialPlan {
    pub source: PathBuf,
    pub dest: PathBuf,
}

/// F2 - true if `path` exists AND is a symlink or Windows junction/reparse point.
/// `symlink_metadata` does NOT traverse the leaf. NotFound => false (safe to
/// create/copy); any other error => true (undeterminable = skip-worthy). Mirrors
/// the private, DirEntry-based `config_seed::is_symlink_or_reparse`.
fn is_reparse_path(path: &Path) -> bool {
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    if md.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Resolve a copy job for `source` given the container bind-mount root
/// (`host_root`, canonical). Returns None (and logs at info) when the host
/// credential file does not exist, so a missing host login is a clean skip.
pub fn resolve_plan(
    source: &ContainerCredentialSource,
    host_root: &str,
) -> Option<ContainerCredentialPlan> {
    // Host config dir: an absolute env override wins, else ~/<host_dir>.
    let host_dir: PathBuf = source
        .host_dir_env
        .and_then(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        // F6 - only honor an ABSOLUTE override; a relative/marker value cleanly
        // falls back to ~/<host_dir> instead of resolving against AC's CWD.
        .filter(|v| Path::new(v).is_absolute())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(source.host_dir)))?;

    let src = host_dir.join(source.file);
    if !src.is_file() {
        log::info!(
            "[container-cred] no host credential at {}; container will start without host login",
            src.display()
        );
        return None;
    }
    let dest = Path::new(host_root)
        .join(source.container_dir)
        .join(source.file);
    Some(ContainerCredentialPlan { source: src, dest })
}

/// Copy the host credential into the replica dir. Best-effort: refuses to write
/// through a container-planted symlink/junction (F2), creates the dest parent
/// dir, overwrites any existing dest with the current host token, and (on Unix)
/// tightens perms to 0o600. Errors are returned for the caller to log; the caller
/// does NOT abort the spawn on failure.
pub fn copy_in(plan: &ContainerCredentialPlan) -> std::io::Result<()> {
    // F2 - never write through a container-planted symlink/junction, on the
    // container config dir or on the credential leaf.
    if let Some(parent) = plan.dest.parent() {
        if is_reparse_path(parent) {
            log::warn!(
                "[container-cred] dest dir {} is a symlink/reparse point; skipping copy-in",
                parent.display()
            );
            return Ok(());
        }
    }
    if is_reparse_path(&plan.dest) {
        log::warn!(
            "[container-cred] dest {} is a symlink/reparse point; skipping copy-in",
            plan.dest.display()
        );
        return Ok(());
    }
    if let Some(parent) = plan.dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&plan.source, &plan.dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // F4 - do not silently swallow a perms failure; a token left broadly
        // readable must at least be observable in the log.
        if let Err(e) =
            std::fs::set_permissions(&plan.dest, std::fs::Permissions::from_mode(0o600))
        {
            log::warn!(
                "[container-cred] failed to set 0o600 on {}: {}",
                plan.dest.display(),
                e
            );
        }
    }
    log::info!(
        "[container-cred] copied host credential into {}",
        plan.dest.display()
    );
    Ok(())
}

/// Delete a previously copied credential file on teardown. Best-effort and
/// idempotent: a missing file is not an error. Refuses to delete THROUGH a
/// symlink/junction (F2): AC never creates a link, so a reparse-point leaf is
/// attacker-planted and is left untouched rather than followed.
pub fn remove_copied(dest: &Path) {
    if is_reparse_path(dest) {
        log::warn!(
            "[container-cred] {} is a symlink/reparse point; refusing to delete through it",
            dest.display()
        );
        return;
    }
    match std::fs::remove_file(dest) {
        Ok(()) => log::info!("[container-cred] removed copied credential {}", dest.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(
            "[container-cred] failed to remove copied credential {}: {}",
            dest.display(),
            e
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // resolve_plan mutates/reads process env; serialize the env-touching tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn source(host_dir_env: Option<&'static str>) -> ContainerCredentialSource {
        ContainerCredentialSource {
            host_dir: ".ac-nonexistent-home-930",
            host_dir_env,
            file: ".credentials.json",
            container_dir: ".claude",
        }
    }

    fn with_env<R>(key: &str, value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let prev = std::env::var_os(key);
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let out = f();
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        out
    }

    #[test]
    fn resolve_plan_uses_absolute_env_override_and_builds_dest() {
        let _g = ENV_LOCK.lock().unwrap();
        let key = "AC_TEST_CRED_DIR_930_ABS";
        let host_dir = tempfile::tempdir().expect("host dir");
        std::fs::write(host_dir.path().join(".credentials.json"), b"tok").expect("write cred");
        let host_root = tempfile::tempdir().expect("host root");

        let plan = with_env(key, Some(&host_dir.path().to_string_lossy()), || {
            resolve_plan(&source(Some(key)), &host_root.path().to_string_lossy())
        })
        .expect("plan present");

        assert_eq!(plan.source, host_dir.path().join(".credentials.json"));
        assert_eq!(
            plan.dest,
            host_root.path().join(".claude").join(".credentials.json")
        );
    }

    #[test]
    fn resolve_plan_skips_when_source_file_absent() {
        let _g = ENV_LOCK.lock().unwrap();
        let key = "AC_TEST_CRED_DIR_930_MISSING";
        let host_dir = tempfile::tempdir().expect("host dir"); // empty, no cred file
        let host_root = tempfile::tempdir().expect("host root");

        let plan = with_env(key, Some(&host_dir.path().to_string_lossy()), || {
            resolve_plan(&source(Some(key)), &host_root.path().to_string_lossy())
        });
        assert!(plan.is_none(), "missing host file must be a clean skip");
    }

    #[test]
    fn resolve_plan_ignores_relative_env_override_f6() {
        let _g = ENV_LOCK.lock().unwrap();
        let key = "AC_TEST_CRED_DIR_930_REL";
        let host_root = tempfile::tempdir().expect("host root");

        // A relative override must be ignored (F6), so the result is identical to
        // having no override at all (both fall back to ~/<host_dir>, which does
        // not contain our sentinel file).
        let with_rel = with_env(key, Some("relative/not-absolute-930"), || {
            resolve_plan(&source(Some(key)), &host_root.path().to_string_lossy())
        });
        let with_none = with_env(key, None, || {
            resolve_plan(&source(None), &host_root.path().to_string_lossy())
        });
        assert_eq!(with_rel, with_none, "relative override must be ignored");
    }

    #[test]
    fn copy_in_then_remove_copied_is_idempotent() {
        let src_dir = tempfile::tempdir().expect("src");
        let src = src_dir.path().join(".credentials.json");
        std::fs::write(&src, b"secret-bytes").expect("write src");
        let dest_root = tempfile::tempdir().expect("dest");
        let dest = dest_root.path().join(".claude").join(".credentials.json");
        let plan = ContainerCredentialPlan {
            source: src,
            dest: dest.clone(),
        };

        copy_in(&plan).expect("copy_in ok");
        assert!(dest.is_file(), "dest created");
        assert_eq!(std::fs::read(&dest).unwrap(), b"secret-bytes");

        remove_copied(&dest);
        assert!(!dest.exists(), "dest removed");
        // Idempotent: a second delete of a now-missing file is a no-op.
        remove_copied(&dest);
        assert!(!dest.exists());
    }

    #[test]
    fn is_reparse_path_false_for_plain_file_and_missing() {
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("plain");
        std::fs::write(&file, b"x").expect("write");
        assert!(!is_reparse_path(&file), "plain file is not a reparse point");
        assert!(
            !is_reparse_path(&dir.path().join("does-not-exist")),
            "missing path is not a reparse point"
        );
    }
}
