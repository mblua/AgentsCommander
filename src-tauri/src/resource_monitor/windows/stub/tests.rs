use super::*;

#[test]
fn non_windows_production_backend_disables_process_tree_enforcement() {
    let backend = PlatformProcessTreeBackend::new();

    assert!(!backend.supports_process_tree_enforcement());
}
