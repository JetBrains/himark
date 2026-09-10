use himark_text::{LineNumber, Text, TextView};
use operation::OperationBuilder;

use crate::{Replacement, TextOperation, TextPosition, TextRange};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

fn content_end(view: &mut TextView, line: usize, start: usize) -> usize {
    let end = view.line_end_offset(LineNumber(line));
    if line + 1 >= view.line_count().0 {
        return end;
    }
    let mut content = end - 1;

    if content > start {
        let mut byte: Vec<u8> = Vec::with_capacity(1);
        view.byte_range_into(content - 1, content, &mut byte);
        if byte.first() == Some(&b'\r') {
            content -= 1;
        }
    }
    content
}

pub fn offset_of(view: &mut TextView, position: &TextPosition) -> Result<usize, String> {
    let line = usize::try_from(position.line).map_err(|_| "line overflow".to_owned())?;
    let character =
        usize::try_from(position.character).map_err(|_| "character overflow".to_owned())?;
    if line >= view.line_count().0 {
        return Err(format!("line {line} out of {}", view.line_count().0));
    }
    let start = view.line_start_offset(LineNumber(line));
    let content = content_end(view, line, start) - start;
    if character > content {
        return Err(format!(
            "character {character} beyond line {line}'s content ({content})"
        ));
    }
    let at = start + character;
    if !view.is_char_boundary(at) {
        return Err(format!(
            "character {character} splits a scalar on line {line}"
        ));
    }
    Ok(at)
}

pub fn position_at(view: &mut TextView, at: usize) -> TextPosition {
    let line = view.line_at(at).0;
    let start = view.line_start_offset(LineNumber(line));
    TextPosition {
        line: line as u64,
        character: (at - start) as u64,
    }
}

pub fn spans_of(text: &Text, operation: &TextOperation) -> Result<Vec<Span>, String> {
    let mut view = text.view();
    let mut spans: Vec<Span> = Vec::new();
    for replacement in &operation.replacements {
        let start = offset_of(&mut view, &replacement.range.start)?;
        let end = offset_of(&mut view, &replacement.range.end)?;
        if end < start {
            return Err("range end before start".to_owned());
        }
        if let Some(previous) = spans.last() {
            if start < previous.end {
                return Err("overlapping replacements".to_owned());
            }
            if start == end && previous.start == previous.end && start == previous.end {
                return Err("two empty ranges at one position".to_owned());
            }
        }
        spans.push(Span {
            start,
            end,
            text: replacement.text.clone(),
        });
    }
    Ok(spans)
}

pub fn wire_of(text: &Text, spans: &[Span]) -> TextOperation {
    let mut view = text.view();
    TextOperation {
        replacements: spans
            .iter()
            .map(|span| Replacement {
                range: TextRange {
                    start: position_at(&mut view, span.start),
                    end: position_at(&mut view, span.end),
                },
                text: span.text.clone(),
            })
            .collect(),
    }
}

pub fn check_spans(text: &Text, spans: &[Span]) -> Result<(), String> {
    let mut view = text.view();
    let total = text.byte_count();
    let mut cursor = 0usize;
    for span in spans {
        if span.start > span.end {
            return Err(format!("span [{}, {}) inverted", span.start, span.end));
        }
        if span.end > total {
            return Err(format!("span [{}, {}) past {total}", span.start, span.end));
        }
        if span.start < cursor {
            return Err(format!("span [{}, {}) out of order", span.start, span.end));
        }
        if !view.is_char_boundary(span.start) || !view.is_char_boundary(span.end) {
            return Err(format!(
                "span [{}, {}) splits a scalar",
                span.start, span.end
            ));
        }
        cursor = span.end;
    }
    Ok(())
}

pub fn apply_spans(text: &Text, spans: &[Span]) -> Text {
    let mut view = text.view();
    let mut builder = OperationBuilder::new();
    let mut cursor = 0usize;
    for span in spans {
        if span.start > cursor {
            builder.push_retain((span.start - cursor) as u32);
        }
        if span.end > span.start {
            builder.push_delete(view.byte_string(span.start, span.end));
        }
        if !span.text.is_empty() {
            builder.push_insert(span.text.clone());
        }
        cursor = span.end;
    }
    let total = text.byte_count();
    if total > cursor {
        builder.push_retain((total - cursor) as u32);
    }
    text.edit(&builder.finish())
}

pub fn apply(text: &Text, operation: &TextOperation) -> Result<Text, String> {
    Ok(apply_spans(text, &spans_of(text, operation)?))
}

pub fn materialize(text: &Text) -> String {
    let count = text.byte_count();
    text.view().byte_string(0, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(source: &str) -> Text {
        Text::from_string_exact(source)
    }

    #[test]
    fn a_span_splitting_a_scalar_is_refused() {
        let text = text("a → b");
        let mid = Span {
            start: 0,

            end: 3,
            text: String::new(),
        };
        let error = check_spans(&text, &[mid]).expect_err("a split scalar cannot apply");
        assert!(error.contains("splits a scalar"), "{error}");
    }

    #[test]
    fn spans_past_the_end_or_out_of_order_are_refused() {
        let text = text("hello");
        let past = Span {
            start: 3,
            end: 99,
            text: String::new(),
        };
        assert!(check_spans(&text, &[past]).is_err(), "past the end");

        let unsorted = vec![
            Span {
                start: 3,
                end: 4,
                text: String::new(),
            },
            Span {
                start: 1,
                end: 2,
                text: String::new(),
            },
        ];
        assert!(check_spans(&text, &unsorted).is_err(), "out of order");
    }

    #[test]
    fn spans_that_address_the_text_apply() {
        let text = text("a → b");
        let spans = vec![Span {
            start: 2,
            end: 5,
            text: "->".to_owned(),
        }];
        check_spans(&text, &spans).expect("valid spans");
        assert_eq!(materialize(&apply_spans(&text, &spans)), "a -> b");
    }
}
