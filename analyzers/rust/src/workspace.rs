//! The Cargo projects of a source tree: a `Cargo.toml` at its root, or one
//! folder down (several of them then). Each is read with
//! `cargo metadata --no-deps --offline`, which resolves nothing and runs no
//! project code; when cargo cannot read it (cargo missing, a broken
//! manifest), the crates are found from Cargo's conventions instead and a
//! diagnostic says so.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde_json::Value;

use crate::manifest;
use crate::process;

/// The analysed source tree. Turns absolute paths into paths relative to it
/// with "/" separators.
#[derive(Debug, Clone)]
pub struct Tree {
    pub root: PathBuf,
    roots: Vec<PathBuf>,
}

impl Tree {
    pub fn new(dir: &Path) -> std::io::Result<Self> {
        let absolute = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir()?.join(dir)
        };
        let mut roots = vec![absolute.clone()];
        if let Ok(real) = absolute.canonicalize() {
            if real != absolute {
                roots.push(real);
            }
        }
        Ok(Self {
            root: absolute,
            roots,
        })
    }

    /// `path` relative to the tree, or None when it lies outside.
    pub fn rel(&self, path: &Path) -> Option<String> {
        let path = normalize(path);
        for root in &self.roots {
            if let Ok(rest) = path.strip_prefix(root) {
                let parts: Vec<String> = rest
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                if parts.is_empty() {
                    return Some(".".to_string());
                }
                return Some(parts.join("/"));
            }
        }
        None
    }
}

/// `path` without `.` and `..` components, without touching the disk.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TargetKind {
    Lib,
    Bin,
    Test,
    Bench,
    Example,
    Build,
}

impl TargetKind {
    fn from_metadata(kinds: &[String]) -> Option<Self> {
        let kind = kinds.first()?;
        Some(match kind.as_str() {
            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" => Self::Lib,
            "bin" => Self::Bin,
            "test" => Self::Test,
            "bench" => Self::Bench,
            "example" => Self::Example,
            "custom-build" => Self::Build,
            _ => return None,
        })
    }

    /// The word `--<kind>` cargo takes, and the folder in a crate root's id.
    pub fn word(self) -> &'static str {
        match self {
            Self::Lib => "lib",
            Self::Bin => "bin",
            Self::Test => "test",
            Self::Bench => "bench",
            Self::Example => "example",
            Self::Build => "build",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Target {
    pub kind: TargetKind,
    pub name: String,
    /// The name rustc compiles it as: `-` read as `_`.
    pub crate_name: String,
    pub src_path: PathBuf,
    /// `cargo test --tests` runs it.
    pub test: bool,
    /// Uses libtest (false for `harness = false`).
    pub harness: bool,
    pub proc_macro: bool,
    /// Has `required-features`: cargo builds it only with them.
    pub required_features: bool,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    /// The name code uses for it: its rename or its name, `-` read as `_`.
    pub code_name: String,
    /// The package it is.
    pub package: String,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub dir: PathBuf,
    pub targets: Vec<Target>,
    pub dependencies: Vec<Dependency>,
}

impl Package {
    pub fn lib(&self) -> Option<&Target> {
        self.targets
            .iter()
            .find(|target| target.kind == TargetKind::Lib)
    }

    pub fn depends_on(&self, package: &str) -> bool {
        self.dependencies
            .iter()
            .any(|dependency| dependency.package == package)
    }
}

/// One Cargo workspace (or lone package) of the tree.
#[derive(Debug, Clone)]
pub struct Unit {
    pub root: PathBuf,
    pub packages: Vec<Package>,
}

#[derive(Debug, Default)]
pub struct Workspace {
    pub units: Vec<Unit>,
    /// Problems met while reading the manifests, as sentences.
    pub warnings: Vec<String>,
    /// What cargo said when it could not read a manifest.
    pub cargo_errors: Vec<String>,
}

impl Workspace {
    #[cfg(test)]
    pub fn packages(&self) -> impl Iterator<Item = &Package> {
        self.units.iter().flat_map(|unit| unit.packages.iter())
    }
}

/// The folders of the tree holding a Cargo.toml to read: the root, else
/// every folder one level down that has one.
pub fn manifest_dirs(root: &Path) -> Vec<PathBuf> {
    if root.join("Cargo.toml").is_file() {
        return vec![root.to_path_buf()];
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.join("Cargo.toml").is_file())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

/// Reads the Cargo projects of `tree`.
pub fn discover(tree: &Tree, deadline: Instant) -> Workspace {
    let mut workspace = Workspace::default();
    let mut roots = BTreeSet::new();
    for dir in manifest_dirs(&tree.root) {
        let unit = match metadata(&dir, deadline) {
            Ok(unit) => unit,
            Err(error) => {
                let rel = tree.rel(&dir).unwrap_or_else(|| ".".to_string());
                workspace.warnings.push(format!(
                    "the crates in {rel} were found from Cargo's conventions, as cargo could not read the manifest: {}",
                    process::first_line(&error)
                ));
                workspace.cargo_errors.push(error);
                conventions(&dir)
            }
        };
        let mut unit = unit;
        // Members outside the tree (a path dependency of a workspace above
        // it) are not part of it.
        unit.packages
            .retain(|package| tree.rel(&package.manifest_path).is_some());
        if roots.insert(unit.root.clone()) && !unit.packages.is_empty() {
            workspace.units.push(unit);
        }
    }
    workspace
}

/// Reads the workspace that `dir/Cargo.toml` belongs to with cargo.
fn metadata(dir: &Path, deadline: Instant) -> Result<Unit, String> {
    let mut command = Command::new("cargo");
    command
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
            "--manifest-path",
        ])
        .arg(dir.join("Cargo.toml"))
        .current_dir(dir)
        .env("CARGO_TERM_COLOR", "never");
    let (stdout, finished) = process::capture(command, deadline)
        .map_err(|error| format!("cargo could not run: {error}"))?;
    if !finished.success() {
        let stderr = finished.stderr.trim();
        return Err(if stderr.is_empty() {
            format!("cargo metadata failed ({})", finished.describe())
        } else {
            stderr.to_string()
        });
    }
    let document: Value = serde_json::from_str(&stdout)
        .map_err(|error| format!("cargo metadata printed something unreadable: {error}"))?;
    let root = document["workspace_root"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.to_path_buf());
    let packages = document["packages"]
        .as_array()
        .map(|packages| packages.iter().filter_map(package_from_metadata).collect())
        .unwrap_or_default();
    Ok(Unit { root, packages })
}

fn package_from_metadata(value: &Value) -> Option<Package> {
    let manifest_path = PathBuf::from(value["manifest_path"].as_str()?);
    let dir = manifest_path.parent()?.to_path_buf();
    let harnessless = std::fs::read_to_string(&manifest_path)
        .map(|text| manifest::harnessless(&manifest::parse(&text)))
        .unwrap_or_default();
    let targets = value["targets"]
        .as_array()?
        .iter()
        .filter_map(|target| {
            let kinds: Vec<String> = target["kind"]
                .as_array()?
                .iter()
                .filter_map(|kind| kind.as_str().map(str::to_string))
                .collect();
            let kind = TargetKind::from_metadata(&kinds)?;
            let name = target["name"].as_str()?.to_string();
            let key = if kind == TargetKind::Lib {
                "lib".to_string()
            } else {
                format!("{}:{name}", kind.word())
            };
            Some(Target {
                kind,
                crate_name: name.replace('-', "_"),
                src_path: PathBuf::from(target["src_path"].as_str()?),
                test: target["test"].as_bool().unwrap_or(matches!(
                    kind,
                    TargetKind::Lib | TargetKind::Bin | TargetKind::Test
                )),
                harness: !harnessless.contains(&key),
                proc_macro: kinds.iter().any(|kind| kind == "proc-macro"),
                required_features: target["required-features"]
                    .as_array()
                    .is_some_and(|features| !features.is_empty()),
                name,
            })
        })
        .collect();
    let dependencies = value["dependencies"]
        .as_array()
        .map(|dependencies| {
            dependencies
                .iter()
                .filter_map(|dependency| {
                    let package = dependency["name"].as_str()?.to_string();
                    let code_name = dependency["rename"]
                        .as_str()
                        .unwrap_or(&package)
                        .replace('-', "_");
                    Some(Dependency { code_name, package })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Package {
        name: value["name"].as_str()?.to_string(),
        version: value["version"].as_str().unwrap_or("0.0.0").to_string(),
        manifest_path,
        dir,
        targets,
        dependencies,
    })
}

/// The workspace of `dir/Cargo.toml` read from Cargo's conventions: its
/// `[workspace] members` (a trailing `*` expands one level) and its own
/// `[package]`, each with the targets Cargo would find by itself plus the
/// ones its manifest declares.
fn conventions(dir: &Path) -> Unit {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
    let tables = manifest::parse(&text);
    let mut dirs = Vec::new();
    if tables.iter().any(|table| table.name == "package") {
        dirs.push(dir.to_path_buf());
    }
    if let Some(workspace) = tables.iter().find(|table| table.name == "workspace") {
        for member in workspace.array("members") {
            if let Some(prefix) = member.strip_suffix("/*") {
                let mut found: Vec<PathBuf> = std::fs::read_dir(dir.join(prefix))
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.path())
                            .collect()
                    })
                    .unwrap_or_default();
                found.sort();
                dirs.extend(
                    found
                        .into_iter()
                        .filter(|path| path.join("Cargo.toml").is_file()),
                );
            } else if dir.join(&member).join("Cargo.toml").is_file() {
                dirs.push(dir.join(member));
            }
        }
    }
    let packages = dirs
        .iter()
        .filter_map(|dir| package_from_conventions(dir))
        .collect();
    Unit {
        root: dir.to_path_buf(),
        packages,
    }
}

fn package_from_conventions(dir: &Path) -> Option<Package> {
    let manifest_path = dir.join("Cargo.toml");
    let tables = manifest::parse(&std::fs::read_to_string(&manifest_path).ok()?);
    let package = tables.iter().find(|table| table.name == "package")?;
    let name = package.string("name")?.to_string();
    let harnessless = manifest::harnessless(&tables);
    let mut targets: Vec<Target> = Vec::new();
    let mut add = |kind: TargetKind, name: String, path: PathBuf| {
        if !path.is_file() || targets.iter().any(|target| target.src_path == path) {
            return;
        }
        let key = if kind == TargetKind::Lib {
            "lib".to_string()
        } else {
            format!("{}:{name}", kind.word())
        };
        targets.push(Target {
            kind,
            crate_name: name.replace('-', "_"),
            src_path: path,
            test: matches!(kind, TargetKind::Lib | TargetKind::Bin | TargetKind::Test),
            harness: !harnessless.contains(&key),
            proc_macro: false,
            required_features: false,
            name,
        });
    };
    let lib = tables.iter().find(|table| table.name == "lib");
    let lib_name = lib
        .and_then(|lib| lib.string("name"))
        .unwrap_or(&name)
        .replace('-', "_");
    let lib_path = lib
        .and_then(|lib| lib.string("path"))
        .unwrap_or("src/lib.rs");
    add(TargetKind::Lib, lib_name, dir.join(lib_path));
    for (section, kind, folder) in [
        ("bin", TargetKind::Bin, "src/bin"),
        ("test", TargetKind::Test, "tests"),
        ("bench", TargetKind::Bench, "benches"),
        ("example", TargetKind::Example, "examples"),
    ] {
        for table in tables.iter().filter(|table| table.name == section) {
            if let (Some(target), Some(path)) = (table.string("name"), table.string("path")) {
                add(kind, target.to_string(), dir.join(path));
            }
        }
        if kind == TargetKind::Bin {
            add(kind, name.clone(), dir.join("src/main.rs"));
        }
        for (target, path) in conventional_targets(&dir.join(folder)) {
            add(kind, target, path);
        }
    }
    let build = package.string("build").unwrap_or("build.rs");
    add(
        TargetKind::Build,
        format!("build-script-{}", "build"),
        dir.join(build),
    );
    let dependencies = manifest::dependencies(&tables)
        .into_iter()
        .map(|(code_name, package)| Dependency { code_name, package })
        .collect();
    Some(Package {
        version: package.string("version").unwrap_or("0.0.0").to_string(),
        name,
        dir: dir.to_path_buf(),
        manifest_path,
        targets,
        dependencies,
    })
}

/// `folder/<name>.rs` and `folder/<name>/main.rs`, sorted.
fn conventional_targets(folder: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let stem = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            if path.extension().is_some_and(|extension| extension == "rs") {
                found.push((stem, path));
            } else if path.join("main.rs").is_file() {
                found.push((stem, path.join("main.rs")));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdata(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name)
    }

    #[test]
    fn paths_are_relative_to_the_tree() {
        let tree = Tree::new(Path::new("/work/tree")).unwrap();
        assert_eq!(
            tree.rel(Path::new("/work/tree/src/lib.rs")).as_deref(),
            Some("src/lib.rs")
        );
        assert_eq!(
            tree.rel(Path::new("/work/tree/a/../src/x.rs")).as_deref(),
            Some("src/x.rs")
        );
        assert_eq!(tree.rel(Path::new("/work/other/x.rs")), None);
        assert_eq!(tree.rel(Path::new("/work/tree")).as_deref(), Some("."));
    }

    #[test]
    fn a_workspace_is_read_by_cargo_and_by_conventions_alike() {
        let tree = Tree::new(&testdata("workspace")).unwrap();
        let workspace = discover(&tree, Instant::now() + std::time::Duration::from_secs(60));
        assert!(workspace.warnings.is_empty(), "{:?}", workspace.warnings);
        let by_cargo: Vec<(String, Vec<(TargetKind, String)>)> = workspace
            .packages()
            .map(|package| {
                let mut targets: Vec<_> = package
                    .targets
                    .iter()
                    .map(|target| (target.kind, target.name.clone()))
                    .collect();
                targets.sort();
                (package.name.clone(), targets)
            })
            .collect();
        let unit = conventions(&tree.root);
        let by_conventions: Vec<(String, Vec<(TargetKind, String)>)> = unit
            .packages
            .iter()
            .map(|package| {
                let mut targets: Vec<_> = package
                    .targets
                    .iter()
                    .map(|target| (target.kind, target.name.clone()))
                    .collect();
                targets.sort();
                (package.name.clone(), targets)
            })
            .collect();
        let mut sorted_cargo = by_cargo.clone();
        sorted_cargo.sort();
        let mut sorted_conventions = by_conventions;
        sorted_conventions.sort();
        assert_eq!(sorted_cargo, sorted_conventions);
        assert!(by_cargo.iter().any(|(name, _)| name == "ledger-core"));
        let app = workspace
            .packages()
            .find(|package| package.name == "ledger-app")
            .unwrap();
        assert!(app
            .dependencies
            .iter()
            .any(|dependency| dependency.code_name == "ledger_core"));
    }
}
