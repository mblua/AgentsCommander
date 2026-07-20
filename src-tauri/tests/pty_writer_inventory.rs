use std::path::{Path, PathBuf};

fn source(relative: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}

fn occurrences(body: &str, needle: &str) -> usize {
    body.match_indices(needle).count()
}

#[test]
fn every_production_pty_writer_is_in_the_explicit_permit_inventory() {
    let expected_surfaces = [
        "src/commands/pty.rs",
        "src/web/mod.rs",
        "src/web/commands.rs",
        "src/pty/inject.rs",
        "src/phone/mailbox.rs",
    ];
    for relative in expected_surfaces {
        let body = source(relative);
        assert!(
            body.contains("write_with_permit("),
            "{relative} must route PTY input through write_with_permit"
        );
        assert!(
            body.contains("acquire_input_writer("),
            "{relative} must acquire the per-session input permit"
        );
    }

    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut permit_paths = Vec::new();
    let mut raw_backend_paths = Vec::new();
    let mut route_guard_write_paths = Vec::new();
    for path in rust_sources(&source_root) {
        let relative = path
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("source is below manifest directory")
            .to_string_lossy()
            .replace('\\', "/");
        let body = source(&relative);
        if body.contains("write_with_permit(") {
            permit_paths.push(relative.clone());
        }
        if body.contains("backend.write(") {
            raw_backend_paths.push(relative.clone());
        }
        if body.contains("route_guard.write(") {
            route_guard_write_paths.push(relative);
        }
    }

    permit_paths.sort();
    assert_eq!(
        permit_paths,
        vec![
            "src/commands/pty.rs",
            "src/phone/mailbox.rs",
            "src/pty/inject.rs",
            "src/pty/manager.rs",
            "src/web/commands.rs",
            "src/web/mod.rs",
        ],
        "a new PTY writer must be deliberately added to this complete permit inventory"
    );
    assert_eq!(
        raw_backend_paths,
        vec!["src/pty/container_backend.rs", "src/pty/manager.rs"],
        "direct backend writes are limited to the route guard and pinned backend tests"
    );
    assert_eq!(
        occurrences(&source("src/pty/container_backend.rs"), "backend.write("),
        2,
        "container backend tests have exactly two direct transport-write probes"
    );
    assert_eq!(
        route_guard_write_paths,
        vec!["src/pty/inject.rs"],
        "the privileged first-write boundary must remain centralized in pty::inject"
    );

    let manager = source("src/pty/manager.rs");
    assert!(manager.contains("pub fn write_with_permit"));
    assert!(manager.contains("pub async fn acquire_input_writer"));
    assert_eq!(
        occurrences(&manager, "self.backend.write(self.session_id, bytes)"),
        1,
        "the route guard has exactly one raw backend-write chokepoint"
    );
    assert!(
        !manager.contains("pub fn write(&self, id: Uuid"),
        "a public permitless PtyManager::write facade must not return"
    );

    let mailbox = source("src/phone/mailbox.rs");
    assert_eq!(
        occurrences(&mailbox, "write_exact_agent_input_first("),
        1,
        "shared PTY actuation must use the single synchronous first-write boundary"
    );
    let inject = source("src/pty/inject.rs");
    assert_eq!(
        occurrences(&inject, "route_guard.write(bytes)"),
        1,
        "exact text must be one backend call"
    );
    assert!(inject.contains("PtyManager::write_with_permit(permit, b\"\\r\")"));
}
