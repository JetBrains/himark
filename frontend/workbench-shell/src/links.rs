// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell's LINK DOOR (`editor::link::LinkDoor`): where a
//! followed link opens. A URL is the host browser's; a location is a
//! `DocumentPlace` at the gesture's origin; a bare path (its
//! document had no location — a chat cell) is placed against the
//! folder the origin's session works in first.

use std::sync::Arc;

use editor::link::LinkTarget;
use editor::location::{ResourceLocation, ResourceType};
use imba::store::Store;

pub(crate) fn install(store: &mut Store) {
    store.put(editor::link::LinkDoor(Arc::new(|store, target| {
        match target {
            LinkTarget::Url(url) => {
                if !himark::host::OpenUrl::open(store, &url) {
                    eprintln!("[links] no browser door seated — link dropped: {url}");
                }
            }
            target => himark::commands::AppRequests::push(store, open_target(target)),
        }
    })));
}

/// The landing half of a followed link — the command the door
/// files for a location or a path.
pub fn open_target(target: LinkTarget) -> Arc<dyn himark::commands::HimarkCommand> {
    Arc::new(OpenLinkTarget { target })
}

struct OpenLinkTarget {
    target: LinkTarget,
}

impl himark::commands::HimarkCommand for OpenLinkTarget {
    fn id(&self) -> &'static str {
        "link.open-target"
    }
    fn name(&self) -> String {
        "Open Link Target".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(origin) = himark::commands::bound_origin(self.id(), bound) else {
            return;
        };
        let (location, target) = match &self.target {
            LinkTarget::Url(_) => return,
            LinkTarget::Location { location, range } => (location.clone(), range.clone()),
            LinkTarget::Path { path, line } => {
                let Some(folder) = origin_folder(store, origin) else {
                    eprintln!("[links] the origin's session works in no folder — dropped: {path}");
                    return;
                };
                let Some(location) = placed(&folder, path) else {
                    return;
                };
                let target = line.map(|line| {
                    let at = documents::text_ext::LineCol { line, col: 0 };
                    at..at
                });
                (location, target)
            }
        };
        fx.scope(himark::app::AppCommand::Verb, |fx| {
            himark::shell::navigate_origin(
                store,
                ui,
                origin,
                himark::places::DocumentPlace {
                    location,
                    target,
                    focus: true,
                },
                fx,
            )
        });
    }
}

/// The first folder the origin's session works in — a window's
/// session, or a mounted origin's.
fn origin_folder(store: &Store, origin: himark::bridge::ViewBridgeId) -> Option<ResourceLocation> {
    let session = match origin.is_mounted() {
        true => hiahp::sessions::session_of(store, origin)?,
        false => {
            let window = ::workbench::window::WindowId::from_raw(origin.raw());
            crate::workspace::entity_session(::workbench::window::Windows::window_ref(
                store, window,
            )?)
        }
    };
    ahp_session::session::folders::session_folders(store, &session)
        .into_iter()
        .next()
}

/// A path against a folder: `/`-rooted from the folder's root,
/// otherwise walked from the folder itself.
fn placed(folder: &ResourceLocation, path: &str) -> Option<ResourceLocation> {
    let mut segments: Vec<String> = match path.starts_with('/') {
        true => Vec::new(),
        false => folder.path().to_vec(),
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            part => segments.push(part.to_owned()),
        }
    }
    (!segments.is_empty()).then(|| {
        ResourceLocation::new(
            ResourceType::document(),
            folder.authority().clone(),
            segments,
        )
    })
}

/// A `DocumentPlace` asked FROM a diff stays in the diff when the
/// target lives there: the same file goes to the byte (lifting a fold over it),
/// another file of the canvas reveals its row. Answers whether the
/// place was taken; a target outside the diff opens an editor.
pub(crate) fn land_in_diff(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
    documents: imba::store::Id<documents::OpenDocuments>,
    place: &himark::places::DocumentPlace,
    fx: &mut imba::command::Fx<'_>,
) -> bool {
    use documents::OpenDocuments;
    let landing = ::workbench::window::Windows::window_ref(store, window).and_then(|entity| {
        match entity.workbench().root.focused_pane() {
            ::workbench::workbench_node::Panel::Plugin(view) => {
                canvas::canvases::diff_landing(store, view.as_any(), documents, &place.location)
            }
            ::workbench::workbench_node::Panel::Editor(_) => None,
        }
    });
    let Some(landing) = landing else {
        return false;
    };
    if let Some((changes, canvas, key)) = landing.reveal {
        canvas::canvases::Canvases::set_reveal(store, changes, canvas, key);
    }
    let byte = landing.pair.zip(place.target.as_ref()).and_then(|(pair, target)| {
        let right = OpenDocuments::diff_view_ref(store, documents, pair)?
            .right
            .document();
        let document = OpenDocuments::document_ref(store, documents, right)?;
        let byte = documents::text_ext::offset_at(&mut document.text().view(), target.start);
        Some((pair, byte as u32))
    });
    if let Some((pair, byte)) = byte {
        fx.scope(
            move |command| imba::command::Verb::at(documents, command),
            |fx| documents::diff_views::go_to(store, documents, ui, pair, byte, fx),
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_placed_against_the_folder() {
        let folder = ResourceLocation::new(
            ResourceType::directory(),
            editor::location::Authority::new("local"),
            vec!["home".to_owned(), "proj".to_owned()],
        );
        let at = |path: &str| placed(&folder, path).map(|location| location.path().to_vec());
        assert_eq!(
            at("src/main.rs"),
            Some(vec!["home".into(), "proj".into(), "src".into(), "main.rs".into()])
        );
        assert_eq!(at("../x.rs"), Some(vec!["home".into(), "x.rs".into()]));
        assert_eq!(at("/etc/hosts"), Some(vec!["etc".into(), "hosts".into()]));
        assert_eq!(
            placed(&folder, "a.rs").map(|location| location.kind().is_document()),
            Some(true)
        );
    }
}
