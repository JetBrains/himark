// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use documents::diff_views::DiffSideInput;

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

use crate::app::{AppCommand, AppFx};
use editor::location::ResourceLocation;


pub struct OpenByLocationEffect {
    pub window: ::workbench::window::WindowId,
    /// The collection the open lands into — stamped at launch.
    pub documents: imba::store::Id<documents::OpenDocuments>,
    pub location: ResourceLocation,
    pub primary: bool,

    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,

    /// Focus the opened editor (a deliberate jump) or just show it.
    pub focus: bool,
}

impl std::fmt::Display for OpenByLocationEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "open by location /{}", self.location.path().join("/"))
    }
}

impl Effect for OpenByLocationEffect {
    type Result = AppCommand;
}

/// The pane road's boundary type (himark → hiahp): the changes view's
/// "Open Diff" and the diff navigator resolve the two sides on the UI
/// thread and launch this; the handler opens both sides and lands the
/// pane-open command.
pub struct OpenDiffByLocationsEffect {
    pub window: ::workbench::window::WindowId,
    /// The collection both sides register into — stamped at launch.
    pub documents: imba::store::Id<documents::OpenDocuments>,
    pub old: DiffSideInput,
    pub new: DiffSideInput,
}

impl std::fmt::Display for OpenDiffByLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("open diff by locations")
    }
}

impl Effect for OpenDiffByLocationsEffect {
    type Result = AppCommand;
}


pub fn open_by_location_effect(
    window: ::workbench::window::WindowId,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: ResourceLocation,
    primary: bool,
    focus: bool,
    target: Option<std::ops::Range<documents::text_ext::LineCol>>,
) -> crate::effects::AppEffect {
    AnyEffect::new(OpenByLocationEffect {
        window,
        documents,
        location,
        primary,
        target,
        focus,
    })
}

pub fn open_locations(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
    locations: &[ResourceLocation],
    fx: &mut AppFx<'_>,
) {
    let folders = ::workbench::window::Windows::window_ref(store, window)
        .map(|entity| ahp_session::session::folders::session_folders(store, &entity.current_session()))
        .unwrap_or_default();
    let locations: Vec<ResourceLocation> = locations
        .iter()
        .map(|location| {
            if location.authority().as_str() != "local" {
                return location.clone();
            }
            match folders
                .iter()
                .find(|folder| location.path().starts_with(folder.path()))
            {
                Some(folder) => ResourceLocation::new(
                    location.kind().clone(),
                    folder.authority().clone(),
                    location.path().to_vec(),
                ),
                None => location.clone(),
            }
        })
        .collect();
    let mut primary = true;
    let Some(documents) =
        ::workbench::window::Windows::session_state(store, window).map(|state| state.documents())
    else {
        return;
    };
    for location in &locations {
        if let Some(document) = documents::OpenDocuments::by_location(store, documents, location) {
            if primary {
                if let Some(mut window_entity) = ::workbench::window::Windows::window(store, window) {
                    fx.scope(AppCommand::Verb, |fx| {
                        window_entity.show_document(store, ui, window, document, None, false, fx);
                    });
                    ::workbench::window::Windows::put(store, window, window_entity);
                }
            }
            primary = false;
            continue;
        }
        fx.push(open_by_location_effect(
            window,
            documents,
            location.clone(),
            primary,
            false,
            None,
        ));
        primary = false;
    }
}

pub(crate) struct EditorNavigator;

impl ::workbench::navigation::WindowedNavigator for EditorNavigator {
    type Place = hikit::navigation::EditorPlace;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        place: &hikit::navigation::EditorPlace,
        fx: &mut imba::command::Fx<'_>,
    ) -> Option<::workbench::workbench_node::Panel> {
        let documents = ::workbench::window::Windows::session_state(store, window)
            .expect("navigation runs in a window with a session")
            .documents();
        let Some(id) = documents::OpenDocuments::by_location(store, documents, &place.location) else {
            // The fetch-then-open lands an AppCommand into the window —
            // the shell escape carries it over the Verb lane.
            fx.push(
                open_by_location_effect(
                    window,
                    documents,
                    place.location.clone(),
                    true,
                    false,
                    None,
                )
                .map(crate::app::shell_verb),
            );
            return None;
        };
        let mut document = documents::OpenDocuments::document(store, documents, id)?;
        let width = ::workbench::window::Windows::window_ref(store, window)
            .and_then(|entity| {
                ::workbench::workbench::panel_width(store, entity.workbench().root.focused_pane())
            })
            .unwrap_or_else(|| ::workbench::workbench::fallback_pane_editor_width(store));
        let editor = fx.scope(
            move |command| {
                imba::command::Verb::at(documents, documents::DocumentsCommand::Editor(id, command))
            },
            |fx| {
                let editor = documents::lifecycle::mount_editor(store, ui, &mut document, width, None, fx);

                if place.caret > 0 {
                    let fonts = ::editor::env::Fonts::of(store)();
                    let theme = ::editor::env::Themes::of(store);
                    document.reveal_at_instant(editor, place.caret, store, ui, &fonts, &theme, fx);
                }
                editor
            },
        );
        documents::scroll_stripes::enable_scroll_stripes(
            store,
            documents,
            id,
            &mut document,
            editor,
        );
        documents::OpenDocuments::put_document(store, documents, id, document);
        documents::OpenDocuments::touch(store, documents, id);
        let mut pane = imba::scroll::ScrollView::new(
            documents::entity_view::EditorIdView::new(documents, id, editor).with_gutter(),
        );
        pane.set_scroll_y(place.scroll_y);
        Some(::workbench::workbench_node::Panel::Editor(pane))
    }
}
