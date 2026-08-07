//! Structured dependency guard for the Claude watcher output seam.
//!
//! The guard parses Rust syntax and records canonical internal module dependencies.
//! Later hardening stages extend the same engine with compiler-faithful module-tree
//! discovery, complete alias resolution, move proofs, and live diagnostics.

use std::{
    collections::BTreeSet,
    fmt, fs,
    path::{Path, PathBuf},
};

use syn::{
    ext::IdentExt,
    visit::{self, Visit},
    ItemMod, ItemUse, Path as SynPath, UseTree, Visibility,
};

const CRATE_ID: &str = "agentscommander_lib";
const TARGET_MODULE: &str = "agentscommander_lib::telegram::claude_watcher";
const TARGET_ROOT_SOURCE: &str = "src/telegram/claude_watcher.rs";
const OUTPUT_MODULE: &str = "agentscommander_lib::telegram::claude_watcher::output";
const OUTPUT_SOURCE: &str = "src/telegram/claude_watcher/output.rs";
const FOCUSED_RERUN: &str = "cargo test --test claude_watcher_layering -- --nocapture";

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
            "contract: {}\n{}\nfocused rerun: {}",
            self.contract, self.detail, FOCUSED_RERUN
        )
    }
}

impl std::error::Error for GuardError {}

fn unraw(ident: &syn::Ident) -> String {
    ident.unraw().to_string()
}

fn render_relative(root: &Path, path: &Path) -> Result<String, GuardError> {
    let relative = path.strip_prefix(root).map_err(|error| {
        GuardError::new(
            "manifest containment",
            format!(
                "canonical source {} is outside canonical manifest root {}: {error}",
                path.display(),
                root.display()
            ),
        )
    })?;
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn is_exact_cfg_test(item: &ItemMod) -> bool {
    item.attrs.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .parse_args::<syn::Ident>()
                .is_ok_and(|ident| unraw(&ident) == "test")
    })
}

fn expand_use_tree(tree: &UseTree, prefix: &mut Vec<String>, leaves: &mut Vec<Vec<String>>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(unraw(&path.ident));
            expand_use_tree(&path.tree, prefix, leaves);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let name = unraw(&name.ident);
            if name == "self" {
                leaves.push(prefix.clone());
            } else {
                prefix.push(name);
                leaves.push(prefix.clone());
                prefix.pop();
            }
        }
        UseTree::Rename(rename) => {
            let name = unraw(&rename.ident);
            if name == "self" {
                leaves.push(prefix.clone());
            } else {
                prefix.push(name);
                leaves.push(prefix.clone());
                prefix.pop();
            }
        }
        UseTree::Group(group) => {
            for item in &group.items {
                expand_use_tree(item, prefix, leaves);
            }
        }
        UseTree::Glob(_) => leaves.push(prefix.clone()),
    }
}

fn resolve_internal_module(
    current_module: &str,
    segments: &[String],
    crate_id: &str,
    declared_modules: &BTreeSet<String>,
) -> Option<String> {
    if segments.is_empty() {
        return None;
    }

    let mut canonical = if segments[0] == "crate" || segments[0] == crate_id {
        vec![crate_id.to_owned()]
    } else if segments[0] == "self" || segments[0] == "super" {
        current_module
            .split("::")
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        let root_candidate = format!("{crate_id}::{}", segments[0]);
        if declared_modules.iter().any(|module| {
            module == &root_candidate || module.starts_with(&(root_candidate.clone() + "::"))
        }) {
            vec![crate_id.to_owned()]
        } else {
            return None;
        }
    };

    let mut index = 0;
    if segments[0] == "crate" || segments[0] == crate_id || segments[0] == "self" {
        index = 1;
    } else if segments[0] != "super" {
        canonical.push(segments[0].clone());
        index = 1;
    }

    while index < segments.len() && segments[index] == "super" {
        if canonical.len() <= 1 {
            return None;
        }
        canonical.pop();
        index += 1;
    }
    canonical.extend(segments[index..].iter().cloned());

    for end in (1..=canonical.len()).rev() {
        let candidate = canonical[..end].join("::");
        if declared_modules.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

struct DependencyVisitor<'a> {
    source: &'a str,
    current_module: &'a str,
    crate_id: &'a str,
    declared_modules: &'a BTreeSet<String>,
    observations: BTreeSet<DependencyObservation>,
}

impl DependencyVisitor<'_> {
    fn observe(&mut self, segments: Vec<String>) {
        if let Some(module) = resolve_internal_module(
            self.current_module,
            &segments,
            self.crate_id,
            self.declared_modules,
        ) {
            if module != self.current_module {
                self.observations.insert(DependencyObservation {
                    source: self.source.to_owned(),
                    module,
                });
            }
        }
    }
}

impl<'ast> Visit<'ast> for DependencyVisitor<'_> {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if !is_exact_cfg_test(item) {
            visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let mut leaves = Vec::new();
        expand_use_tree(&item.tree, &mut Vec::new(), &mut leaves);
        for leaf in leaves {
            self.observe(leaf);
        }
    }

    fn visit_path(&mut self, path: &'ast SynPath) {
        self.observe(
            path.segments
                .iter()
                .map(|segment| unraw(&segment.ident))
                .collect(),
        );
        visit::visit_path(self, path);
    }

    fn visit_visibility(&mut self, _visibility: &'ast Visibility) {
        // Interface visibility is checked structurally in the move-proof stage.
    }
}

fn analyze_guard(
    manifest_root: &Path,
    crate_root_file: &Path,
    crate_id: &str,
    target_module: &str,
    source_specs: &[SourceSpec],
    declared_modules: &BTreeSet<String>,
) -> Result<GuardReport, GuardError> {
    if crate_id.is_empty() || target_module.is_empty() {
        return Err(GuardError::new(
            "guard configuration",
            "crate ID and target module must be explicit and nonempty",
        ));
    }
    if source_specs.is_empty() {
        return Err(GuardError::new(
            "target source discovery",
            format!("zero source files found for target module {target_module}"),
        ));
    }

    let canonical_manifest = fs::canonicalize(manifest_root).map_err(|error| {
        GuardError::new(
            "manifest canonicalization",
            format!(
                "could not canonicalize {}: {error}",
                manifest_root.display()
            ),
        )
    })?;
    let canonical_crate_root = fs::canonicalize(crate_root_file).map_err(|error| {
        GuardError::new(
            "crate-root selection",
            format!(
                "could not canonicalize {}: {error}",
                crate_root_file.display()
            ),
        )
    })?;
    render_relative(&canonical_manifest, &canonical_crate_root)?;

    let mut report = GuardReport {
        sources: BTreeSet::new(),
        dependencies: BTreeSet::new(),
    };
    for spec in source_specs {
        let canonical_source = fs::canonicalize(&spec.path).map_err(|error| {
            GuardError::new(
                "target source read",
                format!("could not canonicalize {}: {error}", spec.path.display()),
            )
        })?;
        let relative_source = render_relative(&canonical_manifest, &canonical_source)?;
        let source = fs::read_to_string(&canonical_source).map_err(|error| {
            GuardError::new(
                "target source read",
                format!("could not read {}: {error}", canonical_source.display()),
            )
        })?;
        let syntax = syn::parse_file(&source).map_err(|error| {
            GuardError::new(
                "target source parse",
                format!("could not parse {relative_source}: {error}"),
            )
        })?;
        let mut visitor = DependencyVisitor {
            source: &relative_source,
            current_module: &spec.module_id,
            crate_id,
            declared_modules,
            observations: BTreeSet::new(),
        };
        visitor.visit_file(&syntax);
        report.sources.insert(relative_source.clone());
        report.dependencies.extend(visitor.observations);
    }
    Ok(report)
}

fn production_modules() -> BTreeSet<String> {
    [
        CRATE_ID,
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

fn production_sources(manifest_root: &Path) -> Vec<SourceSpec> {
    [
        (TARGET_MODULE, TARGET_ROOT_SOURCE),
        (OUTPUT_MODULE, OUTPUT_SOURCE),
    ]
    .into_iter()
    .map(|(module_id, relative)| SourceSpec {
        module_id: module_id.to_owned(),
        path: manifest_root.join(relative),
    })
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
        (OUTPUT_SOURCE, "agentscommander_lib::config"),
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

#[test]
fn production_guard_observes_the_exact_initial_dependency_set() {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crate_root = manifest_root.join("src/lib.rs");
    let sources = production_sources(&manifest_root);
    let modules = production_modules();

    let first = analyze_guard(
        &manifest_root,
        &crate_root,
        CRATE_ID,
        TARGET_MODULE,
        &sources,
        &modules,
    )
    .expect("production guard should parse and resolve");
    let second = analyze_guard(
        &manifest_root,
        &crate_root,
        CRATE_ID,
        TARGET_MODULE,
        &sources,
        &modules,
    )
    .expect("second production guard run should parse and resolve");

    assert_eq!(first, second, "guard observations must be deterministic");
    assert_eq!(
        first.sources,
        [TARGET_ROOT_SOURCE.to_owned(), OUTPUT_SOURCE.to_owned()]
            .into_iter()
            .collect(),
        "initial source set must be explicit"
    );
    assert_eq!(
        first.dependencies,
        expected_dependencies(),
        "structured dependency set mismatch; rerun with {FOCUSED_RERUN}"
    );
}

#[test]
fn zero_target_sources_fail_closed() {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let error = analyze_guard(
        &manifest_root,
        &manifest_root.join("src/lib.rs"),
        CRATE_ID,
        TARGET_MODULE,
        &[],
        &production_modules(),
    )
    .expect_err("zero target sources must not pass vacuously");

    let message = error.to_string();
    assert!(message.contains("target source discovery"));
    assert!(message.contains("zero source files"));
    assert!(message.contains(TARGET_MODULE));
    assert!(message.contains(FOCUSED_RERUN));
}
