// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::completion::PickedFile;

pub(crate) fn resource_attachments(
    text: &str,
    picked: impl IntoIterator<Item = PickedFile>,
    uris: &dyn ahp_wire::client::ResourceUriMap,
) -> Option<Vec<ahp_types::state::MessageAttachment>> {
    let mut attachments = Vec::new();
    for pick in picked {
        let needle = format!("@{}", pick.rel);
        let Some(at) = text.find(&needle) else {
            continue;
        };
        let position = |byte: usize| {
            let before = &text[..byte];
            let line = before.matches('\n').count() as i64;
            let character = before
                .rsplit_once('\n')
                .map(|(_, tail)| tail)
                .unwrap_or(before)
                .chars()
                .count() as i64;
            ahp_types::state::TextPosition { line, character }
        };
        attachments.push(ahp_types::state::MessageAttachment::Resource(
            ahp_types::state::MessageResourceAttachment {
                label: pick.label.clone(),
                range: Some(ahp_types::state::TextRange {
                    start: position(at),
                    end: position(at + needle.len()),
                }),
                display_kind: Some("document".to_owned()),
                meta: None,
                uri: uris.uri_of(&pick.location).into_string(),
                size_hint: None,
                content_type: None,
                nonce: None,
                selection: None,
            },
        ));
    }
    (!attachments.is_empty()).then_some(attachments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::location::ResourceLocation;

    struct TestUris;
    impl ahp_wire::client::ResourceUriMap for TestUris {
        fn uri_of(&self, location: &ResourceLocation) -> ahp_wire::client::ResourceUri {
            ahp_wire::client::ResourceUri::new(format!("file:///{}", location.path().join("/")))
        }
        fn location_of(
            &self,
            _uri: &ahp_wire::client::ResourceUri,
            _kind: editor::location::ResourceType,
            _authority: &editor::location::Authority,
        ) -> Option<ResourceLocation> {
            None
        }
    }

    fn pick(rel: &str, path: &[&str]) -> PickedFile {
        PickedFile {
            label: path.last().unwrap().to_string(),
            rel: rel.to_owned(),
            location: ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("test"),
                path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ),
        }
    }

    #[test]
    fn attachments_pair_inline_spans_and_drop_edited_picks() {
        let text = "first line\nsee @src/main.rs — ok";
        let picked = vec![
            pick("src/main.rs", &["proj", "src", "main.rs"]),
            pick("gone.rs", &["proj", "gone.rs"]),
        ];
        let attachments = resource_attachments(text, picked, &TestUris).expect("one survives");
        assert_eq!(attachments.len(), 1, "the edited-away pick dropped");
        let ahp_types::state::MessageAttachment::Resource(resource) = &attachments[0] else {
            panic!("a resource attachment");
        };
        assert_eq!(resource.label, "main.rs");
        assert_eq!(resource.uri, "file:///proj/src/main.rs");
        let range = resource.range.as_ref().expect("the inline span");
        assert_eq!((range.start.line, range.start.character), (1, 4));
        assert_eq!((range.end.line, range.end.character), (1, 16));
        assert_eq!(resource.display_kind.as_deref(), Some("document"));

        assert!(resource_attachments("plain", vec![pick("x.rs", &["x.rs"])], &TestUris).is_none());
    }
}
