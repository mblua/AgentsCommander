use super::*;
use std::time::Instant;

#[test]
fn response_close_menu_binding_preserves_shared_identity_and_resolves_episode() {
    let state = crate::pty::manager::ResponseCloseState::default();
    let menu = MenuGuard::with_response_close(BlockingMenusStore::shipped_only(), state.clone());
    assert!(menu.response_close_state().unwrap().same_state(&state));
    let id = Uuid::new_v4();
    state.register_route(id, 1, Instant::now());
    let evaluation = menu.evaluate_logical_rows(id, &[], &[]);
    assert!(!evaluation.is_blocked);
    assert!(menu.resolve_current_episode(id));
    assert!(!menu.is_blocked(id));
    assert!(MenuGuard::new().response_close_state().is_none());
}
