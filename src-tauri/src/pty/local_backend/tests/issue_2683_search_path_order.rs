use crate::pty::git_watcher::GitWatcher;
use crate::session::manager::SessionManager;
use std::sync::Arc;

// Split so this file never contains a needle whole; see the no-self-match rule.
const N_APPLY: &str = concat!("apply_search_path", "_to_pty_command(&mut command)");
const N_REMOVE: &str = concat!("for key in ", "&env_remove", "_keys");
const N_CONFIGURED: &str = concat!("for (key, value) in ", "&configured", "_env");

fn only_offset(source: &str, needle: &str) -> usize {
    let hits: Vec<usize> = source.match_indices(needle).map(|(at, _)| at).collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one {needle:?}, got {hits:?}"
    );
    hits[0]
}

#[test]
fn issue_2683_spawn_sync_applies_the_search_path_before_the_env_loops() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pty/local_backend.rs");
    let source = std::fs::read_to_string(&path).expect("read local_backend.rs");
    let apply = only_offset(&source, N_APPLY);
    let remove = only_offset(&source, N_REMOVE);
    let configured = only_offset(&source, N_CONFIGURED);
    assert!(
        apply < remove,
        "search path must be applied before the removal loop"
    );
    assert!(
        apply < configured,
        "search path must be applied before the configured-env loop"
    );
}

#[test]
fn issue_2683_git_watcher_production_constructor_still_requires_an_app_handle() {
    let _: fn(Arc<tokio::sync::RwLock<SessionManager>>, tauri::AppHandle) -> Arc<GitWatcher> =
        GitWatcher::new;
}
