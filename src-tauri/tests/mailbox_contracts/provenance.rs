//! Where the names the contracts check come from.
//!
//! Owner types resolve in the scopes visible at a reference before any homonym of
//! another module. The parameters of the function holding the `[context-alert]`
//! INFO are traced, through the one real call, to the delivery entry's session
//! (the one injected into), target and notice.

use std::collections::{BTreeMap, BTreeSet};

use syn::ext::IdentExt;
use syn::visit::{self, Visit};
use syn::{Block, Expr, Stmt};

use crate::contracts::{origin_of, params, Env, Origin, DELIVER, INJECT, MAX_DEPTH, NOTICE, OWNER};
use crate::sources::{
    expr_attrs, is_test_only, last_ident, stmt_test_only, type_ident, FnInfo, Index,
};

/// What a type name used at a reference denotes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    Clocks,
    Other,
    Unknown,
    Ambiguous,
}

/// Resolves `name` in the scopes visible at the reference first (innermost first,
/// stopping at its module), so a homonym declared in another module cannot change it.
/// A name reached through a module path skips the local scopes: every definition in
/// the tree must then agree.
pub fn resolve_owner(index: &Index, chain: &[String], name: &str, qualified: bool) -> Owner {
    for scope in chain.iter().filter(|_| !qualified) {
        let key = (scope.clone(), name.to_string());
        if let Some(target) = index.aliases.get(&key) {
            return resolve_global(index, target, 0);
        }
        if index.declared.contains(&key) {
            return if name == OWNER {
                Owner::Clocks
            } else {
                Owner::Other
            };
        }
    }
    resolve_global(index, name, 0)
}

/// A name imported from elsewhere: every definition of it in the tree must agree.
fn resolve_global(index: &Index, name: &str, depth: usize) -> Owner {
    if name == OWNER {
        return Owner::Clocks;
    }
    if depth > MAX_DEPTH {
        return Owner::Unknown;
    }
    let mut seen: BTreeSet<Owner> = index
        .aliases
        .iter()
        .filter(|((_, n), target)| n == name && *target != name)
        .map(|(_, target)| resolve_global(index, target, depth + 1))
        .collect();
    if index.declared.iter().any(|(_, n)| n == name) {
        seen.insert(Owner::Other);
    }
    match seen.len() {
        0 => Owner::Unknown,
        1 => seen.into_iter().next().unwrap_or(Owner::Unknown),
        _ => Owner::Ambiguous,
    }
}

/// The delivery entry's parameters that the INFO must use, named by role: the
/// session injected into, the target and the notice.
pub const SESSION_PARAM: &str = "<session>";
pub const TARGET_PARAM: &str = "<target>";
pub const NOTICE_PARAM: &str = "<notice>";

/// Origins of `func`'s parameters, traced to the entry through the one real call.
pub fn entry_env(index: &Index, func: &FnInfo) -> Result<BTreeMap<String, Origin>, String> {
    let entries: Vec<&FnInfo> = index.fns.iter().filter(|f| f.name == DELIVER).collect();
    let [entry] = entries.as_slice() else {
        return Err(format!(
            "want one production {DELIVER}, found {}",
            entries.len()
        ));
    };
    let roles = entry_roles(entry)?;
    let entry_env: Env = vec![params(&entry.sig)
        .into_iter()
        .map(|(name, _)| {
            let origin = roles
                .get(&name)
                .map_or(Origin::Other, |r| Origin::Entry((*r).to_string()));
            (name, origin)
        })
        .collect()];
    if std::ptr::eq(*entry, func) {
        return Ok(entry_env.into_iter().next().unwrap_or_default());
    }
    let mut calls = Vec::new();
    for caller in &index.fns {
        for args in calls_to(index, caller, func) {
            calls.push((caller, args));
        }
    }
    let [(caller, args)] = calls.as_slice() else {
        return Err(format!(
            "want one production call to {}::{}, found {}",
            func.file,
            func.name,
            calls.len()
        ));
    };
    if !std::ptr::eq(*caller, *entry) {
        return Err(format!(
            "{}::{} is called from {}::{}, not from {DELIVER}",
            func.file, func.name, caller.file, caller.name
        ));
    }
    let entry_params: BTreeSet<String> = params(&entry.sig).into_iter().map(|(n, _)| n).collect();
    let rebound: BTreeSet<String> = bound_names(&entry.block)
        .intersection(&entry_params)
        .cloned()
        .collect();
    for arg in args.iter() {
        if let Some(name) = used_names(arg).into_iter().find(|n| rebound.contains(n)) {
            return Err(format!(
                "{DELIVER} rebinds {name} before the call to {}::{}",
                func.file, func.name
            ));
        }
    }
    let names = params(&func.sig);
    if names.len() != args.len() {
        return Err(format!(
            "call to {}::{} has the wrong arity",
            func.file, func.name
        ));
    }
    Ok(names
        .into_iter()
        .zip(args)
        .map(|((name, _), arg)| (name, origin_of(&entry_env, arg)))
        .collect())
}

/// Maps the entry's session, target and notice parameters to their role. The
/// session is the one passed to the real injection.
fn entry_roles(entry: &FnInfo) -> Result<BTreeMap<String, &'static str>, String> {
    let typed = params(&entry.sig);
    let one = |ty: &str| {
        let hits: Vec<&String> = typed
            .iter()
            .filter(|(_, t)| t.as_deref() == Some(ty))
            .map(|(n, _)| n)
            .collect();
        match hits.as_slice() {
            [one] => Ok((*one).clone()),
            _ => Err(format!(
                "{DELIVER} wants one {ty} parameter, found {}",
                hits.len()
            )),
        }
    };
    let mut roles = BTreeMap::new();
    roles.insert(one("InternalSystemTarget")?, TARGET_PARAM);
    roles.insert(one(NOTICE)?, NOTICE_PARAM);
    let uuids: BTreeSet<String> = typed
        .iter()
        .filter(|(_, t)| t.as_deref() == Some("Uuid"))
        .map(|(n, _)| n.clone())
        .collect();
    let injected: BTreeSet<String> = calls_named(&entry.block, INJECT)
        .iter()
        .flatten()
        .filter_map(|arg| match arg {
            Expr::Path(p) => p.path.get_ident().map(|i| i.unraw().to_string()),
            _ => None,
        })
        .filter(|n| uuids.contains(n))
        .collect();
    let [session] = injected.iter().collect::<Vec<_>>()[..] else {
        return Err(format!(
            "{DELIVER} must pass exactly one Uuid parameter to {INJECT}, found {injected:?}"
        ));
    };
    roles.insert(session.clone(), SESSION_PARAM);
    Ok(roles)
}

/// Names bound by a pattern anywhere in the production code of `block`.
fn bound_names(block: &Block) -> BTreeSet<String> {
    struct Bound(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Bound {
        fn visit_stmt(&mut self, stmt: &'ast Stmt) {
            if !stmt_test_only(stmt) {
                visit::visit_stmt(self, stmt);
            }
        }
        fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
            self.0.insert(p.ident.unraw().to_string());
            visit::visit_pat_ident(self, p);
        }
    }
    let mut bound = Bound(BTreeSet::new());
    bound.visit_block(block);
    bound.0
}

/// Single-identifier paths read by `expr`.
fn used_names(expr: &Expr) -> BTreeSet<String> {
    struct Used(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Used {
        fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
            if let Some(i) = p.path.get_ident() {
                self.0.insert(i.unraw().to_string());
            }
        }
    }
    let mut used = Used(BTreeSet::new());
    used.visit_expr(expr);
    used.0
}

/// Argument lists of the production calls in `block` to a function named `name`.
fn calls_named(block: &Block, name: &str) -> Vec<Vec<Expr>> {
    struct Calls<'a>(&'a str, Vec<Vec<Expr>>);
    impl<'ast> Visit<'ast> for Calls<'_> {
        fn visit_stmt(&mut self, stmt: &'ast Stmt) {
            if !stmt_test_only(stmt) {
                visit::visit_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, expr: &'ast Expr) {
            if is_test_only(expr_attrs(expr)) {
                return;
            }
            match expr {
                Expr::Call(c)
                    if matches!(&*c.func, Expr::Path(p)
                    if last_ident(&p.path).as_deref() == Some(self.0)) =>
                {
                    self.1.push(c.args.iter().cloned().collect());
                }
                Expr::MethodCall(m) if m.method.unraw() == self.0 => {
                    self.1.push(m.args.iter().cloned().collect());
                }
                _ => {}
            }
            visit::visit_expr(self, expr);
        }
    }
    let mut calls = Calls(name, Vec::new());
    calls.visit_block(block);
    calls.1
}

/// Calls in `caller` that resolve to `callee`, honoring the path qualifier.
fn calls_to(index: &Index, caller: &FnInfo, callee: &FnInfo) -> Vec<Vec<Expr>> {
    struct Calls<'a>(&'a Index, &'a FnInfo, &'a FnInfo, Vec<Vec<Expr>>);
    impl<'ast> Visit<'ast> for Calls<'_> {
        fn visit_stmt(&mut self, stmt: &'ast Stmt) {
            if !stmt_test_only(stmt) {
                visit::visit_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, expr: &'ast Expr) {
            if is_test_only(expr_attrs(expr)) {
                return;
            }
            let (index, caller, callee) = (self.0, self.1, self.2);
            match expr {
                Expr::Call(c) => {
                    if let Expr::Path(p) = &*c.func {
                        if last_ident(&p.path).as_deref() == Some(callee.name.as_str())
                            && qualifier_reaches(index, caller, &p.path, None, callee)
                        {
                            self.3.push(c.args.iter().cloned().collect());
                        }
                    }
                }
                Expr::MethodCall(m)
                    if m.method.unraw() == callee.name
                        && qualifier_reaches(
                            index,
                            caller,
                            &syn::Path::from(m.method.clone()),
                            Some(&m.receiver),
                            callee,
                        ) =>
                {
                    self.3.push(m.args.iter().cloned().collect());
                }
                _ => {}
            }
            visit::visit_expr(self, expr);
        }
    }
    let mut calls = Calls(index, caller, callee, Vec::new());
    calls.visit_block(&caller.block);
    calls.3
}

/// Whether a call written as `path` (with `receiver` for a method call) in
/// `caller` names `callee`: `Self::`/`self.`/`Owner::` reach methods of that
/// owner, a bare name reaches a free function of the same file. Owner names are
/// resolved to their declaration at both ends; a name that does not resolve with
/// certainty reaches nothing, so the call is reported unresolved where it is made.
pub fn qualifier_reaches(
    index: &Index,
    caller: &FnInfo,
    path: &syn::Path,
    receiver: Option<&Expr>,
    callee: &FnInfo,
) -> bool {
    let segs = &path.segments;
    if receiver.is_none() && segs.len() > 2 {
        // `a::Owner::f` names a module this check does not resolve: not the callee.
        return false;
    }
    let qual = (segs.len() >= 2).then(|| segs[segs.len() - 2].ident.unraw().to_string());
    let owner = |f: &FnInfo| {
        f.owner
            .as_deref()
            .and_then(|o| declaration(index, &f.chain, o).ok())
    };
    let same_owner = || owner(callee).is_some() && owner(callee) == owner(caller);
    match (receiver, qual.as_deref()) {
        (Some(r), _) => {
            matches!(r, Expr::Path(p) if p.path.is_ident("self"))
                && callee.sig.receiver().is_some()
                && same_owner()
        }
        (None, Some("Self")) => same_owner(),
        (None, Some(q)) if q.starts_with(char::is_uppercase) => {
            owner(callee).is_some() && declaration(index, &caller.chain, q).ok() == owner(callee)
        }
        (None, _) => callee.owner.is_none() && callee.file == caller.file,
    }
}

/// The declaration `(scope, name)` a type name used under `chain` denotes,
/// following `use` paths through the indexed module tree.
pub fn declaration(
    index: &Index,
    chain: &[String],
    name: &str,
) -> Result<(String, String), String> {
    for scope in chain {
        if let Some(found) = declared_in(index, scope, name, 0)? {
            return Ok(found);
        }
    }
    Err(format!("{name} is neither declared nor imported by name"))
}

fn declared_in(
    index: &Index,
    scope: &str,
    name: &str,
    depth: usize,
) -> Result<Option<(String, String)>, String> {
    if depth > MAX_DEPTH {
        return Err(format!("{name}: import chain too deep"));
    }
    let key = (scope.to_string(), name.to_string());
    if index.declared.contains(&key) {
        return Ok(Some(key));
    }
    if let Some(path) = index.uses.get(&key) {
        let (last, module) = path.split_last().ok_or("empty use path")?;
        let target = module_scope(index, scope, module)?;
        return declared_in(index, &target, last, depth + 1)?
            .ok_or_else(|| format!("{} does not declare {last}", quote_segs(path)))
            .map(Some);
    }
    if index.aliases.contains_key(&key) {
        return Err(format!("{name} is a type alias in {scope}"));
    }
    // Names a scope binds itself win; glob imports answer only for the rest.
    let mut found = BTreeSet::new();
    for module in index.globs.get(scope).into_iter().flatten() {
        let target = module_scope(index, scope, module)?;
        found.extend(declared_in(index, &target, name, depth + 1)?);
    }
    match found.len() {
        0 | 1 => Ok(found.into_iter().next()),
        _ => Err(format!("{name} is glob-imported from disagreeing modules")),
    }
}

/// The scope key of the module `segs` names from `scope`.
fn module_scope(index: &Index, scope: &str, segs: &[String]) -> Result<String, String> {
    let here = index
        .scope_module
        .get(scope)
        .ok_or("scope without a module")?;
    let mut module = here.clone();
    for (i, seg) in segs.iter().enumerate() {
        match seg.as_str() {
            "crate" if i == 0 => module = vec!["crate".to_string()],
            "self" if i == 0 => {}
            "super" if module.len() > 1 => {
                module.pop();
            }
            _ => module.push(seg.clone()),
        }
    }
    index
        .modules
        .get(&module)
        .cloned()
        .ok_or_else(|| format!("module {} is outside the indexed tree", module.join("::")))
}

fn quote_segs(segs: &[String]) -> String {
    segs.join("::")
}

// ------------------------------------------------------------------ exits and paths

/// How an exit value of a `Result` function ends: `Some(true)` for `Ok(..)`,
/// `Some(false)` for `Err(..)`, `None` when the value cannot be classified.
pub fn exit_kind(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::Paren(p) => exit_kind(&p.expr),
        Expr::Group(g) => exit_kind(&g.expr),
        Expr::Call(call) => {
            let Expr::Path(p) = &*call.func else {
                return None;
            };
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            let tail: Vec<&str> = segs.iter().rev().take(2).map(String::as_str).collect();
            let result_variant = segs.len() == 1 || tail.get(1) == Some(&"Result");
            match tail.first() {
                Some(&"Ok") if result_variant => Some(true),
                Some(&"Err") if result_variant => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn returns_result(sig: &syn::Signature) -> bool {
    match &sig.output {
        syn::ReturnType::Type(_, ty) => type_ident(ty).as_deref() == Some("Result"),
        syn::ReturnType::Default => false,
    }
}

pub fn quote_path(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.unraw().to_string())
        .collect::<Vec<_>>()
        .join("::")
}
