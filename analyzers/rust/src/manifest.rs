//! What the analyzer reads from a Cargo.toml itself, without a TOML
//! library: the few keys `cargo metadata` does not give (`harness`), and,
//! when cargo cannot be run, enough to find the crates by Cargo's
//! conventions. It reads the common shapes of a manifest (tables, arrays of
//! tables, strings, booleans, arrays of strings, inline tables); anything
//! else is skipped.

use std::collections::BTreeMap;

/// One `[table]` or `[[array.table]]` of a manifest, with its plain keys.
#[derive(Debug, Default, Clone)]
pub struct Table {
    pub name: String,
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Bool(bool),
    Array(Vec<String>),
    /// An inline table, with its plain keys.
    Inline(BTreeMap<String, Value>),
    Other,
}

impl Table {
    pub fn string(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(Value::String(text)) => Some(text),
            _ => None,
        }
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.values.get(key) {
            Some(Value::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn array(&self, key: &str) -> Vec<String> {
        match self.values.get(key) {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        }
    }
}

/// The tables of a manifest in order; keys before any header go to a table
/// named "".
pub fn parse(text: &str) -> Vec<Table> {
    let mut tables = vec![Table::default()];
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        let line = strip_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line
            .strip_prefix("[[")
            .and_then(|rest| rest.strip_suffix("]]"))
        {
            tables.push(Table {
                name: header.trim().to_string(),
                ..Table::default()
            });
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            tables.push(Table {
                name: header.trim().to_string(),
                ..Table::default()
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_string();
        // A multi-line array or inline table goes on until it closes.
        while !balanced(&value) {
            let Some(next) = lines.next() else { break };
            value.push(' ');
            value.push_str(strip_comment(next).trim());
        }
        let key = unquote(key.trim());
        if let Some(table) = tables.last_mut() {
            table.values.insert(key, parse_value(&value));
        }
    }
    tables
}

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '#') => return &line[..index],
            _ => {}
        }
    }
    line
}

fn balanced(value: &str) -> bool {
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    for c in value.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_string();
        }
    }
    text.to_string()
}

fn parse_value(value: &str) -> Value {
    let value = value.trim();
    match value {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if value.starts_with('"') || value.starts_with('\'') {
        return Value::String(unquote(value));
    }
    if let Some(inner) = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        let items = split_top_level(inner)
            .into_iter()
            .map(|item| item.trim().to_string())
            .filter(|item| item.starts_with('"') || item.starts_with('\''))
            .map(|item| unquote(&item))
            .collect();
        return Value::Array(items);
    }
    if let Some(inner) = value
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    {
        let mut map = BTreeMap::new();
        for pair in split_top_level(inner) {
            if let Some((key, value)) = pair.split_once('=') {
                map.insert(unquote(key), parse_value(value));
            }
        }
        return Value::Inline(map);
    }
    Value::Other
}

/// Splits on commas outside quotes, brackets and braces.
fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            (None, ',') if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

/// The names of the `[[test]]`, `[[bench]]`, `[[bin]]` and `[[example]]`
/// targets, and `[lib]`, declared with `harness = false`, as "kind:name"
/// ("lib" for the library).
pub fn harnessless(tables: &[Table]) -> Vec<String> {
    tables
        .iter()
        .filter(|table| table.bool("harness") == Some(false))
        .filter_map(|table| match table.name.as_str() {
            "lib" => Some("lib".to_string()),
            "test" | "bench" | "bin" | "example" => table
                .string("name")
                .map(|name| format!("{}:{name}", table.name)),
            _ => None,
        })
        .collect()
}

/// The dependencies a manifest declares (normal, dev and build, for any
/// platform): the name code uses (the key, `-` read as `_`) and the package
/// it is (the `package` key of a renamed one).
pub fn dependencies(tables: &[Table]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let is_dependency_table = |name: &str| {
        let last = name.rsplit('.').next().unwrap_or(name);
        matches!(
            last,
            "dependencies" | "dev-dependencies" | "build-dependencies"
        )
    };
    for table in tables {
        if is_dependency_table(&table.name) {
            for (key, value) in &table.values {
                let name = key.split('.').next().unwrap_or(key);
                let package = match value {
                    Value::Inline(map) => match map.get("package") {
                        Some(Value::String(package)) => package.clone(),
                        _ => name.to_string(),
                    },
                    _ => name.to_string(),
                };
                found.push((name.replace('-', "_"), package));
            }
        } else if let Some((prefix, name)) = table.name.rsplit_once('.') {
            // [dependencies.serde]
            if is_dependency_table(prefix) {
                let package = table.string("package").unwrap_or(name).to_string();
                found.push((name.replace('-', "_"), package));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[package]
name = "demo-app" # the app
version = "0.1.0"

[workspace]
members = [
    "crates/*",
    "tools/gen", # a tool
]

[dependencies]
serde = { version = "1", features = ["derive"] }
web = { package = "actix-web", version = "4" }
tokio-util = "0.7"
clap.workspace = true

[dependencies.axum]
version = "0.7"

[target.'cfg(unix)'.dev-dependencies]
libc = "0.2"

[[test]]
name = "golden"
harness = false

[[bench]]
name = "speed"
harness = false
"#;

    #[test]
    fn reads_the_keys_cargo_metadata_does_not_give() {
        let tables = parse(MANIFEST);
        let package = tables.iter().find(|table| table.name == "package").unwrap();
        assert_eq!(package.string("name"), Some("demo-app"));
        let workspace = tables
            .iter()
            .find(|table| table.name == "workspace")
            .unwrap();
        assert_eq!(workspace.array("members"), ["crates/*", "tools/gen"]);
        assert_eq!(harnessless(&tables), ["test:golden", "bench:speed"]);
        let mut dependencies = dependencies(&tables);
        dependencies.sort();
        assert_eq!(
            dependencies,
            [
                ("axum".to_string(), "axum".to_string()),
                ("clap".to_string(), "clap".to_string()),
                ("libc".to_string(), "libc".to_string()),
                ("serde".to_string(), "serde".to_string()),
                ("tokio_util".to_string(), "tokio-util".to_string()),
                ("web".to_string(), "actix-web".to_string()),
            ]
        );
    }

    #[test]
    fn comments_and_quotes_do_not_confuse_it() {
        let tables = parse("[package]\nname = \"a#b\" # comment\nlicense = 'MIT'\n");
        assert_eq!(tables[1].string("name"), Some("a#b"));
        assert_eq!(tables[1].string("license"), Some("MIT"));
    }
}
