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

/// Collapse every run of ASCII whitespace (including newlines, so this is also
/// CRLF-safe) to a single space, then drop the space in front of the
/// call-joining punctuation `.`, `(`, and `,`. A method call reflowed across
/// several lines (for example `backend\n    .write(\n    ...`) therefore
/// normalizes to the same text as the single-line call.
///
/// The previous inventory matched raw substrings, so a purely cosmetic
/// multiline reflow of `container_backend.rs` silently dropped a real
/// backend-write site from the set and only a compile-clean but wrong guard
/// remained. Normalizing before matching closes that evasion.
fn normalized(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" .", ".")
        .replace(" (", "(")
        .replace(" ,", ",")
}

fn relative_of(path: &Path) -> String {
    path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("source is below manifest directory")
        .to_string_lossy()
        .replace('\\', "/")
}

/// A test-owned, whitespace-normalized inventory proving that every production
/// PTY writer reaches the single permit-guarded chokepoint and that no code
/// outside `pty::manager` can issue a raw `PtyBackend::write`.
///
/// The strong guarantee is compile-time: `PtyBackend::write` requires a
/// `&BackendWriteAuthority`, whose production constructor `for_route_guard` is
/// private to `pty::manager` and whose test constructor is `#[cfg(test)]`. This
/// test pins that capability so a future change cannot quietly widen it, and it
/// counts the capability rather than a `.write(` substring so a cosmetic reflow
/// cannot hide a new raw-write site the way it defeated the previous guard.
#[test]
fn every_production_pty_writer_is_in_the_explicit_permit_inventory() {
    // 1. Every interactive/automated writer surface routes through the permit.
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

    // 2. Whole-tree scan (whitespace-normalized) of the writer/capability sets.
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut permit_paths = Vec::new();
    let mut raw_backend_paths = Vec::new();
    let mut route_guard_write_paths = Vec::new();
    let mut route_guard_capability_paths = Vec::new();
    for path in rust_sources(&source_root) {
        let relative = relative_of(&path);
        let body = normalized(&source(&relative));
        if body.contains("write_with_permit(") {
            permit_paths.push(relative.clone());
        }
        if body.contains("backend.write(") {
            raw_backend_paths.push(relative.clone());
        }
        if body.contains("route_guard.write(") {
            route_guard_write_paths.push(relative.clone());
        }
        if body.contains("for_route_guard") {
            route_guard_capability_paths.push(relative);
        }
    }
    permit_paths.sort();
    raw_backend_paths.sort();
    route_guard_write_paths.sort();
    route_guard_capability_paths.sort();

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

    // 3. Capability-anchored production write inventory. A raw PtyBackend::write
    //    requires a BackendWriteAuthority; its production constructor
    //    `for_route_guard` is private to pty::manager, so no other module can
    //    even name it. Counting the capability is reflow-proof and, together
    //    with the type system, is the complete production raw-write inventory:
    //    exactly one production call site may issue the authority.
    assert_eq!(
        route_guard_capability_paths,
        vec!["src/pty/manager.rs"],
        "only pty::manager may name the private route-guard write capability"
    );
    let manager = normalized(&source("src/pty/manager.rs"));
    assert!(
        manager.contains("fn for_route_guard() -> Self"),
        "the production route-guard write capability constructor must exist"
    );
    assert!(
        !manager.contains("pub fn for_route_guard")
            && !manager.contains("pub(crate) fn for_route_guard"),
        "for_route_guard must stay private so no other module can issue a raw-write authority"
    );
    assert_eq!(
        occurrences(&manager, "BackendWriteAuthority::for_route_guard()"),
        1,
        "there must be exactly one production raw backend-write chokepoint"
    );
    assert!(
        manager.contains("#[cfg(test)] pub(crate) fn for_backend_test"),
        "the raw-write test capability must stay gated behind cfg(test)"
    );

    // 4. Direct PtyBackend::write callers are limited to the manager chokepoint
    //    (production) and the pinned container-backend transport probes (tests).
    //    Whitespace-normalized so a multiline reflow cannot drop a site.
    assert_eq!(
        raw_backend_paths,
        vec!["src/pty/container_backend.rs", "src/pty/manager.rs"],
        "direct backend writes are limited to the route guard and the pinned container-backend tests"
    );
    assert_eq!(
        occurrences(&normalized(&source("src/pty/container_backend.rs")), "backend.write("),
        3,
        "container backend tests have exactly three direct transport-write probes, each issuing the cfg(test) BackendWriteAuthority"
    );

    // 5. The privileged first-write boundary stays centralized in pty::inject,
    //    and the permitless facade must not reappear.
    assert_eq!(
        route_guard_write_paths,
        vec!["src/pty/inject.rs"],
        "the privileged first-write boundary must remain centralized in pty::inject"
    );
    assert!(manager.contains("pub fn write_with_permit"));
    assert!(manager.contains("pub async fn acquire_input_writer"));
    assert!(
        !manager.contains("pub fn write(&self, id: Uuid"),
        "a public permitless PtyManager::write facade must not return"
    );

    let mailbox = normalized(&source("src/phone/mailbox.rs"));
    assert_eq!(
        occurrences(&mailbox, "write_exact_agent_input_first("),
        1,
        "shared PTY actuation must use the single synchronous first-write boundary"
    );
    let inject = normalized(&source("src/pty/inject.rs"));
    assert_eq!(
        occurrences(&inject, "route_guard.write(bytes)"),
        1,
        "exact text must be one backend call"
    );
    assert!(inject.contains("PtyManager::write_with_permit(permit, b\"\\r\")"));
}
