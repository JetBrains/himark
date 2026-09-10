use documents::sync::Resolve;
use himark_ahp_ext_types::text::Span;
use himark_ahp_ext_types::TextOperation;
use operation::{Op, Operation, OperationBuilder};
use std::sync::Arc;

pub fn resolve_wire(operation: TextOperation) -> Resolve {
    Arc::new(move |text: &himark::Text| {
        let spans = himark_ahp_ext_types::text::spans_of(text, &operation).ok()?;
        operation_of_spans(text, &spans)
    })
}

fn operation_of_spans(text: &himark::Text, spans: &[Span]) -> Option<Operation> {
    himark_ahp_ext_types::text::check_spans(text, spans).ok()?;
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
    Some(builder.finish())
}

#[doc(hidden)]
pub fn wire_operation(text: &himark::Text, operation: &Operation) -> TextOperation {
    himark_ahp_ext_types::text::wire_of(text, &spans_of_operation(operation))
}

fn spans_of_operation(operation: &Operation) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut at = 0usize;
    for step in operation.iter() {
        match step {
            Op::Retain(len) => at += len as usize,
            Op::Delete(text) => {
                let end = at + text.len();
                match spans.last_mut() {
                    Some(last) if last.end == at => last.end = end,
                    _ => spans.push(Span {
                        start: at,
                        end,
                        text: String::new(),
                    }),
                }
                at = end;
            }
            Op::Insert(text) => match spans.last_mut() {
                Some(last) if last.end == at => last.text.push_str(&text),
                _ => spans.push(Span {
                    start: at,
                    end: at,
                    text,
                }),
            },
        }
    }
    spans
}

#[cfg(test)]
mod tests;
