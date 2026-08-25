//! #930 - resolve, copy in, and tear down a coding agent's host credential file
//! for container runtime. The per-agent *descriptor* lives on
//! `CodingAgentProfile` (session/profile.rs); this module turns a descriptor +
//! the session's host mount root into a concrete copy job and executes it.
//! Credential contents are NEVER logged (mirror pty/credentials.rs rule).

use std::path::{Path, PathBuf};

use crate::session::profile::{ContainerCredentialSource, ContainerFirstRunState};

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// A resolved copy job: absolute host source file and absolute replica dest file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerCredentialPlan {
    pub source: PathBuf,
    pub dest: PathBuf,
    /// #930 - first-run state to stamp in the dest dir after the copy (None =
    /// none for this agent). Carried on the plan because the container backend
    /// sees only the plan, never the agent profile.
    pub first_run: Option<ContainerFirstRunState>,
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
    Some(ContainerCredentialPlan {
        source: src,
        dest,
        first_run: source.first_run,
    })
}

/// Outcome of a `copy_in` that did not error. An F2 skip is NOT a failure (the
/// spawn continues), but it writes NO token, so the caller MUST distinguish it
/// from a real copy: stamping first-run state after a skip would suppress the
/// agent's login wizard on a container that has no credential at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    /// The host credential was written to `plan.dest`.
    Copied,
    /// The dest dir or the dest leaf is a symlink/junction, so F2 refused to
    /// write through it and nothing was copied.
    SkippedReparse,
}

/// Copy the host credential into the replica dir. Best-effort: refuses to write
/// through a container-planted symlink/junction (F2), creates the dest parent
/// dir, overwrites any existing dest with the current host token, and (on Unix)
/// tightens perms to 0o600. Errors are returned for the caller to log; the caller
/// does NOT abort the spawn on failure. `Ok(CopyOutcome::SkippedReparse)` means
/// no token was written, so it must never be treated as a successful copy.
pub fn copy_in(plan: &ContainerCredentialPlan) -> std::io::Result<CopyOutcome> {
    // F2 - never write through a container-planted symlink/junction, on the
    // container config dir or on the credential leaf.
    if let Some(parent) = plan.dest.parent() {
        if is_reparse_path(parent) {
            log::warn!(
                "[container-cred] dest dir {} is a symlink/reparse point; skipping copy-in",
                parent.display()
            );
            return Ok(CopyOutcome::SkippedReparse);
        }
    }
    if is_reparse_path(&plan.dest) {
        log::warn!(
            "[container-cred] dest {} is a symlink/reparse point; skipping copy-in",
            plan.dest.display()
        );
        return Ok(CopyOutcome::SkippedReparse);
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
        if let Err(e) = std::fs::set_permissions(&plan.dest, std::fs::Permissions::from_mode(0o600))
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
    Ok(CopyOutcome::Copied)
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
        Ok(()) => log::info!(
            "[container-cred] removed copied credential {}",
            dest.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(
            "[container-cred] failed to remove copied credential {}: {}",
            dest.display(),
            e
        ),
    }
}

/// Set `key` to `true` in `map`. Returns true when that actually changed
/// something (so the caller only rewrites the file when needed).
fn set_true(map: &mut serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    if map.get(key) == Some(&serde_json::Value::Bool(true)) {
        return false;
    }
    map.insert(key.to_string(), serde_json::Value::Bool(true));
    true
}

/// #930 - stamp the container's first-run state next to the credential we just
/// copied, so the agent's interactive TUI actually uses it. Claude Code gates
/// its onboarding wizard on `hasCompletedOnboarding` in
/// `$CLAUDE_CONFIG_DIR/.claude.json` and the folder-trust dialog on
/// `projects[<cwd>].hasTrustDialogAccepted`, and checks NEITHER against the
/// credential: without these flags a valid copied token still lands on "Select
/// login method" (reproduced in a real container).
///
/// Best-effort, non-destructive, and never aborts the spawn:
/// - merges into an existing JSON object, preserving every other key;
/// - creates the file when it is absent or empty;
/// - SKIPS with a warn when the file is unparseable or is not a JSON object,
///   so user data is never clobbered;
/// - refuses to write through a symlink/junction (F2), like `copy_in`;
/// - writes via a temp file + rename, so a crash cannot truncate the config.
///
/// The config file can hold user identity fields, so contents are never logged.
pub fn ensure_first_run_state(plan: &ContainerCredentialPlan, container_workdir: &str) {
    let Some(state) = plan.first_run else {
        return;
    };
    let Some(dir) = plan.dest.parent() else {
        return;
    };
    let path = dir.join(state.file);

    // F2 - never write through a container-planted symlink/junction, on the
    // config dir or on the config file itself.
    if is_reparse_path(dir) {
        log::warn!(
            "[container-cred] config dir {} is a symlink/reparse point; skipping first-run state",
            dir.display()
        );
        return;
    }
    if is_reparse_path(&path) {
        log::warn!(
            "[container-cred] {} is a symlink/reparse point; skipping first-run state",
            path.display()
        );
        return;
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!(
            "[container-cred] failed to create {}: {}; skipping first-run state",
            dir.display(),
            e
        );
        return;
    }

    let mut root = match std::fs::read_to_string(&path) {
        // An empty file carries no user data, so treat it like a missing one.
        Ok(text) if text.trim().is_empty() => serde_json::Map::new(),
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => {
                log::warn!(
                    "[container-cred] {} is not a JSON object; skipping first-run state",
                    path.display()
                );
                return;
            }
            Err(e) => {
                log::warn!(
                    "[container-cred] {} is not valid JSON ({}); skipping first-run state",
                    path.display(),
                    e
                );
                return;
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => {
            log::warn!(
                "[container-cred] failed to read {}: {}; skipping first-run state",
                path.display(),
                e
            );
            return;
        }
    };

    let mut changed = set_true(&mut root, state.onboarding_flag);

    // `projects[<container_workdir>]` = { "hasTrustDialogAccepted": true, ... }.
    // The mount root IS the container workdir, and AC already trusts it by
    // bind-mounting it, so pre-accepting the folder-trust dialog adds no new
    // exposure. A non-object at either level is left untouched (warn + skip).
    if !state.project_flags.is_empty() {
        let projects = root
            .entry(state.projects_key)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        match projects.as_object_mut() {
            Some(projects) => {
                let entry = projects
                    .entry(container_workdir)
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                match entry.as_object_mut() {
                    Some(entry) => {
                        for flag in state.project_flags {
                            changed |= set_true(entry, flag);
                        }
                    }
                    None => log::warn!(
                        "[container-cred] {} entry for '{}' is not a JSON object; skipping the trust entry",
                        path.display(),
                        container_workdir
                    ),
                }
            }
            None => log::warn!(
                "[container-cred] {} field '{}' is not a JSON object; skipping the trust entry",
                path.display(),
                state.projects_key
            ),
        }
    }

    if !changed {
        log::debug!(
            "[container-cred] first-run state already present in {}",
            path.display()
        );
        return;
    }

    let text = match serde_json::to_string_pretty(&serde_json::Value::Object(root)) {
        Ok(text) => text,
        Err(e) => {
            log::warn!(
                "[container-cred] failed to serialize first-run state for {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    // Temp + rename: a crash mid-write must never truncate an existing config.
    let tmp = dir.join(format!("{}.ac-{}.tmp", state.file, uuid::Uuid::new_v4()));
    if let Err(e) = std::fs::write(&tmp, text.as_bytes()) {
        log::warn!(
            "[container-cred] failed to write first-run state to {}: {}",
            tmp.display(),
            e
        );
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!(
            "[container-cred] failed to install first-run state at {}: {}",
            path.display(),
            e
        );
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    log::info!(
        "[container-cred] ensured container first-run state in {} ({} + trust for {})",
        path.display(),
        state.onboarding_flag,
        container_workdir
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // resolve_plan mutates/reads process env; serialize the env-touching tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn first_run() -> ContainerFirstRunState {
        ContainerFirstRunState {
            file: ".claude.json",
            onboarding_flag: "hasCompletedOnboarding",
            projects_key: "projects",
            project_flags: &["hasTrustDialogAccepted", "hasCompletedProjectOnboarding"],
        }
    }

    fn source(host_dir_env: Option<&'static str>) -> ContainerCredentialSource {
        ContainerCredentialSource {
            host_dir: ".ac-nonexistent-home-930",
            host_dir_env,
            file: ".credentials.json",
            container_dir: ".claude",
            first_run: Some(first_run()),
        }
    }

    /// A plan pointing at `<root>/.claude/.credentials.json`, like resolve_plan
    /// builds for a container Claude session.
    fn plan_for(root: &Path, first_run: Option<ContainerFirstRunState>) -> ContainerCredentialPlan {
        ContainerCredentialPlan {
            source: root.join("unused-host-source"),
            dest: root.join(".claude").join(".credentials.json"),
            first_run,
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
        // #930 - the first-run descriptor must reach the plan: the container
        // backend sees only the plan, never the agent profile.
        assert_eq!(plan.first_run, Some(first_run()));
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
            first_run: None,
        };

        assert_eq!(
            copy_in(&plan).expect("copy_in ok"),
            CopyOutcome::Copied,
            "a real copy must report Copied"
        );
        assert!(dest.is_file(), "dest created");
        assert_eq!(std::fs::read(&dest).unwrap(), b"secret-bytes");

        remove_copied(&dest);
        assert!(!dest.exists(), "dest removed");
        // Idempotent: a second delete of a now-missing file is a no-op.
        remove_copied(&dest);
        assert!(!dest.exists());
    }

    #[test]
    fn ensure_first_run_state_merges_into_existing_config_without_touching_other_keys() {
        // #930 - the replica config dir already holds a seeded .claude.json.
        // Add the flags, preserve everything else (identity fields, other
        // projects), or we would clobber user config.
        let root = tempfile::tempdir().expect("root");
        let dir = root.path().join(".claude");
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join(".claude.json");
        std::fs::write(
            &path,
            br#"{"userID":"abc","oauthAccount":{"emailAddress":"x@y.z"},"projects":{"/other":{"hasTrustDialogAccepted":false}}}"#,
        )
        .expect("seed config");

        ensure_first_run_state(&plan_for(root.path(), Some(first_run())), "/workspace");

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(v["hasCompletedOnboarding"], serde_json::json!(true));
        assert_eq!(
            v["projects"]["/workspace"]["hasTrustDialogAccepted"],
            serde_json::json!(true)
        );
        assert_eq!(
            v["projects"]["/workspace"]["hasCompletedProjectOnboarding"],
            serde_json::json!(true)
        );
        // Untouched: pre-existing keys and unrelated project entries.
        assert_eq!(v["userID"], serde_json::json!("abc"));
        assert_eq!(
            v["oauthAccount"]["emailAddress"],
            serde_json::json!("x@y.z")
        );
        assert_eq!(
            v["projects"]["/other"]["hasTrustDialogAccepted"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn ensure_first_run_state_creates_config_when_absent() {
        // #930 - an empty config dir onboards too, so "no config seed" does not
        // save us: the file must be created.
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join(".claude").join(".claude.json");
        assert!(!path.exists());

        ensure_first_run_state(&plan_for(root.path(), Some(first_run())), "/workspace");

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(v["hasCompletedOnboarding"], serde_json::json!(true));
        assert_eq!(
            v["projects"]["/workspace"]["hasTrustDialogAccepted"],
            serde_json::json!(true)
        );
        assert_eq!(
            v["projects"]["/workspace"]["hasCompletedProjectOnboarding"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn ensure_first_run_state_skips_unparseable_config() {
        // #930 - never clobber a config we cannot parse: warn and leave it byte
        // for byte. The user sees onboarding, which is exactly today's behavior.
        let root = tempfile::tempdir().expect("root");
        let dir = root.path().join(".claude");
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join(".claude.json");
        std::fs::write(&path, b"{not json").expect("seed config");

        ensure_first_run_state(&plan_for(root.path(), Some(first_run())), "/workspace");

        assert_eq!(
            std::fs::read(&path).expect("read"),
            b"{not json".to_vec(),
            "an unparseable config must be left untouched"
        );
    }

    #[test]
    fn ensure_first_run_state_writes_nothing_without_a_descriptor() {
        // #930 - agents with no first-run descriptor (Codex, Antigravity) are
        // untouched: no file, not even the config dir, is created.
        let root = tempfile::tempdir().expect("root");

        ensure_first_run_state(&plan_for(root.path(), None), "/workspace");

        assert!(
            !root.path().join(".claude").exists(),
            "no descriptor must mean no writes at all"
        );
    }

    #[cfg(unix)]
    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[test]
    fn skipped_copy_reports_skipped_and_never_stamps_first_run_state() {
        // grinch Finding 1 - a prior container plants a symlink at the credential
        // leaf (the mount is RW: exactly the F2 threat model). F2 refuses to write
        // through it, so NO token exists. The caller must see SkippedReparse and
        // NOT stamp first-run state: suppressing the login wizard on a container
        // that has no credential would strand the agent unauthenticated, with no
        // way to notice. A skipped copy is not a copy.
        let root = tempfile::tempdir().expect("root");
        let src_dir = tempfile::tempdir().expect("src");
        let src = src_dir.path().join(".credentials.json");
        std::fs::write(&src, b"secret-bytes").expect("write src");

        let dir = root.path().join(".claude");
        std::fs::create_dir_all(&dir).expect("dir");
        let dest = dir.join(".credentials.json");
        let planted = src_dir.path().join("attacker-target");
        if !try_symlink_file(&planted, &dest) {
            // Creating a symlink needs a privilege on Windows; skip cleanly.
            eprintln!("skipping: no symlink privilege on this platform");
            return;
        }

        let plan = ContainerCredentialPlan {
            source: src,
            dest: dest.clone(),
            first_run: Some(first_run()),
        };

        // Mirror the backend call site (container_backend.rs): stamp ONLY on
        // Copied. If copy_in ever reports a skip as Copied again, this takes the
        // Copied arm, stamps, and the assertion below fails.
        match copy_in(&plan).expect("an F2 skip is not an error") {
            CopyOutcome::Copied => ensure_first_run_state(&plan, "/workspace"),
            CopyOutcome::SkippedReparse => {}
        }

        assert!(
            !planted.exists(),
            "F2: must never write the token through the planted link"
        );
        assert!(
            !dir.join(".claude.json").exists(),
            "a skipped copy must not stamp first-run state: with no token, the login wizard is the correct UX"
        );
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
