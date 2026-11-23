//! The `analyze` command: the functions, calls, externals and diagnostics
//! of a tree (analysis.schema.json), from syntax alone. It reads the
//! manifests with `cargo metadata --no-deps --offline`, never builds, and
//! never writes inside the tree.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use crate::index::{self, Files, Index};
use crate::protocol::{Analysis, Diagnostic, Function};
use crate::walk::{self, Graph};
use crate::workspace::{self, Tree, Workspace};

/// Everything one tree's syntax gives, which `test` uses as well to place
/// tests and coverage.
pub struct Parsed {
    pub tree: Tree,
    pub workspace: Workspace,
    pub specs: Vec<index::CrateSpec>,
    pub files: Files,
}

impl Parsed {
    pub fn read(dir: &Path, deadline: Instant) -> std::io::Result<Self> {
        let tree = Tree::new(dir)?;
        let workspace = workspace::discover(&tree, deadline);
        let specs = index::crate_roots(&workspace);
        let files = index::load_files(&tree, &workspace, &specs);
        Ok(Self {
            tree,
            workspace,
            specs,
            files,
        })
    }

    pub fn index(&self) -> Index<'_> {
        index::build(&self.tree, &self.workspace, &self.specs, &self.files)
    }
}

pub fn analyze(dir: &Path, deadline: Instant) -> std::io::Result<Analysis> {
    let parsed = Parsed::read(dir, deadline)?;
    let index = parsed.index();
    let mut graph = walk::walk_all(&index);
    crate::entrypoints::detect(&index, &mut graph);
    let mut analysis = Analysis::new();
    for message in &parsed.workspace.warnings {
        analysis
            .diagnostics
            .push(Diagnostic::warning(message.clone()));
    }
    if parsed.workspace.units.is_empty() {
        analysis.diagnostics.push(Diagnostic::warning(
            "no Cargo package was found in the tree",
        ));
    }
    analysis
        .diagnostics
        .extend(parsed.files.diagnostics.iter().cloned());
    analysis
        .diagnostics
        .extend(index.diagnostics.iter().cloned());
    assemble(&index, graph, &mut analysis);
    if Instant::now() > deadline {
        analysis.diagnostics.push(Diagnostic::warning(
            "the analysis took longer than its time limit",
        ));
    }
    Ok(analysis)
}

/// The protocol's lists from the index and the walk: functions (closures
/// after the others), calls, then the references no call or registration
/// of the same site covers, and the externals called.
fn assemble(index: &Index, graph: Graph, analysis: &mut Analysis) {
    let rel = |file: usize| {
        index
            .files
            .list
            .get(file)
            .map(|source| source.rel.clone())
            .unwrap_or_default()
    };
    for (position, definition) in index.fns.iter().chain(graph.closures.iter()).enumerate() {
        analysis.functions.push(Function {
            id: definition.id.clone(),
            name: definition.name.clone(),
            qualified_name: definition.qualified.clone(),
            container: definition.container.clone(),
            file: rel(definition.file),
            start_line: definition.start,
            end_line: definition.end.max(definition.start),
            generated: definition.generated,
            test: definition.test,
            complexity: definition.complexity,
            entry_points: graph.entries.get(&position).cloned().unwrap_or_default(),
        });
    }
    let mut covered: HashSet<(String, String, u32)> = graph
        .calls
        .iter()
        .map(|call| (call.caller.clone(), call.callee.clone(), call.line))
        .collect();
    analysis.calls = graph.calls;
    for reference in graph.references {
        if covered.insert((
            reference.caller.clone(),
            reference.callee.clone(),
            reference.line,
        )) {
            analysis.calls.push(reference);
        }
    }
    analysis.externals = graph.externals.into_values().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    fn testdata(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name)
    }

    fn analysis_of(name: &str) -> Analysis {
        analyze(&testdata(name), Instant::now() + Duration::from_secs(60)).unwrap()
    }

    fn functions(analysis: &Analysis) -> HashMap<&str, &Function> {
        analysis
            .functions
            .iter()
            .map(|function| (function.id.as_str(), function))
            .collect()
    }

    /// The calls from `caller`, as (callee, kind, resolution, relation).
    fn calls_from<'a>(
        analysis: &'a Analysis,
        caller: &str,
    ) -> Vec<(&'a str, &'a str, &'a str, &'a str)> {
        analysis
            .calls
            .iter()
            .filter(|call| call.caller == caller)
            .map(|call| {
                (
                    call.callee.as_str(),
                    call.kind.as_str(),
                    call.resolution.as_str(),
                    call.relation.as_str(),
                )
            })
            .collect()
    }

    fn has_call(
        analysis: &Analysis,
        caller: &str,
        callee: &str,
        kind: &str,
        resolution: &str,
        relation: &str,
    ) -> bool {
        calls_from(analysis, caller).contains(&(callee, kind, resolution, relation))
    }

    #[test]
    fn functions_get_stable_ids_names_and_containers() {
        let analysis = analysis_of("shapes");
        let functions = functions(&analysis);
        let area = functions["shapes::<Circle as Shape>::area"];
        assert_eq!(area.name, "<Circle as Shape>::area");
        assert_eq!(area.qualified_name, "shapes::<Circle as Shape>::area");
        assert_eq!(area.container, "shapes");
        assert_eq!(area.file, "src/lib.rs");
        let new = functions["shapes::Circle::new"];
        assert_eq!(new.name, "Circle::new");
        let describe = functions["shapes::Shape::describe"];
        assert_eq!(describe.name, "Shape::describe");
        let total = functions["shapes::geometry::total_area"];
        assert_eq!(total.container, "shapes::geometry");
        assert_eq!(total.qualified_name, "geometry::total_area");
        assert_eq!(total.file, "src/geometry.rs");
        assert!(
            functions.contains_key("shapes::geometry::total_area$1"),
            "{:?}",
            functions.keys()
        );
        assert!(functions.contains_key("shapes::geometry::scaled::helper"));
        let unit = functions["shapes::geometry::tests::totals_two_circles"];
        assert!(unit.test);
        assert!(!total.test);
        let integration = functions["shapes/tests/api::circles_have_area"];
        assert!(integration.test);
        assert_eq!(integration.container, "shapes/tests/api");
        assert!(functions["shapes::generated::table"].generated);
        assert!(!total.generated);
        assert!(analysis
            .functions
            .iter()
            .all(|function| function.start_line <= function.end_line));
    }

    #[test]
    fn complexity_is_counted_per_function() {
        let analysis = analysis_of("shapes");
        let functions = functions(&analysis);
        assert_eq!(functions["shapes::Circle::new"].complexity, Some(1));
        // if, else if, &&, ?
        assert_eq!(functions["shapes::geometry::classify"].complexity, Some(5));
        // A guarded arm, its guard, and the catch-all that adds nothing.
        assert_eq!(
            functions["shapes::geometry::clamp_size"].complexity,
            Some(3)
        );
    }

    #[test]
    fn calls_are_resolved_statically_and_dynamically() {
        let analysis = analysis_of("shapes");
        // A trait method on a generic parameter: every implementation.
        assert!(has_call(
            &analysis,
            "shapes::geometry::describe_all",
            "shapes::<Circle as Shape>::area",
            "conditional",
            "dynamic",
            "call"
        ));
        assert!(has_call(
            &analysis,
            "shapes::geometry::describe_all",
            "shapes::<Square as Shape>::area",
            "conditional",
            "dynamic",
            "call"
        ));
        // The default body, reached through a dyn Shape.
        assert!(has_call(
            &analysis,
            "shapes::geometry::print_boxed",
            "shapes::Shape::describe",
            "always",
            "dynamic",
            "call"
        ));
        // A constructor through a `use`, then its method by the inferred type.
        assert!(has_call(
            &analysis,
            "shapes::geometry::unit_circle_area",
            "shapes::Circle::new",
            "always",
            "static",
            "call"
        ));
        assert!(has_call(
            &analysis,
            "shapes::geometry::unit_circle_area",
            "shapes::<Circle as Shape>::area",
            "always",
            "static",
            "call"
        ));
        // Self:: inside an impl.
        assert!(has_call(
            &analysis,
            "shapes::Circle::unit",
            "shapes::Circle::new",
            "always",
            "static",
            "call"
        ));
        // After a `?`, calls are conditional.
        assert!(has_call(
            &analysis,
            "shapes::geometry::classify",
            "shapes::geometry::parse_size",
            "always",
            "static",
            "call"
        ));
        assert!(has_call(
            &analysis,
            "shapes::geometry::classify",
            "shapes::geometry::scaled",
            "conditional",
            "static",
            "call"
        ));
        // A call in a match guard, which only some arms reach.
        assert!(has_call(
            &analysis,
            "shapes::geometry::clamp_size",
            "shapes::geometry::scaled",
            "conditional",
            "static",
            "call"
        ));
        // Closures: a reference from the parent, calls of their own.
        assert!(has_call(
            &analysis,
            "shapes::geometry::total_area",
            "shapes::geometry::total_area$1",
            "async",
            "static",
            "reference"
        ));
        assert!(has_call(
            &analysis,
            "shapes::geometry::total_area$1",
            "shapes::geometry::scaled",
            "always",
            "static",
            "call"
        ));
        // A spawned closure is an async call; what it calls is its own.
        assert!(has_call(
            &analysis,
            "shapes::geometry::area_in_background",
            "shapes::geometry::area_in_background$1",
            "async",
            "static",
            "call"
        ));
        // Calls inside a spawned async block are async.
        assert!(has_call(
            &analysis,
            "shapes::geometry::refresh",
            "shapes::geometry::total_area",
            "async",
            "static",
            "call"
        ));
        // A function passed as a value.
        assert!(has_call(
            &analysis,
            "shapes::geometry::areas",
            "shapes::geometry::scaled",
            "async",
            "static",
            "reference"
        ));
        // Called through the local it was stored in.
        assert!(has_call(
            &analysis,
            "shapes::geometry::areas$1",
            "shapes::geometry::scaled",
            "always",
            "static",
            "call"
        ));
        // Externals by their full path.
        assert!(has_call(
            &analysis,
            "shapes::geometry::area_in_background",
            "std::thread::spawn",
            "always",
            "static",
            "call"
        ));
        let external = analysis
            .externals
            .iter()
            .find(|external| external.id == "std::thread::spawn")
            .unwrap();
        assert_eq!(
            (external.name.as_str(), external.container.as_str()),
            ("spawn", "std::thread")
        );
        // Calls inside macros count.
        assert!(has_call(
            &analysis,
            "shapes::geometry::tests::totals_two_circles",
            "shapes::geometry::total_area",
            "always",
            "static",
            "call"
        ));
        // An integration test calls the library by its crate name.
        assert!(has_call(
            &analysis,
            "shapes/tests/api::circles_have_area",
            "shapes::Circle::new",
            "always",
            "static",
            "call"
        ));
        // Production code does not list test-only implementations.
        assert!(!calls_from(&analysis, "shapes::geometry::describe_all")
            .iter()
            .any(|call| call.0.contains("Fake")));
    }

    #[test]
    fn entry_points_are_found() {
        let analysis = analysis_of("shapes");
        let functions = functions(&analysis);
        let kinds = |id: &str| -> Vec<(String, String)> {
            functions[id]
                .entry_points
                .iter()
                .map(|entry| (entry.kind.clone(), entry.label.clone()))
                .collect()
        };
        assert_eq!(
            kinds("shapes::geometry::tests::totals_two_circles"),
            [(
                "test".to_string(),
                "geometry::tests::totals_two_circles".to_string()
            )]
        );
        assert_eq!(
            kinds("shapes/tests/api::circles_have_area"),
            [("test".to_string(), "circles_have_area".to_string())]
        );
        assert!(kinds("shapes::Circle::new").contains(&(
            "exported_api".to_string(),
            "shapes::Circle::new".to_string()
        )));
        // Re-exported at the root.
        assert!(kinds("shapes::geometry::total_area")
            .contains(&("exported_api".to_string(), "shapes::total_area".to_string())));
        assert!(kinds("shapes::geometry::parse_size").is_empty());
    }

    #[test]
    fn a_web_service_and_a_cli_have_their_entry_points() {
        let analysis = analysis_of("service");
        let functions = functions(&analysis);
        let labels = |id: &str| -> Vec<String> {
            functions
                .get(id)
                .unwrap_or_else(|| panic!("{id} in {:?}", functions.keys()))
                .entry_points
                .iter()
                .map(|entry| format!("{} {}", entry.kind, entry.label))
                .collect()
        };
        assert_eq!(labels("service::main"), ["main service"]);
        assert!(
            labels("service::routes::list_users")
                .contains(&"http_route GET /api/users".to_string()),
            "{:?}",
            labels("service::routes::list_users")
        );
        assert!(labels("service::routes::create_user")
            .contains(&"http_route POST /api/users".to_string()));
        assert!(labels("service::health").contains(&"http_route GET /health".to_string()));
        assert!(
            labels("service::commands::serve").contains(&"cli_command service serve".to_string())
        );
        assert!(labels("service::commands::migrate")
            .contains(&"cli_command service migrate-db".to_string()));
        // What the arm only prepares is not the command.
        assert!(
            labels("service::app").is_empty(),
            "{:?}",
            labels("service::app")
        );
        assert!(has_call(
            &analysis,
            "service::routes::router",
            "service::routes::list_users",
            "async",
            "static",
            "registration"
        ));
        let registration = analysis
            .calls
            .iter()
            .find(|call| {
                call.callee == "service::routes::list_users" && call.relation == "registration"
            })
            .unwrap();
        assert_eq!(
            registration.evidence.as_deref(),
            Some("route GET /api/users")
        );
        assert!(has_call(
            &analysis,
            "service::app",
            "service::auth",
            "async",
            "static",
            "registration"
        ));
        assert!(
            labels("service::legacy::show").contains(&"http_route GET /v1/legacy/{id}".to_string()),
            "{:?}",
            labels("service::legacy::show")
        );
        assert!(
            labels("service::legacy::ping").contains(&"http_route GET /v1/ping".to_string()),
            "{:?}",
            labels("service::legacy::ping")
        );
    }

    #[test]
    fn a_workspace_links_its_crates() {
        let analysis = analysis_of("workspace");
        let functions = functions(&analysis);
        assert!(functions.contains_key("ledger_core::Ledger::balance"));
        assert!(functions.contains_key("ledger_app::main"));
        assert!(has_call(
            &analysis,
            "ledger_app::main",
            "ledger_core::Ledger::new",
            "always",
            "static",
            "call"
        ));
        assert!(has_call(
            &analysis,
            "ledger_app::main",
            "ledger_core::Ledger::balance",
            "always",
            "static",
            "call"
        ));
    }

    #[test]
    fn a_file_that_does_not_parse_is_a_diagnostic() {
        let analysis = analysis_of("broken");
        let diagnostic = analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.file.as_deref() == Some("bad/tests/unparsable.rs"))
            .unwrap();
        assert_eq!(diagnostic.level, "warning");
        assert!(diagnostic.line.is_some());
        assert!(functions(&analysis).contains_key("good::double"));
    }
}
