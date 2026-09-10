use super::*;

fn text(source: &str) -> himark::Text {
    himark::Text::from_string_exact(source)
}

fn read(value: &himark::Text) -> String {
    let end = value.byte_count() as u32;
    value.view().substring(0..end)
}

fn edit(at: usize, delete: &str, insert: &str, len: usize) -> Operation {
    let mut builder = OperationBuilder::new();
    if at > 0 {
        builder.push_retain(at as u32);
    }
    if !delete.is_empty() {
        builder.push_delete(delete.to_owned());
    }
    if !insert.is_empty() {
        builder.push_insert(insert.to_owned());
    }
    let tail = len - at - delete.len();
    if tail > 0 {
        builder.push_retain(tail as u32);
    }
    builder.finish()
}

#[test]
fn a_replacement_travels_as_one_span() {
    let spans = spans_of_operation(&edit(2, "l", "L", 5));
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].start, spans[0].end), (2, 3));
    assert_eq!(spans[0].text, "L");
}

#[test]
fn the_round_trip_lands_the_same_edit() {
    let before = text("héllo\nwörld\n");
    let ours = edit(8, "ö", "O", before.byte_count());
    let wire = wire_operation(&before, &ours);

    let landed = resolve_wire(wire)(&before).expect("it addresses this text");
    assert_eq!(read(&before.edit(&landed)), read(&before.edit(&ours)));
}

#[test]
fn an_action_that_misses_the_text_lands_as_nothing() {
    let wire = wire_operation(&text("hello"), &edit(0, "hello", "", 5));
    assert!(resolve_wire(wire)(&text("hi")).is_none());
}

#[test]
fn an_append_travels() {
    let before = text("hello");
    let wire = wire_operation(&before, &edit(5, "", "!", 5));
    let landed = resolve_wire(wire)(&before).expect("resolves");
    assert_eq!(read(&before.edit(&landed)), "hello!");
}
