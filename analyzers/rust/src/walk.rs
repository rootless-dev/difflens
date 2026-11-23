//! Walks every function body for its calls, closures and references, and
//! for the registrations of handlers to the frameworks the analyzer knows.
//!
//! Calls are resolved from syntax: paths through the module tree, `use`
//! imports, `Self`, `self`, `super` and `crate`; method calls through the
//! type of their receiver when syntax tells it (a parameter's or a `let`'s
//! type, a constructor, a field of a struct of the tree, a function's return
//! type, `?`, `unwrap`). A method of a type of the tree is a static call;
//! one on a trait object, an `impl Trait` or a generic parameter is dynamic,
//! to every implementation of the tree (each its own call), or to the trait
//! method itself when there is none; one on a receiver of unknown type is
//! dynamic to the methods of the tree with that name, unless the name is a
//! common one of the standard library or more than a few types have it.
//!
//! Kinds: a call is `always` unless it sits in an `if`/`match` branch, a
//! `while`/`for` body, the right side of `&&`/`||`, a `let … else` branch,
//! or after a `?` or a conditional `return`/`break` (a path that returns
//! early skips it, as an early return does in Go); `async` inside the
//! closure, async block or function handed to `spawn` (tokio, std::thread,
//! async-std…). Panics are ignored, as in Go.

use std::collections::{BTreeMap, HashMap};

use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Expr, ExprPath, Pat};

use crate::index::{FnDef, FnId, Index, ModId, Owner, Res, Ty, TyCtx, TypeKind};
use crate::protocol::{Call, EntryPoint, External};
use crate::source::{self, ident, is_type_like, segments, string_literal};

/// Methods of the standard library common enough that a call of one on a
/// receiver of unknown type says nothing about which type it is.
const COMMON_METHODS: &[&str] = &[
    "all",
    "and_then",
    "any",
    "as_bytes",
    "as_deref",
    "as_mut",
    "as_ptr",
    "as_ref",
    "as_slice",
    "as_str",
    "borrow",
    "borrow_mut",
    "build",
    "bytes",
    "chain",
    "chars",
    "clear",
    "clone",
    "cloned",
    "close",
    "cmp",
    "collect",
    "contains",
    "contains_key",
    "copied",
    "count",
    "dedup",
    "default",
    "deref",
    "drain",
    "drop",
    "ends_with",
    "entry",
    "enumerate",
    "eq",
    "err",
    "expect",
    "extend",
    "filter",
    "filter_map",
    "find",
    "first",
    "flat_map",
    "flatten",
    "flush",
    "fmt",
    "fold",
    "for_each",
    "from",
    "get",
    "get_mut",
    "hash",
    "insert",
    "into",
    "into_iter",
    "is_empty",
    "is_err",
    "is_none",
    "is_ok",
    "is_some",
    "iter",
    "iter_mut",
    "join",
    "keys",
    "last",
    "len",
    "lines",
    "lock",
    "map",
    "map_err",
    "max",
    "min",
    "new",
    "next",
    "ok",
    "ok_or",
    "ok_or_else",
    "or_default",
    "or_else",
    "or_insert",
    "or_insert_with",
    "parse",
    "partial_cmp",
    "poll",
    "pop",
    "position",
    "push",
    "push_str",
    "read",
    "recv",
    "remove",
    "replace",
    "reserve",
    "retain",
    "rev",
    "send",
    "skip",
    "sort",
    "sort_by",
    "sort_by_key",
    "split",
    "starts_with",
    "sum",
    "take",
    "then",
    "to_lowercase",
    "to_owned",
    "to_string",
    "to_uppercase",
    "to_vec",
    "trim",
    "truncate",
    "try_into",
    "unwrap",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "values",
    "values_mut",
    "with_capacity",
    "write",
    "write_all",
    "zip",
];

/// The most methods of the tree a call on a receiver of unknown type is
/// resolved to.
const MAX_GUESSES: usize = 4;

/// Functions that run the function, closure or future given to them later
/// or on another thread.
const SPAWNERS: &[&str] = &["spawn", "spawn_blocking", "spawn_local"];

const VERBS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "head", "options", "trace", "connect", "any",
];

/// A handler registered to an HTTP router.
#[derive(Debug, Clone)]
pub struct Route {
    /// The function the registration is written in.
    pub caller: FnId,
    pub handler: FnId,
    /// Index in `Graph::calls` of the registration, when there is one.
    pub call: Option<usize>,
    pub verb: String,
    pub path: String,
    pub file: String,
    pub line: u32,
}

/// What the walk found.
#[derive(Default)]
pub struct Graph<'a> {
    pub closures: Vec<FnDef<'a>>,
    pub calls: Vec<Call>,
    /// Functions used as values; the caller filters out those a call or a
    /// registration of the same site covers.
    pub references: Vec<Call>,
    pub externals: BTreeMap<String, External>,
    pub entries: HashMap<FnId, Vec<EntryPoint>>,
    pub routes: Vec<Route>,
    /// A function building a router that is nested under a prefix, with
    /// the function nesting it.
    pub nests: Vec<(FnId, String, FnId)>,
    closure_counts: HashMap<String, u32>,
}

impl<'a> Graph<'a> {
    pub fn add_entry(&mut self, function: FnId, entry: EntryPoint) {
        let list = self.entries.entry(function).or_default();
        if !list
            .iter()
            .any(|existing| existing.kind == entry.kind && existing.label == entry.label)
        {
            list.push(entry);
        }
    }
}

/// Walks every body of `index`.
pub fn walk_all<'a>(index: &Index<'a>) -> Graph<'a> {
    let mut graph = Graph::default();
    for function in 0..index.fns.len() {
        let definition = &index.fns[function];
        let Some(body) = definition.body else {
            continue;
        };
        let (ctx, self_res) = index.context_of(function);
        let mut walker = Walker::new(index, &mut graph, Some(function), definition, ctx, self_res);
        if let Some(sig) = definition.sig {
            walker.bind_inputs(sig);
        }
        walker.visit_block(body);
    }
    for value in &index.statics {
        let module = &index.modules[value.module];
        let parent_id = format!("{}::{}", module.path, value.name);
        let mut walker = Walker {
            index,
            graph: &mut graph,
            caller: None,
            parent_id,
            parent_name: value.name.clone(),
            scope: value.module,
            krate: module.krate,
            file: module.file,
            test: value.test || module.test,
            ctx: TyCtx::default(),
            self_res: None,
            cond: 0,
            asyncd: 0,
            early: false,
            returned: false,
            locals: vec![HashMap::new()],
            marks: HashMap::new(),
            prefixes: Vec::new(),
            cli: None,
        };
        walker.visit_expr(value.expr);
    }
    graph
}

#[derive(Debug, Clone)]
enum Local {
    Value(Ty),
    Closure(FnId),
    Function(FnId),
}

#[derive(Debug, Clone)]
enum Mark {
    Spawned,
    Registered {
        evidence: String,
        route: Option<(String, String)>,
    },
}

/// Where a call goes.
enum Target {
    Fns(Vec<FnId>, &'static str),
    External {
        id: String,
        name: String,
        container: String,
        resolution: &'static str,
    },
    None,
}

struct Walker<'w, 'a> {
    index: &'w Index<'a>,
    graph: &'w mut Graph<'a>,
    caller: Option<FnId>,
    /// Id and name closures written here are numbered under.
    parent_id: String,
    parent_name: String,
    scope: ModId,
    krate: usize,
    file: usize,
    test: bool,
    ctx: TyCtx,
    self_res: Option<Res>,
    /// Depth of conditional constructs around the current expression.
    cond: u32,
    /// Depth of spawned code around it.
    asyncd: u32,
    /// A path that returns early was seen: what follows is conditional.
    early: bool,
    /// A `return` or `?` was seen (a `break` only leaves its loop).
    returned: bool,
    locals: Vec<HashMap<String, Local>>,
    marks: HashMap<usize, Mark>,
    /// Route prefixes of the routers being built around the current code.
    prefixes: Vec<String>,
    /// The CLI subcommand whose match arm the current code is in.
    /// with its line and the calls that dispatch it (the arm's outermost
    /// calls, by address).
    cli: Option<(String, u32, Vec<usize>)>,
}

fn key(expr: &Expr) -> usize {
    expr as *const Expr as usize
}

fn strip(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip(&inner.expr),
        Expr::Group(inner) => strip(&inner.expr),
        Expr::Reference(inner) => strip(&inner.expr),
        other => other,
    }
}

fn last_segment(path: &syn::Path) -> String {
    path.segments
        .last()
        .map(|segment| ident(&segment.ident))
        .unwrap_or_default()
}

/// An external callee as the protocol lists it: "std::collections::HashMap::new"
/// named "HashMap::new" in "std::collections".
pub fn external_parts(path: &[String]) -> (String, String, String) {
    let id = path.join("::");
    let count = path.len();
    if count >= 3 && is_type_like(&path[count - 2]) {
        return (
            id,
            format!("{}::{}", path[count - 2], path[count - 1]),
            path[..count - 2].join("::"),
        );
    }
    if count == 2 && is_type_like(&path[0]) {
        return (id, format!("{}::{}", path[0], path[1]), path[0].clone());
    }
    let container = if count > 1 {
        path[..count - 1].join("::")
    } else {
        id.clone()
    };
    (id, path[count - 1].clone(), container)
}

/// CamelCase as clap writes subcommands: kebab-case.
pub fn kebab(name: &str) -> String {
    let mut out = String::new();
    for (position, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if position > 0 {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else if c == '_' {
            out.push('-');
        } else {
            out.push(c);
        }
    }
    out
}

/// The calls a match arm dispatches to: its outermost calls (through
/// `.await`, `?` and `return`), and in a block those of its expression
/// statements, not the ones in its `let`s, which prepare them.
fn dispatch_calls(expr: &Expr, out: &mut Vec<usize>) {
    match expr {
        Expr::Paren(inner) => dispatch_calls(&inner.expr, out),
        Expr::Group(inner) => dispatch_calls(&inner.expr, out),
        Expr::Await(inner) => dispatch_calls(&inner.base, out),
        Expr::Try(inner) => dispatch_calls(&inner.expr, out),
        Expr::Return(inner) => {
            if let Some(value) = &inner.expr {
                dispatch_calls(value, out);
            }
        }
        Expr::Call(call) => out.push(call as *const syn::ExprCall as usize),
        Expr::MethodCall(call) => out.push(call as *const syn::ExprMethodCall as usize),
        Expr::Block(block) => {
            for statement in &block.block.stmts {
                if let syn::Stmt::Expr(inner, _) = statement {
                    dispatch_calls(inner, out);
                }
            }
        }
        _ => {}
    }
}

fn join_route(parts: &[&str]) -> String {
    let mut out = String::new();
    for part in parts {
        let part = part.trim_end_matches('/');
        if part.is_empty() {
            continue;
        }
        if !part.starts_with('/') {
            out.push('/');
        }
        out.push_str(part);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

impl<'w, 'a> Walker<'w, 'a> {
    fn new(
        index: &'w Index<'a>,
        graph: &'w mut Graph<'a>,
        caller: Option<FnId>,
        definition: &FnDef<'a>,
        ctx: TyCtx,
        self_res: Option<Res>,
    ) -> Self {
        Self {
            index,
            graph,
            caller,
            parent_id: definition.id.clone(),
            parent_name: definition.name.clone(),
            scope: definition.scope,
            krate: definition.krate,
            file: definition.file,
            test: definition.test,
            ctx,
            self_res,
            cond: 0,
            asyncd: 0,
            early: false,
            returned: false,
            locals: vec![HashMap::new()],
            marks: HashMap::new(),
            prefixes: Vec::new(),
            cli: None,
        }
    }

    fn def(&self, function: FnId) -> &FnDef<'a> {
        let count = self.index.fns.len();
        if function < count {
            &self.index.fns[function]
        } else {
            &self.graph.closures[function - count]
        }
    }

    fn id_of(&self, function: FnId) -> String {
        self.def(function).id.clone()
    }

    fn rel(&self) -> String {
        self.index
            .files
            .list
            .get(self.file)
            .map(|file| file.rel.clone())
            .unwrap_or_default()
    }

    fn kind(&self) -> &'static str {
        if self.asyncd > 0 {
            "async"
        } else if self.cond > 0 || self.early {
            "conditional"
        } else {
            "always"
        }
    }

    fn bind(&mut self, name: String, local: Local) {
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name, local);
        }
    }

    fn local(&self, name: &str) -> Option<&Local> {
        self.locals.iter().rev().find_map(|scope| scope.get(name))
    }

    fn bind_inputs(&mut self, sig: &syn::Signature) {
        for input in &sig.inputs {
            match input {
                syn::FnArg::Receiver(_) => {
                    let ty = self.ctx.self_ty.clone().unwrap_or(Ty::Unknown);
                    self.bind("self".to_string(), Local::Value(ty));
                }
                syn::FnArg::Typed(typed) => {
                    let ty = self.index.resolve_type(self.scope, &typed.ty, &self.ctx);
                    self.bind_pattern(&typed.pat, ty);
                }
            }
        }
    }

    /// Binds the names of a pattern; a single name gets `ty`, the others
    /// are unknown (they shadow what was before).
    fn bind_pattern(&mut self, pattern: &Pat, ty: Ty) {
        match pattern {
            Pat::Ident(binding) => self.bind(ident(&binding.ident), Local::Value(ty)),
            Pat::Type(typed) => {
                let ty = self.index.resolve_type(self.scope, &typed.ty, &self.ctx);
                self.bind_pattern(&typed.pat, ty);
            }
            Pat::Reference(reference) => self.bind_pattern(&reference.pat, ty),
            Pat::TupleStruct(tuple) => {
                let inner = match (&ty, last_segment(&tuple.path).as_str()) {
                    (Ty::Wrapped(inner), "Some" | "Ok") if tuple.elems.len() == 1 => {
                        (**inner).clone()
                    }
                    _ => Ty::Unknown,
                };
                for element in &tuple.elems {
                    self.bind_pattern(element, inner.clone());
                }
            }
            Pat::Tuple(tuple) => {
                for element in &tuple.elems {
                    self.bind_pattern(element, Ty::Unknown);
                }
            }
            Pat::Struct(fields) => {
                for field in &fields.fields {
                    self.bind_pattern(&field.pat, Ty::Unknown);
                }
            }
            Pat::Slice(slice) => {
                for element in &slice.elems {
                    self.bind_pattern(element, Ty::Unknown);
                }
            }
            Pat::Or(alternatives) => {
                if let Some(first) = alternatives.cases.first() {
                    self.bind_pattern(first, Ty::Unknown);
                }
            }
            _ => {}
        }
    }

    // ---- emitting ----

    fn push_call(
        &mut self,
        callee: String,
        line: u32,
        kind: &str,
        resolution: &str,
        relation: &str,
        evidence: Option<String>,
    ) -> Option<usize> {
        let caller = self.caller?;
        let caller = self.id_of(caller);
        self.graph.calls.push(Call {
            caller,
            callee,
            line,
            kind: kind.to_string(),
            resolution: resolution.to_string(),
            relation: relation.to_string(),
            evidence,
        });
        Some(self.graph.calls.len() - 1)
    }

    /// Records the call at `site` (the address of its syntax node, 0 for
    /// none) to `target`.
    fn emit(&mut self, target: Target, line: u32, site: usize) {
        let kind = self.kind();
        match target {
            Target::Fns(functions, resolution) => {
                for function in functions {
                    let callee = self.id_of(function);
                    self.push_call(callee, line, kind, resolution, "call", None);
                    if resolution == "static" && kind != "async" {
                        if let Some((label, arm_line, _)) = self
                            .cli
                            .clone()
                            .filter(|(_, _, sites)| sites.contains(&site))
                        {
                            let file = self.rel();
                            self.graph.add_entry(
                                function,
                                EntryPoint {
                                    kind: "cli_command".to_string(),
                                    label,
                                    file: Some(file),
                                    line: Some(arm_line),
                                },
                            );
                        }
                    }
                }
            }
            Target::External {
                id,
                name,
                container,
                resolution,
            } => {
                if self.caller.is_none() {
                    return;
                }
                self.graph.externals.entry(id.clone()).or_insert(External {
                    id: id.clone(),
                    name,
                    container,
                });
                self.push_call(id, line, kind, resolution, "call", None);
            }
            Target::None => {}
        }
    }

    fn reference(&mut self, callee: FnId, line: u32) {
        let Some(caller) = self.caller else { return };
        if caller == callee {
            return;
        }
        let call = Call {
            caller: self.id_of(caller),
            callee: self.id_of(callee),
            line,
            kind: "async".to_string(),
            resolution: "static".to_string(),
            relation: "reference".to_string(),
            evidence: None,
        };
        self.graph.references.push(call);
    }

    fn register(
        &mut self,
        callee: FnId,
        line: u32,
        evidence: String,
        route: Option<(String, String)>,
    ) {
        let Some(caller) = self.caller else { return };
        if caller == callee {
            return;
        }
        let call = self.push_call(
            self.id_of(callee),
            line,
            "async",
            "static",
            "registration",
            Some(evidence),
        );
        if let Some((verb, path)) = route {
            let file = self.rel();
            self.graph.routes.push(Route {
                caller,
                handler: callee,
                call,
                verb,
                path,
                file,
                line,
            });
        }
    }

    /// Whether a call from here may go to `function`: production code does
    /// not list targets that only tests declare.
    fn allowed(&self, function: FnId) -> bool {
        self.test || !self.def(function).test
    }

    // ---- resolution ----

    fn resolve(&self, segs: &[String], value: bool) -> Res {
        self.index
            .resolve(self.scope, segs, self.self_res.as_ref(), value)
    }

    fn trait_targets(&self, trait_id: usize, name: &str) -> Target {
        let targets: Vec<FnId> = self
            .index
            .trait_method_targets(trait_id, name)
            .into_iter()
            .filter(|function| self.allowed(*function))
            .collect();
        if targets.is_empty() {
            let (id, name, container) = self.index.trait_method_id(trait_id, name);
            return Target::External {
                id,
                name,
                container,
                resolution: "dynamic",
            };
        }
        Target::Fns(targets, "dynamic")
    }

    fn external(path: Vec<String>, resolution: &'static str) -> Target {
        if path.last().is_none_or(|last| is_type_like(last)) {
            return Target::None;
        }
        let (id, name, container) = external_parts(&path);
        Target::External {
            id,
            name,
            container,
            resolution,
        }
    }

    /// What calling the path `path` calls.
    fn path_target(&self, path: &ExprPath) -> Target {
        let segs = segments(&path.path);
        if let Some(qself) = &path.qself {
            let method = segs.last().cloned().unwrap_or_default();
            let ty = self.index.resolve_type(self.scope, &qself.ty, &self.ctx);
            if qself.position == 0 {
                return self.method_targets(&ty, &method);
            }
            let trait_segs = &segs[..qself.position.min(segs.len())];
            return match self.resolve(trait_segs, false) {
                Res::Trait(trait_id) => match ty {
                    Ty::Named(type_id) => match self.index.method_of_type(type_id, &method) {
                        Some(function) => Target::Fns(vec![function], "static"),
                        None => self.trait_targets(trait_id, &method),
                    },
                    _ => self.trait_targets(trait_id, &method),
                },
                Res::External(mut trait_path) => {
                    trait_path.push(method);
                    Self::external(trait_path, "dynamic")
                }
                _ => Target::None,
            };
        }
        if segs.len() == 1 {
            match self.local(&segs[0]) {
                Some(Local::Closure(function) | Local::Function(function)) => {
                    return Target::Fns(vec![*function], "static")
                }
                Some(Local::Value(_)) => return Target::None,
                None => {}
            }
        }
        match self.resolve(&segs, true) {
            Res::Fn(function) => Target::Fns(vec![function], "static"),
            Res::TraitMethod(trait_id, name) => self.trait_targets(trait_id, &name),
            Res::External(path) => Self::external(path, "static"),
            _ => Target::None,
        }
    }

    /// What calling the method `name` on a value of type `ty` calls.
    fn method_targets(&self, ty: &Ty, name: &str) -> Target {
        match ty {
            Ty::Named(type_id) => match self.index.method_of_type(*type_id, name) {
                Some(function) => Target::Fns(vec![function], "static"),
                None => Target::None,
            },
            Ty::Dyn(traits) => {
                for trait_id in traits {
                    if self.index.trait_declares(*trait_id, name).is_some() {
                        return self.trait_targets(*trait_id, name);
                    }
                }
                Target::None
            }
            Ty::External(path) => {
                let mut path = path.clone();
                path.push(name.to_string());
                Self::external(path, "static")
            }
            Ty::ExternalDyn(path) => {
                let mut path = path.clone();
                path.push(name.to_string());
                Self::external(path, "dynamic")
            }
            Ty::Wrapped(_) => Target::None,
            Ty::Unknown => {
                if COMMON_METHODS.contains(&name) {
                    return Target::None;
                }
                let candidates: Vec<FnId> = self
                    .index
                    .methods_by_name
                    .get(name)
                    .map(|list| {
                        list.iter()
                            .copied()
                            .filter(|function| self.allowed(*function))
                            .collect()
                    })
                    .unwrap_or_default();
                if candidates.is_empty() || candidates.len() > MAX_GUESSES {
                    return Target::None;
                }
                Target::Fns(candidates, "dynamic")
            }
        }
    }

    /// The type of an expression, as far as syntax tells.
    fn expr_ty(&self, expr: &Expr) -> Ty {
        match expr {
            Expr::Paren(inner) => self.expr_ty(&inner.expr),
            Expr::Group(inner) => self.expr_ty(&inner.expr),
            Expr::Reference(inner) => self.expr_ty(&inner.expr),
            Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
                self.expr_ty(&unary.expr)
            }
            Expr::Await(inner) => self.expr_ty(&inner.base),
            Expr::Try(inner) => match self.expr_ty(&inner.expr) {
                Ty::Wrapped(inner) => *inner,
                _ => Ty::Unknown,
            },
            Expr::Cast(cast) => self.index.resolve_type(self.scope, &cast.ty, &self.ctx),
            Expr::Path(path) if path.qself.is_none() => {
                let segs = segments(&path.path);
                if segs.len() == 1 {
                    if let Some(Local::Value(ty)) = self.local(&segs[0]) {
                        return ty.clone();
                    }
                }
                Ty::Unknown
            }
            Expr::Struct(literal) => {
                let segs = segments(&literal.path);
                if segs.len() == 1 && segs[0] == "Self" {
                    return self.ctx.self_ty.clone().unwrap_or(Ty::Unknown);
                }
                match self.resolve(&segs, false) {
                    Res::Type(type_id) => Ty::Named(type_id),
                    _ => Ty::Unknown,
                }
            }
            Expr::Field(field) => match self.expr_ty(&field.base) {
                Ty::Named(type_id) => {
                    let name = match &field.member {
                        syn::Member::Named(name) => ident(name),
                        syn::Member::Unnamed(position) => position.index.to_string(),
                    };
                    self.index.field_type(type_id, &name)
                }
                _ => Ty::Unknown,
            },
            Expr::Call(call) => {
                let Expr::Path(path) = strip(&call.func) else {
                    return Ty::Unknown;
                };
                let segs = segments(&path.path);
                let last = segs.last().map(String::as_str).unwrap_or("");
                if matches!(last, "Some" | "Ok") && call.args.len() == 1 {
                    return Ty::Wrapped(Box::new(self.expr_ty(&call.args[0])));
                }
                if segs.len() >= 2 && matches!(last, "new" | "from") && call.args.len() == 1 {
                    let wrapper = segs[segs.len() - 2].as_str();
                    if matches!(
                        wrapper,
                        "Box" | "Arc" | "Rc" | "Mutex" | "RefCell" | "RwLock"
                    ) {
                        return self.expr_ty(&call.args[0]);
                    }
                }
                if path.qself.is_none() && segs.len() == 1 {
                    if let Some(Local::Function(function)) = self.local(&segs[0]) {
                        return self.index.return_type(*function);
                    }
                }
                match self.resolve(&segs, true) {
                    Res::Fn(function) => self.index.return_type(function),
                    Res::Type(type_id) => Ty::Named(type_id),
                    Res::External(mut path) => {
                        let constructor = path.pop();
                        let is_constructor = constructor.as_deref().is_some_and(|name| {
                            matches!(
                                name,
                                "new" | "default" | "with_capacity" | "from" | "builder"
                            )
                        });
                        if is_constructor && path.last().is_some_and(|name| is_type_like(name)) {
                            Ty::External(path)
                        } else {
                            Ty::Unknown
                        }
                    }
                    _ => Ty::Unknown,
                }
            }
            Expr::MethodCall(call) => {
                let receiver = self.expr_ty(&call.receiver);
                let name = ident(&call.method);
                match name.as_str() {
                    "clone" | "to_owned" | "as_ref" | "as_mut" | "borrow" | "borrow_mut"
                    | "lock" | "read" | "write" => {
                        if matches!(receiver, Ty::Named(_) | Ty::Dyn(_)) {
                            return receiver;
                        }
                        Ty::Unknown
                    }
                    "unwrap" | "expect" | "unwrap_or_default" | "unwrap_or" | "unwrap_or_else" => {
                        match receiver {
                            Ty::Wrapped(inner) => *inner,
                            _ => Ty::Unknown,
                        }
                    }
                    _ => match self.method_targets(&receiver, &name) {
                        Target::Fns(functions, "static") if functions.len() == 1 => {
                            self.index.return_type(functions[0])
                        }
                        _ => Ty::Unknown,
                    },
                }
            }
            _ => Ty::Unknown,
        }
    }

    // ---- closures ----

    /// Indexes a closure written here and walks its body as its own
    /// function.
    fn closure(&mut self, closure: &syn::ExprClosure) -> FnId {
        let counter = self
            .graph
            .closure_counts
            .entry(self.parent_id.clone())
            .or_insert(0);
        *counter += 1;
        let suffix = format!("${counter}");
        let id = format!("{}{suffix}", self.parent_id);
        let name = format!("{}{suffix}", self.parent_name);
        let real = self.index.real_module(self.scope);
        let module = &self.index.modules[real];
        let (start, end) = source::lines(closure);
        let function = self.index.fns.len() + self.graph.closures.len();
        self.graph.closures.push(FnDef {
            qualified: format!("{}::{name}", module.short),
            container: module.container.clone(),
            id: id.clone(),
            name: name.clone(),
            file: self.file,
            start,
            end,
            test: self.test,
            generated: self
                .index
                .files
                .list
                .get(self.file)
                .is_some_and(|file| file.generated),
            complexity: Some(crate::complexity::of_expr(&closure.body)),
            owner: Owner::Closure,
            scope: self.scope,
            krate: self.krate,
            sig: None,
            body: None,
            attrs: &[],
            public: false,
            parent: self.caller,
        });
        // Its body is a function of its own: its calls are relative to it.
        let saved = (
            self.caller.replace(function),
            std::mem::replace(&mut self.parent_id, id),
            std::mem::replace(&mut self.parent_name, name),
            std::mem::take(&mut self.cond),
            std::mem::take(&mut self.asyncd),
            std::mem::take(&mut self.early),
            std::mem::take(&mut self.returned),
            self.cli.take(),
        );
        self.locals.push(HashMap::new());
        for input in &closure.inputs {
            self.bind_pattern(input, Ty::Unknown);
        }
        self.visit_expr(&closure.body);
        self.locals.pop();
        (
            self.caller,
            self.parent_id,
            self.parent_name,
            self.cond,
            self.asyncd,
            self.early,
            self.returned,
            self.cli,
        ) = saved;
        function
    }

    // ---- frameworks ----

    fn frameworks(&self) -> crate::index::Frameworks {
        self.index.crates[self.krate].frameworks
    }

    /// The handlers of a method router: `get(a).post(b)` (axum),
    /// `web::get().to(a)` (actix), `on(MethodFilter::GET, a)`.
    fn method_router<'e>(&self, expr: &'e Expr, out: &mut Vec<(String, &'e Expr)>) {
        match strip(expr) {
            Expr::Call(call) => {
                let Expr::Path(path) = strip(&call.func) else {
                    return;
                };
                let name = last_segment(&path.path);
                if VERBS.contains(&name.as_str()) && call.args.len() == 1 {
                    out.push((name.to_uppercase(), &call.args[0]));
                } else if name == "on" && call.args.len() == 2 {
                    let verb = match strip(&call.args[0]) {
                        Expr::Path(filter) => last_segment(&filter.path),
                        _ => "ANY".to_string(),
                    };
                    out.push((verb.to_uppercase(), &call.args[1]));
                } else if name == "to" && call.args.len() == 1 {
                    out.push(("ANY".to_string(), &call.args[0]));
                }
            }
            Expr::MethodCall(call) => {
                let name = ident(&call.method);
                if VERBS.contains(&name.as_str()) && call.args.len() == 1 {
                    self.method_router(&call.receiver, out);
                    out.push((name.to_uppercase(), &call.args[0]));
                } else if name == "to" && call.args.len() == 1 {
                    out.push((Self::actix_verb(&call.receiver), &call.args[0]));
                } else if matches!(
                    name.as_str(),
                    "layer" | "with_state" | "route_layer" | "guard" | "name" | "wrap"
                ) {
                    self.method_router(&call.receiver, out);
                }
            }
            _ => {}
        }
    }

    /// The verb of an actix route: `web::get()`, `web::route().method(Method::POST)`.
    fn actix_verb(expr: &Expr) -> String {
        match strip(expr) {
            Expr::Call(call) => match strip(&call.func) {
                Expr::Path(path) => {
                    let name = last_segment(&path.path);
                    if VERBS.contains(&name.as_str()) && call.args.is_empty() {
                        name.to_uppercase()
                    } else {
                        "ANY".to_string()
                    }
                }
                _ => "ANY".to_string(),
            },
            Expr::MethodCall(call) => {
                if ident(&call.method) == "method" && call.args.len() == 1 {
                    if let Expr::Path(method) = strip(&call.args[0]) {
                        return last_segment(&method.path).to_uppercase();
                    }
                }
                Self::actix_verb(&call.receiver)
            }
            _ => "ANY".to_string(),
        }
    }

    /// The string given to the call at the root of a builder chain when it
    /// is one of `names`: the "/api" of `web::scope("/api").route(…)`.
    fn chain_root(expr: &Expr, names: &[&str]) -> Option<String> {
        match strip(expr) {
            Expr::MethodCall(call) => Self::chain_root(&call.receiver, names),
            Expr::Call(call) => match strip(&call.func) {
                Expr::Path(path) if names.contains(&last_segment(&path.path).as_str()) => Some(
                    call.args
                        .first()
                        .and_then(string_literal)
                        .unwrap_or_else(|| "{dynamic}".to_string()),
                ),
                _ => None,
            },
            _ => None,
        }
    }

    fn route_path(&self, scope: Option<String>, path: &str) -> String {
        let mut parts: Vec<&str> = self.prefixes.iter().map(String::as_str).collect();
        if let Some(scope) = &scope {
            parts.push(scope);
        }
        parts.push(path);
        join_route(&parts)
    }

    /// Marks an expression for when it is visited; an outer registration
    /// marks first and wins.
    fn mark(&mut self, expr: &Expr, mark: Mark, keys: &mut Vec<usize>) {
        let key = key(expr);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.marks.entry(key) {
            entry.insert(mark);
            keys.push(key);
        }
    }

    /// The route an in-tree function declares with an attribute
    /// (`#[get("/x")]` of actix-web or rocket).
    fn attribute_route(&self, function: FnId) -> Option<(String, String)> {
        crate::entrypoints::attribute_route(self.def(function).attrs)
    }

    /// Marks the handlers a method call of a router registers, and returns
    /// the prefix its arguments are nested under.
    fn router_hooks(
        &mut self,
        call: &syn::ExprMethodCall,
        keys: &mut Vec<usize>,
    ) -> Option<String> {
        let frameworks = self.frameworks();
        if !frameworks.http() {
            return None;
        }
        let name = ident(&call.method);
        let args: Vec<&Expr> = call.args.iter().collect();
        let scope = Self::chain_root(&call.receiver, &["scope"]);
        match name.as_str() {
            "route" if (frameworks.axum || frameworks.actix) && args.len() == 2 => {
                let path = string_literal(args[0]).unwrap_or_else(|| "{dynamic}".to_string());
                let full = self.route_path(scope, &path);
                let mut handlers = Vec::new();
                self.method_router(args[1], &mut handlers);
                for (verb, handler) in handlers {
                    let evidence = format!("route {verb} {full}");
                    self.mark(
                        handler,
                        Mark::Registered {
                            evidence,
                            route: Some((verb, full.clone())),
                        },
                        keys,
                    );
                }
            }
            "route" | "to"
                if frameworks.actix
                    && args.len() == 1
                    && Self::chain_root(&call.receiver, &["resource"]).is_some() =>
            {
                let resource = Self::chain_root(&call.receiver, &["resource"]).unwrap_or_default();
                let full = self.route_path(None, &resource);
                let mut handlers = Vec::new();
                if name == "to" {
                    handlers.push(("ANY".to_string(), args[0]));
                } else {
                    self.method_router(args[0], &mut handlers);
                }
                for (verb, handler) in handlers {
                    let evidence = format!("route {verb} {full}");
                    self.mark(
                        handler,
                        Mark::Registered {
                            evidence,
                            route: Some((verb, full.clone())),
                        },
                        keys,
                    );
                }
            }
            "nest" | "nest_service" if frameworks.axum && args.len() == 2 => {
                let prefix = string_literal(args[0]).unwrap_or_else(|| "{dynamic}".to_string());
                if let Expr::Call(inner) = strip(args[1]) {
                    if let Expr::Path(path) = strip(&inner.func) {
                        if let (Res::Fn(function), Some(caller)) =
                            (self.resolve(&segments(&path.path), true), self.caller)
                        {
                            let full = self.route_path(None, &prefix);
                            self.graph.nests.push((function, full, caller));
                        }
                    }
                }
                return Some(prefix);
            }
            "service" if frameworks.actix && args.len() == 1 => {
                if let Expr::Path(path) = strip(args[0]) {
                    if let Res::Fn(function) = self.resolve(&segments(&path.path), true) {
                        if let Some((verb, attribute_path)) = self.attribute_route(function) {
                            let full = self.route_path(scope, &attribute_path);
                            let evidence = format!("route {verb} {full}");
                            self.mark(
                                args[0],
                                Mark::Registered {
                                    evidence,
                                    route: Some((verb, full)),
                                },
                                keys,
                            );
                        }
                    }
                }
            }
            "mount" | "register" if frameworks.rocket && args.len() == 2 => {
                let base = string_literal(args[0]).unwrap_or_else(|| "{dynamic}".to_string());
                if let Expr::Macro(mac) = strip(args[1]) {
                    for path in source::macro_paths(&mac.mac.tokens) {
                        let Res::Fn(function) = self.resolve(&segments(&path), true) else {
                            continue;
                        };
                        let line = source::line(path.span());
                        if name == "register" {
                            self.register(function, line, "error handler".to_string(), None);
                            continue;
                        }
                        let (verb, attribute_path) = self
                            .attribute_route(function)
                            .unwrap_or(("ANY".to_string(), "/".to_string()));
                        let full = self.route_path(None, &join_route(&[&base, &attribute_path]));
                        self.register(
                            function,
                            line,
                            format!("route {verb} {full}"),
                            Some((verb, full)),
                        );
                    }
                }
            }
            "layer" | "route_layer" | "wrap" => {
                for arg in &args {
                    self.middleware(arg, keys);
                }
            }
            "wrap_fn" if args.len() == 1 => {
                self.mark(
                    args[0],
                    Mark::Registered {
                        evidence: "middleware".to_string(),
                        route: None,
                    },
                    keys,
                );
            }
            "fallback" | "default_service" if args.len() == 1 => {
                let mut handlers = Vec::new();
                self.method_router(args[0], &mut handlers);
                if handlers.is_empty() {
                    handlers.push((String::new(), args[0]));
                }
                for (_, handler) in handlers {
                    self.mark(
                        handler,
                        Mark::Registered {
                            evidence: "not found handler".to_string(),
                            route: None,
                        },
                        keys,
                    );
                }
            }
            "service" if frameworks.actix => {}
            _ => {}
        }
        // An actix scope nests what its builder registers, as its
        // arguments: `web::scope("/api").service(…)`.
        if frameworks.actix && matches!(name.as_str(), "service" | "route" | "configure") {
            return Self::chain_root(&call.receiver, &["scope"]);
        }
        None
    }

    /// `from_fn(auth)`-like middleware constructors inside a layer.
    fn middleware(&mut self, expr: &Expr, keys: &mut Vec<usize>) {
        match strip(expr) {
            Expr::Call(call) => {
                if let Expr::Path(path) = strip(&call.func) {
                    let name = last_segment(&path.path);
                    if matches!(
                        name.as_str(),
                        "from_fn"
                            | "from_fn_with_state"
                            | "map_request"
                            | "map_response"
                            | "map_request_with_state"
                            | "map_response_with_state"
                    ) {
                        if let Some(handler) = call.args.last() {
                            self.mark(
                                handler,
                                Mark::Registered {
                                    evidence: "middleware".to_string(),
                                    route: None,
                                },
                                keys,
                            );
                        }
                    }
                }
                for arg in &call.args {
                    self.middleware(arg, keys);
                }
            }
            Expr::MethodCall(call) => {
                self.middleware(&call.receiver, keys);
                for arg in &call.args {
                    self.middleware(arg, keys);
                }
            }
            _ => {}
        }
    }

    /// The CLI subcommand a match arm's pattern names: a variant of a
    /// `#[derive(Subcommand)]` enum, or `Some(("name", _))` of clap's
    /// `matches.subcommand()`.
    fn cli_label(&self, pattern: &Pat, builder: bool) -> Option<String> {
        let name = if builder {
            Self::builder_subcommand(pattern)?
        } else {
            self.derived_subcommand(pattern)?
        };
        let krate = &self.index.crates[self.krate];
        let program = match krate.kind {
            crate::workspace::TargetKind::Bin => krate.target_name.clone(),
            _ => krate.package_name.clone(),
        };
        Some(format!("{program} {name}"))
    }

    fn builder_subcommand(pattern: &Pat) -> Option<String> {
        let Pat::TupleStruct(some) = pattern else {
            return None;
        };
        let first = some.elems.first()?;
        let literal = match first {
            Pat::Tuple(tuple) => tuple.elems.first()?,
            other => other,
        };
        match literal {
            Pat::Lit(syn::PatLit {
                lit: syn::Lit::Str(text),
                ..
            }) => Some(text.value()),
            _ => None,
        }
    }

    fn derived_subcommand(&self, pattern: &Pat) -> Option<String> {
        let path = match pattern {
            Pat::TupleStruct(tuple) => segments(&tuple.path),
            Pat::Struct(fields) => segments(&fields.path),
            Pat::Path(path) => segments(&path.path),
            Pat::Ident(binding) if binding.subpat.is_none() => vec![ident(&binding.ident)],
            Pat::Or(alternatives) => {
                return alternatives
                    .cases
                    .first()
                    .and_then(|first| self.derived_subcommand(first))
            }
            _ => return None,
        };
        let variant = path.last()?.clone();
        let enums: Vec<usize> = if path.len() >= 2 {
            match self.resolve(&path[..path.len() - 1], false) {
                Res::Type(type_id) => vec![type_id],
                _ => Vec::new(),
            }
        } else {
            (0..self.index.types.len())
                .filter(|type_id| {
                    self.index.modules[self.index.types[*type_id].module].krate == self.krate
                })
                .collect()
        };
        enums
            .into_iter()
            .find_map(|type_id| match &self.index.types[type_id].kind {
                TypeKind::Enum {
                    variants,
                    subcommand: true,
                } => variants
                    .iter()
                    .find(|(name, _)| *name == variant)
                    .map(|(name, renamed)| renamed.clone().unwrap_or_else(|| kebab(name))),
                _ => None,
            })
    }

    /// Walks a loop: a `break` or `continue` in it makes the rest of its
    /// body conditional, but only a `return` or `?` the code after it.
    fn in_loop(&mut self, body: impl FnOnce(&mut Self)) {
        let (early, returned) = (self.early, std::mem::take(&mut self.returned));
        body(self);
        let returned_inside = self.returned;
        self.early = early || returned_inside;
        self.returned = returned || returned_inside;
    }

    fn visit_args<'v>(&mut self, args: impl Iterator<Item = &'v Expr>, spawn: bool) {
        let mut keys = Vec::new();
        let args: Vec<&Expr> = args.collect();
        if spawn {
            for arg in &args {
                self.mark(arg, Mark::Spawned, &mut keys);
            }
        }
        for arg in args {
            self.visit_expr(arg);
        }
        for key in keys {
            self.marks.remove(&key);
        }
    }
}

impl<'v, 'w, 'a> Visit<'v> for Walker<'w, 'a> {
    fn visit_item(&mut self, _: &'v syn::Item) {}

    fn visit_block(&mut self, block: &'v syn::Block) {
        self.locals.push(HashMap::new());
        syn::visit::visit_block(self, block);
        self.locals.pop();
    }

    fn visit_expr(&mut self, expr: &'v Expr) {
        if let Some(mark) = self.marks.remove(&key(expr)) {
            match (mark, expr) {
                (Mark::Spawned, Expr::Closure(closure)) => {
                    let line = source::line(closure.span());
                    let function = self.closure(closure);
                    let callee = self.id_of(function);
                    self.push_call(callee, line, "async", "static", "call", None);
                    return;
                }
                (Mark::Spawned, Expr::Path(path)) => {
                    if let Target::Fns(functions, _) = self.path_target(path) {
                        let line = source::line(path.span());
                        for function in functions {
                            let callee = self.id_of(function);
                            self.push_call(callee, line, "async", "static", "call", None);
                        }
                    }
                    return;
                }
                (Mark::Spawned, other) => {
                    self.asyncd += 1;
                    syn::visit::visit_expr(self, other);
                    self.asyncd -= 1;
                    return;
                }
                (Mark::Registered { evidence, route }, Expr::Closure(closure)) => {
                    let line = source::line(closure.span());
                    let function = self.closure(closure);
                    self.register(function, line, evidence, route);
                    return;
                }
                (Mark::Registered { evidence, route }, Expr::Path(path)) => {
                    if let Target::Fns(functions, "static") = self.path_target(path) {
                        let line = source::line(path.span());
                        for function in functions {
                            self.register(function, line, evidence.clone(), route.clone());
                        }
                        return;
                    }
                }
                _ => {}
            }
        }
        syn::visit::visit_expr(self, expr);
    }

    fn visit_expr_call(&mut self, call: &'v syn::ExprCall) {
        let line = source::line(call.paren_token.span.open());
        match strip(&call.func) {
            Expr::Path(path) => {
                let target = self.path_target(path);
                let spawn = SPAWNERS.contains(&last_segment(&path.path).as_str());
                self.visit_args(call.args.iter(), spawn);
                self.emit(target, line, call as *const syn::ExprCall as usize);
            }
            Expr::Closure(closure) => {
                let function = self.closure(closure);
                self.visit_args(call.args.iter(), false);
                self.emit(Target::Fns(vec![function], "static"), line, 0);
            }
            _ => {
                self.visit_expr(&call.func);
                self.visit_args(call.args.iter(), false);
            }
        }
    }

    fn visit_expr_method_call(&mut self, call: &'v syn::ExprMethodCall) {
        self.visit_expr(&call.receiver);
        let name = ident(&call.method);
        let line = source::line(call.method.span());
        let mut keys = Vec::new();
        let prefix = self.router_hooks(call, &mut keys);
        let spawn = SPAWNERS.contains(&name.as_str());
        if let Some(prefix) = &prefix {
            self.prefixes.push(prefix.clone());
        }
        self.visit_args(call.args.iter(), spawn);
        if prefix.is_some() {
            self.prefixes.pop();
        }
        for key in keys {
            self.marks.remove(&key);
        }
        let receiver = self.expr_ty(&call.receiver);
        let target = self.method_targets(&receiver, &name);
        self.emit(target, line, call as *const syn::ExprMethodCall as usize);
    }

    fn visit_expr_closure(&mut self, closure: &'v syn::ExprClosure) {
        let line = source::line(closure.span());
        let function = self.closure(closure);
        self.reference(function, line);
    }

    fn visit_expr_path(&mut self, path: &'v ExprPath) {
        // A function used as a value (a call's function is not visited).
        let segs = segments(&path.path);
        let line = source::line(path.span());
        if path.qself.is_none() && segs.len() == 1 {
            match self.local(&segs[0]) {
                Some(Local::Closure(function) | Local::Function(function)) => {
                    let function = *function;
                    self.reference(function, line);
                    return;
                }
                Some(Local::Value(_)) => return,
                None => {}
            }
        }
        if path.qself.is_some() {
            return;
        }
        if let Res::Fn(function) = self.resolve(&segs, true) {
            self.reference(function, line);
        }
    }

    fn visit_expr_if(&mut self, node: &'v syn::ExprIf) {
        self.locals.push(HashMap::new());
        self.visit_expr(&node.cond);
        self.cond += 1;
        self.visit_block(&node.then_branch);
        self.locals.pop();
        if let Some((_, otherwise)) = &node.else_branch {
            self.visit_expr(otherwise);
        }
        self.cond -= 1;
    }

    fn visit_expr_let(&mut self, node: &'v syn::ExprLet) {
        self.visit_expr(&node.expr);
        let ty = self.expr_ty(&node.expr);
        self.bind_pattern(&node.pat, ty);
    }

    fn visit_expr_match(&mut self, node: &'v syn::ExprMatch) {
        self.visit_expr(&node.expr);
        let builder = matches!(strip(&node.expr), Expr::MethodCall(call) if matches!(ident(&call.method).as_str(), "subcommand" | "subcommand_name"));
        let scrutinee = self.expr_ty(&node.expr);
        let conditional = node.arms.len() > 1;
        let clap = self.frameworks().clap;
        for arm in &node.arms {
            let (pattern, guard) = source::arm_pattern(arm);
            let saved = self.cli.clone();
            if clap {
                if let Some(label) = self.cli_label(pattern, builder) {
                    let mut sites = Vec::new();
                    dispatch_calls(&arm.body, &mut sites);
                    self.cli = Some((label, source::line(pattern.span()), sites));
                }
            }
            if conditional {
                self.cond += 1;
            }
            self.locals.push(HashMap::new());
            self.bind_pattern(
                pattern,
                match pattern {
                    Pat::Ident(_) => scrutinee.clone(),
                    _ => match &scrutinee {
                        Ty::Wrapped(_) => scrutinee.clone(),
                        _ => Ty::Unknown,
                    },
                },
            );
            if let Some(guard) = guard {
                self.visit_expr(guard);
            }
            self.visit_expr(&arm.body);
            self.locals.pop();
            if conditional {
                self.cond -= 1;
            }
            self.cli = saved;
        }
    }

    fn visit_expr_while(&mut self, node: &'v syn::ExprWhile) {
        self.in_loop(|walker| {
            walker.locals.push(HashMap::new());
            walker.visit_expr(&node.cond);
            walker.cond += 1;
            walker.visit_block(&node.body);
            walker.cond -= 1;
            walker.locals.pop();
        });
    }

    fn visit_expr_for_loop(&mut self, node: &'v syn::ExprForLoop) {
        self.visit_expr(&node.expr);
        self.in_loop(|walker| {
            walker.locals.push(HashMap::new());
            walker.bind_pattern(&node.pat, Ty::Unknown);
            walker.cond += 1;
            walker.visit_block(&node.body);
            walker.cond -= 1;
            walker.locals.pop();
        });
    }

    fn visit_expr_loop(&mut self, node: &'v syn::ExprLoop) {
        self.in_loop(|walker| walker.visit_block(&node.body));
    }

    fn visit_expr_binary(&mut self, node: &'v syn::ExprBinary) {
        self.visit_expr(&node.left);
        let short_circuit = matches!(node.op, syn::BinOp::And(_) | syn::BinOp::Or(_));
        if short_circuit {
            self.cond += 1;
        }
        self.visit_expr(&node.right);
        if short_circuit {
            self.cond -= 1;
        }
    }

    fn visit_expr_try(&mut self, node: &'v syn::ExprTry) {
        self.visit_expr(&node.expr);
        self.early = true;
        self.returned = true;
    }

    fn visit_expr_return(&mut self, node: &'v syn::ExprReturn) {
        if let Some(expr) = &node.expr {
            self.visit_expr(expr);
        }
        self.early = true;
        self.returned = true;
    }

    fn visit_expr_break(&mut self, node: &'v syn::ExprBreak) {
        if let Some(expr) = &node.expr {
            self.visit_expr(expr);
        }
        self.early = true;
    }

    fn visit_expr_continue(&mut self, _: &'v syn::ExprContinue) {
        self.early = true;
    }

    fn visit_expr_async(&mut self, node: &'v syn::ExprAsync) {
        self.visit_block(&node.block);
    }

    fn visit_local(&mut self, local: &'v syn::Local) {
        let Some(init) = &local.init else {
            self.bind_pattern(&local.pat, Ty::Unknown);
            return;
        };
        let name = match &local.pat {
            Pat::Ident(binding) => Some(ident(&binding.ident)),
            Pat::Type(typed) => match &*typed.pat {
                Pat::Ident(binding) => Some(ident(&binding.ident)),
                _ => None,
            },
            _ => None,
        };
        if let (Some(name), Expr::Closure(closure)) = (&name, strip(&init.expr)) {
            let line = source::line(closure.span());
            let function = self.closure(closure);
            self.reference(function, line);
            self.bind(name.clone(), Local::Closure(function));
            return;
        }
        let ty = match &local.pat {
            Pat::Type(typed) => self.index.resolve_type(self.scope, &typed.ty, &self.ctx),
            _ => self.expr_ty(&init.expr),
        };
        let function = match (strip(&init.expr), &name) {
            (Expr::Path(path), Some(_)) if path.qself.is_none() => {
                match self.resolve(&segments(&path.path), true) {
                    Res::Fn(function) => Some(function),
                    _ => None,
                }
            }
            _ => None,
        };
        self.visit_expr(&init.expr);
        if let Some((_, diverge)) = &init.diverge {
            self.cond += 1;
            self.visit_expr(diverge);
            self.cond -= 1;
            self.early = true;
        }
        match (function, name) {
            (Some(function), Some(name)) => self.bind(name, Local::Function(function)),
            _ => self.bind_pattern(&local.pat, ty),
        }
    }

    fn visit_macro(&mut self, mac: &'v syn::Macro) {
        for statement in source::macro_statements(&mac.tokens) {
            self.visit_stmt(&statement);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn externals_are_named_like_functions() {
        let path = |text: &str| text.split("::").map(str::to_string).collect::<Vec<_>>();
        assert_eq!(
            external_parts(&path("std::collections::HashMap::new")),
            (
                "std::collections::HashMap::new".to_string(),
                "HashMap::new".to_string(),
                "std::collections".to_string()
            )
        );
        assert_eq!(
            external_parts(&path("serde_json::to_string")),
            (
                "serde_json::to_string".to_string(),
                "to_string".to_string(),
                "serde_json".to_string()
            )
        );
    }

    #[test]
    fn subcommands_and_routes_are_written_as_developers_read_them() {
        assert_eq!(kebab("AddUser"), "add-user");
        assert_eq!(kebab("List"), "list");
        assert_eq!(join_route(&["/api/", "/users"]), "/api/users");
        assert_eq!(join_route(&["", "/"]), "/");
        assert_eq!(join_route(&["/v1", "items/{id}"]), "/v1/items/{id}");
    }
}
