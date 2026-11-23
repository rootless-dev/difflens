//! What an analyzer prints when it prepares and runs the tests
//! (`analyzers/protocol/tests.schema.json`) and when it runs the static
//! checkers (`checks.schema.json`). Objects the schemas close with
//! `additionalProperties: false` reject unknown fields here too.

use serde::{Deserialize, Serialize};

use crate::analysis::{invalid_document, parse_versioned};
use crate::error::{core_text, escaped, CoreText};
use crate::model::{AnalyzerIdentity, PackageOutcomeKind, StaticCheck, TestOutcome, TestProgress};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareOutput {
    pub protocol: u64,
    /// Absolute folders the test step writes to outside its tree.
    pub writable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestOutput {
    pub protocol: u64,
    pub language: String,
    pub runner: AnalyzerIdentity,
    pub race: bool,
    pub tests: Vec<RunnerTest>,
    pub packages: Vec<RunnerPackage>,
    pub coverage: Option<Coverage>,
    pub races: Vec<RunnerRace>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerTest {
    pub container: String,
    /// Container of the package it ran in; missing from runs kept before
    /// runners reported it, which read it as `container`.
    #[serde(default)]
    pub package: Option<String>,
    pub name: String,
    /// Where `name` nests; missing from runs kept before runners reported
    /// it, which read it as `name` split at every "/" (see `levels`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<Vec<String>>,
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub outcome: TestOutcome,
    pub duration_ms: u64,
    pub output: Option<String>,
    /// Missing from runs kept before runners reported it.
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerPackage {
    pub container: String,
    pub outcome: PackageOutcomeKind,
    pub duration_ms: u64,
    pub output: Option<String>,
    /// Missing from runs kept before runners reported it.
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Coverage {
    pub blocks: Vec<CoverageBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageBlock {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub statements: u32,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerRace {
    pub container: String,
    pub test: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChecksOutput {
    pub protocol: u64,
    pub language: String,
    pub runner: AnalyzerIdentity,
    pub checks: Vec<StaticCheck>,
    pub diagnostics: Vec<String>,
}

impl RunnerTest {
    /// Container of the package it ran in.
    pub fn package(&self) -> &str {
        self.package.as_deref().unwrap_or(&self.container)
    }

    /// Where its name nests, from the top-level test down: as the runner
    /// reported it, or for a run kept before runners did, the name split at
    /// every "/" (Go's subtests, which were the only ones then).
    pub fn levels(&self) -> Vec<&str> {
        match &self.hierarchy {
            Some(levels) => levels.iter().map(String::as_str).collect(),
            None => self.name.split('/').collect(),
        }
    }

    /// A test of its own, not a subtest.
    pub fn is_top_level(&self) -> bool {
        match &self.hierarchy {
            Some(levels) => levels.len() <= 1,
            None => !self.name.contains('/'),
        }
    }
}

/// The file `test --progress` keeps up to date.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgressOutput {
    pub protocol: u64,
    pub packages_done: u32,
    pub packages_total: u32,
    pub tests: u32,
    pub failed: u32,
    pub current: Option<String>,
}

/// Parses a progress file; None when it is not one of this protocol.
pub fn parse_progress(bytes: &[u8]) -> Option<TestProgress> {
    let progress: ProgressOutput = serde_json::from_slice(bytes).ok()?;
    if progress.protocol != 1 {
        return None;
    }
    Some(TestProgress {
        packages_done: progress.packages_done,
        packages_total: progress.packages_total,
        tests: progress.tests,
        failed: progress.failed,
        current: progress.current,
    })
}

/// Parses the output of `prepare`. Relative folders are dropped: only
/// absolute ones can be opened in the sandbox.
pub fn parse_prepare(
    bytes: &[u8],
    analyzer: impl Into<CoreText>,
) -> Result<PrepareOutput, CoreText> {
    let mut prepared: PrepareOutput = parse_versioned(bytes, &analyzer.into(), "prepare report")?;
    prepared
        .writable
        .retain(|folder| std::path::Path::new(folder).is_absolute());
    Ok(prepared)
}

/// "it reports language "…" instead of "…"", unless `reported` is one of
/// `expected`.
pub(crate) fn check_language(reported: &str, expected: &[&str]) -> Result<(), CoreText> {
    if expected.contains(&reported) {
        return Ok(());
    }
    Err(core_text!(
        AnalysisWrongLanguage,
        "it reports language \"{reported}\" instead of \"{expected}\"",
        reported = escaped(reported),
        expected = expected
            .iter()
            .map(|language| escaped(language))
            .collect::<Vec<_>>()
            .join("\", \"")
    ))
}

/// Parses and checks the output of `test`, which must report one of
/// `expected_languages`.
pub fn parse_tests(
    bytes: &[u8],
    expected_languages: &[&str],
    analyzer: impl Into<CoreText>,
) -> Result<TestOutput, CoreText> {
    let analyzer = analyzer.into();
    let invalid = |detail: CoreText| invalid_document(&analyzer, "test report", detail);
    let output: TestOutput = parse_versioned(bytes, &analyzer, "test report")?;
    check_language(&output.language, expected_languages).map_err(invalid)?;
    if output.tests.iter().any(|test| test.line == Some(0)) {
        return Err(invalid(core_text!(TestOnLineZero, "a test is on line 0")));
    }
    for test in &output.tests {
        let Some(levels) = &test.hierarchy else {
            continue;
        };
        if levels.is_empty() || levels.join("/") != test.name {
            return Err(invalid(core_text!(
                TestHierarchyMismatch,
                "the hierarchy of the test \"{name}\" does not make up its name",
                name = escaped(&test.name)
            )));
        }
    }
    let blocks = output.coverage.iter().flat_map(|coverage| &coverage.blocks);
    for block in blocks {
        if block.start_line == 0 || block.end_line < block.start_line {
            return Err(invalid(core_text!(
                CoverageBlockLinesInvalid,
                "a coverage block of \"{file}\" has lines {start} to {end}",
                file = escaped(&block.file),
                start = block.start_line,
                end = block.end_line
            )));
        }
    }
    Ok(output)
}

/// Parses and checks the output of `check`, which must report one of
/// `expected_languages`.
pub fn parse_checks(
    bytes: &[u8],
    expected_languages: &[&str],
    analyzer: impl Into<CoreText>,
) -> Result<ChecksOutput, CoreText> {
    let analyzer = analyzer.into();
    let invalid = |detail: CoreText| invalid_document(&analyzer, "check report", detail);
    let output: ChecksOutput = parse_versioned(bytes, &analyzer, "check report")?;
    check_language(&output.language, expected_languages).map_err(invalid)?;
    let findings = output.checks.iter().flat_map(|check| &check.findings);
    if findings.into_iter().any(|finding| finding.line == 0) {
        return Err(invalid(core_text!(
            FindingOnLineZero,
            "a finding is on line 0"
        )));
    }
    Ok(output)
}

#[cfg(test)]
pub mod fixtures {
    //! Runner outputs for tests.

    use super::*;

    pub fn identity() -> AnalyzerIdentity {
        AnalyzerIdentity {
            name: "difflens-analyzer-go".to_string(),
            version: "0.0.1".to_string(),
        }
    }

    pub fn output(tests: Vec<RunnerTest>, blocks: Option<Vec<CoverageBlock>>) -> TestOutput {
        TestOutput {
            protocol: 1,
            language: "go".to_string(),
            runner: identity(),
            race: true,
            tests,
            packages: vec![RunnerPackage {
                container: "pkg".to_string(),
                outcome: PackageOutcomeKind::Pass,
                duration_ms: 12,
                output: None,
                command: None,
            }],
            coverage: blocks.map(|blocks| Coverage { blocks }),
            races: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// A passing test `name` of package "pkg" defined by `function`.
    pub fn test(name: &str, function: Option<&str>) -> RunnerTest {
        RunnerTest {
            container: "pkg".to_string(),
            package: Some("pkg".to_string()),
            name: name.to_string(),
            hierarchy: None,
            function: function.map(str::to_string),
            file: None,
            line: None,
            outcome: TestOutcome::Pass,
            duration_ms: 3,
            output: None,
            command: None,
        }
    }

    pub fn failing(mut test: RunnerTest, output: &str) -> RunnerTest {
        test.outcome = TestOutcome::Fail;
        test.output = Some(output.to_string());
        test
    }

    pub fn block(file: &str, start_line: u32, end_line: u32, count: u64) -> CoverageBlock {
        CoverageBlock {
            file: file.to_string(),
            start_line,
            end_line,
            statements: 1,
            count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CheckStatus;
    use serde_json::json;

    fn test_document() -> serde_json::Value {
        json!({
            "protocol": 1, "language": "go",
            "runner": { "name": "difflens-analyzer-go", "version": "1.0.0" },
            "race": true,
            "tests": [{
                "container": "internal/refund", "name": "TestRequest/declines",
                "function": "example.com/p/internal/refund.TestRequest",
                "file": "internal/refund/refund_test.go", "line": 10,
                "outcome": "fail", "durationMs": 4, "output": "boom"
            }],
            "packages": [{ "container": "internal/refund", "outcome": "build_failed", "durationMs": 0, "output": "x" }],
            "coverage": { "blocks": [{ "file": "a.go", "startLine": 1, "endLine": 2, "statements": 1, "count": 0 }] },
            "races": [{ "container": "internal/refund", "test": null, "output": "WARNING: DATA RACE" }],
            "diagnostics": ["cgo is off"]
        })
    }

    fn parse_test_value(value: &serde_json::Value) -> Result<TestOutput, CoreText> {
        parse_tests(value.to_string().as_bytes(), &["go"], "The Go analyzer")
    }

    #[test]
    fn parses_a_test_report_and_round_trips() {
        let output = parse_test_value(&test_document()).unwrap();
        assert_eq!(output.tests[0].outcome, TestOutcome::Fail);
        assert_eq!(output.packages[0].outcome, PackageOutcomeKind::BuildFailed);
        assert_eq!(output.coverage.as_ref().unwrap().blocks[0].end_line, 2);
        let again = serde_json::to_vec(&output).unwrap();
        assert_eq!(parse_tests(&again, &["go"], "x").unwrap(), output);

        let mut no_coverage = test_document();
        no_coverage["coverage"] = json!(null);
        assert!(parse_test_value(&no_coverage).unwrap().coverage.is_none());
    }

    #[test]
    fn rejects_bad_test_reports() {
        for pointer in [
            "",
            "/runner",
            "/tests/0",
            "/packages/0",
            "/coverage",
            "/races/0",
        ] {
            let mut value = test_document();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("surprise".to_string(), json!(1));
            let error = parse_test_value(&value).unwrap_err();
            assert!(error.contains("surprise"), "{pointer}: {error}");
        }
        let mut backwards = test_document();
        backwards["coverage"]["blocks"][0]["endLine"] = json!(0);
        assert!(parse_test_value(&backwards).is_err());
        let mut python = test_document();
        python["language"] = json!("python");
        assert!(parse_test_value(&python).unwrap_err().contains("python"));
        let mut future = test_document();
        future["protocol"] = json!(2);
        assert!(parse_test_value(&future)
            .unwrap_err()
            .contains("speaks protocol 2"));
        let error = parse_tests(b"nope", &["go"], "The Go analyzer").unwrap_err();
        assert!(
            error.starts_with("The Go analyzer printed an invalid test report"),
            "{error}"
        );
    }

    #[test]
    fn a_test_names_the_package_it_ran_in_or_falls_back_to_its_container() {
        let mut value = test_document();
        value["tests"][0]["package"] = json!("internal/refund/v2");
        let output = parse_test_value(&value).unwrap();
        assert_eq!(output.tests[0].package(), "internal/refund/v2");
        // A run kept before runners reported it.
        let output = parse_test_value(&test_document()).unwrap();
        assert_eq!(output.tests[0].package, None);
        assert_eq!(output.tests[0].package(), "internal/refund");
    }

    #[test]
    fn parses_progress_and_ignores_what_is_not_progress() {
        let progress = parse_progress(
            br#"{"protocol":1,"packagesDone":48,"packagesTotal":167,"tests":2611,"failed":0,"current":"app/billing"}"#,
        )
        .unwrap();
        assert_eq!(
            progress,
            TestProgress {
                packages_done: 48,
                packages_total: 167,
                tests: 2611,
                failed: 0,
                current: Some("app/billing".to_string()),
            }
        );
        assert!(parse_progress(br#"{"protocol":2,"packagesDone":0,"packagesTotal":0,"tests":0,"failed":0,"current":null}"#).is_none());
        assert!(parse_progress(b"{\"protocol\":1,\"packagesDo").is_none());
    }

    #[test]
    fn prepare_keeps_only_absolute_folders() {
        let prepared = parse_prepare(
            br#"{"protocol":1,"writable":["/cache/go-build","relative"]}"#,
            "x",
        )
        .unwrap();
        assert_eq!(prepared.writable, vec!["/cache/go-build"]);
        assert!(parse_prepare(br#"{"protocol":1}"#, "x").is_err());
    }

    #[test]
    fn parses_a_check_report() {
        let value = json!({
            "protocol": 1, "language": "go",
            "runner": { "name": "a", "version": "1" },
            "checks": [
                { "tool": "go vet", "status": "findings", "message": null,
                  "findings": [{ "file": "a.go", "line": 3, "message": "bad printf", "rule": "printf" }] },
                { "tool": "staticcheck", "status": "unavailable", "message": "not installed", "findings": [] }
            ],
            "diagnostics": []
        });
        let output = parse_checks(value.to_string().as_bytes(), &["go"], "x").unwrap();
        assert_eq!(output.checks[0].status, CheckStatus::Findings);
        assert_eq!(output.checks[1].message.as_deref(), Some("not installed"));

        let mut zero = value.clone();
        zero["checks"][0]["findings"][0]["line"] = json!(0);
        assert!(parse_checks(zero.to_string().as_bytes(), &["go"], "x").is_err());
        let mut extra = value.clone();
        extra["checks"][0]["extra"] = json!(true);
        assert!(parse_checks(extra.to_string().as_bytes(), &["go"], "x").is_err());

        // Kept from before severities: a warning.
        let finding = &output.checks[0].findings[0];
        assert_eq!(finding.severity, crate::model::CheckSeverity::Warning);
        let mut severe = value.clone();
        severe["checks"][0]["findings"][0]["severity"] = json!("error");
        let output = parse_checks(severe.to_string().as_bytes(), &["go"], "x").unwrap();
        assert_eq!(output.checks[0].findings[0].severity, crate::model::CheckSeverity::Error);
        let mut unknown = value;
        unknown["checks"][0]["findings"][0]["severity"] = json!("fatal");
        assert!(parse_checks(unknown.to_string().as_bytes(), &["go"], "x").is_err());
    }

    #[test]
    fn a_test_says_where_its_name_nests_or_is_split_at_slashes() {
        let mut value = test_document();
        value["tests"][0]["hierarchy"] = json!(["TestRequest", "declines"]);
        value["tests"][0]["command"] = json!("go test -run '^TestRequest$/^declines$' ./internal/refund");
        value["packages"][0]["command"] = json!("go test ./internal/refund");
        let output = parse_test_value(&value).unwrap();
        let test = &output.tests[0];
        assert_eq!(test.levels(), ["TestRequest", "declines"]);
        assert!(!test.is_top_level());
        assert!(test.command.as_deref().unwrap().starts_with("go test -run"));
        assert_eq!(output.packages[0].command.as_deref(), Some("go test ./internal/refund"));
        let again = serde_json::to_vec(&output).unwrap();
        assert_eq!(parse_tests(&again, &["go"], "x").unwrap(), output);

        // One level, whatever the name holds.
        let mut doc = test_document();
        doc["tests"][0]["name"] = json!("src/lib.rs - add (line 5)");
        doc["tests"][0]["hierarchy"] = json!(["src/lib.rs - add (line 5)"]);
        let output = parse_test_value(&doc).unwrap();
        assert_eq!(output.tests[0].levels(), ["src/lib.rs - add (line 5)"]);
        assert!(output.tests[0].is_top_level());

        // Kept before runners reported it: Go's subtests.
        let output = parse_test_value(&test_document()).unwrap();
        assert_eq!(output.tests[0].hierarchy, None);
        assert_eq!(output.tests[0].levels(), ["TestRequest", "declines"]);
        assert!(!output.tests[0].is_top_level());
        assert_eq!((output.tests[0].command.as_deref(), output.packages[0].command.as_deref()), (None, None));
        let again = serde_json::to_value(&output).unwrap();
        assert!(again["tests"][0].get("hierarchy").is_none(), "{again}");

        for wrong in [json!([]), json!(["TestRequest"]), json!(["TestRequest", "declines", "x"])] {
            let mut value = test_document();
            value["tests"][0]["hierarchy"] = wrong.clone();
            let error = parse_test_value(&value).unwrap_err();
            assert!(error.contains("does not make up its name"), "{wrong}: {error}");
        }
    }
}
