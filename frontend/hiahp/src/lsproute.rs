use std::sync::Arc;

use himark::LineCol;
use imba::effect::EffectHandler;
use serde_json::json;

use crate::fs::SeatDirectory;

pub struct CompletionRoute {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn himark::higent::ResourceUriMap>,
}

impl EffectHandler<himark::LspCompletionEffect> for CompletionRoute {
    async fn handle(&self, effect: himark::LspCompletionEffect) -> Option<himark::LspAnswer> {
        let (seat, session) = crate::fsroute::seat_of(&self.directory, &effect.location)?;
        let uri = self.uris.uri_of(&effect.location).into_string();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": effect.position.line, "character": effect.position.col },
        });
        let result = seat
            .lsp(session, "textDocument/completion".to_owned(), params)
            .await
            .ok()?;
        Some(parse_completion(&result))
    }
}

pub fn parse_completion(result: &serde_json::Value) -> himark::LspAnswer {
    const PARSE_CAP: usize = 512;
    let (items, incomplete) = match result {
        serde_json::Value::Array(items) => (items.as_slice(), false),
        serde_json::Value::Object(list) => (
            list.get("items")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            list.get("isIncomplete")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        ),
        _ => (&[] as &[serde_json::Value], false),
    };
    let parsed = items
        .iter()
        .take(PARSE_CAP)
        .filter_map(|item| {
            let label = item.get("label")?.as_str()?.to_owned();
            let text_of = |value: &serde_json::Value| value.as_str().map(str::to_owned);
            let position = |value: &serde_json::Value| {
                Some(LineCol {
                    line: value.get("line")?.as_u64()? as u32,
                    col: value.get("character")?.as_u64()? as u32,
                })
            };
            let range_of = |value: &serde_json::Value| {
                Some(position(value.get("start")?)?..position(value.get("end")?)?)
            };

            let edit = item.get("textEdit").and_then(|edit| {
                let text = text_of(edit.get("newText")?)?;
                let range = edit
                    .get("range")
                    .or_else(|| edit.get("replace"))
                    .and_then(range_of)?;
                Some((range, text))
            });
            Some(himark::LspItem {
                label,
                detail: item.get("detail").and_then(text_of),
                filter_text: item.get("filterText").and_then(text_of),
                sort_text: item.get("sortText").and_then(text_of),
                edit,
                insert_text: item.get("insertText").and_then(text_of),
            })
        })
        .collect();
    himark::LspAnswer {
        items: parsed,
        incomplete,
    }
}

pub struct HoverRoute {
    pub directory: Arc<SeatDirectory>,
    pub uris: Arc<dyn himark::higent::ResourceUriMap>,
}

impl EffectHandler<himark::hover::LspHoverEffect> for HoverRoute {
    async fn handle(&self, effect: himark::hover::LspHoverEffect) -> Option<himark::hover::HoverInfo> {
        let (seat, session) = crate::fsroute::seat_of(&self.directory, &effect.location)?;
        let uri = self.uris.uri_of(&effect.location).into_string();
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": effect.position.line, "character": effect.position.col },
        });
        let result = seat
            .lsp(session, "textDocument/hover".to_owned(), params)
            .await
            .ok()?;
        let info = parse_hover(&result);
        (!info.markdown.is_empty()).then_some(info)
    }
}

pub fn parse_hover(result: &serde_json::Value) -> himark::hover::HoverInfo {
    const LINE_CAP: usize = 80;
    let mut text = String::new();
    collect_hover(result.get("contents").unwrap_or(&serde_json::Value::Null), &mut text);
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let capped = lines.len() > LINE_CAP;
    let mut markdown = lines[..lines.len().min(LINE_CAP)].join("\n");
    if capped {
        markdown.push_str("\n…");
    }
    himark::hover::HoverInfo { markdown }
}

fn collect_hover(node: &serde_json::Value, out: &mut String) {
    match node {
        serde_json::Value::String(text) => out.push_str(text),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str("\n\n");
                }
                collect_hover(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(value) = map.get("value").and_then(serde_json::Value::as_str) {
                match map.get("language").and_then(serde_json::Value::as_str) {
                    Some(language) => {
                        out.push_str("```");
                        out.push_str(language);
                        out.push('\n');
                        out.push_str(value);
                        if !value.ends_with('\n') {
                            out.push('\n');
                        }
                        out.push_str("```");
                    }
                    None => out.push_str(value),
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod hover_tests {
    use super::parse_hover;
    use serde_json::json;

    #[test]
    fn hover_answers_parse_across_shapes() {
        let markup = json!({"contents": {"kind": "markdown", "value": "fn push(v: T)\n\nAppends."}});
        assert_eq!(parse_hover(&markup).markdown, "fn push(v: T)\n\nAppends.");

        let bare = json!({"contents": "just text"});
        assert_eq!(parse_hover(&bare).markdown, "just text");

        let array = json!({"contents": [{"language": "rust", "value": "let x: u32"}, "docs"]});
        assert_eq!(
            parse_hover(&array).markdown,
            "```rust\nlet x: u32\n```\n\ndocs"
        );

        assert!(parse_hover(&json!(null)).markdown.is_empty());
        assert!(parse_hover(&json!({"contents": ""})).markdown.is_empty());

        let long = "x\n".repeat(200);
        let capped = parse_hover(&json!({"contents": long})).markdown;
        assert_eq!(capped.lines().count(), 81);
        assert!(capped.ends_with('…'));
    }
}

#[cfg(test)]
mod completion_tests {
    use super::parse_completion;
    use serde_json::json;

    #[test]
    fn completion_answers_parse_across_shapes() {
        let array = json!([
            {"label": "push", "detail": "fn push(v)", "sortText": "0",
             "textEdit": {"range": {"start": {"line": 3, "character": 4},
                                    "end": {"line": 3, "character": 6}},
                          "newText": "push($0)"}},
            {"label": "insert", "insertText": "insert"},
            {"label": 42}
        ]);
        let parsed = parse_completion(&array);
        assert!(!parsed.incomplete);
        assert_eq!(parsed.items.len(), 2, "the malformed label drops");
        let push = &parsed.items[0];
        assert_eq!(push.label, "push");
        assert_eq!(push.detail.as_deref(), Some("fn push(v)"));
        let (range, text) = push.edit.as_ref().expect("the textEdit");
        assert_eq!((range.start.line, range.start.col), (3, 4));
        assert_eq!((range.end.line, range.end.col), (3, 6));
        assert_eq!(text, "push($0)");
        assert_eq!(parsed.items[1].insert_text.as_deref(), Some("insert"));

        let list = json!({
            "isIncomplete": true,
            "items": [{"label": "a",
                       "textEdit": {"replace": {"start": {"line": 0, "character": 0},
                                                "end": {"line": 0, "character": 1}},
                                    "insert": {"start": {"line": 0, "character": 0},
                                               "end": {"line": 0, "character": 0}},
                                    "newText": "a"}}]
        });
        let parsed = parse_completion(&list);
        assert!(parsed.incomplete);
        assert_eq!(parsed.items.len(), 1);
        assert!(
            parsed.items[0].edit.is_some(),
            "insertReplaceEdit's replace range parses"
        );

        let parsed = parse_completion(&json!(null));
        assert!(parsed.items.is_empty());
        assert!(!parsed.incomplete);
    }
}
