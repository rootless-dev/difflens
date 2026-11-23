//! Atlassian Document Format (the JSON Jira keeps rich text in) read as
//! plain text: headings as `#` lines, lists as `-` and `1.` items, quotes as
//! `>`, code fenced, tables as `|` rows, links as their text and address.
//! Nothing is ever turned into HTML; marks other than code and links are
//! dropped. Node types this does not know are read through their content.
//! <https://developer.atlassian.com/cloud/jira/platform/apis/document/structure/>

use serde_json::Value;
use time::OffsetDateTime;

/// Deeper than this, nested content is left out rather than followed.
const MAX_DEPTH: usize = 48;

/// `document` as plain text. A string (an older API's plain text) is kept as
/// it is; anything else that is not a document reads as empty.
pub fn plain_text(document: &Value) -> String {
    if let Some(text) = document.as_str() {
        return text.trim().to_string();
    }
    loose(children(document), 0).join("\n").trim().to_string()
}

fn children(node: &Value) -> &[Value] {
    node["content"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn kind(node: &Value) -> &str {
    node["type"].as_str().unwrap_or("")
}

fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a str> {
    node["attrs"][name].as_str().filter(|text| !text.is_empty())
}

/// Blocks with a blank line between them, as paragraphs read.
fn loose(nodes: &[Value], depth: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for block in blocks(nodes, depth) {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(block);
    }
    lines
}

/// Blocks one after another, as the parts of a list item read.
fn tight(nodes: &[Value], depth: usize) -> Vec<String> {
    blocks(nodes, depth).into_iter().flatten().collect()
}

/// Each block's lines; empty blocks are left out. Inline nodes at block
/// level (a task item holds them directly) read as one paragraph.
fn blocks(nodes: &[Value], depth: usize) -> Vec<Vec<String>> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut inline_run: Vec<&Value> = Vec::new();
    let flush = |run: &mut Vec<&Value>, out: &mut Vec<Vec<String>>| {
        if !run.is_empty() {
            let text: String = run.iter().map(|node| inline(node, depth)).collect();
            out.push(text_lines(&text));
            run.clear();
        }
    };
    for node in nodes {
        if is_inline(node) {
            inline_run.push(node);
            continue;
        }
        flush(&mut inline_run, &mut out);
        out.push(block(node, depth + 1));
    }
    flush(&mut inline_run, &mut out);
    out.retain(|lines| !lines.is_empty());
    out
}

fn is_inline(node: &Value) -> bool {
    matches!(
        kind(node),
        "text"
            | "hardBreak"
            | "mention"
            | "emoji"
            | "inlineCard"
            | "status"
            | "date"
            | "placeholder"
            | "inlineExtension"
            | "mediaInline"
    )
}

/// Lines of `text`, trailing spaces and blank edges removed.
fn text_lines(text: &str) -> Vec<String> {
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();
    let first = lines.iter().position(|line| !line.trim().is_empty());
    let last = lines.iter().rposition(|line| !line.trim().is_empty());
    match (first, last) {
        (Some(first), Some(last)) => lines[first..=last].to_vec(),
        _ => Vec::new(),
    }
}

fn block(node: &Value, depth: usize) -> Vec<String> {
    let content = children(node);
    match kind(node) {
        "paragraph" => text_lines(&inline_all(content, depth)),
        "heading" => {
            let level = node["attrs"]["level"].as_u64().unwrap_or(1).clamp(1, 6) as usize;
            let text = inline_all(content, depth).replace('\n', " ");
            let text = text.trim();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![format!("{} {text}", "#".repeat(level))]
            }
        }
        "bulletList" | "decisionList" => list(content, depth, |_| "- ".to_string()),
        "orderedList" => {
            let start = node["attrs"]["order"].as_u64().unwrap_or(1);
            list(content, depth, |index| {
                format!("{}. ", start + index as u64)
            })
        }
        "taskList" => {
            let marks: Vec<&str> = content
                .iter()
                .map(|item| {
                    if attr(item, "state") == Some("DONE") {
                        "- [x] "
                    } else {
                        "- [ ] "
                    }
                })
                .collect();
            list(content, depth, |index| {
                marks.get(index).copied().unwrap_or("- [ ] ").to_string()
            })
        }
        "blockquote" => loose(content, depth)
            .into_iter()
            .map(|line| format!("> {line}").trim_end().to_string())
            .collect(),
        "codeBlock" => {
            let code: String = content
                .iter()
                .filter_map(|node| node["text"].as_str())
                .collect();
            let mut lines = vec![format!("```{}", attr(node, "language").unwrap_or(""))];
            lines.extend(code.lines().map(str::to_string));
            lines.push("```".to_string());
            lines
        }
        "rule" => vec!["---".to_string()],
        "table" => content
            .iter()
            .map(|row| {
                let cells: Vec<String> = children(row)
                    .iter()
                    .map(|cell| tight(children(cell), depth + 1).join(" "))
                    .collect();
                format!("| {} |", cells.join(" | "))
            })
            .collect(),
        "mediaSingle" | "mediaGroup" | "media" => vec!["[attachment]".to_string()],
        "expand" | "nestedExpand" => {
            let mut lines: Vec<String> = attr(node, "title")
                .map(str::to_string)
                .into_iter()
                .collect();
            lines.extend(loose(content, depth));
            lines
        }
        _ => loose(content, depth),
    }
}

/// The items of a list, each marked by `marker(index)`, its further lines
/// indented under the first.
fn list(items: &[Value], depth: usize, marker: impl Fn(usize) -> String) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let marker = marker(index);
        let indent = " ".repeat(marker.chars().count());
        let item_lines = tight(children(item), depth + 1);
        if item_lines.is_empty() {
            lines.push(marker.trim_end().to_string());
            continue;
        }
        for (position, line) in item_lines.into_iter().enumerate() {
            let prefix = if position == 0 { &marker } else { &indent };
            lines.push(format!("{prefix}{line}").trim_end().to_string());
        }
    }
    lines
}

fn inline_all(nodes: &[Value], depth: usize) -> String {
    nodes.iter().map(|node| inline(node, depth)).collect()
}

fn inline(node: &Value, depth: usize) -> String {
    if depth > MAX_DEPTH {
        return String::new();
    }
    match kind(node) {
        "text" => {
            let text = node["text"].as_str().unwrap_or("");
            let marks = node["marks"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            let mut text = text.to_string();
            if marks.iter().any(|mark| kind(mark) == "code") {
                text = format!("`{text}`");
            }
            let link = marks
                .iter()
                .find(|mark| kind(mark) == "link")
                .and_then(|mark| attr(mark, "href"));
            match link {
                Some(href) if href != text => format!("{text} ({href})"),
                _ => text,
            }
        }
        "hardBreak" => "\n".to_string(),
        "mention" => attr(node, "text").unwrap_or("@someone").to_string(),
        "emoji" => attr(node, "text")
            .or_else(|| attr(node, "shortName"))
            .unwrap_or("")
            .to_string(),
        "inlineCard" | "blockCard" => attr(node, "url").unwrap_or("").to_string(),
        "status" => attr(node, "text")
            .map(|text| format!("[{text}]"))
            .unwrap_or_default(),
        "date" => attr(node, "timestamp").map(date).unwrap_or_default(),
        "placeholder" => String::new(),
        _ => inline_all(children(node), depth + 1),
    }
}

/// "2026-11-19" for a timestamp in milliseconds; the text as it is when it is
/// not one.
fn date(timestamp: &str) -> String {
    let Ok(millis) = timestamp.parse::<i64>() else {
        return timestamp.to_string();
    };
    match OffsetDateTime::from_unix_timestamp(millis.div_euclid(1000)) {
        Ok(at) => format!("{}-{:02}-{:02}", at.year(), u8::from(at.month()), at.day()),
        Err(_) => timestamp.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn text(value: &str) -> Value {
        json!({ "type": "text", "text": value })
    }

    fn paragraph(parts: Vec<Value>) -> Value {
        json!({ "type": "paragraph", "content": parts })
    }

    fn item(blocks: Vec<Value>) -> Value {
        json!({ "type": "listItem", "content": blocks })
    }

    fn doc(blocks: Vec<Value>) -> Value {
        json!({ "type": "doc", "version": 1, "content": blocks })
    }

    #[test]
    fn keeps_headings_paragraphs_and_line_breaks() {
        let document = doc(vec![
            json!({ "type": "heading", "attrs": { "level": 2 }, "content": [text("Context")] }),
            paragraph(vec![
                text("Support wants refunds."),
                json!({ "type": "hardBreak" }),
                text("In full or in part."),
            ]),
            paragraph(vec![]),
            json!({ "type": "rule" }),
        ]);
        assert_eq!(
            plain_text(&document),
            "## Context\n\nSupport wants refunds.\nIn full or in part.\n\n---"
        );
    }

    #[test]
    fn keeps_lists_nested_numbered_and_tasks() {
        let nested = json!({
            "type": "bulletList",
            "content": [item(vec![paragraph(vec![text("never above the balance")])])]
        });
        let document = doc(vec![
            json!({ "type": "bulletList", "content": [
                item(vec![paragraph(vec![text("full refund")])]),
                item(vec![paragraph(vec![text("partial refund")]), nested]),
            ]}),
            json!({ "type": "orderedList", "attrs": { "order": 3 }, "content": [
                item(vec![paragraph(vec![text("load")])]),
                item(vec![paragraph(vec![text("refund")])]),
            ]}),
            json!({ "type": "taskList", "content": [
                { "type": "taskItem", "attrs": { "state": "DONE" }, "content": [text("notify finance")] },
                { "type": "taskItem", "attrs": { "state": "TODO" }, "content": [text("idempotency")] },
            ]}),
        ]);
        assert_eq!(
            plain_text(&document),
            "- full refund\n- partial refund\n  - never above the balance\n\n\
             3. load\n4. refund\n\n- [x] notify finance\n- [ ] idempotency"
        );
    }

    #[test]
    fn reads_inline_nodes_as_their_text() {
        let document = doc(vec![paragraph(vec![
            json!({ "type": "mention", "attrs": { "id": "1", "text": "@Dana" } }),
            text(" see "),
            json!({ "type": "text", "text": "the spec", "marks": [
                { "type": "link", "attrs": { "href": "https://example.com/spec" } }
            ]}),
            text(", call "),
            json!({ "type": "text", "text": "Refund()", "marks": [{ "type": "code" }] }),
            text(" by "),
            json!({ "type": "date", "attrs": { "timestamp": "1763510400000" } }),
            text(" "),
            json!({ "type": "status", "attrs": { "text": "BLOCKED", "color": "red" } }),
        ])]);
        assert_eq!(
            plain_text(&document),
            "@Dana see the spec (https://example.com/spec), call `Refund()` by 2025-11-19 [BLOCKED]"
        );
    }

    #[test]
    fn keeps_code_quotes_and_tables_as_text_never_html() {
        let document = doc(vec![
            json!({ "type": "codeBlock", "attrs": { "language": "go" }, "content": [
                text("if x {\n\treturn\n}")
            ]}),
            json!({ "type": "blockquote", "content": [paragraph(vec![text("<b>not bold</b>")])] }),
            json!({ "type": "table", "content": [
                { "type": "tableRow", "content": [
                    { "type": "tableHeader", "content": [paragraph(vec![text("Case")])] },
                    { "type": "tableHeader", "content": [paragraph(vec![text("Result")])] },
                ]},
                { "type": "tableRow", "content": [
                    { "type": "tableCell", "content": [paragraph(vec![text("over")])] },
                    { "type": "tableCell", "content": [paragraph(vec![text("refused")])] },
                ]},
            ]}),
        ]);
        assert_eq!(
            plain_text(&document),
            "```go\nif x {\n\treturn\n}\n```\n\n> <b>not bold</b>\n\n| Case | Result |\n| over | refused |"
        );
    }

    #[test]
    fn reads_unknown_nodes_through_their_content_and_odd_input_as_empty() {
        let document = doc(vec![json!({ "type": "panel", "content": [
            paragraph(vec![text("inside a panel")])
        ]})]);
        assert_eq!(plain_text(&document), "inside a panel");
        assert_eq!(plain_text(&Value::Null), "");
        assert_eq!(plain_text(&json!("  plain  ")), "plain");
        assert_eq!(plain_text(&json!({ "type": "doc" })), "");
    }

    #[test]
    fn stops_following_content_past_a_depth() {
        let mut node = paragraph(vec![text("deep")]);
        for _ in 0..200 {
            node = json!({ "type": "blockquote", "content": [node] });
        }
        // Does not overflow the stack; what is too deep is left out.
        assert!(!plain_text(&doc(vec![node])).contains("deep"));
    }
}
