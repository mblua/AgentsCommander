//! Structured dependency guard for the Claude watcher output seam.
//!
//! The guard parses Rust syntax and records canonical internal module dependencies.
//! Later hardening stages extend the same engine with compiler-faithful module-tree
//! discovery, complete alias resolution, move proofs, and live diagnostics.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use syn::{
    ext::IdentExt,
    visit::{self, Visit},
    Expr, Item, ItemMod, ItemUse, Lit, Meta, Path as SynPath, UseTree, Visibility,
};

const CRATE_ID: &str = "agentscommander_lib";
const TARGET_MODULE: &str = "agentscommander_lib::telegram::claude_watcher";
const TARGET_ROOT_SOURCE: &str = "src/telegram/claude_watcher.rs";
const OUTPUT_MODULE: &str = "agentscommander_lib::telegram::claude_watcher::output";
const OUTPUT_SOURCE: &str = "src/telegram/claude_watcher/output.rs";
const FOCUSED_RERUN: &str = "cargo test --test claude_watcher_layering -- --nocapture";

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

fn direct_path_attribute(item: &ItemMod) -> Result<Option<PathBuf>, GuardError> {
    let mut paths = Vec::new();
    for attribute in &item.attrs {
        if !attribute.path().is_ident("path") {
            continue;
        }
        let Meta::NameValue(name_value) = &attribute.meta else {
            return Err(GuardError::new(
                "module path attribute",
                format!(
                    "module {} has a non-name-value path attribute",
                    unraw(&item.ident)
                ),
            ));
        };
        let Expr::Lit(expression) = &name_value.value else {
            return Err(GuardError::new(
                "module path attribute",
                format!("module {} has a non-literal path value", unraw(&item.ident)),
            ));
        };
        let Lit::Str(value) = &expression.lit else {
            return Err(GuardError::new(
                "module path attribute",
                format!("module {} has a non-string path value", unraw(&item.ident)),
            ));
        };
        paths.push(PathBuf::from(value.value()));
    }
    if paths.len() > 1 {
        return Err(GuardError::new(
            "module path attribute",
            format!(
                "module {} has duplicate direct path attributes",
                unraw(&item.ident)
            ),
        ));
    }
    Ok(paths.pop())
}

fn descendant_base_for_source(source: &Path) -> Result<PathBuf, GuardError> {
    let parent = source.parent().ok_or_else(|| {
        GuardError::new(
            "module source base",
            format!("module source has no parent: {}", source.display()),
        )
    })?;
    if source.file_name().is_some_and(|name| name == "mod.rs") {
        Ok(parent.to_path_buf())
    } else {
        let stem = source.file_stem().ok_or_else(|| {
            GuardError::new(
                "module source base",
                format!("module source has no file stem: {}", source.display()),
            )
        })?;
        Ok(parent.join(stem))
    }
}

struct ModuleTreeBuilder {
    manifest_root: PathBuf,
    index: ModuleIndex,
    out_of_line_owners: BTreeMap<PathBuf, String>,
    active_sources: Vec<PathBuf>,
}

impl ModuleTreeBuilder {
    fn new(manifest_root: PathBuf) -> Self {
        Self {
            manifest_root,
            index: ModuleIndex {
                bodies: Vec::new(),
                declared_modules: BTreeSet::new(),
            },
            out_of_line_owners: BTreeMap::new(),
            active_sources: Vec::new(),
        }
    }

    fn canonical_source(
        &self,
        path: &Path,
        operation: &'static str,
    ) -> Result<PathBuf, GuardError> {
        let canonical = fs::canonicalize(path).map_err(|error| {
            GuardError::new(
                operation,
                format!("could not canonicalize {}: {error}", path.display()),
            )
        })?;
        render_relative(&self.manifest_root, &canonical)?;
        Ok(canonical)
    }

    fn parse_source(&self, source: &Path) -> Result<Vec<Item>, GuardError> {
        let relative = render_relative(&self.manifest_root, source)?;
        let text = fs::read_to_string(source).map_err(|error| {
            GuardError::new(
                "module source read",
                format!("could not read {relative}: {error}"),
            )
        })?;
        syn::parse_file(&text)
            .map(|file| file.items)
            .map_err(|error| {
                GuardError::new(
                    "module source parse",
                    format!("could not parse {relative}: {error}"),
                )
            })
    }

    fn claim_out_of_line_source(
        &mut self,
        module_id: &str,
        source: &Path,
    ) -> Result<(), GuardError> {
        if self.active_sources.iter().any(|active| active == source) {
            return Err(GuardError::new(
                "active module source cycle",
                format!(
                    "module {module_id} revisits active out-of-line source {}",
                    render_relative(&self.manifest_root, source)?
                ),
            ));
        }
        if let Some(owner) = self.out_of_line_owners.get(source) {
            if owner != module_id {
                return Err(GuardError::new(
                    "out-of-line source ownership",
                    format!(
                        "source {} is claimed by both {owner} and {module_id}",
                        render_relative(&self.manifest_root, source)?
                    ),
                ));
            }
        } else {
            self.out_of_line_owners
                .insert(source.to_path_buf(), module_id.to_owned());
        }
        Ok(())
    }

    fn conventional_source(
        &self,
        parent: &ModuleBody,
        item: &ItemMod,
    ) -> Result<PathBuf, GuardError> {
        let name = unraw(&item.ident);
        let direct_path = direct_path_attribute(item)?;
        let candidates = if let Some(path) = direct_path {
            let declaration_base = if parent.inline {
                parent.descendant_base.clone()
            } else {
                parent
                    .source
                    .parent()
                    .ok_or_else(|| {
                        GuardError::new(
                            "module declaration base",
                            format!("source has no parent: {}", parent.source.display()),
                        )
                    })?
                    .to_path_buf()
            };
            vec![declaration_base.join(path)]
        } else {
            vec![
                parent.descendant_base.join(format!("{name}.rs")),
                parent.descendant_base.join(&name).join("mod.rs"),
            ]
        };
        let existing = candidates
            .iter()
            .filter(|candidate| candidate.is_file())
            .collect::<Vec<_>>();
        if existing.len() != 1 {
            let rendered = candidates
                .iter()
                .map(|candidate| candidate.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let problem = if existing.is_empty() {
                "missing"
            } else {
                "ambiguous"
            };
            return Err(GuardError::new(
                "out-of-line module resolution",
                format!(
                    "{problem} source for module {}::{}; candidates: {rendered}",
                    parent.module_id, name
                ),
            ));
        }
        self.canonical_source(existing[0], "out-of-line module canonicalization")
    }

    fn index_body(&mut self, body: ModuleBody) -> Result<(), GuardError> {
        self.index.declared_modules.insert(body.module_id.clone());
        self.index.bodies.push(body.clone());

        let mut variants = BTreeMap::<String, usize>::new();
        for (item_ordinal, item) in body.items.iter().enumerate() {
            let Item::Mod(module) = item else {
                continue;
            };
            if is_exact_cfg_test(module) {
                continue;
            }
            let name = unraw(&module.ident);
            let variant = variants.entry(name.clone()).or_default();
            let variant_ordinal = *variant;
            *variant += 1;
            let child_module_id = format!("{}::{name}", body.module_id);
            let child_body_id = format!(
                "{}::{name}#{variant_ordinal}@item{item_ordinal}",
                body.body_id
            );

            if let Some((_, inline_items)) = &module.content {
                let direct_path = direct_path_attribute(module)?;
                let descendant_base = if let Some(path) = direct_path {
                    let declaration_base = if body.inline {
                        body.descendant_base.clone()
                    } else {
                        body.source
                            .parent()
                            .ok_or_else(|| {
                                GuardError::new(
                                    "inline module base",
                                    format!("source has no parent: {}", body.source.display()),
                                )
                            })?
                            .to_path_buf()
                    };
                    declaration_base.join(path)
                } else {
                    body.descendant_base.join(&name)
                };
                self.index_body(ModuleBody {
                    module_id: child_module_id,
                    source: body.source.clone(),
                    relative_source: body.relative_source.clone(),
                    body_id: child_body_id,
                    descendant_base,
                    inline: true,
                    items: inline_items.clone(),
                })?;
            } else {
                let source = self.conventional_source(&body, module)?;
                self.claim_out_of_line_source(&child_module_id, &source)?;
                let items = self.parse_source(&source)?;
                let relative_source = render_relative(&self.manifest_root, &source)?;
                let descendant_base = descendant_base_for_source(&source)?;
                self.active_sources.push(source.clone());
                let result = self.index_body(ModuleBody {
                    module_id: child_module_id,
                    source: source.clone(),
                    relative_source,
                    body_id: child_body_id,
                    descendant_base,
                    inline: false,
                    items,
                });
                self.active_sources.pop();
                result?;
            }
        }
        Ok(())
    }
}

fn build_module_index(
    manifest_root: &Path,
    crate_root_file: &Path,
    crate_id: &str,
) -> Result<ModuleIndex, GuardError> {
    if crate_id.is_empty() {
        return Err(GuardError::new(
            "module-tree configuration",
            "crate ID must be explicit and nonempty",
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
    let mut builder = ModuleTreeBuilder::new(canonical_manifest);
    let crate_root = builder.canonical_source(crate_root_file, "crate-root selection")?;
    builder.claim_out_of_line_source(crate_id, &crate_root)?;
    let items = builder.parse_source(&crate_root)?;
    let relative_source = render_relative(&builder.manifest_root, &crate_root)?;
    let descendant_base = crate_root
        .parent()
        .ok_or_else(|| {
            GuardError::new(
                "crate-root selection",
                format!("crate root has no parent: {}", crate_root.display()),
            )
        })?
        .to_path_buf();
    builder.active_sources.push(crate_root.clone());
    let result = builder.index_body(ModuleBody {
        module_id: crate_id.to_owned(),
        source: crate_root,
        relative_source,
        body_id: format!("{crate_id}@root"),
        descendant_base,
        inline: false,
        items,
    });
    builder.active_sources.pop();
    result?;
    Ok(builder.index)
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
    aliases: BTreeMap<String, Vec<String>>,
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
    fn visit_item_mod(&mut self, _item: &'ast ItemMod) {
        // The module-tree index scans each body with its own canonical identity.
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let mut leaves = Vec::new();
        expand_use_tree(&item.tree, &mut Vec::new(), &mut leaves);
        for leaf in leaves {
            if let Some(module) = resolve_internal_module(
                self.current_module,
                &leaf,
                self.crate_id,
                self.declared_modules,
            ) {
                if let Some(binding) = leaf.last() {
                    self.aliases.insert(
                        binding.clone(),
                        module.split("::").map(str::to_owned).collect(),
                    );
                }
            }
            self.observe(leaf);
        }
    }

    fn visit_path(&mut self, path: &'ast SynPath) {
        let segments = path
            .segments
            .iter()
            .map(|segment| unraw(&segment.ident))
            .collect::<Vec<_>>();
        if let Some(alias) = segments.first().and_then(|first| self.aliases.get(first)) {
            let mut expanded = alias.clone();
            expanded.extend(segments.iter().skip(1).cloned());
            self.observe(expanded);
        } else {
            self.observe(segments);
        }
        visit::visit_path(self, path);
    }

    fn visit_visibility(&mut self, _visibility: &'ast Visibility) {
        // Interface visibility is checked structurally in the move-proof stage.
    }
}

fn analyze_module_index(
    index: &ModuleIndex,
    crate_id: &str,
    target_module: &str,
) -> Result<GuardReport, GuardError> {
    let target_bodies = index
        .bodies
        .iter()
        .filter(|body| {
            body.module_id == target_module
                || body
                    .module_id
                    .strip_prefix(target_module)
                    .is_some_and(|suffix| suffix.starts_with("::"))
        })
        .collect::<Vec<_>>();
    if target_bodies.is_empty() {
        return Err(GuardError::new(
            "target source discovery",
            format!("zero source files found for target module {target_module}"),
        ));
    }

    let mut report = GuardReport {
        sources: BTreeSet::new(),
        dependencies: BTreeSet::new(),
    };
    for body in target_bodies {
        let mut visitor = DependencyVisitor {
            source: &body.relative_source,
            current_module: &body.module_id,
            crate_id,
            declared_modules: &index.declared_modules,
            aliases: BTreeMap::new(),
            observations: BTreeSet::new(),
        };
        for item in &body.items {
            visitor.visit_item(item);
        }
        report.sources.insert(body.relative_source.clone());
        report.dependencies.extend(visitor.observations);
    }
    Ok(report)
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
            aliases: BTreeMap::new(),
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
            "target {target_module}; missing expected sources: {missing:?}; unexpected observed sources: {unexpected:?}"
        ),
    ))
}

struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let parent = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("claude-watcher-layering");
        fs::create_dir_all(&parent).expect("fixture parent should be created");
        let counter = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!("{label}-{}-{counter}", std::process::id()));
        fs::create_dir(&root).unwrap_or_else(|error| {
            panic!(
                "fixture directory {} should be exclusive: {error}",
                root.display()
            )
        });
        let manifest = root.join("crate");
        fs::create_dir(&manifest).expect("fixture manifest should be created");
        Self { root, manifest }
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.manifest.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!(
                    "fixture parent {} could not be created: {error}",
                    parent.display()
                )
            });
        }
        fs::write(&path, text).unwrap_or_else(|error| {
            panic!(
                "fixture file {} could not be written: {error}",
                path.display()
            )
        });
    }

    fn write_outside_manifest(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("outside fixture parent should be created");
        }
        fs::write(&path, text).expect("outside fixture file should be written");
    }

    fn index(&self) -> Result<ModuleIndex, GuardError> {
        build_module_index(&self.manifest, &self.manifest.join("src/lib.rs"), "fixture")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn module_tree_resolves_both_conventional_source_forms() {
    for (label, telegram_source) in [
        ("name-rs", "src/telegram.rs"),
        ("name-mod-rs", "src/telegram/mod.rs"),
    ] {
        let fixture = Fixture::new(label);
        fixture.write("src/lib.rs", "mod telegram;\n");
        fixture.write(telegram_source, "pub mod claude_watcher {}\n");
        let index = fixture.index().expect("conventional source should resolve");
        assert!(index.declared_modules.contains("fixture::telegram"));
        assert!(index
            .declared_modules
            .contains("fixture::telegram::claude_watcher"));
    }
}

#[test]
fn module_tree_rejects_ambiguous_and_missing_conventional_sources() {
    let ambiguous = Fixture::new("ambiguous");
    ambiguous.write("src/lib.rs", "mod telegram;\n");
    ambiguous.write("src/telegram.rs", "");
    ambiguous.write("src/telegram/mod.rs", "");
    let ambiguous_error = ambiguous
        .index()
        .expect_err("two candidates must fail")
        .to_string();
    assert!(ambiguous_error.contains("ambiguous source"));
    assert!(ambiguous_error.contains("telegram.rs"));
    assert!(
        ambiguous_error.contains("telegram\\mod.rs") || ambiguous_error.contains("telegram/mod.rs")
    );

    let missing = Fixture::new("missing");
    missing.write("src/lib.rs", "mod telegram;\n");
    let missing_error = missing
        .index()
        .expect_err("zero candidates must fail")
        .to_string();
    assert!(missing_error.contains("missing source"));
    assert!(missing_error.contains("telegram.rs"));
    assert!(missing_error.contains(FOCUSED_RERUN));
}

#[test]
fn nested_inline_modules_share_a_source_without_losing_body_identity() {
    let fixture = Fixture::new("nested-inline");
    fixture.write("src/lib.rs", "mod telegram;\n");
    fixture.write(
        "src/telegram.rs",
        "pub mod bridge {}\npub mod claude_watcher { mod nested { use super::super::bridge::Forbidden; } }\n",
    );
    let index = fixture.index().expect("inline tree should index");
    let target_bodies = index
        .bodies
        .iter()
        .filter(|body| {
            body.module_id
                .starts_with("fixture::telegram::claude_watcher")
        })
        .collect::<Vec<_>>();
    assert_eq!(target_bodies.len(), 2);
    assert_eq!(target_bodies[0].source, target_bodies[1].source);
    assert_ne!(target_bodies[0].body_id, target_bodies[1].body_id);
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("inline target should analyze");
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/telegram.rs".to_owned(),
        module: "fixture::telegram::bridge".to_owned(),
    }));
}

#[test]
fn cfg_disjoint_inline_variants_keep_distinct_scopes_and_scan_the_second_body() {
    let fixture = Fixture::new("cfg-inline-variants");
    fixture.write("src/lib.rs", "mod telegram;\n");
    fixture.write(
        "src/telegram.rs",
        "pub mod bridge {}\n#[cfg(unix)] pub mod claude_watcher { fn first() {} }\n#[cfg(windows)] pub mod claude_watcher { use crate::telegram::bridge::Forbidden; }\n",
    );
    let index = fixture
        .index()
        .expect("cfg-disjoint inline variants should index");
    let bodies = index
        .bodies
        .iter()
        .filter(|body| body.module_id == "fixture::telegram::claude_watcher")
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 2);
    assert_ne!(bodies[0].body_id, bodies[1].body_id);
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("both inline variants should scan");
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/telegram.rs".to_owned(),
        module: "fixture::telegram::bridge".to_owned(),
    }));
}

#[test]
fn cfg_disjoint_out_of_line_variants_scan_every_selected_file() {
    let fixture = Fixture::new("cfg-out-of-line-variants");
    fixture.write("src/lib.rs", "mod telegram;\n");
    fixture.write(
        "src/telegram.rs",
        "pub mod bridge {}\n#[cfg(unix)] #[path = \"claude_a.rs\"] pub mod claude_watcher;\n#[cfg(windows)] #[path = \"claude_b.rs\"] pub mod claude_watcher;\n",
    );
    fixture.write("src/claude_a.rs", "fn first() {}\n");
    fixture.write(
        "src/claude_b.rs",
        "use crate::telegram::bridge::Forbidden;\n",
    );
    let index = fixture
        .index()
        .expect("cfg-disjoint out-of-line variants should index");
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("both out-of-line variants should scan");
    assert_eq!(
        report.sources,
        ["src/claude_a.rs".to_owned(), "src/claude_b.rs".to_owned()]
            .into_iter()
            .collect()
    );
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/claude_b.rs".to_owned(),
        module: "fixture::telegram::bridge".to_owned(),
    }));
}

#[test]
fn module_tree_fails_on_source_set_shrink_and_zero_target() {
    let fixture = Fixture::new("source-set-shrink");
    fixture.write("src/lib.rs", "mod telegram;\n");
    fixture.write("src/telegram.rs", "pub mod claude_watcher {}\n");
    let index = fixture.index().expect("fixture tree should index");
    let report = analyze_module_index(&index, "fixture", "fixture::telegram::claude_watcher")
        .expect("target root should exist");
    let expected = [
        "src/telegram.rs".to_owned(),
        "src/telegram/claude_watcher/output.rs".to_owned(),
    ]
    .into_iter()
    .collect();
    let shrink = require_exact_source_set(&report, &expected, "fixture::telegram::claude_watcher")
        .expect_err("missing expected descendant must fail")
        .to_string();
    assert!(shrink.contains("target source-set equality"));
    assert!(shrink.contains("output.rs"));

    let zero = analyze_module_index(&index, "fixture", "fixture::absent")
        .expect_err("absent target must fail")
        .to_string();
    assert!(zero.contains("zero source files"));
    assert!(zero.contains("fixture::absent"));
}

#[test]
fn module_tree_rejects_escape_duplicate_owner_and_active_source_cycle() {
    let escape = Fixture::new("escape");
    escape.write(
        "src/lib.rs",
        "#[path = \"../../outside.rs\"] mod escaped;\n",
    );
    escape.write_outside_manifest("outside.rs", "");
    let escape_error = escape
        .index()
        .expect_err("manifest escape must fail")
        .to_string();
    assert!(escape_error.contains("outside canonical manifest root"));
    assert!(escape_error.contains("outside.rs"));

    let duplicate = Fixture::new("duplicate-owner");
    duplicate.write(
        "src/lib.rs",
        "#[path = \"shared.rs\"] mod first;\n#[path = \"shared.rs\"] mod second;\n",
    );
    duplicate.write("src/shared.rs", "");
    let duplicate_error = duplicate
        .index()
        .expect_err("two module IDs cannot own one source")
        .to_string();
    assert!(duplicate_error.contains("out-of-line source ownership"));
    assert!(duplicate_error.contains("fixture::first"));
    assert!(duplicate_error.contains("fixture::second"));

    let cycle = Fixture::new("active-cycle");
    cycle.write("src/lib.rs", "mod child;\n");
    cycle.write("src/child.rs", "#[path = \"lib.rs\"] mod back;\n");
    let cycle_error = cycle
        .index()
        .expect_err("active source cycle must fail")
        .to_string();
    assert!(cycle_error.contains("active module source cycle"));
    assert!(cycle_error.contains("src/lib.rs"));
}

#[test]
fn production_guard_observes_the_exact_initial_dependency_set() {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crate_root = manifest_root.join("src/lib.rs");
    let first_index = build_module_index(&manifest_root, &crate_root, CRATE_ID)
        .expect("production module tree should build");
    let second_index = build_module_index(&manifest_root, &crate_root, CRATE_ID)
        .expect("second production module tree should build");
    let first = analyze_module_index(&first_index, CRATE_ID, TARGET_MODULE)
        .expect("production guard should parse and resolve");
    let second = analyze_module_index(&second_index, CRATE_ID, TARGET_MODULE)
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
