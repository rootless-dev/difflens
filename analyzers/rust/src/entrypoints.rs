//! The ways a Rust program is reached from outside, besides the
//! registrations the walk found (`walk`):
//!
//! - `main`: `fn main` at the root of a binary (`#[tokio::main]` included),
//!   labelled with the binary's name; a build script's, labelled so.
//! - `test`: functions with a test attribute (`#[test]`, `#[tokio::test]`,
//!   `#[rstest]`…), labelled with the name the test binary prints.
//! - `exported_api`: the public API of a library: `pub` functions, methods
//!   of `pub` types and default methods of `pub` traits reachable through
//!   `pub` modules or `pub use` re-exports, labelled with the shortest path
//!   a user writes (`shapes::Circle::new`); the `#[proc_macro…]` functions
//!   of a proc-macro crate.
//! - `http_route`: handlers of axum (`.route("/x", get(h))`, under
//!   `.nest`), actix-web (`#[get("/x")]`, `web::resource`, `.route`,
//!   `web::scope`) and rocket (`#[get("/x")]` mounted with `routes!`),
//!   labelled "GET /x"; only for packages that depend on the framework.
//! - `cli_command`: the functions a match on a clap `#[derive(Subcommand)]`
//!   enum (or on `matches.subcommand()`) calls for a subcommand, labelled
//!   "<program> <subcommand>".

use std::collections::{HashMap, HashSet, VecDeque};

use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Token};

use crate::index::{is_test_fn, FnId, Index, ModId, Owner, Res, TypeKind};
use crate::protocol::EntryPoint;
use crate::source::{segments, string_literal};
use crate::walk::Graph;
use crate::workspace::TargetKind;

const ROUTE_VERBS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "head", "options", "trace", "connect",
];

/// The route an actix-web or rocket attribute declares on a function:
/// `#[get("/x")]`, `#[route("/x", method = "GET")]`, `#[rocket::post(…)]`.
pub fn attribute_route(attrs: &[Attribute]) -> Option<(String, String)> {
    for attr in attrs {
        let path = segments(attr.path());
        let (prefix, last) = match path.as_slice() {
            [last] => (None, last.as_str()),
            [prefix, last] => (Some(prefix.as_str()), last.as_str()),
            _ => continue,
        };
        if prefix.is_some_and(|prefix| !matches!(prefix, "actix_web" | "rocket" | "ntex")) {
            continue;
        }
        let Ok(args) = attr.parse_args_with(Punctuated::<Expr, Token![,]>::parse_terminated) else {
            continue;
        };
        if ROUTE_VERBS.contains(&last) {
            let path = args.first().and_then(string_literal)?;
            return Some((last.to_uppercase(), path));
        }
        if last == "route" {
            let mut route_path = None;
            let mut verb = None;
            for arg in &args {
                match arg {
                    Expr::Assign(assign) => {
                        let key = match &*assign.left {
                            Expr::Path(key) => segments(&key.path).join("::"),
                            _ => continue,
                        };
                        match key.as_str() {
                            "method" if verb.is_none() => verb = string_literal(&assign.right),
                            "uri" => route_path = string_literal(&assign.right),
                            _ => {}
                        }
                    }
                    // rocket: #[route(GET, uri = "/")]
                    Expr::Path(method) if verb.is_none() => {
                        verb = segments(&method.path).last().cloned()
                    }
                    other => {
                        if route_path.is_none() {
                            route_path = string_literal(other);
                        }
                    }
                }
            }
            let route_path = route_path?;
            return Some((
                verb.unwrap_or_else(|| "ANY".to_string()).to_uppercase(),
                route_path,
            ));
        }
    }
    None
}

fn proc_macro_kind(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("proc_macro")
            || attr.path().is_ident("proc_macro_derive")
            || attr.path().is_ident("proc_macro_attribute")
    })
}

/// Adds the entry points of every function to `graph.entries`, and labels
/// the routes the walk found with the prefixes of the routers they are
/// nested in.
pub fn detect(index: &Index, graph: &mut Graph) {
    let routed: HashSet<FnId> = graph.routes.iter().map(|route| route.handler).collect();
    for (function, definition) in index.fns.iter().enumerate() {
        let krate = &index.crates[definition.krate];
        let at_root = definition.owner == Owner::Free
            && definition.parent.is_none()
            && definition.scope == krate.module;
        if at_root && definition.name == "main" {
            let label = match krate.kind {
                TargetKind::Bin | TargetKind::Example => Some(krate.target_name.clone()),
                TargetKind::Build => Some(format!("{} build script", krate.package_name)),
                _ => None,
            };
            if let Some(label) = label {
                graph.add_entry(
                    function,
                    EntryPoint {
                        kind: "main".to_string(),
                        label,
                        file: None,
                        line: None,
                    },
                );
            }
        }
        if is_test_fn(definition.attrs) {
            let prefix = format!("{}::", krate.spec.root);
            let label = definition
                .id
                .strip_prefix(&prefix)
                .unwrap_or(&definition.id)
                .to_string();
            graph.add_entry(
                function,
                EntryPoint {
                    kind: "test".to_string(),
                    label,
                    file: None,
                    line: None,
                },
            );
        }
        if (krate.frameworks.actix || krate.frameworks.rocket) && !routed.contains(&function) {
            if let Some((verb, path)) = attribute_route(definition.attrs) {
                graph.add_entry(
                    function,
                    EntryPoint {
                        kind: "http_route".to_string(),
                        label: format!("{verb} {path}"),
                        file: None,
                        line: None,
                    },
                );
            }
        }
        if krate.proc_macro && at_root && proc_macro_kind(definition.attrs) {
            graph.add_entry(
                function,
                EntryPoint {
                    kind: "exported_api".to_string(),
                    label: format!("{}::{}", krate.spec.display, definition.name),
                    file: None,
                    line: None,
                },
            );
        }
    }
    for krate in &index.crates {
        if krate.kind == TargetKind::Lib && !krate.proc_macro && !krate.test_code {
            for (function, label) in exported(index, krate.module, &krate.spec.display) {
                graph.add_entry(
                    function,
                    EntryPoint {
                        kind: "exported_api".to_string(),
                        label,
                        file: None,
                        line: None,
                    },
                );
            }
        }
    }
    label_routes(graph);
}

/// The public functions of a library and the shortest path to each.
fn exported(index: &Index, root: ModId, crate_name: &str) -> Vec<(FnId, String)> {
    let mut found = Vec::new();
    let mut seen_functions = HashSet::new();
    let mut seen_modules = HashSet::new();
    let mut queue = VecDeque::from([(root, crate_name.to_string())]);
    let mut add = |function: FnId, label: String, found: &mut Vec<(FnId, String)>| {
        let definition = &index.fns[function];
        if !definition.test && seen_functions.insert(function) {
            found.push((function, label));
        }
    };
    while let Some((module, prefix)) = queue.pop_front() {
        if !seen_modules.insert(module) || index.modules[module].test {
            continue;
        }
        let current = &index.modules[module];
        let mut fns: Vec<(&String, &FnId)> = current.fns.iter().collect();
        fns.sort();
        for (name, function) in fns {
            if index.fns[*function].public && !hidden(index.fns[*function].attrs) {
                add(*function, format!("{prefix}::{name}"), &mut found);
            }
        }
        let mut types: Vec<(&String, &usize)> = current.types.iter().collect();
        types.sort();
        for (name, type_id) in types {
            if index.types[*type_id].public && !index.types[*type_id].hidden {
                for (method, function) in type_methods(index, *type_id) {
                    add(function, format!("{prefix}::{name}::{method}"), &mut found);
                }
            }
        }
        let mut traits: Vec<(&String, &usize)> = current.traits.iter().collect();
        traits.sort();
        for (name, trait_id) in traits {
            let definition = &index.traits[*trait_id];
            if definition.public && !definition.hidden {
                let mut methods: Vec<(&String, &Option<FnId>)> =
                    definition.methods.iter().collect();
                methods.sort();
                for (method, default) in methods {
                    if let Some(function) = default {
                        add(*function, format!("{prefix}::{name}::{method}"), &mut found);
                    }
                }
            }
        }
        let mut children: Vec<(&String, &ModId)> = current.children.iter().collect();
        children.sort();
        for (name, child) in children {
            if index.modules[*child].public {
                queue.push_back((*child, format!("{prefix}::{name}")));
            }
        }
        let mut uses: Vec<(&String, &crate::index::Use)> = current
            .uses
            .iter()
            .filter(|(_, used)| used.public)
            .collect();
        uses.sort_by(|left, right| left.0.cmp(right.0));
        for (alias, used) in uses {
            match index.resolve(module, &used.path, None, false) {
                Res::Module(target) => queue.push_back((target, format!("{prefix}::{alias}"))),
                Res::Fn(function) => add(function, format!("{prefix}::{alias}"), &mut found),
                Res::Type(type_id) => {
                    for (method, function) in type_methods(index, type_id) {
                        add(function, format!("{prefix}::{alias}::{method}"), &mut found);
                    }
                }
                _ => {}
            }
            if let Res::Fn(function) = index.resolve(module, &used.path, None, true) {
                add(function, format!("{prefix}::{alias}"), &mut found);
            }
        }
        for glob in current.globs.iter().filter(|glob| glob.public) {
            if let Res::Module(target) = index.resolve(module, &glob.path, None, false) {
                queue.push_back((target, prefix.clone()));
            }
        }
    }
    found
}

/// The public inherent methods of a type, by name.
fn type_methods(index: &Index, type_id: usize) -> Vec<(String, FnId)> {
    if matches!(index.types[type_id].kind, TypeKind::Alias(_)) {
        return Vec::new();
    }
    let mut methods: Vec<(String, FnId)> = index
        .inherent
        .get(&type_id)
        .into_iter()
        .flatten()
        .flat_map(|impl_id| index.impls[*impl_id].methods.iter())
        .filter(|(_, function)| {
            index.fns[**function].public && !hidden(index.fns[**function].attrs)
        })
        .map(|(name, function)| (name.clone(), *function))
        .collect();
    methods.sort();
    methods
}

fn hidden(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("doc") && matches!(&attr.meta, syn::Meta::List(list) if list.tokens.to_string().contains("hidden"))
    })
}

/// Labels each route with the prefixes of the routers it is nested in,
/// across functions (`.nest("/api", api::router())`), as its handler's
/// entry point and its registration's evidence.
fn label_routes(graph: &mut Graph) {
    // The first nesting of each router function: its prefix, and the
    // function nesting it (which may be nested itself).
    let mut nested: HashMap<FnId, (String, FnId)> = HashMap::new();
    for (router, prefix, caller) in &graph.nests {
        nested
            .entry(*router)
            .or_insert_with(|| (prefix.clone(), *caller));
    }
    let full_prefix = |mut function: FnId| {
        let mut parts = Vec::new();
        let mut seen = HashSet::new();
        while let Some((prefix, caller)) = nested.get(&function) {
            if !seen.insert(function) {
                break;
            }
            parts.push(prefix.trim_end_matches('/').to_string());
            function = *caller;
        }
        parts.reverse();
        parts.concat()
    };
    let routes = graph.routes.clone();
    for route in routes {
        let prefix = full_prefix(route.caller);
        let path = if prefix.is_empty() {
            route.path.clone()
        } else {
            format!("{}{}", prefix.trim_end_matches('/'), route.path)
        };
        let label = format!("{} {path}", route.verb);
        if let Some(call) = route.call {
            graph.calls[call].evidence = Some(format!("route {label}"));
        }
        graph.add_entry(
            route.handler,
            EntryPoint {
                kind: "http_route".to_string(),
                label,
                file: Some(route.file.clone()),
                line: Some(route.line),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_attributes_of_actix_and_rocket_are_read() {
        let function: syn::ItemFn = syn::parse_quote! {
            #[get("/users/{id}")]
            async fn user() {}
        };
        assert_eq!(
            attribute_route(&function.attrs),
            Some(("GET".to_string(), "/users/{id}".to_string()))
        );
        let function: syn::ItemFn = syn::parse_quote! {
            #[actix_web::route("/items", method = "POST", method = "PUT")]
            async fn items() {}
        };
        assert_eq!(
            attribute_route(&function.attrs),
            Some(("POST".to_string(), "/items".to_string()))
        );
        let function: syn::ItemFn = syn::parse_quote! {
            #[route(GET, uri = "/")]
            fn index() {}
        };
        assert_eq!(
            attribute_route(&function.attrs),
            Some(("GET".to_string(), "/".to_string()))
        );
        let function: syn::ItemFn = syn::parse_quote! {
            #[other::get("/x")]
            fn not_a_route() {}
        };
        assert_eq!(attribute_route(&function.attrs), None);
    }
}
