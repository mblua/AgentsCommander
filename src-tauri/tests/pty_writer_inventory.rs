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

#[derive(Default)]
struct BinaryWriterCalls {
    acquire: usize,
    write: usize,
}

impl<'ast> Visit<'ast> for BinaryWriterCalls {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*node.func {
            let mut segments = path.path.segments.iter().rev();
            let call = segments.next().expect("call path segment");
            if segments
                .next()
                .is_some_and(|owner| owner.ident == "PtyManager")
            {
                self.acquire += usize::from(call.ident == "acquire_input_writer");
                self.write += usize::from(call.ident == "write_with_permit");
            }
        }
        visit::visit_expr_call(self, node);
    }
}

fn binary_handler_before_move() -> bool {
    let mut handlers = Vec::new();
    for relative in ["src/web.rs", "src/web/commands.rs"] {
        let parsed = syn::parse_file(&source(relative)).expect("binary handler source parses");
        for item in parsed.items {
            if let syn::Item::Fn(function) = item {
                if function.sig.ident == "handle_binary_message" {
                    handlers.push((relative, function));
                }
            }
        }
    }
    assert_eq!(
        handlers.len(),
        1,
        "binary handler must have exactly one location"
    );
    let (relative, handler) = handlers.pop().expect("one binary handler");
    let mut calls = BinaryWriterCalls::default();
    calls.visit_block(&handler.block);
    assert_eq!(
        calls.acquire, 1,
        "binary handler must call acquire_input_writer exactly once"
    );
    assert_eq!(
        calls.write, 1,
        "binary handler must call write_with_permit exactly once"
    );
    let before_move = relative == "src/web.rs";
    if !before_move {
        let web = normalized(&source("src/web.rs"));
        assert!(!web.contains("write_with_permit("));
        assert!(!web.contains("acquire_input_writer("));
    }
    before_move
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
    let before_move = binary_handler_before_move();
    // 1. Every interactive/automated writer surface routes through the permit.
    let mut expected_surfaces = vec![
        "src/commands/pty.rs",
        "src/web.rs",
        "src/web/commands.rs",
        "src/pty/inject.rs",
        "src/phone/mailbox/teardown.rs",
    ];
    expected_surfaces.retain(|relative| before_move || *relative != "src/web.rs");
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

    let mut expected_permit_paths = vec![
        "src/commands/pty.rs",
        "src/phone/mailbox/teardown.rs",
        "src/pty/inject.rs",
        // Moved out of pty::inject: its permit-write tests.
        "src/pty/inject/tests/submit.rs",
        "src/pty/input_route.rs",
        "src/pty/input_route/tests.rs",
        "src/pty/manager.rs",
        // Moved out of pty::manager: the write_with_permit body and its tests.
        "src/pty/manager/route_proof.rs",
        "src/pty/manager/tests.rs",
        "src/pty/manager/tests/response_close_writer.rs",
        "src/web.rs",
        "src/web/commands.rs",
    ];
    expected_permit_paths.retain(|relative| before_move || *relative != "src/web.rs");
    assert_eq!(
        permit_paths, expected_permit_paths,
        "a new PTY writer must be deliberately added to this complete permit inventory"
    );

    assert_eq!(route_guard_capability_paths, vec!["src/pty/input_route.rs"]);
    let manager = normalized(&source("src/pty/manager.rs"));
    // 4. Direct PtyBackend::write callers are limited to the manager chokepoint
    //    (production) and the pinned container-backend transport probes (tests).
    //    Whitespace-normalized so a multiline reflow cannot drop a site.
    assert_eq!(
        raw_backend_paths,
        vec![
            "src/pty/backend.rs",
            "src/pty/container_backend/tests/transport.rs",
        ],
        "direct backend writes: route guard and pinned container probes"
    );
    assert_eq!(
        occurrences(
            &normalized(&source("src/pty/container_backend/tests/transport.rs")),
            "backend.write("
        ),
        3,
        "exactly three cfg(test) transport-write probes"
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

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mailbox = std::iter::once(manifest.join("src/phone/mailbox/pty_dispatch.rs"))
        .chain(rust_sources(
            &manifest.join("src/phone/mailbox/pty_dispatch"),
        ))
        .map(|path| {
            occurrences(
                &normalized(&source(&relative_of(&path))),
                "write_exact_agent_input_first(",
            )
        })
        .sum::<usize>();
    assert_eq!(
        mailbox, 1,
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

use std::collections::{BTreeMap, BTreeSet};
use syn::visit::{self, Visit};

const DOMAIN: &str = "src/pty/input_route.rs";
const SEALED: &[&str] = &[
    "BackendWriteAuthority",
    "PtyInputPermit",
    "PtyRouteWriteGuard",
    "InputPreparation",
    "PendingRegistration",
    "NonWritingRouteProof",
    "PtyAuthorityRouteProof",
];

#[derive(Default)]
struct CapabilityInventory {
    file: String,
    aliases: BTreeMap<String, String>,
    owner: String,
    function: String,
    test: bool,
    errors: Vec<String>,
    sites: BTreeMap<String, usize>,
}

fn test_cfg(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Path>()
                .is_ok_and(|path| path.is_ident("test"))
    })
}

fn use_names(tree: &syn::UseTree, names: &mut BTreeMap<String, String>) {
    match tree {
        syn::UseTree::Path(path) => use_names(&path.tree, names),
        syn::UseTree::Name(name) => {
            record_alias(names, name.ident.to_string(), name.ident.to_string());
        }
        syn::UseTree::Rename(name) => {
            record_alias(names, name.rename.to_string(), name.ident.to_string());
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                use_names(item, names);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

impl CapabilityInventory {
    fn resolve(&self, name: &str) -> String {
        if name == "Self" {
            return self.owner.clone();
        }
        let mut current = name.to_string();
        let mut seen = BTreeSet::new();
        while let Some(next) = self.aliases.get(&current) {
            if next == &current {
                break;
            }
            if !seen.insert(current.clone()) {
                return "ambiguous_alias".into();
            }
            current = next.clone();
        }
        current
    }

    fn path_type(&self, path: &syn::Path) -> String {
        self.resolve(
            &path
                .segments
                .last()
                .expect("path segment")
                .ident
                .to_string(),
        )
    }

    fn type_name(&self, ty: &syn::Type) -> String {
        match ty {
            syn::Type::Path(path) => self.path_type(&path.path),
            _ => String::new(),
        }
    }

    fn reject(&mut self, reason: &str) {
        self.errors.push(format!(
            "{}:{}::{}: {reason}",
            self.file, self.owner, self.function
        ));
    }

    fn site(&mut self, key: &str) {
        *self.sites.entry(key.to_string()).or_default() += 1;
    }

    fn literal(&mut self, ty: &str) {
        if !SEALED.contains(&ty) && ty != "BackendRawWriter" {
            return;
        }
        let expected = match ty {
            "BackendWriteAuthority" if self.test => "BackendWriteAuthority::for_backend_test",
            "BackendWriteAuthority" => "BackendWriteAuthority::for_route_guard",
            "InputPreparation" => "RouteRegistry::prepare",
            "PtyInputPermit" => "InputPreparation::acquire",
            "PtyRouteWriteGuard" => "::lock_route_for_write",
            "PendingRegistration" => "RouteRegistry::mark_spawning",
            "NonWritingRouteProof" => "RouteRegistry::snapshot_route_proof",
            "PtyAuthorityRouteProof" => "RouteRegistry::authority_route_proof",
            "BackendRawWriter" => "::raw_writer",
            _ => unreachable!(),
        };
        let site = format!("{}::{}", self.owner, self.function);
        let verified_guard =
            ty == "PtyRouteWriteGuard" && site == "::lock_route_for_verified_write" && !self.test;
        let file = if ty == "BackendRawWriter" {
            "src/pty/backend.rs"
        } else {
            DOMAIN
        };
        if self.file != file || (!verified_guard && site != expected) {
            self.reject(&format!("unlisted construction {ty}"));
        }
        self.site(&format!("literal:{ty}:{site}:test={}", self.test));
    }
}

impl<'ast> Visit<'ast> for CapabilityInventory {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        use_names(&node.tree, &mut self.aliases);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        let name = self.type_name(&node.ty);
        if !name.is_empty() {
            record_alias(&mut self.aliases, node.ident.to_string(), name);
        }
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if self.file == "src/pty.rs"
            && node.ident == "input_route"
            && !is_crate_visibility(&node.vis)
        {
            self.reject("domain module visibility");
        }
        let old = self.test;
        self.test |= test_cfg(&node.attrs);
        visit::visit_item_mod(self, node);
        self.test = old;
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let name = node.ident.to_string();
        if name == "BackendRawWriter" {
            if self.file != "src/pty/backend.rs"
                || !matches!(node.vis, syn::Visibility::Inherited)
                || node
                    .fields
                    .iter()
                    .any(|f| !matches!(f.vis, syn::Visibility::Inherited))
            {
                self.reject("raw adapter must be private");
            }
            self.site("adapter-definition");
        }
        if SEALED.contains(&name.as_str()) {
            if self.file != DOMAIN {
                self.reject("capability definition outside domain");
            }
            for field in &node.fields {
                if !matches!(field.vis, syn::Visibility::Inherited) {
                    self.reject(&format!(
                        "nonprivate field {name}.{}",
                        field
                            .ident
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default()
                    ));
                }
                if name == "NonWritingRouteProof" {
                    let field_name = field
                        .ident
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    let mut field_types = TypePaths::default();
                    field_types.visit_type(&field.ty);
                    if ["writer", "input", "token"]
                        .iter()
                        .any(|word| field_name.contains(word))
                        || ["RawPtyWriter", "PtyInputPermit", "BackendWriteAuthority"]
                            .iter()
                            .any(|word| {
                                field_types.0.iter().any(|name| self.resolve(name) == *word)
                            })
                    {
                        self.reject("nonwriting proof has writing authority");
                    }
                }
            }
            for attr in &node.attrs {
                if attr.path().is_ident("derive") {
                    let paths = attr.parse_args_with(
                        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
                        .expect("derive paths");
                    for path in paths {
                        if ["Clone", "Copy", "Default"].contains(&self.path_type(&path).as_str()) {
                            self.reject("authority derive");
                        }
                    }
                }
            }
        }
        visit::visit_item_struct(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let old_owner = self.owner.clone();
        let old_test = self.test;
        self.test |= test_cfg(&node.attrs);
        self.owner = self.type_name(&node.self_ty);
        if let Some((_, path, _)) = &node.trait_ {
            let name = self.path_type(path);
            if SEALED.contains(&self.owner.as_str())
                && ["Clone", "Copy", "Default", "From", "Into"].contains(&name.as_str())
            {
                self.reject("authority manual impl or conversion");
            }
            let mut trait_types = TypePaths::default();
            trait_types.visit_path(path);
            if ["From", "Into"].contains(&name.as_str())
                && trait_types
                    .0
                    .iter()
                    .any(|ty| SEALED.contains(&self.resolve(ty).as_str()))
            {
                self.reject("authority manual impl or conversion");
            }
            if name == "RawPtyWriter" && !self.test {
                if self.file != "src/pty/backend.rs" || self.owner != "BackendRawWriter" {
                    self.reject("unlisted raw adapter");
                }
                self.site("adapter");
            }
        }
        visit::visit_item_impl(self, node);
        self.owner = old_owner;
        self.test = old_test;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let old = self.function.clone();
        let old_test = self.test;
        self.function = node.sig.ident.to_string();
        self.test |= test_cfg(&node.attrs);
        if self.function == "for_route_guard" {
            if self.owner != "BackendWriteAuthority"
                || self.file != DOMAIN
                || self.test
                || !matches!(node.vis, syn::Visibility::Inherited)
            {
                self.reject("issuer must be private production domain constructor");
            }
            self.site("issuer");
        }
        if self.function == "for_backend_test" && self.owner == "BackendWriteAuthority" {
            if !self.test {
                self.reject("ungated backend test helper");
            }
            self.site("test-helper");
        }
        if self.file == DOMAIN
            && matches!(
                self.function.as_str(),
                "publish"
                    | "prepare"
                    | "acquire"
                    | "mark_spawning"
                    | "snapshot_route_proof"
                    | "batch_snapshot_proofs"
                    | "authority_route_proof"
            )
            && !is_crate_visibility(&node.vis)
        {
            self.reject("entry visibility");
        }
        visit::visit_impl_item_fn(self, node);
        self.function = old;
        self.test = old_test;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let old = self.function.clone();
        let old_test = self.test;
        self.function = node.sig.ident.to_string();
        self.test |= test_cfg(&node.attrs);
        if self.function == "raw_writer" {
            if self.file != "src/pty/backend.rs" || !is_crate_visibility(&node.vis) || self.test {
                self.reject("factory visibility or location");
            }
            self.site("factory-definition");
        }
        if self.file == DOMAIN
            && self.function.starts_with("lock_route_for_")
            && !is_crate_visibility(&node.vis)
        {
            self.reject("entry visibility");
        }
        visit::visit_item_fn(self, node);
        self.function = old;
        self.test = old_test;
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        let ty = self.path_type(&node.path);
        self.literal(&ty);
        visit::visit_expr_struct(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*node.func {
            let name = self.path_type(&path.path);
            if SEALED.contains(&name.as_str()) {
                self.reject("unlisted tuple construction");
            }
            if name == "raw_writer" && !self.test {
                if self.file != "src/pty/manager.rs"
                    || self.function != "record_route_with_identities"
                {
                    self.reject("unlisted factory call");
                }
                self.site("factory-call");
            }
            if name == "for_route_guard" {
                if self.file != DOMAIN
                    || self.owner != "PtyRouteWriteGuard"
                    || self.function != "write"
                    || self.test
                {
                    self.reject("extra issuer call");
                }
                self.site("issuer-call");
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if let syn::Expr::Field(field) = &*node.receiver {
            if let syn::Member::Named(member) = &field.member {
                let member = member.to_string();
                if ["routes", "pending"].contains(&member.as_str())
                    && ["insert", "remove"].contains(&node.method.to_string().as_str())
                    && self.file == DOMAIN
                {
                    let method = node.method.to_string();
                    let expected = match (member.as_str(), method.as_str()) {
                        ("routes", "insert") => "publish",
                        ("pending", "insert") => "mark_spawning",
                        ("pending", "remove") => "drop",
                        _ => &self.function,
                    };
                    if self.function != expected {
                        self.reject("unlisted publisher");
                    }
                    self.site(&format!("map:{member}:{method}:{}", self.function));
                }
            }
        }
        visit::visit_expr_method_call(self, node);
    }
}

fn is_crate_visibility(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Restricted(v)
        if v.path.is_ident("crate") && v.in_token.is_none())
}

fn inspect(relative: &str, text: &str) -> CapabilityInventory {
    inspect_with_cfg(relative, text, false)
}

fn inspect_with_cfg(relative: &str, text: &str, test: bool) -> CapabilityInventory {
    let parsed = syn::parse_file(text).expect("valid inventory Rust input");
    let mut inventory = CapabilityInventory {
        file: relative.into(),
        test,
        ..Default::default()
    };
    // Collect imports and aliases before bodies, so source order cannot hide a type.
    for _ in 0..2 {
        for item in &parsed.items {
            match item {
                syn::Item::Use(item) => inventory.visit_item_use(item),
                syn::Item::Type(item) => inventory.visit_item_type(item),
                _ => {}
            }
        }
    }
    inventory.visit_file(&parsed);
    if inventory
        .aliases
        .keys()
        .any(|name| inventory.resolve(name) == "ambiguous_alias")
    {
        inventory.reject("unsupported alias ambiguity");
    }
    inventory
}

#[test]
fn capability_construction_and_privacy_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut total = BTreeMap::<String, usize>::new();
    let files = rust_sources(&root)
        .into_iter()
        .map(|path| {
            let relative = relative_of(&path);
            let text = source(&relative);
            (relative, text)
        })
        .collect::<Vec<_>>();
    let test_files = test_module_files(&files);
    for (relative, text) in files {
        let mut inventory = inspect_with_cfg(&relative, &text, test_files.contains(&relative));
        // External test files are a cfg(test) module tree, not a path-spelling exception.
        // Their module declarations are checked by the compiler; no sealed literals are allowed.
        assert!(inventory.errors.is_empty(), "{:?}", inventory.errors);
        for (key, count) in std::mem::take(&mut inventory.sites) {
            *total.entry(key).or_default() += count;
        }
    }
    let expected = [
        ("issuer", 1),
        ("issuer-call", 1),
        ("test-helper", 1),
        ("adapter", 1),
        ("adapter-definition", 1),
        ("factory-definition", 1),
        ("factory-call", 1),
        (
            "literal:BackendWriteAuthority:BackendWriteAuthority::for_route_guard:test=false",
            1,
        ),
        (
            "literal:BackendWriteAuthority:BackendWriteAuthority::for_backend_test:test=true",
            1,
        ),
        ("literal:BackendRawWriter:::raw_writer:test=false", 1),
        (
            "literal:InputPreparation:RouteRegistry::prepare:test=false",
            1,
        ),
        (
            "literal:PtyInputPermit:InputPreparation::acquire:test=false",
            1,
        ),
        (
            "literal:PtyRouteWriteGuard:::lock_route_for_write:test=false",
            1,
        ),
        (
            "literal:PtyRouteWriteGuard:::lock_route_for_verified_write:test=false",
            1,
        ),
        (
            "literal:PendingRegistration:RouteRegistry::mark_spawning:test=false",
            1,
        ),
        (
            "literal:NonWritingRouteProof:RouteRegistry::snapshot_route_proof:test=false",
            1,
        ),
        (
            "literal:PtyAuthorityRouteProof:RouteRegistry::authority_route_proof:test=false",
            1,
        ),
        ("map:routes:insert:publish", 1),
        ("map:pending:insert:mark_spawning", 1),
        ("map:pending:remove:drop", 1),
        ("map:routes:remove:remove_route_if_kind", 2),
        ("map:routes:remove:try_remove_route_if_kind", 1),
    ]
    .into_iter()
    .map(|(key, count)| (key.to_string(), count))
    .collect::<BTreeMap<_, _>>();
    assert_eq!(total, expected);
}

#[test]
fn inventory_rejects_each_visibility_and_construction_mutation() {
    let valid = source(DOMAIN);
    assert!(inspect(DOMAIN, &valid).errors.is_empty());
    let parsed = syn::parse_file(&valid).unwrap();
    let mut mutations = 0;
    for item in parsed.items {
        let syn::Item::Struct(item) = item else {
            continue;
        };
        let name = item.ident.to_string();
        if !SEALED.contains(&name.as_str()) {
            continue;
        }
        let start = valid.find(&format!("struct {name}")).unwrap();
        let body = &valid[start..];
        for field in item.fields {
            let field = field.ident.unwrap().to_string();
            let offset = start + body.find(&format!("    {field}:")).unwrap() + 4;
            for visibility in ["pub(crate) ", "pub(in crate::pty) "] {
                let mut mutated = valid.clone();
                mutated.insert_str(offset, visibility);
                assert!(
                    inspect(DOMAIN, &mutated)
                        .errors
                        .iter()
                        .any(|error| error.contains(&format!("nonprivate field {name}.{field}"))),
                    "missed {name}.{field} {visibility}"
                );
                mutations += 1;
            }
        }
    }
    for visibility in ["pub(crate) ", "pub(in crate::pty) "] {
        let mutated = valid.replace(
            "    fn for_route_guard",
            &format!("    {visibility}fn for_route_guard"),
        );
        assert!(inspect(DOMAIN, &mutated)
            .errors
            .iter()
            .any(|e| e.contains("issuer must")));
        mutations += 1;
    }
    let ungated = valid.replace(
        "#[cfg(test)]\n    pub(crate) fn for_backend_test",
        "pub(crate) fn for_backend_test",
    );
    assert!(inspect(DOMAIN, &ungated)
        .errors
        .iter()
        .any(|e| e.contains("ungated")));
    for name in [
        "prepare",
        "publish",
        "mark_spawning",
        "snapshot_route_proof",
        "batch_snapshot_proofs",
        "authority_route_proof",
        "acquire",
        "lock_route_for_write",
        "lock_route_for_verified_write",
    ] {
        let mutated = valid
            .replace(&format!("pub(crate) fn {name}"), &format!("pub fn {name}"))
            .replace(
                &format!("pub(crate) async fn {name}"),
                &format!("pub async fn {name}"),
            );
        assert!(
            inspect(DOMAIN, &mutated)
                .errors
                .iter()
                .any(|e| e.contains("entry visibility")),
            "entry {name}"
        );
    }
    for capability in SEALED {
        for trait_name in ["Clone", "Copy", "Default"] {
            let derived = valid
                .replace(
                    &format!("pub struct {capability}"),
                    &format!("#[derive(std::{trait_name}::{trait_name})]\npub struct {capability}"),
                )
                .replace(
                    &format!("pub(crate) struct {capability}"),
                    &format!("#[derive({trait_name})]\npub(crate) struct {capability}"),
                );
            assert!(inspect(DOMAIN, &derived)
                .errors
                .iter()
                .any(|e| e.contains("authority derive")));
            let manual = format!(
                "{valid}\nuse std::clone::{trait_name} as Renamed;\n\
                impl Renamed for {capability} {{}}\n"
            );
            assert!(inspect(DOMAIN, &manual)
                .errors
                .iter()
                .any(|e| e.contains("manual impl")));
        }
        let extra = format!(
            "{valid}\ntype Alias = {capability};\n\
            fn illegal() {{ let _ = Alias {{}}; }}"
        );
        assert!(inspect(DOMAIN, &extra)
            .errors
            .iter()
            .any(|e| e.contains("unlisted construction")));
    }
    let extra_issuer = format!(
        "{valid}\nfn illegal() {{\n\
        BackendWriteAuthority::\n for_route_guard(); }}"
    );
    assert!(inspect(DOMAIN, &extra_issuer)
        .errors
        .iter()
        .any(|e| e.contains("extra issuer")));
    let publisher = format!(
        "{valid}\nimpl RouteRegistry {{ fn illegal() {{\n\
        registry.routes.insert(id, entry); }} }}"
    );
    assert!(inspect(DOMAIN, &publisher)
        .errors
        .iter()
        .any(|e| e.contains("publisher")));
    let adapter = "impl crate::pty::input_route::RawPtyWriter for Extra {}";
    assert!(inspect("src/pty/backend.rs", adapter)
        .errors
        .iter()
        .any(|e| e.contains("adapter")));
    let reflow = valid.replace(
        "BackendWriteAuthority::for_route_guard()",
        "BackendWriteAuthority::\n    for_route_guard(\n)",
    );
    assert!(inspect(DOMAIN, &reflow).errors.is_empty());
    eprintln!("independent visibility mutations rejected: {mutations}");
}

fn test_module_files(files: &[(String, String)]) -> BTreeSet<String> {
    let mut tests = BTreeSet::new();
    loop {
        let before = tests.len();
        for (relative, text) in files {
            let parsed = syn::parse_file(text).expect("module inventory parse");
            for item in parsed.items {
                let syn::Item::Mod(module) = item else {
                    continue;
                };
                if module.content.is_some() {
                    continue;
                }
                if tests.contains(relative) || test_cfg(&module.attrs) {
                    let parent = Path::new(relative).parent().unwrap();
                    let stem = Path::new(relative).file_stem().unwrap().to_str().unwrap();
                    let directory = if ["lib", "main", "mod"].contains(&stem) {
                        parent.to_path_buf()
                    } else {
                        parent.join(stem)
                    };
                    tests.insert(
                        directory
                            .join(format!("{}.rs", module.ident))
                            .to_string_lossy()
                            .replace(char::from(92), "/"),
                    );
                }
            }
        }
        if tests.len() == before {
            return tests;
        }
    }
}

#[derive(Default)]
struct TypePaths(Vec<String>);
impl<'ast> Visit<'ast> for TypePaths {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if let Some(segment) = path.segments.last() {
            self.0.push(segment.ident.to_string());
        }
        visit::visit_path(self, path);
    }
}

fn record_alias(names: &mut BTreeMap<String, String>, name: String, target: String) {
    fn resolve(names: &BTreeMap<String, String>, name: &str) -> String {
        let mut seen = BTreeSet::new();
        let mut current = name.to_string();
        while let Some(next) = names.get(&current) {
            if next == &current {
                break;
            }
            if !seen.insert(current.clone()) {
                return "ambiguous_alias".into();
            }
            current = next.clone();
        }
        current
    }
    if let Some(old) = names.get(&name) {
        let previous = resolve(names, old);
        let next = resolve(names, &target);
        let sensitive = |value: &str| {
            SEALED.contains(&value)
                || [
                    "Clone",
                    "Copy",
                    "Default",
                    "From",
                    "Into",
                    "raw_writer",
                    "for_route_guard",
                    "ambiguous_alias",
                ]
                .contains(&value)
        };
        if previous != next && (sensitive(&previous) || sensitive(&next)) {
            names.insert(name, "ambiguous_alias".into());
            return;
        }
    }
    names.insert(name, target);
}
