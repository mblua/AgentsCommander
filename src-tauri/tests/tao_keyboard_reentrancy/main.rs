//! #2817 regression harness for the `tao` keyboard reentrancy deadlock.
//!
//! `tao 0.34.8` held a global, non-reentrant `KEY_EVENT_BUILDERS` mutex while
//! its `WM_KEYUP` branch called `PeekMessageW`. That peek delivers pending sent
//! messages, so a `WM_KILLFOCUS` arriving at that instant reentered the same
//! callback and the event-loop thread waited on itself forever.
//!
//! T1 pins the resolved `tao` floor and is portable. T2 drives the reentrancy
//! on a hidden window the test owns, and lives in `win_harness`.

use std::path::Path;

const TAO_FLOOR: (u64, u64, u64) = (0, 37, 1);

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// The `version` of every `[[package]]` block named `tao`.
fn tao_versions(lock: &str) -> Vec<String> {
    lock.split("[[package]]")
        .filter(|block| block.lines().any(|line| line == "name = \"tao\""))
        .filter_map(|block| {
            block
                .lines()
                .find_map(|line| line.strip_prefix("version = \""))
                .map(|rest| rest.trim_end_matches('"').to_string())
        })
        .collect()
}

#[test]
fn resolved_tao_is_at_least_0_37_1() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|err| panic!("#2817: cannot read {}: {err}", lock_path.display()));
    let versions = tao_versions(&lock);
    assert_eq!(
        versions.len(),
        1,
        "#2817: expected exactly one resolved tao package, found {versions:?}; \
         a second tao can hide the KEY_EVENT_BUILDERS reentrancy deadlock"
    );
    let found = &versions[0];
    let parsed = parse_version(found)
        .unwrap_or_else(|| panic!("#2817: unparseable tao version {found:?} in Cargo.lock"));
    assert!(
        parsed >= TAO_FLOOR,
        "#2817: resolved tao is {found}, below 0.37.1; tao < 0.37.1 reintroduces \
         the KEY_EVENT_BUILDERS reentrancy deadlock"
    );
}

#[cfg(windows)]
mod win_harness;
