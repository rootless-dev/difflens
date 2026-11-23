//! The crates of a source tree as syntax: their module trees (following
//! `mod x;` to its file, `#[path]` included), the items of every module
//! (functions, types, traits, impls, `use` imports), the stable id of every
//! function, and name resolution over all of it.
//!
//! Ids are built from syntax only, so they do not depend on the order of
//! unrelated declarations or on line numbers:
//!
//! ```text
//! function          <module path>::<name>                  shapes::area::total
//! inherent method   <module path>::<Type>::<name>          shapes::Circle::new
//! trait impl method <module path>::<<Type> as <Trait>>::<name>
//!                                                          shapes::<Circle as Shape>::area
//! trait default     <module path>::<Trait>::<name>         shapes::Shape::describe
//! nested function   <parent id>::<name>
//! closure           <parent id>$<k>                        shapes::total$1, …$1$2
//! static closure    <module path>::<STATIC>$<k>
//! ```
//!
//! The module path starts with the crate root (see `crate_roots`): the
//! library's crate name, or `<package>/<tests|bin|…>/<target>` for the other
//! targets of a package, so a test binary's names (`billing::tests::refunds`)
//! map to ids by prefixing the root. A closure's k is its position among the
//! closures written directly inside its parent, in source order. The same id
//! declared twice (`#[cfg]` variants) is kept once, the first.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Attribute, Expr, Item, Visibility};

use crate::protocol::Diagnostic;
use crate::source::{self, ident, is_type_like, segments};
use crate::workspace::{Package, TargetKind, Tree, Workspace};

pub type ModId = usize;
pub type FnId = usize;
pub type TypeId = usize;
pub type TraitId = usize;
pub type ImplId = usize;

/// One parsed file of the tree.
pub struct SourceFile {
    pub rel: String,
    pub syntax: Option<syn::File>,
    pub generated: bool,
}

/// Every file reached from a crate root, parsed once.
#[derive(Default)]
pub struct Files {
    pub list: Vec<SourceFile>,
    by_path: HashMap<PathBuf, usize>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Files {
    fn index_of(&self, path: &Path) -> Option<usize> {
        self.by_path
            .get(&crate::workspace::normalize(path))
            .copied()
    }

    /// Parses `path` (once) and returns its index; None when it is not a
    /// readable file of the tree.
    fn load(&mut self, tree: &Tree, path: &Path) -> Option<usize> {
        let path = crate::workspace::normalize(path);
        if let Some(index) = self.by_path.get(&path) {
            return Some(*index);
        }
        let rel = tree.rel(&path)?;
        let text = std::fs::read_to_string(&path).ok()?;
        let syntax = match syn::parse_file(&text) {
            Ok(file) => Some(file),
            Err(error) => {
                self.diagnostics.push(Diagnostic {
                    level: "warning".to_string(),
                    message: format!(
                        "{rel} could not be parsed, so its functions are not listed: {error}"
                    ),
                    file: Some(rel.clone()),
                    line: Some(source::line(error.span())),
                });
                None
            }
        };
        let index = self.list.len();
        self.list.push(SourceFile {
            rel,
            generated: source::is_generated(&text),
            syntax,
        });
        self.by_path.insert(path, index);
        Some(index)
    }
}

/// The id prefix and display root of one crate (a target of a package).
#[derive(Debug, Clone)]
pub struct CrateSpec {
    /// Root of ids and containers: "shapes", "shapes/tests/api".
    pub root: String,
    /// First segment of qualified names: the crate name rustc gives it.
    pub display: String,
    pub unit: usize,
    pub package: usize,
    pub target: usize,
}

/// The crates of the workspace in a fixed order (per package: library,
/// binaries, tests, benches, examples, build script), each with its root.
pub fn crate_roots(workspace: &Workspace) -> Vec<CrateSpec> {
    let mut specs: Vec<CrateSpec> = Vec::new();
    let mut used = HashSet::new();
    for (unit_index, unit) in workspace.units.iter().enumerate() {
        for (package_index, package) in unit.packages.iter().enumerate() {
            let mut order: Vec<usize> = (0..package.targets.len()).collect();
            order.sort_by_key(|index| {
                (
                    package.targets[*index].kind,
                    package.targets[*index].name.clone(),
                )
            });
            let base = package.name.replace('-', "_");
            for target_index in order {
                let target = &package.targets[target_index];
                let mut root = match target.kind {
                    TargetKind::Lib => target.crate_name.clone(),
                    TargetKind::Bin if package.lib().is_none() && target.name == package.name => {
                        target.crate_name.clone()
                    }
                    TargetKind::Bin => format!("{base}/bin/{}", target.name),
                    TargetKind::Test => format!("{base}/tests/{}", target.name),
                    TargetKind::Bench => format!("{base}/benches/{}", target.name),
                    TargetKind::Example => format!("{base}/examples/{}", target.name),
                    TargetKind::Build => format!("{base}/build"),
                };
                if !used.insert(root.clone()) {
                    let mut n = 2;
                    while !used.insert(format!("{root}#{n}")) {
                        n += 1;
                    }
                    root = format!("{root}#{n}");
                }
                let display = match target.kind {
                    TargetKind::Build => "build".to_string(),
                    _ => target.crate_name.clone(),
                };
                specs.push(CrateSpec {
                    root,
                    display,
                    unit: unit_index,
                    package: package_index,
                    target: target_index,
                });
            }
        }
    }
    specs
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Frameworks {
    pub axum: bool,
    pub actix: bool,
    pub rocket: bool,
    pub clap: bool,
}

impl Frameworks {
    fn of(package: &Package) -> Self {
        Self {
            axum: package.depends_on("axum"),
            actix: package.depends_on("actix-web"),
            rocket: package.depends_on("rocket"),
            clap: package.depends_on("clap"),
        }
    }

    pub fn http(self) -> bool {
        self.axum || self.actix || self.rocket
    }
}

pub struct Crate {
    pub spec: CrateSpec,
    pub kind: TargetKind,
    pub target_name: String,
    pub package_name: String,
    pub module: ModId,
    /// Everything in it is test code (an integration test or a bench).
    pub test_code: bool,
    pub proc_macro: bool,
    /// Names of other crates its code can use: workspace crates by index,
    /// dependencies outside the tree as None.
    pub externs: HashMap<String, Option<usize>>,
    pub frameworks: Frameworks,
}

pub struct Use {
    pub path: Vec<String>,
    pub public: bool,
}

pub struct Module {
    /// Prefix of the ids declared in it: "shapes::area"; for the block of a
    /// function body, the function's id.
    pub path: String,
    /// The module path functions declared in it report as container.
    pub container: String,
    /// Last segment of `container`, for qualified names.
    pub short: String,
    pub krate: usize,
    pub parent: Option<ModId>,
    /// The body of a function that declares items.
    pub block: bool,
    pub file: usize,
    pub test: bool,
    /// Declared `pub` (for the root of a library: true).
    pub public: bool,
    pub children: HashMap<String, ModId>,
    pub fns: HashMap<String, FnId>,
    pub types: HashMap<String, TypeId>,
    pub traits: HashMap<String, TraitId>,
    pub uses: HashMap<String, Use>,
    pub globs: Vec<Use>,
}

pub enum TypeKind<'a> {
    Struct(HashMap<String, &'a syn::Type>),
    Enum {
        variants: Vec<(String, Option<String>)>,
        subcommand: bool,
    },
    Alias(&'a syn::Type),
    Union,
}

pub struct TypeDef<'a> {
    pub module: ModId,
    pub kind: TypeKind<'a>,
    pub public: bool,
    pub hidden: bool,
}

pub struct TraitDef {
    pub name: String,
    pub module: ModId,
    pub public: bool,
    pub hidden: bool,
    /// Every method it declares, with its default body if it has one.
    pub methods: HashMap<String, Option<FnId>>,
    pub supertraits: Vec<Vec<String>>,
    pub resolved_supertraits: Vec<TraitId>,
    pub impls: Vec<ImplId>,
}

pub struct ImplDef<'a> {
    pub module: ModId,
    pub self_ty: &'a syn::Type,
    pub trait_path: Option<Vec<String>>,
    pub methods: HashMap<String, FnId>,
    pub self_type: Option<TypeId>,
    pub trait_id: Option<TraitId>,
    pub generics: &'a syn::Generics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Free,
    Impl(ImplId),
    Trait(TraitId),
    Closure,
}

pub struct FnDef<'a> {
    pub id: String,
    pub name: String,
    pub qualified: String,
    pub container: String,
    pub file: usize,
    pub start: u32,
    pub end: u32,
    pub test: bool,
    pub generated: bool,
    pub complexity: Option<u32>,
    pub owner: Owner,
    /// Where names in its body resolve: its body's block module, or the
    /// module it is declared in.
    pub scope: ModId,
    pub krate: usize,
    pub sig: Option<&'a syn::Signature>,
    pub body: Option<&'a syn::Block>,
    pub attrs: &'a [Attribute],
    pub public: bool,
    pub parent: Option<FnId>,
}

/// A `const` or `static` initializer, walked for the closures it holds.
pub struct StaticDef<'a> {
    pub module: ModId,
    pub name: String,
    pub expr: &'a Expr,
    pub test: bool,
}

/// What a path names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Res {
    Module(ModId),
    Fn(FnId),
    Type(TypeId),
    Trait(TraitId),
    TraitMethod(TraitId, String),
    /// An enum variant, a constant or an associated item that is not a
    /// function.
    Other,
    /// Something outside the tree, as a path ("std::collections::HashMap").
    External(Vec<String>),
    Unknown,
}

/// What a value is, as far as syntax tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Named(TypeId),
    /// A trait object, `impl Trait` or a generic parameter: any type with
    /// these traits of the tree.
    Dyn(Vec<TraitId>),
    External(Vec<String>),
    /// A trait object or bound of a trait outside the tree.
    ExternalDyn(Vec<String>),
    /// `Option<T>` or `Result<T, E>`: `?`, `unwrap` and `expect` give T.
    Wrapped(Box<Ty>),
    Unknown,
}

/// The type context names in a function resolve in.
#[derive(Debug, Clone, Default)]
pub struct TyCtx {
    pub self_ty: Option<Ty>,
    pub generics: HashMap<String, Ty>,
}

pub struct Index<'a> {
    pub files: &'a Files,
    pub crates: Vec<Crate>,
    pub modules: Vec<Module>,
    pub fns: Vec<FnDef<'a>>,
    pub types: Vec<TypeDef<'a>>,
    pub traits: Vec<TraitDef>,
    pub impls: Vec<ImplDef<'a>>,
    pub statics: Vec<StaticDef<'a>>,
    pub by_id: HashMap<String, FnId>,
    pub inherent: HashMap<TypeId, Vec<ImplId>>,
    pub trait_impls: HashMap<TypeId, Vec<ImplId>>,
    pub methods_by_name: HashMap<String, Vec<FnId>>,
    pub diagnostics: Vec<Diagnostic>,
    /// Line ranges of test code per file (test modules and functions), which
    /// coverage leaves out.
    pub test_spans: HashMap<usize, Vec<(u32, u32)>>,
}

/// Parses every file reachable from the crate roots of `workspace`.
pub fn load_files(tree: &Tree, workspace: &Workspace, specs: &[CrateSpec]) -> Files {
    let mut files = Files::default();
    for spec in specs {
        let target = &workspace.units[spec.unit].packages[spec.package].targets[spec.target];
        if let Some(index) = files.load(tree, &target.src_path) {
            let dir = target
                .src_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            discover(&mut files, tree, index, &dir);
        }
    }
    files
}

/// Loads the files of the `mod x;` declarations of a file, recursively.
fn discover(files: &mut Files, tree: &Tree, file: usize, children_dir: &Path) {
    let mut requests = Vec::new();
    let file_dir = files_dir(files, file, tree);
    if let Some(syntax) = &files.list[file].syntax {
        module_requests(&syntax.items, children_dir, &file_dir, false, &mut requests);
    }
    for (candidates, path_attr) in requests {
        for candidate in candidates {
            if !candidate.is_file() {
                continue;
            }
            let first_time = files.index_of(&candidate).is_none();
            if let Some(index) = files.load(tree, &candidate) {
                if first_time {
                    discover(files, tree, index, &children_dir_of(&candidate, path_attr));
                }
            }
            break;
        }
    }
}

fn files_dir(files: &Files, file: usize, tree: &Tree) -> PathBuf {
    let rel = &files.list[file].rel;
    tree.root
        .join(rel)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// The files that may hold the modules declared (without a body) among
/// `items`, with whether each came from a `#[path]`.
fn module_requests(
    items: &[Item],
    children_dir: &Path,
    file_dir: &Path,
    inline: bool,
    out: &mut Vec<(Vec<PathBuf>, bool)>,
) {
    for item in items {
        let Item::Mod(module) = item else { continue };
        let name = ident(&module.ident);
        match &module.content {
            Some((_, inner)) => {
                module_requests(inner, &children_dir.join(&name), file_dir, true, out)
            }
            None => {
                let path_attr = path_attribute(&module.attrs);
                out.push((
                    module_candidates(children_dir, file_dir, inline, &name, path_attr.as_deref()),
                    path_attr.is_some(),
                ));
            }
        }
    }
}

fn module_candidates(
    children_dir: &Path,
    file_dir: &Path,
    inline: bool,
    name: &str,
    path_attr: Option<&str>,
) -> Vec<PathBuf> {
    match path_attr {
        Some(path) if inline => vec![children_dir.join(path)],
        Some(path) => vec![file_dir.join(path)],
        None => vec![
            children_dir.join(format!("{name}.rs")),
            children_dir.join(name).join("mod.rs"),
        ],
    }
}

/// Where the modules a module file declares live: beside a `mod.rs` (or a
/// file named by `#[path]`), else in the folder named after the file.
fn children_dir_of(file: &Path, path_attr: bool) -> PathBuf {
    let parent = file.parent().map(Path::to_path_buf).unwrap_or_default();
    if path_attr || file.file_name().is_some_and(|name| name == "mod.rs") {
        return parent;
    }
    match file.file_stem() {
        Some(stem) => parent.join(stem),
        None => parent,
    }
}

fn path_attribute(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        match &attr.meta {
            syn::Meta::NameValue(value) => source::string_literal(&value.value),
            _ => None,
        }
    })
}

pub fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// `#[cfg(test)]`, or `#[cfg(all(test, …))]`.
pub fn is_cfg_test(attrs: &[Attribute]) -> bool {
    fn predicate(tokens: proc_macro2::TokenStream) -> bool {
        let trees: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
        match trees.as_slice() {
            [proc_macro2::TokenTree::Ident(word)] => word == "test",
            [proc_macro2::TokenTree::Ident(word), proc_macro2::TokenTree::Group(group)]
                if word == "all" =>
            {
                group.stream().into_iter().any(
                    |tree| matches!(&tree, proc_macro2::TokenTree::Ident(word) if word == "test"),
                )
            }
            _ => false,
        }
    }
    attrs.iter().any(|attr| match &attr.meta {
        syn::Meta::List(list) if list.path.is_ident("cfg") => predicate(list.tokens.clone()),
        _ => false,
    })
}

/// Attributes that make a function a test (libtest's own and the common
/// test macros: tokio, async-std, actix, sqlx, rstest, test-case…).
pub fn is_test_fn(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = segments(attr.path());
        let last = path.last().map(String::as_str).unwrap_or("");
        matches!(
            last,
            "test"
                | "bench"
                | "rstest"
                | "test_case"
                | "wasm_bindgen_test"
                | "quickcheck"
                | "proptest"
        ) && !(path.len() == 1 && last == "proptest")
    })
}

fn doc_hidden(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("doc")
            && matches!(&attr.meta, syn::Meta::List(list) if list.tokens.to_string().contains("hidden"))
    })
}

/// The names a `#[derive(…)]` lists (their last segment).
pub fn derives(attrs: &[Attribute]) -> Vec<String> {
    let mut names = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("derive")) {
        let _ = attr.parse_nested_meta(|meta| {
            if let Some(last) = meta.path.segments.last() {
                names.push(ident(&last.ident));
            }
            Ok(())
        });
    }
    names
}

/// The `name = "…"` of a `#[command(…)]` or `#[clap(…)]` attribute.
fn clap_name(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs
        .iter()
        .filter(|attr| attr.path().is_ident("command") || attr.path().is_ident("clap"))
    {
        let mut found = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                if let Ok(value) = meta.value() {
                    if let Ok(syn::Lit::Str(text)) = value.parse::<syn::Lit>() {
                        found = Some(text.value());
                    }
                }
            } else if meta.input.peek(syn::Token![=]) {
                let _ = meta.value().and_then(|value| value.parse::<Expr>());
            }
            Ok(())
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

/// A type as an impl's id writes it: its last path segment, without generic
/// arguments ("Circle", "&Circle", "[T]", "dyn Shape").
pub fn type_label(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| ident(&segment.ident))
            .unwrap_or_else(|| "_".to_string()),
        syn::Type::Reference(reference) => format!("&{}", type_label(&reference.elem)),
        syn::Type::Ptr(pointer) => format!("*{}", type_label(&pointer.elem)),
        syn::Type::Slice(slice) => format!("[{}]", type_label(&slice.elem)),
        syn::Type::Array(array) => format!("[{}; _]", type_label(&array.elem)),
        syn::Type::Paren(inner) => type_label(&inner.elem),
        syn::Type::Group(inner) => type_label(&inner.elem),
        syn::Type::Tuple(tuple) => format!(
            "({})",
            tuple
                .elems
                .iter()
                .map(type_label)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        syn::Type::TraitObject(object) => format!("dyn {}", bounds_label(&object.bounds)),
        syn::Type::ImplTrait(implementation) => {
            format!("impl {}", bounds_label(&implementation.bounds))
        }
        syn::Type::Never(_) => "!".to_string(),
        _ => "_".to_string(),
    }
}

fn bounds_label(
    bounds: &syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>,
) -> String {
    bounds
        .iter()
        .filter_map(|bound| match bound {
            syn::TypeParamBound::Trait(bound) => bound
                .path
                .segments
                .last()
                .map(|segment| ident(&segment.ident)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// The names the standard prelude brings into every module.
fn prelude(name: &str) -> Option<Vec<String>> {
    let path: &[&str] = match name {
        "Vec" => &["std", "vec", "Vec"],
        "String" => &["std", "string", "String"],
        "Box" => &["std", "boxed", "Box"],
        "Option" => &["std", "option", "Option"],
        "Result" => &["std", "result", "Result"],
        "Some" | "None" => &["std", "option", "Option"],
        "Ok" | "Err" => &["std", "result", "Result"],
        "ToString" => &["std", "string", "ToString"],
        "ToOwned" => &["std", "borrow", "ToOwned"],
        "Default" => &["std", "default", "Default"],
        "Clone" => &["std", "clone", "Clone"],
        "Iterator" => &["std", "iter", "Iterator"],
        "IntoIterator" => &["std", "iter", "IntoIterator"],
        "From" => &["std", "convert", "From"],
        "Into" => &["std", "convert", "Into"],
        "drop" => &["std", "mem", "drop"],
        _ => return None,
    };
    let mut path: Vec<String> = path.iter().map(|part| part.to_string()).collect();
    if matches!(name, "Some" | "None" | "Ok" | "Err") {
        path.push(name.to_string());
    }
    Some(path)
}

/// Finds the items declared inside a function body (nested functions,
/// types, impls), without entering nested items' own bodies.
#[derive(Default)]
struct NestedItems<'a> {
    items: Vec<&'a Item>,
}

impl<'a> Visit<'a> for NestedItems<'a> {
    fn visit_item(&mut self, item: &'a Item) {
        self.items.push(item);
    }
}

struct Builder<'a> {
    tree: &'a Tree,
    workspace: &'a Workspace,
    index: Index<'a>,
    file_modules: HashMap<usize, ModId>,
}

/// Builds the index of the crates of `workspace` from `files`.
pub fn build<'a>(
    tree: &'a Tree,
    workspace: &'a Workspace,
    specs: &[CrateSpec],
    files: &'a Files,
) -> Index<'a> {
    let mut builder = Builder {
        tree,
        workspace,
        index: Index {
            files,
            crates: Vec::new(),
            modules: Vec::new(),
            fns: Vec::new(),
            types: Vec::new(),
            traits: Vec::new(),
            impls: Vec::new(),
            statics: Vec::new(),
            by_id: HashMap::new(),
            inherent: HashMap::new(),
            trait_impls: HashMap::new(),
            methods_by_name: HashMap::new(),
            diagnostics: Vec::new(),
            test_spans: HashMap::new(),
        },
        file_modules: HashMap::new(),
    };
    // Crates first, so that every crate knows the others by name.
    for spec in specs {
        builder.add_crate(spec.clone());
    }
    builder.link_externs();
    for krate in 0..builder.index.crates.len() {
        builder.crate_items(krate);
    }
    builder.resolve_impls();
    builder.index
}

impl<'a> Builder<'a> {
    fn package(&self, spec: &CrateSpec) -> &'a Package {
        &self.workspace.units[spec.unit].packages[spec.package]
    }

    fn add_crate(&mut self, spec: CrateSpec) {
        let package = self.package(&spec);
        let target = &package.targets[spec.target];
        let krate = self.index.crates.len();
        let file = self
            .index
            .files
            .index_of(&target.src_path)
            .unwrap_or(usize::MAX);
        let test_code = matches!(target.kind, TargetKind::Test | TargetKind::Bench);
        let module = self.index.modules.len();
        self.index.modules.push(Module {
            path: spec.root.clone(),
            container: spec.root.clone(),
            short: spec.display.clone(),
            krate,
            parent: None,
            block: false,
            file,
            test: test_code,
            public: target.kind == TargetKind::Lib,
            children: HashMap::new(),
            fns: HashMap::new(),
            types: HashMap::new(),
            traits: HashMap::new(),
            uses: HashMap::new(),
            globs: Vec::new(),
        });
        self.index.crates.push(Crate {
            kind: target.kind,
            target_name: target.name.clone(),
            package_name: package.name.clone(),
            module,
            test_code,
            proc_macro: target.proc_macro,
            externs: HashMap::new(),
            frameworks: Frameworks::of(package),
            spec,
        });
    }

    /// Every crate sees its package's library, and the workspace libraries
    /// it depends on, by their crate names; other dependencies are outside.
    fn link_externs(&mut self) {
        let libs: HashMap<String, usize> = self
            .index
            .crates
            .iter()
            .enumerate()
            .filter(|(_, krate)| krate.kind == TargetKind::Lib)
            .map(|(index, krate)| (krate.package_name.clone(), index))
            .collect();
        for krate in 0..self.index.crates.len() {
            let spec = self.index.crates[krate].spec.clone();
            let package = self.package(&spec);
            let mut externs = HashMap::new();
            for dependency in &package.dependencies {
                externs.insert(
                    dependency.code_name.clone(),
                    libs.get(&dependency.package).copied(),
                );
            }
            if self.index.crates[krate].kind != TargetKind::Lib {
                if let (Some(lib), Some(target)) = (libs.get(&package.name), package.lib()) {
                    externs.insert(target.crate_name.clone(), Some(*lib));
                }
            }
            self.index.crates[krate].externs = externs;
        }
    }

    fn crate_items(&mut self, krate: usize) {
        let module = self.index.crates[krate].module;
        let file = self.index.modules[module].file;
        let Some(source) = self.index.files.list.get(file) else {
            return;
        };
        let Some(syntax) = &source.syntax else { return };
        let dir = self
            .tree
            .root
            .join(&source.rel)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let test = self.index.modules[module].test;
        if self.file_modules.contains_key(&file) {
            // Two targets with the same root file: the first lists it.
            return;
        }
        if test {
            self.mark_test_span(file, (1, u32::MAX));
        }
        self.file_modules.insert(file, module);
        self.items(module, &syntax.items, file, &dir, &dir, false, test);
    }

    #[allow(clippy::too_many_arguments)]
    fn items(
        &mut self,
        module: ModId,
        items: &'a [Item],
        file: usize,
        children_dir: &Path,
        file_dir: &Path,
        inline: bool,
        test: bool,
    ) {
        for item in items {
            match item {
                Item::Mod(declared) => {
                    self.module_item(module, declared, file, children_dir, file_dir, inline, test)
                }
                Item::Fn(function) => {
                    let test = test || is_cfg_test(&function.attrs) || is_test_fn(&function.attrs);
                    self.free_fn(
                        module,
                        &function.sig,
                        &function.block,
                        &function.attrs,
                        &function.vis,
                        file,
                        test,
                        None,
                    );
                    if test {
                        self.mark_test_span(file, source::lines(function));
                    }
                }
                Item::Impl(implementation) => self.impl_item(module, implementation, file, test),
                Item::Trait(definition) => self.trait_item(module, definition, file, test),
                Item::Struct(definition) => {
                    let fields = definition
                        .fields
                        .iter()
                        .enumerate()
                        .map(|(position, field)| {
                            (
                                field
                                    .ident
                                    .as_ref()
                                    .map(ident)
                                    .unwrap_or_else(|| position.to_string()),
                                &field.ty,
                            )
                        })
                        .collect();
                    self.add_type(
                        module,
                        ident(&definition.ident),
                        TypeKind::Struct(fields),
                        &definition.vis,
                        &definition.attrs,
                    );
                }
                Item::Enum(definition) => {
                    let variants = definition
                        .variants
                        .iter()
                        .map(|variant| (ident(&variant.ident), clap_name(&variant.attrs)))
                        .collect();
                    let subcommand = derives(&definition.attrs)
                        .iter()
                        .any(|name| name == "Subcommand");
                    self.add_type(
                        module,
                        ident(&definition.ident),
                        TypeKind::Enum {
                            variants,
                            subcommand,
                        },
                        &definition.vis,
                        &definition.attrs,
                    );
                }
                Item::Union(definition) => {
                    self.add_type(
                        module,
                        ident(&definition.ident),
                        TypeKind::Union,
                        &definition.vis,
                        &definition.attrs,
                    );
                }
                Item::Type(alias) => {
                    self.add_type(
                        module,
                        ident(&alias.ident),
                        TypeKind::Alias(&alias.ty),
                        &alias.vis,
                        &alias.attrs,
                    );
                }
                Item::Use(declaration) => {
                    let public = is_pub(&declaration.vis);
                    let mut prefix = Vec::new();
                    if declaration.leading_colon.is_some() {
                        prefix.push("::".to_string());
                    }
                    self.use_tree(module, &declaration.tree, &mut prefix, public);
                }
                Item::Const(constant) => self.index.statics.push(StaticDef {
                    module,
                    name: ident(&constant.ident),
                    expr: &constant.expr,
                    test,
                }),
                Item::Static(value) => self.index.statics.push(StaticDef {
                    module,
                    name: ident(&value.ident),
                    expr: &value.expr,
                    test,
                }),
                _ => {}
            }
        }
    }

    fn mark_test_span(&mut self, file: usize, span: (u32, u32)) {
        self.index.test_spans.entry(file).or_default().push(span);
    }

    #[allow(clippy::too_many_arguments)]
    fn module_item(
        &mut self,
        parent: ModId,
        declared: &'a syn::ItemMod,
        file: usize,
        children_dir: &Path,
        file_dir: &Path,
        inline: bool,
        test: bool,
    ) {
        let name = ident(&declared.ident);
        let test = test || is_cfg_test(&declared.attrs);
        let public = self.index.modules[parent].public && is_pub(&declared.vis);
        match &declared.content {
            Some((_, items)) => {
                let module = self.add_module(parent, &name, file, test, public);
                if test {
                    self.mark_test_span(file, source::lines(declared));
                }
                self.items(
                    module,
                    items,
                    file,
                    &children_dir.join(&name),
                    file_dir,
                    true,
                    test,
                );
            }
            None => {
                let path_attr = path_attribute(&declared.attrs);
                let candidates =
                    module_candidates(children_dir, file_dir, inline, &name, path_attr.as_deref());
                let Some((path, child)) = candidates.iter().find_map(|candidate| {
                    self.index
                        .files
                        .index_of(candidate)
                        .map(|index| (candidate.clone(), index))
                }) else {
                    let known = candidates.iter().any(|candidate| candidate.is_file());
                    if !known && !is_cfg_gated(&declared.attrs) {
                        let rel = &self.index.files.list[file].rel;
                        self.index.diagnostics.push(Diagnostic {
                            level: "warning".to_string(),
                            message: format!(
                                "the file of module {name}, declared in {rel}, was not found"
                            ),
                            file: Some(rel.clone()),
                            line: Some(source::line(declared.ident.span())),
                        });
                    }
                    return;
                };
                if let Some(existing) = self.file_modules.get(&child) {
                    // The same file used by another crate (a `mod common;` of
                    // several tests): its functions keep their first ids.
                    let existing = *existing;
                    self.index.modules[parent].children.insert(name, existing);
                    return;
                }
                let module = self.add_module(parent, &name, child, test, public);
                self.file_modules.insert(child, module);
                let Some(syntax) = &self.index.files.list[child].syntax else {
                    return;
                };
                let child_dir = children_dir_of(&path, path_attr.is_some());
                let child_file_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                if test {
                    self.mark_test_span(child, (1, u32::MAX));
                }
                self.items(
                    module,
                    &syntax.items,
                    child,
                    &child_dir,
                    &child_file_dir,
                    false,
                    test,
                );
            }
        }
    }

    fn add_module(
        &mut self,
        parent: ModId,
        name: &str,
        file: usize,
        test: bool,
        public: bool,
    ) -> ModId {
        let id = self.index.modules.len();
        let parent_module = &self.index.modules[parent];
        let path = format!("{}::{name}", parent_module.path);
        let krate = parent_module.krate;
        self.index.modules.push(Module {
            container: path.clone(),
            path,
            short: name.to_string(),
            krate,
            parent: Some(parent),
            block: false,
            file,
            test,
            public,
            children: HashMap::new(),
            fns: HashMap::new(),
            types: HashMap::new(),
            traits: HashMap::new(),
            uses: HashMap::new(),
            globs: Vec::new(),
        });
        self.index.modules[parent]
            .children
            .insert(name.to_string(), id);
        id
    }

    fn add_type(
        &mut self,
        module: ModId,
        name: String,
        kind: TypeKind<'a>,
        vis: &Visibility,
        attrs: &[Attribute],
    ) {
        let id = self.index.types.len();
        self.index.types.push(TypeDef {
            module,
            kind,
            public: is_pub(vis),
            hidden: doc_hidden(attrs),
        });
        self.index.modules[module].types.entry(name).or_insert(id);
    }

    fn use_tree(
        &mut self,
        module: ModId,
        tree: &syn::UseTree,
        prefix: &mut Vec<String>,
        public: bool,
    ) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(ident(&path.ident));
                self.use_tree(module, &path.tree, prefix, public);
                prefix.pop();
            }
            syn::UseTree::Name(name) => {
                let name = ident(&name.ident);
                let (alias, path) = if name == "self" {
                    (prefix.last().cloned().unwrap_or_default(), prefix.clone())
                } else {
                    let mut path = prefix.clone();
                    path.push(name.clone());
                    (name, path)
                };
                self.index.modules[module]
                    .uses
                    .insert(alias, Use { path, public });
            }
            syn::UseTree::Rename(rename) => {
                let mut path = prefix.clone();
                let name = ident(&rename.ident);
                if name != "self" {
                    path.push(name);
                }
                let alias = ident(&rename.rename);
                if alias != "_" {
                    self.index.modules[module]
                        .uses
                        .insert(alias, Use { path, public });
                }
            }
            syn::UseTree::Glob(_) => self.index.modules[module].globs.push(Use {
                path: prefix.clone(),
                public,
            }),
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.use_tree(module, item, prefix, public);
                }
            }
        }
    }

    /// Declares a function and the items its body declares.
    #[allow(clippy::too_many_arguments)]
    fn free_fn(
        &mut self,
        module: ModId,
        sig: &'a syn::Signature,
        body: &'a syn::Block,
        attrs: &'a [Attribute],
        vis: &Visibility,
        file: usize,
        test: bool,
        parent: Option<FnId>,
    ) -> FnId {
        let name = ident(&sig.ident);
        let (id, display) = match parent {
            Some(parent) => (
                format!("{}::{name}", self.index.fns[parent].id),
                format!("{}::{name}", self.index.fns[parent].name),
            ),
            None => (
                format!("{}::{name}", self.index.modules[module].path),
                name.clone(),
            ),
        };
        let function = self.add_fn(
            module,
            id,
            display,
            Owner::Free,
            sig,
            Some(body),
            attrs,
            is_pub(vis),
            file,
            test,
            parent,
        );
        self.index.modules[module]
            .fns
            .entry(name)
            .or_insert(function);
        function
    }

    #[allow(clippy::too_many_arguments)]
    fn add_fn(
        &mut self,
        module: ModId,
        id: String,
        name: String,
        owner: Owner,
        sig: &'a syn::Signature,
        body: Option<&'a syn::Block>,
        attrs: &'a [Attribute],
        public: bool,
        file: usize,
        test: bool,
        parent: Option<FnId>,
    ) -> FnId {
        if let Some(existing) = self.index.by_id.get(&id) {
            return *existing;
        }
        let real = self.real_module(module);
        let container = self.index.modules[real].container.clone();
        let qualified = format!("{}::{name}", self.index.modules[real].short);
        let (start, end) = match body {
            Some(body) => (
                source::line(sig.fn_token.span),
                source::end_line(body.brace_token.span.close()),
            ),
            None => source::lines(sig),
        };
        let start = attrs
            .iter()
            .filter(|attr| !attr.path().is_ident("doc"))
            .map(|attr| source::line(attr.span()))
            .min()
            .unwrap_or(start)
            .min(start);
        let function = self.index.fns.len();
        self.index.by_id.insert(id.clone(), function);
        let krate = self.index.modules[module].krate;
        self.index.fns.push(FnDef {
            id,
            name,
            qualified,
            container,
            file,
            start,
            end: end.max(start),
            test: test || self.index.crates[krate].test_code,
            generated: self
                .index
                .files
                .list
                .get(file)
                .is_some_and(|source| source.generated),
            complexity: body.map(crate::complexity::of_block),
            owner,
            scope: module,
            krate,
            sig: Some(sig),
            body,
            attrs,
            public,
            parent,
        });
        if let Some(body) = body {
            self.nested_items(function, module, body, file, test);
        }
        function
    }

    /// The items a function body declares go into a block module of their
    /// own, which its names resolve in first.
    fn nested_items(
        &mut self,
        function: FnId,
        module: ModId,
        body: &'a syn::Block,
        file: usize,
        test: bool,
    ) {
        let mut nested = NestedItems::default();
        nested.visit_block(body);
        if nested.items.is_empty() {
            return;
        }
        let block = self.index.modules.len();
        let real = self.real_module(module);
        let outer = &self.index.modules[real];
        self.index.modules.push(Module {
            path: self.index.fns[function].id.clone(),
            container: outer.container.clone(),
            short: outer.short.clone(),
            krate: outer.krate,
            parent: Some(module),
            block: true,
            file,
            test,
            public: false,
            children: HashMap::new(),
            fns: HashMap::new(),
            types: HashMap::new(),
            traits: HashMap::new(),
            uses: HashMap::new(),
            globs: Vec::new(),
        });
        self.index.fns[function].scope = block;
        for item in nested.items {
            match item {
                Item::Fn(inner) => {
                    let test = test || is_cfg_test(&inner.attrs) || is_test_fn(&inner.attrs);
                    let id = self.free_fn(
                        block,
                        &inner.sig,
                        &inner.block,
                        &inner.attrs,
                        &inner.vis,
                        file,
                        test,
                        Some(function),
                    );
                    self.index.modules[block]
                        .fns
                        .entry(ident(&inner.sig.ident))
                        .or_insert(id);
                }
                other => self.items(
                    block,
                    std::slice::from_ref(other),
                    file,
                    Path::new(""),
                    Path::new(""),
                    true,
                    test,
                ),
            }
        }
    }

    fn real_module(&self, mut module: ModId) -> ModId {
        while self.index.modules[module].block {
            match self.index.modules[module].parent {
                Some(parent) => module = parent,
                None => break,
            }
        }
        module
    }

    fn impl_item(
        &mut self,
        module: ModId,
        implementation: &'a syn::ItemImpl,
        file: usize,
        test: bool,
    ) {
        let test = test || is_cfg_test(&implementation.attrs);
        let impl_id = self.index.impls.len();
        let trait_path = implementation
            .trait_
            .as_ref()
            .map(|(_, path, _)| segments(path));
        self.index.impls.push(ImplDef {
            module,
            self_ty: &implementation.self_ty,
            trait_path: trait_path.clone(),
            methods: HashMap::new(),
            self_type: None,
            trait_id: None,
            generics: &implementation.generics,
        });
        let self_label = type_label(&implementation.self_ty);
        let prefix = match &trait_path {
            Some(path) => format!(
                "<{self_label} as {}>",
                path.last().cloned().unwrap_or_default()
            ),
            None => self_label,
        };
        for item in &implementation.items {
            let syn::ImplItem::Fn(method) = item else {
                continue;
            };
            let name = ident(&method.sig.ident);
            let display = format!("{prefix}::{name}");
            let id = format!("{}::{display}", self.index.modules[module].path);
            let method_test = test || is_cfg_test(&method.attrs);
            let public = is_pub(&method.vis) || trait_path.is_some();
            let function = self.add_fn(
                module,
                id,
                display,
                Owner::Impl(impl_id),
                &method.sig,
                Some(&method.block),
                &method.attrs,
                public,
                file,
                method_test,
                None,
            );
            if self.index.fns[function].owner == Owner::Impl(impl_id) {
                self.index.impls[impl_id]
                    .methods
                    .insert(name.clone(), function);
                self.index
                    .methods_by_name
                    .entry(name)
                    .or_default()
                    .push(function);
            }
        }
    }

    fn trait_item(
        &mut self,
        module: ModId,
        definition: &'a syn::ItemTrait,
        file: usize,
        test: bool,
    ) {
        let test = test || is_cfg_test(&definition.attrs);
        let trait_id = self.index.traits.len();
        let name = ident(&definition.ident);
        let supertraits = definition
            .supertraits
            .iter()
            .filter_map(|bound| match bound {
                syn::TypeParamBound::Trait(bound) => Some(segments(&bound.path)),
                _ => None,
            })
            .collect();
        self.index.traits.push(TraitDef {
            name: name.clone(),
            module,
            public: is_pub(&definition.vis),
            hidden: doc_hidden(&definition.attrs),
            methods: HashMap::new(),
            supertraits,
            resolved_supertraits: Vec::new(),
            impls: Vec::new(),
        });
        self.index.modules[module]
            .traits
            .entry(name.clone())
            .or_insert(trait_id);
        for item in &definition.items {
            let syn::TraitItem::Fn(method) = item else {
                continue;
            };
            let method_name = ident(&method.sig.ident);
            let default = method.default.as_ref().map(|body| {
                let display = format!("{name}::{method_name}");
                let id = format!("{}::{display}", self.index.modules[module].path);
                let function = self.add_fn(
                    module,
                    id,
                    display,
                    Owner::Trait(trait_id),
                    &method.sig,
                    Some(body),
                    &method.attrs,
                    true,
                    file,
                    test,
                    None,
                );
                self.index
                    .methods_by_name
                    .entry(method_name.clone())
                    .or_default()
                    .push(function);
                function
            });
            self.index.traits[trait_id]
                .methods
                .insert(method_name, default);
        }
    }

    /// Links impls to the types and traits of the tree they are for.
    fn resolve_impls(&mut self) {
        for impl_id in 0..self.index.impls.len() {
            let module = self.index.impls[impl_id].module;
            let generics: HashSet<String> = self.index.impls[impl_id]
                .generics
                .type_params()
                .map(|param| ident(&param.ident))
                .collect();
            let self_type = match strip_type(self.index.impls[impl_id].self_ty) {
                syn::Type::Path(path) if path.qself.is_none() => {
                    let segs = segments(&path.path);
                    if segs.len() == 1 && generics.contains(&segs[0]) {
                        None
                    } else {
                        match self.index.resolve_type_path(module, &segs, 0) {
                            Res::Type(id) => Some(id),
                            _ => None,
                        }
                    }
                }
                _ => None,
            };
            let trait_id = self.index.impls[impl_id]
                .trait_path
                .clone()
                .and_then(
                    |path| match self.index.resolve(module, &path, None, false) {
                        Res::Trait(id) => Some(id),
                        _ => None,
                    },
                );
            self.index.impls[impl_id].self_type = self_type;
            self.index.impls[impl_id].trait_id = trait_id;
            if let Some(type_id) = self_type {
                let table = if trait_id.is_some() || self.index.impls[impl_id].trait_path.is_some()
                {
                    &mut self.index.trait_impls
                } else {
                    &mut self.index.inherent
                };
                table.entry(type_id).or_default().push(impl_id);
            }
            if let Some(trait_id) = trait_id {
                self.index.traits[trait_id].impls.push(impl_id);
            }
        }
        for trait_id in 0..self.index.traits.len() {
            let module = self.index.traits[trait_id].module;
            let supers: Vec<TraitId> = self.index.traits[trait_id]
                .supertraits
                .clone()
                .iter()
                .filter_map(|path| match self.index.resolve(module, path, None, false) {
                    Res::Trait(id) if id != trait_id => Some(id),
                    _ => None,
                })
                .collect();
            self.index.traits[trait_id].resolved_supertraits = supers;
        }
    }
}

/// `#[cfg(…)]` other than test: a module that may only exist on another
/// platform or with another feature.
fn is_cfg_gated(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("cfg"))
}

fn strip_type(ty: &syn::Type) -> &syn::Type {
    match ty {
        syn::Type::Reference(reference) => strip_type(&reference.elem),
        syn::Type::Paren(inner) => strip_type(&inner.elem),
        syn::Type::Group(inner) => strip_type(&inner.elem),
        other => other,
    }
}

/// The deepest resolution of `use` chains followed.
const MAX_DEPTH: u32 = 12;

impl<'a> Index<'a> {
    pub fn function(&self, id: &str) -> Option<&FnDef<'a>> {
        self.by_id.get(id).map(|index| &self.fns[*index])
    }

    /// The module a block module belongs to.
    pub fn real_module(&self, mut module: ModId) -> ModId {
        while self.modules[module].block {
            match self.modules[module].parent {
                Some(parent) => module = parent,
                None => break,
            }
        }
        module
    }

    fn crate_root(&self, module: ModId) -> ModId {
        self.crates[self.modules[module].krate].module
    }

    /// Resolves a path written in `scope`. `self_res` is what `Self` names
    /// there; `value` prefers functions for the last segment.
    pub fn resolve(
        &self,
        scope: ModId,
        segs: &[String],
        self_res: Option<&Res>,
        value: bool,
    ) -> Res {
        self.resolve_at(scope, segs, self_res, value, 0)
    }

    fn resolve_at(
        &self,
        scope: ModId,
        segs: &[String],
        self_res: Option<&Res>,
        value: bool,
        depth: u32,
    ) -> Res {
        let Some((first, rest)) = segs.split_first() else {
            return Res::Unknown;
        };
        if depth > MAX_DEPTH {
            return Res::Unknown;
        }
        let mut res = match first.as_str() {
            "Self" => self_res.cloned().unwrap_or(Res::Unknown),
            "::" => match rest.first() {
                Some(name) => {
                    let found = self.extern_crate(scope, name);
                    return rest[1..]
                        .iter()
                        .enumerate()
                        .fold(found, |res, (position, segment)| {
                            self.step(res, segment, value && position + 2 == rest.len(), depth)
                        });
                }
                None => Res::Unknown,
            },
            _ => self.resolve_first(scope, first, value && rest.is_empty(), depth),
        };
        for (position, segment) in rest.iter().enumerate() {
            res = self.step(res, segment, value && position + 1 == rest.len(), depth);
        }
        if res == Res::Unknown
            && segs.len() > 1
            && !matches!(first.as_str(), "crate" | "self" | "super" | "Self")
        {
            // A path from a crate the analyzer does not know (a dependency
            // when cargo could not list them).
            if self.resolve_first(scope, first, false, depth) == Res::Unknown {
                return Res::External(segs.to_vec());
            }
        }
        res
    }

    fn extern_crate(&self, scope: ModId, name: &str) -> Res {
        match self.crates[self.modules[scope].krate].externs.get(name) {
            Some(Some(krate)) => Res::Module(self.crates[*krate].module),
            _ => Res::External(vec![name.to_string()]),
        }
    }

    fn resolve_first(&self, scope: ModId, name: &str, value: bool, depth: u32) -> Res {
        match name {
            "crate" => return Res::Module(self.crate_root(scope)),
            "self" => return Res::Module(self.real_module(scope)),
            "super" => {
                let real = self.real_module(scope);
                return match self.modules[real].parent {
                    Some(parent) => Res::Module(self.real_module(parent)),
                    None => Res::Unknown,
                };
            }
            _ => {}
        }
        let mut current = Some(scope);
        while let Some(module) = current {
            if let Some(found) = self.lookup(module, name, value, depth) {
                return found;
            }
            if !self.modules[module].block {
                break;
            }
            current = self.modules[module].parent;
        }
        let krate = &self.crates[self.modules[scope].krate];
        if let Some(found) = krate.externs.get(name) {
            return match found {
                Some(other) => Res::Module(self.crates[*other].module),
                None => Res::External(vec![name.to_string()]),
            };
        }
        if matches!(name, "std" | "core" | "alloc" | "proc_macro" | "test") {
            return Res::External(vec![name.to_string()]);
        }
        if let Some(path) = prelude(name) {
            return Res::External(path);
        }
        // A name from the one glob import of a crate outside the tree.
        let mut outside = Vec::new();
        let mut current = Some(scope);
        while let Some(module) = current {
            for glob in &self.modules[module].globs {
                if let Res::External(path) =
                    self.resolve_at(module, &glob.path, None, false, depth + 1)
                {
                    outside.push(path);
                }
            }
            if !self.modules[module].block {
                break;
            }
            current = self.modules[module].parent;
        }
        if outside.len() == 1 {
            let mut path = outside.remove(0);
            path.push(name.to_string());
            return Res::External(path);
        }
        Res::Unknown
    }

    /// `name` among the items, imports and glob imports of `module`.
    fn lookup(&self, module: ModId, name: &str, value: bool, depth: u32) -> Option<Res> {
        let found = &self.modules[module];
        let function = found.fns.get(name).map(|id| Res::Fn(*id));
        let named = found
            .children
            .get(name)
            .map(|id| Res::Module(*id))
            .or_else(|| found.types.get(name).map(|id| Res::Type(*id)))
            .or_else(|| found.traits.get(name).map(|id| Res::Trait(*id)));
        let local = if value {
            function.or(named)
        } else {
            named.or(function)
        };
        if local.is_some() {
            return local;
        }
        if let Some(imported) = found.uses.get(name) {
            if depth < MAX_DEPTH {
                // A use is resolved from the module it is written in, but
                // not through itself.
                let res = self.resolve_use(module, &imported.path, value, depth + 1);
                let relative = imported
                    .path
                    .first()
                    .is_some_and(|first| matches!(first.as_str(), "crate" | "self" | "super"));
                if res != Res::Unknown || relative {
                    return Some(res);
                }
                return Some(Res::External(imported.path.clone()));
            }
        }
        if depth < MAX_DEPTH / 2 {
            for glob in &found.globs {
                match self.resolve_use(module, &glob.path, false, depth + 1) {
                    Res::Module(target) if target != module => {
                        if let Some(res) = self.lookup(target, name, value, depth + 1) {
                            return Some(res);
                        }
                    }
                    Res::Type(type_id) => {
                        if let TypeKind::Enum { variants, .. } = &self.types[type_id].kind {
                            if variants.iter().any(|(variant, _)| variant == name) {
                                return Some(Res::Other);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    fn resolve_use(&self, module: ModId, path: &[String], value: bool, depth: u32) -> Res {
        let Some(first) = path.first() else {
            return Res::Unknown;
        };
        if first == "::" {
            return self.resolve_at(module, path, None, value, depth);
        }
        // `use x::y` where x is the name of a crate: the crate, unless the
        // module has an item of that name (uniform paths).
        let local = self.modules[module].children.contains_key(first)
            || self.modules[module].types.contains_key(first)
            || self.modules[module].traits.contains_key(first);
        if !local && !matches!(first.as_str(), "crate" | "self" | "super") {
            let krate = &self.crates[self.modules[module].krate];
            if let Some(found) = krate.externs.get(first) {
                let start = match found {
                    Some(other) => Res::Module(self.crates[*other].module),
                    None => Res::External(vec![first.clone()]),
                };
                return path[1..]
                    .iter()
                    .enumerate()
                    .fold(start, |res, (position, segment)| {
                        self.step(res, segment, value && position + 2 == path.len(), depth)
                    });
            }
        }
        self.resolve_at(module, path, None, value, depth)
    }

    /// The item `segment` of what `res` names.
    fn step(&self, res: Res, segment: &str, value: bool, depth: u32) -> Res {
        match res {
            Res::Module(module) => self
                .lookup(module, segment, value, depth)
                .unwrap_or(Res::Unknown),
            Res::Type(type_id) => {
                let type_id = self.unalias(type_id, depth);
                match type_id {
                    Some(type_id) if !is_type_like(segment) => self
                        .method_of_type(type_id, segment)
                        .map(Res::Fn)
                        .unwrap_or(Res::Other),
                    _ => Res::Other,
                }
            }
            Res::Trait(trait_id) => {
                if self.trait_declares(trait_id, segment).is_some() {
                    Res::TraitMethod(trait_id, segment.to_string())
                } else {
                    Res::Other
                }
            }
            Res::External(mut path) => {
                path.push(segment.to_string());
                Res::External(path)
            }
            _ => Res::Unknown,
        }
    }

    /// The type an alias stands for, when it is a type of the tree.
    fn unalias(&self, type_id: TypeId, depth: u32) -> Option<TypeId> {
        match &self.types[type_id].kind {
            TypeKind::Alias(ty) => match strip_type(ty) {
                syn::Type::Path(path) if depth < MAX_DEPTH => {
                    match self.resolve_type_path(
                        self.types[type_id].module,
                        &segments(&path.path),
                        depth + 1,
                    ) {
                        Res::Type(target) if target != type_id => self.unalias(target, depth + 1),
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => Some(type_id),
        }
    }

    fn resolve_type_path(&self, scope: ModId, segs: &[String], depth: u32) -> Res {
        self.resolve_at(scope, segs, None, false, depth)
    }

    /// The trait among `trait_id` and its supertraits that declares `name`.
    pub fn trait_declares(&self, trait_id: TraitId, name: &str) -> Option<TraitId> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([trait_id]);
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current) {
                continue;
            }
            if self.traits[current].methods.contains_key(name) {
                return Some(current);
            }
            queue.extend(self.traits[current].resolved_supertraits.iter().copied());
        }
        None
    }

    /// The method `name` of a type of the tree: inherent first, then its
    /// trait impls, then the default bodies of the traits it implements.
    pub fn method_of_type(&self, type_id: TypeId, name: &str) -> Option<FnId> {
        let impls =
            |table: &HashMap<TypeId, Vec<ImplId>>| table.get(&type_id).cloned().unwrap_or_default();
        for impl_id in impls(&self.inherent)
            .into_iter()
            .chain(impls(&self.trait_impls))
        {
            if let Some(function) = self.impls[impl_id].methods.get(name) {
                return Some(*function);
            }
        }
        for impl_id in impls(&self.trait_impls) {
            if let Some(trait_id) = self.impls[impl_id].trait_id {
                if let Some(declaring) = self.trait_declares(trait_id, name) {
                    if let Some(Some(default)) = self.traits[declaring].methods.get(name) {
                        return Some(*default);
                    }
                }
            }
        }
        None
    }

    /// Every function a call of the trait method `name` may reach: each
    /// impl's own method, or the trait's default body where an impl keeps it.
    pub fn trait_method_targets(&self, trait_id: TraitId, name: &str) -> Vec<FnId> {
        let Some(declaring) = self.trait_declares(trait_id, name) else {
            return Vec::new();
        };
        let default = self.traits[declaring].methods.get(name).copied().flatten();
        let mut targets = BTreeSet::new();
        // Impls of the trait itself and of every trait that has it as a
        // supertrait implement it.
        let mut implementors: Vec<ImplId> = self.traits[declaring].impls.clone();
        if declaring != trait_id {
            implementors.extend(self.traits[trait_id].impls.iter().copied());
        }
        for impl_id in implementors {
            if self.impls[impl_id].trait_id != Some(declaring) {
                continue;
            }
            match self.impls[impl_id].methods.get(name) {
                Some(function) => {
                    targets.insert(*function);
                }
                None => {
                    if let Some(default) = default {
                        targets.insert(default);
                    }
                }
            }
        }
        if targets.is_empty() {
            if let Some(default) = default {
                targets.insert(default);
            }
        }
        targets.into_iter().collect()
    }

    /// The external id of a trait method with no body in the tree to call:
    /// "<module path>::<Trait>::<method>".
    pub fn trait_method_id(&self, trait_id: TraitId, name: &str) -> (String, String, String) {
        let declaring = self.trait_declares(trait_id, name).unwrap_or(trait_id);
        let definition = &self.traits[declaring];
        let container = self.modules[definition.module].container.clone();
        let short = format!("{}::{name}", definition.name);
        (format!("{container}::{short}"), short, container)
    }

    /// The type a `syn::Type` written in `scope` stands for.
    pub fn resolve_type(&self, scope: ModId, ty: &syn::Type, ctx: &TyCtx) -> Ty {
        self.resolve_type_at(scope, ty, ctx, 0)
    }

    fn resolve_type_at(&self, scope: ModId, ty: &syn::Type, ctx: &TyCtx, depth: u32) -> Ty {
        if depth > MAX_DEPTH {
            return Ty::Unknown;
        }
        match ty {
            syn::Type::Reference(reference) => {
                self.resolve_type_at(scope, &reference.elem, ctx, depth + 1)
            }
            syn::Type::Paren(inner) => self.resolve_type_at(scope, &inner.elem, ctx, depth + 1),
            syn::Type::Group(inner) => self.resolve_type_at(scope, &inner.elem, ctx, depth + 1),
            syn::Type::Ptr(pointer) => self.resolve_type_at(scope, &pointer.elem, ctx, depth + 1),
            syn::Type::TraitObject(object) => self.bounds_type(scope, object.bounds.iter(), depth),
            syn::Type::ImplTrait(implementation) => {
                self.bounds_type(scope, implementation.bounds.iter(), depth)
            }
            syn::Type::Path(path) if path.qself.is_none() => {
                let segs = segments(&path.path);
                if segs.len() == 1 {
                    if segs[0] == "Self" {
                        return ctx.self_ty.clone().unwrap_or(Ty::Unknown);
                    }
                    if let Some(generic) = ctx.generics.get(&segs[0]) {
                        return generic.clone();
                    }
                }
                let last = path.path.segments.last();
                let first_argument = last.and_then(|segment| match &segment.arguments {
                    syn::PathArguments::AngleBracketed(arguments) => {
                        arguments.args.iter().find_map(|argument| match argument {
                            syn::GenericArgument::Type(inner) => Some(inner),
                            _ => None,
                        })
                    }
                    _ => None,
                });
                let last_name = segs.last().map(String::as_str).unwrap_or("");
                if let Some(inner) = first_argument {
                    if matches!(last_name, "Box" | "Arc" | "Rc" | "Pin" | "Cow") {
                        return self.resolve_type_at(scope, inner, ctx, depth + 1);
                    }
                    if matches!(last_name, "Option" | "Result") {
                        return Ty::Wrapped(Box::new(self.resolve_type_at(
                            scope,
                            inner,
                            ctx,
                            depth + 1,
                        )));
                    }
                }
                match self.resolve_at(scope, &segs, None, false, depth + 1) {
                    Res::Type(type_id) => match self.unalias(type_id, depth) {
                        Some(target) => Ty::Named(target),
                        None => match &self.types[type_id].kind {
                            TypeKind::Alias(aliased) => self.resolve_type_at(
                                self.types[type_id].module,
                                aliased,
                                &TyCtx::default(),
                                depth + 1,
                            ),
                            _ => Ty::Unknown,
                        },
                    },
                    Res::Trait(trait_id) => Ty::Dyn(vec![trait_id]),
                    Res::External(path) => Ty::External(path),
                    _ => Ty::Unknown,
                }
            }
            _ => Ty::Unknown,
        }
    }

    /// The type of a value with these bounds: the traits of the tree among
    /// them, else the first trait outside it.
    pub fn bounds_type<'b>(
        &self,
        scope: ModId,
        bounds: impl Iterator<Item = &'b syn::TypeParamBound>,
        depth: u32,
    ) -> Ty {
        let mut inside = Vec::new();
        let mut outside = None;
        for bound in bounds {
            let syn::TypeParamBound::Trait(bound) = bound else {
                continue;
            };
            let segs = segments(&bound.path);
            if matches!(
                segs.last().map(String::as_str),
                Some("Send" | "Sync" | "Sized" | "Unpin" | "Copy" | "Clone" | "Debug")
            ) {
                continue;
            }
            match self.resolve_at(scope, &segs, None, false, depth + 1) {
                Res::Trait(trait_id) => inside.push(trait_id),
                Res::External(path) => {
                    outside.get_or_insert(path);
                }
                _ => {}
            }
        }
        if !inside.is_empty() {
            return Ty::Dyn(inside);
        }
        outside.map(Ty::ExternalDyn).unwrap_or(Ty::Unknown)
    }

    /// The generic parameters of `generics` with the type their bounds
    /// (inline and in the where clause) give a value of them.
    pub fn generic_types(
        &self,
        scope: ModId,
        generics: &syn::Generics,
        into: &mut HashMap<String, Ty>,
    ) {
        let mut bounds: HashMap<String, Vec<&syn::TypeParamBound>> = HashMap::new();
        for param in generics.type_params() {
            bounds
                .entry(ident(&param.ident))
                .or_default()
                .extend(param.bounds.iter());
        }
        if let Some(clause) = &generics.where_clause {
            for predicate in &clause.predicates {
                if let syn::WherePredicate::Type(predicate) = predicate {
                    if let syn::Type::Path(path) = &predicate.bounded_ty {
                        if let Some(name) = path.path.get_ident() {
                            if let Some(list) = bounds.get_mut(&ident(name)) {
                                list.extend(predicate.bounds.iter());
                            }
                        }
                    }
                }
            }
        }
        for (name, list) in bounds {
            into.insert(name, self.bounds_type(scope, list.into_iter(), 0));
        }
    }

    /// The type context of a function's body: `Self` and its generics.
    pub fn context_of(&self, function: FnId) -> (TyCtx, Option<Res>) {
        let definition = &self.fns[function];
        let mut ctx = TyCtx::default();
        let mut self_res = None;
        match definition.owner {
            Owner::Impl(impl_id) => {
                let implementation = &self.impls[impl_id];
                self.generic_types(
                    implementation.module,
                    implementation.generics,
                    &mut ctx.generics,
                );
                match implementation.self_type {
                    Some(type_id) => {
                        ctx.self_ty = Some(Ty::Named(type_id));
                        self_res = Some(Res::Type(type_id));
                    }
                    None => {
                        // A blanket impl: Self is the generic parameter.
                        let label = type_label(implementation.self_ty);
                        if let Some(generic) = ctx.generics.get(&label).cloned() {
                            ctx.self_ty = Some(generic);
                        }
                        if let Some(trait_id) = implementation.trait_id {
                            ctx.self_ty.get_or_insert(Ty::Dyn(vec![trait_id]));
                            self_res = Some(Res::Trait(trait_id));
                        }
                    }
                }
            }
            Owner::Trait(trait_id) => {
                ctx.self_ty = Some(Ty::Dyn(vec![trait_id]));
                self_res = Some(Res::Trait(trait_id));
            }
            // A nested function sees neither the Self nor the generics of
            // the function it is written in.
            Owner::Free | Owner::Closure => {}
        }
        if let Some(sig) = definition.sig {
            self.generic_types(definition.scope, &sig.generics, &mut ctx.generics);
        }
        (ctx, self_res)
    }

    /// The type a call of `function` returns.
    pub fn return_type(&self, function: FnId) -> Ty {
        let definition = &self.fns[function];
        let Some(sig) = definition.sig else {
            return Ty::Unknown;
        };
        let syn::ReturnType::Type(_, ty) = &sig.output else {
            return Ty::Unknown;
        };
        let (ctx, _) = self.context_of(function);
        self.resolve_type(definition.scope, ty, &ctx)
    }

    /// The type of the field `name` of a struct of the tree.
    pub fn field_type(&self, type_id: TypeId, name: &str) -> Ty {
        let definition = &self.types[type_id];
        let TypeKind::Struct(fields) = &definition.kind else {
            return Ty::Unknown;
        };
        let Some(ty) = fields.get(name) else {
            return Ty::Unknown;
        };
        let ctx = TyCtx {
            self_ty: Some(Ty::Named(type_id)),
            generics: HashMap::new(),
        };
        self.resolve_type(definition.module, ty, &ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_test_and_test_attributes_are_told_apart() {
        let item: syn::ItemFn = syn::parse_quote! {
            #[cfg(all(test, feature = "x"))]
            #[tokio::test]
            async fn works() {}
        };
        assert!(is_cfg_test(&item.attrs));
        assert!(is_test_fn(&item.attrs));
        let item: syn::ItemFn = syn::parse_quote! {
            #[cfg(not(test))]
            fn production() {}
        };
        assert!(!is_cfg_test(&item.attrs));
        assert!(!is_test_fn(&item.attrs));
    }

    #[test]
    fn types_are_labelled_by_their_last_segment() {
        let ty: syn::Type = syn::parse_quote!(&'a mut crate::shapes::Circle<T>);
        assert_eq!(type_label(&ty), "&Circle");
        let ty: syn::Type = syn::parse_quote!([u8]);
        assert_eq!(type_label(&ty), "[u8]");
        let ty: syn::Type = syn::parse_quote!(Box<dyn Shape + Send>);
        assert_eq!(type_label(&ty), "Box");
    }

    #[test]
    fn derives_and_clap_names_are_read() {
        let item: syn::ItemEnum = syn::parse_quote! {
            #[derive(Debug, clap::Subcommand)]
            enum Commands {
                #[command(name = "ls", about = "list")]
                List,
                Add,
            }
        };
        assert_eq!(derives(&item.attrs), ["Debug", "Subcommand"]);
        assert_eq!(clap_name(&item.variants[0].attrs).as_deref(), Some("ls"));
        assert_eq!(clap_name(&item.variants[1].attrs), None);
    }
}
