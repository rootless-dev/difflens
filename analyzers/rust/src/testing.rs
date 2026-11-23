//! Helpers of the unit tests: the fixtures of `testdata/`, copied into a
//! folder of their own when a test builds or runs them (they are never
//! written in place), and the protocol's schemas, to check documents
//! against.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

pub fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// The schema `name` of `analyzers/protocol/`.
pub fn schema(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../protocol")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// The ways `document` breaks `node` of the schema `root` (empty when it
/// matches). It knows the keywords the protocol's schemas use and panics
/// on any other, so that a schema change cannot pass unchecked.
pub fn validate(document: &Value, root: &Value, node: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    check(document, root, node, "$", &mut problems);
    problems
}

fn check(value: &Value, root: &Value, node: &Value, at: &str, problems: &mut Vec<String>) {
    const KNOWN: &[&str] = &[
        "$schema",
        "$id",
        "$defs",
        "$ref",
        "title",
        "description",
        "type",
        "const",
        "enum",
        "minimum",
        "minLength",
        "required",
        "properties",
        "additionalProperties",
        "items",
        "prefixItems",
        "minItems",
        "maxItems",
    ];
    let keywords = node.as_object().expect("a schema node is an object");
    if let Some(unknown) = keywords.keys().find(|key| !KNOWN.contains(&key.as_str())) {
        panic!("{at}: the schema keyword {unknown} is not supported here");
    }
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .expect("only references to $defs");
        return check(value, root, &root["$defs"][name], at, problems);
    }
    if let Some(kinds) = node.get("type") {
        let kinds: Vec<&str> = match kinds {
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            kind => vec![kind.as_str().expect("a type name")],
        };
        let is = |kind: &str| match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            other => panic!("{at}: type {other} is not supported here"),
        };
        if !kinds.iter().any(|kind| is(kind)) {
            problems.push(format!("{at}: {value} is not of type {kinds:?}"));
            return;
        }
    }
    if let Some(expected) = node.get("const") {
        if value != expected {
            problems.push(format!("{at}: {value} is not {expected}"));
        }
    }
    if let Some(options) = node.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            problems.push(format!("{at}: {value} is not one of {options:?}"));
        }
    }
    if let (Some(minimum), Some(number)) =
        (node.get("minimum").and_then(Value::as_f64), value.as_f64())
    {
        if number < minimum {
            problems.push(format!("{at}: {value} is below {minimum}"));
        }
    }
    if let (Some(minimum), Some(text)) = (
        node.get("minLength").and_then(Value::as_u64),
        value.as_str(),
    ) {
        if (text.chars().count() as u64) < minimum {
            problems.push(format!("{at}: {value} is shorter than {minimum}"));
        }
    }
    if let Value::Object(object) = value {
        for key in node
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(key) {
                problems.push(format!("{at}: {key} is missing"));
            }
        }
        let properties = node.get("properties").and_then(Value::as_object);
        for (key, item) in object {
            match properties.and_then(|properties| properties.get(key)) {
                Some(property) => check(item, root, property, &format!("{at}.{key}"), problems),
                None if node.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    problems.push(format!("{at}: {key} is not a field of the schema"));
                }
                None => {}
            }
        }
    }
    if let Value::Array(items) = value {
        let count = |keyword: &str| node.get(keyword).and_then(Value::as_u64);
        if count("minItems").is_some_and(|minimum| (items.len() as u64) < minimum) {
            problems.push(format!("{at}: too few items"));
        }
        if count("maxItems").is_some_and(|maximum| (items.len() as u64) > maximum) {
            problems.push(format!("{at}: too many items"));
        }
        let prefix = node.get("prefixItems").and_then(Value::as_array);
        for (index, item) in items.iter().enumerate() {
            let schema = prefix
                .and_then(|prefix| prefix.get(index))
                .or_else(|| node.get("items"));
            if let Some(schema) = schema {
                check(item, root, schema, &format!("{at}[{index}]"), problems);
            }
        }
    }
}

/// A temporary folder, removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "difflens-rust-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    /// A copy of the fixture `name` in a new folder, at `<folder>/tree`.
    pub fn with_fixture(name: &str) -> (Self, PathBuf) {
        let folder = Self::new();
        let tree = folder.0.join("tree");
        crate::checks::copy_tree(&testdata(name), &tree).unwrap();
        (folder, tree)
    }
}

/// Writes at `root` a crate `name` without dependencies, its own
/// workspace, with `files` (path, text) besides its Cargo.toml.
pub fn write_crate(root: &Path, name: &str, files: &[(&str, &str)]) {
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"
    );
    for (path, text) in [("Cargo.toml", manifest.as_str())]
        .into_iter()
        .chain(files.iter().copied())
    {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// Sets every file under `root` modified long ago, as a copy that keeps
/// old mtimes leaves them.
pub fn age_tree(root: &Path) {
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            age_tree(&path);
        } else {
            std::fs::File::open(&path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
