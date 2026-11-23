//! Duplicated code a review introduces: what an analyzer's `dupes` prints
//! (`analyzers/protocol/dupes.schema.json`), the file of added lines it is
//! given, and the duplicates as the Quality tab and the signals show them,
//! with the code of every occurrence read from the new side's tree and the
//! changed production function that holds the block.
//!
//! The analyzer only tokenizes and compares; which lines the review adds is
//! the core's (`hunks`), handed over in a file outside the tree, as the tree
//! is never written in. So is which files are translation catalogs, left out
//! of the search entirely (`TRANSLATION_DIRS`, `TRANSLATION_EXTENSIONS`).
//! Which code is a test is the analyzer's, as its language says: it searches
//! production code and test code apart and says which each duplicate is in
//! (`DuplicateScope`), so the Quality tab counts only production copies and
//! shows the tests' own copies apart.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::analysis::{invalid_document, parse_versioned};
use crate::error::{core_text, escaped, CoreText};
use crate::hunks::ChangedLines;
use crate::model::{
    AnalyzerIdentity, Duplicate, DuplicateBlock, DuplicateKind, DuplicateScope, FunctionRef,
    QualityFunction,
};
use crate::runner_protocol::check_language;

/// Lines of an occurrence kept to show, at most: enough for a helper, while
/// a whole copied file does not fill the report.
pub const MAX_CODE_LINES: usize = 80;

const DOCUMENT: &str = "duplicate code report";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DupesOutput {
    pub protocol: u64,
    pub language: String,
    pub runner: AnalyzerIdentity,
    pub min_tokens: u32,
    pub min_lines: u32,
    pub duplicates: Vec<RunnerDuplicate>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerDuplicate {
    pub kind: DuplicateKind,
    /// Required: an analyzer that does not tell tests apart is rejected
    /// rather than read as production code.
    pub scope: DuplicateScope,
    pub block: CodeRange,
    pub others: Vec<CodeRange>,
    pub tokens: u32,
    pub lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeRange {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Parses and checks the output of `dupes`, which must report one of
/// `expected_languages`.
pub fn parse(
    bytes: &[u8],
    expected_languages: &[&str],
    analyzer: impl Into<CoreText>,
) -> Result<DupesOutput, CoreText> {
    let analyzer = analyzer.into();
    let invalid = |detail: CoreText| invalid_document(&analyzer, DOCUMENT, detail);
    let output: DupesOutput = parse_versioned(bytes, &analyzer, DOCUMENT)?;
    check_language(&output.language, expected_languages).map_err(invalid)?;
    for duplicate in &output.duplicates {
        if duplicate.others.is_empty() {
            return Err(invalid(core_text!(
                DuplicateWithoutOthers,
                "the duplicate at \"{file}\" line {line} lists no other occurrence",
                file = escaped(&duplicate.block.file),
                line = duplicate.block.start_line
            )));
        }
        for range in std::iter::once(&duplicate.block).chain(&duplicate.others) {
            if range.start_line == 0 || range.end_line < range.start_line {
                return Err(invalid(core_text!(
                    DuplicateLinesInvalid,
                    "a duplicate in \"{file}\" has lines {start} to {end}",
                    file = escaped(&range.file),
                    start = range.start_line,
                    end = range.end_line
                )));
            }
        }
    }
    Ok(output)
}

/// Translation catalogs, left out of the duplicate code search entirely:
/// every file under a folder with one of these names, at any depth
/// (`src/i18n/locales/en.ts`, `app/locale/pt_BR/LC_MESSAGES/django.po`,
/// `resources/lang/en/messages.php`). The analyzers compare the names
/// ignoring ASCII case (`dupes.schema.json`, `$defs/changed`, `exclude`).
/// Catalogs of one app repeat the same keys in every language, so as copies
/// they are noise, and code that looks like a catalog is not worth flagging
/// as copied from one. This list and `TRANSLATION_EXTENSIONS` are the whole
/// rule: change it here and every analyzer follows.
pub const TRANSLATION_DIRS: &[&str] = &[
    "i18n",
    "l10n",
    "lang",
    "locale",
    "locales",
    "translations",
];

/// Translation catalogs by file name, wherever they are: gettext's.
pub const TRANSLATION_EXTENSIONS: &[&str] = &[".po", ".pot"];

/// Bumped whenever what a search finds for the same code changes (which
/// files it leaves out, how it tells tests apart), so a kept search made by
/// an older rule is never served.
pub const SEARCH_RULES: u32 = 2;

#[derive(Serialize)]
struct ChangedDocument<'a> {
    protocol: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    exclude: Option<Exclude>,
    files: Vec<ChangedFile<'a>>,
}

#[derive(Serialize)]
struct Exclude {
    dirs: &'static [&'static str],
    extensions: &'static [&'static str],
}

#[derive(Serialize)]
struct ChangedFile<'a> {
    path: &'a str,
    added: Vec<(u32, u32)>,
}

fn changed_bytes(lines: &ChangedLines, exclude: Option<Exclude>) -> Vec<u8> {
    let document = ChangedDocument {
        protocol: 1,
        exclude,
        files: lines
            .added_ranges()
            .into_iter()
            .map(|(path, added)| ChangedFile { path, added })
            .collect(),
    };
    serde_json::to_vec(&document).unwrap_or_default()
}

/// The `--changed` document of `lines` (`dupes.schema.json`,
/// `$defs/changed`): the lines the review adds, per file, as `mutate`
/// reads it.
pub fn changed_document(lines: &ChangedLines) -> Vec<u8> {
    changed_bytes(lines, None)
}

/// The `--changed` document of a duplicate code search: the lines the
/// review adds, and the translation catalogs to leave out.
pub fn dupes_document(lines: &ChangedLines) -> Vec<u8> {
    changed_bytes(
        lines,
        Some(Exclude {
            dirs: TRANSLATION_DIRS,
            extensions: TRANSLATION_EXTENSIONS,
        }),
    )
}

/// The `--changed` file of one search (`dupes_document`), deleted when
/// dropped.
pub struct ChangedInput {
    path: PathBuf,
}

impl ChangedInput {
    /// Writes the search's document of `lines` to a new file in `directory`.
    pub fn write(directory: &Path, lines: &ChangedLines) -> Result<Self, CoreText> {
        let path = directory.join(format!("changed-{}.json", Uuid::new_v4()));
        let failed = |error: io::Error| {
            core_text!(
                DuplicateInputFailed,
                "The duplicate code search could not be started: {path}: {detail}",
                path = path.display(),
                detail = error
            )
        };
        fs::create_dir_all(directory).map_err(failed)?;
        fs::write(&path, dupes_document(lines)).map_err(failed)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ChangedInput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// One search, as the Quality report keeps it (and its cache stores it):
/// the duplicates with their code, production and test code apart, and the
/// analyzer's own diagnostics. A kept search from before tests were told
/// apart has no `testDuplicates`, so it does not read back and is searched
/// again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Found {
    pub min_tokens: u32,
    pub min_lines: u32,
    pub duplicates: Vec<Duplicate>,
    pub test_duplicates: Vec<Duplicate>,
    pub diagnostics: Vec<String>,
}

/// `output` with the code of every occurrence read from `tree`, the tree
/// the search ran on. A file that cannot be read leaves its code empty.
pub fn with_code(output: DupesOutput, tree: &Path) -> Found {
    let mut files: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut block = |range: CodeRange| {
        let lines = files
            .entry(range.file.clone())
            .or_insert_with(|| read_lines(tree, &range.file));
        let code = lines.as_ref().map_or_else(Vec::new, |lines| {
            let skip = (range.start_line as usize).saturating_sub(1);
            let take = ((range.end_line - range.start_line) as usize + 1).min(MAX_CODE_LINES);
            lines.iter().skip(skip).take(take).cloned().collect()
        });
        DuplicateBlock {
            file: range.file,
            start_line: range.start_line,
            end_line: range.end_line,
            code,
        }
    };
    let mut duplicates = Vec::new();
    let mut test_duplicates = Vec::new();
    for duplicate in output.duplicates {
        let found = Duplicate {
            kind: duplicate.kind,
            block: block(duplicate.block),
            others: duplicate.others.into_iter().map(&mut block).collect(),
            tokens: duplicate.tokens,
            lines: duplicate.lines,
            function: None,
        };
        match duplicate.scope {
            DuplicateScope::Production => duplicates.push(found),
            DuplicateScope::Test => test_duplicates.push(found),
        }
    }
    sort(&mut duplicates);
    sort(&mut test_duplicates);
    Found {
        min_tokens: output.min_tokens,
        min_lines: output.min_lines,
        duplicates,
        test_duplicates,
        diagnostics: output.diagnostics,
    }
}

/// By the block's file, then its first line.
pub fn sort(duplicates: &mut [Duplicate]) {
    duplicates.sort_by(|left, right| {
        (&left.block.file, left.block.start_line).cmp(&(&right.block.file, right.block.start_line))
    });
}

/// The lines of `file` in `tree`, or `None` when it cannot be read or does
/// not name a file inside the tree.
pub fn read_lines(tree: &Path, file: &str) -> Option<Vec<String>> {
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

/// Places every duplicate on the innermost of `functions` holding its
/// block's first line, else on the innermost one its block overlaps.
pub fn locate(duplicates: &mut [Duplicate], functions: &[QualityFunction]) {
    for duplicate in duplicates {
        let block = &duplicate.block;
        let innermost = |holds: &dyn Fn(&QualityFunction) -> bool| {
            functions
                .iter()
                .filter(|function| function.file == block.file && holds(function))
                .min_by_key(|function| function.end_line - function.start_line)
        };
        let found = innermost(&|function| {
            (function.start_line..=function.end_line).contains(&block.start_line)
        })
        .or_else(|| {
            innermost(&|function| {
                function.start_line <= block.end_line && block.start_line <= function.end_line
            })
        });
        duplicate.function = found.map(|function| FunctionRef {
            id: function.id.clone(),
            qualified_name: function.qualified_name.clone(),
        });
    }
}

#[cfg(test)]
pub mod fixtures {
    //! Duplicates for tests.

    use super::*;

    pub fn block(file: &str, start_line: u32, end_line: u32) -> DuplicateBlock {
        DuplicateBlock {
            file: file.to_string(),
            start_line,
            end_line,
            code: Vec::new(),
        }
    }

    pub fn duplicate(kind: DuplicateKind, block: DuplicateBlock, others: Vec<DuplicateBlock>) -> Duplicate {
        let lines = block.end_line - block.start_line + 1;
        Duplicate {
            kind,
            block,
            others,
            tokens: 60,
            lines,
            function: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChangedState;

    fn document(duplicates: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1, "language": "go",
            "runner": { "name": "difflens-analyzer-go", "version": "0.0.1" },
            "minTokens": 50, "minLines": 6,
            "duplicates": duplicates,
            "diagnostics": ["one file skipped"]
        }))
        .unwrap()
    }

    fn range(file: &str, start: u32, end: u32) -> serde_json::Value {
        serde_json::json!({ "file": file, "startLine": start, "endLine": end })
    }

    #[test]
    fn a_report_is_read_and_checked() {
        let bytes = document(serde_json::json!([
            { "kind": "copies_existing", "scope": "production", "block": range("a.go", 3, 12),
              "others": [range("pkg/money.go", 40, 49)], "tokens": 81, "lines": 10 },
            { "kind": "repeated_in_change", "scope": "test", "block": range("b_test.go", 1, 6),
              "others": [range("b_test.go", 9, 14), range("c_test.go", 2, 7)], "tokens": 55, "lines": 6 }
        ]));
        let output = parse(&bytes, &["go"], "The Go analyzer").unwrap();
        assert_eq!(output.duplicates.len(), 2);
        assert_eq!(output.duplicates[0].kind, DuplicateKind::CopiesExisting);
        assert_eq!(output.duplicates[0].scope, DuplicateScope::Production);
        assert_eq!(output.duplicates[1].scope, DuplicateScope::Test);
        assert_eq!(output.duplicates[1].others[1].file, "c_test.go");
        assert_eq!((output.min_tokens, output.min_lines), (50, 6));

        let wrong = parse(&bytes, &["python"], "The Python analyzer").unwrap_err();
        assert!(wrong.contains("instead of"), "{wrong}");

        let backwards = document(serde_json::json!([
            { "kind": "copies_existing", "scope": "production", "block": range("a.go", 9, 3),
              "others": [range("b.go", 1, 6)], "tokens": 60, "lines": 6 }
        ]));
        let error = parse(&backwards, &["go"], "The Go analyzer").unwrap_err();
        assert!(error.contains("has lines 9 to 3"), "{error}");
        assert!(error.contains("invalid duplicate code report"), "{error}");

        let alone = document(serde_json::json!([
            { "kind": "repeated_in_change", "scope": "production", "block": range("a.go", 1, 6),
              "others": [], "tokens": 60, "lines": 6 }
        ]));
        let error = parse(&alone, &["go"], "The Go analyzer").unwrap_err();
        assert!(error.contains("no other occurrence"), "{error}");

        let unknown = document(serde_json::json!([
            { "kind": "semantic", "scope": "production", "block": range("a.go", 1, 6),
              "others": [range("b.go", 1, 6)], "tokens": 60, "lines": 6 }
        ]));
        assert!(parse(&unknown, &["go"], "The Go analyzer").is_err());
    }

    #[test]
    fn a_report_that_does_not_tell_tests_apart_is_rejected() {
        // An analyzer from before scopes would put test copies among the
        // production ones; it must not be read as if it had none.
        let unscoped = document(serde_json::json!([
            { "kind": "copies_existing", "block": range("a.go", 3, 12), "others": [range("b.go", 1, 10)],
              "tokens": 81, "lines": 10 }
        ]));
        assert!(parse(&unscoped, &["go"], "The Go analyzer").is_err());
        let unknown = document(serde_json::json!([
            { "kind": "copies_existing", "scope": "fixtures", "block": range("a.go", 3, 12),
              "others": [range("b.go", 1, 10)], "tokens": 81, "lines": 10 }
        ]));
        assert!(parse(&unknown, &["go"], "The Go analyzer").is_err());
    }

    #[test]
    fn production_and_test_duplicates_are_kept_apart() {
        let tree = tempfile::TempDir::new().unwrap();
        let bytes = document(serde_json::json!([
            { "kind": "repeated_in_change", "scope": "test", "block": range("z_test.go", 1, 6),
              "others": [range("y_test.go", 1, 6)], "tokens": 60, "lines": 6 },
            { "kind": "copies_existing", "scope": "production", "block": range("b.go", 1, 6),
              "others": [range("a.go", 1, 6)], "tokens": 60, "lines": 6 },
            { "kind": "repeated_in_change", "scope": "test", "block": range("a_test.go", 1, 6),
              "others": [range("b_test.go", 1, 6)], "tokens": 60, "lines": 6 }
        ]));
        let found = with_code(parse(&bytes, &["go"], "x").unwrap(), tree.path());
        let files = |duplicates: &[Duplicate]| -> Vec<String> {
            duplicates.iter().map(|duplicate| duplicate.block.file.clone()).collect()
        };
        assert_eq!(files(&found.duplicates), vec!["b.go"]);
        assert_eq!(files(&found.test_duplicates), vec!["a_test.go", "z_test.go"], "sorted too");
    }

    #[test]
    fn the_search_document_names_the_translation_catalogs_to_leave_out() {
        let lines = ChangedLines::default().with_added("src/i18n/locales/en.ts", 1, 3);
        let document: serde_json::Value = serde_json::from_slice(&dupes_document(&lines)).unwrap();
        assert_eq!(
            document["exclude"],
            serde_json::json!({
                "dirs": ["i18n", "l10n", "lang", "locale", "locales", "translations"],
                "extensions": [".po", ".pot"]
            })
        );
        assert_eq!(document["files"][0]["path"], "src/i18n/locales/en.ts", "the analyzer leaves it out");
        let mutation: serde_json::Value = serde_json::from_slice(&changed_document(&lines)).unwrap();
        assert!(mutation.get("exclude").is_none(), "mutate needs every file");
    }

    #[test]
    fn a_search_kept_before_tests_were_told_apart_does_not_read_back() {
        let old = serde_json::json!({
            "minTokens": 50, "minLines": 6,
            "duplicates": [], "diagnostics": []
        });
        assert!(serde_json::from_value::<Found>(old).is_err());
    }

    #[test]
    fn the_changed_document_lists_added_ranges_per_file() {
        let lines = ChangedLines::default()
            .with_added("b.go", 4, 9)
            .with_added("a.go", 1, 2)
            .with_added("b.go", 1, 3)
            .with_removed("c.go", 1, 1);
        let document: serde_json::Value = serde_json::from_slice(&changed_document(&lines)).unwrap();
        assert_eq!(
            document,
            serde_json::json!({
                "protocol": 1,
                "files": [
                    { "path": "a.go", "added": [[1, 2]] },
                    { "path": "b.go", "added": [[1, 9]] }
                ]
            })
        );
    }

    #[test]
    fn the_changed_input_is_deleted_when_dropped() {
        let dir = tempfile::TempDir::new().unwrap();
        let folder = dir.path().join("scratch");
        let input = ChangedInput::write(&folder, &ChangedLines::default().with_added("a.go", 1, 1)).unwrap();
        let path = input.path().to_path_buf();
        assert!(fs::read_to_string(&path).unwrap().contains("\"a.go\""));
        drop(input);
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    }

    #[test]
    fn every_occurrence_gets_its_code_from_the_tree() {
        let tree = tempfile::TempDir::new().unwrap();
        let text: String = (1..=200).map(|n| format!("line {n}\n")).collect();
        fs::write(tree.path().join("a.go"), &text).unwrap();
        fs::create_dir(tree.path().join("pkg")).unwrap();
        fs::write(tree.path().join("pkg").join("money.go"), "one\r\ntwo\r\nthree\r\n").unwrap();
        let bytes = document(serde_json::json!([
            { "kind": "copies_existing", "scope": "production", "block": range("a.go", 101, 200),
              "others": [range("pkg/money.go", 2, 3)], "tokens": 81, "lines": 100 },
            { "kind": "repeated_in_change", "scope": "production", "block": range("a.go", 3, 4),
              "others": [range("../outside.go", 1, 2), range("gone.go", 1, 2)], "tokens": 60, "lines": 2 }
        ]));
        let found = with_code(parse(&bytes, &["go"], "x").unwrap(), tree.path());

        let first = &found.duplicates[0];
        assert_eq!((first.block.start_line, first.block.code[..2].to_vec()), (3, vec!["line 3".to_string(), "line 4".to_string()]));
        assert!(first.others.iter().all(|other| other.code.is_empty()), "outside the tree, or missing");
        let copy = &found.duplicates[1];
        assert_eq!(copy.block.code.len(), MAX_CODE_LINES, "a long block is cut");
        assert_eq!(copy.block.code[0], "line 101");
        assert_eq!(copy.others[0].code, vec!["two", "three"], "line ends left out");
        assert_eq!(found.diagnostics, vec!["one file skipped"]);
        assert_eq!((found.min_tokens, found.min_lines), (50, 6));
    }

    fn function(id: &str, file: &str, start_line: u32, end_line: u32) -> QualityFunction {
        QualityFunction {
            id: id.to_string(),
            qualified_name: format!("p.{id}"),
            name: id.to_string(),
            container: "p".to_string(),
            file: file.to_string(),
            start_line,
            end_line,
            state: ChangedState::New,
            complexity: Some(1),
            previous_complexity: None,
            changed_lines: None,
            crap: None,
            untested: false,
            code: Vec::new(),
        }
    }

    #[test]
    fn a_duplicate_is_placed_on_the_function_holding_its_block() {
        use fixtures::{block, duplicate};
        let functions = vec![
            function("Outer", "a.go", 1, 50),
            function("Inner", "a.go", 10, 20),
            function("Late", "b.go", 30, 40),
        ];
        let mut duplicates = vec![
            duplicate(DuplicateKind::CopiesExisting, block("a.go", 12, 18), vec![block("c.go", 1, 7)]),
            duplicate(DuplicateKind::CopiesExisting, block("a.go", 30, 36), vec![block("c.go", 1, 7)]),
            // Starts before any function, runs into one.
            duplicate(DuplicateKind::RepeatedInChange, block("b.go", 25, 33), vec![block("b.go", 50, 58)]),
            duplicate(DuplicateKind::RepeatedInChange, block("d.go", 1, 9), vec![block("b.go", 50, 58)]),
        ];
        locate(&mut duplicates, &functions);
        let placed: Vec<_> = duplicates
            .iter()
            .map(|duplicate| duplicate.function.as_ref().map(|function| function.id.as_str()))
            .collect();
        assert_eq!(placed, vec![Some("Inner"), Some("Outer"), Some("Late"), None]);
    }

    #[test]
    fn found_duplicates_read_back_as_they_were_kept() {
        let found = Found {
            min_tokens: 50,
            min_lines: 6,
            duplicates: vec![fixtures::duplicate(
                DuplicateKind::RepeatedInChange,
                fixtures::block("a.go", 1, 6),
                vec![fixtures::block("a.go", 10, 15)],
            )],
            test_duplicates: vec![fixtures::duplicate(
                DuplicateKind::RepeatedInChange,
                fixtures::block("a_test.go", 1, 6),
                vec![fixtures::block("b_test.go", 10, 15)],
            )],
            diagnostics: Vec::new(),
        };
        let bytes = serde_json::to_vec(&found).unwrap();
        assert_eq!(serde_json::from_slice::<Found>(&bytes).unwrap(), found);
    }
}
