use super::*;

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
pub(crate) struct UseLeaf {
    pub(crate) path: Vec<String>,
    pub(crate) binding: Option<String>,
    pub(crate) glob: bool,
}

pub(crate) fn expand_use_tree(tree: &UseTree, prefix: &mut Vec<String>, leaves: &mut Vec<UseLeaf>) {
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

pub(crate) fn resolve_internal_module(
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

pub(crate) fn analyze_module_index(
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

pub(crate) fn analyze_guard(
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

    let canonical_manifest = canonicalize(manifest_root, "manifest canonicalization")?;
    let canonical_crate_root = canonicalize(crate_root_file, "crate-root selection")?;
    render_relative(&canonical_manifest, &canonical_crate_root)?;

    let mut report = GuardReport {
        sources: BTreeSet::new(),
        dependencies: BTreeSet::new(),
    };
    for spec in source_specs {
        let canonical_source = canonicalize(&spec.path, "target source read")?;
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
