//! The summary a model answers with: the schema it is given, and reading
//! its answer, each criterion's location checked against the change.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::ai::ModelError;
use crate::model::{Criterion, CriterionVerdict, ModelProviderKind};

/// Names the schema for the providers that want a name.
pub const SCHEMA_NAME: &str = "change_overview";

/// The answer's schema, in the strict subset every provider accepts: every
/// object closed and every property required. A criterion without a place
/// has an empty path and line 0; a summary with nothing that needs attention
/// has an empty `attention`.
pub fn schema() -> Value {
    let text = json!({ "type": "string" });
    let texts = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "title": text,
            "lead": text,
            "points": texts,
            "attention": texts,
            "card_summary": text,
            "criteria": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "criterion": text,
                        "verdict": {
                            "type": "string",
                            "enum": ["met", "partly_met", "not_met", "no_evidence"]
                        },
                        "reasoning": text,
                        "path": text,
                        "line": { "type": "integer" }
                    },
                    "required": ["criterion", "verdict", "reasoning", "path", "line"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["title", "lead", "points", "attention", "card_summary", "criteria"],
        "additionalProperties": false
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub title: String,
    pub lead: String,
    pub points: Vec<String>,
    pub attention: Vec<String>,
    pub card_summary: String,
    pub criteria: Vec<Criterion>,
}

/// The non-empty items of a list of texts, trimmed.
fn texts(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Reads `answer`. Without a card, whatever the model said about one is
/// dropped. Empty points are left out; an answer without points still reads
/// (its lead is shown alone). A criterion with no text or an unknown verdict
/// is left out rather than failing the answer; a place is `located` when it
/// is a line of the new side of a changed file (`lines` counts them by path).
pub fn parse(
    answer: &Value,
    with_card: bool,
    lines: &HashMap<String, usize>,
    provider: ModelProviderKind,
) -> Result<Summary, ModelError> {
    let text = |value: &Value| value.as_str().map(str::trim).unwrap_or("").to_string();
    let title = text(&answer["title"]);
    let lead = text(&answer["lead"]);
    if title.is_empty() || lead.is_empty() {
        return Err(ModelError::InvalidOutput {
            provider,
            detail: crate::error::core_text!(
                SummaryIncomplete,
                "the summary has no title or no lead"
            ),
        });
    }
    let criteria = if with_card {
        answer["criteria"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| criterion(item, lines))
            .collect()
    } else {
        Vec::new()
    };
    Ok(Summary {
        title: title.trim_end_matches('.').to_string(),
        lead,
        points: texts(&answer["points"]),
        attention: texts(&answer["attention"]),
        card_summary: if with_card {
            text(&answer["card_summary"])
        } else {
            String::new()
        },
        criteria,
    })
}

fn criterion(item: &Value, lines: &HashMap<String, usize>) -> Option<Criterion> {
    let text = |name: &str| item[name].as_str().map(str::trim).unwrap_or("").to_string();
    let criterion = text("criterion");
    if criterion.is_empty() {
        return None;
    }
    let verdict: CriterionVerdict = serde_json::from_value(item["verdict"].clone()).ok()?;
    let path = clean_path(&text("path"));
    let line = item["line"]
        .as_u64()
        .or_else(|| {
            item["line"]
                .as_f64()
                .filter(|n| *n >= 1.0)
                .map(|n| n as u64)
        })
        .and_then(|line| u32::try_from(line).ok())
        .filter(|line| *line > 0);
    // "No evidence" rests on nothing in the change.
    let (path, line) = match (verdict, path.is_empty()) {
        (CriterionVerdict::NoEvidence, _) | (_, true) => (None, None),
        (_, false) => (Some(path), line),
    };
    let located = match (&path, line) {
        (Some(path), Some(line)) => lines
            .get(path)
            .is_some_and(|count| (line as usize) <= *count),
        _ => false,
    };
    Some(Criterion {
        criterion,
        verdict,
        reasoning: text("reasoning"),
        path,
        line,
        located,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> HashMap<String, usize> {
        HashMap::from([("refund/validate.go".to_string(), 30)])
    }

    fn item(verdict: &str, path: &str, line: u64) -> Value {
        json!({
            "criterion": "A partial refund is allowed up to the remaining balance",
            "verdict": verdict,
            "reasoning": "The balance check exists, but no test goes over the limit.",
            "path": path,
            "line": line
        })
    }

    fn answer(criteria: Vec<Value>) -> Value {
        json!({
            "title": "Adds the refund flow.",
            "lead": "Customers can get part of a settled payment back.",
            "points": [
                " A new `POST /refunds` endpoint takes the payment and the amount. ",
                "",
                "The request is checked by `refund.Validate`, then the gateway pays it back."
            ],
            "attention": [],
            "card_summary": "Support wants refunds.",
            "criteria": criteria
        })
    }

    #[test]
    fn reads_the_lead_the_points_and_what_needs_attention() {
        let summary = parse_with_card(&answer(Vec::new()));
        assert_eq!(summary.lead, "Customers can get part of a settled payment back.");
        assert_eq!(
            summary.points,
            [
                "A new `POST /refunds` endpoint takes the payment and the amount.",
                "The request is checked by `refund.Validate`, then the gateway pays it back."
            ]
        );
        assert!(summary.attention.is_empty());

        let mut with_attention = answer(Vec::new());
        with_attention["attention"] =
            json!(["The diff also carries the payouts work, which the base does not have yet."]);
        assert_eq!(parse_with_card(&with_attention).attention.len(), 1);

        // An answer without points still reads: its lead is shown alone.
        let mut bare = answer(Vec::new());
        bare.as_object_mut().unwrap().remove("points");
        assert!(parse_with_card(&bare).points.is_empty());
    }

    fn parse_with_card(value: &Value) -> Summary {
        parse(value, true, &lines(), ModelProviderKind::Anthropic).unwrap()
    }

    #[test]
    fn reads_the_summary_and_locates_each_criterion_in_the_change() {
        let summary = parse_with_card(&answer(vec![
            item("partly_met", "./refund/validate.go", 19),
            item("met", "refund/validate.go", 31),
            item("not_met", "refund/handler.go", 4),
            item("no_evidence", "refund/validate.go", 3),
            item("maybe", "refund/validate.go", 3),
            json!({ "criterion": " ", "verdict": "met", "reasoning": "", "path": "", "line": 0 }),
        ]));
        assert_eq!(summary.title, "Adds the refund flow");
        assert_eq!(summary.card_summary, "Support wants refunds.");
        let places: Vec<(CriterionVerdict, Option<&str>, Option<u32>, bool)> = summary
            .criteria
            .iter()
            .map(|c| (c.verdict, c.path.as_deref(), c.line, c.located))
            .collect();
        assert_eq!(
            places,
            [
                (
                    CriterionVerdict::PartlyMet,
                    Some("refund/validate.go"),
                    Some(19),
                    true
                ),
                // Past the end of the file.
                (
                    CriterionVerdict::Met,
                    Some("refund/validate.go"),
                    Some(31),
                    false
                ),
                // Not a changed file.
                (
                    CriterionVerdict::NotMet,
                    Some("refund/handler.go"),
                    Some(4),
                    false
                ),
                (CriterionVerdict::NoEvidence, None, None, false),
            ]
        );
    }

    #[test]
    fn without_a_card_nothing_about_one_is_kept() {
        let summary = parse(
            &answer(vec![item("met", "refund/validate.go", 1)]),
            false,
            &lines(),
            ModelProviderKind::Anthropic,
        )
        .unwrap();
        assert_eq!(summary.card_summary, "");
        assert!(summary.criteria.is_empty());
    }

    #[test]
    fn an_answer_without_a_title_is_invalid() {
        let error = parse(
            &json!({
                "title": "", "lead": "x", "points": ["y"], "attention": [],
                "card_summary": "", "criteria": []
            }),
            false,
            &lines(),
            ModelProviderKind::Anthropic,
        )
        .unwrap_err();
        assert!(matches!(error, ModelError::InvalidOutput { .. }));
    }

    #[test]
    fn the_schema_closes_every_object_and_requires_every_property() {
        fn check(schema: &Value) {
            if schema["type"] == "object" {
                assert_eq!(schema["additionalProperties"], false);
                let properties = schema["properties"].as_object().unwrap();
                let required: Vec<&str> = schema["required"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|name| name.as_str().unwrap())
                    .collect();
                for (name, property) in properties {
                    assert!(required.contains(&name.as_str()), "{name}");
                    check(property);
                }
            }
            if schema["type"] == "array" {
                check(&schema["items"]);
            }
        }
        check(&schema());
        for list in ["points", "attention"] {
            assert_eq!(schema()["properties"][list]["type"], "array");
            assert_eq!(schema()["properties"][list]["items"]["type"], "string");
        }
    }
}
