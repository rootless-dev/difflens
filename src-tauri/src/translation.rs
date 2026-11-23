//! Translating what a model wrote (Insights findings, the Overview summary,
//! the explanations of functions on the Flow tab and of failed tests on the
//! Tests tab) into another language,
//! without reading the code again: only the prose is
//! sent, each piece under its id, and the answer must give back the same ids.
//! The findings, verdicts and locations stay what they were; a translation is
//! one more language version of the same text.
//!
//! Nothing here asks a model: the services build the request and read the
//! answer, and they only run after a click on Translate.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{core_text, CoreText};
use crate::insights::plan::{estimate_tokens, CALL_BUDGET_TOKENS};
use crate::language::Language;

/// What the model is told about translating; see `translation/prompt.md`.
pub const SYSTEM_PROMPT: &str = include_str!("translation/prompt.md");

pub const FINDINGS_SCHEMA_NAME: &str = "translated_findings";
pub const SUMMARY_SCHEMA_NAME: &str = "translated_overview";
pub const FUNCTIONS_SCHEMA_NAME: &str = "translated_explanations";
pub const DIAGNOSES_SCHEMA_NAME: &str = "translated_failure_explanations";

/// Text a call may carry to translate. The answer is about as long as what
/// it translates (longer in Portuguese), so this is kept well under the
/// output limit of every provider rather than at the input budget.
pub const TEXT_BUDGET_TOKENS: u64 = 6_000;

/// The prose of one finding, under its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindingProse {
    pub id: String,
    pub title: String,
    pub explanation: String,
    pub suggested_direction: String,
    pub evidence: Vec<String>,
    pub related: Vec<String>,
}

/// The prose of a criterion; its id is its position, "c1" for the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionProse {
    pub criterion: String,
    pub reasoning: String,
}

/// The prose of an Overview summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryProse {
    pub title: String,
    pub lead: String,
    /// Empty for a summary written before points existed.
    pub points: Vec<String>,
    pub attention: Vec<String>,
    pub card_summary: String,
    pub criteria: Vec<CriterionProse>,
}

/// Why an answer cannot be used, for `ModelError::InvalidOutput`.
pub type Unusable = CoreText;

/// The id a criterion goes by: "c1", "c2"…
pub fn criterion_id(index: usize) -> String {
    format!("c{}", index + 1)
}

/// The tokens one call costs besides its texts.
pub fn overhead_tokens(language: Language) -> u64 {
    estimate_tokens(SYSTEM_PROMPT) + estimate_tokens(&question(language, "[]"))
}

/// The tokens a finding's prose takes in a call.
pub fn finding_tokens(finding: &FindingProse) -> u64 {
    estimate_tokens(&serde_json::to_string(finding).unwrap_or_default())
}

/// Packs findings, in order, into calls of at most `TEXT_BUDGET_TOKENS` of
/// text each, and within the input budget of an Insights call. A finding
/// larger than that goes in a call of its own. Returns the ids of each call
/// and its estimated input tokens.
pub fn pack_findings(language: Language, findings: &[FindingProse]) -> Vec<(Vec<String>, u64)> {
    let overhead = overhead_tokens(language);
    let budget = TEXT_BUDGET_TOKENS.min(CALL_BUDGET_TOKENS.saturating_sub(overhead));
    let mut calls: Vec<(Vec<String>, u64)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut used = 0;
    for finding in findings {
        let tokens = finding_tokens(finding);
        if !current.is_empty() && used + tokens > budget {
            calls.push((std::mem::take(&mut current), overhead + used));
            used = 0;
        }
        current.push(finding.id.clone());
        used += tokens;
    }
    if !current.is_empty() {
        calls.push((current, overhead + used));
    }
    calls
}

/// The question: the target language, then the texts.
fn question(language: Language, texts: &str) -> String {
    format!(
        "Translate these texts into {}.\n\nTexts, as JSON:\n{texts}\n",
        language.english_name()
    )
}

/// The prompt that asks for `findings` in `language`.
pub fn findings_prompt(language: Language, findings: &[FindingProse]) -> String {
    let texts = serde_json::to_string_pretty(&json!({ "findings": findings })).unwrap_or_default();
    question(language, &texts)
}

/// The answer's schema: one entry per finding sent, by id, with every prose
/// field and nothing else. Closed and fully required, the strict subset every
/// provider accepts.
pub fn findings_schema(ids: &[String]) -> Value {
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
                        "id": { "type": "string", "enum": ids },
                        "title": text,
                        "explanation": text,
                        "suggested_direction": text,
                        "evidence": texts,
                        "related": texts
                    },
                    "required": [
                        "id",
                        "title",
                        "explanation",
                        "suggested_direction",
                        "evidence",
                        "related"
                    ],
                    "additionalProperties": false
                }
            }
        },
        "required": ["findings"],
        "additionalProperties": false
    })
}

fn text(value: &Value) -> String {
    value.as_str().map(str::trim).unwrap_or("").to_string()
}

fn texts(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .filter(|item| !item.is_empty())
        .collect()
}

/// The translations in `answer` of the findings `sent`. An entry for an id
/// that was not sent, a second entry for the same id, or one without a title
/// or explanation is left out: that finding keeps its original text. Fails
/// only when nothing usable came back.
pub fn parse_findings(
    answer: &Value,
    sent: &[FindingProse],
) -> Result<Vec<FindingProse>, Unusable> {
    let wanted: HashSet<&str> = sent.iter().map(|finding| finding.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut translated = Vec::new();
    for item in answer["findings"].as_array().into_iter().flatten() {
        let id = text(&item["id"]);
        if !wanted.contains(id.as_str()) || !seen.insert(id.clone()) {
            continue;
        }
        let finding = FindingProse {
            id,
            title: text(&item["title"]),
            explanation: text(&item["explanation"]),
            suggested_direction: text(&item["suggested_direction"]),
            evidence: texts(&item["evidence"]),
            related: texts(&item["related"]),
        };
        if finding.title.is_empty() || finding.explanation.is_empty() {
            continue;
        }
        translated.push(finding);
    }
    if translated.is_empty() && !sent.is_empty() {
        return Err(core_text!(
            TranslationNamesNone,
            "the translation names none of the findings it was given"
        ));
    }
    Ok(translated)
}

/// The prose of a function's explanations that is to be translated, under
/// the function's id: its short explanation (`brief`) and the three parts of
/// its detailed one. A part that has nothing to translate is empty, such as
/// the whole detailed one of a function explained only in short, or what
/// changed and why for a function of the context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionProse {
    pub id: String,
    pub brief: String,
    pub what_changed: String,
    pub why: String,
    pub purpose: String,
}

/// The tokens a function's prose takes in a call.
pub fn function_tokens(function: &FunctionProse) -> u64 {
    estimate_tokens(&serde_json::to_string(function).unwrap_or_default())
}

/// Packs explanations, in order, into calls as `pack_findings` packs
/// findings.
pub fn pack_functions(language: Language, functions: &[FunctionProse]) -> Vec<(Vec<String>, u64)> {
    let overhead = overhead_tokens(language);
    let budget = TEXT_BUDGET_TOKENS.min(CALL_BUDGET_TOKENS.saturating_sub(overhead));
    let mut calls: Vec<(Vec<String>, u64)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut used = 0;
    for function in functions {
        let tokens = function_tokens(function);
        if !current.is_empty() && used + tokens > budget {
            calls.push((std::mem::take(&mut current), overhead + used));
            used = 0;
        }
        current.push(function.id.clone());
        used += tokens;
    }
    if !current.is_empty() {
        calls.push((current, overhead + used));
    }
    calls
}

/// The prompt that asks for the explanations `functions` in `language`.
pub fn functions_prompt(language: Language, functions: &[FunctionProse]) -> String {
    let texts =
        serde_json::to_string_pretty(&json!({ "functions": functions })).unwrap_or_default();
    question(language, &texts)
}

/// The answer's schema: one entry per explanation sent, by function id.
pub fn functions_schema(ids: &[String]) -> Value {
    let text = json!({ "type": "string" });
    json!({
        "type": "object",
        "properties": {
            "functions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "enum": ids },
                        "brief": text,
                        "what_changed": text,
                        "why": text,
                        "purpose": text
                    },
                    "required": ["id", "brief", "what_changed", "why", "purpose"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["functions"],
        "additionalProperties": false
    })
}

/// The translations in `answer` of the explanations `sent`. An entry for an
/// id that was not sent, a second one for the same id, or one that leaves
/// out a part the original has, is left out: that function keeps its
/// original text. Fails only when nothing usable came back.
pub fn parse_functions(
    answer: &Value,
    sent: &[FunctionProse],
) -> Result<Vec<FunctionProse>, Unusable> {
    let by_id: HashMap<&str, &FunctionProse> = sent
        .iter()
        .map(|function| (function.id.as_str(), function))
        .collect();
    let mut seen = HashSet::new();
    let mut translated = Vec::new();
    for item in answer["functions"].as_array().into_iter().flatten() {
        let id = text(&item["id"]);
        let Some(original) = by_id.get(id.as_str()) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let function = FunctionProse {
            id,
            brief: text(&item["brief"]),
            what_changed: text(&item["what_changed"]),
            why: text(&item["why"]),
            purpose: text(&item["purpose"]),
        };
        let lacks = |translated: &str, original: &str| {
            translated.is_empty() && !original.trim().is_empty()
        };
        if lacks(&function.brief, &original.brief)
            || lacks(&function.what_changed, &original.what_changed)
            || lacks(&function.why, &original.why)
            || lacks(&function.purpose, &original.purpose)
        {
            continue;
        }
        translated.push(function);
    }
    if translated.is_empty() && !sent.is_empty() {
        return Err(core_text!(
            TranslationNamesNoFunction,
            "the translation names none of the functions it was given"
        ));
    }
    Ok(translated)
}

/// The prose of an explanation of a failed test, under its key: why it
/// failed and the fix proposed. The code of the fix is not translated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosisProse {
    pub id: String,
    pub cause: String,
    pub fix: String,
}

/// Packs explanations of failures, in order, into calls as `pack_findings`
/// packs findings.
pub fn pack_diagnoses(language: Language, diagnoses: &[DiagnosisProse]) -> Vec<(Vec<String>, u64)> {
    let overhead = overhead_tokens(language);
    let budget = TEXT_BUDGET_TOKENS.min(CALL_BUDGET_TOKENS.saturating_sub(overhead));
    let mut calls: Vec<(Vec<String>, u64)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut used = 0;
    for diagnosis in diagnoses {
        let tokens = estimate_tokens(&serde_json::to_string(diagnosis).unwrap_or_default());
        if !current.is_empty() && used + tokens > budget {
            calls.push((std::mem::take(&mut current), overhead + used));
            used = 0;
        }
        current.push(diagnosis.id.clone());
        used += tokens;
    }
    if !current.is_empty() {
        calls.push((current, overhead + used));
    }
    calls
}

/// The prompt that asks for the explanations of failures `diagnoses` in
/// `language`.
pub fn diagnoses_prompt(language: Language, diagnoses: &[DiagnosisProse]) -> String {
    let texts =
        serde_json::to_string_pretty(&json!({ "failures": diagnoses })).unwrap_or_default();
    question(language, &texts)
}

/// The answer's schema: one entry per explanation of a failure sent, by key.
pub fn diagnoses_schema(ids: &[String]) -> Value {
    let text = json!({ "type": "string" });
    json!({
        "type": "object",
        "properties": {
            "failures": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "enum": ids },
                        "cause": text,
                        "fix": text
                    },
                    "required": ["id", "cause", "fix"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["failures"],
        "additionalProperties": false
    })
}

/// The translations in `answer` of the explanations of failures `sent`. An
/// entry for an id that was not sent, a second one for the same id, or one
/// that leaves out a part the original has, is left out: that explanation
/// keeps its original text. Fails only when nothing usable came back.
pub fn parse_diagnoses(
    answer: &Value,
    sent: &[DiagnosisProse],
) -> Result<Vec<DiagnosisProse>, Unusable> {
    let by_id: HashMap<&str, &DiagnosisProse> = sent
        .iter()
        .map(|diagnosis| (diagnosis.id.as_str(), diagnosis))
        .collect();
    let mut seen = HashSet::new();
    let mut translated = Vec::new();
    for item in answer["failures"].as_array().into_iter().flatten() {
        let id = text(&item["id"]);
        let Some(original) = by_id.get(id.as_str()) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let diagnosis = DiagnosisProse {
            id,
            cause: text(&item["cause"]),
            fix: text(&item["fix"]),
        };
        let lacks =
            |translated: &str, original: &str| translated.is_empty() && !original.trim().is_empty();
        if lacks(&diagnosis.cause, &original.cause) || lacks(&diagnosis.fix, &original.fix) {
            continue;
        }
        translated.push(diagnosis);
    }
    if translated.is_empty() && !sent.is_empty() {
        return Err(core_text!(
            TranslationNamesNoDiagnosis,
            "the translation names none of the explanations of failures it was given"
        ));
    }
    Ok(translated)
}

/// The prompt that asks for `summary` in `language`.
pub fn summary_prompt(language: Language, summary: &SummaryProse) -> String {
    let criteria: Vec<Value> = summary
        .criteria
        .iter()
        .enumerate()
        .map(|(index, criterion)| {
            json!({
                "id": criterion_id(index),
                "criterion": criterion.criterion,
                "reasoning": criterion.reasoning,
            })
        })
        .collect();
    let texts = serde_json::to_string_pretty(&json!({
        "title": summary.title,
        "lead": summary.lead,
        "points": summary.points,
        "attention": summary.attention,
        "card_summary": summary.card_summary,
        "criteria": criteria,
    }))
    .unwrap_or_default();
    question(language, &texts)
}

/// The tokens a call translating `summary` costs.
pub fn summary_tokens(language: Language, summary: &SummaryProse) -> u64 {
    estimate_tokens(SYSTEM_PROMPT) + estimate_tokens(&summary_prompt(language, summary))
}

/// The answer's schema for `summary`: its texts, its lists (points and
/// attention, one translated item per item) and each criterion by id.
pub fn summary_schema(summary: &SummaryProse) -> Value {
    let text = json!({ "type": "string" });
    let texts = json!({ "type": "array", "items": { "type": "string" } });
    let ids: Vec<String> = (0..summary.criteria.len()).map(criterion_id).collect();
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
                        "id": { "type": "string", "enum": ids },
                        "criterion": text,
                        "reasoning": text
                    },
                    "required": ["id", "criterion", "reasoning"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["title", "lead", "points", "attention", "card_summary", "criteria"],
        "additionalProperties": false
    })
}

/// The translation in `answer` of `sent`. It must have a title and a lead,
/// one point and one attention item for each of the original's, a card
/// summary when the original has one, and every criterion by its id;
/// otherwise the original is kept whole rather than shown half translated.
pub fn parse_summary(answer: &Value, sent: &SummaryProse) -> Result<SummaryProse, Unusable> {
    let title = text(&answer["title"]);
    let lead = text(&answer["lead"]);
    let card_summary = text(&answer["card_summary"]);
    if title.is_empty() || lead.is_empty() {
        return Err(core_text!(
            TranslationIncomplete,
            "the translation has no title or no lead"
        ));
    }
    let points = texts(&answer["points"]);
    let attention = texts(&answer["attention"]);
    if points.len() != sent.points.len() || attention.len() != sent.attention.len() {
        return Err(core_text!(
            TranslationLacksPoints,
            "the translation does not have one point for each point of the summary"
        ));
    }
    if card_summary.is_empty() && !sent.card_summary.trim().is_empty() {
        return Err(core_text!(
            TranslationLacksCardSummary,
            "the translation has no card summary"
        ));
    }
    let by_id: HashMap<String, &Value> = answer["criteria"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| (text(&item["id"]), item))
        .collect();
    let mut criteria = Vec::with_capacity(sent.criteria.len());
    for index in 0..sent.criteria.len() {
        let lacking = || {
            core_text!(
                TranslationLacksCriterion,
                "the translation lacks criterion {criterion}",
                criterion = criterion_id(index)
            )
        };
        let item = by_id.get(&criterion_id(index)).ok_or_else(lacking)?;
        let criterion = text(&item["criterion"]);
        if criterion.is_empty() {
            return Err(lacking());
        }
        criteria.push(CriterionProse {
            criterion,
            reasoning: text(&item["reasoning"]),
        });
    }
    Ok(SummaryProse {
        title,
        lead,
        points,
        attention,
        card_summary,
        criteria,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(id: &str, size: usize) -> FindingProse {
        FindingProse {
            id: id.to_string(),
            title: "Refund can be paid out and never recorded".to_string(),
            explanation: "x".repeat(size),
            suggested_direction: "Store the refund first.".to_string(),
            evidence: vec!["Line 52 asks the gateway to refund.".to_string()],
            related: Vec::new(),
        }
    }

    fn summary() -> SummaryProse {
        SummaryProse {
            title: "Adds the refund flow".to_string(),
            lead: "Customers can get a payment back.".to_string(),
            points: vec![
                "A new `POST /refunds` endpoint takes the payment.".to_string(),
                "The gateway pays it back.".to_string(),
            ],
            attention: vec!["The diff also carries the payouts work.".to_string()],
            card_summary: "The card asks for refunds.".to_string(),
            criteria: vec![
                CriterionProse {
                    criterion: "A refund is recorded".to_string(),
                    reasoning: "Save stores it.".to_string(),
                },
                CriterionProse {
                    criterion: "A partial refund is allowed".to_string(),
                    reasoning: String::new(),
                },
            ],
        }
    }

    #[test]
    fn the_prompt_names_the_language_and_sends_only_the_prose_by_id() {
        let prompt = findings_prompt(Language::BrazilianPortuguese, &[finding("f-1", 10)]);
        assert!(prompt.starts_with("Translate these texts into Brazilian Portuguese."));
        assert!(prompt.contains("\"id\": \"f-1\""));
        assert!(prompt.contains("\"suggested_direction\": \"Store the refund first.\""));
        assert!(!prompt.contains("severity"));
        assert!(!prompt.contains("path"));
    }

    #[test]
    fn the_findings_schema_is_strict_and_keyed_by_the_ids_sent() {
        let schema = findings_schema(&["f-1".to_string(), "f-2".to_string()]);
        let item = &schema["properties"]["findings"]["items"];
        assert_eq!(item["properties"]["id"]["enum"], json!(["f-1", "f-2"]));
        assert_eq!(item["additionalProperties"], json!(false));
        assert_eq!(item["required"].as_array().unwrap().len(), 6);
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn findings_are_split_into_calls_under_the_text_budget() {
        let findings = vec![
            finding("a", 12_000),
            finding("b", 12_000),
            finding("c", 100),
            finding("d", 40_000),
        ];
        let calls = pack_findings(Language::BrazilianPortuguese, &findings);
        let ids: Vec<Vec<String>> = calls.iter().map(|(ids, _)| ids.clone()).collect();
        assert_eq!(
            ids,
            vec![
                vec!["a".to_string()],
                vec!["b".to_string(), "c".to_string()],
                vec!["d".to_string()]
            ]
        );
        let overhead = overhead_tokens(Language::BrazilianPortuguese);
        assert!(calls[1].1 > overhead && calls[1].1 <= overhead + TEXT_BUDGET_TOKENS);
        assert_eq!(pack_findings(Language::English, &findings[2..3]).len(), 1);
    }

    #[test]
    fn a_translation_keeps_only_the_findings_it_was_given_once() {
        let sent = vec![finding("f-1", 10), finding("f-2", 10)];
        let answer = json!({ "findings": [
            { "id": "f-2", "title": " O reembolso nunca é registrado ", "explanation": "Explicação.",
              "suggested_direction": "Grave antes.", "evidence": ["Linha 52.", ""], "related": [] },
            { "id": "f-2", "title": "Outra vez", "explanation": "x", "suggested_direction": "",
              "evidence": [], "related": [] },
            { "id": "f-9", "title": "Inventado", "explanation": "x", "suggested_direction": "",
              "evidence": [], "related": [] },
            { "id": "f-1", "title": "", "explanation": "x", "suggested_direction": "",
              "evidence": [], "related": [] }
        ]});
        let translated = parse_findings(&answer, &sent).unwrap();
        assert_eq!(translated.len(), 1);
        assert_eq!(translated[0].id, "f-2");
        assert_eq!(translated[0].title, "O reembolso nunca é registrado");
        assert_eq!(translated[0].evidence, vec!["Linha 52.".to_string()]);
        assert!(parse_findings(&json!({ "findings": [] }), &sent).is_err());
    }

    #[test]
    fn a_summary_is_sent_with_criteria_by_id_and_read_back_whole() {
        let prompt = summary_prompt(Language::BrazilianPortuguese, &summary());
        assert!(prompt.contains("\"id\": \"c2\""));
        assert!(prompt.contains("\"The gateway pays it back.\""), "{prompt}");
        assert!(prompt.contains("\"attention\": [\n    \"The diff also carries the payouts work.\""), "{prompt}");
        let schema = summary_schema(&summary());
        assert_eq!(
            schema["properties"]["criteria"]["items"]["properties"]["id"]["enum"],
            json!(["c1", "c2"])
        );
        assert_eq!(schema["properties"]["points"]["items"]["type"], "string");
        assert_eq!(schema["required"].as_array().unwrap().len(), 6);
        let answer = json!({
            "title": "Adiciona o fluxo de reembolso",
            "lead": "Clientes podem receber um pagamento de volta.",
            "points": [
                "Um novo endpoint `POST /refunds` recebe o pagamento.",
                "O gateway o devolve."
            ],
            "attention": ["O diff também traz o trabalho de repasses."],
            "card_summary": "O card pede reembolsos.",
            "criteria": [
                { "id": "c2", "criterion": "Um reembolso parcial é permitido", "reasoning": "" },
                { "id": "c1", "criterion": "Um reembolso é registrado", "reasoning": "Save o grava." }
            ]
        });
        let translated = parse_summary(&answer, &summary()).unwrap();
        assert_eq!(
            translated.criteria[0].criterion,
            "Um reembolso é registrado"
        );
        assert_eq!(
            translated.criteria[1].criterion,
            "Um reembolso parcial é permitido"
        );
        assert_eq!(translated.points[1], "O gateway o devolve.");
        assert_eq!(translated.attention, ["O diff também traz o trabalho de repasses."]);

        // A point left out keeps the original whole.
        let mut short = answer.clone();
        short["points"] = json!(["Um novo endpoint `POST /refunds` recebe o pagamento."]);
        assert!(parse_summary(&short, &summary()).is_err());
        let mut no_attention = answer.clone();
        no_attention["attention"] = json!([]);
        assert!(parse_summary(&no_attention, &summary()).is_err());

        // A summary written before points has none to translate.
        let mut old = summary();
        old.points.clear();
        old.attention.clear();
        let mut answer_old = answer;
        answer_old["points"] = json!([]);
        answer_old["attention"] = json!([]);
        assert!(parse_summary(&answer_old, &old).unwrap().points.is_empty());
    }

    #[test]
    fn explanations_are_sent_by_function_id_and_kept_only_when_whole() {
        let sent = vec![
            FunctionProse {
                id: "refund.Request".to_string(),
                brief: "Now pays before saving.".to_string(),
                what_changed: "Calls the gateway before saving.".to_string(),
                why: "Inferred: the card asks for refunds.".to_string(),
                purpose: "Requests a refund.".to_string(),
            },
            // Explained only in short.
            FunctionProse {
                id: "payment.Get".to_string(),
                brief: "Loads a payment.".to_string(),
                what_changed: String::new(),
                why: String::new(),
                purpose: String::new(),
            },
            // Explained only in detail, before short ones existed.
            FunctionProse {
                id: "payment.Save".to_string(),
                brief: String::new(),
                what_changed: String::new(),
                why: String::new(),
                purpose: "Stores a payment.".to_string(),
            },
        ];
        let prompt = functions_prompt(Language::BrazilianPortuguese, &sent);
        assert!(prompt.contains("\"id\": \"refund.Request\""));
        assert!(prompt.contains("\"brief\": \"Now pays before saving.\""));
        let ids: Vec<String> = sent.iter().map(|function| function.id.clone()).collect();
        let schema = functions_schema(&ids);
        let item = &schema["properties"]["functions"]["items"];
        assert_eq!(item["properties"]["id"]["enum"], json!(["refund.Request", "payment.Get", "payment.Save"]));
        assert_eq!(item["required"], json!(["id", "brief", "what_changed", "why", "purpose"]));
        let answer = json!({ "functions": [
            { "id": "refund.Request", "brief": "Agora paga antes de gravar.",
              "what_changed": "Chama o gateway antes de gravar.", "why": "", "purpose": "Pede um reembolso." },
            { "id": "payment.Get", "brief": "Carrega um pagamento.", "what_changed": "", "why": "",
              "purpose": "" },
            { "id": "payment.Save", "brief": "", "what_changed": "", "why": "",
              "purpose": "Grava um pagamento." },
            { "id": "invented", "brief": "w", "what_changed": "x", "why": "y", "purpose": "z" }
        ]});
        // The first lacks its why: only the other two are kept.
        let translated = parse_functions(&answer, &sent).unwrap();
        assert_eq!(translated.len(), 2);
        assert_eq!(translated[0].brief, "Carrega um pagamento.");
        assert_eq!(translated[1].purpose, "Grava um pagamento.");
        let no_brief = json!({ "functions": [
            { "id": "payment.Get", "brief": "", "what_changed": "", "why": "", "purpose": "x" }
        ]});
        assert!(parse_functions(&no_brief, &sent).is_err());
        assert!(parse_functions(&json!({ "functions": [] }), &sent).is_err());
        assert_eq!(pack_functions(Language::English, &sent).len(), 1);
    }

    #[test]
    fn explanations_of_failures_are_sent_by_key_without_their_code_and_kept_only_when_whole() {
        let sent = vec![
            DiagnosisProse {
                id: "app/auth::TestLogin".to_string(),
                cause: "`checkAsync` writes `lastLogin` after `SignUp` returns.".to_string(),
                fix: "Hold `s.mu` while writing it.".to_string(),
            },
            DiagnosisProse {
                id: "app/containers::".to_string(),
                cause: "`NewService` now takes a policy.".to_string(),
                fix: String::new(),
            },
        ];
        let prompt = diagnoses_prompt(Language::BrazilianPortuguese, &sent);
        assert!(prompt.starts_with("Translate these texts into Brazilian Portuguese."));
        assert!(prompt.contains("\"id\": \"app/auth::TestLogin\""));
        assert!(prompt.contains("\"fix\": \"\""));
        let ids: Vec<String> = sent.iter().map(|diagnosis| diagnosis.id.clone()).collect();
        let schema = diagnoses_schema(&ids);
        let item = &schema["properties"]["failures"]["items"];
        assert_eq!(item["properties"]["id"]["enum"], json!(ids));
        assert_eq!(item["required"], json!(["id", "cause", "fix"]));
        assert_eq!(item["additionalProperties"], json!(false));
        let answer = json!({ "failures": [
            { "id": "app/auth::TestLogin", "cause": "`checkAsync` grava `lastLogin` depois.", "fix": "" },
            { "id": "app/containers::", "cause": " `NewService` agora pede uma política. ", "fix": "" },
            { "id": "app/containers::", "cause": "De novo.", "fix": "" },
            { "id": "invented", "cause": "x", "fix": "y" }
        ]});
        // The first lacks its fix: only the second is kept, once.
        let translated = parse_diagnoses(&answer, &sent).unwrap();
        assert_eq!(translated.len(), 1);
        assert_eq!(translated[0].cause, "`NewService` agora pede uma política.");
        assert!(parse_diagnoses(&json!({ "failures": [] }), &sent).is_err());
        assert_eq!(pack_diagnoses(Language::English, &sent).len(), 1);
    }

    #[test]
    fn a_summary_missing_a_criterion_or_its_lead_is_not_used() {
        let missing = json!({
            "title": "T", "lead": "L", "points": ["P1", "P2"], "attention": ["A"],
            "card_summary": "C",
            "criteria": [{ "id": "c1", "criterion": "A", "reasoning": "" }]
        });
        assert!(parse_summary(&missing, &summary()).is_err());
        let no_lead = json!({
            "title": "T", "lead": "", "points": ["P1", "P2"], "attention": ["A"],
            "card_summary": "C", "criteria": []
        });
        assert!(parse_summary(&no_lead, &summary()).is_err());
    }
}
