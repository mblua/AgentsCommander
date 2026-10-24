use super::*;

pub(crate) fn unraw(ident: &syn::Ident) -> String {
    ident.unraw().to_string()
}

pub(crate) fn render_relative(root: &Path, path: &Path) -> Result<String, GuardError> {
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

pub(crate) fn canonicalize(path: &Path, operation: &'static str) -> Result<PathBuf, GuardError> {
    fs::canonicalize(path).map_err(|error| {
        GuardError::new(
            operation,
            format!("could not canonicalize {}: {error}", path.display()),
        )
    })
}

/// Base that `#[path]` attributes resolve against for modules declared in `parent`.
fn declaration_base(parent: &ModuleBody) -> Result<PathBuf, GuardError> {
    if parent.inline {
        return Ok(parent.descendant_base.clone());
    }
    let base = parent.source.parent().ok_or_else(|| {
        GuardError::new(
            "module declaration base",
            format!("source has no parent: {}", parent.source.display()),
        )
    })?;
    Ok(base.to_path_buf())
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
                test_only_sources: BTreeSet::new(),
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
        let canonical = canonicalize(path, operation)?;
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
            vec![declaration_base(parent)?.join(path)]
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
            let declaration_base = declaration_base(parent)?;
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

    /// Records the files of an out-of-line test-only module and, recursively, of
    /// the out-of-line modules they declare. None of them is indexed as production.
    fn record_test_only_sources(
        &mut self,
        parent: &ModuleBody,
        module: &ItemMod,
    ) -> Result<(), GuardError> {
        if module.content.is_some() {
            return Ok(());
        }
        for source in self.out_of_line_sources(parent, module)? {
            let relative = render_relative(&self.manifest_root, &source)?;
            let body = ModuleBody {
                module_id: format!("{}::{}", parent.module_id, unraw(&module.ident)),
                body_id: format!("{relative}@test-only"),
                descendant_base: descendant_base_for_source(&source)?,
                items: self.parse_source(&source)?,
                inline: false,
                relative_source: relative.clone(),
                source,
            };
            self.index.test_only_sources.insert(relative);
            for item in &body.items {
                if let Item::Mod(child) = item {
                    self.record_test_only_sources(&body, child)?;
                }
            }
        }
        Ok(())
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
                // Not indexed, but a `#[cfg(test)] mod x;` file is still declared.
                self.record_test_only_sources(&body, module)?;
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
                            "inline module {name} combines a direct path with conditional path \
        variants"
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

pub(crate) fn build_module_index(
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
    let canonical_manifest = canonicalize(manifest_root, "manifest canonicalization")?;
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
