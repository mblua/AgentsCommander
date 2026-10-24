use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolOccurrence {
    pub(crate) source: String,
    pub(crate) body_id: String,
    pub(crate) item_ordinal: usize,
    pub(crate) associated_item_ordinal: Option<usize>,
}

pub(crate) type SymbolOccurrences = BTreeMap<String, Vec<SymbolOccurrence>>;

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

pub(crate) fn collect_moved_symbol_occurrences(
    index: &ModuleIndex,
    crate_id: &str,
) -> SymbolOccurrences {
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

pub(crate) fn verify_exact_moved_symbols(
    index: &ModuleIndex,
    crate_id: &str,
) -> Result<(), GuardError> {
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
            "missing expected symbols: {missing:?}; unexpected observed \
             symbols: {unexpected:?}; duplicate occurrences: {duplicates:?}; \
             out-of-destination occurrences: {misplaced:?}"
        ),
    ))
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RequiredVisibility {
    Private,
    Crate,
    Super,
    Public,
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

pub(crate) fn require_visibility(
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
        RequiredVisibility::Public => matches!(visibility, Visibility::Public(_)),
    };
    if matches {
        return Ok(());
    }

    let observed = describe_visibility(visibility);
    let expected_text = match expected {
        RequiredVisibility::Private => "private",
        RequiredVisibility::Crate => "pub(crate) with absent in_token",
        RequiredVisibility::Super => "pub(super) with absent in_token",
        RequiredVisibility::Public => "pub",
    };
    let evidence = if observed == "pub(in crate)" {
        "; measured detector 1.1.0 variant is graph-neutral, but it \
         violates the canonical in_token.is_none() contract"
    } else if observed == "pub(in crate::telegram)" {
        "; external live detector 1.1.0 evidence records telegram::output \
         -> telegram for this spelling"
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

fn use_leaves(item_use: &ItemUse) -> Vec<UseLeaf> {
    let mut leaves = Vec::new();
    expand_use_tree(&item_use.tree, &mut Vec::new(), &mut leaves);
    leaves
}

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

pub(crate) fn verify_exact_interface(
    index: &ModuleIndex,
    crate_id: &str,
) -> Result<(), GuardError> {
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
    if !output_declarations.is_empty() {
        return Err(GuardError::new(
            "output module declaration",
            format!(
                "claude_watcher.rs declares output; the module must live under \
                 telegram (got {} declarations in {TARGET_ROOT_SOURCE})",
                output_declarations.len()
            ),
        ));
    }

    let telegram_output_declarations = index
        .bodies
        .iter()
        .filter(|body| body.module_id == "agentscommander_lib::telegram")
        .flat_map(|body| &body.items)
        .filter_map(|item| match item {
            Item::Mod(item_mod) if unraw(&item_mod.ident) == "output" => Some(item_mod),
            _ => None,
        })
        .collect::<Vec<_>>();
    if telegram_output_declarations.len() != 1 || telegram_output_declarations[0].content.is_some()
    {
        return Err(GuardError::new(
            "output module ownership",
            format!(
                "telegram/mod.rs must declare exactly one out-of-line output module, got {}",
                telegram_output_declarations.len()
            ),
        ));
    }
    require_visibility(
        &telegram_output_declarations[0].vis,
        RequiredVisibility::Public,
        "telegram::output module",
    )?;

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
    let bridge_import = bridge_uses
        .iter()
        .copied()
        .filter(|item_use| {
            exact_use_leaf_set(
                item_use,
                &["crate", "telegram", "output"],
                &[
                    "flush_buffer",
                    "BridgeLogger",
                    "DiagLogger",
                    "TelegramErrKind",
                ],
            )
        })
        .collect::<Vec<_>>();
    if bridge_import.len() != 1 {
        return Err(GuardError::new(
            "bridge direct output import",
            format!(
                "expected one exact four-item import, got {}",
                bridge_import.len()
            ),
        ));
    }
    require_visibility(
        &bridge_import[0].vis,
        RequiredVisibility::Private,
        "bridge output import",
    )?;

    // Zero re-exports from bridge: any `use` whose visibility is not inherited and
    // that exposes one of the four cluster names, the `output` module itself, or a
    // glob. Written over raw leaves on purpose — `exact_use_leaf_set` requires an
    // exact-size leaf set, so mirroring the old facade check with four names would
    // never match the real three-name re-export and would pass vacuously.
    let leaked = bridge_uses
        .iter()
        .copied()
        .filter(|item_use| !matches!(item_use.vis, Visibility::Inherited))
        .filter(|item_use| {
            use_leaves(item_use).iter().any(|leaf| {
                leaf.glob
                    || leaf.path.iter().any(|segment| segment == "output")
                    || leaf.path.last().is_some_and(|last| {
                        matches!(
                            last.as_str(),
                            "flush_buffer" | "BridgeLogger" | "DiagLogger" | "TelegramErrKind"
                        )
                    })
            })
        })
        .count();
    if leaked != 0 {
        return Err(GuardError::new(
            "bridge re-export forbidden",
            format!(
                "bridge must not re-export the output cluster; got {leaked} re-exporting use items"
            ),
        ));
    }

    let claude_import = target_body
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Use(item_use)
                if exact_use_leaf_set(
                    item_use,
                    &["crate", "telegram", "output"],
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
