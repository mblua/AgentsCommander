//! The two mailbox contracts, checked over the production portions of the tree.
//!
//! T10: no production code acquires `CoordinatorClocks::mark_auto_closed`, in any form.
//! Logs: the `[context-alert]`/`[remote-activity]` lines keep their roles, format and
//! argument origins, and every successful internal notice delivery emits its INFO once.

use std::collections::{BTreeMap, BTreeSet};

use syn::ext::IdentExt;
use syn::parse::{ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Arm, Block, Expr, Pat, Stmt};

use crate::sources::{
    expr_attrs, first_literal, inventory, is_test_only, last_ident, stmt_test_only, toks,
    type_ident, FnInfo, Index, LogSite, Tok, Tree, CONTEXT, MUTATOR, REMOTE,
};

const OWNER: &str = "CoordinatorClocks";
const INJECT: &str = "inject_internal_notice_into_supported_agent_session";
const DELIVER: &str = "inject_internal_system_notice";
const NOTICE: &str = "InternalSystemNotice";
pub const CONTEXT_INFO: &str = "[context-alert] delivered coordinatorSession={} project={} \
    workgroup={} member={} observed={} thresholds={:?}";
const VARIANTS: [&str; 2] = ["ContextAlert", "RemoteActivity"];
const FIELDS: [&str; 4] = ["workgroup", "member", "observed", "thresholds"];
const MAX_DEPTH: usize = 8;

/// Counts the log contract identified, for the acceptance controls.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub context_info: usize,
    pub context_warn: usize,
    pub remote_info: usize,
    pub delivered_variants: usize,
}

/// Runs both contracts once over the shared inventory and AST of `tree`.
pub fn verify(tree: &Tree) -> Result<Report, Vec<String>> {
    let mut index = inventory(tree);
    let mut errors = std::mem::take(&mut index.errors);
    errors.extend(close_marker_errors(&index));
    let report = log_contract(&index, &mut errors);
    if errors.is_empty() {
        Ok(report)
    } else {
        Err(errors)
    }
}

// ------------------------------------------------------------------ T10

fn resolve_owner(index: &Index, name: &str) -> Option<String> {
    let mut current = name.to_string();
    for _ in 0..MAX_DEPTH {
        if current == OWNER {
            return Some(current);
        }
        match index.aliases.get(&current) {
            Some(next) => current = next.clone(),
            None => return index.declared.contains(&current).then_some(current),
        }
    }
    None
}

fn close_marker_errors(index: &Index) -> Vec<String> {
    let mut errors = Vec::new();
    for r in &index.refs {
        let resolved = r.owner.as_deref().and_then(|o| resolve_owner(index, o));
        match (r.form, resolved.as_deref()) {
            ("method call" | "method call in macro", _) | (_, Some(OWNER)) => errors.push(format!(
                "T10 close-marker restore: {OWNER}::{MUTATOR} acquired by {} at {}",
                r.form, r.site
            )),
            (_, Some(_)) => {}
            (_, None) => errors.push(format!(
                "T10 close-marker restore: unresolved owner {:?} of {MUTATOR} ({}) at {}",
                r.owner, r.form, r.site
            )),
        }
    }
    errors
}

// ------------------------------------------------------------------ logs

fn log_contract(index: &Index, errors: &mut Vec<String>) -> Report {
    let count = |prefix: &str, level: &str| {
        index
            .logs
            .iter()
            .filter(|l| l.value.starts_with(prefix) && l.level == level)
            .count()
    };
    let report = Report {
        context_info: count(CONTEXT, "info"),
        context_warn: count(CONTEXT, "warn"),
        remote_info: count(REMOTE, "info"),
        delivered_variants: 0,
    };
    let all = |prefix: &str| {
        index
            .logs
            .iter()
            .filter(|l| l.value.starts_with(prefix))
            .count()
    };
    let files = |prefix: &str| {
        let set: BTreeSet<&str> = index
            .logs
            .iter()
            .filter(|l| l.value.starts_with(prefix))
            .map(|l| l.file.as_str())
            .collect();
        set.into_iter().collect::<Vec<_>>().join(",")
    };
    if all(CONTEXT) != 2 || report.context_info != 1 || report.context_warn != 1 {
        errors.push(format!(
            "log roles: want one warn and one info under {CONTEXT:?}, found {} logs \
             ({} info, {} warn) in [{}]",
            all(CONTEXT),
            report.context_info,
            report.context_warn,
            files(CONTEXT)
        ));
    }
    if all(REMOTE) != 1 || report.remote_info != 1 {
        errors.push(format!(
            "log roles: want one info under {REMOTE:?}, found {} in [{}]",
            all(REMOTE),
            files(REMOTE)
        ));
    }
    for warn in index
        .logs
        .iter()
        .filter(|l| l.value.starts_with(CONTEXT) && l.level == "warn")
    {
        if !warn.missing_arm {
            errors.push(format!(
                "log roles: {CONTEXT:?} warn in {} is outside the err_is_pty_session_missing arm",
                warn.file
            ));
        }
    }
    let infos: Vec<&LogSite> = index
        .logs
        .iter()
        .filter(|l| l.value.starts_with(CONTEXT) && l.level == "info")
        .collect();
    let exact: Vec<&&LogSite> = infos.iter().filter(|l| l.value == CONTEXT_INFO).collect();
    if exact.len() != 1 {
        errors.push(format!(
            "log format: want exactly one {CONTEXT:?} info equal to {CONTEXT_INFO:?}, \
             found {} in [{}]",
            exact.len(),
            files(CONTEXT)
        ));
    } else {
        context_arguments(index, exact[0], errors);
    }
    let delivered = delivery_contract(index, errors);
    Report {
        delivered_variants: delivered,
        ..report
    }
}

// ------------------------------------------------------------------ argument origins

#[derive(Clone, Debug, PartialEq, Eq)]
enum Origin {
    Param(Option<String>),
    Field(String),
    Project,
    Other,
}

type Env = Vec<BTreeMap<String, Origin>>;

fn lookup(env: &Env, name: &str) -> Origin {
    env.iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
        .unwrap_or(Origin::Other)
}

fn origin_of(env: &Env, expr: &Expr) -> Origin {
    match expr {
        Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 => {
            lookup(env, &p.path.segments[0].ident.unraw().to_string())
        }
        Expr::Reference(r) => origin_of(env, &r.expr),
        Expr::Paren(p) => origin_of(env, &p.expr),
        Expr::Group(g) => origin_of(env, &g.expr),
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => origin_of(env, &u.expr),
        _ => Origin::Other,
    }
}

fn pat_ident(pat: &Pat) -> Option<String> {
    match pat {
        Pat::Ident(i) => Some(i.ident.unraw().to_string()),
        Pat::Reference(r) => pat_ident(&r.pat),
        Pat::Type(t) => pat_ident(&t.pat),
        _ => None,
    }
}

fn params(sig: &syn::Signature) -> Vec<(String, Option<String>)> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(t) => pat_ident(&t.pat).map(|n| (n, type_ident(&t.ty))),
            syn::FnArg::Receiver(_) => None,
        })
        .collect()
}

fn variant_of(pat: &Pat) -> Option<String> {
    let path = match pat {
        Pat::Struct(s) => &s.path,
        Pat::TupleStruct(t) => &t.path,
        Pat::Path(p) => &p.path,
        Pat::Reference(r) => return variant_of(&r.pat),
        _ => return None,
    };
    last_ident(path)
}

fn bind_fields(pat: &Pat, scope: &mut BTreeMap<String, Origin>) {
    if let Pat::Struct(s) = pat {
        for field in &s.fields {
            let syn::Member::Named(member) = &field.member else {
                continue;
            };
            if let Some(local) = pat_ident(&field.pat) {
                scope.insert(local, Origin::Field(member.unraw().to_string()));
            }
        }
    } else if let Pat::Reference(r) = pat {
        bind_fields(&r.pat, scope);
    }
}

fn bind_local(env: &mut Env, local: &syn::Local) {
    let Some(init) = &local.init else { return };
    let scope = env.last_mut().map(std::mem::take).unwrap_or_default();
    let mut scope = scope;
    match &local.pat {
        Pat::Tuple(t) if is_project_split(&scope_env(env, &scope), &init.expr) => {
            if let Some(first) = t.elems.first().and_then(pat_ident) {
                scope.insert(first, Origin::Project);
            }
        }
        pat => {
            if let Some(name) = pat_ident(pat) {
                let origin = origin_of(&scope_env(env, &scope), &init.expr);
                scope.insert(name, origin);
            }
        }
    }
    if let Some(last) = env.last_mut() {
        *last = scope;
    }
}

fn scope_env(env: &Env, scope: &BTreeMap<String, Origin>) -> Env {
    let mut all = env.clone();
    all.push(scope.clone());
    all
}

fn is_project_split(env: &Env, expr: &Expr) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Expr::Path(func) = &*call.func else {
        return false;
    };
    if last_ident(&func.path).as_deref() != Some("split_project_prefix") || call.args.len() != 1 {
        return false;
    }
    let Some(Expr::MethodCall(fqn)) = call.args.first() else {
        return false;
    };
    fqn.method == "fqn"
        && fqn.args.is_empty()
        && origin_of(env, &fqn.receiver) == Origin::Param(Some("InternalSystemTarget".into()))
}

fn context_arguments(index: &Index, site: &LogSite, errors: &mut Vec<String>) {
    let Some(func) = site.func.map(|i| &index.fns[i]) else {
        return errors.push(format!(
            "log arguments: {CONTEXT:?} info outside a function"
        ));
    };
    let mut env: Env = vec![params(&func.sig)
        .into_iter()
        .map(|(name, ty)| (name, Origin::Param(ty)))
        .collect()];
    let mut found = None;
    find_in_block(&func.block, &mut env, &site.mac, &mut found);
    let Some((env, args)) = found else {
        return errors.push(format!(
            "log arguments: unresolved {CONTEXT:?} info in {}::{}",
            func.file, func.name
        ));
    };
    if args.len() != 6 {
        return errors.push(format!(
            "log arguments: want 6 arguments, found {} in {}::{}",
            args.len(),
            func.file,
            func.name
        ));
    }
    let mut want = vec![Origin::Param(Some("Uuid".into())), Origin::Project];
    want.extend(FIELDS.iter().map(|f| Origin::Field((*f).into())));
    for (at, (arg, want)) in args.iter().zip(&want).enumerate() {
        let got = match (at, arg) {
            (1, Expr::MethodCall(m)) if m.method == "unwrap_or" && is_empty_str(&m.args) => {
                origin_of(&env, &m.receiver)
            }
            (1, _) => Origin::Other,
            _ => origin_of(&env, arg),
        };
        if &got != want {
            errors.push(format!(
                "log arguments: argument {} of the {CONTEXT:?} info comes from {got:?}, \
                 want {want:?}, in {}::{}",
                at + 1,
                func.file,
                func.name
            ));
        }
    }
}

fn is_empty_str(args: &Punctuated<Expr, syn::Token![,]>) -> bool {
    matches!(args.first(), Some(Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }))
        if s.value().is_empty() && args.len() == 1)
}

type Found = Option<(Env, Vec<Expr>)>;

fn find_in_block(block: &Block, env: &mut Env, target: &syn::Macro, found: &mut Found) {
    env.push(BTreeMap::new());
    for stmt in &block.stmts {
        if stmt_test_only(stmt) {
            continue;
        }
        match stmt {
            Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    find_in_expr(&init.expr, env, target, found);
                }
                bind_local(env, local);
            }
            Stmt::Expr(expr, _) => find_in_expr(expr, env, target, found),
            Stmt::Macro(mac) => check_macro(&mac.mac, env, target, found),
            Stmt::Item(_) => {}
        }
    }
    env.pop();
}

fn check_macro(mac: &syn::Macro, env: &Env, target: &syn::Macro, found: &mut Found) {
    if found.is_none() && mac.tokens.to_string() == target.tokens.to_string() {
        let parser = Punctuated::<Expr, syn::Token![,]>::parse_terminated;
        if let Ok(args) = mac.parse_body_with(parser) {
            *found = Some((env.clone(), args.into_iter().skip(1).collect()));
        }
    }
}

fn find_in_expr(expr: &Expr, env: &mut Env, target: &syn::Macro, found: &mut Found) {
    if is_test_only(expr_attrs(expr)) {
        return;
    }
    match expr {
        Expr::Block(b) => find_in_block(&b.block, env, target, found),
        Expr::Unsafe(b) => find_in_block(&b.block, env, target, found),
        Expr::Macro(m) => check_macro(&m.mac, env, target, found),
        Expr::If(i) => {
            find_in_block(&i.then_branch, env, target, found);
            if let Some((_, other)) = &i.else_branch {
                find_in_expr(other, env, target, found);
            }
        }
        Expr::Match(m) => {
            for arm in m.arms.iter().filter(|a| !is_test_only(&a.attrs)) {
                let mut scope = BTreeMap::new();
                bind_fields(&arm.pat, &mut scope);
                env.push(scope);
                find_in_expr(&arm.body, env, target, found);
                env.pop();
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ delivery paths

#[derive(Clone, Copy, Debug, Default)]
struct State {
    injected: bool,
    own: u32,
    other: u32,
}

enum Flow {
    Next(State),
    Exit(State, bool),
}

struct Paths<'a> {
    index: &'a Index,
    variant: &'static str,
    relevant: BTreeSet<String>,
    errors: Vec<String>,
}

fn delivery_contract(index: &Index, errors: &mut Vec<String>) -> usize {
    let entries: Vec<&FnInfo> = index.fns.iter().filter(|f| f.name == DELIVER).collect();
    let [entry] = entries.as_slice() else {
        errors.push(format!(
            "delivery: want one production {DELIVER}, found {}",
            entries.len()
        ));
        return 0;
    };
    let mut relevant: BTreeSet<String> = index
        .logs
        .iter()
        .filter(|l| l.level == "info")
        .filter_map(|l| l.func.map(|i| index.fns[i].name.clone()))
        .collect();
    relevant.insert(INJECT.to_string());
    loop {
        let before = relevant.len();
        for f in &index.fns {
            if !relevant.contains(&f.name) && block_mentions(&f.block, &relevant) {
                relevant.insert(f.name.clone());
            }
        }
        if relevant.len() == before {
            break;
        }
    }
    let mut delivered = 0;
    for variant in VARIANTS {
        let mut paths = Paths {
            index,
            variant,
            relevant: relevant.clone(),
            errors: Vec::new(),
        };
        let notice = notice_names(&entry.sig);
        let flows = paths.call(entry, &notice, State::default(), 0);
        let ok: Vec<State> = flows
            .into_iter()
            .filter_map(|f| match f {
                Flow::Exit(s, true) => Some(s),
                _ => None,
            })
            .collect();
        if ok.is_empty() {
            paths.errors.push(format!(
                "delivery {variant}: no successful path in {}::{DELIVER}",
                entry.file
            ));
        }
        for state in &ok {
            if !state.injected || state.own != 1 || state.other != 0 {
                paths.errors.push(format!(
                    "delivery {variant}: a successful path of {}::{DELIVER} has injected={} \
                     own INFO={} other INFO={}, want true/1/0",
                    entry.file, state.injected, state.own, state.other
                ));
            }
        }
        if paths.errors.is_empty() {
            delivered += 1;
        }
        errors.extend(paths.errors);
    }
    delivered
}

fn block_mentions(block: &Block, names: &BTreeSet<String>) -> bool {
    struct Mentions<'a>(&'a BTreeSet<String>, bool);
    impl Visit<'_> for Mentions<'_> {
        fn visit_ident(&mut self, ident: &syn::Ident) {
            self.1 |= self.0.contains(&ident.unraw().to_string());
        }
        fn visit_macro(&mut self, mac: &syn::Macro) {
            self.1 |= tokens_mention(mac, self.0);
            visit::visit_macro(self, mac);
        }
    }
    let mut seen = Mentions(names, false);
    seen.visit_block(block);
    seen.1
}

fn notice_names(sig: &syn::Signature) -> BTreeSet<String> {
    params(sig)
        .into_iter()
        .filter(|(_, ty)| ty.as_deref() == Some(NOTICE))
        .map(|(name, _)| name)
        .collect()
}

fn is_ok_unit(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else { return false };
    matches!(&*call.func, Expr::Path(p) if p.path.is_ident("Ok"))
}

fn is_notice(notice: &BTreeSet<String>, expr: &Expr) -> bool {
    match expr {
        Expr::Path(p) => p
            .path
            .get_ident()
            .is_some_and(|i| notice.contains(&i.to_string())),
        Expr::Reference(r) => is_notice(notice, &r.expr),
        Expr::Paren(p) => is_notice(notice, &p.expr),
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => is_notice(notice, &u.expr),
        _ => false,
    }
}

fn tokens_mention(mac: &syn::Macro, names: &BTreeSet<String>) -> bool {
    toks(mac)
        .iter()
        .any(|t| matches!(t, Tok::Ident(i) if names.contains(i)))
}

impl Paths<'_> {
    fn fail(&mut self, func: &FnInfo, what: &str) {
        self.errors.push(format!(
            "delivery {}: {what} in {}::{}",
            self.variant, func.file, func.name
        ));
    }

    fn call(
        &mut self,
        func: &FnInfo,
        notice: &BTreeSet<String>,
        start: State,
        depth: usize,
    ) -> Vec<Flow> {
        if depth > MAX_DEPTH {
            self.fail(func, "call depth exceeded");
            return Vec::new();
        }
        self.block(
            func,
            &func.block,
            notice,
            vec![Flow::Next(start)],
            depth,
            true,
        )
    }

    fn block(
        &mut self,
        func: &FnInfo,
        block: &Block,
        notice: &BTreeSet<String>,
        flows: Vec<Flow>,
        depth: usize,
        body: bool,
    ) -> Vec<Flow> {
        let mut notice = notice.clone();
        let mut flows = flows;
        let stmts: Vec<&Stmt> = block.stmts.iter().filter(|s| !stmt_test_only(s)).collect();
        for (at, stmt) in stmts.iter().enumerate() {
            let tail = at + 1 == stmts.len();
            flows = match stmt {
                Stmt::Local(local) => {
                    let init = local.init.as_ref().map(|i| &*i.expr);
                    if let (Some(name), Some(init)) = (pat_ident(&local.pat), init) {
                        if is_notice(&notice, init) {
                            notice.insert(name);
                        }
                    }
                    match init {
                        Some(init) => self.expr(func, init, &notice, flows, depth),
                        None => flows,
                    }
                }
                Stmt::Expr(expr, semi) => {
                    let next = self.expr(func, expr, &notice, flows, depth);
                    if body && tail && semi.is_none() {
                        let ok = is_ok_unit(expr);
                        next.into_iter().map(|f| finish(f, ok)).collect()
                    } else {
                        next
                    }
                }
                Stmt::Macro(mac) => self.mac(func, &mac.mac, flows),
                Stmt::Item(_) => flows,
            };
        }
        flows
    }

    fn each(flows: Vec<Flow>, mut f: impl FnMut(State) -> Vec<Flow>) -> Vec<Flow> {
        flows
            .into_iter()
            .flat_map(|flow| match flow {
                Flow::Next(s) => f(s),
                exit @ Flow::Exit(..) => vec![exit],
            })
            .collect()
    }

    fn mac(&mut self, func: &FnInfo, mac: &syn::Macro, flows: Vec<Flow>) -> Vec<Flow> {
        let level = last_ident(&mac.path).unwrap_or_default();
        let value = first_literal(mac).unwrap_or_default();
        let own_prefix = if self.variant == VARIANTS[0] {
            CONTEXT
        } else {
            REMOTE
        };
        let kind = match (level.as_str(), value.as_str()) {
            ("info", v) if v.starts_with(CONTEXT) || v.starts_with(REMOTE) => {
                Some(v.starts_with(own_prefix))
            }
            _ => None,
        };
        if kind.is_none() && tokens_mention(mac, &self.relevant) {
            self.fail(func, "macro hides a relevant call");
        }
        let mut before = false;
        let flows = Self::each(flows, |mut s| {
            match kind {
                Some(true) => s.own += 1,
                Some(false) => s.other += 1,
                None => {}
            }
            before |= kind.is_some() && !s.injected;
            vec![Flow::Next(s)]
        });
        if before {
            self.fail(func, "INFO emitted before the injection");
        }
        flows
    }

    fn decide(&self, notice: &BTreeSet<String>, cond: &Expr) -> Option<bool> {
        match cond {
            Expr::Let(l) if is_notice(notice, &l.expr) => Some(self.matches(&l.pat)),
            Expr::Unary(u) if matches!(u.op, syn::UnOp::Not(_)) => {
                self.decide(notice, &u.expr).map(|b| !b)
            }
            Expr::Paren(p) => self.decide(notice, &p.expr),
            Expr::Macro(m) if m.mac.path.is_ident("matches") => {
                let parser = |input: ParseStream| {
                    let scrutinee: Expr = input.parse()?;
                    input.parse::<syn::Token![,]>()?;
                    let pat = Pat::parse_multi_with_leading_vert(input)?;
                    input.step(|cursor| {
                        let mut rest = *cursor;
                        while let Some((_, next)) = rest.token_tree() {
                            rest = next;
                        }
                        Ok(((), rest))
                    })?;
                    Ok((scrutinee, pat))
                };
                let (scrutinee, pat) = parser.parse2(m.mac.tokens.clone()).ok()?;
                is_notice(notice, &scrutinee).then(|| self.matches(&pat))
            }
            _ => None,
        }
    }

    fn matches(&self, pat: &Pat) -> bool {
        match pat {
            Pat::Wild(_) | Pat::Ident(_) => true,
            Pat::Or(o) => o.cases.iter().any(|c| self.matches(c)),
            other => variant_of(other).as_deref() == Some(self.variant),
        }
    }

    fn expr(
        &mut self,
        func: &FnInfo,
        expr: &Expr,
        notice: &BTreeSet<String>,
        flows: Vec<Flow>,
        depth: usize,
    ) -> Vec<Flow> {
        if is_test_only(expr_attrs(expr)) {
            return flows;
        }
        match expr {
            Expr::Block(b) => self.block(func, &b.block, notice, flows, depth, false),
            Expr::Unsafe(b) => self.block(func, &b.block, notice, flows, depth, false),
            Expr::Paren(p) => self.expr(func, &p.expr, notice, flows, depth),
            Expr::Reference(r) => self.expr(func, &r.expr, notice, flows, depth),
            Expr::Await(a) => self.expr(func, &a.base, notice, flows, depth),
            Expr::Try(t) => {
                let next = self.expr(func, &t.expr, notice, flows, depth);
                Self::each(next, |s| vec![Flow::Next(s), Flow::Exit(s, false)])
            }
            Expr::Return(r) => {
                let ok = r.expr.as_deref().is_some_and(is_ok_unit);
                let next = match &r.expr {
                    Some(e) => self.expr(func, e, notice, flows, depth),
                    None => flows,
                };
                Self::each(next, |s| vec![Flow::Exit(s, ok)])
            }
            Expr::Macro(m) => self.mac(func, &m.mac, flows),
            Expr::If(i) => self.branch_if(func, i, notice, flows, depth),
            Expr::Match(m) => self.branch_match(func, m, notice, flows, depth),
            Expr::Call(c) => {
                let mut flows = flows;
                for arg in &c.args {
                    flows = self.expr(func, arg, notice, flows, depth);
                }
                let Expr::Path(p) = &*c.func else {
                    return self.opaque(func, &c.func, flows);
                };
                let name = last_ident(&p.path).unwrap_or_default();
                let args: Vec<&Expr> = c.args.iter().collect();
                self.invoke(func, &name, None, &args, notice, flows, depth)
            }
            Expr::MethodCall(m) => {
                let mut flows = self.expr(func, &m.receiver, notice, flows, depth);
                for arg in &m.args {
                    flows = self.expr(func, arg, notice, flows, depth);
                }
                let args: Vec<&Expr> = m.args.iter().collect();
                let name = m.method.unraw().to_string();
                let receiver = Some(&*m.receiver);
                self.invoke(func, &name, receiver, &args, notice, flows, depth)
            }
            Expr::Closure(_) | Expr::Async(_) | Expr::Lit(_) | Expr::Path(_) => flows,
            other => self.opaque(func, other, flows),
        }
    }

    fn opaque(&mut self, func: &FnInfo, expr: &Expr, flows: Vec<Flow>) -> Vec<Flow> {
        struct Mentions<'a>(&'a BTreeSet<String>, bool);
        impl Visit<'_> for Mentions<'_> {
            fn visit_ident(&mut self, ident: &syn::Ident) {
                self.1 |= self.0.contains(&ident.unraw().to_string());
            }
            fn visit_macro(&mut self, mac: &syn::Macro) {
                self.1 |= tokens_mention(mac, self.0)
                    || first_literal(mac)
                        .is_some_and(|v| v.starts_with(CONTEXT) || v.starts_with(REMOTE));
                visit::visit_macro(self, mac);
            }
            fn visit_expr_return(&mut self, _: &syn::ExprReturn) {
                self.1 = true;
            }
            fn visit_expr_try(&mut self, _: &syn::ExprTry) {
                self.1 = true;
            }
        }
        let mut seen = Mentions(&self.relevant, false);
        seen.visit_expr(expr);
        if seen.1 {
            self.fail(func, "unsupported construct on the delivery path");
        }
        flows
    }

    fn branch_if(
        &mut self,
        func: &FnInfo,
        i: &syn::ExprIf,
        notice: &BTreeSet<String>,
        flows: Vec<Flow>,
        depth: usize,
    ) -> Vec<Flow> {
        let decided = self.decide(notice, &i.cond);
        let flows = match &*i.cond {
            Expr::Let(l) => self.expr(func, &l.expr, notice, flows, depth),
            cond => self.expr(func, cond, notice, flows, depth),
        };
        let mut out = Vec::new();
        let (then, other): (Vec<Flow>, Vec<Flow>) =
            flows
                .into_iter()
                .fold((Vec::new(), Vec::new()), |(mut a, mut b), f| {
                    match f {
                        Flow::Next(s) => {
                            if decided != Some(false) {
                                a.push(Flow::Next(s));
                            }
                            if decided != Some(true) {
                                b.push(Flow::Next(s));
                            }
                        }
                        exit @ Flow::Exit(..) => out.push(exit),
                    }
                    (a, b)
                });
        out.extend(self.block(func, &i.then_branch, notice, then, depth, false));
        match &i.else_branch {
            Some((_, e)) => out.extend(self.expr(func, e, notice, other, depth)),
            None => out.extend(other),
        }
        out
    }

    fn branch_match(
        &mut self,
        func: &FnInfo,
        m: &syn::ExprMatch,
        notice: &BTreeSet<String>,
        flows: Vec<Flow>,
        depth: usize,
    ) -> Vec<Flow> {
        let flows = self.expr(func, &m.expr, notice, flows, depth);
        let on_notice = is_notice(notice, &m.expr);
        let arms: Vec<&Arm> = m.arms.iter().filter(|a| !is_test_only(&a.attrs)).collect();
        let mut chosen = Vec::new();
        for arm in arms {
            if on_notice && !self.matches(&arm.pat) {
                continue;
            }
            chosen.push(arm);
            if on_notice && arm.guard.is_none() {
                break;
            }
        }
        let mut out = Vec::new();
        let mut live = Vec::new();
        for flow in flows {
            match flow {
                Flow::Next(s) => live.push(s),
                exit @ Flow::Exit(..) => out.push(exit),
            }
        }
        for arm in chosen {
            let start = live.iter().map(|s| Flow::Next(*s)).collect();
            out.extend(self.expr(func, &arm.body, notice, start, depth));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke(
        &mut self,
        func: &FnInfo,
        name: &str,
        receiver: Option<&Expr>,
        args: &[&Expr],
        notice: &BTreeSet<String>,
        flows: Vec<Flow>,
        depth: usize,
    ) -> Vec<Flow> {
        if name == INJECT {
            return Self::each(flows, |mut s| {
                s.injected = true;
                vec![Flow::Next(s)]
            });
        }
        if !self.relevant.contains(name) {
            return flows;
        }
        let self_call =
            receiver.is_some_and(|r| matches!(r, Expr::Path(p) if p.path.is_ident("self")));
        let candidates: Vec<&FnInfo> = self.index.fns.iter().filter(|f| f.name == name).collect();
        let target = match candidates.as_slice() {
            [] => return flows,
            [one] => *one,
            many => match many
                .iter()
                .find(|f| f.owner == func.owner && f.file == func.file)
            {
                Some(found) if receiver.is_none() || self_call => *found,
                _ => {
                    if self.relevant.contains(name) {
                        self.fail(func, &format!("ambiguous call to {name}"));
                    }
                    return flows;
                }
            },
        };
        if receiver.is_some() && !self_call && target.sig.receiver().is_none() {
            return flows;
        }
        let callee_params = params(&target.sig);
        let mut callee_notice = BTreeSet::new();
        for ((pname, _), arg) in callee_params.iter().zip(args) {
            if is_notice(notice, arg) {
                callee_notice.insert(pname.clone());
            }
        }
        let flows_in: Vec<State> = flows
            .iter()
            .filter_map(|f| match f {
                Flow::Next(s) => Some(*s),
                Flow::Exit(..) => None,
            })
            .collect();
        let mut out: Vec<Flow> = flows
            .into_iter()
            .filter(|f| matches!(f, Flow::Exit(..)))
            .collect();
        for state in flows_in {
            for flow in self.call(target, &callee_notice, state, depth + 1) {
                let (Flow::Next(inner) | Flow::Exit(inner, _)) = flow;
                out.push(Flow::Next(inner));
            }
        }
        out
    }
}

fn finish(flow: Flow, ok: bool) -> Flow {
    match flow {
        Flow::Next(s) => Flow::Exit(s, ok),
        exit @ Flow::Exit(..) => exit,
    }
}
