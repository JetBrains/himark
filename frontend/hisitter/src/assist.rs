use std::ops::Range;

use editor::{Assist, AssistKind, AssistRequest};
use operation::OperationBuilder;
use text::Text;

use crate::TsTree;

const INDENT_UNIT: &str = "    ";

const BALANCE_WINDOW: usize = 16 * 1024;

pub(crate) fn assist(request: &AssistRequest<'_>) -> Option<Assist> {
    match request.kind {
        AssistKind::Enter { .. } => enter(request),
        AssistKind::Indent => reindent(request, true),
        AssistKind::Outdent => reindent(request, false),
        AssistKind::Typed(ch) => auto_close(request, ch),
    }
}

fn closer_of(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        _ => return None,
    })
}

fn enter(request: &AssistRequest<'_>) -> Option<Assist> {
    let caret = request.location.start;
    let line = line_of(request.text, caret);
    let head = slice(request.text, line.start..caret.max(line.start));
    let indent: String = head
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();

    let previous = head.trim_end().chars().last();
    let bump = matches!(previous, Some('{' | '(' | '['));

    let line_end = line_content_end(request.text, &line);
    let tail = slice(request.text, request.location.end.min(line_end)..line_end);
    let double = bump
        && previous
            .and_then(closer_of)
            .is_some_and(|closer| tail.trim_start().starts_with(closer));

    let mut insert = format!("\n{indent}");
    if bump {
        insert.push_str(INDENT_UNIT);
    }
    let caret_after = request.location.start + insert.len() as u32;
    if double {
        insert.push('\n');
        insert.push_str(&indent);
    }
    let mut assist = replace(request.text, &request.location, &insert);
    assist.caret = Some(caret_after);
    Some(assist)
}

fn reindent(request: &AssistRequest<'_>, deeper: bool) -> Option<Assist> {
    let location = &request.location;
    if deeper && location.start == location.end {
        return None;
    }
    let mut view = request.text.view();
    let byte_count = view.byte_count();
    let first_line = view.line_at((location.start as usize).min(byte_count)).0;
    let last = location.end.max(location.start);
    let last_line = view
        .line_at((last.saturating_sub((last > location.start) as u32) as usize).min(byte_count))
        .0;
    let line_starts: Vec<u32> = (first_line..=last_line)
        .map(|line| view.line_start_offset(text::LineNumber(line)) as u32)
        .collect();

    let mut builder = OperationBuilder::new();
    let mut cursor = 0u32;
    let mut caret_shift = 0i64;
    let unit = INDENT_UNIT.len() as u32;
    let mut changed = false;
    for line_start in line_starts {
        match deeper {
            true => {
                builder.push_retain(line_start - cursor);
                builder.push_insert(INDENT_UNIT.to_owned());
                cursor = line_start;
                changed = true;
                if line_start <= location.start {
                    caret_shift += unit as i64;
                }
            }
            false => {
                let leading = slice(
                    request.text,
                    line_start..(line_start + unit).min(byte_count as u32),
                );
                let removable = leading.bytes().take_while(|byte| *byte == b' ').count() as u32;
                if removable == 0 {
                    continue;
                }
                builder.push_retain(line_start - cursor);
                builder.push_delete(slice(request.text, line_start..line_start + removable));
                cursor = line_start + removable;
                changed = true;
                if line_start < location.start {
                    caret_shift -= removable.min(location.start.saturating_sub(line_start)) as i64;
                }
            }
        }
    }
    if !changed && deeper {
        return None;
    }
    Some(Assist {
        operation: builder.finish(),
        caret: Some((location.start as i64 + caret_shift).max(0) as u32),
    })
}

fn auto_close(request: &AssistRequest<'_>, ch: char) -> Option<Assist> {
    let closer = closer_of(ch)?;
    if request.location.start != request.location.end {
        return None;
    }
    let window = balance_window(request);
    let counted = slice(request.text, window);
    let balanced = match ch == closer {
        true => counted.matches(ch).count() % 2 == 0,
        false => counted.matches(ch).count() == counted.matches(closer).count(),
    };
    if !balanced {
        return None;
    }
    let mut insert = String::with_capacity(8);
    insert.push(ch);
    insert.push(closer);
    let mut assist = replace(request.text, &request.location, &insert);
    assist.caret = Some(request.location.start + ch.len_utf8() as u32);
    Some(assist)
}

fn balance_window(request: &AssistRequest<'_>) -> Range<u32> {
    let local = request.location.start.saturating_sub(request.range.start) as usize;
    let tree = TsTree::of(request.tree);
    let mut best = None;
    if let Some(tree) = tree {
        let mut node = tree.root_node().descendant_for_byte_range(local, local);
        while let Some(current) = node {
            if current.byte_range().len() <= BALANCE_WINDOW {
                best = Some(current);
            } else {
                break;
            }
            node = current.parent();
        }
    }
    match best {
        Some(node) => {
            let range = node.byte_range();
            request
                .range
                .start
                .saturating_add(range.start.min(u32::MAX as usize) as u32)
                ..request
                    .range
                    .start
                    .saturating_add(range.end.min(u32::MAX as usize) as u32)
        }

        None => {
            let start = request
                .location
                .start
                .saturating_sub(BALANCE_WINDOW as u32 / 2);
            start.max(request.range.start)
                ..request
                    .location
                    .start
                    .saturating_add(BALANCE_WINDOW as u32 / 2)
                    .min(request.range.end)
        }
    }
}

fn line_of(text: &Text, offset: u32) -> Range<u32> {
    let mut view = text.view();
    let byte_count = view.byte_count();
    let line = view.line_at((offset as usize).min(byte_count));
    let start = view.line_start_offset(line) as u32;
    let end = match line.0 + 1 < view.line_count().0 {
        true => view.line_start_offset(text::LineNumber(line.0 + 1)) as u32,
        false => byte_count as u32,
    };
    start..end
}

fn line_content_end(text: &Text, line: &Range<u32>) -> u32 {
    if line.end > line.start && slice(text, line.end - 1..line.end) == "\n" {
        return line.end - 1;
    }
    line.end
}

fn slice(text: &Text, range: Range<u32>) -> String {
    let mut view = text.view();
    let byte_count = view.byte_count();
    let start = (range.start as usize).min(byte_count);
    let end = (range.end as usize).clamp(start, byte_count);
    view.byte_string(start, end)
}

fn replace(text: &Text, range: &Range<u32>, insert: &str) -> Assist {
    let mut builder = OperationBuilder::new();
    let byte_count = text.view().byte_count() as u32;
    let start = range.start.min(byte_count);
    let end = range.end.clamp(start, byte_count);
    builder.push_retain(start);
    if end > start {
        builder.push_delete(slice(text, start..end));
    }
    builder.push_insert(insert.to_owned());
    Assist {
        operation: builder.finish(),
        caret: Some(start + insert.len() as u32),
    }
}
