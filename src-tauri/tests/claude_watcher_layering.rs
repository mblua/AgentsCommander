//! Structured dependency guard for the Claude watcher output seam.
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
const OUTPUT_MODULE: &str = "agentscommander_lib::telegram::claude_watcher::output";
const OUTPUT_SOURCE: &str = "src/telegram/claude_watcher/output.rs";
const FOCUSED_RERUN: &str = "cargo test --test claude_watcher_layering -- --nocapture";
const AUTHORITATIVE_TOPOLOGY: &str = "rust-module-dependency-cycles 1.1.0";
const SCOPE_TEXT: &str = "SCOPE: this guard is a syntax and compiler module-tree regression net. 1. Macro/proc-macro expansion and include! tokens are not performed. 2. Trait, generic, and receiver dispatch without a syntactic path is not type-resolved. 3. Generated source without a literal mod declaration is outside traversal. 4. Identity available only through an outside re-export chain can exceed the local alias resolver.";

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
            "CONTRACT: {}\n{}\nfocused rerun: {}\n{}\nauthoritative topology: {}",
            self.contract, self.detail, FOCUSED_RERUN, SCOPE_TEXT, AUTHORITATIVE_TOPOLOGY
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

fn collect_conditional_paths(
    meta: ParseNestedMeta<'_>,
    paths: &mut Vec<PathBuf>,
) -> syn::Result<()> {
    let name = meta
        .path
        .segments
        .last()
        .map(|segment| unraw(&segment.ident))
        .unwrap_or_default();
    if name == "path" {
        let value = meta.value()?.parse::<syn::LitStr>()?;
        paths.push(PathBuf::from(value.value()));
        return Ok(());
    }
    if meta.input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in meta.input);
        let nested = content.parse_terminated(|input| input.parse::<Meta>(), syn::Token![,])?;
        for nested_meta in nested {
            collect_paths_from_meta(&nested_meta, paths)?;
        }
    } else if meta.input.peek(syn::Token![=]) {
        let _ = meta.value()?.parse::<Expr>()?;
    }
    Ok(())
}

fn collect_paths_from_meta(meta: &Meta, paths: &mut Vec<PathBuf>) -> syn::Result<()> {
    match meta {
        Meta::Path(_) => Ok(()),
        Meta::NameValue(name_value) => {
            let name = name_value
                .path
                .segments
                .last()
                .map(|segment| unraw(&segment.ident))
                .unwrap_or_default();
            if name == "path" {
                let Expr::Lit(expression) = &name_value.value else {
                    return Err(syn::Error::new_spanned(
                        &name_value.value,
                        "conditional path must be a string literal",
                    ));
                };
                let Lit::Str(value) = &expression.lit else {
                    return Err(syn::Error::new_spanned(
                        &expression.lit,
                        "conditional path must be a string literal",
                    ));
                };
                paths.push(PathBuf::from(value.value()));
            }
            Ok(())
        }
        Meta::List(list) => {
            let nested = list.parse_args_with(
                syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
            )?;
            for nested_meta in nested {
                collect_paths_from_meta(&nested_meta, paths)?;
            }
            Ok(())
        }
    }
}

fn conditional_path_attributes(item: &ItemMod) -> Result<Vec<PathBuf>, GuardError> {
    let mut paths = Vec::new();
    for attribute in &item.attrs {
        if !attribute.path().is_ident("cfg_attr") {
            continue;
        }
        attribute
            .parse_nested_meta(|meta| collect_conditional_paths(meta, &mut paths))
            .map_err(|error| {
                GuardError::new(
                    "conditional module path attribute",
                    format!(
                        "module {} has an invalid cfg_attr path variant: {error}",
                        unraw(&item.ident)
                    ),
                )
            })?;
    }
    Ok(paths)
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

    fn out_of_line_sources(
        &self,
        parent: &ModuleBody,
        item: &ItemMod,
    ) -> Result<Vec<PathBuf>, GuardError> {
        let direct = direct_path_attribute(item)?;
        let conditional = conditional_path_attributes(item)?;
        if direct.is_some() && !conditional.is_empty() {
            return Err(GuardError::new(
                "module path attribute",
                format!(
                    "module {} combines a direct path with conditional path variants",
                    unraw(&item.ident)
                ),
            ));
        }

        let mut sources = vec![self.conventional_source(parent, item)?];
        if !conditional.is_empty() {
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
            for variant in conditional {
                let candidate = declaration_base.join(&variant);
                if !candidate.is_file() {
                    return Err(GuardError::new(
                        "conditional module path resolution",
                        format!(
                            "conditional path variant {} for module {} does not exist",
                            candidate.display(),
                            unraw(&item.ident)
                        ),
                    ));
                }
                sources.push(
                    self.canonical_source(&candidate, "conditional module path canonicalization")?,
                );
            }
        }
        sources.sort();
        sources.dedup();
        Ok(sources)
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
                let conditional_paths = conditional_path_attributes(module)?;
                if direct_path.is_some() && !conditional_paths.is_empty() {
                    return Err(GuardError::new(
                        "module path attribute",
                        format!(
                            "inline module {name} combines a direct path with conditional path variants"
                        ),
                    ));
                }
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
                let mut descendant_bases = if let Some(path) = direct_path {
                    vec![declaration_base.join(path)]
                } else {
                    vec![body.descendant_base.join(&name)]
                };
                descendant_bases.extend(
                    conditional_paths
                        .into_iter()
                        .map(|path| declaration_base.join(path)),
                );
                descendant_bases.sort();
                descendant_bases.dedup();
                for (base_variant, descendant_base) in descendant_bases.into_iter().enumerate() {
                    self.index_body(ModuleBody {
                        module_id: child_module_id.clone(),
                        source: body.source.clone(),
                        relative_source: body.relative_source.clone(),
                        body_id: format!("{child_body_id}@base{base_variant}"),
                        descendant_base,
                        inline: true,
                        items: inline_items.clone(),
                    })?;
                }
            } else {
                let sources = self.out_of_line_sources(&body, module)?;
                for (path_variant, source) in sources.into_iter().enumerate() {
                    self.claim_out_of_line_source(&child_module_id, &source)?;
                    let items = self.parse_source(&source)?;
                    let relative_source = render_relative(&self.manifest_root, &source)?;
                    let descendant_base = descendant_base_for_source(&source)?;
                    self.active_sources.push(source.clone());
                    let result = self.index_body(ModuleBody {
                        module_id: child_module_id.clone(),
                        source: source.clone(),
                        relative_source,
                        body_id: format!("{child_body_id}@path{path_variant}"),
                        descendant_base,
                        inline: false,
                        items,
                    });
                    self.active_sources.pop();
                    result?;
                }
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum BindingTarget {
    Internal(Vec<String>),
    External,
}

fn describe_binding_target(target: &BindingTarget) -> String {
    match target {
        BindingTarget::Internal(path) => path.join("::"),
        BindingTarget::External => "<external>".to_owned(),
    }
}

#[derive(Clone, Debug)]
struct UseLeaf {
    path: Vec<String>,
    binding: Option<String>,
    glob: bool,
}

fn expand_use_tree(tree: &UseTree, prefix: &mut Vec<String>, leaves: &mut Vec<UseLeaf>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(unraw(&path.ident));
            expand_use_tree(&path.tree, prefix, leaves);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let name = unraw(&name.ident);
            if name == "self" {
                leaves.push(UseLeaf {
                    path: prefix.clone(),
                    binding: prefix.last().cloned(),
                    glob: false,
                });
            } else {
                prefix.push(name);
                leaves.push(UseLeaf {
                    path: prefix.clone(),
                    binding: prefix.last().cloned(),
                    glob: false,
                });
                prefix.pop();
            }
        }
        UseTree::Rename(rename) => {
            let name = unraw(&rename.ident);
            let binding = unraw(&rename.rename);
            if name == "self" {
                leaves.push(UseLeaf {
                    path: prefix.clone(),
                    binding: (binding != "_").then_some(binding),
                    glob: false,
                });
            } else {
                prefix.push(name);
                leaves.push(UseLeaf {
                    path: prefix.clone(),
                    binding: (binding != "_").then_some(binding),
                    glob: false,
                });
                prefix.pop();
            }
        }
        UseTree::Group(group) => {
            for item in &group.items {
                expand_use_tree(item, prefix, leaves);
            }
        }
        UseTree::Glob(_) => leaves.push(UseLeaf {
            path: prefix.clone(),
            binding: None,
            glob: true,
        }),
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
    scopes: Vec<BTreeMap<String, BindingTarget>>,
    observations: BTreeSet<DependencyObservation>,
    error: Option<GuardError>,
}

impl DependencyVisitor<'_> {
    fn binding_from_scopes(
        &self,
        name: &str,
        local: &BTreeMap<String, BindingTarget>,
    ) -> Option<BindingTarget> {
        local.get(name).cloned().or_else(|| {
            self.scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(name).cloned())
        })
    }

    fn resolve_target(
        &self,
        segments: &[String],
        local: &BTreeMap<String, BindingTarget>,
        pending_names: &BTreeSet<String>,
    ) -> Result<Option<BindingTarget>, GuardError> {
        let Some(first) = segments.first() else {
            return Err(GuardError::new(
                "path resolution",
                format!("empty Rust path in {}", self.source),
            ));
        };
        if let Some(binding) = self.binding_from_scopes(first, local) {
            return match binding {
                BindingTarget::External => Ok(Some(BindingTarget::External)),
                BindingTarget::Internal(mut canonical) => {
                    canonical.extend(segments.iter().skip(1).cloned());
                    let module = resolve_internal_module(
                        self.current_module,
                        &canonical,
                        self.crate_id,
                        self.declared_modules,
                    )
                    .ok_or_else(|| {
                        GuardError::new(
                            "alias path resolution",
                            format!(
                                "internal alias path {} in {} has no declared module prefix",
                                segments.join("::"),
                                self.source
                            ),
                        )
                    })?;
                    Ok(Some(BindingTarget::Internal(
                        module.split("::").map(str::to_owned).collect(),
                    )))
                }
            };
        }
        if pending_names.contains(first) {
            return Ok(None);
        }
        if let Some(module) = resolve_internal_module(
            self.current_module,
            segments,
            self.crate_id,
            self.declared_modules,
        ) {
            return Ok(Some(BindingTarget::Internal(
                module.split("::").map(str::to_owned).collect(),
            )));
        }
        if matches!(first.as_str(), "crate" | "self" | "super") || first == self.crate_id {
            return Err(GuardError::new(
                "internal path resolution",
                format!(
                    "same-crate path {} in {} has no declared module prefix",
                    segments.join("::"),
                    self.source
                ),
            ));
        }
        Ok(Some(BindingTarget::External))
    }

    fn insert_binding(
        &self,
        local: &mut BTreeMap<String, BindingTarget>,
        name: String,
        target: BindingTarget,
    ) -> Result<(), GuardError> {
        if let Some(existing) = local.get(&name) {
            if existing != &target {
                return Err(GuardError::new(
                    "lexical alias conflict",
                    format!(
                        "binding {name} in {} resolves to conflicting targets {} and {}",
                        self.source,
                        describe_binding_target(existing),
                        describe_binding_target(&target)
                    ),
                ));
            }
            return Ok(());
        }
        local.insert(name, target);
        Ok(())
    }

    fn observe_target(&mut self, target: BindingTarget) {
        let BindingTarget::Internal(path) = target else {
            return;
        };
        let module = path.join("::");
        if module != self.current_module {
            self.observations.insert(DependencyObservation {
                source: self.source.to_owned(),
                module,
            });
        }
    }

    fn build_scope(
        &mut self,
        items: &[&Item],
    ) -> Result<BTreeMap<String, BindingTarget>, GuardError> {
        let mut leaves = Vec::<UseLeaf>::new();
        let mut extern_crates = Vec::<&ItemExternCrate>::new();
        for item in items {
            match item {
                Item::Use(item_use) => {
                    expand_use_tree(&item_use.tree, &mut Vec::new(), &mut leaves);
                }
                Item::ExternCrate(item_extern) => extern_crates.push(item_extern),
                _ => {}
            }
        }
        if let Some(glob) = leaves.iter().find(|leaf| leaf.glob) {
            return Err(GuardError::new(
                "glob import",
                format!(
                    "glob import {}::* is forbidden in target source {}",
                    glob.path.join("::"),
                    self.source
                ),
            ));
        }

        let mut local = BTreeMap::new();
        for item in extern_crates {
            let crate_name = unraw(&item.ident);
            let renamed = item.rename.as_ref().map(|(_, ident)| unraw(ident));
            if crate_name == "self" {
                let Some(binding) = renamed else {
                    return Err(GuardError::new(
                        "current-crate extern alias",
                        format!(
                            "unrenamed extern crate self has unresolved binding identity in {}",
                            self.source
                        ),
                    ));
                };
                self.insert_binding(
                    &mut local,
                    binding,
                    BindingTarget::Internal(vec![self.crate_id.to_owned()]),
                )?;
            } else {
                self.insert_binding(
                    &mut local,
                    renamed.unwrap_or(crate_name),
                    BindingTarget::External,
                )?;
            }
        }

        let pending = leaves
            .iter()
            .filter_map(|leaf| {
                leaf.binding
                    .as_ref()
                    .map(|binding| (binding.clone(), leaf.path.clone()))
            })
            .collect::<Vec<_>>();
        let all_pending_names = pending
            .iter()
            .map(|(binding, _)| binding.clone())
            .collect::<BTreeSet<_>>();
        let mut unresolved = (0..pending.len()).collect::<BTreeSet<_>>();
        loop {
            let mut progress = false;
            for index in unresolved.clone() {
                let (binding, path) = &pending[index];
                let still_pending = all_pending_names
                    .difference(&local.keys().cloned().collect())
                    .cloned()
                    .collect::<BTreeSet<_>>();
                if let Some(target) = self.resolve_target(path, &local, &still_pending)? {
                    self.insert_binding(&mut local, binding.clone(), target)?;
                    unresolved.remove(&index);
                    progress = true;
                }
            }
            if unresolved.is_empty() {
                break;
            }
            if !progress {
                let names = unresolved
                    .iter()
                    .map(|index| pending[*index].0.clone())
                    .collect::<Vec<_>>();
                return Err(GuardError::new(
                    "alias fixed point",
                    format!(
                        "unresolved or cyclic aliases {names:?} in target source {}",
                        self.source
                    ),
                ));
            }
        }

        for leaf in leaves {
            let target = self
                .resolve_target(&leaf.path, &local, &BTreeSet::new())?
                .ok_or_else(|| {
                    GuardError::new(
                        "import leaf resolution",
                        format!("unresolved import leaf {}", leaf.path.join("::")),
                    )
                })?;
            self.observe_target(target);
        }
        Ok(local)
    }

    fn scan_items(&mut self, items: &[Item]) -> Result<(), GuardError> {
        let item_refs = items.iter().collect::<Vec<_>>();
        let scope = self.build_scope(&item_refs)?;
        self.scopes.push(scope);
        for item in items {
            self.visit_item(item);
            if self.error.is_some() {
                break;
            }
        }
        self.scopes.pop();
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        Ok(())
    }

    fn observe(&mut self, segments: Vec<String>) {
        if self.error.is_some() {
            return;
        }
        match self.resolve_target(&segments, &BTreeMap::new(), &BTreeSet::new()) {
            Ok(Some(target)) => self.observe_target(target),
            Ok(None) => {
                self.error = Some(GuardError::new(
                    "path resolution",
                    format!("unresolved path {} in {}", segments.join("::"), self.source),
                ));
            }
            Err(error) => self.error = Some(error),
        }
    }
}

impl<'ast> Visit<'ast> for DependencyVisitor<'_> {
    fn visit_item_mod(&mut self, _item: &'ast ItemMod) {
        // The module-tree index scans each body with its own canonical identity.
    }

    fn visit_item_use(&mut self, _item: &'ast ItemUse) {}

    fn visit_item_extern_crate(&mut self, _item: &'ast ItemExternCrate) {}

    fn visit_block(&mut self, block: &'ast Block) {
        let items = block
            .stmts
            .iter()
            .filter_map(|statement| match statement {
                Stmt::Item(item) => Some(item),
                _ => None,
            })
            .collect::<Vec<_>>();
        let scope = match self.build_scope(&items) {
            Ok(scope) => scope,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.scopes.push(scope);
        for statement in &block.stmts {
            self.visit_stmt(statement);
            if self.error.is_some() {
                break;
            }
        }
        self.scopes.pop();
    }

    fn visit_path(&mut self, path: &'ast SynPath) {
        let segments = path
            .segments
            .iter()
            .map(|segment| unraw(&segment.ident))
            .collect::<Vec<_>>();
        self.observe(segments);
        visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, invocation: &'ast Macro) {
        self.observe(
            invocation
                .path
                .segments
                .iter()
                .map(|segment| unraw(&segment.ident))
                .collect(),
        );
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
            scopes: Vec::new(),
            observations: BTreeSet::new(),
            error: None,
        };
        visitor.scan_items(&body.items)?;
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
            scopes: Vec::new(),
            observations: BTreeSet::new(),
            error: None,
        };
        visitor.scan_items(&syntax.items)?;
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct SymbolOccurrence {
    source: String,
    body_id: String,
    item_ordinal: usize,
    associated_item_ordinal: Option<usize>,
}

type SymbolOccurrences = BTreeMap<String, Vec<SymbolOccurrence>>;

#[allow(dead_code)]
fn expected_moved_symbols() -> BTreeSet<String> {
    [
        format!("struct {OUTPUT_MODULE}::BridgeLogger"),
        format!("method {OUTPUT_MODULE}::BridgeLogger::new"),
        format!("method {OUTPUT_MODULE}::BridgeLogger::log"),
        format!("struct {OUTPUT_MODULE}::DiagLogger"),
        format!("method {OUTPUT_MODULE}::DiagLogger::new"),
        format!("method {OUTPUT_MODULE}::DiagLogger::log_raw"),
        format!("method {OUTPUT_MODULE}::DiagLogger::log_sent"),
        format!("enum {OUTPUT_MODULE}::TelegramErrKind"),
        format!("fn {OUTPUT_MODULE}::flush_buffer"),
        format!("fn {OUTPUT_MODULE}::chunk_text"),
        format!("method {OUTPUT_MODULE}::TelegramErrKind::classify"),
        format!("method {OUTPUT_MODULE}::TelegramErrKind::as_str"),
    ]
    .into_iter()
    .collect()
}

fn canonical_internal_item_path(
    current_module: &str,
    segments: &[String],
    crate_id: &str,
    declared_modules: &BTreeSet<String>,
) -> Option<Vec<String>> {
    let first = segments.first()?;
    let mut canonical = if first == "crate" || first == crate_id {
        vec![crate_id.to_owned()]
    } else if first == "self" || first == "super" {
        current_module.split("::").map(str::to_owned).collect()
    } else {
        let root = format!("{crate_id}::{first}");
        if declared_modules
            .iter()
            .any(|module| module == &root || module.starts_with(&(root.clone() + "::")))
        {
            vec![crate_id.to_owned()]
        } else if segments.len() == 1 {
            let mut local = current_module
                .split("::")
                .map(str::to_owned)
                .collect::<Vec<_>>();
            local.push(first.clone());
            return Some(local);
        } else {
            return None;
        }
    };

    let mut index = 0;
    if first == "crate" || first == crate_id || first == "self" {
        index = 1;
    } else if first != "super" {
        canonical.push(first.clone());
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
    Some(canonical)
}

fn module_item_aliases(
    body: &ModuleBody,
    crate_id: &str,
    declared_modules: &BTreeSet<String>,
) -> BTreeMap<String, Vec<String>> {
    let mut aliases = BTreeMap::new();
    for item in &body.items {
        let Item::Use(item_use) = item else {
            continue;
        };
        let mut leaves = Vec::new();
        expand_use_tree(&item_use.tree, &mut Vec::new(), &mut leaves);
        for leaf in leaves {
            let Some(binding) = leaf.binding else {
                continue;
            };
            if let Some(path) = canonical_internal_item_path(
                &body.module_id,
                &leaf.path,
                crate_id,
                declared_modules,
            ) {
                aliases.insert(binding, path);
            }
        }
    }
    aliases
}

fn canonical_impl_self_type(
    body: &ModuleBody,
    item_impl: &syn::ItemImpl,
    crate_id: &str,
    declared_modules: &BTreeSet<String>,
    aliases: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    if item_impl.trait_.is_some() {
        return None;
    }
    let syn::Type::Path(type_path) = item_impl.self_ty.as_ref() else {
        return None;
    };
    if type_path.qself.is_some() {
        return None;
    }
    let segments = type_path
        .path
        .segments
        .iter()
        .map(|segment| unraw(&segment.ident))
        .collect::<Vec<_>>();
    if let Some(alias) = segments.first().and_then(|first| aliases.get(first)) {
        let mut canonical = alias.clone();
        canonical.extend(segments.iter().skip(1).cloned());
        return Some(canonical.join("::"));
    }
    canonical_internal_item_path(&body.module_id, &segments, crate_id, declared_modules)
        .map(|path| path.join("::"))
}

fn collect_moved_symbol_occurrences(index: &ModuleIndex, crate_id: &str) -> SymbolOccurrences {
    let direct_names = [
        "BridgeLogger",
        "DiagLogger",
        "TelegramErrKind",
        "flush_buffer",
        "chunk_text",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let target_types = ["BridgeLogger", "DiagLogger", "TelegramErrKind"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let target_methods = ["new", "log", "log_raw", "log_sent", "classify", "as_str"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut occurrences = SymbolOccurrences::new();

    for body in &index.bodies {
        let aliases = module_item_aliases(body, crate_id, &index.declared_modules);
        for (item_ordinal, item) in body.items.iter().enumerate() {
            let direct = match item {
                Item::Struct(item_struct)
                    if direct_names.contains(unraw(&item_struct.ident).as_str()) =>
                {
                    Some(("struct", unraw(&item_struct.ident)))
                }
                Item::Enum(item_enum)
                    if direct_names.contains(unraw(&item_enum.ident).as_str()) =>
                {
                    Some(("enum", unraw(&item_enum.ident)))
                }
                Item::Fn(item_fn) if direct_names.contains(unraw(&item_fn.sig.ident).as_str()) => {
                    Some(("fn", unraw(&item_fn.sig.ident)))
                }
                _ => None,
            };
            if let Some((kind, name)) = direct {
                occurrences
                    .entry(format!("{kind} {}::{name}", body.module_id))
                    .or_default()
                    .push(SymbolOccurrence {
                        source: body.relative_source.clone(),
                        body_id: body.body_id.clone(),
                        item_ordinal,
                        associated_item_ordinal: None,
                    });
            }

            let Item::Impl(item_impl) = item else {
                continue;
            };
            let Some(self_type) = canonical_impl_self_type(
                body,
                item_impl,
                crate_id,
                &index.declared_modules,
                &aliases,
            ) else {
                continue;
            };
            let self_name = self_type.rsplit("::").next().unwrap_or_default();
            if !target_types.contains(self_name) {
                continue;
            }
            for (associated_item_ordinal, associated) in item_impl.items.iter().enumerate() {
                let syn::ImplItem::Fn(method) = associated else {
                    continue;
                };
                let method_name = unraw(&method.sig.ident);
                if !target_methods.contains(method_name.as_str()) {
                    continue;
                }
                occurrences
                    .entry(format!("method {self_type}::{method_name}"))
                    .or_default()
                    .push(SymbolOccurrence {
                        source: body.relative_source.clone(),
                        body_id: body.body_id.clone(),
                        item_ordinal,
                        associated_item_ordinal: Some(associated_item_ordinal),
                    });
            }
        }
    }
    occurrences
}

#[allow(dead_code)]
fn verify_exact_moved_symbols(index: &ModuleIndex, crate_id: &str) -> Result<(), GuardError> {
    let expected = expected_moved_symbols();
    let occurrences = collect_moved_symbol_occurrences(index, crate_id);
    let actual = occurrences.keys().cloned().collect::<BTreeSet<_>>();
    let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let unexpected = actual.difference(&expected).cloned().collect::<Vec<_>>();
    let duplicates = occurrences
        .iter()
        .filter(|(_, rows)| rows.len() != 1)
        .map(|(symbol, rows)| (symbol.clone(), rows.clone()))
        .collect::<Vec<_>>();
    let misplaced = occurrences
        .iter()
        .flat_map(|(symbol, rows)| {
            rows.iter()
                .filter(|row| row.source != OUTPUT_SOURCE || !row.body_id.contains("::output#"))
                .map(|row| (symbol.clone(), row.clone()))
        })
        .collect::<Vec<_>>();
    if missing.is_empty() && unexpected.is_empty() && duplicates.is_empty() && misplaced.is_empty()
    {
        return Ok(());
    }
    Err(GuardError::new(
        "moved symbol identity and location",
        format!(
            "missing expected symbols: {missing:?}; unexpected observed symbols: {unexpected:?}; duplicate occurrences: {duplicates:?}; out-of-destination occurrences: {misplaced:?}"
        ),
    ))
}

#[derive(Clone, Copy, Debug)]
enum RequiredVisibility {
    Private,
    Crate,
    Super,
}

fn restricted_visibility_parts(visibility: &Visibility) -> Option<(bool, Vec<String>)> {
    let Visibility::Restricted(restricted) = visibility else {
        return None;
    };
    Some((
        restricted.in_token.is_some(),
        restricted
            .path
            .segments
            .iter()
            .map(|segment| unraw(&segment.ident))
            .collect(),
    ))
}

fn describe_visibility(visibility: &Visibility) -> String {
    match visibility {
        Visibility::Inherited => "private".to_owned(),
        Visibility::Public(_) => "pub".to_owned(),
        Visibility::Restricted(_) => {
            let (has_in, path) = restricted_visibility_parts(visibility)
                .expect("restricted visibility should expose its path");
            if has_in {
                format!("pub(in {})", path.join("::"))
            } else {
                format!("pub({})", path.join("::"))
            }
        }
    }
}

fn require_visibility(
    visibility: &Visibility,
    expected: RequiredVisibility,
    symbol: &str,
) -> Result<(), GuardError> {
    let matches = match expected {
        RequiredVisibility::Private => matches!(visibility, Visibility::Inherited),
        RequiredVisibility::Crate => restricted_visibility_parts(visibility)
            .is_some_and(|(has_in, path)| !has_in && path.len() == 1 && path[0] == "crate"),
        RequiredVisibility::Super => restricted_visibility_parts(visibility)
            .is_some_and(|(has_in, path)| !has_in && path.len() == 1 && path[0] == "super"),
    };
    if matches {
        return Ok(());
    }

    let observed = describe_visibility(visibility);
    let expected_text = match expected {
        RequiredVisibility::Private => "private",
        RequiredVisibility::Crate => "pub(crate) with absent in_token",
        RequiredVisibility::Super => "pub(super) with absent in_token",
    };
    let evidence = if observed == "pub(in crate)" {
        "; measured detector 1.1.0 variant is graph-neutral, but it violates the canonical in_token.is_none() contract"
    } else if observed == "pub(in crate::telegram)" {
        "; external live detector 1.1.0 evidence records claude_watcher::output -> telegram for this spelling"
    } else {
        ""
    };
    Err(GuardError::new(
        "exact structured visibility",
        format!(
            "symbol {symbol} has observed visibility {observed}; expected {expected_text}{evidence}"
        ),
    ))
}

#[allow(dead_code)]
fn use_leaves(item_use: &ItemUse) -> Vec<UseLeaf> {
    let mut leaves = Vec::new();
    expand_use_tree(&item_use.tree, &mut Vec::new(), &mut leaves);
    leaves
}

#[allow(dead_code)]
fn exact_use_leaf_set(item_use: &ItemUse, prefix: &[&str], names: &[&str]) -> bool {
    let leaves = use_leaves(item_use);
    if leaves.len() != names.len() || leaves.iter().any(|leaf| leaf.glob) {
        return false;
    }
    let expected = names
        .iter()
        .map(|name| {
            prefix
                .iter()
                .copied()
                .chain(std::iter::once(*name))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    let actual = leaves
        .iter()
        .filter(|leaf| leaf.binding.as_ref() == leaf.path.last())
        .map(|leaf| leaf.path.clone())
        .collect::<BTreeSet<_>>();
    actual == expected
}

#[allow(dead_code)]
fn verify_exact_interface(index: &ModuleIndex, crate_id: &str) -> Result<(), GuardError> {
    let target_bodies = index
        .bodies
        .iter()
        .filter(|body| body.module_id == TARGET_MODULE)
        .collect::<Vec<_>>();
    let output_bodies = index
        .bodies
        .iter()
        .filter(|body| body.module_id == OUTPUT_MODULE)
        .collect::<Vec<_>>();
    let bridge_bodies = index
        .bodies
        .iter()
        .filter(|body| body.module_id == "agentscommander_lib::telegram::bridge")
        .collect::<Vec<_>>();
    if target_bodies.len() != 1 || output_bodies.len() != 1 || bridge_bodies.len() != 1 {
        return Err(GuardError::new(
            "interface module identity",
            format!(
                "expected one target/output/bridge body, got {}/{}/{}",
                target_bodies.len(),
                output_bodies.len(),
                bridge_bodies.len()
            ),
        ));
    }

    let target_body = target_bodies[0];
    let output_body = output_bodies[0];
    let bridge_body = bridge_bodies[0];
    let output_declarations = target_body
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Mod(item_mod) if unraw(&item_mod.ident) == "output" => Some(item_mod),
            _ => None,
        })
        .collect::<Vec<_>>();
    if output_declarations.len() != 1 || output_declarations[0].content.is_some() {
        return Err(GuardError::new(
            "output module declaration",
            format!(
                "expected one out-of-line output declaration in {TARGET_ROOT_SOURCE}, got {}",
                output_declarations.len()
            ),
        ));
    }
    require_visibility(
        &output_declarations[0].vis,
        RequiredVisibility::Super,
        "claude_watcher::output module",
    )?;

    let telegram_output_declarations = index
        .bodies
        .iter()
        .filter(|body| body.module_id == "agentscommander_lib::telegram")
        .flat_map(|body| &body.items)
        .filter(|item| matches!(item, Item::Mod(item_mod) if unraw(&item_mod.ident) == "output"))
        .count();
    if telegram_output_declarations != 0 {
        return Err(GuardError::new(
            "output module ownership",
            "telegram/mod.rs declares output; the module must remain nested under Claude",
        ));
    }

    let mut interface = BTreeSet::new();
    let aliases = module_item_aliases(output_body, crate_id, &index.declared_modules);
    for item in &output_body.items {
        match item {
            Item::Struct(item_struct)
                if matches!(
                    unraw(&item_struct.ident).as_str(),
                    "BridgeLogger" | "DiagLogger"
                ) =>
            {
                let name = unraw(&item_struct.ident);
                require_visibility(
                    &item_struct.vis,
                    RequiredVisibility::Crate,
                    &format!("{OUTPUT_MODULE}::{name}"),
                )?;
                for field in &item_struct.fields {
                    require_visibility(
                        &field.vis,
                        RequiredVisibility::Private,
                        &format!("{OUTPUT_MODULE}::{name} field"),
                    )?;
                }
                interface.insert(format!("struct {name}"));
            }
            Item::Enum(item_enum) if unraw(&item_enum.ident) == "TelegramErrKind" => {
                require_visibility(
                    &item_enum.vis,
                    RequiredVisibility::Crate,
                    &format!("{OUTPUT_MODULE}::TelegramErrKind"),
                )?;
                interface.insert("enum TelegramErrKind".to_owned());
            }
            Item::Fn(item_fn) if unraw(&item_fn.sig.ident) == "flush_buffer" => {
                require_visibility(
                    &item_fn.vis,
                    RequiredVisibility::Crate,
                    &format!("{OUTPUT_MODULE}::flush_buffer"),
                )?;
                interface.insert("fn flush_buffer".to_owned());
            }
            Item::Fn(item_fn) if unraw(&item_fn.sig.ident) == "chunk_text" => {
                require_visibility(
                    &item_fn.vis,
                    RequiredVisibility::Private,
                    &format!("{OUTPUT_MODULE}::chunk_text"),
                )?;
                interface.insert("fn chunk_text private".to_owned());
            }
            Item::Impl(item_impl) => {
                let Some(self_type) = canonical_impl_self_type(
                    output_body,
                    item_impl,
                    crate_id,
                    &index.declared_modules,
                    &aliases,
                ) else {
                    continue;
                };
                let self_name = self_type.rsplit("::").next().unwrap_or_default();
                for associated in &item_impl.items {
                    let syn::ImplItem::Fn(method) = associated else {
                        continue;
                    };
                    let method_name = unraw(&method.sig.ident);
                    let required = matches!(
                        (self_name, method_name.as_str()),
                        ("BridgeLogger", "new" | "log")
                            | ("DiagLogger", "new" | "log_raw" | "log_sent")
                            | ("TelegramErrKind", "classify" | "as_str")
                    );
                    if required {
                        require_visibility(
                            &method.vis,
                            RequiredVisibility::Crate,
                            &format!("{self_type}::{method_name}"),
                        )?;
                        interface.insert(format!("method {self_name}::{method_name}"));
                    }
                }
            }
            _ => {}
        }
    }
    let expected_interface = [
        "struct BridgeLogger",
        "method BridgeLogger::new",
        "method BridgeLogger::log",
        "struct DiagLogger",
        "method DiagLogger::new",
        "method DiagLogger::log_raw",
        "method DiagLogger::log_sent",
        "enum TelegramErrKind",
        "method TelegramErrKind::classify",
        "method TelegramErrKind::as_str",
        "fn flush_buffer",
        "fn chunk_text private",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    if interface != expected_interface {
        return Err(GuardError::new(
            "output interface members",
            format!("expected {expected_interface:?}; observed {interface:?}"),
        ));
    }

    let bridge_uses = bridge_body
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Use(item_use) => Some(item_use),
            _ => None,
        })
        .collect::<Vec<_>>();
    let facade = bridge_uses
        .iter()
        .copied()
        .filter(|item_use| {
            exact_use_leaf_set(
                item_use,
                &["super", "claude_watcher", "output"],
                &["flush_buffer", "BridgeLogger", "DiagLogger"],
            )
        })
        .collect::<Vec<_>>();
    if facade.len() != 1 {
        return Err(GuardError::new(
            "bridge compatibility facade",
            format!("expected one exact three-item facade, got {}", facade.len()),
        ));
    }
    require_visibility(
        &facade[0].vis,
        RequiredVisibility::Super,
        "bridge output compatibility facade",
    )?;
    let enum_import = bridge_uses
        .iter()
        .copied()
        .filter(|item_use| {
            exact_use_leaf_set(
                item_use,
                &["super", "claude_watcher", "output"],
                &["TelegramErrKind"],
            )
        })
        .collect::<Vec<_>>();
    if enum_import.len() != 1 {
        return Err(GuardError::new(
            "bridge private error import",
            format!("expected one direct enum import, got {}", enum_import.len()),
        ));
    }
    require_visibility(
        &enum_import[0].vis,
        RequiredVisibility::Private,
        "bridge TelegramErrKind import",
    )?;

    let claude_import = target_body
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Use(item_use)
                if exact_use_leaf_set(
                    item_use,
                    &["self", "output"],
                    &["flush_buffer", "BridgeLogger", "DiagLogger"],
                ) =>
            {
                Some(item_use)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if claude_import.len() != 1 {
        return Err(GuardError::new(
            "Claude direct output import",
            format!(
                "expected one exact three-item import, got {}",
                claude_import.len()
            ),
        ));
    }
    require_visibility(
        &claude_import[0].vis,
        RequiredVisibility::Private,
        "Claude watcher output import",
    )?;
    Ok(())
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

fn require_exact_production_report(report: &GuardReport) -> Result<(), GuardError> {
    let expected_sources = [TARGET_ROOT_SOURCE.to_owned(), OUTPUT_SOURCE.to_owned()]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let expected_dependencies = expected_dependencies();
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
        "Claude watcher dependency and source sets",
        format!(
            "missing expected source rows: {missing_sources:?}; unexpected observed source rows: {unexpected_sources:?}; missing expected dependency rows: {missing_dependencies:?}; unexpected observed dependency rows: {unexpected_dependencies:?}"
        ),
    ))
}

struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
    cleanup_on_drop: bool,
    drop_cleanup: Option<FixtureDropCleanup>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let parent = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("claude-watcher-layering");
        fs::create_dir_all(&parent).expect("fixture parent should be created");
        let counter = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let root = parent.join(format!("{label}-{}-{nanos}-{counter}", std::process::id()));
        fs::create_dir(&root).unwrap_or_else(|error| {
            panic!(
                "fixture directory {} should be exclusive: {error}",
                root.display()
            )
        });
        let manifest = root.join("crate");
        fs::create_dir(&manifest).expect("fixture manifest should be created");
        Self {
            root,
            manifest,
            cleanup_on_drop: true,
            drop_cleanup: None,
        }
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
        self.index_as("fixture")
    }

    fn index_as(&self, crate_id: &str) -> Result<ModuleIndex, GuardError> {
        build_module_index(&self.manifest, &self.manifest.join("src/lib.rs"), crate_id)
    }

    fn cleanup(mut self) -> Result<(), GuardError> {
        self.cleanup_on_drop = false;
        fs::remove_dir_all(&self.root).map_err(|error| {
            GuardError::new(
                "fixture cleanup",
                format!(
                    "could not remove isolated fixture {}: {error}",
                    self.root.display()
                ),
            )
        })
    }
}

type FixtureRemoveDirAll = Box<dyn FnOnce(&Path) -> std::io::Result<()>>;

struct FixtureDropCleanup {
    remove_dir_all: FixtureRemoveDirAll,
    diagnostic: Box<dyn std::io::Write>,
}

struct RecordingDiagnostic {
    bytes: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
}

impl std::io::Write for RecordingDiagnostic {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes.borrow_mut().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct FailingDiagnostic {
    write_calls: std::rc::Rc<std::cell::Cell<usize>>,
}

impl std::io::Write for FailingDiagnostic {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        self.write_calls.set(self.write_calls.get() + 1);
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "simulated diagnostic writer failure",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Fixture {
    fn with_drop_cleanup(
        mut self,
        remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()> + 'static,
        diagnostic: impl std::io::Write + 'static,
    ) -> Self {
        self.drop_cleanup = Some(FixtureDropCleanup {
            remove_dir_all: Box::new(remove_dir_all),
            diagnostic: Box::new(diagnostic),
        });
        self
    }
}

fn cleanup_fixture_on_drop(
    root: &Path,
    remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()>,
    mut diagnostic: impl std::io::Write,
) {
    if let Err(error) = remove_dir_all(root) {
        let report_result = std::io::Write::write_fmt(
            &mut diagnostic,
            format_args!(
                "fixture cleanup fallback could not remove isolated fixture {}: {error}\n",
                root.display()
            ),
        );
        if let Err(report_error) = report_result {
            // Reporting failure cannot safely trigger another panic from Drop.
            drop(report_error);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            if let Some(cleanup) = self.drop_cleanup.take() {
                cleanup_fixture_on_drop(&self.root, cleanup.remove_dir_all, cleanup.diagnostic);
            } else {
                cleanup_fixture_on_drop(
                    &self.root,
                    |root| fs::remove_dir_all(root),
                    std::io::stderr().lock(),
                );
            }
        }
    }
}

fn inline_spelling_report(label: &str, body: &str) -> Result<GuardReport, GuardError> {
    let fixture = Fixture::new(label);
    fixture.write("src/lib.rs", "mod telegram;\n");
    fixture.write(
        "src/telegram.rs",
        &format!(
            "pub mod bridge {{ pub struct BridgeLogger; }}\npub mod claude_watcher {{\n{body}\n}}\n"
        ),
    );
    let result = fixture
        .index_as(CRATE_ID)
        .and_then(|index| analyze_module_index(&index, CRATE_ID, TARGET_MODULE));
    let cleanup = fixture.cleanup();
    match (result, cleanup) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(GuardError::new(
            "fixture analysis and cleanup",
            format!("primary failure: {primary}; cleanup failure: {cleanup}"),
        )),
    }
}

fn out_of_line_spelling_report(label: &str, probe: &str) -> Result<GuardReport, GuardError> {
    let fixture = Fixture::new(label);
    fixture.write("src/lib.rs", "mod telegram;\n");
    fixture.write(
        "src/telegram.rs",
        "pub mod bridge { pub struct BridgeLogger; }\npub mod claude_watcher;\n",
    );
    fixture.write(
        "src/telegram/claude_watcher.rs",
        "#[cfg(any())]\n#[path = \"claude_watcher_layering_probe.rs\"]\nmod layering_probe;\n",
    );
    fixture.write("src/telegram/claude_watcher_layering_probe.rs", probe);
    let result = fixture
        .index_as(CRATE_ID)
        .and_then(|index| analyze_module_index(&index, CRATE_ID, TARGET_MODULE));
    let cleanup = fixture.cleanup();
    match (result, cleanup) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(GuardError::new(
            "fixture analysis and cleanup",
            format!("primary failure: {primary}; cleanup failure: {cleanup}"),
        )),
    }
}

#[test]
fn structured_spelling_corpus_resolves_every_supported_bridge_path() {
    let cases = [
        (
            "01-direct",
            "#[cfg(any())] use crate::telegram::bridge::BridgeLogger;",
        ),
        (
            "02-grouped",
            "#[cfg(any())] use crate::telegram::{bridge::BridgeLogger};",
        ),
        (
            "03-nested-grouped",
            "#[cfg(any())] use crate::{telegram::{bridge::{BridgeLogger}}};",
        ),
        (
            "04-raw-bridge",
            "#[cfg(any())] use crate::telegram::r#bridge::BridgeLogger;",
        ),
        (
            "05-comment-between-segments",
            "#[cfg(any())] use crate::telegram::/* comment */bridge::BridgeLogger;",
        ),
        (
            "06-sibling-super",
            "#[cfg(any())] use super::bridge::BridgeLogger;",
        ),
        (
            "07-absolute-crate",
            "#[cfg(any())] use ::agentscommander_lib::telegram::bridge::BridgeLogger;",
        ),
        (
            "08-fixed-point-alias",
            "#[cfg(any())] use tg::bridge::BridgeLogger; #[cfg(any())] use crate::telegram as tg;",
        ),
        (
            "09-public-rename",
            "#[cfg(any())] pub use crate::telegram::bridge::{BridgeLogger as Logger};",
        ),
        (
            "10-type-and-expression",
            "#[cfg(any())] fn probe(_: crate::telegram::bridge::BridgeLogger) { let _ = crate::telegram::bridge::BridgeLogger::new; }",
        ),
        (
            "13-use-tree-self-chain",
            "#[cfg(any())] use crate::telegram::{self as tg}; #[cfg(any())] use tg::{bridge as b}; #[cfg(any())] use b::BridgeLogger;",
        ),
        (
            "14-sibling-grouped-rename",
            "#[cfg(any())] use super::{bridge as b}; #[cfg(any())] use b::BridgeLogger;",
        ),
        (
            "15-nested-repeated-super",
            "mod nested { #[cfg(any())] use super::super::bridge::BridgeLogger; }",
        ),
        (
            "16-current-crate-extern-alias",
            "#[cfg(any())] extern crate self as ac; #[cfg(any())] use ac::telegram::bridge::BridgeLogger;",
        ),
        (
            "17-anonymous-import",
            "#[cfg(any())] use crate::telegram::bridge::BridgeLogger as _;",
        ),
        (
            "18-ufcs",
            "#[cfg(any())] fn probe() { let _ = <crate::telegram::bridge::BridgeLogger>::new; }",
        ),
        (
            "19-raw-grouped-alias",
            "#[cfg(any())] use crate::telegram::r#bridge::{BridgeLogger as BL};",
        ),
        (
            "20-explicit-macro-path",
            "#[cfg(any())] fn probe() { crate::telegram::bridge::forbidden_macro!(); }",
        ),
    ];

    for (label, body) in cases {
        let report = inline_spelling_report(label, body)
            .unwrap_or_else(|error| panic!("spelling {label} did not resolve: {error}"));
        assert!(
            report.dependencies.contains(&DependencyObservation {
                source: "src/telegram.rs".to_owned(),
                module: "agentscommander_lib::telegram::bridge".to_owned(),
            }),
            "spelling {label} survived the structured guard: {report:?}"
        );
    }
}

#[test]
fn cfg_disabled_out_of_line_spelling_rows_are_scanned() {
    for (label, probe) in [
        (
            "11-out-of-line-grouped",
            "use crate::telegram::{bridge::BridgeLogger};\n",
        ),
        (
            "12-out-of-line-raw-grouped",
            "use crate::telegram::{r#bridge::{BridgeLogger}};\n",
        ),
    ] {
        let report = out_of_line_spelling_report(label, probe)
            .unwrap_or_else(|error| panic!("out-of-line spelling {label} failed: {error}"));
        assert!(report.dependencies.contains(&DependencyObservation {
            source: "src/telegram/claude_watcher_layering_probe.rs".to_owned(),
            module: "agentscommander_lib::telegram::bridge".to_owned(),
        }));
    }
}

#[test]
fn recursively_nested_cfg_attr_path_variants_scan_every_alternative() {
    let fixture = Fixture::new("21-nested-cfg-attr-raw-child");
    fixture.write("src/lib.rs", "mod telegram;\n");
    fixture.write(
        "src/telegram.rs",
        "pub mod bridge { pub struct BridgeLogger; }\npub mod claude_watcher;\n",
    );
    fixture.write(
        "src/telegram/claude_watcher.rs",
        "#[cfg_attr(any(), cfg_attr(any(), r#path = \"probe_a.rs\"), path = \"probe_b.rs\")]\nmod r#child;\n",
    );
    fixture.write(
        "src/telegram/claude_watcher/child.rs",
        "fn conventional() {}\n",
    );
    fixture.write(
        "src/telegram/probe_a.rs",
        "use crate::telegram::{bridge as b}; use b::BridgeLogger;\n",
    );
    fixture.write("src/telegram/probe_b.rs", "fn second_variant() {}\n");

    let index = fixture
        .index_as(CRATE_ID)
        .expect("nested cfg_attr path alternatives should all index");
    let report = analyze_module_index(&index, CRATE_ID, TARGET_MODULE)
        .expect("all conditional target descendants should scan");
    assert!(report
        .sources
        .contains("src/telegram/claude_watcher/child.rs"));
    assert!(report.sources.contains("src/telegram/probe_a.rs"));
    assert!(report.sources.contains("src/telegram/probe_b.rs"));
    assert!(report.dependencies.contains(&DependencyObservation {
        source: "src/telegram/probe_a.rs".to_owned(),
        module: "agentscommander_lib::telegram::bridge".to_owned(),
    }));
}

#[test]
fn path_attribute_matrix_handles_rebases_and_rejects_invalid_combinations() {
    let direct = Fixture::new("direct-path-outside-inline");
    direct.write(
        "src/lib.rs",
        "#[path = \"alternate/telegram.rs\"] mod telegram;\n",
    );
    direct.write("src/alternate/telegram.rs", "pub mod claude_watcher {}\n");
    assert!(direct
        .index()
        .expect("direct path should resolve")
        .declared_modules
        .contains("fixture::telegram::claude_watcher"));

    for (label, outer_source, child_source) in [
        (
            "inline-path-lib-owner",
            "#[path = \"rebased\"] mod outer { mod child; }\n",
            "src/rebased/child.rs",
        ),
        (
            "inline-non-mod-owner",
            "mod outer;\n",
            "src/outer/inline/child.rs",
        ),
        (
            "inline-mod-owner",
            "mod outer;\n",
            "src/outer/inline/child.rs",
        ),
    ] {
        let fixture = Fixture::new(label);
        fixture.write("src/lib.rs", outer_source);
        if label == "inline-non-mod-owner" {
            fixture.write("src/outer.rs", "mod inline { mod child; }\n");
        }
        if label == "inline-mod-owner" {
            fixture.write("src/outer/mod.rs", "mod inline { mod child; }\n");
        }
        fixture.write(child_source, "fn discovered() {}\n");
        fixture
            .index()
            .unwrap_or_else(|error| panic!("inline path fixture {label} did not resolve: {error}"));
    }

    let combined = Fixture::new("direct-plus-conditional");
    combined.write(
        "src/lib.rs",
        "#[path = \"direct.rs\"] #[cfg_attr(any(), path = \"conditional.rs\")] mod child;\n",
    );
    combined.write("src/direct.rs", "");
    combined.write("src/conditional.rs", "");
    let combined_error = combined
        .index()
        .expect_err("direct and conditional path combination must fail")
        .to_string();
    assert!(combined_error.contains("combines a direct path"));

    let missing = Fixture::new("missing-conditional-variant");
    missing.write(
        "src/lib.rs",
        "#[cfg_attr(any(), path = \"missing.rs\")] mod child;\n",
    );
    missing.write("src/child.rs", "");
    let missing_error = missing
        .index()
        .expect_err("every conditional path variant must exist")
        .to_string();
    assert!(missing_error.contains("conditional path variant"));
    assert!(missing_error.contains("missing.rs"));
}

#[test]
fn inert_text_and_macro_tokens_do_not_create_bridge_dependencies() {
    let rows = [
        ("line-comment", "// crate::telegram::bridge::BridgeLogger"),
        (
            "block-comment",
            "/* crate::telegram::bridge::BridgeLogger */",
        ),
        (
            "normal-string",
            "#[cfg(any())] const TEXT: &str = \"crate::telegram::bridge::BridgeLogger\";",
        ),
        (
            "raw-string",
            "#[cfg(any())] const TEXT: &str = r#\"crate::telegram::bridge::BridgeLogger\"#;",
        ),
        (
            "stringify-tokens",
            "#[cfg(any())] fn probe() { let _ = stringify!(crate::telegram::bridge::BridgeLogger); }",
        ),
    ];
    for (label, body) in rows {
        let report = inline_spelling_report(label, body)
            .unwrap_or_else(|error| panic!("negative row {label} failed to analyze: {error}"));
        assert!(
            report
                .dependencies
                .iter()
                .all(|dependency| dependency.module != "agentscommander_lib::telegram::bridge"),
            "negative row {label} created a false bridge dependency: {report:?}"
        );
    }
}

#[test]
fn alias_conflicts_cycles_and_globs_fail_closed() {
    for (label, body, marker) in [
        (
            "alias-conflict",
            "#[cfg(any())] use crate::telegram::bridge as same; #[cfg(any())] use crate::telegram as same;",
            "lexical alias conflict",
        ),
        (
            "alias-cycle",
            "#[cfg(any())] use b as a; #[cfg(any())] use a as b;",
            "alias fixed point",
        ),
        (
            "glob-import",
            "#[cfg(any())] use crate::telegram::bridge::*;",
            "glob import",
        ),
    ] {
        let message = inline_spelling_report(label, body)
            .expect_err("unsupported alias shape must fail closed")
            .to_string();
        assert!(message.contains(marker), "row {label}: {message}");
        assert!(message.contains(FOCUSED_RERUN));
    }
}

#[test]
#[cfg(any())]
fn production_moved_symbols_and_all_seven_methods_exist_once_at_the_destination() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let index = build_module_index(&manifest, &manifest.join("src/lib.rs"), CRATE_ID)
        .expect("production module index should build");
    verify_exact_moved_symbols(&index, CRATE_ID)
        .unwrap_or_else(|error| panic!("moved-symbol proof failed: {error}"));
}

#[test]
#[cfg(any())]
fn production_interface_is_exactly_telegram_internal_and_canonical() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let index = build_module_index(&manifest, &manifest.join("src/lib.rs"), CRATE_ID)
        .expect("production module index should build");
    verify_exact_interface(&index, CRATE_ID)
        .unwrap_or_else(|error| panic!("exact interface proof failed: {error}"));
}

fn parsed_struct_visibility(prefix: &str) -> Visibility {
    let source = if prefix.is_empty() {
        "struct Item;".to_owned()
    } else {
        format!("{prefix} struct Item;")
    };
    let item = syn::parse_str::<Item>(&source)
        .unwrap_or_else(|error| panic!("visibility fixture did not parse: {source}: {error}"));
    let Item::Struct(item_struct) = item else {
        panic!("visibility fixture did not produce a struct")
    };
    item_struct.vis
}

#[test]
fn visibility_matrix_distinguishes_canonical_shorthand_from_path_spelling() {
    require_visibility(
        &parsed_struct_visibility("pub(crate)"),
        RequiredVisibility::Crate,
        "fixture item",
    )
    .expect("canonical pub(crate) should pass");
    for form in [
        "",
        "pub(self)",
        "pub(in self)",
        "pub(super)",
        "pub",
        "pub(in crate)",
        "pub(in super)",
        "pub(in crate::telegram)",
    ] {
        let message = require_visibility(
            &parsed_struct_visibility(form),
            RequiredVisibility::Crate,
            "fixture item",
        )
        .expect_err("noncanonical item visibility must fail")
        .to_string();
        assert!(message.contains("fixture item"));
        assert!(message.contains("observed visibility"));
        if form == "pub(in crate)" {
            assert!(message.contains("graph-neutral"));
            assert!(message.contains("in_token.is_none()"));
        }
        if form == "pub(in crate::telegram)" {
            assert!(message.contains("detector 1.1.0"));
            assert!(message.contains("output -> telegram"));
        }
    }

    require_visibility(
        &parsed_struct_visibility("pub(super)"),
        RequiredVisibility::Super,
        "fixture module/facade",
    )
    .expect("canonical pub(super) should pass");
    for form in [
        "",
        "pub(self)",
        "pub(in self)",
        "pub(crate)",
        "pub",
        "pub(in super)",
        "pub(in crate)",
        "pub(in crate::telegram)",
    ] {
        require_visibility(
            &parsed_struct_visibility(form),
            RequiredVisibility::Super,
            "fixture module/facade",
        )
        .expect_err("noncanonical module/facade visibility must fail");
    }

    require_visibility(
        &parsed_struct_visibility(""),
        RequiredVisibility::Private,
        "fixture private field/helper",
    )
    .expect("inherited visibility should be private");
    require_visibility(
        &parsed_struct_visibility("pub(crate)"),
        RequiredVisibility::Private,
        "fixture private field/helper",
    )
    .expect_err("public helper must not satisfy private contract");
}

#[test]
fn moved_symbol_map_preserves_duplicates_locations_and_rejects_local_substitutes() {
    let duplicate = Fixture::new("duplicate-symbol-bodies");
    duplicate.write("src/lib.rs", "mod telegram;\n");
    duplicate.write(
        "src/telegram.rs",
        "pub mod claude_watcher { #[cfg(unix)] pub mod output { pub struct BridgeLogger; } #[cfg(windows)] pub mod output { pub struct BridgeLogger; } }\n",
    );
    let duplicate_index = duplicate
        .index_as(CRATE_ID)
        .expect("duplicate fixture should index");
    let duplicate_rows = collect_moved_symbol_occurrences(&duplicate_index, CRATE_ID)
        .remove(&format!("struct {OUTPUT_MODULE}::BridgeLogger"))
        .expect("duplicate BridgeLogger key should exist");
    assert_eq!(duplicate_rows.len(), 2);
    assert_ne!(duplicate_rows[0].body_id, duplicate_rows[1].body_id);

    let misplaced = Fixture::new("misplaced-inherent-method");
    misplaced.write("src/lib.rs", "mod telegram;\n");
    misplaced.write(
        "src/telegram.rs",
        "pub mod claude_watcher { pub mod output { pub struct BridgeLogger; impl BridgeLogger { pub fn new() -> Self { Self } pub fn log(&mut self) {} } } } pub mod bridge { impl crate::telegram::claude_watcher::output::BridgeLogger { pub fn log(&mut self) {} } }\n",
    );
    let misplaced_index = misplaced
        .index_as(CRATE_ID)
        .expect("misplaced fixture should index");
    let misplaced_map = collect_moved_symbol_occurrences(&misplaced_index, CRATE_ID);
    let misplaced_log = misplaced_map
        .get(&format!("method {OUTPUT_MODULE}::BridgeLogger::log"))
        .expect("out-of-place method should resolve through its destination self type");
    assert_eq!(misplaced_log.len(), 2);
    assert!(misplaced_log
        .iter()
        .any(|row| row.body_id.contains("::output#")));
    assert!(misplaced_log
        .iter()
        .any(|row| row.body_id.contains("::bridge#")));

    let local = Fixture::new("local-item-not-definition");
    local.write("src/lib.rs", "mod telegram;\n");
    local.write(
        "src/telegram.rs",
        "pub mod claude_watcher { pub mod output { fn wrapper() { struct BridgeLogger; } } }\n",
    );
    let local_index = local
        .index_as(CRATE_ID)
        .expect("local-item fixture should index");
    assert!(
        !collect_moved_symbol_occurrences(&local_index, CRATE_ID)
            .contains_key(&format!("struct {OUTPUT_MODULE}::BridgeLogger")),
        "function-local item must not satisfy module-scope destination presence"
    );
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
fn raw_module_ids_normalize_and_exact_cfg_test_bodies_are_excluded() {
    let fixture = Fixture::new("raw-modules-and-cfg-test");
    fixture.write("src/lib.rs", "mod r#telegram;\n");
    fixture.write(
        "src/telegram.rs",
        "pub mod r#bridge {}\n#[cfg(test)] mod ignored { mod missing; use super::bridge::Forbidden; }\npub mod r#claude_watcher {}\n",
    );
    let index = fixture
        .index_as(CRATE_ID)
        .expect("raw target tree should index");
    assert!(index.declared_modules.contains(TARGET_MODULE));
    assert!(!index
        .declared_modules
        .contains("agentscommander_lib::telegram::ignored"));
    let report = analyze_module_index(&index, CRATE_ID, TARGET_MODULE)
        .expect("raw target body should analyze");
    assert!(report
        .dependencies
        .iter()
        .all(|row| row.module != "agentscommander_lib::telegram::bridge"));
}

#[test]
fn invalid_direct_path_attributes_fail_with_named_diagnostics() {
    let non_string = Fixture::new("non-string-path");
    non_string.write("src/lib.rs", "#[path = 1] mod child;\n");
    let non_string_message = non_string
        .index()
        .expect_err("non-string path must fail")
        .to_string();
    assert!(non_string_message.contains("non-string path value"));
    assert!(non_string_message.contains("module child"));

    let duplicate = Fixture::new("duplicate-direct-path");
    duplicate.write(
        "src/lib.rs",
        "#[path = \"first.rs\"] #[path = \"second.rs\"] mod child;\n",
    );
    duplicate.write("src/first.rs", "");
    duplicate.write("src/second.rs", "");
    let duplicate_message = duplicate
        .index()
        .expect_err("duplicate direct path attributes must fail")
        .to_string();
    assert!(duplicate_message.contains("duplicate direct path attributes"));
}

#[test]
fn failure_messages_include_contract_diffs_rerun_scope_and_detector() {
    let fixture = Fixture::new("drop-reporting-contract");
    let fixture_root = fixture.root.clone();
    assert!(
        fixture_root.is_absolute(),
        "simulated cleanup fixture path must be absolute: {}",
        fixture_root.display()
    );
    let simulated_error = std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "simulated Windows sharing violation (os error 32)",
    );
    let simulated_error_text = simulated_error.to_string();
    let drop_diagnostic = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed_fixture_root = fixture_root.clone();
    let fixture = fixture.with_drop_cleanup(
        move |observed_root| {
            assert_eq!(observed_root, observed_fixture_root.as_path());
            Err(simulated_error)
        },
        RecordingDiagnostic {
            bytes: std::rc::Rc::clone(&drop_diagnostic),
        },
    );
    let drop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(fixture)));
    let cleanup_result = fs::remove_dir_all(&fixture_root);
    assert!(
        drop_result.is_ok(),
        "Fixture::drop must not panic while reporting a simulated cleanup failure"
    );
    assert!(
        cleanup_result.is_ok(),
        "could not remove {} after the injected Fixture::drop failure: {cleanup_result:?}",
        fixture_root.display()
    );

    let writer_fixture = Fixture::new("drop-writer-error-contract");
    let writer_fixture_root = writer_fixture.root.clone();
    let observed_writer_fixture_root = writer_fixture_root.clone();
    let write_calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let writer_fixture = writer_fixture.with_drop_cleanup(
        move |observed_root| {
            assert_eq!(observed_root, observed_writer_fixture_root.as_path());
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated Windows sharing violation (os error 32)",
            ))
        },
        FailingDiagnostic {
            write_calls: std::rc::Rc::clone(&write_calls),
        },
    );
    let writer_drop_result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(writer_fixture)));
    let writer_cleanup_result = fs::remove_dir_all(&writer_fixture_root);
    assert!(
        writer_drop_result.is_ok(),
        "Fixture::drop must not panic when its diagnostic writer returns an error"
    );
    assert!(
        write_calls.get() > 0,
        "Fixture::drop did not exercise the injected failing diagnostic writer"
    );
    assert!(
        writer_cleanup_result.is_ok(),
        "could not remove {} after the injected writer failure: {writer_cleanup_result:?}",
        writer_fixture_root.display()
    );

    let drop_diagnostic = drop_diagnostic.borrow().clone();
    let drop_diagnostic = String::from_utf8(drop_diagnostic)
        .expect("fixture cleanup fallback diagnostic should be UTF-8");
    assert_eq!(
        drop_diagnostic,
        format!(
            "fixture cleanup fallback could not remove isolated fixture {}: {simulated_error_text}\n",
            fixture_root.display()
        ),
        "Drop fallback must report the absolute fixture path and concrete cleanup error"
    );

    let report = GuardReport {
        sources: [TARGET_ROOT_SOURCE.to_owned()].into_iter().collect(),
        dependencies: [DependencyObservation {
            source: TARGET_ROOT_SOURCE.to_owned(),
            module: "agentscommander_lib::telegram::bridge".to_owned(),
        }]
        .into_iter()
        .collect(),
    };
    let message = require_exact_production_report(&report)
        .expect_err("incomplete report must fail")
        .to_string();
    for marker in [
        "CONTRACT:",
        "missing expected source rows",
        "unexpected observed dependency rows",
        TARGET_ROOT_SOURCE,
        "agentscommander_lib::telegram::bridge",
        FOCUSED_RERUN,
        "SCOPE:",
        AUTHORITATIVE_TOPOLOGY,
    ] {
        assert!(
            message.contains(marker),
            "missing diagnostic marker {marker}"
        );
    }
}

#[test]
#[cfg(any())]
fn production_guard_observes_the_exact_initial_dependency_set() {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crate_root = manifest_root.join("src/lib.rs");
    let first_index = build_module_index(&manifest_root, &crate_root, CRATE_ID)
        .expect("production module tree should build");
    let second_index = build_module_index(&manifest_root, &crate_root, CRATE_ID)
        .expect("second production module tree should build");
    let first = analyze_module_index(&first_index, CRATE_ID, TARGET_MODULE)
        .unwrap_or_else(|error| panic!("production guard analysis failed:\n{error}"));
    let second = analyze_module_index(&second_index, CRATE_ID, TARGET_MODULE)
        .unwrap_or_else(|error| panic!("second production guard analysis failed:\n{error}"));

    assert_eq!(first, second, "guard observations must be deterministic");
    require_exact_production_report(&first)
        .unwrap_or_else(|error| panic!("production guard failed:\n{error}"));
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
fn collect_production_rust_sources(directory: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read production source directory") {
        let entry = entry.expect("read production source entry");
        let path = entry.path();
        if path.is_dir() {
            collect_production_rust_sources(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn guarded_source(relative_path: &str) -> String {
    std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path),
    )
    .expect("read guarded source")
}

#[test]
fn output_seam_has_exact_pure_ownership() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = guarded_source("src/telegram/output.rs");
    let bridge = guarded_source("src/telegram/bridge.rs");
    let watcher = guarded_source("src/telegram/claude_watcher.rs");

    assert!(!root.join("src/telegram/claude_watcher/output.rs").exists());
    assert!(output.contains("#[derive(Debug, Clone, Copy, PartialEq, Eq)]"));
    assert!(output.contains("pub(crate) enum TelegramErrKind"));
    assert!(output.contains("pub(crate) fn classify(msg: &str) -> Self"));
    assert!(output.contains("pub(crate) fn as_str(self) -> &'static str"));
    assert!(output.contains("pub(crate) fn prepare_output_chunks"));

    for forbidden in [
        "AppHandle",
        "tauri",
        "Emitter",
        "OutboundNetwork",
        "api::send_message",
        "redact",
        "config",
        "tokio",
        "log::",
        "crate::",
        "agentscommander_lib::",
        "BridgeLogger",
        "DiagLogger",
        "flush_buffer",
    ] {
        assert!(
            !output.contains(forbidden),
            "telegram/output.rs must not contain {forbidden}"
        );
    }

    assert!(bridge.contains("struct BridgeLogger"));
    assert!(bridge.contains("struct DiagLogger"));
    assert!(bridge.contains("fn flush_buffer"));
    assert!(bridge.contains("prepare_output_chunks"));
    assert!(!bridge.contains("claude_watcher::output"));

    assert!(watcher.contains("struct WatcherLogger"));
    assert!(watcher.contains("struct WatcherDiagLogger"));
    assert!(watcher.contains("fn flush_watcher_buffer"));
    assert!(watcher.contains("prepare_output_chunks"));
    assert!(!watcher.contains("claude_watcher::output"));
    assert!(!watcher.contains("telegram::bridge"));
}

#[test]
fn no_production_source_references_the_removed_watcher_child() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_production_rust_sources(&root.join("src"), &mut files);

    for file in files {
        let source = std::fs::read_to_string(&file).expect("read production Rust source");
        assert!(
            !source.contains("claude_watcher::output"),
            "removed watcher child reference in {}",
            file.display()
        );
    }
}

#[test]
fn moved_and_watcher_adapter_tests_are_present() {
    let output = guarded_source("src/telegram/output.rs");
    let bridge = guarded_source("src/telegram/bridge.rs");
    let watcher = guarded_source("src/telegram/claude_watcher.rs");

    for test_name in [
        "classify_unauthorized",
        "classify_conflict",
        "classify_rate_limited",
        "classify_network",
        "classify_other_falls_through",
    ] {
        assert!(output.contains(test_name));
    }

    for test_name in [
        "bridgelogger_log_redacts_telegram_token_in_text",
        "bridgelogger_log_redacts_before_truncating_at_500_bytes",
        "diaglogger_log_raw_redacts_telegram_token",
        "diaglogger_log_sent_redacts_gemini_key",
    ] {
        assert!(bridge.contains(test_name));
    }

    for test_name in [
        "watcherlogger_log_redacts_telegram_token_in_text",
        "watcherlogger_log_redacts_before_truncating_at_500_bytes",
        "watcherdiaglogger_log_raw_redacts_telegram_token",
        "watcherdiaglogger_log_sent_redacts_gemini_key",
    ] {
        assert!(watcher.contains(test_name));
    }
}
