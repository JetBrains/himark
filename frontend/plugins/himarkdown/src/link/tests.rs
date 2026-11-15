// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn base() -> ResourceLocation {
    ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("local"),
        vec!["repo".to_owned(), "notes".to_owned(), "page.md".to_owned()],
    )
}

fn document(source: &str) -> Document {
    let store = &imba::store::Store::new();
    let ui = ::editor::test_document::test_ui();
    crate::document_from_markdown(source, store, ui, &editor::theme::embedded())
}

#[test]
fn a_markdown_link_text_is_a_link_span_with_its_destination() {
    let source = "see [the site](https://example.com \"title\") and [code](../src/main.rs#L10)\n";
    let document = document(source);
    let at = |needle: &str| source.find(needle).unwrap() as u32;

    let site = document
        .link_range_at(at("the site") + 2)
        .expect("link text is a link span");
    assert_eq!(site, at("the site")..at("the site") + 8);
    assert_eq!(
        destination_after(&document, site).as_deref(),
        Some("https://example.com")
    );

    let code = document.link_range_at(at("code](")).expect("link span");
    assert_eq!(
        destination_after(&document, code).as_deref(),
        Some("../src/main.rs#L10")
    );

    assert_eq!(document.link_range_at(at("see")), None, "plain text is not");
}

#[test]
fn a_scheme_is_external_and_a_path_resolves_against_the_file() {
    let base = base();
    let base = Some(&base);
    assert_eq!(
        link_target(base, "https://example.com/a#b"),
        Some(LinkTarget::Url("https://example.com/a#b".to_owned()))
    );
    assert_eq!(
        link_target(base, "mailto:a@example.com"),
        Some(LinkTarget::Url("mailto:a@example.com".to_owned()))
    );
    let at = editor::linecol::LineCol { line: 9, col: 0 };
    let Some(LinkTarget::Location { location, range }) = link_target(base, "../src/main.rs#L10")
    else {
        panic!("a path is a location");
    };
    assert_eq!(location.path(), ["repo", "src", "main.rs"]);
    assert_eq!(range, Some(at..at));
    let Some(LinkTarget::Location { location, range }) = link_target(base, "other.md") else {
        panic!("a path is a location");
    };
    assert_eq!(location.path(), ["repo", "notes", "other.md"]);
    assert_eq!(range, None);
    let Some(LinkTarget::Location { location, .. }) = link_target(base, "/etc/hosts") else {
        panic!("a rooted path is a location");
    };
    assert_eq!(location.path(), ["etc", "hosts"]);
    assert_eq!(link_target(base, "#heading"), None);
}

#[test]
fn a_document_with_no_location_hands_the_path_on() {
    assert_eq!(
        link_target(None, "src/main.rs#L3"),
        Some(LinkTarget::Path {
            path: "src/main.rs".to_owned(),
            line: Some(2),
        })
    );
    assert_eq!(
        link_target(None, "https://example.com"),
        Some(LinkTarget::Url("https://example.com".to_owned()))
    );
    assert_eq!(link_target(None, "#heading"), None);
}

/// A chat cell's case: an editor over a document with NO location.
/// The link is still claimed (the hover's hand) and still followed —
/// a URL as itself, a path handed on for the door to place.
#[test]
fn a_document_with_no_location_still_follows_its_links() {
    use editor::link::{LinkDoor, LinkResolvers};
    let source = "[site](https://example.com) and [code](src/main.rs#L3) and [bare]\n";
    let document = document(source);
    let at = |needle: &str| source.find(needle).unwrap() as u32 + 1;

    let store = &mut imba::store::Store::new();
    LinkResolvers::register(store, std::sync::Arc::new(MarkdownLinks));
    let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let door = std::sync::Arc::clone(&opened);
    store.put(LinkDoor(std::sync::Arc::new(move |_, target| {
        door.lock().unwrap().push(target);
    })));

    assert!(editor::link::claimed_at(store, &document, at("site"), None).is_some());
    assert!(
        editor::link::claimed_at(store, &document, at("bare"), None).is_none(),
        "a bracket pair with no destination is nobody's link"
    );

    let mut batch = imba::effect::Batch::new();
    editor::link::follow_at(store, &document, at("site"), None, &mut batch.effects());
    editor::link::follow_at(store, &document, at("code"), None, &mut batch.effects());
    editor::link::follow_at(store, &document, at("bare"), None, &mut batch.effects());
    assert_eq!(
        *opened.lock().unwrap(),
        vec![
            LinkTarget::Url("https://example.com".to_owned()),
            LinkTarget::Path {
                path: "src/main.rs".to_owned(),
                line: Some(2),
            },
        ]
    );
}
