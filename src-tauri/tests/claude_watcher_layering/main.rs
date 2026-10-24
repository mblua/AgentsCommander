//! Structured dependency guard for the Telegram output seam.
//!
//! `telegram::output` is the single owner of the output/logging cluster, sitting
//! below its three consumers (`telegram::bridge` and the Claude and Codex
//! watchers), which import from it directly with no intermediate re-export.
//!
//! The guard parses Rust syntax and records canonical internal module dependencies.
//! Later hardening stages extend the same engine with compiler-faithful module-tree
//! discovery, complete alias resolution, move proofs, and live diagnostics.
//!
//! Known blind spots:
//!
//! 1. Macro and proc-macro token expansion is not performed. Qualified invocation
//!    paths are observed, but generated tokens and `include!` contents are not.
//! 2. Trait, generic, and receiver-method dispatch without a syntactic module path
//!    is not type-resolved.
//! 3. Build-script or proc-macro generated source without a literal Rust `mod`
//!    declaration is outside the module-tree traversal.
//! 4. Semantic identity introduced only through an outside re-export chain can
//!    exceed the local alias resolver. The final topology detector remains authoritative.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use syn::{
    ext::IdentExt,
    meta::ParseNestedMeta,
    visit::{self, Visit},
    Block, Expr, Item, ItemExternCrate, ItemMod, ItemUse, Lit, Macro, Meta, Path as SynPath, Stmt,
    UseTree, Visibility,
};

const CRATE_ID: &str = "agentscommander_lib";
const TARGET_MODULE: &str = "agentscommander_lib::telegram::claude_watcher";
const TARGET_ROOT_SOURCE: &str = "src/telegram/claude_watcher.rs";
const OUTPUT_MODULE: &str = "agentscommander_lib::telegram::output";
const OUTPUT_SOURCE: &str = "src/telegram/output.rs";
const FOCUSED_RERUN: &str = "cargo test --test claude_watcher_layering -- --nocapture";
const AUTHORITATIVE_TOPOLOGY: &str = "rust-module-dependency-cycles 1.1.0";
const SCOPE_TEXT: &str = "SCOPE: this guard is a syntax and compiler module-tree regression \
        net. 1. Macro/proc-macro expansion and include! tokens are not \
        performed. 2. Trait, generic, and receiver dispatch without a \
        syntactic path is not type-resolved. 3. Generated source without \
        a literal mod declaration is outside traversal. 4. Identity \
        available only through an outside re-export chain can exceed the \
        local alias resolver.";

static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DependencyObservation {
    source: String,
    module: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GuardReport {
    sources: BTreeSet<String>,
    dependencies: BTreeSet<DependencyObservation>,
}

#[derive(Clone, Debug)]
struct SourceSpec {
    module_id: String,
    path: PathBuf,
}

#[derive(Clone)]
struct ModuleBody {
    module_id: String,
    source: PathBuf,
    relative_source: String,
    body_id: String,
    descendant_base: PathBuf,
    inline: bool,
    items: Vec<Item>,
}

impl fmt::Debug for ModuleBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModuleBody")
            .field("module_id", &self.module_id)
            .field("source", &self.source)
            .field("relative_source", &self.relative_source)
            .field("body_id", &self.body_id)
            .field("descendant_base", &self.descendant_base)
            .field("inline", &self.inline)
            .field("item_count", &self.items.len())
            .finish()
    }
}

#[derive(Clone, Debug)]
struct ModuleIndex {
    bodies: Vec<ModuleBody>,
    declared_modules: BTreeSet<String>,
    test_only_sources: BTreeSet<String>,
}

#[derive(Debug)]
struct GuardError {
    contract: &'static str,
    detail: String,
}

impl GuardError {
    fn new(contract: &'static str, detail: impl Into<String>) -> Self {
        Self {
            contract,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for GuardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "CONTRACT: {}\n{}\nfocused rerun: {}\n{}\nauthoritative topology: {}",
            self.contract, self.detail, FOCUSED_RERUN, SCOPE_TEXT, AUTHORITATIVE_TOPOLOGY
        )
    }
}

impl std::error::Error for GuardError {}

// #2232 phase 4: the sets below are **unchanged**, and deliberately so. Phase 4
// gave `claude_watcher` a destination `watch`, per-line preamble starts and a
// live `Option<Sender>`, but every one of those is either std, `tokio`, or a
// name the watcher already carried: `capture::record` for the record,
// `capture::key` and `capture::state` for the attachment, and `jsonl_kernel`
// for the preamble window constants. A restart now releases the old reader and
// raises a fresh one on the new session UUID, so no re-anchor signal exists and
// no watcher gained a reference to the supervisor. No new module arc was
// created, so no row is added — adding one to satisfy a plan row would break the
// equality pin the plan tells us not to relax.
fn production_modules() -> BTreeSet<String> {
    [
        CRATE_ID,
        // #2232 phase 1: the capture record module the watchers now depend on.
        "agentscommander_lib::capture",
        // #2232 phase 3: the epoch predicate and the observation prefix cap the
        // watcher now names, so the record carries a real epoch.
        "agentscommander_lib::capture::key",
        "agentscommander_lib::capture::record",
        "agentscommander_lib::capture::state",
        "agentscommander_lib::capture::turn_codex", // #2232 phase 5
        "agentscommander_lib::config",
        "agentscommander_lib::network",
        "agentscommander_lib::telegram",
        "agentscommander_lib::telegram::api",
        "agentscommander_lib::telegram::bridge",
        TARGET_MODULE,
        OUTPUT_MODULE,
        "agentscommander_lib::telegram::jsonl_kernel",
        "agentscommander_lib::telegram::redact",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn expected_dependencies() -> BTreeSet<DependencyObservation> {
    [
        (TARGET_ROOT_SOURCE, "agentscommander_lib::network"),
        (TARGET_ROOT_SOURCE, OUTPUT_MODULE),
        (
            TARGET_ROOT_SOURCE,
            "agentscommander_lib::telegram::jsonl_kernel",
        ),
        // #2232 phase 1: the watcher emits one capture record per accepted
        // assistant record.
        (TARGET_ROOT_SOURCE, "agentscommander_lib::capture::record"),
        // #2232 phase 3: the pure epoch predicate. The watcher evaluates it
        // over bytes it has already read; it takes no lock and writes nothing.
        (TARGET_ROOT_SOURCE, "agentscommander_lib::capture::key"),
        // #2232 phase 3: the observation prefix cap `N` and its pure helper.
        // Both are constants and a byte slice; `capture::state`'s disk and lock
        // work stays on the supervisor thread.
        (TARGET_ROOT_SOURCE, "agentscommander_lib::capture::state"),
    ]
    .into_iter()
    .map(|(source, module)| DependencyObservation {
        source: source.to_owned(),
        module: module.to_owned(),
    })
    .collect()
}

// The `config::instance_artifacts` row is the #1446 artifact registry: a
// pure-constants leaf with zero outgoing arcs in `src-tauri/module-arcs.txt`,
// so an arc into it can neither create, grow, nor join any SCC (the same
// argument `instance_gitignore_layering` records for its own row). It grants
// the output seam no new capability: no I/O, no transport, no telegram
// surface. It only lets the module name its three log artifacts
// (`telegram-bridge.log`, `diag-raw.log`, `diag-sent.log`) through the
// registry, so a rename breaks the build instead of silently reopening the
// instance `.gitignore` coverage gap. The set stays equality-pinned: any
// sixth dependency still turns this guard red.
fn expected_output_dependencies() -> BTreeSet<DependencyObservation> {
    [
        (OUTPUT_SOURCE, "agentscommander_lib::config"),
        (
            OUTPUT_SOURCE,
            "agentscommander_lib::config::instance_artifacts",
        ),
        (OUTPUT_SOURCE, "agentscommander_lib::network"),
        (OUTPUT_SOURCE, "agentscommander_lib::telegram::api"),
        (OUTPUT_SOURCE, "agentscommander_lib::telegram::redact"),
    ]
    .into_iter()
    .map(|(source, module)| DependencyObservation {
        source: source.to_owned(),
        module: module.to_owned(),
    })
    .collect()
}

fn require_exact_source_set(
    report: &GuardReport,
    expected: &BTreeSet<String>,
    target_module: &str,
) -> Result<(), GuardError> {
    let missing = expected
        .difference(&report.sources)
        .cloned()
        .collect::<Vec<_>>();
    let unexpected = report
        .sources
        .difference(expected)
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }
    Err(GuardError::new(
        "target source-set equality",
        format!(
            "target {target_module}; missing expected sources: {missing:?}; \
             unexpected observed sources: {unexpected:?}"
        ),
    ))
}

fn require_exact_production_report(report: &GuardReport) -> Result<(), GuardError> {
    require_exact_report(
        report,
        [TARGET_ROOT_SOURCE.to_owned()].into_iter().collect(),
        expected_dependencies(),
        "Claude watcher dependency and source sets",
    )
}

fn require_exact_output_report(report: &GuardReport) -> Result<(), GuardError> {
    require_exact_report(
        report,
        [OUTPUT_SOURCE.to_owned()].into_iter().collect(),
        expected_output_dependencies(),
        "Telegram output dependency and source sets",
    )
}

fn require_exact_report(
    report: &GuardReport,
    expected_sources: BTreeSet<String>,
    expected_dependencies: BTreeSet<DependencyObservation>,
    contract_label: &'static str,
) -> Result<(), GuardError> {
    let missing_sources = expected_sources
        .difference(&report.sources)
        .cloned()
        .collect::<Vec<_>>();
    let unexpected_sources = report
        .sources
        .difference(&expected_sources)
        .cloned()
        .collect::<Vec<_>>();
    let missing_dependencies = expected_dependencies
        .difference(&report.dependencies)
        .cloned()
        .collect::<Vec<_>>();
    let unexpected_dependencies = report
        .dependencies
        .difference(&expected_dependencies)
        .cloned()
        .collect::<Vec<_>>();
    if missing_sources.is_empty()
        && unexpected_sources.is_empty()
        && missing_dependencies.is_empty()
        && unexpected_dependencies.is_empty()
    {
        return Ok(());
    }
    Err(GuardError::new(
        contract_label,
        format!(
            "missing expected source rows: {missing_sources:?}; unexpected \
             observed source rows: {unexpected_sources:?}; missing expected \
             dependency rows: {missing_dependencies:?}; unexpected observed \
             dependency rows: {unexpected_dependencies:?}"
        ),
    ))
}

use dependencies::{analyze_guard, analyze_module_index, expand_use_tree, UseLeaf};
use fixtures::{
    inline_spelling_report, out_of_line_spelling_report, FailingDiagnostic, Fixture,
    RecordingDiagnostic,
};
use interface::{
    collect_moved_symbol_occurrences, require_visibility, verify_exact_interface,
    verify_exact_moved_symbols, RequiredVisibility,
};
use module_tree::{build_module_index, canonicalize, render_relative, unraw};

mod dependencies;
mod fixtures;
mod interface;
mod module_cases;
mod module_tree;
mod production;
mod spelling;
