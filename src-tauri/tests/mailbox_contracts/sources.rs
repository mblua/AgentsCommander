//! Discovery of the mailbox module tree and the production/test role of every file.
//!
//! The walk starts at the facade, follows every `mod` declaration (external and
//! inline, with `#[path]` literals) and compares the reached files with the physical
//! `.rs` set under the facade's directory. Each physical read, directory listing and
//! walk entry is recorded as a trace event at the I/O point itself.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::buffer::Cursor;
use syn::ext::IdentExt;
use syn::parse::{ParseStream, Parser};
use syn::visit::{self, Visit};
use syn::{Arm, Attribute, Block, Expr, Item, Meta, Stmt};

/// Role of a file once every declaration reaching it is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Prod,
    Test,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Role::Prod => "prod",
            Role::Test => "test",
        }
    }
}

/// One physical source of the tree, read and parsed exactly once.
pub struct Source {
    /// Repo-relative path with `/`.
    pub path: String,
    pub file: syn::File,
    pub role: Role,
}

/// Result of discovery: every source plus the trace of real I/O.
pub struct Tree {
    pub sources: Vec<Source>,
    pub events: Vec<String>,
}

impl Tree {
    pub fn production(&self) -> impl Iterator<Item = &Source> {
        self.sources.iter().filter(|s| s.role == Role::Prod)
    }
}

struct Walker<'a> {
    repo: &'a Path,
    boundary: String,
    facade: String,
    events: Vec<String>,
    parsed: BTreeMap<String, syn::File>,
    /// Contexts (`true` = test) already walked for each file.
    seen: BTreeMap<String, BTreeSet<bool>>,
}

/// Discovers `facade` (repo-relative, e.g. `src-tauri/src/phone/mailbox.rs`) under `repo`.
pub fn discover(repo: &Path, facade: &str) -> Result<Tree, String> {
    let boundary = facade
        .strip_suffix(".rs")
        .ok_or_else(|| format!("{facade}: facade is not a .rs file"))?
        .to_string();
    let mut walker = Walker {
        repo,
        boundary: boundary.clone(),
        facade: facade.to_string(),
        events: vec![format!("MB_WALK\t{facade}")],
        parsed: BTreeMap::new(),
        seen: BTreeMap::new(),
    };
    let mut physical = BTreeSet::new();
    walker.list(&boundary, &mut physical)?;
    if physical.is_empty() {
        return Err(format!("{boundary}: empty module tree"));
    }
    physical.insert(facade.to_string());
    walker.walk_file(facade, false)?;
    let reached: BTreeSet<String> = walker.seen.keys().cloned().collect();
    if let Some(orphan) = physical.difference(&reached).next() {
        return Err(format!(
            "{orphan}: orphan source, no module declaration reaches it"
        ));
    }
    if let Some(extra) = reached.difference(&physical).next() {
        return Err(format!("{extra}: reached source outside the physical tree"));
    }
    let mut sources = Vec::new();
    for (path, file) in std::mem::take(&mut walker.parsed) {
        let role = if walker.seen[&path].contains(&false) {
            Role::Prod
        } else {
            Role::Test
        };
        walker
            .events
            .push(format!("MB_SOURCE\t{}\t{path}", role.label()));
        sources.push(Source { path, file, role });
    }
    Ok(Tree {
        sources,
        events: walker.events,
    })
}

impl Walker<'_> {
    fn abs(&self, rel: &str) -> PathBuf {
        self.repo.join(rel)
    }

    fn list(&mut self, dir: &str, out: &mut BTreeSet<String>) -> Result<(), String> {
        self.events.push(format!("MB_DIR\t{dir}"));
        let entries = fs::read_dir(self.abs(dir)).map_err(|e| format!("{dir}: unreadable: {e}"))?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("{dir}: unreadable entry: {e}"))?;
            let kind = entry.file_type().map_err(|e| format!("{dir}: {e}"))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|raw| format!("{dir}: name is not UTF-8: {raw:?}"))?;
            names.push((name, kind.is_dir()));
        }
        names.sort();
        for (name, is_dir) in names {
            let rel = format!("{dir}/{name}");
            if is_dir {
                self.list(&rel, out)?;
            } else if name.ends_with(".rs") {
                out.insert(rel);
            }
        }
        Ok(())
    }

    fn parse(&mut self, rel: &str) -> Result<(), String> {
        if self.parsed.contains_key(rel) {
            return Ok(());
        }
        self.events.push(format!("MB_READ\t{rel}"));
        let text =
            fs::read_to_string(self.abs(rel)).map_err(|e| format!("{rel}: unreadable: {e}"))?;
        let file = syn::parse_file(&text).map_err(|e| format!("{rel}: malformed source: {e}"))?;
        self.parsed.insert(rel.to_string(), file);
        Ok(())
    }

    fn walk_file(&mut self, rel: &str, test: bool) -> Result<(), String> {
        if !self.seen.entry(rel.to_string()).or_default().insert(test) {
            return Ok(());
        }
        self.parse(rel)?;
        let items = self.parsed[rel].items.clone();
        let dir = child_dir(rel, rel == self.facade);
        self.walk_items(rel, &dir, &items, test, false)
    }

    fn walk_items(
        &mut self,
        rel: &str,
        dir: &str,
        items: &[Item],
        test: bool,
        inline: bool,
    ) -> Result<(), String> {
        for item in items {
            match item {
                Item::Mod(module) => {
                    let edge = test || is_test_only(&module.attrs);
                    let name = module.ident.to_string();
                    let name = name.trim_start_matches("r#");
                    let path_attr = path_literal(&module.attrs)
                        .map_err(|e| format!("{rel}: mod {name}: {e}"))?;
                    match (&module.content, path_attr) {
                        (Some((_, inner)), path_attr) => {
                            let sub = path_attr
                                .map_or_else(|| format!("{dir}/{name}"), |p| format!("{dir}/{p}"));
                            self.walk_items(rel, &sub, inner, edge, true)?;
                        }
                        (None, Some(p)) => {
                            // Outside inline modules `#[path]` is relative to the
                            // declaring file's directory; inside one, to the inline
                            // module's own directory.
                            let base = if inline { dir } else { parent_dir(rel) };
                            let target = normalize(&format!("{base}/{p}"))
                                .ok_or_else(|| format!("{rel}: mod {name}: path escapes"))?;
                            self.enter(rel, name, &target, edge)?;
                        }
                        (None, None) => {
                            let target = self.resolve(rel, dir, name)?;
                            self.enter(rel, name, &target, edge)?;
                        }
                    }
                }
                Item::Macro(mac) if mac.mac.path.is_ident("include") => {
                    return Err(format!("{rel}: include! source inclusion is not supported"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn enter(&mut self, rel: &str, name: &str, target: &str, test: bool) -> Result<(), String> {
        if !target.starts_with(&format!("{}/", self.boundary)) {
            return Err(format!(
                "{rel}: mod {name}: {target} is outside {}",
                self.boundary
            ));
        }
        if !self.abs(target).is_file() {
            return Err(format!("{rel}: mod {name}: missing source {target}"));
        }
        self.walk_file(target, test)
    }

    fn resolve(&self, rel: &str, dir: &str, name: &str) -> Result<String, String> {
        let flat = format!("{dir}/{name}.rs");
        let nested = format!("{dir}/{name}/mod.rs");
        match (self.abs(&flat).is_file(), self.abs(&nested).is_file()) {
            (true, false) => Ok(flat),
            (false, true) => Ok(nested),
            (true, true) => Err(format!("{rel}: mod {name}: ambiguous {flat} and {nested}")),
            (false, false) => Err(format!("{rel}: mod {name}: missing source {flat}")),
        }
    }
}

fn parent_dir(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Directory holding the children of the module whose file is `rel`.
fn child_dir(rel: &str, facade: bool) -> String {
    if rel.ends_with("/mod.rs") && !facade {
        parent_dir(rel).to_string()
    } else {
        rel.trim_end_matches(".rs").to_string()
    }
}

fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn path_literal(attrs: &[Attribute]) -> Result<Option<String>, String> {
    for attr in attrs {
        if attr.path().is_ident("path") {
            let Meta::NameValue(nv) = &attr.meta else {
                return Err("#[path] without a value".to_string());
            };
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(lit),
                ..
            }) = &nv.value
            else {
                return Err("#[path] is not a string literal".to_string());
            };
            return Ok(Some(lit.value().replace('\\', "/")));
        }
        if let (true, Meta::List(list)) = (attr.path().is_ident("cfg_attr"), &attr.meta) {
            if list.tokens.to_string().contains("path") {
                return Err("#[cfg_attr(.., path)] is not supported".to_string());
            }
        }
    }
    Ok(None)
}

/// True when the attributes only allow the node in a `test` build:
/// `#[test]`, `#[cfg(test)]`, `#[cfg(all(test, ..))]` or an `any` of test-only arms.
pub fn is_test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("test") {
            return true;
        }
        if !attr.path().is_ident("cfg") {
            return false;
        }
        attr.parse_args::<Meta>()
            .is_ok_and(|meta| predicate_needs_test(&meta))
    })
}

fn predicate_needs_test(meta: &Meta) -> bool {
    match meta {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) => {
            let Ok(args) = list.parse_args_with(
                syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return false;
            };
            if list.path.is_ident("all") {
                args.iter().any(predicate_needs_test)
            } else if list.path.is_ident("any") {
                !args.is_empty() && args.iter().all(predicate_needs_test)
            } else {
                false
            }
        }
        Meta::NameValue(_) => false,
    }
}

// ------------------------------------------------------------------ production inventory

pub const MUTATOR: &str = "mark_auto_closed";
pub const CONTEXT: &str = "[context-alert] ";
pub const REMOTE: &str = "[remote-activity] ";

/// Shared inventory of the production portions of every production source.
pub fn inventory(tree: &Tree) -> Index {
    let mut index = Index::default();
    for source in tree.production() {
        let mut collector = Collector {
            index: &mut index,
            file: source.path.clone(),
            owner: vec![None],
            func: Vec::new(),
            missing_arm: 0,
            scopes: vec![(source.path.clone(), true)],
            refs_in_site: BTreeMap::new(),
        };
        collector.visit_file(&source.file);
    }
    index
}

/// Flattened macro tokens; groups become `Open`/`Close` markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Punct(char),
    Lit(String),
    Open,
    Close,
}

pub fn flatten_into<'a>(mut cursor: Cursor<'a>, out: &mut Vec<Tok>) -> Cursor<'a> {
    loop {
        if let Some((inner, _, _, next)) = cursor.any_group() {
            out.push(Tok::Open);
            flatten_into(inner, out);
            out.push(Tok::Close);
            cursor = next;
        } else if let Some((ident, next)) = cursor.ident() {
            out.push(Tok::Ident(ident.unraw().to_string()));
            cursor = next;
        } else if let Some((punct, next)) = cursor.punct() {
            out.push(Tok::Punct(punct.as_char()));
            cursor = next;
        } else if let Some((lit, next)) = cursor.literal() {
            out.push(Tok::Lit(lit.to_string()));
            cursor = next;
        } else if let Some((_, next)) = cursor.token_tree() {
            cursor = next;
        } else {
            return cursor;
        }
    }
}

pub fn toks(mac: &syn::Macro) -> Vec<Tok> {
    let parser = |input: ParseStream| {
        input.step(|cursor| {
            let mut out = Vec::new();
            let rest = flatten_into(*cursor, &mut out);
            Ok((out, rest))
        })
    };
    parser.parse2(mac.tokens.clone()).unwrap_or_default()
}

/// One production function, indexed by owner (impl type or none) and name.
pub struct FnInfo {
    pub file: String,
    pub owner: Option<String>,
    pub name: String,
    pub sig: syn::Signature,
    pub block: Block,
}

pub struct LogSite {
    pub file: String,
    pub func: Option<usize>,
    pub level: String,
    pub value: String,
    pub mac: syn::Macro,
    pub missing_arm: bool,
}

pub struct MutatorRef {
    /// Item path plus the ordinal of the reference inside that item.
    pub site: String,
    pub owner: Option<String>,
    pub form: &'static str,
    /// The owner is reached through a module path (`a::Owner::…`), not a local name.
    pub qualified: bool,
    /// Name scopes visible at the reference, innermost first, ending at its module.
    pub chain: Vec<String>,
}

#[derive(Default)]
pub struct Index {
    pub fns: Vec<FnInfo>,
    pub logs: Vec<LogSite>,
    pub refs: Vec<MutatorRef>,
    /// `(scope, name) -> target name` for renames, type aliases and imports.
    pub aliases: BTreeMap<(String, String), String>,
    /// `(scope, name)` of the types declared in that scope.
    pub declared: BTreeSet<(String, String)>,
    pub errors: Vec<String>,
}

pub struct Collector<'a> {
    pub index: &'a mut Index,
    pub file: String,
    pub owner: Vec<Option<String>>,
    pub func: Vec<usize>,
    pub missing_arm: usize,
    /// Name scopes: the file, inline modules (`true`) and function bodies (`false`).
    pub scopes: Vec<(String, bool)>,
    pub refs_in_site: BTreeMap<String, usize>,
}

// ------------------------------------------------------------------ collection

pub fn expr_attrs(expr: &Expr) -> &[Attribute] {
    macro_rules! attrs {
        ($($v:ident),*) => {
            match expr {
                $(Expr::$v(e) => &e.attrs,)*
                _ => &[],
            }
        };
    }
    attrs!(
        Array, Assign, Async, Await, Binary, Block, Break, Call, Cast, Closure, Const, Continue,
        Field, ForLoop, Group, If, Index, Infer, Let, Lit, Loop, Macro, Match, MethodCall, Paren,
        Path, Range, RawAddr, Reference, Repeat, Return, Struct, Try, TryBlock, Tuple, Unary,
        Unsafe, While, Yield
    )
}

pub fn stmt_test_only(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Local(local) => is_test_only(&local.attrs),
        Stmt::Macro(mac) => is_test_only(&mac.attrs),
        Stmt::Expr(expr, _) => is_test_only(expr_attrs(expr)),
        Stmt::Item(_) => false,
    }
}

pub fn last_ident(path: &syn::Path) -> Option<String> {
    path.segments.last().map(|s| s.ident.unraw().to_string())
}

pub fn type_ident(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) => last_ident(&p.path),
        syn::Type::Reference(r) => type_ident(&r.elem),
        syn::Type::Paren(p) => type_ident(&p.elem),
        syn::Type::Group(g) => type_ident(&g.elem),
        _ => None,
    }
}

impl Collector<'_> {
    fn site(&self) -> String {
        let func = self.func.last().map(|&i| {
            let f = &self.index.fns[i];
            f.owner
                .as_ref()
                .map_or_else(|| f.name.clone(), |o| format!("{o}::{}", f.name))
        });
        let module = self
            .scopes
            .iter()
            .rev()
            .find(|(_, m)| *m)
            .map_or("", |(k, _)| k);
        let inline = module.strip_prefix(self.file.as_str()).unwrap_or("");
        format!(
            "{}{inline}::{}",
            self.file,
            func.unwrap_or_else(|| "<item>".to_string())
        )
    }

    fn scope(&self) -> String {
        self.scopes
            .last()
            .map(|(k, _)| k.clone())
            .unwrap_or_default()
    }

    fn chain(&self) -> Vec<String> {
        let mut chain = Vec::new();
        for (key, module) in self.scopes.iter().rev() {
            chain.push(key.clone());
            if *module {
                break;
            }
        }
        chain
    }

    fn name(&mut self, name: String, target: String) {
        self.index.aliases.insert((self.scope(), name), target);
    }

    fn declare(&mut self, ident: &syn::Ident) {
        let key = (self.scope(), ident.unraw().to_string());
        self.index.declared.insert(key);
    }

    fn push_fn(&mut self, sig: &syn::Signature, block: &Block) -> usize {
        self.index.fns.push(FnInfo {
            file: self.file.clone(),
            owner: self.owner.last().cloned().flatten(),
            name: sig.ident.unraw().to_string(),
            sig: sig.clone(),
            block: block.clone(),
        });
        self.index.fns.len() - 1
    }

    fn record_use(&mut self, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(p) => self.record_use(&p.tree),
            syn::UseTree::Name(n) => {
                let name = n.ident.unraw().to_string();
                self.name(name.clone(), name);
            }
            syn::UseTree::Rename(r) => {
                let (from, to) = (r.ident.unraw().to_string(), r.rename.unraw().to_string());
                self.name(to, from);
            }
            syn::UseTree::Group(g) => g.items.iter().for_each(|t| self.record_use(t)),
            syn::UseTree::Glob(_) => {}
        }
    }

    fn mutator_ref(&mut self, owner: Option<String>, form: &'static str, qualified: bool) {
        let item = self.site();
        let n = self.refs_in_site.entry(item.clone()).or_insert(0);
        *n += 1;
        let site = format!("{item} (reference #{n} in this item)");
        let owner = owner.map(|o| if o == "Self" { self.self_owner() } else { o });
        let chain = self.chain();
        self.index.refs.push(MutatorRef {
            site,
            owner,
            form,
            qualified,
            chain,
        });
    }

    fn self_owner(&self) -> String {
        self.owner
            .last()
            .cloned()
            .flatten()
            .unwrap_or_else(|| "Self".to_string())
    }

    fn scan_tokens(&mut self, mac: &syn::Macro) {
        let trees = toks(mac);
        for (i, tree) in trees.iter().enumerate() {
            if *tree != Tok::Ident(MUTATOR.to_string()) {
                continue;
            }
            let punct = |back: usize| match i.checked_sub(back).map(|j| &trees[j]) {
                Some(Tok::Punct(p)) => Some(*p),
                _ => None,
            };
            match (punct(1), punct(2)) {
                (Some('.'), _) => self.mutator_ref(None, "method call in macro", false),
                (Some(':'), Some(':')) => {
                    let owner = trees[..i - 2].iter().rev().find_map(|t| match t {
                        Tok::Ident(q) => Some(q.clone()),
                        _ => None,
                    });
                    let qualified = matches!(trees.get(i.wrapping_sub(4)), Some(Tok::Punct(':')));
                    self.mutator_ref(owner, "path in macro", qualified);
                }
                _ => self.mutator_ref(None, "bare reference in macro", false),
            }
        }
    }

    fn record_log(&mut self, mac: &syn::Macro) {
        let Some(level) = last_ident(&mac.path) else {
            return;
        };
        let segments = mac.path.segments.len();
        let first = mac.path.segments.first().map(|s| s.ident.to_string());
        let is_log = ["error", "warn", "info", "debug", "trace"].contains(&level.as_str())
            && (segments == 1 || (segments == 2 && first.as_deref() == Some("log")));
        if !is_log {
            return;
        }
        let Some(value) = first_literal(mac) else {
            return;
        };
        if value.starts_with(CONTEXT) || value.starts_with(REMOTE) {
            self.index.logs.push(LogSite {
                file: self.file.clone(),
                func: self.func.last().copied(),
                level,
                value,
                mac: mac.clone(),
                missing_arm: self.missing_arm > 0,
            });
        }
    }
}

pub fn first_literal(mac: &syn::Macro) -> Option<String> {
    toks(mac).into_iter().find_map(|t| match t {
        Tok::Lit(text) => syn::parse_str::<syn::LitStr>(&text).ok().map(|s| s.value()),
        _ => None,
    })
}

pub fn guard_names_missing(arm: &Arm) -> bool {
    struct Finder(bool);
    impl Visit<'_> for Finder {
        fn visit_expr_call(&mut self, call: &syn::ExprCall) {
            if let Expr::Path(p) = &*call.func {
                self.0 |= last_ident(&p.path).as_deref() == Some("err_is_pty_session_missing");
            }
            visit::visit_expr_call(self, call);
        }
    }
    let mut finder = Finder(false);
    if let Some((_, guard)) = &arm.guard {
        finder.visit_expr(guard);
    }
    finder.0
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs = match item {
            syn::Item::Fn(f) => &f.attrs,
            syn::Item::Impl(i) => &i.attrs,
            syn::Item::Mod(m) => &m.attrs,
            syn::Item::Use(u) => &u.attrs,
            syn::Item::Type(t) => &t.attrs,
            syn::Item::Struct(s) => &s.attrs,
            syn::Item::Enum(e) => &e.attrs,
            syn::Item::Const(c) => &c.attrs,
            syn::Item::Static(s) => &s.attrs,
            syn::Item::Trait(t) => &t.attrs,
            syn::Item::Macro(m) => &m.attrs,
            _ => return visit::visit_item(self, item),
        };
        if is_test_only(attrs) {
            return;
        }
        match item {
            syn::Item::Fn(f) => {
                let at = self.push_fn(&f.sig, &f.block);
                self.func.push(at);
                self.scopes
                    .push((format!("{}::fn {}#{at}", self.scope(), f.sig.ident), false));
                visit::visit_item_fn(self, f);
                self.scopes.pop();
                self.func.pop();
            }
            syn::Item::Mod(m) if m.content.is_some() => {
                self.scopes
                    .push((format!("{}::{}", self.scope(), m.ident.unraw()), true));
                visit::visit_item_mod(self, m);
                self.scopes.pop();
            }
            syn::Item::Impl(i) => {
                self.owner.push(type_ident(&i.self_ty));
                visit::visit_item_impl(self, i);
                self.owner.pop();
            }
            syn::Item::Use(u) => self.record_use(&u.tree),
            syn::Item::Type(t) => {
                if let Some(target) = type_ident(&t.ty) {
                    self.name(t.ident.unraw().to_string(), target);
                }
                visit::visit_item_type(self, t);
            }
            syn::Item::Struct(s) => {
                self.declare(&s.ident);
                visit::visit_item_struct(self, s);
            }
            syn::Item::Enum(e) => {
                self.declare(&e.ident);
                visit::visit_item_enum(self, e);
            }
            _ => visit::visit_item(self, item),
        }
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        match item {
            syn::ImplItem::Fn(f) if !is_test_only(&f.attrs) => {
                let at = self.push_fn(&f.sig, &f.block);
                self.func.push(at);
                self.scopes
                    .push((format!("{}::fn {}#{at}", self.scope(), f.sig.ident), false));
                visit::visit_impl_item_fn(self, f);
                self.scopes.pop();
                self.func.pop();
            }
            syn::ImplItem::Fn(_) => {}
            syn::ImplItem::Const(c) if is_test_only(&c.attrs) => {}
            _ => visit::visit_impl_item(self, item),
        }
    }

    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        if let syn::TraitItem::Fn(f) = item {
            if is_test_only(&f.attrs) {
                return;
            }
        }
        visit::visit_trait_item(self, item);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if !stmt_test_only(stmt) {
            visit::visit_stmt(self, stmt);
        }
    }

    fn visit_arm(&mut self, arm: &'ast Arm) {
        if is_test_only(&arm.attrs) {
            return;
        }
        let missing = guard_names_missing(arm);
        self.missing_arm += usize::from(missing);
        visit::visit_arm(self, arm);
        self.missing_arm -= usize::from(missing);
    }

    fn visit_field_value(&mut self, field: &'ast syn::FieldValue) {
        if !is_test_only(&field.attrs) {
            visit::visit_field_value(self, field);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if is_test_only(expr_attrs(expr)) {
            return;
        }
        match expr {
            Expr::MethodCall(call) if call.method.unraw() == MUTATOR => {
                self.mutator_ref(None, "method call", false);
            }
            Expr::Path(path) if last_ident(&path.path).as_deref() == Some(MUTATOR) => {
                let owner = match &path.qself {
                    Some(q) => type_ident(&q.ty),
                    None => {
                        let segs = &path.path.segments;
                        (segs.len() >= 2).then(|| segs[segs.len() - 2].ident.unraw().to_string())
                    }
                };
                let qualified = path.qself.is_none() && path.path.segments.len() > 2;
                self.mutator_ref(owner, "path", qualified);
            }
            _ => {}
        }
        visit::visit_expr(self, expr);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.record_log(mac);
        self.scan_tokens(mac);
        visit::visit_macro(self, mac);
    }
}
