//! Mutation testing of a review: what an analyzer's `mutate` prints
//! (`analyzers/protocol/mutation.schema.json`), the files it is given (the
//! lines the review adds, as `dupes` gets them, and the coverage of the last
//! test run), the progress file it keeps, and the mutants as the Tests tab
//! and the signals show them: with the lines they span on the new side as
//! they are and as mutated, and the changed function holding them.
//!
//! Which mutants, how many, and which tests run them are the analyzer's
//! (it knows the language and its test framework); the core only says which
//! lines to try and reads the result. Running it is `mutation_runs`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::analysis::{invalid_document, parse_versioned, Function};
use crate::error::{core_text, escaped, CoreText};
use crate::hunks::ChangedLines;
use crate::model::{
    AnalyzerIdentity, FunctionRef, Mutant, MutantStatus, MutationOperator, MutationPhase,
    MutationProgress, MutationSummary,
};
use crate::runner_protocol::{check_language, Coverage, CoverageBlock};

/// The most mutants one run tests; past it, the analyzer samples evenly
/// across the changed functions.
pub const MAX_MUTANTS: u32 = 200;

/// Lines of a mutant shown as they are and as mutated, at most; a longer one
/// (a whole function body replaced) shows only what changes.
pub const MAX_CODE_LINES: usize = 20;

const DOCUMENT: &str = "mutation report";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutateOutput {
    pub protocol: u64,
    pub language: String,
    pub runner: AnalyzerIdentity,
    pub engine: AnalyzerIdentity,
    pub mutants: Vec<RunnerMutant>,
    pub summary: RunnerSummary,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerMutant {
    pub id: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub original: String,
    pub mutated: String,
    pub operator: MutationOperator,
    pub description: String,
    pub function: Option<String>,
    pub status: MutantStatus,
    pub killed_by: Option<String>,
    pub duration_ms: u64,
    pub cached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerSummary {
    pub candidates: u32,
    pub sampled: bool,
    pub max_mutants: u32,
    pub killed: u32,
    pub survived: u32,
    pub timeout: u32,
    pub error: u32,
    pub no_coverage: u32,
    pub cached: u32,
    pub duration_ms: u64,
}

/// Parses and checks the output of `mutate`, which must report one of
/// `expected_languages`.
pub fn parse(
    bytes: &[u8],
    expected_languages: &[&str],
    analyzer: impl Into<CoreText>,
) -> Result<MutateOutput, CoreText> {
    let analyzer = analyzer.into();
    let invalid = |detail: CoreText| invalid_document(&analyzer, DOCUMENT, detail);
    let output: MutateOutput = parse_versioned(bytes, &analyzer, DOCUMENT)?;
    check_language(&output.language, expected_languages).map_err(invalid)?;
    let mut ids = HashSet::new();
    for mutant in &output.mutants {
        let backwards = mutant.end_line < mutant.line
            || (mutant.end_line == mutant.line && mutant.end_column < mutant.column);
        if mutant.line == 0 || mutant.column == 0 || mutant.end_column == 0 || backwards {
            return Err(invalid(core_text!(
                MutantPositionInvalid,
                "a mutant in \"{file}\" goes from {line}:{column} to {end_line}:{end_column}",
                file = escaped(&mutant.file),
                line = mutant.line,
                column = mutant.column,
                end_line = mutant.end_line,
                end_column = mutant.end_column
            )));
        }
        if mutant.id.is_empty() || !ids.insert(mutant.id.as_str()) {
            return Err(invalid(core_text!(
                MutantIdRepeated,
                "the mutant id \"{id}\" is empty or used twice",
                id = escaped(&mutant.id)
            )));
        }
    }
    Ok(output)
}

/// The file `mutate --progress` keeps up to date.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProgressOutput {
    protocol: u64,
    phase: MutationPhase,
    total: u32,
    done: u32,
    killed: u32,
    survived: u32,
    timeout: u32,
    error: u32,
    current: Option<String>,
}

/// Parses a progress file; None when it is not one of this protocol.
pub fn parse_progress(bytes: &[u8]) -> Option<MutationProgress> {
    let progress: ProgressOutput = serde_json::from_slice(bytes).ok()?;
    if progress.protocol != 1 {
        return None;
    }
    Some(MutationProgress {
        phase: progress.phase,
        total: progress.total,
        done: progress.done,
        killed: progress.killed,
        survived: progress.survived,
        timeout: progress.timeout,
        error: progress.error,
        current: progress.current,
    })
}

#[derive(Serialize)]
struct CoverageDocument<'a> {
    protocol: u64,
    blocks: &'a [CoverageBlock],
}

/// The `--coverage` document of a test run's coverage
/// (`mutation.schema.json`, `$defs/coverage`).
pub fn coverage_document(coverage: &Coverage) -> Vec<u8> {
    let document = CoverageDocument {
        protocol: 1,
        blocks: &coverage.blocks,
    };
    serde_json::to_vec(&document).unwrap_or_default()
}

/// How many lines the review adds that `coverage` says ran: the lines a
/// mutant may be on. None of them means there is nothing to mutate.
pub fn candidate_lines(lines: &ChangedLines, coverage: &Coverage) -> usize {
    let mut ran: HashMap<&str, Vec<(u32, u32)>> = HashMap::new();
    for block in coverage.blocks.iter().filter(|block| block.count > 0) {
        ran.entry(block.file.as_str())
            .or_default()
            .push((block.start_line, block.end_line));
    }
    lines
        .added_ranges()
        .into_iter()
        .filter_map(|(path, _)| ran.get(path).map(|blocks| (path, blocks)))
        .map(|(path, blocks)| {
            lines
                .added_lines(path)
                .filter(|line| blocks.iter().any(|(start, end)| (start..=end).contains(&line)))
                .count()
        })
        .sum()
}

/// The summary of `output`: its counts recounted from the mutants it lists,
/// so the two never disagree; the cap as it reported it.
pub fn summary(output: &MutateOutput) -> MutationSummary {
    let mut summary = MutationSummary {
        candidates: output.summary.candidates,
        sampled: output.summary.sampled,
        max_mutants: output.summary.max_mutants,
        ..MutationSummary::default()
    };
    for mutant in &output.mutants {
        match mutant.status {
            MutantStatus::Killed => summary.killed += 1,
            MutantStatus::Survived => summary.survived += 1,
            MutantStatus::Timeout => summary.timeout += 1,
            MutantStatus::Error => summary.error += 1,
            MutantStatus::NoCoverage => summary.no_coverage += 1,
        }
        if mutant.cached {
            summary.cached += 1;
        }
    }
    summary
}

/// The mutants of `output`, with their lines read from `tree`, the tree the
/// run mutated, and placed on the innermost of `functions` (the new side's)
/// holding them: the one the analyzer named when it is a production
/// function, else the innermost one holding the mutant's line. By file, line
/// and column.
pub fn mutants(output: MutateOutput, tree: &Path, functions: &[Function]) -> Vec<Mutant> {
    let mut files: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let production: Vec<&Function> = functions.iter().filter(|function| !function.test).collect();
    let mut mutants: Vec<Mutant> = output
        .mutants
        .into_iter()
        .map(|mutant| {
            let lines = files
                .entry(mutant.file.clone())
                .or_insert_with(|| read_lines(tree, &mutant.file));
            let (before, after) = lines
                .as_deref()
                .and_then(|lines| code(lines, &mutant))
                .unwrap_or_default();
            let function = holding(&production, &mutant);
            Mutant {
                id: mutant.id,
                file: mutant.file,
                line: mutant.line,
                column: mutant.column,
                end_line: mutant.end_line,
                end_column: mutant.end_column,
                original: mutant.original,
                mutated: mutant.mutated,
                operator: mutant.operator,
                description: mutant.description,
                status: mutant.status,
                killed_by: mutant.killed_by,
                duration_ms: mutant.duration_ms,
                cached: mutant.cached,
                function,
                before,
                after,
            }
        })
        .collect();
    mutants.sort_by(|left, right| {
        (&left.file, left.line, left.column).cmp(&(&right.file, right.line, right.column))
    });
    mutants
}

fn holding(functions: &[&Function], mutant: &RunnerMutant) -> Option<FunctionRef> {
    let named = mutant
        .function
        .as_deref()
        .and_then(|id| functions.iter().find(|function| function.id == id));
    let found = named.or_else(|| {
        functions
            .iter()
            .filter(|function| {
                function.file == mutant.file
                    && (function.start_line..=function.end_line).contains(&mutant.line)
            })
            .min_by_key(|function| function.end_line - function.start_line)
    })?;
    Some(FunctionRef {
        id: found.id.clone(),
        qualified_name: found.qualified_name.clone(),
    })
}

/// The lines `mutant` spans as they are and as mutated, or None when they
/// do not hold its `original` where it says, or are too many to show.
fn code(lines: &[String], mutant: &RunnerMutant) -> Option<(Vec<String>, Vec<String>)> {
    let first = mutant.line as usize - 1;
    let last = mutant.end_line as usize - 1;
    if last >= lines.len() || last - first + 1 > MAX_CODE_LINES {
        return None;
    }
    let span = &lines[first..=last];
    let start: Vec<char> = span[0].chars().collect();
    let end: Vec<char> = span[span.len() - 1].chars().collect();
    let (from, to) = (mutant.column as usize - 1, mutant.end_column as usize - 1);
    if from > start.len() || to > end.len() || (first == last && to < from) {
        return None;
    }
    let prefix: String = start[..from].iter().collect();
    let suffix: String = end[to..].iter().collect();
    let joined = span.join("\n");
    let replaced: String = joined
        .chars()
        .skip(from)
        .take(joined.chars().count() - from - (end.len() - to))
        .collect();
    if replaced != mutant.original.replace("\r\n", "\n") {
        return None;
    }
    let after = format!("{prefix}{}{suffix}", mutant.mutated);
    let after: Vec<String> = after.split('\n').map(str::to_string).collect();
    Some((span.to_vec(), after))
}

/// The lines of `file` in `tree` without their line ends, or `None` when it
/// cannot be read or does not name a file inside the tree.
fn read_lines(tree: &Path, file: &str) -> Option<Vec<String>> {
    let relative = Path::new(file);
    if !relative.components().all(|part| matches!(part, Component::Normal(_))) {
        return None;
    }
    let bytes = fs::read(tree.join(relative)).ok()?;
    Some(
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
pub mod fixtures {
    //! Mutation reports for tests.

    use serde_json::{json, Value};

    /// A `mutate` document of language "go" listing `mutants`.
    pub fn document(mutants: Value) -> Value {
        let count = |status: &str| {
            mutants
                .as_array()
                .map_or(0, |all| all.iter().filter(|mutant| mutant["status"] == status).count())
        };
        json!({
            "protocol": 1, "language": "go",
            "runner": { "name": "difflens-analyzer-go", "version": "0.0.1" },
            "engine": { "name": "difflens-analyzer-go mutator", "version": "0.0.1" },
            "summary": {
                "candidates": mutants.as_array().map_or(0, Vec::len), "sampled": false, "maxMutants": 200,
                "killed": count("killed"), "survived": count("survived"), "timeout": count("timeout"),
                "error": count("error"), "noCoverage": count("no_coverage"), "cached": 0, "durationMs": 1200
            },
            "mutants": mutants,
            "diagnostics": ["one package skipped"]
        })
    }

    /// A single-line mutant of `file` replacing `original` at `line:column`
    /// with `mutated`.
    pub fn mutant(id: &str, file: &str, line: u32, column: u32, original: &str, mutated: &str, status: &str) -> Value {
        json!({
            "id": id, "file": file, "line": line, "column": column,
            "endLine": line, "endColumn": column + original.chars().count() as u32,
            "original": original, "mutated": mutated,
            "operator": "relational_boundary",
            "description": format!("replace {original} with {mutated}"),
            "function": null, "status": status,
            "killedBy": if status == "killed" { json!("TestWithdraw") } else { json!(null) },
            "durationMs": 40, "cached": false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{document, mutant};
    use super::*;
    use serde_json::json;

    fn parse_value(value: &serde_json::Value) -> Result<MutateOutput, CoreText> {
        parse(value.to_string().as_bytes(), &["go"], "The Go analyzer")
    }

    #[test]
    fn a_report_is_read_and_checked() {
        let value = document(json!([
            mutant("a", "bank.go", 4, 12, ">=", ">", "survived"),
            mutant("b", "bank.go", 9, 3, "+", "-", "killed"),
        ]));
        let output = parse_value(&value).unwrap();
        assert_eq!(output.mutants.len(), 2);
        assert_eq!(output.mutants[0].status, MutantStatus::Survived);
        assert_eq!(output.mutants[1].killed_by.as_deref(), Some("TestWithdraw"));
        assert_eq!(output.engine.name, "difflens-analyzer-go mutator");

        let mut wrong = value.clone();
        wrong["language"] = json!("python");
        assert!(parse_value(&wrong).unwrap_err().contains("instead of"));
        let mut extra = value.clone();
        extra["mutants"][0]["surprise"] = json!(1);
        assert!(parse_value(&extra).unwrap_err().contains("surprise"));
        let mut operator = value.clone();
        operator["mutants"][0]["operator"] = json!("teleport");
        assert!(parse_value(&operator).is_err());
        let mut backwards = value.clone();
        backwards["mutants"][0]["endColumn"] = json!(3);
        let error = parse_value(&backwards).unwrap_err();
        assert!(error.contains("goes from 4:12 to 4:3"), "{error}");
        assert!(error.contains("invalid mutation report"), "{error}");
        let mut twice = value.clone();
        twice["mutants"][1]["id"] = json!("a");
        assert!(parse_value(&twice).unwrap_err().contains("used twice"));
        let error = parse(b"nope", &["go"], "The Go analyzer").unwrap_err();
        assert!(error.starts_with("The Go analyzer printed an invalid mutation report"), "{error}");
    }

    #[test]
    fn progress_is_read_and_what_is_not_progress_ignored() {
        let progress = parse_progress(
            br#"{"protocol":1,"phase":"mutating","total":40,"done":12,"killed":9,"survived":2,"timeout":1,"error":0,"current":"bank.go:4"}"#,
        )
        .unwrap();
        assert_eq!(progress.phase, MutationPhase::Mutating);
        assert_eq!((progress.total, progress.done, progress.killed), (40, 12, 9));
        assert_eq!(progress.current.as_deref(), Some("bank.go:4"));
        assert!(parse_progress(br#"{"protocol":2,"phase":"baseline","total":0,"done":0,"killed":0,"survived":0,"timeout":0,"error":0,"current":null}"#).is_none());
        assert!(parse_progress(b"{\"protocol\":1,\"pha").is_none());
    }

    fn block(file: &str, start_line: u32, end_line: u32, count: u64) -> CoverageBlock {
        CoverageBlock {
            file: file.to_string(),
            start_line,
            end_line,
            statements: 1,
            count,
        }
    }

    #[test]
    fn the_candidates_are_the_added_lines_that_ran() {
        let lines = ChangedLines::default()
            .with_added("bank.go", 3, 8)
            .with_added("other.go", 1, 2)
            .with_removed("gone.go", 1, 4);
        let coverage = Coverage {
            blocks: vec![
                block("bank.go", 1, 4, 3),
                block("bank.go", 6, 6, 0),
                block("bank.go", 7, 12, 1),
                block("gone.go", 1, 4, 9),
            ],
        };
        // 3, 4, 7 and 8; line 6 never ran, 5 holds no statement.
        assert_eq!(candidate_lines(&lines, &coverage), 4);
        assert_eq!(candidate_lines(&lines, &Coverage { blocks: Vec::new() }), 0);

        let document: serde_json::Value = serde_json::from_slice(&coverage_document(&coverage)).unwrap();
        assert_eq!(document["protocol"], 1);
        assert_eq!(document["blocks"][2], json!({ "file": "bank.go", "startLine": 7, "endLine": 12, "statements": 1, "count": 1 }));
    }

    fn function(id: &str, file: &str, start_line: u32, end_line: u32, test: bool) -> Function {
        Function {
            id: id.to_string(),
            name: id.to_string(),
            qualified_name: format!("bank.{id}"),
            container: "bank".to_string(),
            file: file.to_string(),
            start_line,
            end_line,
            generated: false,
            test,
            complexity: Some(1),
            entry_points: Vec::new(),
        }
    }

    #[test]
    fn every_mutant_gets_its_lines_as_they_are_and_as_mutated_and_its_function() {
        let tree = tempfile::TempDir::new().unwrap();
        fs::write(
            tree.path().join("bank.go"),
            "package bank\r\n\r\nfunc Withdraw(saldo, valor int) bool {\r\n\tif saldo >= valor { // ação\r\n\t\treturn true\r\n\t}\r\n\treturn false\r\n}\r\n",
        )
        .unwrap();
        let mut multi = mutant("c", "bank.go", 4, 5, "", "", "survived");
        multi["endLine"] = json!(6);
        multi["endColumn"] = json!(3);
        multi["original"] = json!("saldo >= valor { // ação\n\t\treturn true\n\t}");
        multi["mutated"] = json!("true {}");
        let mut named = mutant("d", "bank.go", 7, 9, "false", "true", "killed");
        named["function"] = json!("bank.Withdraw");
        let value = document(json!([
            named,
            mutant("a", "bank.go", 4, 11, ">=", ">", "survived"),
            mutant("b", "bank.go", 4, 11, "<=", ">", "survived"),
            mutant("e", "../outside.go", 1, 1, "x", "y", "killed"),
            multi,
        ]));
        let functions = vec![
            function("bank.Withdraw", "bank.go", 3, 8, false),
            function("bank.TestWithdraw", "bank.go", 3, 8, true),
        ];
        let found = mutants(parse_value(&value).unwrap(), tree.path(), &functions);
        let ids: Vec<&str> = found.iter().map(|mutant| mutant.id.as_str()).collect();
        assert_eq!(ids, vec!["e", "c", "a", "b", "d"], "by file, line and column");

        let boundary = &found[2];
        assert_eq!(boundary.before, vec!["\tif saldo >= valor { // ação"]);
        assert_eq!(boundary.after, vec!["\tif saldo > valor { // ação"]);
        assert_eq!(boundary.function.as_ref().unwrap().id, "bank.Withdraw", "the production one");
        assert!(found[3].before.is_empty() && found[3].after.is_empty(), "the line does not hold it");
        assert!(found[0].before.is_empty() && found[0].function.is_none(), "outside the tree");
        let whole = &found[1];
        assert_eq!(whole.before.len(), 3);
        assert_eq!(whole.after, vec!["\tif true {}"]);
        assert_eq!(found[4].after, vec!["\treturn true"]);
        assert_eq!(found[4].function.as_ref().unwrap().qualified_name, "bank.bank.Withdraw");
    }

    #[test]
    fn the_summary_is_recounted_from_the_mutants() {
        let mut value = document(json!([
            mutant("a", "bank.go", 4, 11, ">=", ">", "survived"),
            mutant("b", "bank.go", 5, 1, "+", "-", "killed"),
            mutant("c", "bank.go", 6, 1, "++", "--", "timeout"),
        ]));
        value["mutants"][1]["cached"] = json!(true);
        value["summary"]["killed"] = json!(7);
        value["summary"]["candidates"] = json!(340);
        value["summary"]["sampled"] = json!(true);
        let summary = summary(&parse_value(&value).unwrap());
        assert_eq!(
            summary,
            MutationSummary {
                candidates: 340,
                sampled: true,
                max_mutants: 200,
                killed: 1,
                survived: 1,
                timeout: 1,
                error: 0,
                no_coverage: 0,
                cached: 1,
            }
        );
    }
}
