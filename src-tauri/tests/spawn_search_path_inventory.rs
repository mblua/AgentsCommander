use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a workspace parent")
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

/// Collapse every run of ASCII whitespace to a single space, then drop the
/// space in front of `.`, `(` and `,`, so a cosmetic multiline reflow of a
/// constructor call cannot hide a spawn site.
fn normalized(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" .", ".")
        .replace(" (", "(")
        .replace(" ,", ",")
}

/// Repo-root-relative, forward-slash key. Unlike `pty_writer_inventory.rs`,
/// this strips the WORKSPACE root, so paths under `crates/` resolve too.
fn relative_of(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .expect("source is below workspace root")
        .to_string_lossy()
        .replace('\\', "/")
}

/// The `members` of the workspace `Cargo.toml`, derived rather than listed, so
/// a workspace crate added later is scanned without editing this guard.
fn workspace_members() -> Vec<String> {
    let manifest = std::fs::read_to_string(workspace_root().join("Cargo.toml"))
        .expect("read workspace Cargo.toml");
    let mut members = Vec::new();
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if !inside {
            inside = line.starts_with("members = [");
            continue;
        }
        if line.starts_with(']') {
            break;
        }
        let member = line.trim_matches(|c| c == '"' || c == ',' || c == ' ');
        if !member.is_empty() {
            members.push(member.to_string());
        }
    }
    assert!(!members.is_empty(), "parsed no workspace members");
    assert!(
        members.iter().any(|member| member == "src-tauri"),
        "workspace members {members:?} do not include src-tauri"
    );
    for member in &members {
        assert!(
            workspace_root().join(member).join("src").is_dir(),
            "workspace member {member} has no src directory"
        );
    }
    members
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    /// `config/agent_path.rs`: defines the adapters and runs the login-shell
    /// probe that is the source of the search path.
    Owner,
    /// Already resolves and executes with `effective_search_path()`.
    CoveredBy2589,
    /// The coding-agent PTY launch, covered by P1's
    /// `apply_search_path_to_pty_command`.
    CoveredByP1,
    /// Runs inside a container, where the image's own PATH governs.
    ContainerInternal,
    /// This site is under `#[cfg(windows)]`; Windows lookup stays unchanged.
    WindowsOnly,
    /// This site is inside `#[cfg(test)]` code.
    TestOnly,
    /// A base-system utility that must not be shadowed by a user bin dir.
    PosixBaseUtility,
    /// An `open::` call handing a path or URL to the desktop handler.
    DesktopOpener,
}

use Reason::*;

const REASON_VOCABULARY: &str = "owner, covered-by-2589, covered-by-p1, container-internal, \
windows-only, test-only, posix-base-utility, desktop-opener (precedence in that order, \
test-only first)";

/// file, total constructor occurrences, covered, and the exempt breakdown.
/// Sum of `covered` plus every reason count MUST equal `total`.
type Row = (&'static str, usize, usize, &'static [(Reason, usize)]);

const INVENTORY: &[Row] = &[
    (
        "crates/session-bridge/src/bin/agentscommander-api-helper.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "crates/session-bridge/src/lib.rs",
        1,
        0,
        &[(ContainerInternal, 1)],
    ),
    (
        "src-tauri/src/agent_update.rs",
        6,
        0,
        &[(CoveredBy2589, 2), (TestOnly, 4)],
    ),
    (
        "src-tauri/src/agent_version.rs",
        1,
        0,
        &[(CoveredBy2589, 1)],
    ),
    (
        "src-tauri/src/cli/agency_templates.rs",
        3,
        1,
        &[(WindowsOnly, 1), (PosixBaseUtility, 1)],
    ),
    ("src-tauri/src/cli/harness.rs", 3, 2, &[(WindowsOnly, 1)]),
    ("src-tauri/src/cli/mod.rs", 1, 0, &[(TestOnly, 1)]),
    // #2839: issue_2837_child relaunches the test binary inside #[cfg(test)].
    ("src-tauri/src/cli/task_ops.rs", 1, 0, &[(TestOnly, 1)]),
    (
        "src-tauri/src/commands/ac_discovery.rs",
        32,
        1,
        &[(TestOnly, 31)],
    ),
    (
        "src-tauri/src/commands/config.rs",
        2,
        0,
        &[(DesktopOpener, 1), (TestOnly, 1)],
    ),
    (
        "src-tauri/src/commands/entity_creation.rs",
        7,
        4,
        &[(TestOnly, 3)],
    ),
    ("src-tauri/src/commands/repos.rs", 2, 1, &[(TestOnly, 1)]),
    ("src-tauri/src/commands/session.rs", 1, 0, &[(TestOnly, 1)]),
    (
        "src-tauri/src/commands/wg_delete_diagnostic.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "src-tauri/src/commands/window.rs",
        2,
        0,
        &[(DesktopOpener, 2)],
    ),
    (
        "src-tauri/src/config/agent_config.rs",
        4,
        0,
        &[(TestOnly, 4)],
    ),
    (
        "src-tauri/src/config/agent_memory.rs",
        2,
        0,
        &[(TestOnly, 2)],
    ),
    (
        "src-tauri/src/config/agent_path.rs",
        4,
        0,
        &[(Owner, 1), (TestOnly, 3)],
    ),
    (
        "src-tauri/src/config/coding_agents_catalog.rs",
        5,
        0,
        &[(TestOnly, 5)],
    ),
    (
        "src-tauri/src/config/config_seed.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    ("src-tauri/src/config/daemon_pid.rs", 1, 0, &[(TestOnly, 1)]),
    (
        "src-tauri/src/config/injected_messages.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "src-tauri/src/config/instance_gitignore.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "src-tauri/src/config/local_config_io.rs",
        2,
        0,
        &[(TestOnly, 2)],
    ),
    ("src-tauri/src/config/loops.rs", 3, 0, &[(TestOnly, 3)]),
    (
        "src-tauri/src/config/naming_migration.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    // #2717 (B4a): both inside `#[cfg(test)]`: the one `git` helper that E7 and
    // E17 share, and the `current_exe()` child of E7d and E13.
    (
        "src-tauri/src/config/project_settings.rs",
        2,
        0,
        &[(TestOnly, 2)],
    ),
    ("src-tauri/src/config/session_context.rs", 1, 1, &[]),
    ("src-tauri/src/config/teams.rs", 1, 0, &[(TestOnly, 1)]),
    ("src-tauri/src/lib.rs", 1, 0, &[(TestOnly, 1)]),
    ("src-tauri/src/loops/scheduler.rs", 1, 0, &[(TestOnly, 1)]),
    ("src-tauri/src/path_identity.rs", 1, 0, &[(TestOnly, 1)]),
    ("src-tauri/src/pty/credentials.rs", 4, 0, &[(TestOnly, 4)]),
    ("src-tauri/src/pty/docker_runtime.rs", 1, 1, &[]),
    ("src-tauri/src/pty/git_watcher.rs", 2, 1, &[(TestOnly, 1)]),
    ("src-tauri/src/pty/job.rs", 2, 0, &[(TestOnly, 2)]),
    (
        "src-tauri/src/pty/local_backend.rs",
        4,
        0,
        &[(CoveredByP1, 1), (WindowsOnly, 1), (TestOnly, 2)],
    ),
    ("src-tauri/src/pty/remote_watcher.rs", 2, 2, &[]),
    (
        "src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs",
        10,
        0,
        &[(TestOnly, 10)],
    ),
    (
        "src-tauri/src/pty/terminal_snapshot/resource_tests.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "src-tauri/src/resource_monitor/windows.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "src-tauri/src/session/context_alerts.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "src-tauri/src/session/remote_alerts.rs",
        1,
        0,
        &[(TestOnly, 1)],
    ),
    (
        "src-tauri/src/testability/ui_automation.rs",
        2,
        0,
        &[(TestOnly, 2)],
    ),
];

const NEEDLES: &[&str] = &[
    concat!("Command", "::new("),
    concat!("CommandBuilder", "::new("),
    concat!("open::", "that("),
    concat!("open::", "that_detached("),
    concat!("open::", "with("),
    concat!("open::", "with_detached("),
    concat!("open::", "commands("),
    concat!("open::", "with_command("),
    concat!("open::", "that_in_background("),
    concat!("open::", "with_in_background("),
];

fn constructor_count(body: &str) -> usize {
    NEEDLES.iter().map(|needle| occurrences(body, needle)).sum()
}

/// Every production point where AgentsCommander executes something either
/// applies the #2589 effective search path or is classified exempt with one
/// reason from a fixed vocabulary. For each scanned file the guard checks
/// `total == covered + sum(exempt)`, so an added or removed site anywhere,
/// covered or not, reddens it until a human classifies the site.
///
/// What this guard still cannot see:
///
/// - It cannot tell a production site from a test site; it relies on a human
///   classification and only notices that the COUNT moved. So it cannot catch
///   converting an existing test site into a production one in place, which
///   leaves the total unchanged.
/// - It sees only the `src/` directory of each workspace member. A spawn from
///   `src-tauri/build.rs`, from any `tests/` directory, from a vendored or
///   non-member directory, or through an external crate that spawns internally
///   (as `open` does) is invisible unless its call needle is in `NEEDLES`.
/// - It is a text scan. A command built through a macro, an alias
///   (`use std::process::Command as Proc;`) or a helper returning a `Command`
///   is not matched.
#[test]
fn every_production_spawn_site_is_covered_or_classified() {
    // Needle self-check: a longer identifier ending in `Command(` is not a site.
    for identifier in ["build_agent_spawn_command(", "resolve_agent_spawn_command("] {
        assert_eq!(
            constructor_count(identifier),
            0,
            "{identifier} matched a needle"
        );
    }

    let members = workspace_members();
    let mut scanned = std::collections::BTreeMap::new();
    for member in &members {
        for path in rust_sources(&workspace_root().join(member).join("src")) {
            let body = normalized(&std::fs::read_to_string(&path).expect("read source"));
            let total = constructor_count(&body);
            if total > 0 {
                scanned.insert(relative_of(&path), body);
            }
        }
    }

    // 1. Set equality.
    let scanned_keys: BTreeSet<&str> = scanned.keys().map(String::as_str).collect();
    let inventory_keys: BTreeSet<&str> = INVENTORY.iter().map(|row| row.0).collect();
    assert_eq!(
        INVENTORY.len(),
        inventory_keys.len(),
        "duplicate INVENTORY row"
    );
    let unlisted: Vec<String> = scanned_keys
        .difference(&inventory_keys)
        .map(|file| format!("{file} (total {})", constructor_count(&scanned[*file])))
        .collect();
    let stale: Vec<&&str> = inventory_keys.difference(&scanned_keys).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "spawn census out of date.\nunlisted files (open each, classify every site, add a row):\n  {}\n\
         listed files with no constructor left: {stale:?}\nreasons: {REASON_VOCABULARY}",
        unlisted.join("\n  ")
    );

    // 2. Arithmetic identity, per file.
    for (file, total, covered, exempt) in INVENTORY {
        let declared = covered + exempt.iter().map(|(_, count)| count).sum::<usize>();
        assert_eq!(declared, *total, "{file}: row does not sum to its total");
        let measured = constructor_count(&scanned[*file]);
        assert_eq!(
            measured, *total,
            "{file}: constructor total moved from {total} to {measured}. Open the file, classify \
             the new site as covered or one of: {REASON_VOCABULARY}; then update its row"
        );
    }

    // 3. Decorator count, with no exclusion list.
    for (file, _, covered, _) in INVENTORY.iter().filter(|row| row.2 > 0) {
        let decorators = occurrences(&scanned[*file], concat!("apply_search_path", "_to_"));
        assert_eq!(decorators, *covered, "{file}: decorator count != covered");
    }
    let p1_file = "src-tauri/src/pty/local_backend.rs";
    let p1_row = INVENTORY
        .iter()
        .find(|row| row.0 == p1_file)
        .expect("local_backend.rs row");
    assert_eq!(p1_row.2, 0, "{p1_file} must stay covered 0");
    assert_eq!(
        p1_row
            .3
            .iter()
            .filter(|(reason, _)| *reason == CoveredByP1)
            .map(|(_, n)| n)
            .sum::<usize>(),
        1,
        "{p1_file} must carry exactly one covered-by-p1 site"
    );

    // 4. P1 anti-revert: the full production statement, exactly once.
    let p1_statement = concat!(
        "crate::config::agent_path::",
        "apply_search_path_to_pty_command",
        "(&mut command);"
    );
    assert_eq!(
        occurrences(&scanned[p1_file], p1_statement),
        1,
        "P1 statement"
    );

    // 5. Non-vacuity, including scope.
    assert!(!scanned.is_empty(), "scanned no spawn sites");
    assert!(INVENTORY.iter().any(|row| row.2 > 0), "no covered row");
    assert!(
        INVENTORY.iter().any(|row| !row.3.is_empty()),
        "no exempt row"
    );
    assert!(members.len() > 1, "only one scan root derived");
    assert!(
        scanned.keys().any(|key| key.starts_with("crates/")),
        "no crates/ key"
    );
}
