use std::ops::Range;

use himark::{Assist, AssistKind};
use operation::OperationBuilder;
use text::Text;
use tree_sitter::{Node, Tree};

const LIST_INDENT: &str = "  ";

pub(crate) fn assist(
    text: &Text,
    tree: &Tree,
    base: u32,
    kind: AssistKind,
    location: &Range<u32>,
) -> Option<Assist> {
    if matches!(kind, AssistKind::Typed(_)) {
        return None;
    }
    let local = location.start.checked_sub(base)? as usize;
    let node = tree.root_node().descendant_for_byte_range(local, local)?;
    let item = enclosing_list_item(node)?;
    let item_start = base + item.byte_range().start.min(u32::MAX as usize) as u32;
    let line = line_of(text, item_start);
    let marker = parse_marker(&slice(text, line.clone()))?;
    let content_start = line.start + marker.content_column as u32;

    match kind {
        AssistKind::Enter { soft: true } => {
            let insert = format!("\n{}", " ".repeat(marker.content_column));
            Some(replace(text, location, &insert))
        }
        AssistKind::Enter { soft: false } => {
            let line_end = line_content_end(text, &line);
            let empty = content_start >= line_end && location.start >= line_end;
            match empty {
                true => {
                    let mut assist = replace(text, &(line.start..content_start), "");
                    assist.caret = Some(line.start);
                    Some(assist)
                }

                false => {
                    let insert = format!("\n{}{}", marker.indent, marker.next());
                    Some(replace(text, location, &insert))
                }
            }
        }
        AssistKind::Indent => Some(renest(text, item, base, location, true)),
        AssistKind::Outdent => Some(renest(text, item, base, location, false)),
        AssistKind::Typed(_) => None,
    }
}

fn enclosing_list_item(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = Some(node);
    while let Some(node) = current {
        if node.kind() == "list_item" {
            return Some(node);
        }
        current = node.parent();
    }
    None
}

fn renest(text: &Text, item: Node<'_>, base: u32, location: &Range<u32>, deeper: bool) -> Assist {
    let item_range = item.byte_range();
    let start = base + item_range.start.min(u32::MAX as usize) as u32;

    let mut view = text.view();
    let byte_count = view.byte_count();
    let first_line = view.line_at((start as usize).min(byte_count)).0;
    let own_end = base + item_range.end.min(u32::MAX as usize) as u32;
    let last_line = match nested_list_start(item) {
        Some(nested) => view
            .line_at((base as usize + nested).min(byte_count))
            .0
            .saturating_sub(1)
            .max(first_line),
        None => {
            view.line_at((own_end.saturating_sub(1) as usize).min(byte_count))
                .0
        }
    };
    let line_starts: Vec<u32> = (first_line..=last_line)
        .map(|line| view.line_start_offset(text::LineNumber(line)) as u32)
        .collect();

    let mut builder = OperationBuilder::new();
    let mut cursor = 0u32;
    let mut caret_shift = 0i64;
    let step = LIST_INDENT.len() as u32;
    for line_start in line_starts {
        match deeper {
            true => {
                builder.push_retain(line_start - cursor);
                builder.push_insert(LIST_INDENT.to_owned());
                cursor = line_start;
                if line_start <= location.start {
                    caret_shift += step as i64;
                }
            }
            false => {
                let leading = slice(text, line_start..(line_start + step).min(byte_count as u32));
                let removable = leading
                    .bytes()
                    .take(step as usize)
                    .take_while(|byte| *byte == b' ')
                    .count() as u32;
                if removable == 0 {
                    continue;
                }
                builder.push_retain(line_start - cursor);
                builder.push_delete(slice(text, line_start..line_start + removable));
                cursor = line_start + removable;
                if line_start < location.start {
                    caret_shift -= removable.min(location.start.saturating_sub(line_start)) as i64;
                }
            }
        }
    }
    Assist {
        operation: builder.finish(),
        caret: Some((location.start as i64 + caret_shift).max(0) as u32),
    }
}

fn nested_list_start(item: Node<'_>) -> Option<usize> {
    for index in 0..item.child_count() {
        let child = item.child(index as u32)?;
        if child.kind() == "list" {
            return Some(child.byte_range().start);
        }
    }
    None
}

struct Marker {
    indent: String,

    kind: MarkerKind,

    task: bool,

    content_column: usize,
}

enum MarkerKind {
    Bullet(String),
    Ordered {
        number: u64,
        delimiter: char,
        trailing: String,
    },
}

impl Marker {
    fn next(&self) -> String {
        let mut next = match &self.kind {
            MarkerKind::Bullet(text) => text.clone(),
            MarkerKind::Ordered {
                number,
                delimiter,
                trailing,
            } => format!("{}{delimiter}{trailing}", number + 1),
        };
        if self.task {
            next.push_str("[ ] ");
        }
        next
    }
}

fn parse_marker(line: &str) -> Option<Marker> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let mut consumed = indent_len;

    let kind = match rest.chars().next()? {
        bullet @ ('-' | '*' | '+') => {
            let spaces = trailing_spaces(&rest[1..]);
            if spaces.is_empty() {
                return None;
            }
            consumed += 1 + spaces.len();
            MarkerKind::Bullet(format!("{bullet}{spaces}"))
        }
        digit if digit.is_ascii_digit() => {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let after = &rest[digits.len()..];
            let delimiter = match after.chars().next() {
                Some(delimiter @ ('.' | ')')) => delimiter,
                _ => return None,
            };
            let trailing = trailing_spaces(&after[1..]);
            if trailing.is_empty() {
                return None;
            }
            consumed += digits.len() + 1 + trailing.len();
            MarkerKind::Ordered {
                number: digits.parse().ok()?,
                delimiter,
                trailing,
            }
        }
        _ => return None,
    };

    let after_marker = &line[consumed..];
    let task = after_marker.len() >= 4
        && after_marker.starts_with('[')
        && matches!(after_marker.as_bytes()[1], b' ' | b'x' | b'X')
        && after_marker.as_bytes()[2] == b']'
        && after_marker.as_bytes()[3] == b' ';
    if task {
        consumed += 4;
    }

    Some(Marker {
        indent: indent.to_owned(),
        kind,
        task,
        content_column: consumed,
    })
}

fn trailing_spaces(rest: &str) -> String {
    rest.chars().take_while(|ch| *ch == ' ').collect()
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

#[cfg(test)]
mod tests {
    use himark::{AssistKind, AssistRequest, SyntaxLanguage};
    use operation::Op;

    fn applied(source: &str, assist: &himark::Assist) -> String {
        let mut out = String::new();
        let mut position = 0usize;
        for op in assist.operation.iter() {
            match op {
                Op::Retain(len) => {
                    out.push_str(&source[position..position + len as usize]);
                    position += len as usize;
                }
                Op::Insert(text) => out.push_str(&text),
                Op::Delete(text) => {
                    assert_eq!(
                        &source[position..position + text.len()],
                        text,
                        "the operation deletes what the text holds"
                    );
                    position += text.len();
                }
            }
        }
        out.push_str(&source[position..]);
        out
    }

    fn assist_on(source: &str, caret: u32, kind: AssistKind) -> Option<(String, u32)> {
        let language = crate::MarkdownLanguage;
        let text = text::Text::from_string_exact(source);
        let len = source.len() as u32;
        let tree = language.parse(&text, 0..len, None)?;
        let assist = language.assist(&AssistRequest {
            kind,
            text: &text,
            tree: tree.as_ref(),
            range: 0..len,
            location: caret..caret,
        })?;
        let caret_after = assist.caret.expect("markdown assists name their caret");
        Some((applied(source, &assist), caret_after))
    }

    #[test]
    fn enter_continues_a_bullet_item() {
        let (text, caret) =
            assist_on("- one", 5, AssistKind::Enter { soft: false }).expect("assists");
        assert_eq!(text, "- one\n- ");
        assert_eq!(caret, 8);
    }

    #[test]
    fn enter_increments_an_ordered_item() {
        let (text, caret) =
            assist_on("1. a", 4, AssistKind::Enter { soft: false }).expect("assists");
        assert_eq!(text, "1. a\n2. ");
        assert_eq!(caret, 8);
    }

    #[test]
    fn enter_continues_a_task_unchecked() {
        let (text, caret) =
            assist_on("- [x] done", 10, AssistKind::Enter { soft: false }).expect("assists");
        assert_eq!(text, "- [x] done\n- [ ] ");
        assert_eq!(caret, 17);
    }

    #[test]
    fn enter_on_an_empty_item_terminates_the_list() {
        let (text, caret) =
            assist_on("- one\n- ", 8, AssistKind::Enter { soft: false }).expect("assists");
        assert_eq!(text, "- one\n");
        assert_eq!(caret, 6);
    }

    #[test]
    fn enter_splits_an_item_at_the_caret() {
        let (text, caret) =
            assist_on("- ab", 3, AssistKind::Enter { soft: false }).expect("assists");
        assert_eq!(text, "- a\n- b");
        assert_eq!(caret, 6);
    }

    #[test]
    fn soft_enter_breaks_at_the_content_column() {
        let (text, caret) =
            assist_on("- one two", 9, AssistKind::Enter { soft: true }).expect("assists");
        assert_eq!(text, "- one two\n  ");
        assert_eq!(caret, 12);
    }

    #[test]
    fn nested_soft_enter_keeps_the_nested_column() {
        let (text, caret) =
            assist_on("- a\n  - bee", 11, AssistKind::Enter { soft: true }).expect("assists");
        assert_eq!(text, "- a\n  - bee\n    ");
        assert_eq!(caret, 16);
    }

    #[test]
    fn tab_nests_an_item() {
        let (text, caret) = assist_on("- one", 3, AssistKind::Indent).expect("assists");
        assert_eq!(text, "  - one");
        assert_eq!(caret, 5);
    }

    #[test]
    fn tab_leaves_a_nested_sublist_where_it_is() {
        let (text, caret) = assist_on("- a\n  - b", 1, AssistKind::Indent).expect("assists");
        assert_eq!(text, "  - a\n  - b");
        assert_eq!(caret, 3);
    }

    #[test]
    fn shift_tab_unnests_an_item() {
        let (text, caret) = assist_on("- a\n  - b", 8, AssistKind::Outdent).expect("assists");
        assert_eq!(text, "- a\n- b");
        assert_eq!(caret, 6);
    }

    #[test]
    fn shift_tab_at_top_level_stays() {
        let (text, caret) = assist_on("- one", 3, AssistKind::Outdent).expect("assists");
        assert_eq!(text, "- one");
        assert_eq!(caret, 3);
    }

    #[test]
    fn outside_a_list_nothing_answers() {
        assert!(assist_on("plain prose", 5, AssistKind::Enter { soft: false }).is_none());
        assert!(assist_on("plain prose", 5, AssistKind::Indent).is_none());
    }

    #[test]
    fn typed_characters_stay_plain_in_prose_and_lists() {
        assert!(assist_on("- one", 5, AssistKind::Typed('(')).is_none());
    }
}
