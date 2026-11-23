//! The findings a model answers with: the schema it is given, reading its
//! answer (its new findings, and what it says of each earlier finding it was
//! asked to re-check), checking each finding's location against the code it
//! was shown, and telling two findings that say the same thing apart from
//! two that do not.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::model::{FindingCategory, LocationProblem, Severity};

/// Names the schema for the providers that want a name.
pub const SCHEMA_NAME: &str = "review_findings";

/// The answer's schema, in the strict subset every provider accepts: every
/// object closed and every property required.
pub fn schema() -> Value {
    let text = json!({ "type": "string" });
    let texts = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": text,
                        "severity": { "type": "string", "enum": ["high", "medium", "low"] },
                        "category": {
                            "type": "string",
                            "enum": [
                                "correctness",
                                "concurrency",
                                "security",
                                "data_loss",
                                "error_handling",
                                "tests",
                                "observability"
                            ]
                        },
                        "path": text,
                        "line": { "type": "integer" },
                        "end_line": { "type": "integer" },
                        "explanation": text,
                        "suggested_direction": text,
                        "evidence": texts,
                        "related": texts
                    },
                    "required": [
                        "title",
                        "severity",
                        "category",
                        "path",
                        "line",
                        "end_line",
                        "explanation",
                        "suggested_direction",
                        "evidence",
                        "related"
                    ],
                    "additionalProperties": false
                }
            },
            "earlier": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "ref": text,
                        "status": { "type": "string", "enum": ["still_present", "fixed"] },
                        "reason": text,
                        "line": { "type": "integer" },
                        "end_line": { "type": "integer" }
                    },
                    "required": ["ref", "status", "reason", "line", "end_line"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["findings", "earlier"],
        "additionalProperties": false
    })
}

/// What the model said of an earlier finding it was asked to re-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecheck {
    /// The reference the prompt gave the finding.
    pub reference: String,
    pub status: RecheckStatus,
    /// One sentence on why.
    pub reason: String,
    /// Where it is now, for one still present; 0 when not given.
    pub line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecheckStatus {
    StillPresent,
    Fixed,
}

/// What the answer says of the earlier findings. One without a reference
/// or with an unknown status is left out, so the finding stays as it was;
/// a reference said twice counts the first time only.
pub fn parse_rechecks(answer: &Value) -> Vec<RawRecheck> {
    let mut seen = HashSet::new();
    answer["earlier"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let reference = item["ref"].as_str().map(str::trim).unwrap_or("").to_string();
            if reference.is_empty() {
                return None;
            }
            let status = serde_json::from_value(item["status"].clone()).ok()?;
            let line = number(item, "line").unwrap_or(0);
            Some(RawRecheck {
                reference,
                status,
                reason: item["reason"].as_str().map(str::trim).unwrap_or("").to_string(),
                line,
                end_line: number(item, "end_line").unwrap_or(line).max(line),
            })
        })
        .filter(|recheck| seen.insert(recheck.reference.clone()))
        .collect()
}

/// A non-negative whole number field of `item`, as a model may write it.
fn number(item: &Value, name: &str) -> Option<u32> {
    item[name]
        .as_u64()
        .or_else(|| item[name].as_f64().filter(|n| *n >= 0.0).map(|n| n as u64))
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// A finding as the model wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFinding {
    pub title: String,
    pub severity: Severity,
    pub category: FindingCategory,
    pub path: String,
    pub line: u32,
    pub end_line: u32,
    pub explanation: String,
    pub suggested_direction: String,
    pub evidence: Vec<String>,
    pub related: Vec<String>,
}

/// The findings of an answer. One that lacks a title or an explanation,
/// or names an unknown severity or category, is left out rather than
/// failing the whole answer.
pub fn parse(answer: &Value) -> Vec<RawFinding> {
    answer["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_one)
        .collect()
}

fn parse_one(item: &Value) -> Option<RawFinding> {
    let text = |name: &str| item[name].as_str().map(str::trim).unwrap_or("").to_string();
    let texts = |name: &str| -> Vec<String> {
        item[name]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect()
    };
    let number = |name: &str| number(item, name);
    let title = text("title");
    let explanation = text("explanation");
    if title.is_empty() || explanation.is_empty() {
        return None;
    }
    let severity = serde_json::from_value(item["severity"].clone()).ok()?;
    let category = serde_json::from_value(item["category"].clone()).ok()?;
    let line = number("line").unwrap_or(0);
    let end_line = number("end_line").unwrap_or(line).max(line);
    Some(RawFinding {
        title,
        severity,
        category,
        path: clean_path(&text("path")),
        line,
        end_line,
        explanation,
        suggested_direction: text("suggested_direction"),
        evidence: texts("evidence"),
        related: texts("related"),
    })
}

/// A path as a model may write it, `./a/b.go` or `/a/b.go`, as the review
/// names it.
fn clean_path(path: &str) -> String {
    let mut path = path.trim();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    path.trim_start_matches('/').to_string()
}

/// Where a finding points, checked against the code its call was shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// The cited lines, `end_line` kept inside the file, and their text.
    Located {
        line: u32,
        end_line: u32,
        cited_text: String,
    },
    NotLocated(LocationProblem),
}

/// Checks `finding` against `sent`, the new-side lines of the files its
/// call was shown, and `changed`, whether a path is a changed file at all.
pub fn check(
    finding: &RawFinding,
    sent: &HashMap<String, Vec<String>>,
    changed: impl Fn(&str) -> bool,
) -> Checked {
    check_place(&finding.path, finding.line, finding.end_line, sent, changed)
}

/// Checks the lines `line` to `end_line` of `path`, as a model cited them,
/// the way `check` does a finding's.
pub fn check_place(
    path: &str,
    line: u32,
    end_line: u32,
    sent: &HashMap<String, Vec<String>>,
    changed: impl Fn(&str) -> bool,
) -> Checked {
    let Some(lines) = sent.get(path) else {
        return Checked::NotLocated(if changed(path) {
            LocationProblem::NotSent
        } else {
            LocationProblem::NotChanged
        });
    };
    let count = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    if line == 0 || line > count {
        return Checked::NotLocated(LocationProblem::OutsideFile);
    }
    let end_line = end_line.clamp(line, count);
    Checked::Located {
        line,
        end_line,
        cited_text: cited_text(lines, line, end_line),
    }
}

/// Lines `line` to `end_line` (1-based, both included) of `lines`.
pub fn cited_text(lines: &[String], line: u32, end_line: u32) -> String {
    let start = (line as usize).saturating_sub(1).min(lines.len());
    let end = (end_line as usize).clamp(start, lines.len());
    lines[start..end].join("\n")
}

/// Two findings say the same thing when they point at the same file and at
/// overlapping lines with the same title (case and spacing aside).
pub fn same(left: (&str, u32, u32, &str), right: (&str, u32, u32, &str)) -> bool {
    let (left_path, left_line, left_end, left_title) = left;
    let (right_path, right_line, right_end, right_title) = right;
    left_path == right_path
        && left_line <= right_end
        && right_line <= left_end
        && normalized(left_title) == normalized(right_title)
}

fn normalized(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches('.')
        .to_lowercase()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A finding as a model would write it, about `path` at `line`.
    pub fn answer_item(title: &str, path: &str, line: u32, end_line: u32) -> Value {
        json!({
            "title": title,
            "severity": "high",
            "category": "correctness",
            "path": path,
            "line": line,
            "end_line": end_line,
            "explanation": "The gateway refund runs first and the row is saved afterwards.",
            "suggested_direction": "Save the refund as pending before calling the gateway.",
            "evidence": ["Line 52 asks the gateway to refund; line 56 stores it after that."],
            "related": ["No test makes Save fail."]
        })
    }

    #[test]
    fn the_schema_is_in_the_strict_subset() {
        fn check_object(schema: &Value) {
            match schema["type"].as_str() {
                Some("object") => {
                    assert_eq!(schema["additionalProperties"], false);
                    let properties = schema["properties"].as_object().unwrap();
                    let required: Vec<&str> = schema["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|name| name.as_str().unwrap())
                        .collect();
                    let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
                    names.sort_unstable();
                    let mut sorted = required.clone();
                    sorted.sort_unstable();
                    assert_eq!(names, sorted);
                    properties.values().for_each(check_object);
                }
                Some("array") => check_object(&schema["items"]),
                _ => {}
            }
        }
        check_object(&schema());
    }

    #[test]
    fn reads_the_findings_and_leaves_out_the_unusable_ones() {
        let mut odd = answer_item("Retrying refunds twice", "./refund/handler.go", 31, 0);
        odd["severity"] = json!("medium");
        odd["category"] = json!("data_loss");
        odd["evidence"] = json!(["  ", "Line 31 retries."]);
        let mut unknown = answer_item("Bad", "a.go", 1, 1);
        unknown["severity"] = json!("critical");
        let mut untitled = answer_item("  ", "a.go", 1, 1);
        untitled["title"] = json!("");
        let answer = json!({ "findings": [
            answer_item("Refund can be paid out and never recorded", "refund/service.go", 52, 56),
            odd,
            unknown,
            untitled,
            "not an object"
        ]});

        let findings = parse(&answer);
        assert_eq!(findings.len(), 2);
        assert_eq!(
            findings[0].title,
            "Refund can be paid out and never recorded"
        );
        assert_eq!((findings[0].line, findings[0].end_line), (52, 56));
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[1].path, "refund/handler.go");
        assert_eq!(findings[1].severity, Severity::Medium);
        assert_eq!(findings[1].category, FindingCategory::DataLoss);
        // An end before the start is the start.
        assert_eq!((findings[1].line, findings[1].end_line), (31, 31));
        assert_eq!(findings[1].evidence, ["Line 31 retries."]);
        assert!(parse(&json!({ "findings": [] })).is_empty());
        assert!(parse(&json!({})).is_empty());
    }

    #[test]
    fn reads_what_the_model_said_of_each_earlier_finding_once() {
        let answer = json!({ "findings": [], "earlier": [
            { "ref": " E1 ", "status": "fixed", "reason": "Saved first now.", "line": 0, "end_line": 0 },
            { "ref": "E2", "status": "still_present", "reason": "Still retried.", "line": 14, "end_line": 9 },
            { "ref": "E1", "status": "still_present", "reason": "Said twice.", "line": 1, "end_line": 1 },
            { "ref": "E3", "status": "gone", "reason": "Unknown status.", "line": 1, "end_line": 1 },
            { "ref": "", "status": "fixed", "reason": "No reference.", "line": 1, "end_line": 1 },
            "not an object"
        ]});
        let rechecks = parse_rechecks(&answer);
        assert_eq!(
            rechecks,
            [
                RawRecheck {
                    reference: "E1".to_string(),
                    status: RecheckStatus::Fixed,
                    reason: "Saved first now.".to_string(),
                    line: 0,
                    end_line: 0,
                },
                RawRecheck {
                    reference: "E2".to_string(),
                    status: RecheckStatus::StillPresent,
                    reason: "Still retried.".to_string(),
                    line: 14,
                    end_line: 14,
                },
            ]
        );
        // An answer from before re-checks has none.
        assert!(parse_rechecks(&json!({ "findings": [] })).is_empty());
    }

    #[test]
    fn a_location_outside_what_was_shown_is_not_taken_as_found() {
        let sent = HashMap::from([(
            "a.go".to_string(),
            vec!["one".to_string(), "two".to_string(), "three".to_string()],
        )]);
        let changed = |path: &str| path == "a.go" || path == "b.go";
        let finding = |path: &str, line, end_line| {
            let raw = parse(&json!({ "findings": [answer_item("T", path, line, end_line)] }));
            check(&raw[0], &sent, changed)
        };
        assert_eq!(
            finding("a.go", 2, 9),
            Checked::Located {
                line: 2,
                end_line: 3,
                cited_text: "two\nthree".to_string()
            }
        );
        assert_eq!(
            finding("a.go", 4, 4),
            Checked::NotLocated(LocationProblem::OutsideFile)
        );
        assert_eq!(
            finding("a.go", 0, 0),
            Checked::NotLocated(LocationProblem::OutsideFile)
        );
        assert_eq!(
            finding("b.go", 1, 1),
            Checked::NotLocated(LocationProblem::NotSent)
        );
        assert_eq!(
            finding("c.go", 1, 1),
            Checked::NotLocated(LocationProblem::NotChanged)
        );
    }

    #[test]
    fn the_same_finding_is_the_same_title_on_overlapping_lines_of_one_file() {
        let title = "Refund can be paid out and never recorded";
        let shouted = "  REFUND can be paid out   and never recorded. ";
        assert!(same(("a.go", 52, 56, title), ("a.go", 56, 60, shouted)));
        assert!(!same(("a.go", 52, 56, title), ("a.go", 57, 60, title)));
        assert!(!same(("a.go", 52, 56, title), ("b.go", 52, 56, title)));
        assert!(!same(
            ("a.go", 52, 56, title),
            ("a.go", 52, 56, "Another title")
        ));
    }
}
