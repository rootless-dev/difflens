//! What a language analyzer prints, as described by
//! `analyzers/protocol/analysis.schema.json`. Objects the schema closes with
//! `additionalProperties: false` reject unknown fields here too.

use serde::{Deserialize, Serialize};

use crate::error::{core_text, escaped, CoreText, Reason};
use crate::model::{AnalyzerIdentity, CallKind, CallResolution, EntryPointKind, Relation};

/// The protocol version this core understands.
pub const PROTOCOL_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Analysis {
    pub protocol: u64,
    pub language: String,
    pub analyzer: AnalyzerIdentity,
    pub functions: Vec<Function>,
    pub calls: Vec<Call>,
    #[serde(default)]
    pub externals: Vec<External>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Function {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub container: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub generated: bool,
    /// Required: an analysis written before this field existed (such as a
    /// cache file of analyzer 0.1.0) fails to parse and is analysed again.
    pub test: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_points: Vec<ProtocolEntryPoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolEntryPoint {
    pub kind: EntryPointKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Call {
    pub caller: String,
    pub callee: String,
    pub line: u32,
    pub kind: CallKind,
    pub resolution: CallResolution,
    /// Required: an analysis written before this field existed (an older
    /// analyzer's cache file) fails to parse and is analysed again.
    pub relation: Relation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct External {
    pub id: String,
    pub name: String,
    pub container: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Only the version, read first so that a document of another protocol is
/// reported as such rather than as a pile of unexpected fields.
#[derive(Deserialize)]
struct ProtocolProbe {
    protocol: serde_json::Value,
}

/// Parses and checks one analyzer output. The error is a sentence for the
/// user; `analyzer` names the analyzer in it, such as "The Go analyzer"
/// (`AnalyzerSpec::title`). The document must report one of
/// `expected_languages` (an analyzer may cover several, such as JavaScript
/// and TypeScript).
pub fn parse(
    bytes: &[u8],
    expected_languages: &[&str],
    analyzer: impl Into<CoreText>,
) -> Result<Analysis, CoreText> {
    let analyzer = analyzer.into();
    let invalid = |detail: CoreText| invalid_document(&analyzer, "analysis", detail);
    let analysis: Analysis = parse_versioned(bytes, &analyzer, "analysis")?;
    if !expected_languages.contains(&analysis.language.as_str()) {
        return Err(invalid(core_text!(
            AnalysisWrongLanguage,
            "it reports language \"{reported}\" instead of \"{expected}\"",
            reported = escaped(&analysis.language),
            expected = expected_languages
                .iter()
                .map(|language| escaped(language))
                .collect::<Vec<_>>()
                .join("\", \"")
        )));
    }
    check_lines(&analysis).map_err(invalid)?;
    Ok(analysis)
}

/// "{analyzer} printed an invalid {document}: {detail}", worded by the
/// document ("analysis", "prepare report", "test report", "check report").
pub fn invalid_document(analyzer: &CoreText, document: &str, detail: CoreText) -> CoreText {
    let reason = match document {
        "analysis" => Reason::InvalidAnalysis,
        "prepare report" => Reason::InvalidPrepareReport,
        "test report" => Reason::InvalidTestReport,
        "duplicate code report" => Reason::InvalidDuplicateReport,
        "mutation report" => Reason::InvalidMutationReport,
        _ => Reason::InvalidCheckReport,
    };
    CoreText::new(
        reason,
        format!("{analyzer} printed an invalid {document}: {detail}"),
    )
    .param("analyzer", analyzer.clone())
    .param("detail", detail)
    .verified()
}

/// Reads one document of any analyzer command, after checking it speaks this
/// protocol version. `document` names it in errors: "analysis".
pub fn parse_versioned<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    analyzer: &CoreText,
    document: &str,
) -> Result<T, CoreText> {
    let invalid =
        |error: serde_json::Error| invalid_document(analyzer, document, error.to_string().into());
    let probe: ProtocolProbe = serde_json::from_slice(bytes).map_err(invalid)?;
    if probe.protocol.as_u64() != Some(PROTOCOL_VERSION) {
        return Err(core_text!(
            ProtocolMismatch,
            "{analyzer} speaks protocol {spoken}, but this version of DiffLens understands \
             protocol {understood}. Rebuild the analyzer or update DiffLens.",
            analyzer = analyzer.clone(),
            spoken = probe.protocol.to_string(),
            understood = PROTOCOL_VERSION
        ));
    }
    serde_json::from_slice(bytes).map_err(invalid)
}

/// Line numbers are 1-based and a function does not end before it starts.
fn check_lines(analysis: &Analysis) -> Result<(), CoreText> {
    for function in &analysis.functions {
        if function.start_line == 0 || function.end_line < function.start_line {
            return Err(core_text!(
                FunctionLinesInvalid,
                "function \"{function}\" has lines {start} to {end}",
                function = escaped(&function.id),
                start = function.start_line,
                end = function.end_line
            ));
        }
        let entry_on_line_zero = function
            .entry_points
            .iter()
            .any(|entry| entry.line == Some(0));
        if function.complexity == Some(0) || entry_on_line_zero {
            return Err(core_text!(
                FunctionValueBelowOne,
                "function \"{function}\" has a value below 1",
                function = escaped(&function.id)
            ));
        }
    }
    if let Some(call) = analysis.calls.iter().find(|call| call.line == 0) {
        return Err(core_text!(
            CallOnLineZero,
            "the call from \"{caller}\" to \"{callee}\" is on line 0",
            caller = escaped(&call.caller),
            callee = escaped(&call.callee)
        ));
    }
    if analysis.diagnostics.iter().any(|item| item.line == Some(0)) {
        return Err(core_text!(
            DiagnosticOnLineZero,
            "a diagnostic is on line 0"
        ));
    }
    Ok(())
}

#[cfg(test)]
pub mod fixtures {
    //! Hand-written analyses for tests.

    use super::*;

    pub fn analysis(functions: Vec<Function>, calls: Vec<Call>) -> Analysis {
        Analysis {
            protocol: PROTOCOL_VERSION,
            language: "go".to_string(),
            analyzer: AnalyzerIdentity {
                name: "difflens-analyzer-go".to_string(),
                version: "0.0.1".to_string(),
            },
            functions,
            calls,
            externals: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// A function `id` in `file`, named after the last part of its id.
    pub fn function(id: &str, file: &str, start_line: u32, end_line: u32) -> Function {
        let name = id.rsplit('.').next().unwrap_or(id).to_string();
        Function {
            id: id.to_string(),
            name: name.clone(),
            qualified_name: format!("pkg.{name}"),
            container: "pkg".to_string(),
            file: file.to_string(),
            start_line,
            end_line,
            generated: false,
            test: false,
            complexity: None,
            entry_points: Vec::new(),
        }
    }

    /// `function` defined in test code.
    pub fn in_test(mut function: Function) -> Function {
        function.test = true;
        function
    }

    pub fn entry(mut function: Function, label: &str) -> Function {
        function.entry_points.push(ProtocolEntryPoint {
            kind: EntryPointKind::HttpRoute,
            label: label.to_string(),
            file: None,
            line: None,
        });
        function
    }

    pub fn call(caller: &str, callee: &str, line: u32) -> Call {
        Call {
            caller: caller.to_string(),
            callee: callee.to_string(),
            line,
            kind: CallKind::Always,
            resolution: CallResolution::Static,
            relation: Relation::Call,
            evidence: None,
        }
    }

    /// `caller` registers `callee` with a known mechanism, such as a route.
    pub fn registration(caller: &str, callee: &str, line: u32, evidence: &str) -> Call {
        Call {
            kind: CallKind::Async,
            relation: Relation::Registration,
            evidence: Some(evidence.to_string()),
            ..call(caller, callee, line)
        }
    }

    /// `caller` passes `callee` as a value.
    pub fn reference(caller: &str, callee: &str, line: u32) -> Call {
        Call {
            kind: CallKind::Async,
            relation: Relation::Reference,
            ..call(caller, callee, line)
        }
    }

    pub fn external(id: &str, container: &str) -> External {
        External {
            id: id.to_string(),
            name: id.rsplit('.').next().unwrap_or(id).to_string(),
            container: container.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> serde_json::Value {
        json!({
            "protocol": 1,
            "language": "go",
            "analyzer": { "name": "difflens-analyzer-go", "version": "1.2.3" },
            "functions": [{
                "id": "example.com/a.Run", "name": "Run", "qualifiedName": "a.Run",
                "container": "a", "file": "a/run.go", "startLine": 3, "endLine": 9,
                "generated": false, "test": false, "complexity": 2,
                "entryPoints": [{ "kind": "http_route", "label": "POST /runs", "line": 4 }]
            }],
            "calls": [{
                "caller": "example.com/a.Run", "callee": "fmt.Println", "line": 5,
                "kind": "conditional", "resolution": "static", "relation": "call"
            }, {
                "caller": "example.com/a.Run", "callee": "example.com/a.Run", "line": 6,
                "kind": "async", "resolution": "static", "relation": "registration",
                "evidence": "route POST /runs"
            }],
            "externals": [{ "id": "fmt.Println", "name": "Println", "container": "fmt" }],
            "diagnostics": [{ "level": "warning", "message": "slow", "file": "a/run.go" }]
        })
    }

    fn parse_value(value: &serde_json::Value) -> Result<Analysis, CoreText> {
        parse(value.to_string().as_bytes(), &["go"], "The Go analyzer")
    }

    #[test]
    fn parses_a_complete_document_and_round_trips() {
        let analysis = parse_value(&document()).unwrap();
        assert_eq!(analysis.functions[0].complexity, Some(2));
        assert_eq!(
            analysis.functions[0].entry_points[0].kind,
            EntryPointKind::HttpRoute
        );
        assert_eq!(analysis.calls[0].kind, CallKind::Conditional);
        assert_eq!(
            (
                analysis.calls[0].relation,
                analysis.calls[0].evidence.as_deref()
            ),
            (Relation::Call, None)
        );
        assert_eq!(
            (
                analysis.calls[1].relation,
                analysis.calls[1].evidence.as_deref()
            ),
            (Relation::Registration, Some("route POST /runs"))
        );
        let again = serde_json::to_vec(&analysis).unwrap();
        assert_eq!(parse(&again, &["go"], "x").unwrap(), analysis);
    }

    #[test]
    fn optional_parts_may_be_left_out() {
        let mut value = document();
        let object = value.as_object_mut().unwrap();
        object.remove("externals");
        let function = object["functions"][0].as_object_mut().unwrap();
        function.remove("complexity");
        function.remove("entryPoints");
        let analysis = parse_value(&value).unwrap();
        assert!(analysis.externals.is_empty());
        assert!(analysis.functions[0].entry_points.is_empty());
    }

    #[test]
    fn requires_the_test_flag_of_every_function() {
        let mut value = document();
        value["functions"][0]["test"] = json!(true);
        assert!(parse_value(&value).unwrap().functions[0].test);

        // What analyzer 0.1.0 printed, and what older cache files hold.
        value["functions"][0]
            .as_object_mut()
            .unwrap()
            .remove("test");
        let error = parse_value(&value).unwrap_err();
        assert!(error.contains("test"), "{error}");
    }

    #[test]
    fn requires_the_relation_of_every_call() {
        let mut value = document();
        value["calls"][1]["relation"] = json!("reference");
        value["calls"][1]
            .as_object_mut()
            .unwrap()
            .remove("evidence");
        assert_eq!(
            parse_value(&value).unwrap().calls[1].relation,
            Relation::Reference
        );

        // What analyzers before the relation printed, and what older cache files hold.
        value["calls"][0]
            .as_object_mut()
            .unwrap()
            .remove("relation");
        let error = parse_value(&value).unwrap_err();
        assert!(error.contains("relation"), "{error}");

        let mut unknown = document();
        unknown["calls"][0]["relation"] = json!("inheritance");
        assert!(parse_value(&unknown).is_err());
    }

    #[test]
    fn rejects_unknown_fields_where_the_schema_closes_the_object() {
        for pointer in ["", "/analyzer", "/functions/0", "/calls/0", "/externals/0"] {
            let mut value = document();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("surprise".to_string(), json!(true));
            let error = parse_value(&value).unwrap_err();
            assert!(error.contains("surprise"), "{pointer}: {error}");
        }
    }

    #[test]
    fn rejects_another_protocol_by_name() {
        let mut value = document();
        value["protocol"] = json!(2);
        value["somethingNew"] = json!([]);
        let error = parse_value(&value).unwrap_err();
        assert!(error.contains("protocol 2"), "{error}");
    }

    #[test]
    fn rejects_garbage_bad_enums_lines_and_the_wrong_language() {
        assert!(parse(b"not json", &["go"], "x").is_err());
        assert!(parse(b"", &["go"], "x").is_err());
        let mut bad_kind = document();
        bad_kind["calls"][0]["kind"] = json!("sometimes");
        assert!(parse_value(&bad_kind).is_err());
        let mut backwards = document();
        backwards["functions"][0]["endLine"] = json!(1);
        assert!(parse_value(&backwards).is_err());
        let mut zero = document();
        zero["calls"][0]["line"] = json!(0);
        assert!(parse_value(&zero).is_err());
        let mut python = document();
        python["language"] = json!("python");
        assert!(parse_value(&python).unwrap_err().contains("python"));
    }

    #[test]
    fn an_analyzer_of_several_languages_may_report_any_of_them() {
        let mut typescript = document();
        typescript["language"] = json!("typescript");
        let bytes = typescript.to_string();
        let both = ["javascript", "typescript"];
        assert_eq!(parse(bytes.as_bytes(), &both, "x").unwrap().language, "typescript");
        let error = parse(bytes.as_bytes(), &["javascript", "rust"], "x").unwrap_err();
        assert!(
            error.contains("instead of \"javascript\", \"rust\""),
            "{error}"
        );
    }
}
