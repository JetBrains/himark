// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

use ::workbench::workbench::pane_width;
use ::workbench::workbench::workbench_geometry;
use crate::app::Application;
use documents::entity_view::EditorIdView;
use ::workbench::workbench_node::Panel;

struct TestUris;

impl ahp_wire::client::ResourceUriMap for TestUris {
    fn uri_of(&self, location: &editor::location::ResourceLocation) -> ahp_wire::client::ResourceUri {
        ahp_wire::client::ResourceUri::new(format!("file:///{}", location.path().join("/")))
    }

    fn location_of(
        &self,
        uri: &ahp_wire::client::ResourceUri,
        kind: editor::location::ResourceType,
        _authority: &editor::location::Authority,
    ) -> Option<editor::location::ResourceLocation> {
        let path: Vec<String> = uri
            .as_str()
            .strip_prefix("file://")?
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();

        Some(editor::location::ResourceLocation::new(
            kind,
            editor::location::Authority::new("test"),
            path,
        ))
    }
}

pub fn seed_session_folders(
    store: &mut Store,
    folders: &[editor::location::ResourceLocation],
) -> ahp_wire::SessionId {
    let host = ahp_wire::SessionId::local_default(store).host;
    ahp_session::session::agents::Agents::seed(store, host, "Test Host");
    ahp_session::session::state::Hosts::install_uris(store, host, std::sync::Arc::new(TestUris));
    static SEEDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let minted = SEEDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let id = ahp_wire::SessionId {
        host,
        session: ahp_wire::client::SessionUri::new(format!("test-session:/{minted}")),
    };
    let uris = ahp_session::session::state::Hosts::uris(store, host).expect("installed above");
    let working_directories: Vec<String> = folders
        .iter()
        .map(|folder| uris.uri_of(folder).into_string())
        .collect();
    ahp_session::session::agents::Agents::set_channel(
        store,
        &id,
        ahp_wire::client::SessionChannel {
            provider: "test".to_owned(),
            chats: rpds::VectorSync::new_sync(),
            default_chat: None,
            working_directories: working_directories.iter().cloned().collect(),
            config: None,
        },
    );

    ahp_session::session::agents::Agents::add_sessions(
        store,
        host,
        vec![ahp_types::state::SessionSummary {
            provider: "test".to_owned(),
            title: String::new(),
            status: 0,
            activity: None,
            project: None,
            origin: None,
            working_directories: Some(working_directories),
            annotations: None,
            resource: id.session.as_str().to_owned(),
            created_at: String::new(),
            modified_at: String::new(),
            changes: None,
            meta: None,
        }],
        false,
    );
    id
}

pub fn add_session_folders(
    store: &mut Store,
    id: &ahp_wire::SessionId,
    folders: &[editor::location::ResourceLocation],
) {
    let uris = ahp_session::session::state::Hosts::uris(store, id.host).expect("a seeded session");
    let mut channel = ahp_session::session::agents::Agents::channel(store, id).expect("a seeded session");
    for folder in folders {
        let uri = uris.uri_of(folder).into_string();
        if !channel.working_directories.iter().any(|held| held == &uri) {
            channel.working_directories.push_back_mut(uri);
        }
    }
    ahp_session::session::agents::Agents::set_channel(store, id, channel);
}

#[allow(dead_code)]
fn pane_height_content(store: &Store, view: EditorIdView) -> Option<f32> {
    Some(
        documents::OpenDocuments::document_ref(store, view.documents(), view.document())?
            .content_height(view.editor()),
    )
}

impl Application {
    pub fn sole_window(&self) -> ::workbench::window::WindowId {
        let windows = self.window_ids();
        assert!(windows.len() <= 1, "multiple windows: tests must name one");
        *windows.first().expect("a window")
    }

    /// The session the sole window is working in — the owner a test
    /// names when it reaches session-addressed state.
    pub fn sole_window_session(&self) -> ahp_wire::SessionId {
        crate::grip::entity_session(
            ::workbench::window::Windows::window_ref(self.store(), self.sole_window())
                .expect("the window entity"),
        )
    }

    /// The sole window's session — the ids a test threads when it
    /// reaches a collection directly.
    pub fn sole_family(&self) -> ahp_session::session::state::SessionState {
        crate::grip::session_state(self.store(), self.sole_window())
            .expect("the sole window's state")
    }

    /// The sole window session's documents collection.
    pub fn sole_documents(&self) -> imba::store::Id<documents::OpenDocuments> {
        self.sole_family().documents()
    }

    fn workbench(&self) -> &::workbench::workbench::Workbench {
        ::workbench::window::Windows::window_ref(self.store(), self.sole_window())
            .expect("the window entity")
            .workbench()
    }

    pub fn viewport_size(&self) -> skia_safe::Size {
        ::workbench::window::Windows::window_ref(self.store(), self.sole_window())
            .expect("the window entity")
            .viewport_size()
    }

    pub fn plugin_modal(&self) -> Option<&dyn hikit::modal::ModalView> {
        ::workbench::window::Windows::window_ref(self.store(), self.sole_window())?.plugin_modal()
    }

    pub fn focused_document_text(&self) -> Option<String> {
        let document = documents::OpenDocuments::document_ref(
            self.store(),
            self.sole_documents(),
            ::workbench::window::Windows::window_ref(self.store(), self.sole_window())?.focused_document_id()?,
        )?;
        let end = document.text().byte_count().min(u32::MAX as usize) as u32;
        Some(document.text().view().substring(0..end))
    }

    pub fn first_pane_heights_vs_fresh(&self) -> (Vec<(u32, f32)>, Vec<(u32, f32)>, String) {
        let mut first = None;
        self.workbench().root.for_each_pane(&mut |panel| {
            if first.is_none() {
                if let Panel::Editor(pane) = panel {
                    first = Some(*pane.content());
                }
            }
        });
        let entity = first.expect("an editor pane");
        let document =
            documents::OpenDocuments::document_ref(self.store(), entity.documents(), entity.document())
                .expect("document");
        let live = document.element_heights(entity.editor());
        let width = document.layout_width(entity.editor());
        let ui = ::editor::test_document::test_ui();
        let mut fresh = editor::editor_view::EditorView::complete(
            document.clone(),
            width,
            self.store(),
            ui,
            ::editor::test_document::test_fonts_collection(),
            &::editor::env::Themes::of(self.store()),
        );
        fresh.reveal_caret(
            document.caret_byte(entity.editor()),
            self.store(),
            ui,
            ::editor::test_document::test_fonts_collection(),
            &::editor::env::Themes::of(self.store()),
        );
        let text = document.text().byte_string(0, document.text().byte_count());
        (live, fresh.element_heights(), text)
    }

    pub fn focused_document_is_header_at(&self, byte: u32) -> bool {
        let Some(document) = ::workbench::window::Windows::window_ref(self.store(), self.sole_window())
            .and_then(|window| window.focused_document_id())
            .and_then(|id| {
                documents::OpenDocuments::document_ref(self.store(), self.sole_documents(), id)
            })
        else {
            return false;
        };
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let marks =
            document
                .markup()
                .marks_inline_hidden_in(byte..byte + 1, &mut inline, &mut hidden);
        marks
            .ids()
            .iter()
            .any(|id| matches!(id, ::editor::theme::StyleId::Header(_)))
    }

    pub fn unconverged_panes(&self) -> Vec<(usize, f32, f32)> {
        let ui = ::editor::test_document::test_ui();
        let mut offenders = Vec::new();
        let mut index = 0usize;
        let store = self.store();
        let mut check = |index: usize, entity: EditorIdView| {
            if let Some(document) =
                documents::OpenDocuments::document_ref(store, entity.documents(), entity.document())
            {
                let (pending, ..) = document.probe_state(entity.editor());
                if pending.is_some() {
                    offenders.push((index, f32::NAN, f32::NAN));
                    return;
                }
                let width = document.layout_width(entity.editor());
                let fonts = ::editor::test_document::test_fonts_collection();
                let theme = ::editor::theme::Theme::embedded();
                let mut reference =
                    editor::editor_view::EditorView::complete(document.clone(), width, store, ui, &fonts, &theme);

                reference.reveal_caret(
                    document.caret_byte(entity.editor()),
                    store,
                    ui,
                    &fonts,
                    &theme,
                );
                let actual = document.content_height(entity.editor());
                let expected = reference.content_height();
                if (actual - expected).abs() > 0.5 {
                    offenders.push((index, actual, expected));
                }
            }
        };
        self.workbench().root.for_each_pane(&mut |panel| {
            match panel {
                Panel::Editor(pane) => check(index, *pane.content()),

                Panel::Plugin(_) => {}
            }
            index += 1;
        });
        offenders
    }

    pub fn first_inlay_probe_point(&self) -> Option<(f32, f32)> {
        let toolbar = ::editor::env::Themes::of(self.store()).ui().toolbar.height;
        let geometry = workbench_geometry(
            self.viewport_size().width,
            (self.viewport_size().height - toolbar).max(1.0),
            &::editor::env::Themes::of(self.store()).ui().window,
        );
        let pane = self.workbench().root.focused_pane().editor()?;
        let entity = *pane.content();
        let document = documents::OpenDocuments::document_ref(
            self.store(),
            entity.documents(),
            entity.document(),
        )?;
        let byte_count = document.text().byte_count().min(u32::MAX as usize) as u32;
        let extras = document.extras_keyed(entity.editor());
        let interval = ::editor::markup::OverlaidMarkup::new(document.markup(), &extras)
            .all_inlays_in(0..byte_count)
            .into_iter()
            .find(|interval| {
                matches!(
                    interval.inlay.mode(),
                    editor::markup::InlayMode::Above | editor::markup::InlayMode::Under
                )
            })?;
        let anchor = interval.range.start;
        let y_in_document = document.height_before(entity.editor(), anchor);

        let ui = ::editor::env::Themes::of(self.store());
        Some((
            geometry.x + ui.ui().window.content_pad + ui.ui().editor_gutter.width + 24.0,
            toolbar + geometry.top + y_in_document - pane.scroll_y() + 20.0,
        ))
    }

    pub fn focused_caret_byte(&self) -> Option<u32> {
        let pane = self.workbench().root.focused_pane().editor()?;
        let entity = pane.content();
        let document = documents::OpenDocuments::document_ref(
            self.store(),
            entity.documents(),
            entity.document(),
        )?;
        Some(document.caret_byte(entity.editor()))
    }

    pub fn focused_reveal_pending(&self) -> Option<bool> {
        let pane = self.workbench().root.focused_pane().editor()?;
        let entity = pane.content();
        let document = documents::OpenDocuments::document_ref(
            self.store(),
            entity.documents(),
            entity.document(),
        )?;
        Some(document.reveal_pending(entity.editor()))
    }

    pub fn focused_pane_scroll_y(&self) -> f32 {
        self.workbench()
            .root
            .focused_pane()
            .editor()
            .map_or(0.0, |pane| pane.scroll_y())
    }

    pub fn focused_pane_content_height(&self) -> f32 {
        let Some(pane) = self.workbench().root.focused_pane().editor() else {
            return 0.0;
        };
        pane_height_content(self.store(), *pane.content()).unwrap_or(0.0)
    }

    pub fn styled_pane_count(&self) -> usize {
        let store = self.store();
        let mut styled = 0;
        self.workbench().root.for_each_pane(&mut |panel| {
            let Some(pane) = panel.editor() else { return };
            let entity = pane.content();
            if let Some(document) =
                documents::OpenDocuments::document_ref(store, entity.documents(), entity.document())
            {
                if document.syntax().is_some() {
                    styled += 1;
                }
            }
        });
        styled
    }

    pub fn pane_widths(&self) -> Vec<f32> {
        let mut widths = Vec::new();
        let store = self.store();
        self.workbench().root.for_each_pane(&mut |panel| {
            if let Some(width) = panel.editor().and_then(|pane| pane_width(store, pane)) {
                widths.push(width);
            }
        });
        widths
    }

    pub fn focused_document_header_at_start(&self) -> Option<Option<u8>> {
        let view = self.workbench().root.focused_pane().editor()?.content();
        let document =
            documents::OpenDocuments::document_ref(self.store(), view.documents(), view.document())?;
        if document.text().view().byte_count() == 0 {
            return None;
        }
        document
            .markup()
            .block_marks_in(0..8)
            .ids()
            .iter()
            .find_map(|id| match *id {
                ::editor::theme::StyleId::Header(level) => Some(Some(level)),
                _ => None,
            })
            .or(Some(None))
    }

    pub fn document_count(&self) -> usize {
        documents::OpenDocuments::list(self.store(), self.sole_documents()).len()
    }

    pub fn pane_count(&self) -> usize {
        let mut count = 0;
        self.workbench().root.for_each_pane(&mut |_| count += 1);
        count
    }

    pub fn for_each_plugin_panel(&self, visit: &mut dyn FnMut(&dyn hikit::panel::DynPanelView)) {
        // The chat slot is a workbench panel too — always open, just
        // not a tree citizen.
        if let Some(chat) = self.workbench().chat() {
            if let Panel::Plugin(view) = chat.panel() {
                visit(view.as_ref());
            }
        }
        self.workbench().root.for_each_pane(&mut |panel| {
            if let Panel::Plugin(view) = panel {
                visit(view.as_ref());
            }
        });
    }

    pub fn focused_editor_id(&self) -> (documents::DocumentId, ::editor::editor::EditorId) {
        let view = self
            .workbench()
            .root
            .focused_pane()
            .editor()
            .expect("the focused panel is an editor")
            .content();
        (view.document(), view.editor())
    }
}

pub fn surviving_launches<R: 'static>(
    batch: imba::effect::Batch<R>,
) -> Vec<imba::effect::AnyEffect<R>> {
    batch.surviving_launches()
}

pub fn handle_effect<R: 'static>(
    effect: imba::effect::AnyEffect<R>,
    workshop: &std::sync::Arc<::editor::env::Workshop>,
) -> R {
    use imba::effect::{block_on, EffectHandler};
    let payload = effect.into_payload();
    let (value, lift) = payload.split();
    let outcome: Box<dyn std::any::Any + Send + Sync> = match value
        .downcast::<::editor::repair::RepairEffect>()
    {
        Ok(effect) => {
            let handler = ::editor::repair::RepairHandler(std::sync::Arc::clone(workshop));
            Box::new(block_on(Box::pin(
                async move { handler.handle(*effect).await },
            )))
        }
        Err(value) => match value.downcast::<::editor::reparse::ReparseEffect>() {
            Ok(effect) => {
                let handler = ::editor::reparse::ReparseHandler(std::sync::Arc::clone(workshop));
                Box::new(block_on(Box::pin(
                    async move { handler.handle(*effect).await },
                )))
            }
            Err(value) => match value.downcast::<::editor::enrich::EnrichEffect>() {
                Ok(effect) => {
                    let handler = ::editor::enrich::EnrichHandler {
                        workshop: std::sync::Arc::clone(workshop),
                        caller: imba::effect::EffectCaller::disconnected(),
                    };
                    Box::new(block_on(Box::pin(
                        async move { handler.handle(*effect).await },
                    )))
                }
                Err(value) => match value.downcast::<::editor::scroll_stripe::ScrollStripeEffect>()
                {
                    Ok(effect) => {
                        let handler = ::editor::scroll_stripe::ScrollStripeHandler(
                            std::sync::Arc::clone(workshop),
                        );
                        Box::new(block_on(Box::pin(
                            async move { handler.handle(*effect).await },
                        )))
                    }
                    Err(value) => match value.downcast::<::editor::split_diff::RepairDiffEffect>() {
                        Ok(effect) => {
                            let handler =
                                ::editor::split_diff::RepairDiffHandler(std::sync::Arc::clone(workshop));
                            Box::new(block_on(Box::pin(
                                async move { handler.handle(*effect).await },
                            )))
                        }
                        Err(value) => match value.downcast::<documents::diffs::DiffNormalizeEffect>() {
                            Ok(effect) => {
                                let handler = documents::diffs::DiffNormalizeHandler;
                                Box::new(block_on(Box::pin(async move {
                                    handler.handle(*effect).await
                                })))
                            }
                            Err(value) => match value.downcast::<crate::app::OpenEffect>() {
                                Ok(effect) => {
                                    let handler =
                                        crate::app::OpenHandler(std::sync::Arc::clone(workshop));
                                    Box::new(block_on(Box::pin(async move {
                                        handler.handle(*effect).await
                                    })))
                                }
                                Err(_) => panic!("handle_effect: unknown effect type"),
                            },
                        },
                    },
                },
            },
        },
    };
    lift(outcome).expect("a notification lands nothing — this harness drives only landing effects")
}

pub fn test_workshop(theme: ::editor::theme::Theme) -> std::sync::Arc<::editor::env::Workshop> {
    std::sync::Arc::new(::editor::env::Workshop::new(
        ::editor::embedded_fonts::source(),
        theme,
    ))
}
